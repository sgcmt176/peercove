use std::net::Ipv4Addr;
use std::path::Path;

use anyhow::{bail, Context};
use peercove_core::config::Config;
use peercove_core::ipalloc::next_free_ip;
use peercove_core::keys::PublicKey;

/// ホスト設定へ `[[peer]]` ブロックを追記する。
///
/// TOML 全体を再シリアライズするとコメントが失われるため、テキスト追記方式にする。
/// 実行中の host プロセスは設定の定期再読込で新規ピアを取り込む(ADR-0002)。
pub fn run(config_path: &Path, pubkey: &str, ip: Ipv4Addr) -> anyhow::Result<()> {
    let config = Config::load(config_path)?;
    let public_key = PublicKey::from_base64(pubkey)
        .context("--pubkey が不正です(base64 の X25519 公開鍵を指定してください)")?;

    let subnet = config.interface.address.trunc();
    if !subnet.contains(&ip) {
        bail!("--ip {ip} はトンネルのサブネット {subnet} の範囲外です");
    }
    if ip == config.interface.address.addr() {
        bail!("--ip {ip} はホスト自身のアドレスです");
    }
    if config.peers.iter().any(|p| p.public_key == public_key) {
        bail!("公開鍵 {public_key} のピアは既に登録されています");
    }
    let used: Vec<Ipv4Addr> = std::iter::once(config.interface.address.addr())
        .chain(
            config
                .peers
                .iter()
                .flat_map(|p| p.allowed_ips.iter().map(|net| net.addr())),
        )
        .collect();
    if used.contains(&ip) {
        let suggestion = next_free_ip(subnet, &used)
            .map(|free| format!("(空きの例: {free})"))
            .unwrap_or_default();
        bail!("--ip {ip} は使用済みです{suggestion}");
    }

    let original = std::fs::read_to_string(config_path)
        .with_context(|| format!("{} の読み込みに失敗しました", config_path.display()))?;
    let block = format!("\n[[peer]]\npublic_key = \"{public_key}\"\nallowed_ips = [\"{ip}/32\"]\n");
    let updated = format!("{original}{block}");

    // 書き込む前に、追記結果が正しく解析できることを確認する
    let parsed: Config = toml::from_str(&updated).context("追記結果の TOML が不正です")?;
    parsed.validate()?;

    std::fs::write(config_path, &updated)
        .with_context(|| format!("{} の書き込みに失敗しました", config_path.display()))?;
    println!("ピアを追加しました: {public_key} -> {ip}/32");
    println!("実行中の host プロセスには約 5 秒で自動反映されます(再起動不要)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST_TOML: &str = r#"# ホスト設定のコメント
[interface]
private_key_file = "host.key"
address = "100.100.42.1/24"
listen_port = 51820
"#;

    fn setup(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("peercove-add-peer-{name}"));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("host.toml");
        std::fs::write(&config, HOST_TOML).unwrap();
        // Config::load が秘密鍵の存在を要求しないことを前提にしない(現状は load
        // 時に鍵ファイルを読まないが、パス解決だけは行われる)
        config
    }

    const PUBKEY: &str = "hSDwCYkwp1R0i33ctD73Wg2/Og0mOBr06uSpB6ipTmo=";
    const PUBKEY2: &str = "3p7bfXt9wbTTW2HC7OQ1Nz+DQ8hbeGdNrfx+FG+IK08=";

    #[test]
    fn appends_peer_preserving_comments() {
        let config = setup("append");
        run(&config, PUBKEY, "100.100.42.2".parse().unwrap()).unwrap();
        let text = std::fs::read_to_string(&config).unwrap();
        assert!(text.starts_with("# ホスト設定のコメント"));
        assert!(text.contains(PUBKEY));
        assert!(text.contains("100.100.42.2/32"));
        // 2 人目も追加できる
        run(&config, PUBKEY2, "100.100.42.3".parse().unwrap()).unwrap();
        let parsed: Config = toml::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
        assert_eq!(parsed.peers.len(), 2);
    }

    #[test]
    fn rejects_duplicate_key_ip_and_out_of_subnet() {
        let config = setup("reject");
        run(&config, PUBKEY, "100.100.42.2".parse().unwrap()).unwrap();
        assert!(run(&config, PUBKEY, "100.100.42.3".parse().unwrap()).is_err());
        assert!(run(&config, PUBKEY2, "100.100.42.2".parse().unwrap()).is_err());
        assert!(run(&config, PUBKEY2, "100.100.43.9".parse().unwrap()).is_err());
        assert!(run(&config, PUBKEY2, "100.100.42.1".parse().unwrap()).is_err());
        assert!(run(&config, "short", "100.100.42.9".parse().unwrap()).is_err());
    }
}
