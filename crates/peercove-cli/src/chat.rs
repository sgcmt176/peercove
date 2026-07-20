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

use std::collections::VecDeque;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use peercove_core::ipc::{ChatMessageInfo, MAX_CHAT_BYTES_PER_REPLY, MAX_CHAT_MESSAGES_PER_REPLY};

/// 保持する履歴の上限(通)。超えたら古い方から捨てる(ADR-0015/0016)。
const MAX_HISTORY: usize = 10_000;
/// ファイルの行数がこの値を超えたら詰め直す(追記のたびに書き直さない)。
const REWRITE_THRESHOLD: usize = MAX_HISTORY + 1_000;

pub type SharedChatLog = Arc<Mutex<ChatLog>>;

/// 1 ネットワーク分のチャット履歴(メモリ上の直近 [`MAX_HISTORY`] 通 +
/// 追記ファイル)。
pub struct ChatLog {
    path: PathBuf,
    entries: VecDeque<ChatMessageInfo>,
    /// ファイル内の行数(メモリから溢れた古い行を含む)。詰め直しの判定に使う。
    file_lines: usize,
    next_seq: u64,
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
        }))
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
