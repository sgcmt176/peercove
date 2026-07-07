//! ホスト側: メンバーの削除(M1-G3)。
//!
//! host.toml から対象の `[[peer]]` を取り除くだけで、実行中の host プロセスが
//! 定期再読込で検知し、(1) 本人へ削除通知 → (2) バックエンドから実削除、を行う
//! (ADR-0002/0005 の仕組みの削除版)。コメント等を保持するため toml_edit を使う。

use std::net::Ipv4Addr;
use std::path::Path;

use anyhow::{bail, Context};
use peercove_core::config::{Config, PeerConfig};

/// 削除対象の指定(いずれか 1 つ)。
pub enum Selector<'a> {
    Name(&'a str),
    PublicKey(&'a str),
    Ip(Ipv4Addr),
}

pub fn run(config_path: &Path, selector: &Selector) -> anyhow::Result<()> {
    let config = Config::load(config_path)?;
    let target = find_target(&config, selector)?;
    let target_key = target.public_key.to_base64();
    let display = target
        .name
        .clone()
        .unwrap_or_else(|| target.public_key.to_string());

    // toml_edit で該当 [[peer]] だけを取り除く(コメント・整形を保持)
    let text = std::fs::read_to_string(config_path)
        .with_context(|| format!("{} の読み込みに失敗しました", config_path.display()))?;
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .context("host.toml の解析に失敗しました(手編集の構文エラー?)")?;
    let peers = doc
        .get_mut("peer")
        .and_then(|item| item.as_array_of_tables_mut())
        .context("[[peer]] が見つかりません")?;
    let before = peers.len();
    peers.retain(|table| {
        table
            .get("public_key")
            .and_then(|v| v.as_str())
            .map(str::trim)
            != Some(target_key.as_str())
    });
    if peers.len() != before - 1 {
        bail!("host.toml から対象ピアを特定できませんでした(手編集で public_key が変わっている可能性)");
    }
    let updated = doc.to_string();
    // 書き込む前に整合性を確認
    let parsed: Config = toml::from_str(&updated).context("編集結果の TOML が不正です")?;
    parsed.validate()?;
    std::fs::write(config_path, &updated)
        .with_context(|| format!("{} の書き込みに失敗しました", config_path.display()))?;

    // invite --psk が作ったホスト側 PSK ファイルを片付ける
    if let Some(psk_path) = &target.preshared_key_file {
        match std::fs::remove_file(psk_path) {
            Ok(()) => println!("PSK ファイル {} を削除しました", psk_path.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!("PSK ファイルの削除に失敗しました: {e}"),
        }
    }

    println!("メンバー {display} を host.toml から削除しました");
    println!(
        "実行中の host には約 10 秒で反映されます(本人へ削除通知 → トンネルから除外)。\
         本人が保持しているトークン・鍵は以後使えません"
    );
    Ok(())
}

fn find_target<'a>(config: &'a Config, selector: &Selector) -> anyhow::Result<&'a PeerConfig> {
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
        _ => bail!("複数のピアに一致しました。--pubkey で一意に指定してください"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::invite::{run as invite, InviteOptions};
    use peercove_core::keys::{write_secret_file, PrivateKey};
    use std::path::PathBuf;

    fn setup(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("peercove-remove-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("host.toml");
        std::fs::write(
            &config,
            "# ホスト設定のコメント\n[interface]\nprivate_key_file = \"host.key\"\naddress = \"100.100.42.1/24\"\nlisten_port = 51820\n",
        )
        .unwrap();
        write_secret_file(&dir.join("host.key"), &PrivateKey::generate().to_base64()).unwrap();
        config
    }

    fn invite_member(config: &Path, name: &str, psk: bool) {
        let out = config.parent().unwrap().join(format!("{name}.token"));
        invite(&InviteOptions {
            config_path: config,
            name: Some(name),
            ip: None,
            extra_endpoints: &["203.0.113.5:51820".parse().unwrap()],
            psk,
            out: &out,
            force: false,
            print: false,
            qr: false,
        })
        .unwrap();
    }

    #[test]
    fn removes_by_name_preserving_comments_and_other_peers() {
        let config_path = setup("by-name");
        invite_member(&config_path, "alice", true);
        invite_member(&config_path, "bob", false);

        let psk_path = config_path.parent().unwrap().join("peer-100.100.42.2.psk");
        assert!(psk_path.exists());

        run(&config_path, &Selector::Name("alice")).unwrap();

        let text = std::fs::read_to_string(&config_path).unwrap();
        assert!(
            text.starts_with("# ホスト設定のコメント"),
            "コメントが保持される"
        );
        let config = Config::load(&config_path).unwrap();
        assert_eq!(config.peers.len(), 1);
        assert_eq!(config.peers[0].name.as_deref(), Some("bob"));
        assert!(!psk_path.exists(), "PSK ファイルも削除される");
    }

    #[test]
    fn removes_by_ip_and_pubkey() {
        let config_path = setup("by-ip");
        invite_member(&config_path, "alice", false);
        invite_member(&config_path, "bob", false);
        let config = Config::load(&config_path).unwrap();
        let bob_key = config.peers[1].public_key.to_base64();

        run(&config_path, &Selector::Ip("100.100.42.2".parse().unwrap())).unwrap();
        run(&config_path, &Selector::PublicKey(&bob_key)).unwrap();
        assert!(Config::load(&config_path).unwrap().peers.is_empty());
    }

    #[test]
    fn rejects_unknown_target() {
        let config_path = setup("unknown");
        invite_member(&config_path, "alice", false);
        let err = run(&config_path, &Selector::Name("nobody")).unwrap_err();
        assert!(
            err.to_string().contains("alice"),
            "登録済み一覧をヒントに出す"
        );
    }
}
