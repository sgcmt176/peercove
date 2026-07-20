//! トンネル内 DNS のゾーン導出(ADR-0011)。
//!
//! `<ラベル>.<ネットワーク名>.peercove.internal` の A レコード表を、台帳と
//! カスタムレコードから**決定的に**導出する。全ノードが同じ台帳から同じ結果を
//! 得られるよう、ここは純関数のみ(I/O なし)。
//!
//! 実際の DNS 応答(UDP サーバ)とゾーンの合算は `peercove` 側。

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

/// サービス情報に載せる URI スキームの最大長(ADR-0023)。
pub const MAX_SERVICE_SCHEME_LEN: usize = 31;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthCheckKind {
    Tcp,
    HttpHead,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceHealthStatus {
    Healthy,
    Unhealthy,
    Unknown,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceHealthReason {
    NotChecked,
    Offline,
    Timeout,
    ConnectionFailed,
    NameResolutionFailed,
    UnexpectedStatus,
    Disabled,
}

/// ホストが観測し、DNS レコードと一緒に任意配布するサービス状態。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceHealth {
    pub status: ServiceHealthStatus,
    pub reason: ServiceHealthReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
}

impl ServiceHealth {
    pub fn unknown(reason: ServiceHealthReason) -> Self {
        Self {
            status: ServiceHealthStatus::Unknown,
            reason,
            checked_at_unix_ms: None,
            response_ms: None,
            http_status: None,
        }
    }
}

/// 文字列がサービス情報用の URI スキームとして正規形かどうか。
/// 先頭は小文字英字、以降は小文字英数字または `+.-`、最大 31 文字。
pub fn is_service_scheme(input: &str) -> bool {
    if input.is_empty() || input.len() > MAX_SERVICE_SCHEME_LEN {
        return false;
    }
    let mut bytes = input.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z'))
        && bytes.all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'+' | b'.' | b'-'))
}

/// サービス情報から UI 表示・コピー用 URL を組み立てる。
/// scheme が無ければ URL にはせず、http:80 / https:443 は既定ポートとして省略する。
pub fn service_url(fqdn: &str, scheme: Option<&str>, port: Option<u16>) -> Option<String> {
    let scheme = scheme.filter(|value| is_service_scheme(value))?;
    if port == Some(0) {
        return None;
    }
    let show_port = match (scheme, port) {
        ("http", Some(80)) | ("https", Some(443)) | (_, None) => None,
        (_, port) => port,
    };
    Some(match show_port {
        Some(port) => format!("{scheme}://{fqdn}:{port}/"),
        None => format!("{scheme}://{fqdn}/"),
    })
}

/// カスタム A レコード(ADR-0011 §1b)。ホストが設定に持ち、台帳と一緒に配布する。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsRecord {
    /// ラベル(正規化済み。`nas` など)
    pub name: String,
    pub ip: Ipv4Addr,
    /// UI の URL コピー用メタ情報。DNS 応答には使わない(ADR-0023)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<ServiceHealth>,
}

/// カスタム CNAME レコード(ADR-0025、M3-17)。`name` を別ドメイン `target` の
/// 別名にする。ホストが解決して台帳と一緒に配布し、内蔵リゾルバが CNAME 応答を
/// 返す。A レコードとは別枠で配る(旧メンバー互換のため)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CnameRecord {
    /// 相対名(正規化済み。`docs`, `*.app` など)
    pub name: String,
    /// 別名の指す先(小文字・末尾ドットなしの絶対ドメイン。外部可)
    pub target: String,
    /// 転送先を IPv4 へ解決した結果(ADR-0025 フラット化)。ホストが解決して
    /// 載せる。埋まっていればリゾルバは A レコードで返す(スプリット DNS でも
    /// クライアントが直接使えるようにするため)。未解決なら CNAME RR で返す。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_ip: Option<Ipv4Addr>,
    /// UI の URL コピー用メタ情報(ADR-0023 と同じ)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<ServiceHealth>,
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

/// 1 ネットワーク分のゾーンを導出する(ADR-0011 §2、ADR-0021)。
///
/// ラベルの決定規則(全ノードで同一の結果になるよう、仮想 IP 順に確定する):
/// 1. **確定済みの DNS 名(`dns_name`、ADR-0021)を持つメンバーを先に確保する**
///    (固定した名前が従来導出の名前に奪われないため)。ホストが重複検証済み
///    だが、防御的な一意化は維持する
/// 2. `dns_name` の無いメンバーは従来導出: 表示名を [`names::dns_label`] で
///    正規化。空になる名前(日本語など)はホスト = `host`、
///    メンバー = `member-<IP 第 4 オクテット>`
/// 3. 正規化後の衝突は仮想 IP が小さい方が勝ち、負けた方は `<ラベル>-<第4オクテット>`
///    (それも取られていたら `-2`, `-3`, …)で一意化
/// 4. カスタムレコードは自動生成の後に足す。**自動生成側が勝つ**
///    (改名でぶつかったとき、メンバー名の解決が奪われないため)。
///    不正なラベル・重複したカスタムは黙って飛ばす(配布側の検証をすり抜けた
///    場合でも解決全体を壊さない)
pub fn zone_for(network: &str, ledger: &[LedgerEntry], custom: &[DnsRecord]) -> Vec<ZoneEntry> {
    let mut taken: HashSet<String> = HashSet::new();
    let mut entries = Vec::with_capacity(ledger.len() + custom.len());

    // 決定性のため仮想 IP 順に処理する(台帳の並び順に依存しない)
    let mut members: Vec<&LedgerEntry> = ledger.iter().collect();
    members.sort_by_key(|entry| entry.ip);
    let labels = assign_labels(&members, &mut taken);

    for (member, label) in members.iter().zip(labels) {
        entries.push(ZoneEntry {
            fqdn: format!("{label}.{network}.{DNS_SUFFIX}"),
            ip: member.ip,
            public_key: Some(member.public_key),
        });
    }

    for record in custom {
        // ドット付きサブドメイン + 先頭 `*` ワイルドカード(ADR-0022 / ADR-0024)を
        // ラベル単位で受け入れる。単一ラベルはメンバー名との衝突で自動生成が勝つ
        if !names::is_custom_dns_name(&record.name) || taken.contains(&record.name) {
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

/// 仮想 IP 順に並んだメンバーへ DNS ラベルを割り当てる(zone_for の
/// ラベル決定規則そのもの。`resolve_records` と共有する)。
fn assign_labels(members: &[&LedgerEntry], taken: &mut HashSet<String>) -> Vec<String> {
    // 第 1 パス: 確定済みの DNS 名を先に確保(ADR-0021。不正なラベルは
    // 従来導出へフォールバック — 配布側の検証をすり抜けても解決を壊さない)
    let mut labels: Vec<Option<String>> = members
        .iter()
        .map(|member| {
            let fixed = member
                .dns_name
                .as_deref()
                .filter(|n| names::is_dns_label(n))?;
            let label = uniquify(fixed, member.ip.octets()[3], taken);
            taken.insert(label.clone());
            Some(label)
        })
        .collect();

    // 第 2 パス: 残りは従来導出(表示名 → フォールバック → 一意化)
    for (member, label) in members.iter().zip(labels.iter_mut()) {
        if label.is_some() {
            continue;
        }
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
        let derived = uniquify(&base, oct, taken);
        taken.insert(derived.clone());
        *label = Some(derived);
    }
    labels
        .into_iter()
        .map(|label| label.expect("両パスでどのメンバーにもラベルが付く"))
        .collect()
}

/// カスタムレコード設定(ADR-0022)を配布用の `{name, ip}` へ解決する。
/// ホストが台帳構築のたびに呼ぶ(5 秒周期)ため、メンバー参照はその時点の
/// 仮想 IP・DNS ラベルへ追随する。参照先が台帳に見つからないレコード
/// (削除直後など)は黙って外す — 解決全体を壊さない。
pub fn resolve_records(
    records: &[crate::config::DnsRecordConfig],
    ledger: &[LedgerEntry],
) -> Vec<DnsRecord> {
    use crate::config::MemberRef;

    let mut taken = HashSet::new();
    let mut members: Vec<&LedgerEntry> = ledger.iter().collect();
    members.sort_by_key(|entry| entry.ip);
    let labels = assign_labels(&members, &mut taken);
    let lookup = |reference: &MemberRef| {
        members
            .iter()
            .zip(labels.iter())
            .find(|(member, _)| match reference {
                MemberRef::Host => member.is_host,
                MemberRef::Key(key) => member.public_key == *key,
            })
    };

    let mut resolved = Vec::with_capacity(records.len());
    for record in records {
        let ip = match (record.ip, &record.member) {
            (Some(ip), _) => ip,
            (None, Some(member)) => match lookup(member) {
                Some((entry, _)) => entry.ip,
                None => continue,
            },
            (None, None) => continue,
        };
        let name = match &record.under {
            None => record.name.clone(),
            Some(under) => match lookup(under) {
                Some((_, label)) => format!("{}.{label}", record.name),
                None => continue,
            },
        };
        resolved.push(DnsRecord {
            name,
            ip,
            scheme: record.scheme.clone(),
            port: record.port,
            health: None,
        });
    }
    resolved
}

/// カスタム CNAME レコード(ADR-0025)を配布用へ解決する。`under`(親メンバー)は
/// その時点の DNS ラベルへ解決する(A レコードと同じ規則)。参照先が台帳に無い
/// レコードは黙って外す。ターゲットは解決不要(絶対ドメインをそのまま配る)。
pub fn resolve_cnames(
    records: &[crate::config::DnsRecordConfig],
    ledger: &[LedgerEntry],
) -> Vec<CnameRecord> {
    use crate::config::MemberRef;

    let mut taken = HashSet::new();
    let mut members: Vec<&LedgerEntry> = ledger.iter().collect();
    members.sort_by_key(|entry| entry.ip);
    let labels = assign_labels(&members, &mut taken);
    let label_of = |reference: &MemberRef| {
        members
            .iter()
            .zip(labels.iter())
            .find(|(member, _)| match reference {
                MemberRef::Host => member.is_host,
                MemberRef::Key(key) => member.public_key == *key,
            })
            .map(|(_, label)| label.clone())
    };

    let mut resolved = Vec::new();
    for record in records {
        let Some(target) = &record.cname else {
            continue;
        };
        let name = match &record.under {
            None => record.name.clone(),
            Some(under) => match label_of(under) {
                Some(label) => format!("{}.{label}", record.name),
                None => continue,
            },
        };
        resolved.push(CnameRecord {
            name,
            target: target.clone(),
            resolved_ip: None, // ホスト(poc)が外部解決してから埋める
            scheme: record.scheme.clone(),
            port: record.port,
            health: None,
        });
    }
    resolved
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
            dns_name: None,
            ip: ip.parse().unwrap(),
            public_key: PrivateKey::generate().public_key(),
            app_version: None,
            platform: None,
            capabilities: vec![],
            invite_status: None,
            invite_expires_at: None,
            online: true,
            is_host,
            endpoint: None,
            endpoint_age_secs: None,
            subnets: vec![],
            blocked: false,
            force_relay: false,
            acl_rule_id: None,
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

    /// 確定済みの DNS 名(ADR-0021)は表示名の導出より優先され、
    /// 従来導出の同名メンバー(IP が小さくても)に奪われない。
    #[test]
    fn fixed_dns_name_wins_over_derived() {
        let mut fixed = member(Some("山田"), "10.1.0.5", false);
        fixed.dns_name = Some("alice".to_string());
        let ledger = vec![
            member(Some("alice"), "10.1.0.2", false), // 従来導出で "alice" になるはずだった
            fixed,
        ];
        let zone = zone_for("net", &ledger, &[]);
        assert_eq!(
            fqdns(&zone),
            vec![
                "alice-2.net.peercove.internal", // 従来導出側が一意化で譲る
                "alice.net.peercove.internal",   // 確定名が勝つ
            ]
        );

        // 表示名を変えても確定名は変わらない(表示名と DNS 名の独立)
        let mut renamed = member(Some("べつの名前"), "10.1.0.5", false);
        renamed.dns_name = Some("alice".to_string());
        let zone = zone_for("net", &[renamed], &[]);
        assert_eq!(fqdns(&zone), vec!["alice.net.peercove.internal"]);

        // 不正な dns_name(正規化されていない)は従来導出へフォールバック
        let mut bad = member(Some("Bob"), "10.1.0.7", false);
        bad.dns_name = Some("Bad Label".to_string());
        let zone = zone_for("net", &[bad], &[]);
        assert_eq!(fqdns(&zone), vec!["bob.net.peercove.internal"]);
    }

    /// レコード解決(ADR-0022): member 参照はその時点の IP、under は
    /// その時点のラベルへ追随する。参照先が居なければ黙って外す。
    #[test]
    fn resolve_records_follows_member_ip_and_label() {
        use crate::config::{DnsRecordConfig, MemberRef};

        let host = member(None, "10.1.0.1", true);
        let mut alice = member(Some("山田"), "10.1.0.5", false);
        alice.dns_name = Some("alice".to_string());
        let alice_key = alice.public_key;
        let ledger = vec![host, alice];

        let records = vec![
            DnsRecordConfig {
                id: None,
                // エイリアス(要望 10/11): メンバー参照 → IP 追随
                name: "gamehost".to_string(),
                ip: None,
                member: Some(MemberRef::Key(alice_key)),
                cname: None,
                under: None,
                scheme: Some("http".to_string()),
                port: Some(8080),
                health_check: None,
                health_kind: None,
                health_path: None,
                health_expect_status: None,
                health_external: false,
            },
            DnsRecordConfig {
                id: None,
                // 端末配下サブドメイン(要望 12): ホスト配下
                name: "web".to_string(),
                ip: None,
                member: Some(MemberRef::Host),
                cname: None,
                under: Some(MemberRef::Host),
                scheme: None,
                port: None,
                health_check: None,
                health_kind: None,
                health_path: None,
                health_expect_status: None,
                health_external: false,
            },
            DnsRecordConfig {
                id: None,
                // LAN 機器(要望 14): alice 配下の固定 IP
                name: "printer".to_string(),
                ip: Some("192.168.10.50".parse().unwrap()),
                member: None,
                cname: None,
                under: Some(MemberRef::Key(alice_key)),
                scheme: None,
                port: Some(9100),
                health_check: None,
                health_kind: None,
                health_path: None,
                health_expect_status: None,
                health_external: false,
            },
            DnsRecordConfig {
                id: None,
                // 参照先が台帳に居ない(削除直後)→ 黙って外す
                name: "ghost".to_string(),
                ip: None,
                member: Some(MemberRef::Key(PrivateKey::generate().public_key())),
                cname: None,
                under: None,
                scheme: None,
                port: None,
                health_check: None,
                health_kind: None,
                health_path: None,
                health_expect_status: None,
                health_external: false,
            },
            DnsRecordConfig {
                id: None,
                // CNAME(ADR-0025): 外部ドメインの別名
                name: "docs".to_string(),
                ip: None,
                member: None,
                cname: Some("example.com".to_string()),
                under: None,
                scheme: None,
                port: None,
                health_check: None,
                health_kind: None,
                health_path: None,
                health_expect_status: None,
                health_external: false,
            },
        ];
        let resolved = resolve_records(&records, &ledger);
        // CNAME レコードは A 解決に混ざらない(別枠)
        let cnames = resolve_cnames(&records, &ledger);
        assert_eq!(cnames.len(), 1);
        assert_eq!(cnames[0].name, "docs");
        assert_eq!(cnames[0].target, "example.com");
        let names: Vec<(&str, String)> = resolved
            .iter()
            .map(|r| (r.name.as_str(), r.ip.to_string()))
            .collect();
        assert_eq!(
            names,
            vec![
                ("gamehost", "10.1.0.5".to_string()),
                ("web.host", "10.1.0.1".to_string()),
                ("printer.alice", "192.168.10.50".to_string()),
            ]
        );
        assert_eq!(resolved[0].scheme.as_deref(), Some("http"));
        assert_eq!(resolved[0].port, Some(8080));
        assert_eq!(resolved[2].scheme, None);
        assert_eq!(resolved[2].port, Some(9100));

        // IP が変わっても(再招待)同じ設定で追随する
        let mut moved = member(Some("山田"), "10.1.0.9", false);
        moved.dns_name = Some("alice".to_string());
        moved.public_key = alice_key;
        let resolved = resolve_records(&records[..1], &[moved]);
        assert_eq!(resolved[0].ip.to_string(), "10.1.0.9");
    }

    /// ドット付きの相対名(サブドメインレコード)はゾーンに入る。
    /// ラベル単位で不正なものは従来どおり無視する。
    #[test]
    fn zone_accepts_dotted_custom_records() {
        let ledger = vec![member(Some("alice"), "10.1.0.2", false)];
        let custom = vec![
            DnsRecord {
                name: "web.alice".to_string(),
                ip: "10.1.0.2".parse().unwrap(),
                scheme: None,
                port: None,
                health: None,
            },
            DnsRecord {
                name: "Bad.alice".to_string(), // 大文字ラベル → 無視
                ip: "10.1.0.3".parse().unwrap(),
                scheme: None,
                port: None,
                health: None,
            },
        ];
        let zone = zone_for("home", &ledger, &custom);
        assert_eq!(
            fqdns(&zone),
            vec![
                "alice.home.peercove.internal",
                "web.alice.home.peercove.internal",
            ]
        );
    }

    #[test]
    fn custom_records_append_but_lose_to_members() {
        let ledger = vec![member(Some("nas"), "10.1.0.2", false)];
        let custom = vec![
            DnsRecord {
                name: "nas".to_string(), // メンバー名と衝突 → 自動生成が勝つ
                ip: "10.1.0.99".parse().unwrap(),
                scheme: None,
                port: None,
                health: None,
            },
            DnsRecord {
                name: "printer".to_string(),
                ip: "10.1.0.50".parse().unwrap(),
                scheme: None,
                port: None,
                health: None,
            },
            DnsRecord {
                name: "Bad Label".to_string(), // 不正 → 無視
                ip: "10.1.0.51".parse().unwrap(),
                scheme: None,
                port: None,
                health: None,
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

    #[test]
    fn service_scheme_and_url_rules() {
        assert!(is_service_scheme("http"));
        assert!(is_service_scheme("git+ssh"));
        assert!(is_service_scheme("web.v2-test"));
        assert!(!is_service_scheme("HTTP"));
        assert!(!is_service_scheme("1http"));
        assert!(!is_service_scheme("http_test"));
        assert!(!is_service_scheme(""));
        assert!(!is_service_scheme(&"a".repeat(32)));

        let fqdn = "gamehost.home.peercove.internal";
        assert_eq!(
            service_url(fqdn, Some("http"), Some(8080)).as_deref(),
            Some("http://gamehost.home.peercove.internal:8080/")
        );
        assert_eq!(
            service_url(fqdn, Some("http"), Some(80)).as_deref(),
            Some("http://gamehost.home.peercove.internal/")
        );
        assert_eq!(
            service_url(fqdn, Some("https"), Some(443)).as_deref(),
            Some("https://gamehost.home.peercove.internal/")
        );
        assert_eq!(service_url(fqdn, None, Some(8080)), None);
        assert_eq!(service_url(fqdn, Some("HTTP"), None), None);
        assert_eq!(service_url(fqdn, Some("http"), Some(0)), None);
    }
}
