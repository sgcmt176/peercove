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

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Context;
use peercove_core::msg::GroupInfo;

pub type SharedGroups = Arc<Mutex<GroupStore>>;

/// 1 ネットワーク分の既知グループ(メモリ + JSON ファイル)。
pub struct GroupStore {
    path: PathBuf,
    groups: HashMap<String, GroupInfo>,
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
        Arc::new(Mutex::new(Self { path, groups }))
    }

    /// 受信した(または自分で作った)グループ全量を取り込む。
    /// **最新リビジョン勝ち**: 手元より revision が大きければ置換、同値は
    /// updated_by の IP が大きい方が勝つ(決定的にどちらかへ収束させる)。
    /// 取り込んだら true(ファイルへも保存する)。
    pub fn apply(&mut self, group: GroupInfo) -> bool {
        let newer = match self.groups.get(&group.id) {
            None => true,
            Some(current) => {
                group.revision > current.revision
                    || (group.revision == current.revision && group.updated_by > current.updated_by)
            }
        };
        if !newer {
            return false;
        }
        self.groups.insert(group.id.clone(), group);
        if let Err(e) = self.save() {
            tracing::warn!("グループ情報の保存に失敗しました: {e:#}");
        }
        true
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

    /// このメンバーが属する既知グループ(オンライン復帰時の追いつき再送用)。
    pub fn groups_with(&self, member: Ipv4Addr) -> Vec<GroupInfo> {
        let mut list: Vec<GroupInfo> = self
            .groups
            .values()
            .filter(|g| g.members.contains(&member))
            .cloned()
            .collect();
        list.sort_by(|a, b| a.id.cmp(&b.id));
        list
    }
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

    /// 取り込み → 再読込で残る(永続化)。
    #[test]
    fn apply_and_reload() {
        let config = temp_config("reload");
        {
            let store = GroupStore::load(&config);
            let mut store = store.lock().unwrap();
            assert!(store.apply(group("g1", 1, "10.0.0.1", &["10.0.0.1", "10.0.0.2"])));
        }
        let store = GroupStore::load(&config);
        let store = store.lock().unwrap();
        let listed = store.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "g1");
        assert_eq!(listed[0].members.len(), 2);
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }

    /// 最新リビジョン勝ち: 古い revision は捨て、同値は updated_by で決着。
    #[test]
    fn newest_revision_wins() {
        let config = temp_config("rev");
        let store = GroupStore::load(&config);
        let mut store = store.lock().unwrap();
        assert!(store.apply(group("g1", 2, "10.0.0.1", &["10.0.0.1"])));
        assert!(
            !store.apply(group("g1", 1, "10.0.0.9", &["10.0.0.9"])),
            "古い revision は捨てる"
        );
        assert_eq!(store.get("g1").unwrap().updated_by.octets()[3], 1);
        assert!(
            store.apply(group("g1", 2, "10.0.0.5", &["10.0.0.5"])),
            "同値は updated_by の大きい方が勝つ"
        );
        assert!(
            !store.apply(group("g1", 2, "10.0.0.3", &["10.0.0.3"])),
            "同値で小さい方は捨てる"
        );
        assert!(store.apply(group("g1", 3, "10.0.0.2", &["10.0.0.2"])));
        assert_eq!(store.get("g1").unwrap().revision, 3);
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

    /// groups_with はそのメンバーが属するグループだけを返す。
    #[test]
    fn groups_with_filters_by_member() {
        let config = temp_config("with");
        let store = GroupStore::load(&config);
        let mut store = store.lock().unwrap();
        store.apply(group("g1", 1, "10.0.0.1", &["10.0.0.1", "10.0.0.2"]));
        store.apply(group("g2", 1, "10.0.0.1", &["10.0.0.1", "10.0.0.3"]));
        let hits = store.groups_with("10.0.0.2".parse().unwrap());
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "g1");
        assert!(store.groups_with("10.0.0.9".parse().unwrap()).is_empty());
        let _ = std::fs::remove_dir_all(config.parent().unwrap());
    }
}
