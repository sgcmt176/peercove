//! ホスト設定 `[acl]` の読み書き(ADR-0018、M3-10)。
//!
//! `deny` は仮想 IP の組(順不同)。実行中の host デーモンは定期再読込で
//! 約 5 秒後に追随する(リレー遮断 + 台帳の再配布)。

use std::net::Ipv4Addr;
use std::path::Path;

use peercove_core::config::Config;

use crate::peers::{load_doc, write_validated};

/// 現在の遮断組(正規化済み: 小さい IP が先、重複なし)を返す。
pub fn list_deny(config_path: &Path) -> anyhow::Result<Vec<(Ipv4Addr, Ipv4Addr)>> {
    Ok(Config::load(config_path)?.acl.normalized_deny())
}

/// 遮断組を丸ごと差し替える。空なら `[acl]` セクションごと消す。
/// ホスト IP を含む組・サブネット外の IP は検証(`Config::validate`)で拒否される。
pub fn set_deny(config_path: &Path, deny: &[(Ipv4Addr, Ipv4Addr)]) -> anyhow::Result<()> {
    let mut doc = load_doc(config_path)?;
    write_deny(&mut doc, deny);
    write_validated(config_path, &doc.to_string())
}

/// toml_edit ドキュメントへ遮断組を書き込む(remove-peer の掃除と共用)。
pub(crate) fn write_deny(doc: &mut toml_edit::DocumentMut, deny: &[(Ipv4Addr, Ipv4Addr)]) {
    // 正規化(小さい IP を先、重複除去)してから書く
    let mut pairs: Vec<(Ipv4Addr, Ipv4Addr)> = deny
        .iter()
        .map(|&(a, b)| if a <= b { (a, b) } else { (b, a) })
        .collect();
    pairs.sort_unstable();
    pairs.dedup();
    if pairs.is_empty() {
        doc.remove("acl");
        return;
    }
    let mut array = toml_edit::Array::new();
    for (a, b) in pairs {
        let mut pair = toml_edit::Array::new();
        pair.push(a.to_string());
        pair.push(b.to_string());
        array.push(pair);
    }
    // `doc["acl"]["deny"] = …` だと `acl = { deny = … }` が文書先頭に
    // 入ってしまうため、明示テーブル([acl] セクション)として書く
    let mut table = toml_edit::Table::new();
    table["deny"] = toml_edit::value(array);
    doc["acl"] = toml_edit::Item::Table(table);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::peers::{append_peer, remove_peer, NewPeer, Selector};
    use peercove_core::keys::PrivateKey;

    fn setup(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("peercove-ops-acl-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("host.toml");
        std::fs::write(
            &config,
            "# コメント保持の確認用\n[interface]\nprivate_key_file = \"host.key\"\naddress = \"10.100.42.1/24\"\nlisten_port = 51820\n",
        )
        .unwrap();
        crate::secret::write_secret(&dir.join("host.key"), &PrivateKey::generate().to_base64())
            .unwrap();
        config
    }

    fn add(config: &Path, name: &str, ip: &str) {
        append_peer(
            config,
            &NewPeer {
                public_key: PrivateKey::generate().public_key(),
                ip: ip.parse().unwrap(),
                name: Some(name),
                dns_name: None,
                preshared_key_file: None,
                invite_id: None,
                invite_issued_at: None,
                invite_expires_at: None,
            },
        )
        .unwrap();
    }

    fn pair(a: &str, b: &str) -> (Ipv4Addr, Ipv4Addr) {
        (a.parse().unwrap(), b.parse().unwrap())
    }

    #[test]
    fn set_and_list_normalize_pairs() {
        let config = setup("set-list");
        add(&config, "alice", "10.100.42.2");
        add(&config, "bob", "10.100.42.3");
        assert!(list_deny(&config).unwrap().is_empty());

        // 逆順 + 重複で渡しても正規化される
        set_deny(
            &config,
            &[
                pair("10.100.42.3", "10.100.42.2"),
                pair("10.100.42.2", "10.100.42.3"),
            ],
        )
        .unwrap();
        assert_eq!(
            list_deny(&config).unwrap(),
            vec![pair("10.100.42.2", "10.100.42.3")]
        );
        let text = std::fs::read_to_string(&config).unwrap();
        assert!(text.starts_with("# コメント保持の確認用"), "{text}");

        // 空で差し替えるとセクションごと消える
        set_deny(&config, &[]).unwrap();
        assert!(list_deny(&config).unwrap().is_empty());
        let text = std::fs::read_to_string(&config).unwrap();
        assert!(!text.contains("[acl]"), "{text}");
    }

    #[test]
    fn set_rejects_host_ip_and_out_of_subnet() {
        let config = setup("reject");
        add(&config, "alice", "10.100.42.2");
        assert!(set_deny(&config, &[pair("10.100.42.1", "10.100.42.2")]).is_err());
        assert!(set_deny(&config, &[pair("192.168.1.2", "10.100.42.2")]).is_err());
        // 失敗しても元の設定は壊れていない
        assert!(list_deny(&config).unwrap().is_empty());
    }

    /// remove-peer は削除したメンバーの IP を含む組も掃除する。
    #[test]
    fn remove_peer_cleans_up_acl_entries() {
        let config = setup("cleanup");
        add(&config, "alice", "10.100.42.2");
        add(&config, "bob", "10.100.42.3");
        add(&config, "carol", "10.100.42.4");
        set_deny(
            &config,
            &[
                pair("10.100.42.2", "10.100.42.3"),
                pair("10.100.42.3", "10.100.42.4"),
            ],
        )
        .unwrap();

        remove_peer(&config, &Selector::Name("bob")).unwrap();
        assert!(
            list_deny(&config).unwrap().is_empty(),
            "bob を含む組がすべて消える"
        );
        let text = std::fs::read_to_string(&config).unwrap();
        assert!(!text.contains("[acl]"), "空になったらセクションごと消える");

        // 残る組がある場合はそれだけ残る
        add(&config, "bob", "10.100.42.3");
        set_deny(
            &config,
            &[
                pair("10.100.42.2", "10.100.42.3"),
                pair("10.100.42.2", "10.100.42.4"),
            ],
        )
        .unwrap();
        remove_peer(&config, &Selector::Name("bob")).unwrap();
        assert_eq!(
            list_deny(&config).unwrap(),
            vec![pair("10.100.42.2", "10.100.42.4")]
        );
    }
}
