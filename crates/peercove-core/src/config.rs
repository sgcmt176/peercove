//! TOML 設定ファイルの型と読み込み。
//!
//! 設定内の相対パス(鍵ファイル等)は設定ファイルのあるディレクトリ基準で解決する。

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use ipnet::Ipv4Net;
use serde::{Deserialize, Serialize};

use crate::keys::PublicKey;
use crate::{Error, Result};

pub const DEFAULT_MTU: u16 = 1420;
pub const DEFAULT_LISTEN_PORT: u16 = 51820;

/// OS ごとの既定インターフェース名。
pub const DEFAULT_IF_NAME: &str = "peercove0";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub interface: InterfaceConfig,
    #[serde(default, rename = "peer")]
    pub peers: Vec<PeerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceConfig {
    /// トンネルインターフェース名(省略時 `peercove0`)。
    #[serde(default = "default_if_name")]
    pub name: String,
    /// 台帳・コントロールチャネルで使う自分の表示名(join で設定される)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub private_key_file: PathBuf,
    /// 仮想 IP とサブネット(例: `100.100.42.2/24`)。
    pub address: Ipv4Net,
    /// UDP 待受ポート。ホストでは省略時 51820、メンバーでは省略時 OS 任せ。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub listen_port: Option<u16>,
    #[serde(default = "default_mtu")]
    pub mtu: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerConfig {
    /// 台帳用の表示名(invite で発行したメンバーに付く)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// このピア(ホスト)の仮想 IP。メンバー側でコントロールチャネルの
    /// 接続先として使う(join が設定する)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub control_host: Option<std::net::Ipv4Addr>,
    pub public_key: PublicKey,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<SocketAddr>,
    pub allowed_ips: Vec<Ipv4Net>,
    /// 秒。NAT 維持のためメンバー→ホストでは 25 を推奨。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persistent_keepalive: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preshared_key_file: Option<PathBuf>,
}

fn default_mtu() -> u16 {
    DEFAULT_MTU
}

fn default_if_name() -> String {
    DEFAULT_IF_NAME.to_string()
}

impl Config {
    /// 設定ファイルを読み込み、検証し、相対パスを設定ファイル基準で解決する。
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let mut config: Config = toml::from_str(&text)?;
        let base_dir = path.parent().unwrap_or(Path::new("."));
        config.interface.private_key_file = resolve(base_dir, &config.interface.private_key_file);
        for peer in &mut config.peers {
            if let Some(psk) = &peer.preshared_key_file {
                peer.preshared_key_file = Some(resolve(base_dir, psk));
            }
        }
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        let invalid = |message: String| Err(Error::InvalidConfig(message));
        if self.interface.address.prefix_len() > 30 {
            return invalid(format!(
                "interface.address のプレフィックス長 /{} が長すぎます(/30 以下にしてください)",
                self.interface.address.prefix_len()
            ));
        }
        let ip = self.interface.address.addr();
        if ip == self.interface.address.network() || ip == self.interface.address.broadcast() {
            return invalid(format!(
                "interface.address {ip} はネットワーク/ブロードキャストアドレスです"
            ));
        }
        if self.interface.mtu < 576 {
            return invalid(format!(
                "mtu {} が小さすぎます(576 以上)",
                self.interface.mtu
            ));
        }
        for (i, peer) in self.peers.iter().enumerate() {
            if peer.allowed_ips.is_empty() {
                return invalid(format!("peer[{i}] の allowed_ips が空です"));
            }
        }
        let mut keys: Vec<_> = self.peers.iter().map(|p| p.public_key).collect();
        keys.sort_unstable_by_key(|k| *k.as_bytes());
        keys.dedup();
        if keys.len() != self.peers.len() {
            return invalid("同じ public_key のピアが重複しています".to_string());
        }
        Ok(())
    }
}

fn resolve(base_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// handoff 4.1 の member.toml 例(公開鍵は実在する 32 バイト値に置換)。
    const MEMBER_TOML: &str = r#"
[interface]
private_key_file = "member_a.key"
address = "100.100.42.2/24"
mtu = 1420

[[peer]]
public_key = "hSDwCYkwp1R0i33ctD73Wg2/Og0mOBr06uSpB6ipTmo="
endpoint = "203.0.113.5:51820"
allowed_ips = ["100.100.42.0/24"]
persistent_keepalive = 25
"#;

    fn parse(text: &str) -> Config {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn parses_handoff_member_example() {
        let config = parse(MEMBER_TOML);
        assert_eq!(config.interface.address.addr().to_string(), "100.100.42.2");
        assert_eq!(config.interface.address.prefix_len(), 24);
        assert_eq!(config.interface.mtu, 1420);
        assert_eq!(config.interface.name, DEFAULT_IF_NAME);
        assert_eq!(config.peers.len(), 1);
        let peer = &config.peers[0];
        assert_eq!(peer.endpoint.unwrap().to_string(), "203.0.113.5:51820");
        assert_eq!(peer.allowed_ips[0].to_string(), "100.100.42.0/24");
        assert_eq!(peer.persistent_keepalive, Some(25));
        assert!(peer.preshared_key_file.is_none());
        config.validate().unwrap();
    }

    #[test]
    fn mtu_defaults_to_1420_and_peers_default_to_empty() {
        let config = parse(
            r#"
[interface]
private_key_file = "host.key"
address = "100.100.42.1/24"
listen_port = 51820
"#,
        );
        assert_eq!(config.interface.mtu, DEFAULT_MTU);
        assert_eq!(config.interface.listen_port, Some(51820));
        assert!(config.peers.is_empty());
        config.validate().unwrap();
    }

    #[test]
    fn rejects_invalid_public_key() {
        let result: std::result::Result<Config, _> = toml::from_str(
            r#"
[interface]
private_key_file = "a.key"
address = "100.100.42.2/24"

[[peer]]
public_key = "short"
allowed_ips = ["100.100.42.0/24"]
"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn rejects_unknown_fields() {
        let result: std::result::Result<Config, _> = toml::from_str(
            r#"
[interface]
private_key_file = "a.key"
address = "100.100.42.2/24"
typo_field = 1
"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn validate_rejects_network_address() {
        let mut config = parse(MEMBER_TOML);
        config.interface.address = "100.100.42.0/24".parse().unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_rejects_duplicate_peers() {
        let mut config = parse(MEMBER_TOML);
        config.peers.push(config.peers[0].clone());
        assert!(config.validate().is_err());
    }

    #[test]
    fn load_resolves_relative_key_path() {
        let dir = std::env::temp_dir().join("peercove-core-test-config");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("member.toml");
        std::fs::write(&path, MEMBER_TOML).unwrap();
        let config = Config::load(&path).unwrap();
        assert_eq!(config.interface.private_key_file, dir.join("member_a.key"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
