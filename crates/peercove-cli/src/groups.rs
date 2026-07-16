//! グループ情報のローカル保存(ADR-0016、M3-13c)。
//!
//! `networks/<ネットワーク>.groups.json` に既知のグループ全量を JSON で持つ
//! (status ファイルと同じ配置規則)。共有状態はサーバーに置かず、
//! `group_update` フレームでメンバー同士が配り合う。整合性は
//! **最新リビジョン勝ち**([`GroupStore::apply`])。
//!
//! 自分が抜けた・外されたグループも消さずに残す(UI が履歴の表示名に使う。
//! また、より新しい update を再受信したときに古い版へ巻き戻さないため)。
//!
//! 秘匿ルール: グループ名はチャット本文と同様にログへ出さない(id・IP は可)。

use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context;
use peercove_core::msg::GroupInfo;

pub type SharedGroups = Arc<Mutex<GroupStore>>;

/// [`GroupStore::apply`] の結果: 取り込んだ場合の置換前の値
/// (新規なら `None`)。システムメッセージの差分生成に使う。
pub struct AppliedGroup {
    pub previous: Option<GroupInfo>,
}

/// 1 ネットワーク分の既知グループ(メモリ + JSON ファイル)。
///
/// あわせて**送達管理**(メモリのみ)を持つ: グループごと・相手ごとに
/// ack 済みの revision を覚え、supervise が未達分を定期的に送り直す。
/// オンライン判定は最終ハンドシェイクの猶予(180 秒)があり、短時間の
/// オフライン→オンラインは「遷移」として見えないため、遷移検知ではなく
/// **ack が取れるまで再送**する方式にしている(2026-07-11 検証フィードバック)。
pub struct GroupStore {
    path: PathBuf,
    groups: HashMap<String, GroupInfo>,
    /// (グループ ID, 相手 IP) → ack 済み revision。デーモン再起動で消える
    /// (= 再起動後に一度ずつ送り直すだけ。受信側の apply は冪等)
    acked: HashMap<(String, Ipv4Addr), u64>,
    /// (グループ ID, 相手 IP) → 直近の送信試行。失敗の連打を防ぐ
    last_attempt: HashMap<(String, Ipv4Addr), Instant>,
}

impl GroupStore {
    /// 保存先(`game.toml` → `game.groups.json`)。
    pub fn path_for(config_path: &Path) -> PathBuf {
        config_path.with_extension("groups.json")
    }

    /// 読み込んで共有ハンドルにする(ファイルが無ければ空。壊れていたら
    /// 警告して空から始める — グループは伝搬で再取得できる)。
    pub fn load(config_path: &Path) -> SharedGroups {
        let path = Self::path_for(config_path);
        let groups = match std::fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str::<Vec<GroupInfo>>(&content) {
                Ok(list) => list.into_iter().map(|g| (g.id.clone(), g)).collect(),
                Err(e) => {
                    tracing::warn!("グループ情報の読み込みに失敗しました(空から再開): {e}");
                    HashMap::new()
                }
            },
            Err(_) => HashMap::new(),
        };
        Arc::new(Mutex::new(Self {
            path,
            groups,
            acked: HashMap::new(),
            last_attempt: HashMap::new(),
        }))
    }

    /// ネットワークから受信した `group_update` を取り込んでよいか(認可)。
    ///
    /// グループ情報は署名を持たず P2P で配り合うため、`apply` の revision 勝負だけ
    /// だと**招待済みの悪意あるメンバーが、自分が属さないグループを改名・追放・
    /// 自分を勝手に追加**できてしまう(`updated_by` も詐称可能)。そこで受信時に
    /// 「送信元がそのグループのメンバーか」を検査する:
    /// - 既知グループの変更 → 送信元が**現在の**メンバーであること
    ///   (中継はメンバーが行うため gossip を壊さない。非メンバーの改竄を弾く)
    /// - 未知グループ(新規)→ 送信元が**その**グループのメンバーであること
    ///   (見ず知らずの第三者が勝手なグループを作って見せるのを弾く)
    ///
    /// 送信元 IP は WG のトンネル内送信元(AllowedIPs /32)なので詐称できない。
    /// 自分で作った更新(ローカル操作)はこの検査を通さず直接 `apply` する。
    pub fn accepts_update(&self, group: &GroupInfo, sender: Ipv4Addr) -> bool {
        match self.groups.get(&group.id) {
            Some(current) => current.members.contains(&sender),
            None => group.members.contains(&sender),
        }
    }

    /// 受信した(または自分で作った)グループ全量を取り込む。
    /// **最新リビジョン勝ち**: 手元より revision が大きければ置換、同値は
    /// updated_by の IP が大きい方が勝つ(決定的にどちらかへ収束させる)。
    /// 取り込んだら置換前の値(ファイルへも保存する)。古ければ `None`。
    pub fn apply(&mut self, group: GroupInfo) -> Option<AppliedGroup> {
        let newer = match self.groups.get(&group.id) {
            None => true,
            Some(current) => {
                group.revision > current.revision
                    || (group.revision == current.revision && group.updated_by > current.updated_by)
            }
        };
        if !newer {
            return None;
        }
        let previous = self.groups.insert(group.id.clone(), group);
        if let Err(e) = self.save() {
            tracing::warn!("グループ情報の保存に失敗しました: {e:#}");
        }
        Some(AppliedGroup { previous })
    }

    /// 相手がこの revision まで持っていると分かった(ack を受けた、または
    /// 相手からその revision の update を受信した)。
    pub fn mark_acked(&mut self, id: &str, peer: Ipv4Addr, revision: u64) {
        let entry = self.acked.entry((id.to_string(), peer)).or_insert(0);
        if revision > *entry {
            *entry = revision;
        }
    }

    /// いま送るべき (相手, グループ全量) の一覧。オンラインのグループメンバーの
    /// うち、最新 revision の ack が無く、直近 `retry_after` 以内に試行して
    /// いない相手を返す(返した分は試行済みとして記録する)。
    pub fn pending_sync(
        &mut self,
        self_ip: Ipv4Addr,
        online: &HashSet<Ipv4Addr>,
        retry_after: Duration,
    ) -> Vec<(Ipv4Addr, GroupInfo)> {
        let now = Instant::now();
        let mut out = Vec::new();
        for group in self.groups.values() {
            for peer in &group.members {
                if *peer == self_ip || !online.contains(peer) {
                    continue;
                }
                let key = (group.id.clone(), *peer);
                if self.acked.get(&key).copied().unwrap_or(0) >= group.revision {
                    continue;
                }
                if self
                    .last_attempt
                    .get(&key)
                    .is_some_and(|at| now.duration_since(*at) < retry_after)
                {
                    continue;
                }
                out.push((*peer, group.clone()));
            }
        }
        for (peer, group) in &out {
            self.last_attempt.insert((group.id.clone(), *peer), now);
        }
        out
    }

    fn save(&self) -> anyhow::Result<()> {
        let mut list: Vec<&GroupInfo> = self.groups.values().collect();
        list.sort_by(|a, b| a.id.cmp(&b.id));
        let json = serde_json::to_string_pretty(&list).context("グループの直列化に失敗しました")?;
        let tmp = self.path.with_extension("groups.json.tmp");
        std::fs::write(&tmp, json).with_context(|| format!("{} へ書けません", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("{} の置き換えに失敗しました", self.path.display()))?;
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<GroupInfo> {
        self.groups.get(id).cloned()
    }

    /// 既知のグループ全部(id 順 — status 応答の表示が揺れないように)。
    pub fn list(&self) -> Vec<GroupInfo> {
        let mut list: Vec<GroupInfo> = self.groups.values().cloned().collect();
        list.sort_by(|a, b| a.id.cmp(&b.id));
        list
    }
}

/// グループ更新の差分から、会話に出すシステムメッセージ本文を作る
/// (LINE 風 — 2026-07-11 検証フィードバック)。`old` は置換前(新規なら
/// `None`)、`name_of` は仮想 IP → 表示名の解決。
pub fn system_messages(
    old: Option<&GroupInfo>,
    new: &GroupInfo,
    self_ip: Ipv4Addr,
    name_of: &dyn Fn(Ipv4Addr) -> String,
) -> Vec<String> {
    let mut out = Vec::new();
    let Some(old) = old else {
        // 初めて知ったグループ: 自分が作った / 追加された
        if new.updated_by == self_ip {
            out.push(format!("グループ「{}」を作成しました", new.name));
        } else if new.members.contains(&self_ip) {
            out.push(format!(
                "{}があなたをグループ「{}」に追加しました",
                name_of(new.updated_by),
                new.name
            ));
        }
        return out;
    };
    if old.name != new.name {
        out.push(format!(
            "グループ名が「{}」から「{}」に変わりました",
            old.name, new.name
        ));
    }
    let added: Vec<Ipv4Addr> = new
        .members
        .iter()
        .filter(|ip| !old.members.contains(ip))
        .copied()
        .collect();
    if added.contains(&self_ip) {
        out.push(format!(
            "{}があなたをグループ「{}」に追加しました",
            name_of(new.updated_by),
            new.name
        ));
    }
    let added_names: Vec<String> = added
        .iter()
        .filter(|ip| **ip != self_ip)
        .map(|ip| name_of(*ip))
        .collect();
    if !added_names.is_empty() {
        out.push(format!(
            "{}が{}を追加しました",
            name_of(new.updated_by),
            added_names.join("、")
        ));
    }
    let removed: Vec<Ipv4Addr> = old
        .members
        .iter()
        .filter(|ip| !new.members.contains(ip))
        .copied()
        .collect();
    if removed.contains(&self_ip) {
        out.push("グループから退出しました".to_string());
    }
    let removed_names: Vec<String> = removed
        .iter()
        .filter(|ip| **ip != self_ip)
        .map(|ip| name_of(*ip))
        .collect();
    if !removed_names.is_empty() {
        out.push(format!(
            "{}がグループから退出しました",
            removed_names.join("、")
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "peercove-groups-{label}-{}-{}",
            std::process::id(),
            crate::msg::new_transfer_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("net.toml")
    }

    fn group(id: &str, revision: u64, updated_by: &str, members: &[&str]) -> GroupInfo {
        GroupInfo {
            id: id.to_string(),
            name: format!("グループ{id}"),
            members: members.iter().map(|ip| ip.parse().unwrap()).collect(),
            revision,
            updated_by: updated_by.parse().unwrap(),
        }
    }

    /// 取り込み → 再読込で残る(永続化)。apply は置換前の値を返す。
    #[test]
    fn apply_and_reload() {
        let config = temp_config("reload");
        {
            let store = GroupStore::load(&config);
            let mut store = store.lock().unwrap();
            let applied = store
                .apply(group("g1", 1, "10.0.0.1", &["10.0.0.1", "10.0.0.2"]))
                .expect("新規は取り込む");
            assert!(applied.previous.is_none(), "新規なので置換前なし");
            let applied = store
                .apply(group("g1", 2, "10.0.0.1", &["10.0.0.1"]))
                .expect("新しい revision は取り込む");
            assert_eq!(applied.previous.unwrap().revision, 1, "置換前が返る");
        }
        let store = GroupStore::load(&config);
        let store = store.lock().unwrap();
        let listed = store.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "g1");
        assert_eq!(listed[0].revision, 2);
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    /// 最新リビジョン勝ち: 古い revision は捨て、同値は updated_by で決着。
    #[test]
    fn newest_revision_wins() {
        let config = temp_config("rev");
        let store = GroupStore::load(&config);
        let mut store = store.lock().unwrap();
        assert!(store
            .apply(group("g1", 2, "10.0.0.1", &["10.0.0.1"]))
            .is_some());
        assert!(
            store
                .apply(group("g1", 1, "10.0.0.9", &["10.0.0.9"]))
                .is_none(),
            "古い revision は捨てる"
        );
        assert_eq!(store.get("g1").unwrap().updated_by.octets()[3], 1);
        assert!(
            store
                .apply(group("g1", 2, "10.0.0.5", &["10.0.0.5"]))
                .is_some(),
            "同値は updated_by の大きい方が勝つ"
        );
        assert!(
            store
                .apply(group("g1", 2, "10.0.0.3", &["10.0.0.3"]))
                .is_none(),
            "同値で小さい方は捨てる"
        );
        assert!(store
            .apply(group("g1", 3, "10.0.0.2", &["10.0.0.2"]))
            .is_some());
        assert_eq!(store.get("g1").unwrap().revision, 3);
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    /// 認可(accepts_update): 受信更新は送信元がメンバーの場合のみ受理する。
    #[test]
    fn accepts_update_requires_membership() {
        let config = temp_config("authz");
        let store = GroupStore::load(&config);
        let mut store = store.lock().unwrap();
        let g = group("g1", 1, "10.0.0.1", &["10.0.0.1", "10.0.0.2"]);

        // 新規グループ: 送信元がそのグループのメンバーなら受理、非メンバーは拒否。
        assert!(
            store.accepts_update(&g, "10.0.0.2".parse().unwrap()),
            "新規でも送信元がメンバーなら受理"
        );
        assert!(
            !store.accepts_update(&g, "10.0.0.9".parse().unwrap()),
            "新規で送信元が非メンバーなら拒否(第三者の勝手なグループを弾く)"
        );

        store.apply(g).unwrap();

        // 既知グループの変更: 現メンバーからのみ受理。非メンバーが自分を
        // 追加しようとする更新(members に自分を入れる)も拒否する。
        let renamed = group("g1", 2, "10.0.0.1", &["10.0.0.1", "10.0.0.2"]);
        assert!(
            store.accepts_update(&renamed, "10.0.0.1".parse().unwrap()),
            "現メンバーからの変更は受理"
        );
        let self_add = group("g1", 2, "10.0.0.9", &["10.0.0.1", "10.0.0.2", "10.0.0.9"]);
        assert!(
            !store.accepts_update(&self_add, "10.0.0.9".parse().unwrap()),
            "非メンバーが自分を追加する更新は拒否(現メンバーで判定するため)"
        );
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    /// 壊れたファイルは空から再開(伝搬で再取得できる)。
    #[test]
    fn corrupt_file_starts_empty() {
        let config = temp_config("corrupt");
        std::fs::write(GroupStore::path_for(&config), "{壊れた JSON").unwrap();
        let store = GroupStore::load(&config);
        assert!(store.lock().unwrap().list().is_empty());
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    /// 送達管理: ack が取れるまで再送対象になり、ack 後は出てこない。
    /// オフラインの相手・自分自身は対象外。再試行には間隔がある。
    #[test]
    fn pending_sync_until_acked() {
        let config = temp_config("sync");
        let me: Ipv4Addr = "10.0.0.1".parse().unwrap();
        let peer: Ipv4Addr = "10.0.0.2".parse().unwrap();
        let offline: Ipv4Addr = "10.0.0.3".parse().unwrap();
        let store = GroupStore::load(&config);
        let mut store = store.lock().unwrap();
        store.apply(group(
            "g1",
            1,
            "10.0.0.1",
            &["10.0.0.1", "10.0.0.2", "10.0.0.3"],
        ));

        let online: HashSet<Ipv4Addr> = [peer].into_iter().collect();
        let pending = store.pending_sync(me, &online, Duration::from_secs(30));
        assert_eq!(pending.len(), 1, "オンラインの未達メンバーだけ");
        assert_eq!(pending[0].0, peer);

        // 直後の再問い合わせは間隔内なので出てこない(失敗の連打防止)
        assert!(store
            .pending_sync(me, &online, Duration::from_secs(30))
            .is_empty());
        // 間隔ゼロなら再試行として出てくる(まだ ack が無いため)
        assert_eq!(store.pending_sync(me, &online, Duration::ZERO).len(), 1);

        // ack が付いたら出てこない
        store.mark_acked("g1", peer, 1);
        assert!(store.pending_sync(me, &online, Duration::ZERO).is_empty());
        // revision が進んだらまた対象になる
        store.apply(group(
            "g1",
            2,
            "10.0.0.1",
            &["10.0.0.1", "10.0.0.2", "10.0.0.3"],
        ));
        assert_eq!(store.pending_sync(me, &online, Duration::ZERO).len(), 1);

        // オフラインだったメンバーがオンラインになれば対象になる(追いつき)
        let online: HashSet<Ipv4Addr> = [peer, offline].into_iter().collect();
        store.mark_acked("g1", peer, 2);
        let pending = store.pending_sync(me, &online, Duration::ZERO);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].0, offline);
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    /// システムメッセージの差分生成(LINE 風)。
    #[test]
    fn system_messages_from_diff() {
        let me: Ipv4Addr = "10.0.0.2".parse().unwrap();
        let name_of = |ip: Ipv4Addr| -> String {
            match ip.octets()[3] {
                1 => "alice".to_string(),
                2 => "自分".to_string(),
                3 => "carol".to_string(),
                _ => ip.to_string(),
            }
        };
        // 自分が作成
        let created = group("g1", 1, "10.0.0.2", &["10.0.0.2", "10.0.0.1"]);
        assert_eq!(
            system_messages(None, &created, me, &name_of),
            vec!["グループ「グループg1」を作成しました"]
        );
        // 初受信(自分がメンバー)= 追加された
        let received = group("g1", 1, "10.0.0.1", &["10.0.0.1", "10.0.0.2"]);
        assert_eq!(
            system_messages(None, &received, me, &name_of),
            vec!["aliceがあなたをグループ「グループg1」に追加しました"]
        );
        // 改名は変更前後の両方を出す
        let old = group("g1", 1, "10.0.0.1", &["10.0.0.1", "10.0.0.2"]);
        let mut renamed = group("g1", 2, "10.0.0.1", &["10.0.0.1", "10.0.0.2"]);
        renamed.name = "新チーム".to_string();
        assert_eq!(
            system_messages(Some(&old), &renamed, me, &name_of),
            vec!["グループ名が「グループg1」から「新チーム」に変わりました"]
        );
        // メンバー追加(他人)
        let grown = group("g1", 2, "10.0.0.1", &["10.0.0.1", "10.0.0.2", "10.0.0.3"]);
        assert_eq!(
            system_messages(Some(&old), &grown, me, &name_of),
            vec!["aliceがcarolを追加しました"]
        );
        // 退出(他人)
        assert_eq!(
            system_messages(Some(&grown), &old, me, &name_of),
            vec!["carolがグループから退出しました"]
        );
        // 自分の退出
        let without_me = group("g1", 2, "10.0.0.2", &["10.0.0.1"]);
        assert_eq!(
            system_messages(Some(&old), &without_me, me, &name_of),
            vec!["グループから退出しました"]
        );
        // メンバー外の update(自分が居ないグループの転送)ではメッセージなし
        let others = group("g2", 1, "10.0.0.1", &["10.0.0.1", "10.0.0.3"]);
        assert!(system_messages(None, &others, me, &name_of).is_empty());
    }
}
