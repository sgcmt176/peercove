//! ホスト設定 `[[peer]]` の追加・削除・名前変更(ADR-0002 / 0005 / 0008)。
//!
//! いずれも host.toml を書き換えるだけ。実行中の host プロセスは定期再読込で
//! 追随する(追加・変更は約 5 秒、削除は通知 → 実削除で約 10 秒)。
//! コメントや整形を保持するため、削除・変更には `toml_edit` を使う。

use std::net::Ipv4Addr;
use std::path::Path;

use anyhow::{bail, Context};
use peercove_core::config::{Config, PeerConfig};
use peercove_core::ipalloc::next_free_ip;
use peercove_core::keys::PublicKey;

/// 追加するピアの内容。
pub struct NewPeer<'a> {
    pub public_key: PublicKey,
    pub ip: Ipv4Addr,
    /// 台帳用の表示名(invite 経由のとき)
    pub name: Option<&'a str>,
    /// 確定済みの DNS 名(ADR-0021、M3-14a)。invite が IP 割当と同時に決める。
    pub dns_name: Option<&'a str>,
    /// 設定ファイルからの相対パス(invite --psk のとき)
    pub preshared_key_file: Option<&'a str>,
}

/// 削除・変更対象の指定(いずれか 1 つ)。
pub enum Selector<'a> {
    Name(&'a str),
    PublicKey(&'a str),
    Ip(Ipv4Addr),
}

/// ホスト自身 + 登録済みピアの使用中 IP。
pub fn used_ips(config: &Config) -> impl Iterator<Item = Ipv4Addr> + '_ {
    std::iter::once(config.interface.address.addr()).chain(
        config
            .peers
            .iter()
            .flat_map(|p| p.allowed_ips.iter().map(|net| net.addr())),
    )
}

/// TOML の基本文字列としてエスケープする(Rust のエスケープは TOML basic string と互換)。
fn toml_string(value: &str) -> String {
    format!("{value:?}")
}

/// ホスト設定へ `[[peer]]` ブロックを追記する。
///
/// TOML 全体を再シリアライズするとコメントが失われるため、テキスト追記方式にする。
pub fn append_peer(config_path: &Path, peer: &NewPeer) -> anyhow::Result<()> {
    let config = Config::load(config_path)?;
    let NewPeer { public_key, ip, .. } = *peer;

    let subnet = config.interface.address.trunc();
    if !subnet.contains(&ip) {
        bail!("IP {ip} はトンネルのサブネット {subnet} の範囲外です");
    }
    if ip == config.interface.address.addr() {
        bail!("IP {ip} はホスト自身のアドレスです");
    }
    if config.peers.iter().any(|p| p.public_key == public_key) {
        bail!("公開鍵 {public_key} のピアは既に登録されています");
    }
    let used: Vec<Ipv4Addr> = used_ips(&config).collect();
    if used.contains(&ip) {
        let suggestion = next_free_ip(subnet, &used)
            .map(|free| format!("(空きの例: {free})"))
            .unwrap_or_default();
        bail!("IP {ip} は使用済みです{suggestion}");
    }

    let original = std::fs::read_to_string(config_path)
        .with_context(|| format!("{} の読み込みに失敗しました", config_path.display()))?;
    let mut block = String::from("\n[[peer]]\n");
    if let Some(name) = peer.name {
        block.push_str(&format!("name = {}\n", toml_string(name)));
    }
    if let Some(dns_name) = peer.dns_name {
        block.push_str(&format!("dns_name = {}\n", toml_string(dns_name)));
    }
    block.push_str(&format!("public_key = \"{public_key}\"\n"));
    block.push_str(&format!("allowed_ips = [\"{ip}/32\"]\n"));
    if let Some(psk) = peer.preshared_key_file {
        block.push_str(&format!("preshared_key_file = {}\n", toml_string(psk)));
    }
    let updated = format!("{original}{block}");
    write_validated(config_path, &updated)
}

/// 書き込む前に、結果が正しく解析・検証できることを確認する。
pub(crate) fn write_validated(config_path: &Path, text: &str) -> anyhow::Result<()> {
    let parsed: Config = toml::from_str(text).context("編集結果の TOML が不正です")?;
    parsed.validate()?;
    std::fs::write(config_path, text)
        .with_context(|| format!("{} の書き込みに失敗しました", config_path.display()))
}

/// セレクタに一致するピアを 1 つだけ返す。
pub fn find_peer<'a>(config: &'a Config, selector: &Selector) -> anyhow::Result<&'a PeerConfig> {
    let matches: Vec<&PeerConfig> = config
        .peers
        .iter()
        .filter(|peer| match selector {
            Selector::Name(name) => peer.name.as_deref() == Some(*name),
            Selector::PublicKey(key) => peer.public_key.to_base64() == key.trim(),
            Selector::Ip(ip) => peer.allowed_ips.first().map(|net| net.addr()) == Some(*ip),
        })
        .collect();
    match matches.as_slice() {
        [peer] => Ok(peer),
        [] => {
            let known: Vec<String> = config
                .peers
                .iter()
                .map(|p| {
                    format!(
                        "{}({})",
                        p.name.as_deref().unwrap_or("名前なし"),
                        p.allowed_ips
                            .first()
                            .map(|net| net.addr().to_string())
                            .unwrap_or_default()
                    )
                })
                .collect();
            bail!(
                "対象のピアが見つかりません。登録済み: {}",
                if known.is_empty() {
                    "(なし)".to_string()
                } else {
                    known.join(", ")
                }
            )
        }
        _ => bail!("複数のピアに一致しました。公開鍵で一意に指定してください"),
    }
}

/// host.toml から該当 `[[peer]]` の toml_edit テーブルを引く。
fn peer_tables(doc: &mut toml_edit::DocumentMut) -> anyhow::Result<&mut toml_edit::ArrayOfTables> {
    doc.get_mut("peer")
        .and_then(|item| item.as_array_of_tables_mut())
        .context("[[peer]] が見つかりません")
}

pub(crate) fn load_doc(config_path: &Path) -> anyhow::Result<toml_edit::DocumentMut> {
    std::fs::read_to_string(config_path)
        .with_context(|| format!("{} の読み込みに失敗しました", config_path.display()))?
        .parse()
        .context("host.toml の解析に失敗しました(手編集の構文エラー?)")
}

pub struct RemovedPeer {
    /// 表示用の名前(無ければ公開鍵)
    pub display: String,
    pub public_key: PublicKey,
    /// 併せて削除したホスト側 PSK ファイル
    pub removed_psk_file: Option<std::path::PathBuf>,
}

/// メンバーを削除する。ホスト側 PSK ファイルも片付ける。
pub fn remove_peer(config_path: &Path, selector: &Selector) -> anyhow::Result<RemovedPeer> {
    let config = Config::load(config_path)?;
    let target = find_peer(&config, selector)?;
    let public_key = target.public_key;
    let target_key = public_key.to_base64();
    let display = target
        .name
        .clone()
        .unwrap_or_else(|| public_key.to_string());
    let psk_file = target.preshared_key_file.clone();

    let removed_ip = target.allowed_ips.first().map(|net| net.addr());

    let mut doc = load_doc(config_path)?;
    let peers = peer_tables(&mut doc)?;
    let before = peers.len();
    peers.retain(|table| {
        table
            .get("public_key")
            .and_then(|v| v.as_str())
            .map(str::trim)
            != Some(target_key.as_str())
    });
    if peers.len() != before - 1 {
        bail!(
            "host.toml から対象ピアを特定できませんでした(手編集で public_key が変わっている可能性)"
        );
    }
    // ACL(ADR-0018)に残った組も掃除する(残しても効果はないが紛らわしい)
    if let Some(ip) = removed_ip {
        if config.acl.deny.iter().any(|&(a, b)| a == ip || b == ip) {
            let keep: Vec<_> = config
                .acl
                .normalized_deny()
                .into_iter()
                .filter(|&(a, b)| a != ip && b != ip)
                .collect();
            crate::acl::write_deny(&mut doc, &keep);
        }
    }
    // 削除メンバーを参照するカスタムレコードも掃除する(ADR-0022 §5。
    // 残すと Config::validate の参照先チェックで設定全体が壊れる)
    if let Some(records) = doc
        .get_mut("dns_record")
        .and_then(|item| item.as_array_of_tables_mut())
    {
        records.retain(|table| {
            !["member", "under"].iter().any(|field| {
                table.get(field).and_then(|v| v.as_str()).map(str::trim)
                    == Some(target_key.as_str())
            })
        });
        if records.is_empty() {
            doc.remove("dns_record");
        }
    }
    write_validated(config_path, &doc.to_string())?;

    let removed_psk_file = psk_file.and_then(|path| match std::fs::remove_file(&path) {
        Ok(()) => Some(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            tracing::warn!("PSK ファイルの削除に失敗しました: {e}");
            None
        }
    });

    Ok(RemovedPeer {
        display,
        public_key,
        removed_psk_file,
    })
}

/// メンバーの表示名を変更する(台帳に反映される)。
pub fn rename_peer(config_path: &Path, selector: &Selector, new_name: &str) -> anyhow::Result<()> {
    crate::invite::validate_name(new_name)?;
    let config = Config::load(config_path)?;
    let target = find_peer(&config, selector)?;
    let target_key = target.public_key.to_base64();
    if config
        .peers
        .iter()
        .any(|p| p.name.as_deref() == Some(new_name) && p.public_key != target.public_key)
    {
        bail!("名前 {new_name} は既に使われています");
    }

    let mut doc = load_doc(config_path)?;
    let mut renamed = false;
    for table in peer_tables(&mut doc)?.iter_mut() {
        let matches = table
            .get("public_key")
            .and_then(|v| v.as_str())
            .map(str::trim)
            == Some(target_key.as_str());
        if matches {
            table["name"] = toml_edit::value(new_name);
            renamed = true;
        }
    }
    if !renamed {
        bail!("host.toml から対象ピアを特定できませんでした");
    }
    write_validated(config_path, &doc.to_string())
}

/// メンバーの広告サブネット(ADR-0014、M3-7)を設定する。空スライスで解除。
/// 重複・仮想サブネットとの衝突などの検証は `write_validated`(Config::validate)
/// に任せる。戻り値は表示名(CLI / UI のメッセージ用)。
pub fn set_subnets(
    config_path: &Path,
    selector: &Selector,
    subnets: &[ipnet::Ipv4Net],
) -> anyhow::Result<String> {
    let config = Config::load(config_path)?;
    let target = find_peer(&config, selector)?;
    let target_key = target.public_key.to_base64();
    let display = target.name.clone().unwrap_or_else(|| target_key.clone());

    let mut doc = load_doc(config_path)?;
    let mut updated = false;
    for table in peer_tables(&mut doc)?.iter_mut() {
        let matches = table
            .get("public_key")
            .and_then(|v| v.as_str())
            .map(str::trim)
            == Some(target_key.as_str());
        if matches {
            if subnets.is_empty() {
                table.remove("subnets");
            } else {
                let array: toml_edit::Array =
                    subnets.iter().map(|subnet| subnet.to_string()).collect();
                table["subnets"] = toml_edit::value(array);
            }
            updated = true;
        }
    }
    if !updated {
        bail!("host.toml から対象ピアを特定できませんでした");
    }
    write_validated(config_path, &doc.to_string())?;
    Ok(display)
}

/// 鍵ローテーション(ADR-0020、M3-11)の適用結果。
#[derive(Debug, PartialEq, Eq)]
pub enum RotateOutcome {
    /// host.toml の public_key を書き換えた(表示名を返す)。
    Applied { display: String },
    /// 既に同じ鍵が登録されていた(冪等 — 再送された依頼)。
    Unchanged,
}

/// メンバーのデバイス公開鍵を差し替える(ADR-0020、M3-11)。
/// ホストデーモンがコントロールチャネルの `rotate_key` を受けて呼ぶ。
/// name / IP / subnets / PSK / ACL は変更しない(PSK は静的鍵と独立)。
pub fn rotate_peer_key(
    config_path: &Path,
    member_ip: Ipv4Addr,
    new_key: &PublicKey,
    host_public_key: &PublicKey,
) -> anyhow::Result<RotateOutcome> {
    let config = Config::load(config_path)?;
    let target = find_peer(&config, &Selector::Ip(member_ip))?;
    if target.public_key == *new_key {
        return Ok(RotateOutcome::Unchanged);
    }
    if *new_key == *host_public_key {
        bail!("新しい公開鍵がホストの公開鍵と同じです");
    }
    if config.peers.iter().any(|p| p.public_key == *new_key) {
        bail!("新しい公開鍵は別のメンバーが使用中です");
    }
    let target_key = target.public_key.to_base64();
    let display = target.name.clone().unwrap_or_else(|| member_ip.to_string());

    let mut doc = load_doc(config_path)?;
    let mut updated = false;
    for table in peer_tables(&mut doc)?.iter_mut() {
        let matches = table
            .get("public_key")
            .and_then(|v| v.as_str())
            .map(str::trim)
            == Some(target_key.as_str());
        if matches {
            table["public_key"] = toml_edit::value(new_key.to_base64());
            updated = true;
        }
    }
    if !updated {
        bail!("host.toml から対象ピアを特定できませんでした");
    }
    // カスタムレコードのメンバー参照(ADR-0022)も同じ書き込みで追随させる
    if let Some(records) = doc
        .get_mut("dns_record")
        .and_then(|item| item.as_array_of_tables_mut())
    {
        for table in records.iter_mut() {
            for field in ["member", "under"] {
                if table.get(field).and_then(|v| v.as_str()).map(str::trim)
                    == Some(target_key.as_str())
                {
                    table[field] = toml_edit::value(new_key.to_base64());
                }
            }
        }
    }
    write_validated(config_path, &doc.to_string())?;
    Ok(RotateOutcome::Applied { display })
}

/// DNS 名の重複検証(ADR-0021 §4)で除外する対象(自分自身の現名は衝突にしない)。
#[derive(Clone, Copy)]
pub enum DnsExclude<'a> {
    None,
    Host,
    Peer(&'a PublicKey),
}

/// ホスト自身の現在の DNS ラベル(確定 dns_name → 表示名の正規化 → `host`)。
/// zone_for の従来導出と同じ規則(表示・重複検証用)。
pub fn host_dns_label(config: &Config) -> String {
    use peercove_core::names;
    config.interface.dns_name.clone().unwrap_or_else(|| {
        config
            .interface
            .display_name
            .as_deref()
            .and_then(names::dns_label)
            .unwrap_or_else(|| names::HOST_DNS_LABEL.to_string())
    })
}

/// ピアの現在の DNS ラベル(確定 dns_name → 表示名の正規化 → `member-<oct>`)。
pub fn peer_dns_label(peer: &PeerConfig) -> String {
    use peercove_core::names;
    peer.dns_name
        .clone()
        .or_else(|| peer.name.as_deref().and_then(names::dns_label))
        .unwrap_or_else(|| {
            let oct = peer
                .allowed_ips
                .first()
                .map(|net| net.addr().octets()[3])
                .unwrap_or(0);
            format!("member-{oct}")
        })
}

/// ネットワーク内で使用中の DNS ラベル一覧(ADR-0021 の重複検証用)。
///
/// 確定済みの `dns_name` に加え、**未確定エントリの従来導出ラベル**
/// (表示名の正規化 → 空なら host / member-<第4オクテット>)と
/// カスタムレコード名(最上位のみ — under 付きは親配下なので衝突しない、
/// ADR-0022)も含める。従来導出で現に解決できている名前を、
/// 明示設定が奪えないようにするため。
pub fn taken_dns_labels(config: &Config, exclude: DnsExclude) -> std::collections::HashSet<String> {
    let mut taken = std::collections::HashSet::new();
    if !matches!(exclude, DnsExclude::Host) {
        taken.insert(host_dns_label(config));
    }
    for peer in &config.peers {
        if matches!(exclude, DnsExclude::Peer(key) if *key == peer.public_key) {
            continue;
        }
        taken.insert(peer_dns_label(peer));
    }
    for record in &config.dns_records {
        if record.under.is_none() {
            taken.insert(record.name.clone());
        }
    }
    taken
}

/// DNS 名変更(ADR-0021、M3-14a)の適用結果。
#[derive(Debug, PartialEq, Eq)]
pub enum DnsNameOutcome {
    /// host.toml の dns_name を書き換えた。
    Applied { display: String, label: String },
    /// 既に同じ名前だった(冪等 — 再送された依頼)。
    Unchanged { label: String },
}

/// メンバーの DNS 名を設定する(ADR-0021、M3-14a)。
/// 入力は正規化(小文字化・空白/記号 → ハイフン)してから予約語・重複を検証する。
pub fn set_peer_dns_name(
    config_path: &Path,
    selector: &Selector,
    input: &str,
) -> anyhow::Result<DnsNameOutcome> {
    let label = peercove_core::names::normalize_dns_name(input, false)?;
    let config = Config::load(config_path)?;
    let target = find_peer(&config, selector)?;
    let display = target.name.clone().unwrap_or_else(|| label.clone());
    if target.dns_name.as_deref() == Some(label.as_str()) {
        return Ok(DnsNameOutcome::Unchanged { label });
    }
    if taken_dns_labels(&config, DnsExclude::Peer(&target.public_key)).contains(&label) {
        bail!("DNS 名「{label}」はこのネットワークで既に使用されています");
    }
    let target_key = target.public_key.to_base64();

    let mut doc = load_doc(config_path)?;
    let mut updated = false;
    for table in peer_tables(&mut doc)?.iter_mut() {
        let matches = table
            .get("public_key")
            .and_then(|v| v.as_str())
            .map(str::trim)
            == Some(target_key.as_str());
        if matches {
            table["dns_name"] = toml_edit::value(&label);
            updated = true;
        }
    }
    if !updated {
        bail!("host.toml から対象ピアを特定できませんでした");
    }
    write_validated(config_path, &doc.to_string())?;
    Ok(DnsNameOutcome::Applied { display, label })
}

/// コントロールチャネル経由(本人の仮想 IP で特定)の入り口(ADR-0021)。
/// ホストデーモンが `set_dns_name` を受けて呼ぶ。
pub fn set_peer_dns_name_by_ip(
    config_path: &Path,
    member_ip: Ipv4Addr,
    input: &str,
) -> anyhow::Result<DnsNameOutcome> {
    set_peer_dns_name(config_path, &Selector::Ip(member_ip), input)
}

/// ホスト自身の DNS 名を設定する(ADR-0021)。戻り値は確定したラベル。
pub fn set_host_dns_name(config_path: &Path, input: &str) -> anyhow::Result<String> {
    let label = peercove_core::names::normalize_dns_name(input, true)?;
    let config = Config::load(config_path)?;
    if config.interface.dns_name.as_deref() == Some(label.as_str()) {
        return Ok(label);
    }
    if taken_dns_labels(&config, DnsExclude::Host).contains(&label) {
        bail!("DNS 名「{label}」はこのネットワークで既に使用されています");
    }
    let mut doc = load_doc(config_path)?;
    let interface = doc
        .get_mut("interface")
        .and_then(|item| item.as_table_mut())
        .context("[interface] が見つかりません")?;
    interface.insert("dns_name", toml_edit::value(&label));
    write_validated(config_path, &doc.to_string())?;
    Ok(label)
}

#[cfg(test)]
mod tests {
    use super::*;
    use peercove_core::keys::PrivateKey;

    fn setup(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("peercove-ops-peers-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("host.toml");
        std::fs::write(
            &config,
            "# ホスト設定のコメント\n[interface]\nprivate_key_file = \"host.key\"\naddress = \"10.100.42.1/24\"\nlisten_port = 51820\n",
        )
        .unwrap();
        crate::secret::write_secret(&dir.join("host.key"), &PrivateKey::generate().to_base64())
            .unwrap();
        config
    }

    fn add(config: &Path, name: &str, ip: &str) -> PublicKey {
        let key = PrivateKey::generate().public_key();
        append_peer(
            config,
            &NewPeer {
                public_key: key,
                ip: ip.parse().unwrap(),
                name: Some(name),
                dns_name: None,
                preshared_key_file: None,
            },
        )
        .unwrap();
        key
    }

    #[test]
    fn append_remove_preserves_comments_and_other_peers() {
        let config = setup("crud");
        add(&config, "alice", "10.100.42.2");
        add(&config, "bob", "10.100.42.3");

        let removed = remove_peer(&config, &Selector::Name("alice")).unwrap();
        assert_eq!(removed.display, "alice");

        let text = std::fs::read_to_string(&config).unwrap();
        assert!(text.starts_with("# ホスト設定のコメント"));
        let parsed = Config::load(&config).unwrap();
        assert_eq!(parsed.peers.len(), 1);
        assert_eq!(parsed.peers[0].name.as_deref(), Some("bob"));
    }

    /// invite --psk が作ったホスト側 PSK ファイルも一緒に片付ける。
    #[test]
    fn remove_deletes_host_side_psk_file() {
        let config = setup("psk");
        let result = crate::invite::invite(&crate::invite::InviteOptions {
            config_path: &config,
            name: Some("alice"),
            ip: None,
            extra_endpoints: &[],
            psk: true,
        })
        .unwrap();

        let parsed = Config::load(&config).unwrap();
        let psk_path = parsed.peers[0].preshared_key_file.clone().unwrap();
        assert!(psk_path.exists());

        let removed = remove_peer(&config, &Selector::Ip(result.ip)).unwrap();
        assert_eq!(
            removed.removed_psk_file.as_deref(),
            Some(psk_path.as_path())
        );
        assert!(!psk_path.exists(), "PSK ファイルが削除される");
    }

    #[test]
    fn rename_updates_name_only() {
        let config = setup("rename");
        let key = add(&config, "alice", "10.100.42.2");
        add(&config, "bob", "10.100.42.3");

        rename_peer(&config, &Selector::PublicKey(&key.to_base64()), "アリス").unwrap();
        let parsed = Config::load(&config).unwrap();
        assert_eq!(parsed.peers[0].name.as_deref(), Some("アリス"));
        assert_eq!(parsed.peers[0].public_key, key);
        assert_eq!(parsed.peers[1].name.as_deref(), Some("bob"));

        // 既存の名前とは衝突させない
        assert!(rename_peer(&config, &Selector::Name("アリス"), "bob").is_err());
        // 不正な名前は拒否
        assert!(rename_peer(&config, &Selector::Name("アリス"), "").is_err());
    }

    #[test]
    fn append_rejects_duplicates_and_out_of_subnet() {
        let config = setup("reject");
        let key = add(&config, "alice", "10.100.42.2");
        let dup = NewPeer {
            public_key: key,
            ip: "10.100.42.9".parse().unwrap(),
            name: None,
            dns_name: None,
            preshared_key_file: None,
        };
        assert!(append_peer(&config, &dup).is_err(), "公開鍵の重複");

        let other = PrivateKey::generate().public_key();
        for bad_ip in ["10.100.42.2", "10.100.42.1", "10.100.43.9"] {
            let peer = NewPeer {
                public_key: other,
                ip: bad_ip.parse().unwrap(),
                name: None,
                dns_name: None,
                preshared_key_file: None,
            };
            assert!(
                append_peer(&config, &peer).is_err(),
                "{bad_ip} は拒否される"
            );
        }
    }

    /// 鍵ローテーション(ADR-0020): 対象の public_key だけが変わり、name /
    /// allowed_ips / コメントは保持される。同じ鍵の再送は Unchanged(冪等)。
    #[test]
    fn rotate_peer_key_swaps_only_the_target_key() {
        let config = setup("rotate");
        let alice = add(&config, "alice", "10.100.42.2");
        let bob = add(&config, "bob", "10.100.42.3");
        let host_key = PrivateKey::generate().public_key();

        let new_key = PrivateKey::generate().public_key();
        let outcome =
            rotate_peer_key(&config, "10.100.42.2".parse().unwrap(), &new_key, &host_key).unwrap();
        assert_eq!(
            outcome,
            RotateOutcome::Applied {
                display: "alice".to_string()
            }
        );

        let text = std::fs::read_to_string(&config).unwrap();
        assert!(text.starts_with("# ホスト設定のコメント"), "コメント保持");
        let parsed = Config::load(&config).unwrap();
        assert_eq!(parsed.peers[0].public_key, new_key);
        assert_eq!(parsed.peers[0].name.as_deref(), Some("alice"));
        assert_eq!(
            parsed.peers[0].allowed_ips[0].addr().to_string(),
            "10.100.42.2"
        );
        assert_eq!(parsed.peers[1].public_key, bob, "他のピアは触らない");

        // 同じ依頼の再送は成功扱い(冪等)
        let outcome =
            rotate_peer_key(&config, "10.100.42.2".parse().unwrap(), &new_key, &host_key).unwrap();
        assert_eq!(outcome, RotateOutcome::Unchanged);

        // 旧鍵はもう登録されていない
        assert!(!parsed.peers.iter().any(|p| p.public_key == alice));
    }

    #[test]
    fn rotate_peer_key_rejects_collisions_and_unknown_ip() {
        let config = setup("rotate-reject");
        add(&config, "alice", "10.100.42.2");
        let bob = add(&config, "bob", "10.100.42.3");
        let host_key = PrivateKey::generate().public_key();
        let ip: Ipv4Addr = "10.100.42.2".parse().unwrap();

        // 別メンバーの鍵・ホストの鍵とは衝突させない
        assert!(rotate_peer_key(&config, ip, &bob, &host_key).is_err());
        assert!(rotate_peer_key(&config, ip, &host_key, &host_key).is_err());
        // 登録のない IP からの依頼は拒否
        let new_key = PrivateKey::generate().public_key();
        assert!(rotate_peer_key(
            &config,
            "10.100.42.99".parse().unwrap(),
            &new_key,
            &host_key
        )
        .is_err());
    }

    /// カスタムレコードのメンバー参照(ADR-0022): 鍵ローテーションで参照が
    /// 追随し、メンバー削除で参照レコードが掃除される。
    #[test]
    fn records_follow_rotation_and_removal() {
        use crate::dns::{add_record, list_records, RecordTarget};
        use peercove_core::config::MemberRef;

        let config = setup("records-follow");
        let alice = add(&config, "alice", "10.100.42.2");
        add(&config, "bob", "10.100.42.3");
        let host_key = PrivateKey::generate().public_key();

        add_record(
            &config,
            "gamehost",
            RecordTarget::Member(MemberRef::Key(alice)),
            None,
        )
        .unwrap();
        add_record(
            &config,
            "web",
            RecordTarget::Member(MemberRef::Key(alice)),
            Some(MemberRef::Key(alice)),
        )
        .unwrap();
        add_record(
            &config,
            "nas",
            RecordTarget::Ip("10.100.42.50".parse().unwrap()),
            None,
        )
        .unwrap();

        // 鍵ローテーション: member / under の両参照が新しい鍵へ差し替わる
        let new_key = PrivateKey::generate().public_key();
        rotate_peer_key(&config, "10.100.42.2".parse().unwrap(), &new_key, &host_key).unwrap();
        let parsed = Config::load(&config).unwrap();
        assert_eq!(parsed.dns_records[0].member, Some(MemberRef::Key(new_key)));
        assert_eq!(parsed.dns_records[1].under, Some(MemberRef::Key(new_key)));

        // メンバー削除: 参照レコードだけ消え、固定 IP レコードは残る
        remove_peer(&config, &Selector::Name("alice")).unwrap();
        let records = list_records(&config).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "nas");
    }

    /// DNS 名の設定(ADR-0021): 正規化して書き込み、表示名は触らない。
    /// 冪等・重複・予約語・従来導出ラベルとの衝突も検証する。
    #[test]
    fn set_peer_dns_name_validates_and_persists() {
        let config = setup("dns-name");
        let alice = add(&config, "alice", "10.100.42.2");
        add(&config, "テレビ", "10.100.42.3"); // 従来導出では member-3

        // 正規化(大文字・空白)して書き込まれる。表示名は変わらない
        let outcome = set_peer_dns_name(
            &config,
            &Selector::PublicKey(&alice.to_base64()),
            "Alice PC",
        )
        .unwrap();
        assert_eq!(
            outcome,
            DnsNameOutcome::Applied {
                display: "alice".to_string(),
                label: "alice-pc".to_string()
            }
        );
        let parsed = Config::load(&config).unwrap();
        assert_eq!(parsed.peers[0].dns_name.as_deref(), Some("alice-pc"));
        assert_eq!(parsed.peers[0].name.as_deref(), Some("alice"));
        let text = std::fs::read_to_string(&config).unwrap();
        assert!(text.starts_with("# ホスト設定のコメント"), "コメント保持");

        // 同じ名前の再設定は Unchanged(冪等)
        let outcome = set_peer_dns_name(
            &config,
            &Selector::Ip("10.100.42.2".parse().unwrap()),
            "alice-pc",
        )
        .unwrap();
        assert_eq!(
            outcome,
            DnsNameOutcome::Unchanged {
                label: "alice-pc".to_string()
            }
        );

        // 他メンバーの確定名・従来導出ラベル・予約語・ホスト名は拒否
        let bob_selector = Selector::Name("テレビ");
        assert!(set_peer_dns_name(&config, &bob_selector, "alice-pc").is_err());
        assert!(set_peer_dns_name(&config, &bob_selector, "host").is_err());
        assert!(set_peer_dns_name(&config, &bob_selector, "localhost").is_err());
        assert!(
            set_peer_dns_name(&config, &bob_selector, "開発機").is_err(),
            "英数字なし"
        );
        // 従来導出中の自分のラベル(member-3)は自分でなら取れる
        assert!(set_peer_dns_name(&config, &bob_selector, "member-3").is_ok());
    }

    /// ホスト自身の DNS 名(ADR-0021): host は許可、メンバーの名前とは衝突不可。
    #[test]
    fn set_host_dns_name_validates_and_persists() {
        let config = setup("host-dns");
        add(&config, "alice", "10.100.42.2");
        set_peer_dns_name(&config, &Selector::Name("alice"), "game-pc").unwrap();

        assert_eq!(set_host_dns_name(&config, "host").unwrap(), "host");
        assert_eq!(
            set_host_dns_name(&config, "Game Room").unwrap(),
            "game-room"
        );
        let parsed = Config::load(&config).unwrap();
        assert_eq!(parsed.interface.dns_name.as_deref(), Some("game-room"));

        assert!(
            set_host_dns_name(&config, "game-pc").is_err(),
            "メンバーと重複"
        );
        assert!(set_host_dns_name(&config, "localhost").is_err(), "予約語");
    }

    #[test]
    fn find_peer_reports_known_members() {
        let config = setup("find");
        add(&config, "alice", "10.100.42.2");
        let parsed = Config::load(&config).unwrap();
        let err = find_peer(&parsed, &Selector::Name("nobody")).unwrap_err();
        assert!(err.to_string().contains("alice"));
    }
}
