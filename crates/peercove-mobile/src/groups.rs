//! グループ情報のローカル保存(ADR-0016)。デスクトップ(peercove-cli/groups.rs)の
//! GroupStore の受信側部分の移植。
//!
//! E-C のモバイルは**受信側のみ**(グループの作成・編集はデスクトップで行う)。
//! そのため送達管理(pending_sync)は持たず、認可(accepts_update)と
//! 最新リビジョン勝ちの取り込み(apply)+ JSON 永続化だけを移植する。
//!
//! 秘匿ルール: グループ名はログへ出さない(id・IP は可)。

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

use peercove_core::msg::GroupInfo;

pub struct GroupStore {
    path: PathBuf,
    groups: HashMap<String, GroupInfo>,
}

/// [`GroupStore::apply`] の結果: 取り込んだ場合の置換前の値(新規なら None)。
pub struct AppliedGroup {
    pub previous: Option<GroupInfo>,
}

impl GroupStore {
    pub fn path_for(config_path: &Path) -> PathBuf {
        config_path.with_extension("groups.json")
    }

    /// 読み込む(ファイルが無ければ空。壊れていたら警告して空から始める —
    /// グループは伝搬で再取得できる)。
    pub fn load(config_path: &Path) -> GroupStore {
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
        Self { path, groups }
    }

    /// 受信した `group_update` を取り込んでよいか(認可、ADR-0037)。
    /// - 既知グループの変更 → 送信元が**現在の**メンバーであること
    /// - 未知グループ(新規)→ 送信元が**その**グループのメンバーであること
    pub fn accepts_update(&self, group: &GroupInfo, sender: Ipv4Addr) -> bool {
        match self.groups.get(&group.id) {
            Some(current) => current.members.contains(&sender),
            None => group.members.contains(&sender),
        }
    }

    /// **最新リビジョン勝ち**で取り込む(同値は updated_by の IP が大きい方)。
    /// 取り込んだら置換前の値を返す。古ければ None。
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

    pub fn get(&self, id: &str) -> Option<&GroupInfo> {
        self.groups.get(id)
    }

    /// 自分がメンバーのグループ一覧(トーク一覧用。名前順)。
    pub fn joined(&self, self_ip: Ipv4Addr) -> Vec<GroupInfo> {
        let mut list: Vec<GroupInfo> = self
            .groups
            .values()
            .filter(|g| g.members.contains(&self_ip))
            .cloned()
            .collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        list
    }

    fn save(&self) -> anyhow::Result<()> {
        let mut list: Vec<&GroupInfo> = self.groups.values().collect();
        list.sort_by(|a, b| a.id.cmp(&b.id));
        let json = serde_json::to_string_pretty(&list)?;
        std::fs::write(&self.path, json)?;
        Ok(())
    }
}

/// グループ変更のお知らせ本文(LINE 風の中央 1 行)。デスクトップの
/// system_messages の簡略版(追加・退出・改名・新規)。
pub fn system_messages(
    previous: Option<&GroupInfo>,
    group: &GroupInfo,
    self_ip: Ipv4Addr,
    name_of: &dyn Fn(Ipv4Addr) -> String,
) -> Vec<String> {
    let mut out = Vec::new();
    match previous {
        None => {
            if group.members.contains(&self_ip) {
                out.push(format!("グループ「{}」に追加されました", group.name));
            }
        }
        Some(prev) => {
            if prev.name != group.name {
                out.push(format!(
                    "グループ名が「{}」から「{}」に変わりました",
                    prev.name, group.name
                ));
            }
            for added in group.members.iter().filter(|m| !prev.members.contains(m)) {
                out.push(format!("{} が参加しました", name_of(*added)));
            }
            for left in prev.members.iter().filter(|m| !group.members.contains(m)) {
                out.push(format!("{} が退出しました", name_of(*left)));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(id: &str, rev: u64, members: &[&str]) -> GroupInfo {
        GroupInfo {
            id: id.to_string(),
            name: format!("グループ{id}"),
            members: members.iter().map(|m| m.parse().unwrap()).collect(),
            revision: rev,
            updated_by: members[0].parse().unwrap(),
        }
    }

    fn store(label: &str) -> GroupStore {
        let dir = std::env::temp_dir().join(format!(
            "peercove-mobile-groups-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        GroupStore::load(&dir.join("member.toml"))
    }

    #[test]
    fn accepts_update_requires_membership() {
        let mut s = store("authz");
        let g = group("g1", 1, &["10.0.0.2", "10.0.0.3"]);
        // 新規: 送信元がそのグループのメンバーでなければ拒否
        assert!(s.accepts_update(&g, "10.0.0.2".parse().unwrap()));
        assert!(!s.accepts_update(&g, "10.0.0.9".parse().unwrap()));
        s.apply(g);
        // 既知: 現在のメンバーだけが更新できる
        let updated = group("g1", 2, &["10.0.0.2", "10.0.0.9"]);
        assert!(s.accepts_update(&updated, "10.0.0.2".parse().unwrap()));
        assert!(!s.accepts_update(&updated, "10.0.0.9".parse().unwrap()));
    }

    #[test]
    fn apply_is_latest_revision_wins() {
        let mut s = store("rev");
        assert!(s.apply(group("g1", 2, &["10.0.0.2"])).is_some());
        assert!(s.apply(group("g1", 1, &["10.0.0.3"])).is_none(), "古い版");
        assert_eq!(
            s.get("g1").unwrap().members[0],
            "10.0.0.2".parse::<Ipv4Addr>().unwrap()
        );
        assert!(s.apply(group("g1", 3, &["10.0.0.3"])).is_some());
    }

    #[test]
    fn system_messages_describe_changes() {
        let name_of = |ip: Ipv4Addr| format!("m{}", ip.octets()[3]);
        let self_ip: Ipv4Addr = "10.0.0.5".parse().unwrap();
        let created = group("g1", 1, &["10.0.0.2", "10.0.0.5"]);
        let msgs = system_messages(None, &created, self_ip, &name_of);
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].contains("追加されました"));

        let mut renamed = created.clone();
        renamed.name = "新名".to_string();
        renamed.members.push("10.0.0.7".parse().unwrap());
        let msgs = system_messages(Some(&created), &renamed, self_ip, &name_of);
        assert!(msgs.iter().any(|m| m.contains("新名")));
        assert!(msgs.iter().any(|m| m.contains("m7 が参加")));
    }
}
