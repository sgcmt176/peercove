//! メンバー側: 招待トークン(pcv1)から鍵と設定を生成する(ADR-0005 案 B)。

use std::path::Path;

use anyhow::{bail, Context};
use peercove_core::config::Config;
use peercove_core::keys::write_secret_file;
use peercove_core::token::InviteToken;

#[derive(Clone, Copy)]
pub struct JoinOptions<'a> {
    /// トークン文字列(--token)。token_file とどちらか一方
    pub token: Option<&'a str>,
    /// トークンファイル(--token-file)
    pub token_file: Option<&'a Path>,
    /// 出力先ディレクトリ(member.key / member.toml / member.psk)
    pub out_dir: &'a Path,
    pub force: bool,
}

pub fn run(options: &JoinOptions) -> anyhow::Result<()> {
    let text = match (options.token, options.token_file) {
        (Some(text), None) => text.to_string(),
        (None, Some(path)) => std::fs::read_to_string(path)
            .with_context(|| format!("{} の読み込みに失敗しました", path.display()))?,
        _ => bail!("--token か --token-file のどちらか一方を指定してください"),
    };
    let token = InviteToken::parse(&text)?;

    std::fs::create_dir_all(options.out_dir)
        .with_context(|| format!("{} の作成に失敗しました", options.out_dir.display()))?;
    let key_path = options.out_dir.join("member.key");
    let config_path = options.out_dir.join("member.toml");
    let psk_path = options.out_dir.join("member.psk");
    for path in [&key_path, &config_path] {
        if path.exists() && !options.force {
            bail!(
                "{} は既に存在します。上書きするには --force を指定してください",
                path.display()
            );
        }
    }

    // 秘密鍵(と PSK)を保存
    write_secret_file(
        &key_path,
        &format!("{}\n", token.member_private_key.to_base64()),
    )
    .context("秘密鍵の保存に失敗しました")?;
    super::restrict_secret_file_acl(&key_path);
    if let Some(psk) = &token.preshared_key {
        write_secret_file(&psk_path, &format!("{}\n", psk.to_base64()))
            .context("PSK の保存に失敗しました")?;
        super::restrict_secret_file_acl(&psk_path);
    }

    // member.toml を生成
    let config_text = render_member_config(&token);
    std::fs::write(&config_path, &config_text)
        .with_context(|| format!("{} の書き込みに失敗しました", config_path.display()))?;
    // 生成物が正しく読めることを確認(自己検証)
    Config::load(&config_path).context("生成した設定の検証に失敗しました(バグの可能性)")?;

    println!("参加設定を生成しました({} さん)", token.name);
    println!("  設定: {}", config_path.display());
    println!("  割当 IP: {}", token.member_address);
    if token.endpoints.len() > 1 {
        println!(
            "  エンドポイント: {}(他の候補は member.toml のコメント参照)",
            token.endpoints[0]
        );
        println!("  ※ ホストと同じ LAN にいる場合は LAN 側の候補を使ってください");
    } else {
        println!("  エンドポイント: {}", token.endpoints[0]);
    }
    println!();
    println!("次の手順で接続します:");
    #[cfg(windows)]
    println!(
        "  (管理者ターミナルで) .\\peercove-poc.exe member --config {}",
        config_path.display()
    );
    #[cfg(not(windows))]
    println!(
        "  sudo ./peercove-poc member --config {}",
        config_path.display()
    );
    println!("使い終わったトークン(文字列・ファイル)は削除してください");
    Ok(())
}

fn render_member_config(token: &InviteToken) -> String {
    let mut out = String::from("# peercove-poc join により生成\n[interface]\n");
    out.push_str(&format!("display_name = {:?}\n", token.name));
    out.push_str("private_key_file = \"member.key\"\n");
    out.push_str(&format!("address = \"{}\"\n", token.member_address));
    out.push_str("\n[[peer]]\n");
    out.push_str(&format!("control_host = \"{}\"\n", token.host_virtual_ip));
    out.push_str(&format!("public_key = \"{}\"\n", token.host_public_key));
    out.push_str(&format!("endpoint = \"{}\"\n", token.endpoints[0]));
    for other in &token.endpoints[1..] {
        out.push_str(&format!("# endpoint = \"{other}\"  # 別の候補\n"));
    }
    out.push_str(&format!(
        "allowed_ips = [\"{}\"]\n",
        token.member_address.trunc()
    ));
    out.push_str("persistent_keepalive = 25\n");
    if token.preshared_key.is_some() {
        out.push_str("preshared_key_file = \"member.psk\"\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use peercove_core::keys::{PresharedKey, PrivateKey};
    use std::path::PathBuf;

    fn sample_token(psk: bool) -> InviteToken {
        InviteToken {
            member_private_key: PrivateKey::generate(),
            host_public_key: PrivateKey::generate().public_key(),
            preshared_key: psk.then(PresharedKey::generate),
            member_address: "100.100.42.5/24".parse().unwrap(),
            host_virtual_ip: "100.100.42.1".parse().unwrap(),
            endpoints: vec![
                "192.168.0.12:51820".parse().unwrap(),
                "203.0.113.5:51820".parse().unwrap(),
            ],
            name: "carol".to_string(),
        }
    }

    fn out_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("peercove-join-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn join_writes_working_member_config() {
        let token = sample_token(true);
        let encoded = token.encode().unwrap();
        let dir = out_dir("basic");
        run(&JoinOptions {
            token: Some(&encoded),
            token_file: None,
            out_dir: &dir,
            force: false,
        })
        .unwrap();

        let config = Config::load(&dir.join("member.toml")).unwrap();
        assert_eq!(config.interface.display_name.as_deref(), Some("carol"));
        assert_eq!(config.interface.address.to_string(), "100.100.42.5/24");
        let peer = &config.peers[0];
        assert_eq!(peer.public_key, token.host_public_key);
        assert_eq!(peer.control_host, Some("100.100.42.1".parse().unwrap()));
        assert_eq!(peer.endpoint.unwrap().to_string(), "192.168.0.12:51820");
        assert_eq!(peer.allowed_ips[0].to_string(), "100.100.42.0/24");
        assert_eq!(peer.persistent_keepalive, Some(25));

        // 鍵・PSK がトークンと一致
        let key = peercove_core::keys::read_private_key_file(&dir.join("member.key")).unwrap();
        assert_eq!(key.as_bytes(), token.member_private_key.as_bytes());
        let psk = peercove_core::keys::read_preshared_key_file(&dir.join("member.psk")).unwrap();
        assert_eq!(psk.as_bytes(), token.preshared_key.unwrap().as_bytes());
    }

    #[test]
    fn join_from_token_file_and_overwrite_guard() {
        let token = sample_token(false);
        let encoded = token.encode().unwrap();
        let dir = out_dir("file");
        std::fs::create_dir_all(&dir).unwrap();
        let token_path = dir.join("invite.token");
        std::fs::write(&token_path, format!("{encoded}\n")).unwrap();

        let opts = JoinOptions {
            token: None,
            token_file: Some(&token_path),
            out_dir: &dir,
            force: false,
        };
        run(&opts).unwrap();
        // 2 回目は上書きガード
        assert!(run(&opts).is_err());
        // --force で成功
        run(&JoinOptions {
            force: true,
            ..opts
        })
        .unwrap();
    }

    #[test]
    fn join_requires_exactly_one_source() {
        let dir = out_dir("source");
        assert!(run(&JoinOptions {
            token: None,
            token_file: None,
            out_dir: &dir,
            force: false,
        })
        .is_err());
    }
}
