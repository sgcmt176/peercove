//! チャット履歴のローカル保存(ADR-0016、M3-13)。
//!
//! `networks/<ネットワーク>.chat.jsonl` に 1 行 1 通で追記する(JSON Lines、
//! status ファイルと同じ配置規則)。履歴は端末ローカルのみ(端末間同期なし)。
//! 上限([`MAX_HISTORY`])を超えたら古い方から捨てる。
//!
//! 書き込みは同期 I/O だが 1 行の追記だけなので、async コンテキストからの
//! 呼び出しを許容する(status ファイルの書き出しと同じ割り切り)。
//!
//! 秘匿ルール: 本文はログへ出さない(seq・id・IP は可)。

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write as _;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;
use peercove_core::ipc::{ChatMessageInfo, MAX_CHAT_BYTES_PER_REPLY, MAX_CHAT_MESSAGES_PER_REPLY};
use peercove_core::msg::ChatScope;
use peercove_core::proto::LedgerEntry;
use serde::{Deserialize, Serialize};

/// 保持する履歴の上限(通)。超えたら古い方から捨てる(ADR-0015/0016)。
const MAX_HISTORY: usize = 10_000;
/// ファイルの行数がこの値を超えたら詰め直す(追記のたびに書き直さない)。
const REWRITE_THRESHOLD: usize = MAX_HISTORY + 1_000;

pub type SharedChatLog = Arc<Mutex<ChatLog>>;

/// 台帳の各メンバーの「同一性」を公開鍵で覚えておくサイドカーの場所。
fn identity_path(config_path: &Path) -> PathBuf {
    config_path.with_extension("chatids.json")
}

/// メンバーの再追加(削除 → 同名・同 IP で再参加)を検知して、その相手との
/// 1:1 履歴を消す。同一性は**公開鍵**で判定する(再追加すると鍵が変わる)。
///
/// `<config>.chatids.json` に `IP → 公開鍵` を保存し、台帳同期のたびに突き合わせる。
/// 初回(サイドカー無し)は現状を記録するだけ(既存履歴は消さない)。
/// `self_ip`(自分)と `is_host` は 1:1 の対象外なので飛ばす。
pub fn reconcile_identities(
    config_path: &Path,
    self_ip: Ipv4Addr,
    ledger: &[LedgerEntry],
    chat: &SharedChatLog,
    queue: &SharedChatQueue,
) -> ReconcileOutcome {
    let path = identity_path(config_path);
    let mut map: HashMap<String, String> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let mut outcome = ReconcileOutcome::default();
    let mut changed = false;
    for entry in ledger {
        if entry.ip == self_ip || entry.is_host {
            continue;
        }
        // 同一性はホストが振る member_id(invite_id)で判定する。公開鍵は
        // 鍵ローテーション(ADR-0020。Android は参加直後に自動実行)でも
        // 変わるため使えない。旧版ホスト等で ID が無ければ検知しない。
        let Some(member_id) = entry.member_id.as_ref() else {
            continue;
        };
        let ip = entry.ip.to_string();
        let id = format!("id:{member_id}");
        match map.get(&ip) {
            Some(prev) if *prev == id => {} // 同一メンバー
            // 旧形式(公開鍵を記録していた v0.1.1)からの移行: 記録し直す
            // だけで消さない(鍵ローテーション誤検知を持ち越さない)
            Some(prev) if !prev.starts_with("id:") => {
                map.insert(ip, id);
                changed = true;
            }
            Some(_) => {
                // ID が変わった = 別のメンバーに置き換わった。1:1 履歴を消す
                if chat.lock().unwrap().clear_direct(entry.ip) {
                    tracing::info!(
                        "{} は別のメンバーに置き換わったため 1:1 履歴を消去しました",
                        entry.ip
                    );
                }
                append_system(
                    chat,
                    ChatScope::Direct,
                    Some(entry.ip),
                    self_ip,
                    format!(
                        "{} は新しい端末として参加しました(以前の履歴と送信待ちを破棄しました)",
                        entry.name.as_deref().unwrap_or(&ip)
                    ),
                );
                outcome.replaced.push(entry.ip);
                map.insert(ip, id);
                changed = true;
            }
            None => {
                map.insert(ip, id);
                changed = true;
            }
        }
    }
    // サイドカーに居るのに台帳から消えた IP = ネットワークから削除された。
    // 記録を落として一度だけお知らせを出す(以後は「未知の IP」扱い)。
    let departed: Vec<Ipv4Addr> = map
        .keys()
        .filter_map(|ip| ip.parse::<Ipv4Addr>().ok())
        .filter(|ip| !ledger.iter().any(|e| e.ip == *ip))
        .collect();
    for ip in &departed {
        map.remove(&ip.to_string());
        changed = true;
        append_system(
            chat,
            ChatScope::Network,
            None,
            self_ip,
            format!("{ip} がネットワークから削除されました"),
        );
        tracing::info!("{ip} が台帳から消えたため送信待ちを破棄します");
    }
    outcome.departed = departed;
    // 置き換わった・居なくなった相手宛の送信待ち(1:1)は破棄する
    // (再追加された別人へ旧メッセージを届けない — 2026-07-20 検証 FB)
    let stale = outcome.all();
    if !stale.is_empty() {
        let pruned = {
            let mut queue = queue.lock().unwrap();
            let before = queue.len();
            queue.retain(|p| {
                !(p.scope == ChatScope::Direct && p.to.is_some_and(|to| stale.contains(&to)))
            });
            queue.len() != before
        };
        if pruned {
            save_queue(config_path, queue);
        }
    }
    if changed {
        if let Ok(json) = serde_json::to_string(&map) {
            let tmp = path.with_extension("chatids.json.tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }
    outcome
}

/// [`reconcile_identities`] の結果: 鍵が変わった(= 別人に置き換わった)IP と
/// 台帳から消えた IP。呼び出し側はグループからの自動除外に使う。
#[derive(Default)]
pub struct ReconcileOutcome {
    pub replaced: Vec<Ipv4Addr>,
    pub departed: Vec<Ipv4Addr>,
}

impl ReconcileOutcome {
    /// グループから外すべき IP(置き換わった + 居なくなった)。
    pub fn all(&self) -> Vec<Ipv4Addr> {
        self.replaced
            .iter()
            .chain(self.departed.iter())
            .copied()
            .collect()
    }
}

/// お知らせ行(system)を履歴へ 1 行足す。
fn append_system(
    chat: &SharedChatLog,
    scope: ChatScope,
    to: Option<Ipv4Addr>,
    self_ip: Ipv4Addr,
    text: String,
) {
    chat.lock().unwrap().append(ChatMessageInfo {
        seq: 0, // append が振る
        id: crate::msg::new_transfer_id(),
        scope,
        group_id: None,
        from: self_ip,
        to,
        text,
        sent_at: crate::msg::now_unix_ms(),
        failed: false,
        file: None,
        system: true,
    });
}

/// チャットの自動再送間隔(E-E 3。モバイルと同値)。
pub const CHAT_RESEND_INTERVAL: Duration = Duration::from_secs(10);

/// 送信待ち(再送キュー)のチャット 1 通(E-E 3 のデスクトップ版)。
/// 送達条件(direct = 宛先本人、network/group = 1 人以上)を満たすまで
/// [`CHAT_RESEND_INTERVAL`] ごとに再送する。同じ ID を使い続けるので
/// 受信側の重複弾き(contains_id)と対で冪等。
pub struct PendingChat {
    pub seq: u64,
    pub id: String,
    pub scope: ChatScope,
    pub to: Option<Ipv4Addr>,
    pub group_id: Option<String>,
    pub text: String,
    pub sent_at: u64,
    /// ack が取れた宛先(network/group の部分送達を重複させない)
    pub delivered: HashSet<Ipv4Addr>,
    pub next_at: Instant,
}

pub type SharedChatQueue = Arc<Mutex<Vec<PendingChat>>>;

/// 送信待ちキューの保存形式(`<config>.chatq.json`)。デーモン再起動を
/// 跨いで自動再送を続けるための永続化。next_at は保存せず即時再送。
#[derive(Serialize, Deserialize)]
struct PersistedChat {
    seq: u64,
    id: String,
    scope: ChatScope,
    #[serde(default)]
    to: Option<Ipv4Addr>,
    #[serde(default)]
    group_id: Option<String>,
    text: String,
    sent_at: u64,
    #[serde(default)]
    delivered: Vec<Ipv4Addr>,
}

/// 送信待ちキューの保存先(`game.toml` → `game.chatq.json`)。
pub fn queue_path_for(config_path: &Path) -> PathBuf {
    config_path.with_extension("chatq.json")
}

/// 保存済みの送信待ちキューを読む(壊れていたら空 = 諦めて捨てる)。
pub fn load_queue(config_path: &Path) -> SharedChatQueue {
    let queue = std::fs::read_to_string(queue_path_for(config_path))
        .ok()
        .and_then(|data| serde_json::from_str::<Vec<PersistedChat>>(&data).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|p| PendingChat {
            seq: p.seq,
            id: p.id,
            scope: p.scope,
            to: p.to,
            group_id: p.group_id,
            text: p.text,
            sent_at: p.sent_at,
            delivered: p.delivered.into_iter().collect(),
            next_at: Instant::now(),
        })
        .collect();
    Arc::new(Mutex::new(queue))
}

/// 送信待ちキューをディスクへ反映する(空になったらファイルを消す)。
/// 件数は高々数十なので毎回全量書き直しで足りる。
pub fn save_queue(config_path: &Path, queue: &SharedChatQueue) {
    let path = queue_path_for(config_path);
    let snapshot: Vec<PersistedChat> = queue
        .lock()
        .unwrap()
        .iter()
        .map(|p| PersistedChat {
            seq: p.seq,
            id: p.id.clone(),
            scope: p.scope,
            to: p.to,
            group_id: p.group_id.clone(),
            text: p.text.clone(),
            sent_at: p.sent_at,
            delivered: p.delivered.iter().copied().collect(),
        })
        .collect();
    if snapshot.is_empty() {
        let _ = std::fs::remove_file(&path);
        return;
    }
    let Ok(json) = serde_json::to_string(&snapshot) else {
        return;
    };
    let tmp = path.with_extension("chatq.json.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// 1 ネットワーク分のチャット履歴(メモリ上の直近 [`MAX_HISTORY`] 通 +
/// 追記ファイル)。
pub struct ChatLog {
    path: PathBuf,
    entries: VecDeque<ChatMessageInfo>,
    /// ファイル内の行数(メモリから溢れた古い行を含む)。詰め直しの判定に使う。
    file_lines: usize,
    next_seq: u64,
    /// 履歴を消した(clear_direct)たびに増える世代番号。UI が変化を見て、
    /// 手元の履歴を捨てて取り直す(seq は据え置きなので seq では検知できない)。
    generation: u64,
}

impl ChatLog {
    /// 履歴ファイルの場所(`game.toml` → `game.chat.jsonl`)。
    pub fn path_for(config_path: &Path) -> PathBuf {
        config_path.with_extension("chat.jsonl")
    }

    /// 履歴を読み込んで共有ハンドルにする(ファイルが無ければ空。壊れた行は
    /// 読み飛ばす)。seq はファイルの続きから振る(再起動で重複させない)。
    pub fn load(config_path: &Path) -> SharedChatLog {
        let path = Self::path_for(config_path);
        let mut entries: VecDeque<ChatMessageInfo> = VecDeque::new();
        let mut file_lines = 0usize;
        if let Ok(content) = std::fs::read_to_string(&path) {
            for line in content.lines() {
                file_lines += 1;
                if let Ok(entry) = serde_json::from_str::<ChatMessageInfo>(line) {
                    entries.push_back(entry);
                }
            }
        }
        while entries.len() > MAX_HISTORY {
            entries.pop_front();
        }
        let next_seq = entries.back().map(|e| e.seq + 1).unwrap_or(1);
        Arc::new(Mutex::new(Self {
            path,
            entries,
            file_lines,
            next_seq,
            generation: 0,
        }))
    }

    /// 履歴の消去世代(UI の取り直し判定用)。status 応答に載せる。
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// 1 通追記する(seq はここで振る)。ファイルへの追記に失敗しても
    /// メモリ上の履歴には残す(警告ログのみ)。
    pub fn append(&mut self, mut entry: ChatMessageInfo) -> ChatMessageInfo {
        entry.seq = self.next_seq;
        self.next_seq += 1;
        self.entries.push_back(entry.clone());
        if self.entries.len() > MAX_HISTORY {
            self.entries.pop_front();
        }
        if let Err(e) = self.write_entry(&entry) {
            tracing::warn!("チャット履歴の書き出しに失敗しました: {e:#}");
        }
        entry
    }

    fn write_entry(&mut self, entry: &ChatMessageInfo) -> anyhow::Result<()> {
        if self.file_lines >= REWRITE_THRESHOLD {
            return self.rewrite();
        }
        // 失敗フラグは揮発(ADR-0016)なのでファイルには書かない
        let mut persisted = entry.clone();
        persisted.failed = false;
        let mut line = serde_json::to_string(&persisted).context("履歴の直列化に失敗しました")?;
        line.push('\n');
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .with_context(|| format!("{} を開けません", self.path.display()))?;
        file.write_all(line.as_bytes())
            .with_context(|| format!("{} へ追記できません", self.path.display()))?;
        self.file_lines += 1;
        Ok(())
    }

    /// メモリ上の履歴(= 直近 [`MAX_HISTORY`] 通)でファイルを詰め直す。
    fn rewrite(&mut self) -> anyhow::Result<()> {
        let mut buf = String::new();
        for entry in &self.entries {
            let mut persisted = entry.clone();
            persisted.failed = false;
            buf.push_str(&serde_json::to_string(&persisted).context("履歴の直列化に失敗しました")?);
            buf.push('\n');
        }
        let tmp = self.path.with_extension("chat.jsonl.tmp");
        std::fs::write(&tmp, buf).with_context(|| format!("{} へ書けません", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("{} の置き換えに失敗しました", self.path.display()))?;
        self.file_lines = self.entries.len();
        Ok(())
    }

    /// 送信失敗の印を付ける(メモリ上のみ — 再起動で消える)。
    pub fn mark_failed(&mut self, seq: u64) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.seq == seq) {
            entry.failed = true;
        }
    }

    /// 送信失敗の印を消す(自動再送で届いたとき)。
    pub fn clear_failed(&mut self, seq: u64) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.seq == seq) {
            entry.failed = false;
        }
    }

    /// seq のエントリを返す(手動再送がキューへ積み直すのに使う)。
    pub fn get(&self, seq: u64) -> Option<ChatMessageInfo> {
        self.entries.iter().find(|e| e.seq == seq).cloned()
    }

    /// 指定 IP との 1:1(direct)履歴を消す。メンバーを削除 → 同名・同 IP で
    /// 再追加すると別人(別の鍵)になるため、旧履歴が混ざらないよう消す。
    /// 件数が変わったら true。seq は据え置き(以後の新着は続きの seq)。
    pub fn clear_direct(&mut self, ip: Ipv4Addr) -> bool {
        let before = self.entries.len();
        self.entries
            .retain(|e| !(e.scope == ChatScope::Direct && (e.from == ip || e.to == Some(ip))));
        if self.entries.len() == before {
            return false;
        }
        self.generation += 1;
        if let Err(e) = self.rewrite() {
            tracing::warn!("履歴の詰め直しに失敗しました: {e:#}");
        }
        true
    }

    /// メッセージ ID が既に履歴にあるか。モバイルの再送キュー(E-E 3)は
    /// ack を取り損ねると同じ ID で再送してくるため、受信側で重複を弾く。
    pub fn contains_id(&self, id: &str) -> bool {
        self.entries.iter().any(|e| e.id == id)
    }

    /// `after_seq` より後のエントリを返す(IPC の 1 応答に収まるよう件数と
    /// 本文バイトで打ち切る)。戻りは `(履歴全体の最新 seq, エントリ)`。
    /// エントリの末尾 seq が最新 seq に達するまで繰り返し呼ぶ。
    pub fn fetch(&self, after_seq: u64) -> (u64, Vec<ChatMessageInfo>) {
        let mut messages = Vec::new();
        let mut bytes = 0usize;
        for entry in self.entries.iter().filter(|e| e.seq > after_seq) {
            bytes += entry.text.len() + 256; // 本文 + フィールドのおおよその上乗せ
            if !messages.is_empty()
                && (messages.len() >= MAX_CHAT_MESSAGES_PER_REPLY
                    || bytes > MAX_CHAT_BYTES_PER_REPLY)
            {
                break;
            }
            messages.push(entry.clone());
        }
        (self.latest_seq(), messages)
    }

    /// 履歴全体の最新 seq(0 = 履歴なし)。status 応答の `chat_seq` に載せる。
    pub fn latest_seq(&self) -> u64 {
        self.next_seq - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peercove_core::msg::ChatScope;

    fn temp_config(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "peercove-chat-{label}-{}-{}",
            std::process::id(),
            crate::msg::new_transfer_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("net.toml")
    }

    fn entry(text: &str) -> ChatMessageInfo {
        ChatMessageInfo {
            seq: 0, // append が振り直す
            id: "c".to_string(),
            scope: ChatScope::Direct,
            group_id: None,
            from: "10.0.0.1".parse().unwrap(),
            to: Some("10.0.0.2".parse().unwrap()),
            text: text.to_string(),
            sent_at: 1_700_000_000_000,
            failed: false,
            file: None,
            system: false,
        }
    }

    fn ledger_entry(ip: &str, member_id: &str) -> LedgerEntry {
        use peercove_core::keys::PrivateKey;
        LedgerEntry {
            name: Some("m".to_string()),
            dns_name: None,
            ip: ip.parse().unwrap(),
            // 鍵は毎回変わってもよい(同一性判定に使わないことのテストを兼ねる)
            public_key: PrivateKey::generate().public_key(),
            app_version: None,
            platform: None,
            capabilities: vec![],
            member_id: Some(member_id.to_string()),
            invite_status: None,
            invite_expires_at: None,
            online: true,
            is_host: false,
            endpoint: None,
            endpoint_age_secs: None,
            subnets: vec![],
            blocked: false,
            force_relay: false,
            acl_rule_id: None,
        }
    }

    /// clear_direct はその IP との 1:1 だけ消し、seq は続きから振る。
    #[test]
    fn clear_direct_removes_only_that_peer() {
        let config = temp_config("cleardirect");
        let log = ChatLog::load(&config);
        let mut log = log.lock().unwrap();
        // 10.0.0.2 との 1:1(entry の from/to)を 2 通
        log.append(entry("a"));
        log.append(entry("b"));
        // 別の相手 10.0.0.9 との 1:1 を 1 通
        log.append(ChatMessageInfo {
            to: Some("10.0.0.9".parse().unwrap()),
            ..entry("keep")
        });
        assert_eq!(log.latest_seq(), 3);
        assert!(log.clear_direct("10.0.0.2".parse().unwrap()));
        let (seq, messages) = log.fetch(0);
        assert_eq!(seq, 3, "seq は据え置き");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text, "keep");
        // 新着は続きの seq
        assert_eq!(log.append(entry("next")).seq, 4);
        drop(log);

        // reconcile: member_id が変わったら(= 再追加)1:1 を消す。初回は
        // 消さない。鍵の変化(ローテーション)だけでは何もしない
        let log = ChatLog::load(&config);
        let self_ip = "10.0.0.1".parse().unwrap();
        let peer: Ipv4Addr = "10.0.0.2".parse().unwrap();
        let pending = |to: &str| PendingChat {
            seq: 1,
            id: format!("p-{to}"),
            scope: ChatScope::Direct,
            to: Some(to.parse().unwrap()),
            group_id: None,
            text: "たまっていた".to_string(),
            sent_at: 0,
            delivered: HashSet::new(),
            next_at: Instant::now(),
        };
        let queue: SharedChatQueue = Arc::new(Mutex::new(vec![pending("10.0.0.2")]));
        // 初回: 記録するだけ(履歴・キューは残る)
        let outcome = reconcile_identities(
            &config,
            self_ip,
            &[ledger_entry("10.0.0.2", "inv-1")],
            &log,
            &queue,
        );
        assert!(outcome.all().is_empty());
        assert!(!log.lock().unwrap().fetch(0).1.is_empty());
        assert_eq!(queue.lock().unwrap().len(), 1);
        // 同じ member_id(鍵はローテーションで変わっている)= 何もしない
        let outcome = reconcile_identities(
            &config,
            self_ip,
            &[ledger_entry("10.0.0.2", "inv-1")],
            &log,
            &queue,
        );
        assert!(outcome.all().is_empty(), "鍵ローテーションでは消さない");
        assert_eq!(queue.lock().unwrap().len(), 1);
        // member_id が変わる = 再追加。10.0.0.2 との 1:1 が消え、送信待ちも破棄
        let outcome = reconcile_identities(
            &config,
            self_ip,
            &[ledger_entry("10.0.0.2", "inv-2")],
            &log,
            &queue,
        );
        assert_eq!(outcome.replaced, vec![peer]);
        let (_, after) = log.lock().unwrap().fetch(0);
        assert!(
            after
                .iter()
                .filter(|m| !m.system)
                .all(|m| m.to != Some(peer)),
            "再追加で 1:1 が消える: {after:?}"
        );
        assert!(
            queue.lock().unwrap().is_empty(),
            "再追加でその相手宛の送信待ちを破棄する"
        );
        // 台帳から消えた = ネットワークから削除。送信待ちを破棄しお知らせが載る
        queue.lock().unwrap().push(pending("10.0.0.2"));
        let outcome = reconcile_identities(&config, self_ip, &[], &log, &queue);
        assert_eq!(outcome.departed, vec![peer]);
        assert!(queue.lock().unwrap().is_empty(), "削除でも送信待ちを破棄");
        let (_, after) = log.lock().unwrap().fetch(0);
        assert!(
            after
                .iter()
                .any(|m| m.system && m.text.contains("ネットワークから削除")),
            "削除のお知らせが載る: {after:?}"
        );
        // もう一度回しても何も起きない(記録は落ちている)
        let outcome = reconcile_identities(&config, self_ip, &[], &log, &queue);
        assert!(outcome.all().is_empty(), "検知は一度きり");
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    /// 追記 → 再読込で seq が続きから振られ、内容も残る。
    #[test]
    fn append_and_reload_continues_seq() {
        let config = temp_config("reload");
        {
            let log = ChatLog::load(&config);
            let mut log = log.lock().unwrap();
            assert_eq!(log.latest_seq(), 0);
            assert_eq!(log.append(entry("一通目")).seq, 1);
            assert_eq!(log.append(entry("二通目")).seq, 2);
        }
        let log = ChatLog::load(&config);
        let mut log = log.lock().unwrap();
        assert_eq!(log.latest_seq(), 2);
        let (seq, messages) = log.fetch(0);
        assert_eq!(seq, 2);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].text, "一通目");
        assert_eq!(log.append(entry("三通目")).seq, 3, "seq は続きから");
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    /// 壊れた行は読み飛ばす(履歴ファイルの手編集・部分書き込み対策)。
    #[test]
    fn load_skips_corrupt_lines() {
        let config = temp_config("corrupt");
        let path = ChatLog::path_for(&config);
        let good = serde_json::to_string(&ChatMessageInfo {
            seq: 5,
            ..entry("残る")
        })
        .unwrap();
        std::fs::write(&path, format!("{{壊れた行\n{good}\nもう一つ壊れた行\n")).unwrap();
        let log = ChatLog::load(&config);
        let log = log.lock().unwrap();
        let (seq, messages) = log.fetch(0);
        assert_eq!(seq, 5);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text, "残る");
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    /// fetch は after_seq より後だけを、件数・バイト上限で打ち切って返す。
    #[test]
    fn fetch_pages_by_count_and_bytes() {
        let config = temp_config("page");
        let log = ChatLog::load(&config);
        let mut log = log.lock().unwrap();
        for i in 0..(MAX_CHAT_MESSAGES_PER_REPLY + 10) {
            log.append(entry(&format!("m{i}")));
        }
        let (seq, first) = log.fetch(0);
        assert_eq!(seq, (MAX_CHAT_MESSAGES_PER_REPLY + 10) as u64);
        assert_eq!(
            first.len(),
            MAX_CHAT_MESSAGES_PER_REPLY,
            "件数上限で打ち切り"
        );
        let (_, rest) = log.fetch(first.last().unwrap().seq);
        assert_eq!(rest.len(), 10, "続きから取り切れる");
        assert!(log.fetch(seq).1.is_empty(), "最新まで読んだら空");

        // バイト上限: 上限いっぱいの本文(約 8 KB)は 1 応答に十数通しか
        // 載らない(128 KB ÷ 8 KB。ただし必ず 1 通は返す)
        let big = "あ".repeat(peercove_core::msg::MAX_CHAT_TEXT_BYTES / 3);
        let start = log.latest_seq();
        for _ in 0..20 {
            log.append(entry(&big));
        }
        let (_, page) = log.fetch(start);
        assert!(
            !page.is_empty() && page.len() < 20,
            "バイト上限で打ち切り: {}",
            page.len()
        );
        let (_, rest) = log.fetch(page.last().unwrap().seq);
        assert_eq!(page.len() + rest.len(), 20, "続きから取り切れる");
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    /// 上限超過で古い方から消え、ファイルも詰め直される。
    #[test]
    fn history_is_capped_and_file_rewritten() {
        let config = temp_config("cap");
        let log = ChatLog::load(&config);
        let mut log = log.lock().unwrap();
        for i in 0..(REWRITE_THRESHOLD + 1) {
            log.append(entry(&format!("m{i}")));
        }
        assert_eq!(log.entries.len(), MAX_HISTORY, "メモリは上限まで");
        assert_eq!(
            log.entries.front().unwrap().seq,
            (REWRITE_THRESHOLD + 1 - MAX_HISTORY) as u64 + 1
        );
        assert!(
            log.file_lines <= MAX_HISTORY + 1,
            "ファイルは詰め直し済み: {}",
            log.file_lines
        );
        // 詰め直し後も読み込める
        drop(log);
        let reloaded = ChatLog::load(&config);
        let reloaded = reloaded.lock().unwrap();
        assert_eq!(reloaded.latest_seq(), (REWRITE_THRESHOLD + 1) as u64);
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    /// 送信失敗の印はメモリ上のみ(再読込で消える)。
    #[test]
    fn mark_failed_is_volatile() {
        let config = temp_config("failed");
        {
            let log = ChatLog::load(&config);
            let mut log = log.lock().unwrap();
            let appended = log.append(entry("届かない"));
            log.mark_failed(appended.seq);
            assert!(log.fetch(0).1[0].failed);
        }
        let log = ChatLog::load(&config);
        let log = log.lock().unwrap();
        assert!(!log.fetch(0).1[0].failed, "失敗フラグは揮発");
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }
}
