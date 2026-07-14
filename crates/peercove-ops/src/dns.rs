//! カスタム DNS レコードの設定ファイル操作(ADR-0011 §1b、ADR-0022)。
//!
//! ホストの設定に `[[dns_record]]` を追加・削除する。実行中のホストは 5 秒の
//! 再読込で拾い、メンバー参照をその時点の IP へ解決してから台帳と一緒に
//! メンバーへ配布する(peers.rs と同じ反映経路)。
//! 表示は持たず、UI / CLI 双方から使う(ADR-0008)。

use std::net::Ipv4Addr;
use std::path::Path;

use anyhow::{bail, Context};
use peercove_core::config::{Config, MemberRef};
use peercove_core::dns::{is_service_scheme, service_url, DNS_SUFFIX};
use peercove_core::names;

use crate::peers::{load_doc, write_validated};

/// レコードのターゲット(ADR-0022 / ADR-0025)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordTarget {
    /// 固定 IP(従来型・LAN 機器)
    Ip(Ipv4Addr),
    /// メンバー参照(配布時にその時点の仮想 IP へ解決)
    Member(MemberRef),
    /// CNAME(別ドメインの別名。外部ドメイン可 — ADR-0025)
    Cname(String),
}

/// 追加するカスタム DNS レコード(ADR-0022 / ADR-0023)。
pub struct NewRecord<'a> {
    pub name: &'a str,
    pub target: RecordTarget,
    pub under: Option<MemberRef>,
    pub scheme: Option<&'a str>,
    pub port: Option<u16>,
}

/// 一覧表示用に解決済みの情報を添えたレコード。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordDetail {
    pub id: Option<String>,
    /// 正規化済みラベル(設定の `name` そのもの)
    pub name: String,
    /// 親メンバー(端末配下サブドメインのとき)
    pub under: Option<MemberRef>,
    /// under を親ラベルへ解決したドット付き相対名(`web.alice` 等)
    pub relative: String,
    /// 表示用の完全修飾名
    pub fqdn: String,
    pub target: RecordTarget,
    /// member ターゲットを設定から解決した現在の仮想 IP(参照切れは None)
    pub resolved_ip: Option<Ipv4Addr>,
    /// URL コピー用のサービス情報(ADR-0023)。
    pub scheme: Option<String>,
    pub port: Option<u16>,
    /// scheme がある場合に組み立て済みの URL。既定ポートは省略する。
    pub url: Option<String>,
    pub health: HealthSettings,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthSettings {
    pub enabled: bool,
    pub kind: peercove_core::dns::HealthCheckKind,
    pub path: String,
    pub expected_status: Option<u16>,
    pub external: bool,
}

/// メンバー参照の現在の DNS ラベルを設定から引く(表示・相対名の組み立て用。
/// 配布時の正式な解決は core `resolve_records` が台帳から行う)。
fn label_of(config: &Config, reference: &MemberRef) -> Option<String> {
    match reference {
        MemberRef::Host => Some(crate::peers::host_dns_label(config)),
        MemberRef::Key(key) => config
            .peers
            .iter()
            .find(|p| p.public_key == *key)
            .map(crate::peers::peer_dns_label),
    }
}

/// メンバー参照の現在の仮想 IP を設定から引く。
fn ip_of(config: &Config, reference: &MemberRef) -> Option<Ipv4Addr> {
    match reference {
        MemberRef::Host => Some(config.interface.address.addr()),
        MemberRef::Key(key) => config
            .peers
            .iter()
            .find(|p| p.public_key == *key)
            .and_then(|p| p.allowed_ips.first())
            .map(|net| net.addr()),
    }
}

/// 設定のカスタムレコード一覧(表示用の解決情報つき)。
pub fn list_records(config_path: &Path) -> anyhow::Result<Vec<RecordDetail>> {
    let mut config = Config::load(config_path)?;
    // ADR-0035以前のレコードにも、一度だけ安定IDを補う。名前変更後もIDは維持される。
    if config.dns_records.iter().any(|record| record.id.is_none()) {
        let mut doc = load_doc(config_path)?;
        let records = doc["dns_record"]
            .as_array_of_tables_mut()
            .context("dns_record が配列テーブルではありません")?;
        let mut used: std::collections::HashSet<String> = config
            .dns_records
            .iter()
            .filter_map(|record| record.id.clone())
            .collect();
        for (index, table) in records.iter_mut().enumerate() {
            if table.get("id").is_some() {
                continue;
            }
            let mut serial = index + 1;
            let id = loop {
                let candidate = format!("svc-{serial}");
                if used.insert(candidate.clone()) {
                    break candidate;
                }
                serial += 1;
            };
            table.insert("id", toml_edit::value(id));
        }
        write_validated(config_path, &doc.to_string())?;
        config = Config::load(config_path)?;
    }
    let network = config.network_name().to_string();
    Ok(config
        .dns_records
        .iter()
        .map(|record| {
            let relative = match &record.under {
                None => record.name.clone(),
                Some(under) => match label_of(&config, under) {
                    Some(parent) => format!("{}.{parent}", record.name),
                    None => record.name.clone(), // 参照切れ(remove_peer が掃除するので一瞬)
                },
            };
            let (target, resolved_ip) = match (record.ip, &record.member, &record.cname) {
                (Some(ip), _, _) => (RecordTarget::Ip(ip), Some(ip)),
                (None, Some(member), _) => (RecordTarget::Member(*member), ip_of(&config, member)),
                (None, None, Some(cname)) => (RecordTarget::Cname(cname.clone()), None),
                // validate が通っているので来ないが、保守的に IP 0.0.0.0 扱いにしない
                (None, None, None) => (RecordTarget::Ip(Ipv4Addr::UNSPECIFIED), None),
            };
            let fqdn = format!("{relative}.{network}.{DNS_SUFFIX}");
            let url = service_url(&fqdn, record.scheme.as_deref(), record.port);
            let health = HealthSettings {
                enabled: record.health_check.unwrap_or(
                    record.cname.is_none() && record.scheme.is_some() && record.port.is_some(),
                ),
                kind: record
                    .health_kind
                    .unwrap_or(peercove_core::dns::HealthCheckKind::Tcp),
                path: record.health_path.clone().unwrap_or_else(|| "/".into()),
                expected_status: record.health_expect_status,
                external: record.health_external,
            };
            RecordDetail {
                id: record.id.clone(),
                name: record.name.clone(),
                under: record.under,
                fqdn,
                relative,
                target,
                resolved_ip,
                scheme: record.scheme.clone(),
                port: record.port,
                url,
                health,
            }
        })
        .collect())
}

/// 既存レコードのヘルスチェック設定を更新する(M3-14e-b)。
pub fn set_health(
    config_path: &Path,
    name: &str,
    under: Option<MemberRef>,
    settings: &HealthSettings,
) -> anyhow::Result<()> {
    let under_string = under.map(|reference| reference.to_config_string());
    let mut doc = load_doc(config_path)?;
    let records = doc
        .get_mut("dns_record")
        .and_then(|item| item.as_array_of_tables_mut())
        .context("dns_record が配列テーブルではありません")?;
    let table = records
        .iter_mut()
        .find(|table| {
            table
                .get("name")
                .and_then(|value| value.as_str())
                .map(str::trim)
                == Some(name)
                && table
                    .get("under")
                    .and_then(|value| value.as_str())
                    .map(str::trim)
                    == under_string.as_deref()
        })
        .ok_or_else(|| anyhow::anyhow!("レコード \"{name}\" は存在しません"))?;
    for key in [
        "health_check",
        "health_kind",
        "health_path",
        "health_expect_status",
        "health_external",
    ] {
        table.remove(key);
    }
    table.insert("health_check", toml_edit::value(settings.enabled));
    if settings.kind != peercove_core::dns::HealthCheckKind::Tcp {
        table.insert("health_kind", toml_edit::value("http_head"));
    }
    if settings.path != "/" {
        table.insert("health_path", toml_edit::value(settings.path.as_str()));
    }
    if let Some(status) = settings.expected_status {
        table.insert("health_expect_status", toml_edit::value(i64::from(status)));
    }
    if settings.external {
        table.insert("health_external", toml_edit::value(true));
    }
    write_validated(config_path, &doc.to_string())
}

/// カスタムレコードを追加する。`name` は表示名のままでよく、ここで正規化する。
/// 最上位(under なし)は予約語とメンバー名(確定 DNS 名・従来導出ラベル)との
/// 重複を拒否する(ADR-0021 §4 / ADR-0022 §4)。参照先の存在・LAN 機器
/// (ip + under)の広告サブネット内チェックは `Config::validate` が行う。
/// 解決済みの相対名(`web.alice` 等)を返す。
pub fn add_record(config_path: &Path, record: &NewRecord<'_>) -> anyhow::Result<String> {
    // 自由入力(サブドメイン・先頭 * ワイルドカード可)を各ラベル正規化する(ADR-0024)
    let Some(name) = names::normalize_custom_dns_name(record.name) else {
        bail!(
            "\"{}\" から有効な DNS 名を作れませんでした。英数字を含めてください(先頭ラベルのみ * が使えます)",
            record.name
        );
    };
    let scheme = record
        .scheme
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    if let Some(scheme) = &scheme {
        if !is_service_scheme(scheme) {
            bail!(
                "スキーム \"{scheme}\" が不正です(先頭は英字、以降は英数字と + . -、31 文字以内)"
            );
        }
    }
    if record.port == Some(0) {
        bail!("ポートは 1〜65535 で指定してください");
    }
    let config = Config::load(config_path)?;
    if config
        .dns_records
        .iter()
        .any(|r| r.name == name && r.under == record.under)
    {
        bail!("レコード \"{name}\" は既に存在します(削除してから追加し直してください)");
    }
    if record.under.is_none() {
        // 予約語・メンバー名との衝突は、ネットワーク直下のラベル(= 末尾ラベル。
        // `web.app` なら `app`)で判定する(ADR-0022 §4 / ADR-0024)
        let base = name.rsplit('.').next().unwrap_or(name.as_str());
        if names::RESERVED_DNS_LABELS.contains(&base) || base == names::HOST_DNS_LABEL {
            bail!("「{base}」は予約されているためレコード名に使えません");
        }
        // `member-<数字>` は未参加メンバーの自動ラベル用に予約(将来の登録と衝突しうる。
        // 実在メンバー自身の DNS 名としてはメンバー設定側で許可 — ADR-0024)
        if names::is_reserved_member_label(base) {
            bail!("「{base}」は新しく参加するメンバーの自動名と衝突する可能性があるため、レコード名には使えません");
        }
        if crate::peers::taken_dns_labels(&config, crate::peers::DnsExclude::None).contains(base) {
            bail!("DNS 名「{base}」はメンバーが使用しています(別の名前にしてください)");
        }
    }
    let relative = match &record.under {
        None => name.clone(),
        Some(reference) => match label_of(&config, reference) {
            Some(parent) => format!("{name}.{parent}"),
            None => bail!("親に指定したメンバーが登録されていません"),
        },
    };

    let mut doc = load_doc(config_path)?;
    let records = doc["dns_record"]
        .or_insert(toml_edit::Item::ArrayOfTables(Default::default()))
        .as_array_of_tables_mut()
        .context("dns_record が配列テーブルではありません(手編集の可能性)")?;
    let mut table = toml_edit::Table::new();
    let stable_id = format!(
        "svc-{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    table.insert("id", toml_edit::value(stable_id));
    table.insert("name", toml_edit::value(name.as_str()));
    match &record.target {
        RecordTarget::Ip(ip) => {
            table.insert("ip", toml_edit::value(ip.to_string()));
        }
        RecordTarget::Member(member) => {
            table.insert("member", toml_edit::value(member.to_config_string()));
        }
        RecordTarget::Cname(domain) => {
            let Some(target) = names::normalize_cname_target(domain) else {
                bail!("転送先ドメイン \"{domain}\" が不正です(例: docs.example.com)");
            };
            table.insert("cname", toml_edit::value(target));
        }
    }
    if let Some(under) = record.under {
        table.insert("under", toml_edit::value(under.to_config_string()));
    }
    if let Some(scheme) = scheme {
        table.insert("scheme", toml_edit::value(scheme));
    }
    if let Some(port) = record.port {
        table.insert("port", toml_edit::value(i64::from(port)));
    }
    records.push(table);
    write_validated(config_path, &doc.to_string())?;
    Ok(relative)
}

/// カスタムレコードを (name, under) で削除する(ADR-0022: 親が違えば同名可)。
pub fn remove_record(
    config_path: &Path,
    name: &str,
    under: Option<MemberRef>,
) -> anyhow::Result<()> {
    let under_string = under.map(|reference| reference.to_config_string());
    let removed_id = Config::load(config_path)?
        .dns_records
        .into_iter()
        .find(|record| record.name == name && record.under == under)
        .and_then(|record| record.id);
    let mut doc = load_doc(config_path)?;
    let Some(records) = doc
        .get_mut("dns_record")
        .and_then(|item| item.as_array_of_tables_mut())
    else {
        bail!("レコード \"{name}\" は存在しません");
    };
    let before = records.len();
    records.retain(|table| {
        let name_matches = table.get("name").and_then(|v| v.as_str()).map(str::trim) == Some(name);
        let under_matches =
            table.get("under").and_then(|v| v.as_str()).map(str::trim) == under_string.as_deref();
        !(name_matches && under_matches)
    });
    if records.len() == before {
        bail!("レコード \"{name}\" は存在しません");
    }
    if let Some(id) = removed_id {
        if let Some(rules) = doc
            .get_mut("acl")
            .and_then(|item| item.get_mut("rule"))
            .and_then(toml_edit::Item::as_array_of_tables_mut)
        {
            rules.retain(|rule| {
                !["source", "destination"].iter().any(|field| {
                    rule.get(field)
                        .and_then(toml_edit::Item::as_inline_table)
                        .and_then(|target| target.get("service"))
                        .and_then(toml_edit::Value::as_str)
                        == Some(id.as_str())
                })
            });
        }
    }
    write_validated(config_path, &doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("peercove-ops-dns-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::init::init_host(&dir, "home", 51820, false)
            .unwrap()
            .config_path
    }

    fn ip(target: &str) -> RecordTarget {
        RecordTarget::Ip(target.parse().unwrap())
    }

    fn add(
        config: &Path,
        name: &str,
        target: RecordTarget,
        under: Option<MemberRef>,
    ) -> anyhow::Result<String> {
        add_record(
            config,
            &NewRecord {
                name,
                target,
                under,
                scheme: None,
                port: None,
            },
        )
    }

    #[test]
    fn add_list_remove_roundtrip() {
        let config = setup("roundtrip");
        assert!(list_records(&config).unwrap().is_empty());

        // 表示名のままでも正規化される
        let relative = add(&config, "My NAS", ip("10.68.1.50"), None).unwrap();
        assert_eq!(relative, "my-nas");
        add(&config, "printer", ip("10.68.1.51"), None).unwrap();

        let records = list_records(&config).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].name, "my-nas");
        assert_eq!(records[0].relative, "my-nas");
        assert_eq!(records[0].resolved_ip.unwrap().to_string(), "10.68.1.50");
        assert!(records[0].fqdn.starts_with("my-nas.home."));

        // 同じ (name, under) の重複追加は拒否
        assert!(add(&config, "my-nas", ip("10.68.1.52"), None).is_err());

        remove_record(&config, "my-nas", None).unwrap();
        assert_eq!(list_records(&config).unwrap().len(), 1);
        assert!(remove_record(&config, "my-nas", None).is_err(), "二重削除");

        // 設定全体が有効なまま(Config::load が通る)
        Config::load(&config).unwrap();
    }

    #[test]
    fn health_settings_roundtrip_and_validate() {
        let config = setup("health");
        add_record(
            &config,
            &NewRecord {
                name: "web",
                target: ip("10.68.1.50"),
                under: None,
                scheme: Some("http"),
                port: Some(8080),
            },
        )
        .unwrap();
        let settings = HealthSettings {
            enabled: true,
            kind: peercove_core::dns::HealthCheckKind::HttpHead,
            path: "/ready".into(),
            expected_status: Some(204),
            external: false,
        };
        set_health(&config, "web", None, &settings).unwrap();
        let records = list_records(&config).unwrap();
        assert_eq!(records[0].health, settings);
        Config::load(&config).unwrap();

        let mut invalid = records[0].health.clone();
        invalid.path = "no-leading-slash".into();
        assert!(set_health(&config, "web", None, &invalid).is_err());
    }

    #[test]
    fn removing_service_cleans_acl_reference() {
        let config = setup("acl-service-remove");
        add_record(
            &config,
            &NewRecord {
                name: "web",
                target: ip("10.68.1.50"),
                under: None,
                scheme: Some("http"),
                port: Some(8080),
            },
        )
        .unwrap();
        let id = list_records(&config).unwrap()[0].id.clone().unwrap();
        crate::acl::write_policy(
            &config,
            &crate::acl::PolicySettings {
                default: peercove_core::acl::AclAction::Allow,
                groups: vec![],
                rules: vec![peercove_core::acl::AclRule {
                    id: "deny-web".into(),
                    action: peercove_core::acl::AclAction::Deny,
                    source: peercove_core::acl::AclTarget::Any("any".into()),
                    destination: peercove_core::acl::AclTarget::Service { service: id },
                    protocol: peercove_core::acl::AclProtocol::Tcp,
                    ports: vec!["8080".into()],
                    enabled: true,
                }],
            },
        )
        .unwrap();
        remove_record(&config, "web", None).unwrap();
        assert!(Config::load(&config).unwrap().acl.rules.is_empty());
    }

    #[test]
    fn add_preserves_comments_and_rejects_unusable_names() {
        let config = setup("comments");
        // 手書きコメントが消えないこと(toml_edit の目的)
        let text = std::fs::read_to_string(&config).unwrap();
        std::fs::write(&config, format!("# 大事なコメント\n{text}")).unwrap();
        add(&config, "nas", ip("10.68.1.50"), None).unwrap();
        assert!(std::fs::read_to_string(&config)
            .unwrap()
            .contains("# 大事なコメント"));

        assert!(add(&config, "たろう", ip("10.68.1.53"), None).is_err());
    }

    #[test]
    fn service_info_is_normalized_and_builds_urls() {
        let config = setup("service-info");
        add_record(
            &config,
            &NewRecord {
                name: "gamehost",
                target: ip("10.68.1.50"),
                under: None,
                scheme: Some("HTTP"),
                port: Some(8080),
            },
        )
        .unwrap();
        add_record(
            &config,
            &NewRecord {
                name: "secure",
                target: ip("10.68.1.51"),
                under: None,
                scheme: Some("https"),
                port: Some(443),
            },
        )
        .unwrap();
        add_record(
            &config,
            &NewRecord {
                name: "port-only",
                target: ip("10.68.1.52"),
                under: None,
                scheme: None,
                port: Some(9000),
            },
        )
        .unwrap();

        let parsed = Config::load(&config).unwrap();
        assert_eq!(parsed.dns_records[0].scheme.as_deref(), Some("http"));
        let records = list_records(&config).unwrap();
        assert_eq!(
            records[0].url.as_deref(),
            Some("http://gamehost.home.peercove.internal:8080/")
        );
        assert_eq!(
            records[1].url.as_deref(),
            Some("https://secure.home.peercove.internal/")
        );
        assert_eq!(records[2].url, None);
        assert_eq!(records[2].port, Some(9000));

        for (scheme, port) in [
            (Some("1http"), None),
            (Some("http_test"), None),
            (None, Some(0)),
        ] {
            assert!(add_record(
                &config,
                &NewRecord {
                    name: "bad-service",
                    target: ip("10.68.1.99"),
                    under: None,
                    scheme,
                    port,
                },
            )
            .is_err());
        }
    }

    /// 予約語とメンバー名(確定 DNS 名 / 従来導出)との重複を拒否する(ADR-0021)。
    #[test]
    fn add_rejects_reserved_and_member_labels() {
        let config = setup("reserved");
        assert!(add(&config, "localhost", ip("10.68.1.50"), None).is_err());
        assert!(
            add(&config, "host", ip("10.68.1.50"), None).is_err(),
            "ホストの従来導出ラベルと衝突"
        );

        let result = crate::invite::invite(&crate::invite::InviteOptions {
            config_path: &config,
            name: Some("alice"),
            ip: None,
            extra_endpoints: &[],
            psk: false,
            expires_in_secs: None,
        });
        // init 環境ではエンドポイント検出に失敗する場合があるためスキップ可
        if result.is_ok() {
            assert!(
                add(&config, "alice", ip("10.68.1.50"), None).is_err(),
                "メンバーの確定 DNS 名と衝突"
            );
        }
    }

    /// 拡張レコード(ADR-0022): エイリアス・サブドメイン・LAN 機器の追加と
    /// 検証(参照切れ・広告サブネット外・親ごとの一意性)。
    #[test]
    fn member_targets_and_subdomains() {
        let config = setup("member-targets");
        let alice = peercove_core::keys::PrivateKey::generate().public_key();
        crate::peers::append_peer(
            &config,
            &crate::peers::NewPeer {
                public_key: alice,
                ip: {
                    let parsed = Config::load(&config).unwrap();
                    parsed.interface.address.trunc().hosts().nth(1).unwrap()
                },
                name: Some("山田"),
                dns_name: Some("alice"),
                preshared_key_file: None,
                invite_id: None,
                invite_issued_at: None,
                invite_expires_at: None,
            },
        )
        .unwrap();
        crate::peers::set_subnets(
            &config,
            &crate::peers::Selector::PublicKey(&alice.to_base64()),
            &["192.168.10.0/24".parse().unwrap()],
        )
        .unwrap();

        // エイリアス(member ターゲット)。解決 IP は alice の仮想 IP
        let relative = add(
            &config,
            "gamehost",
            RecordTarget::Member(MemberRef::Key(alice)),
            None,
        )
        .unwrap();
        assert_eq!(relative, "gamehost");
        let records = list_records(&config).unwrap();
        assert_eq!(
            records[0].resolved_ip,
            Config::load(&config).unwrap().peers[0]
                .allowed_ips
                .first()
                .map(|net| net.addr())
        );

        // ホスト配下のサブドメイン
        let relative = add(
            &config,
            "web",
            RecordTarget::Member(MemberRef::Host),
            Some(MemberRef::Host),
        )
        .unwrap();
        assert_eq!(relative, "web.host");
        // 親が違えば同名可
        let relative = add(
            &config,
            "web",
            RecordTarget::Member(MemberRef::Key(alice)),
            Some(MemberRef::Key(alice)),
        )
        .unwrap();
        assert_eq!(relative, "web.alice");
        // 同じ親なら重複拒否
        assert!(add(
            &config,
            "web",
            RecordTarget::Member(MemberRef::Host),
            Some(MemberRef::Host),
        )
        .is_err());
        // under 付きは予約語チェックの対象外
        add(
            &config,
            "dns",
            RecordTarget::Member(MemberRef::Key(alice)),
            Some(MemberRef::Key(alice)),
        )
        .unwrap();

        // LAN 機器: 広告サブネット内は可、外・ホスト配下は不可
        add(
            &config,
            "printer",
            ip("192.168.10.50"),
            Some(MemberRef::Key(alice)),
        )
        .unwrap();
        assert!(add(
            &config,
            "cam",
            ip("192.168.99.50"),
            Some(MemberRef::Key(alice)),
        )
        .is_err());
        assert!(add(&config, "cam", ip("192.168.10.51"), Some(MemberRef::Host)).is_err());

        // 未登録メンバーへの参照は不可
        let stranger = peercove_core::keys::PrivateKey::generate().public_key();
        assert!(add(
            &config,
            "x",
            RecordTarget::Member(MemberRef::Key(stranger)),
            None
        )
        .is_err());

        // (name, under) 指定の削除: web.alice だけ消え web.host は残る
        remove_record(&config, "web", Some(MemberRef::Key(alice))).unwrap();
        let names: Vec<String> = list_records(&config)
            .unwrap()
            .iter()
            .map(|r| r.relative.clone())
            .collect();
        assert!(names.contains(&"web.host".to_string()));
        assert!(!names.contains(&"web.alice".to_string()));
    }
}
