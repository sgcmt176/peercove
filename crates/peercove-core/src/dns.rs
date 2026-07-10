//! トンネル内 DNS のゾーン導出(ADR-0011)。
//!
//! `<ラベル>.<ネットワーク名>.peercove.internal` の A レコード表を、台帳と
//! カスタムレコードから**決定的に**導出する。全ノードが同じ台帳から同じ結果を
//! 得られるよう、ここは純関数のみ(I/O なし)。
//!
//! 実際の DNS 応答(UDP サーバ)とゾーンの合算は `peercove-poc` 側。

use std::collections::HashSet;
use std::net::Ipv4Addr;

use serde::{Deserialize, Serialize};

use crate::keys::PublicKey;
use crate::names;
use crate::proto::LedgerEntry;

/// 固定サフィックス(ICANN がプライベート用途に予約した TLD 配下)。
pub const DNS_SUFFIX: &str = "peercove.internal";

/// 応答 TTL。台帳の更新(5 秒周期)にすぐ追随できる短さにする。
pub const DNS_TTL_SECS: u32 = 30;

/// カスタム A レコード(ADR-0011 §1b)。ホストが設定に持ち、台帳と一緒に配布する。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsRecord {
    /// ラベル(正規化済み。`nas` など)
    pub name: String,
    pub ip: Ipv4Addr,
}

/// ゾーンの 1 エントリ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoneEntry {
    /// 完全修飾名(小文字、末尾ドットなし)。例 `alice.game.peercove.internal`
    pub fqdn: String,
    pub ip: Ipv4Addr,
    /// 台帳のメンバー由来なら、そのメンバーの公開鍵(UI が突き合わせに使う)。
    /// カスタムレコード由来は `None`。
    pub public_key: Option<PublicKey>,
}

/// 1 ネットワーク分のゾーンを導出する(ADR-0011 §2)。
///
/// ラベルの決定規則(全ノードで同一の結果になるよう、仮想 IP 順に確定する):
/// 1. 表示名を [`names::dns_label`] で正規化。空になる名前(日本語など)は
///    ホスト = `host`、メンバー = `member-<IP 第 4 オクテット>`
/// 2. 正規化後の衝突は仮想 IP が小さい方が勝ち、負けた方は `<ラベル>-<第4オクテット>`
///    (それも取られていたら `-2`, `-3`, …)で一意化
/// 3. カスタムレコードは自動生成の後に足す。**自動生成側が勝つ**
///    (改名でぶつかったとき、メンバー名の解決が奪われないため)。
///    不正なラベル・重複したカスタムは黙って飛ばす(配布側の検証をすり抜けた
///    場合でも解決全体を壊さない)
pub fn zone_for(network: &str, ledger: &[LedgerEntry], custom: &[DnsRecord]) -> Vec<ZoneEntry> {
    let mut taken: HashSet<String> = HashSet::new();
    let mut entries = Vec::with_capacity(ledger.len() + custom.len());

    // 決定性のため仮想 IP 順に処理する(台帳の並び順に依存しない)
    let mut members: Vec<&LedgerEntry> = ledger.iter().collect();
    members.sort_by_key(|entry| entry.ip);

    for member in members {
        let oct = member.ip.octets()[3];
        let base = member
            .name
            .as_deref()
            .and_then(names::dns_label)
            .unwrap_or_else(|| {
                if member.is_host {
                    "host".to_string()
                } else {
                    format!("member-{oct}")
                }
            });
        let label = uniquify(&base, oct, &taken);
        taken.insert(label.clone());
        entries.push(ZoneEntry {
            fqdn: format!("{label}.{network}.{DNS_SUFFIX}"),
            ip: member.ip,
            public_key: Some(member.public_key),
        });
    }

    for record in custom {
        if !names::is_dns_label(&record.name) || taken.contains(&record.name) {
            continue; // 自動生成が勝つ / 不正ラベルは無視(doc コメント参照)
        }
        taken.insert(record.name.clone());
        entries.push(ZoneEntry {
            fqdn: format!("{}.{network}.{DNS_SUFFIX}", record.name),
            ip: record.ip,
            public_key: None,
        });
    }
    entries
}

/// `base` が取られていたら `-<oct>`、それも駄目なら `-<oct>-2`, … で一意化。
fn uniquify(base: &str, oct: u8, taken: &HashSet<String>) -> String {
    if !taken.contains(base) {
        return base.to_string();
    }
    let with_oct = format!("{base}-{oct}");
    if !taken.contains(&with_oct) {
        return with_oct;
    }
    (2..)
        .map(|i| format!("{with_oct}-{i}"))
        .find(|candidate| !taken.contains(candidate))
        .expect("いつかは空きがある")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::PrivateKey;

    fn member(name: Option<&str>, ip: &str, is_host: bool) -> LedgerEntry {
        LedgerEntry {
            name: name.map(String::from),
            ip: ip.parse().unwrap(),
            public_key: PrivateKey::generate().public_key(),
            online: true,
            is_host,
            endpoint: None,
            endpoint_age_secs: None,
            subnets: vec![],
        }
    }

    fn fqdns(zone: &[ZoneEntry]) -> Vec<&str> {
        zone.iter().map(|entry| entry.fqdn.as_str()).collect()
    }

    #[test]
    fn derives_labels_with_fallbacks() {
        let ledger = vec![
            member(Some("Alice"), "10.1.0.2", false),
            member(None, "10.1.0.1", true), // 名前なしホスト → host
            member(Some("たろう"), "10.1.0.3", false), // 非 ASCII → member-3
        ];
        let zone = zone_for("game", &ledger, &[]);
        assert_eq!(
            fqdns(&zone),
            vec![
                "host.game.peercove.internal",
                "alice.game.peercove.internal",
                "member-3.game.peercove.internal",
            ],
            "仮想 IP 順に並ぶ"
        );
        assert!(zone.iter().all(|entry| entry.public_key.is_some()));
    }

    #[test]
    fn collision_resolves_deterministically_by_ip() {
        // 正規化するとどちらも "alice"。IP の小さい方が勝つ
        let ledger = vec![
            member(Some("ALICE"), "10.1.0.5", false),
            member(Some("alice"), "10.1.0.2", false),
        ];
        let zone = zone_for("net", &ledger, &[]);
        assert_eq!(
            fqdns(&zone),
            vec![
                "alice.net.peercove.internal",
                "alice-5.net.peercove.internal",
            ]
        );

        // 台帳の並び順を変えても結果は同じ(決定性)
        let reversed: Vec<LedgerEntry> = ledger.into_iter().rev().collect();
        let zone2 = zone_for("net", &reversed, &[]);
        assert_eq!(fqdns(&zone2), fqdns(&zone));
    }

    #[test]
    fn custom_records_append_but_lose_to_members() {
        let ledger = vec![member(Some("nas"), "10.1.0.2", false)];
        let custom = vec![
            DnsRecord {
                name: "nas".to_string(), // メンバー名と衝突 → 自動生成が勝つ
                ip: "10.1.0.99".parse().unwrap(),
            },
            DnsRecord {
                name: "printer".to_string(),
                ip: "10.1.0.50".parse().unwrap(),
            },
            DnsRecord {
                name: "Bad Label".to_string(), // 不正 → 無視
                ip: "10.1.0.51".parse().unwrap(),
            },
        ];
        let zone = zone_for("home", &ledger, &custom);
        assert_eq!(
            fqdns(&zone),
            vec![
                "nas.home.peercove.internal",
                "printer.home.peercove.internal",
            ]
        );
        assert_eq!(zone[0].ip.to_string(), "10.1.0.2", "メンバー側の IP");
        assert!(zone[1].public_key.is_none(), "カスタム由来");
    }
}
