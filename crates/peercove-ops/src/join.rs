//! 招待トークン(pcv1)から参加用の鍵と設定を生成する(ADR-0005 案 B)。

use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use ipnet::Ipv4Net;
use peercove_core::config::Config;
use peercove_core::token::InviteToken;

pub struct JoinResult {
    pub config_path: PathBuf,
    pub key_path: PathBuf,
    pub psk_path: Option<PathBuf>,
    pub name: String,
    pub address: Ipv4Net,
    /// 採用したエンドポイント(先頭候補)
    pub endpoint: SocketAddrV4,
    /// 他の候補(member.toml にコメントとして残る)
    pub other_endpoints: Vec<SocketAddrV4>,
    pub host_virtual_ip: Ipv4Addr,
}

/// トークンを解釈し、`out_dir` に member.key / member.toml(必要なら member.psk)を作る。
pub fn join(token_text: &str, out_dir: &Path, force: bool) -> anyhow::Result<JoinResult> {
    let token = InviteToken::parse(token_text)?;

    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("{} の作成に失敗しました", out_dir.display()))?;
    let key_path = out_dir.join("member.key");
    let config_path = out_dir.join("member.toml");
    let psk_path = out_dir.join("member.psk");
    for path in [&key_path, &config_path] {
        if path.exists() && !force {
            bail!("{} は既に存在します", path.display());
        }
    }

    crate::secret::write_secret(
        &key_path,
        &format!("{}\n", token.member_private_key.to_base64()),
    )
    .context("秘密鍵の保存に失敗しました")?;
    let written_psk = match &token.preshared_key {
        Some(psk) => {
            crate::secret::write_secret(&psk_path, &format!("{}\n", psk.to_base64()))
                .context("PSK の保存に失敗しました")?;
            Some(psk_path)
        }
        None => None,
    };

    std::fs::write(&config_path, render_member_config(&token))
        .with_context(|| format!("{} の書き込みに失敗しました", config_path.display()))?;
    Config::load(&config_path).context("生成した設定の検証に失敗しました(バグの可能性)")?;

    Ok(JoinResult {
        config_path,
        key_path,
        psk_path: written_psk,
        name: token.name.clone(),
        address: token.member_address,
        endpoint: token.endpoints[0],
        other_endpoints: token.endpoints[1..].to_vec(),
        host_virtual_ip: token.host_virtual_ip,
    })
}

fn render_member_config(token: &InviteToken) -> String {
    let mut out = String::from("# peercove の join により生成\n[interface]\n");
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

    fn sample_token(psk: bool) -> InviteToken {
        InviteToken {
            member_private_key: PrivateKey::generate(),
            host_public_key: PrivateKey::generate().public_key(),
            preshared_key: psk.then(PresharedKey::generate),
            member_address: "10.100.42.5/24".parse().unwrap(),
            host_virtual_ip: "10.100.42.1".parse().unwrap(),
            endpoints: vec![
                "192.168.0.12:51820".parse().unwrap(),
                "203.0.113.5:51820".parse().unwrap(),
            ],
            name: "carol".to_string(),
        }
    }

    fn out_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("peercove-ops-join-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn join_writes_working_member_config() {
        let token = sample_token(true);
        let dir = out_dir("basic");
        let result = join(&token.encode().unwrap(), &dir, false).unwrap();

        assert_eq!(result.name, "carol");
        assert_eq!(result.endpoint.to_string(), "192.168.0.12:51820");
        assert_eq!(result.other_endpoints.len(), 1);

        let config = Config::load(&result.config_path).unwrap();
        assert_eq!(config.interface.display_name.as_deref(), Some("carol"));
        assert_eq!(config.interface.address.to_string(), "10.100.42.5/24");
        let peer = &config.peers[0];
        assert_eq!(peer.public_key, token.host_public_key);
        assert_eq!(peer.control_host, Some(token.host_virtual_ip));
        assert_eq!(peer.persistent_keepalive, Some(25));

        let key = peercove_core::keys::read_private_key_file(&result.key_path).unwrap();
        assert_eq!(key.as_bytes(), token.member_private_key.as_bytes());
        let psk = peercove_core::keys::read_preshared_key_file(&result.psk_path.unwrap()).unwrap();
        assert_eq!(psk.as_bytes(), token.preshared_key.unwrap().as_bytes());
    }

    #[test]
    fn join_guards_overwrite_and_rejects_bad_token() {
        let token = sample_token(false).encode().unwrap();
        let dir = out_dir("guard");
        join(&token, &dir, false).unwrap();
        assert!(join(&token, &dir, false).is_err(), "上書きガード");
        join(&token, &dir, true).unwrap();

        // 途中で切れたトークン
        assert!(join(&token[..token.len() - 8], &dir, true).is_err());
    }
}
