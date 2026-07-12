//! カスタム DNS レコードの設定ファイル操作(ADR-0011 §1b、M3-1)。
//!
//! ホストの設定に `[[dns_record]]` を追加・削除する。実行中のホストは 5 秒の
//! 再読込で拾い、台帳と一緒にメンバーへ配布する(peers.rs と同じ反映経路)。
//! 表示は持たず、UI / CLI 双方から使う(ADR-0008)。

use std::net::Ipv4Addr;
use std::path::Path;

use anyhow::{bail, Context};
use peercove_core::config::Config;
use peercove_core::dns::DnsRecord;
use peercove_core::names;

use crate::peers::{load_doc, write_validated};

/// 設定のカスタムレコード一覧。
pub fn list_records(config_path: &Path) -> anyhow::Result<Vec<DnsRecord>> {
    Ok(Config::load(config_path)?.dns_records)
}

/// カスタムレコードを追加する。`name` は表示名のままでよく、ここで正規化する。
/// 予約語とメンバー名(確定 DNS 名・従来導出ラベル)との重複は拒否する
/// (ADR-0021 §4)。正規化後のラベルを返す。
pub fn add_record(config_path: &Path, name: &str, ip: Ipv4Addr) -> anyhow::Result<String> {
    let Some(label) = names::dns_label(name) else {
        bail!("\"{name}\" から有効なラベルを作れませんでした。半角英数字を含めてください");
    };
    if names::RESERVED_DNS_LABELS.contains(&label.as_str()) {
        bail!("「{label}」は予約されているためレコード名に使えません");
    }
    let config = Config::load(config_path)?;
    if config.dns_records.iter().any(|r| r.name == label) {
        bail!("レコード \"{label}\" は既に存在します(削除してから追加し直してください)");
    }
    if crate::peers::taken_dns_labels(&config, crate::peers::DnsExclude::None).contains(&label) {
        bail!("DNS 名「{label}」はメンバーが使用しています(別の名前にしてください)");
    }

    let mut doc = load_doc(config_path)?;
    let records = doc["dns_record"]
        .or_insert(toml_edit::Item::ArrayOfTables(Default::default()))
        .as_array_of_tables_mut()
        .context("dns_record が配列テーブルではありません(手編集の可能性)")?;
    let mut table = toml_edit::Table::new();
    table.insert("name", toml_edit::value(label.as_str()));
    table.insert("ip", toml_edit::value(ip.to_string()));
    records.push(table);
    write_validated(config_path, &doc.to_string())?;
    Ok(label)
}

/// カスタムレコードをラベルで削除する。
pub fn remove_record(config_path: &Path, name: &str) -> anyhow::Result<()> {
    let mut doc = load_doc(config_path)?;
    let Some(records) = doc
        .get_mut("dns_record")
        .and_then(|item| item.as_array_of_tables_mut())
    else {
        bail!("レコード \"{name}\" は存在しません");
    };
    let before = records.len();
    records.retain(|table| table.get("name").and_then(|v| v.as_str()).map(str::trim) != Some(name));
    if records.len() == before {
        bail!("レコード \"{name}\" は存在しません");
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

    #[test]
    fn add_list_remove_roundtrip() {
        let config = setup("roundtrip");
        assert!(list_records(&config).unwrap().is_empty());

        // 表示名のままでも正規化される
        let label = add_record(&config, "My NAS", "10.68.1.50".parse().unwrap()).unwrap();
        assert_eq!(label, "my-nas");
        add_record(&config, "printer", "10.68.1.51".parse().unwrap()).unwrap();

        let records = list_records(&config).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].name, "my-nas");
        assert_eq!(records[0].ip.to_string(), "10.68.1.50");

        // 重複追加は拒否
        assert!(add_record(&config, "my-nas", "10.68.1.52".parse().unwrap()).is_err());

        remove_record(&config, "my-nas").unwrap();
        assert_eq!(list_records(&config).unwrap().len(), 1);
        assert!(remove_record(&config, "my-nas").is_err(), "二重削除");

        // 設定全体が有効なまま(Config::load が通る)
        Config::load(&config).unwrap();
    }

    #[test]
    fn add_preserves_comments_and_rejects_unusable_names() {
        let config = setup("comments");
        // 手書きコメントが消えないこと(toml_edit の目的)
        let text = std::fs::read_to_string(&config).unwrap();
        std::fs::write(&config, format!("# 大事なコメント\n{text}")).unwrap();
        add_record(&config, "nas", "10.68.1.50".parse().unwrap()).unwrap();
        assert!(std::fs::read_to_string(&config)
            .unwrap()
            .contains("# 大事なコメント"));

        assert!(add_record(&config, "たろう", "10.68.1.53".parse().unwrap()).is_err());
    }

    /// 予約語とメンバー名(確定 DNS 名 / 従来導出)との重複を拒否する(ADR-0021)。
    #[test]
    fn add_rejects_reserved_and_member_labels() {
        let config = setup("reserved");
        assert!(add_record(&config, "localhost", "10.68.1.50".parse().unwrap()).is_err());
        assert!(
            add_record(&config, "host", "10.68.1.50".parse().unwrap()).is_err(),
            "ホストの従来導出ラベルと衝突"
        );

        let result = crate::invite::invite(&crate::invite::InviteOptions {
            config_path: &config,
            name: Some("alice"),
            ip: None,
            extra_endpoints: &[],
            psk: false,
        });
        // init 環境ではエンドポイント検出に失敗する場合があるためスキップ可
        if result.is_ok() {
            assert!(
                add_record(&config, "alice", "10.68.1.50".parse().unwrap()).is_err(),
                "メンバーの確定 DNS 名と衝突"
            );
        }
    }
}
