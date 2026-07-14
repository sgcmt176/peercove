//! ホスト設定 `[acl]` の読み書き(ADR-0018、M3-10)。
//!
//! `deny` は仮想 IP の組(順不同)。実行中の host デーモンは定期再読込で
//! 約 5 秒後に追随する(リレー遮断 + 台帳の再配布)。

use std::net::Ipv4Addr;
use std::path::Path;

use peercove_core::acl::{AclAction, AclGroup, AclRule, AclTarget};
use peercove_core::config::Config;
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicySettings {
    pub default: AclAction,
    pub groups: Vec<AclGroup>,
    pub rules: Vec<AclRule>,
}

/// 旧deny pairは、画面上で同じ結果の双方向v2ルールへ展開する。
pub fn read_policy(config_path: &Path) -> anyhow::Result<PolicySettings> {
    let config = Config::load(config_path)?;
    let mut rules = Vec::new();
    for (index, (a, b)) in config.acl.normalized_deny().into_iter().enumerate() {
        for (suffix, source, destination) in [("a-b", a, b), ("b-a", b, a)] {
            rules.push(AclRule {
                id: format!("legacy-{index}-{suffix}"),
                action: AclAction::Deny,
                source: AclTarget::Subnet {
                    subnet: ipnet::Ipv4Net::new(source, 32).unwrap(),
                },
                destination: AclTarget::Subnet {
                    subnet: ipnet::Ipv4Net::new(destination, 32).unwrap(),
                },
                protocol: peercove_core::acl::AclProtocol::Any,
                ports: vec![],
                enabled: true,
            });
        }
    }
    rules.extend(config.acl.rules);
    Ok(PolicySettings {
        default: config.acl.default,
        groups: config.acl.groups,
        rules,
    })
}

/// ACL v2全体を原子的に差し替える。保存時に旧deny pairはv2へ移行される。
pub fn write_policy(config_path: &Path, policy: &PolicySettings) -> anyhow::Result<()> {
    let mut doc = load_doc(config_path)?;
    let mut acl = toml_edit::Table::new();
    acl["default"] = toml_edit::value(match policy.default {
        AclAction::Allow => "allow",
        AclAction::Deny => "deny",
    });
    if !policy.groups.is_empty() {
        let mut groups = toml_edit::ArrayOfTables::new();
        for group in &policy.groups {
            let mut table = toml_edit::Table::new();
            table["id"] = toml_edit::value(&group.id);
            let mut members = toml_edit::Array::new();
            for member in &group.members {
                members.push(member.to_base64());
            }
            table["members"] = toml_edit::value(members);
            groups.push(table);
        }
        acl["group"] = toml_edit::Item::ArrayOfTables(groups);
    }
    if !policy.rules.is_empty() {
        let mut rules = toml_edit::ArrayOfTables::new();
        for rule in &policy.rules {
            let mut table = toml_edit::Table::new();
            table["id"] = toml_edit::value(&rule.id);
            table["action"] = toml_edit::value(match rule.action {
                AclAction::Allow => "allow",
                AclAction::Deny => "deny",
            });
            table["source"] = target_item(&rule.source);
            table["destination"] = target_item(&rule.destination);
            table["protocol"] = toml_edit::value(match rule.protocol {
                peercove_core::acl::AclProtocol::Any => "any",
                peercove_core::acl::AclProtocol::Tcp => "tcp",
                peercove_core::acl::AclProtocol::Udp => "udp",
                peercove_core::acl::AclProtocol::Icmp => "icmp",
            });
            if !rule.ports.is_empty() {
                let mut ports = toml_edit::Array::new();
                for port in &rule.ports {
                    ports.push(port);
                }
                table["ports"] = toml_edit::value(ports);
            }
            if !rule.enabled {
                table["enabled"] = toml_edit::value(false);
            }
            rules.push(table);
        }
        acl["rule"] = toml_edit::Item::ArrayOfTables(rules);
    }
    doc["acl"] = toml_edit::Item::Table(acl);
    write_validated(config_path, &doc.to_string())
}

fn target_item(target: &AclTarget) -> toml_edit::Item {
    match target {
        AclTarget::Any(_) => toml_edit::value("any"),
        AclTarget::Member { member } => inline("member", &member.to_base64()),
        AclTarget::Group { group } => inline("group", group),
        AclTarget::Subnet { subnet } => inline("subnet", &subnet.to_string()),
        AclTarget::Service { service } => inline("service", service),
    }
}

fn inline(key: &str, value: &str) -> toml_edit::Item {
    let mut table = toml_edit::InlineTable::new();
    table.insert(key, toml_edit::Value::from(value));
    toml_edit::Item::Value(toml_edit::Value::InlineTable(table))
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

    #[test]
    fn v2_policy_roundtrip_and_legacy_migration() {
        let config = setup("v2");
        add(&config, "alice", "10.100.42.2");
        add(&config, "bob", "10.100.42.3");
        set_deny(&config, &[pair("10.100.42.2", "10.100.42.3")]).unwrap();
        let policy = read_policy(&config).unwrap();
        assert_eq!(policy.rules.len(), 2);
        write_policy(&config, &policy).unwrap();
        let loaded = Config::load(&config).unwrap();
        assert!(loaded.acl.deny.is_empty());
        assert_eq!(loaded.acl.rules.len(), 2);
        let evaluated = peercove_core::acl::AclPolicy::compile(&loaded).unwrap();
        assert_eq!(
            evaluated
                .evaluate(
                    "10.100.42.2".parse().unwrap(),
                    "10.100.42.3".parse().unwrap(),
                    6,
                    Some(80)
                )
                .action,
            AclAction::Deny
        );
    }

    #[test]
    fn removing_member_cleans_v2_group_and_rules() {
        let config = setup("v2-remove");
        add(&config, "alice", "10.100.42.2");
        add(&config, "bob", "10.100.42.3");
        let loaded = Config::load(&config).unwrap();
        let bob = loaded
            .peers
            .iter()
            .find(|peer| peer.name.as_deref() == Some("bob"))
            .unwrap()
            .public_key;
        write_policy(
            &config,
            &PolicySettings {
                default: AclAction::Allow,
                groups: vec![AclGroup {
                    id: "servers".into(),
                    members: vec![bob],
                }],
                rules: vec![AclRule {
                    id: "deny-servers".into(),
                    action: AclAction::Deny,
                    source: AclTarget::Any("any".into()),
                    destination: AclTarget::Group {
                        group: "servers".into(),
                    },
                    protocol: peercove_core::acl::AclProtocol::Any,
                    ports: vec![],
                    enabled: true,
                }],
            },
        )
        .unwrap();
        remove_peer(&config, &Selector::Name("bob")).unwrap();
        let after = Config::load(&config).unwrap();
        assert!(after.acl.groups.is_empty());
        assert!(after.acl.rules.is_empty());
    }
}
