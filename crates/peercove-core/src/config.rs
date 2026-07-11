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
    /// カスタム DNS レコード(ADR-0011 §1b)。ホスト設定のみ意味を持ち、
    /// 台帳と一緒にメンバーへ配布される。
    #[serde(default, rename = "dns_record", skip_serializing_if = "Vec::is_empty")]
    pub dns_records: Vec<crate::dns::DnsRecord>,
    /// アクセス制御(ADR-0018、M3-10)。ホスト設定のみ意味を持つ。
    /// 注意: `deny_unknown_fields` のため、これを書いた設定は旧バージョンでは
    /// 読めない(明示エラーになる)。
    #[serde(default, skip_serializing_if = "AclConfig::is_empty")]
    pub acl: AclConfig,
}

/// アクセス制御の設定(ADR-0018、M3-10)。既定はすべて許可で、
/// `deny` に載せた仮想 IP の組(順不同)だけメンバー間通信を遮断する。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AclConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<(std::net::Ipv4Addr, std::net::Ipv4Addr)>,
}

impl AclConfig {
    pub fn is_empty(&self) -> bool {
        self.deny.is_empty()
    }

    /// 順不同の組を正規化(小さい IP を先)して返す。重複は除去。
    pub fn normalized_deny(&self) -> Vec<(std::net::Ipv4Addr, std::net::Ipv4Addr)> {
        let mut pairs: Vec<_> = self
            .deny
            .iter()
            .map(|&(a, b)| if a <= b { (a, b) } else { (b, a) })
            .collect();
        pairs.sort_unstable();
        pairs.dedup();
        pairs
    }

    /// この組は遮断対象か(順不同)。
    pub fn is_denied(&self, x: std::net::Ipv4Addr, y: std::net::Ipv4Addr) -> bool {
        self.deny
            .iter()
            .any(|&(a, b)| (a == x && b == y) || (a == y && b == x))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceConfig {
    /// トンネルインターフェース名(省略時 `peercove0`)。
    #[serde(default = "default_if_name")]
    pub name: String,
    /// 所属ネットワーク名(ADR-0012)。正規化済みの DNS ラベル。
    /// 旧設定には無いフィールドで、省略時は [`crate::names::DEFAULT_NETWORK_NAME`]。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_name: Option<String>,
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
    /// 受信するファイルサイズの上限(MB、ADR-0015 / M3-9)。0 で無制限。
    /// **受け取る側**の設定として効く(超える申し出は拒否する)。
    /// 注意: `deny_unknown_fields` のため、これを書いた設定は旧バージョンでは
    /// 読めない(明示エラーになる)。
    #[serde(default = "default_max_recv_file_mb")]
    pub max_recv_file_mb: u64,
    /// メンバー間直接通信(ADR-0013)を試すか(既定 true)。false なら
    /// このマシンは常にホスト経由(中継)で通信する。ADR-0013 追加条件 2:
    /// 将来 UI の設定画面から切り替えられるようにするためのフラグ。
    #[serde(default = "default_direct")]
    pub direct: bool,
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
    /// このメンバーが広告する背後 LAN のサブネット(ADR-0014、M3-7)。
    /// ホスト設定が正本で、台帳経由で全メンバーへ配布される。
    /// 注意: `deny_unknown_fields` のため、これを書いた設定は旧バージョンでは
    /// 読めない(明示エラーになる)。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subnets: Vec<Ipv4Net>,
    /// 秒。NAT 維持のためメンバー→ホストでは 25 を推奨。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persistent_keepalive: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preshared_key_file: Option<PathBuf>,
}

fn default_mtu() -> u16 {
    DEFAULT_MTU
}

fn default_direct() -> bool {
    true
}

/// 受信ファイルサイズ上限の既定(100 MB、2026-07-11 依頼者指定)。
pub const DEFAULT_MAX_RECV_FILE_MB: u64 = 100;

fn default_max_recv_file_mb() -> u64 {
    DEFAULT_MAX_RECV_FILE_MB
}

fn default_if_name() -> String {
    DEFAULT_IF_NAME.to_string()
}

impl Config {
    /// 所属ネットワーク名。旧設定(フィールドなし)は既定名として扱う。
    pub fn network_name(&self) -> &str {
        self.interface
            .network_name
            .as_deref()
            .unwrap_or(crate::names::DEFAULT_NETWORK_NAME)
    }

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
        // 広告サブネット(ADR-0014)。仮想サブネットやピア間で重なると
        // 経路が奪い合いになるため、設定段階で拒否する
        let virtual_subnet = self.interface.address.trunc();
        let mut seen_subnets: Vec<(usize, Ipv4Net)> = Vec::new();
        for (i, peer) in self.peers.iter().enumerate() {
            for subnet in &peer.subnets {
                if subnet.prefix_len() < 8 {
                    return invalid(format!(
                        "peer[{i}] の subnet {subnet} が広すぎます(/8 以上にしてください)"
                    ));
                }
                if *subnet != subnet.trunc() {
                    return invalid(format!(
                        "peer[{i}] の subnet {subnet} はネットワークアドレスで指定してください({})",
                        subnet.trunc()
                    ));
                }
                if virtual_subnet.contains(subnet) || subnet.contains(&virtual_subnet) {
                    return invalid(format!(
                        "peer[{i}] の subnet {subnet} が仮想サブネット {virtual_subnet} と重なっています"
                    ));
                }
                if let Some((j, other)) = seen_subnets
                    .iter()
                    .find(|(_, other)| other.contains(subnet) || subnet.contains(other))
                {
                    return invalid(format!(
                        "peer[{i}] の subnet {subnet} が peer[{j}] の {other} と重なっています"
                    ));
                }
                seen_subnets.push((i, *subnet));
            }
        }
        let mut keys: Vec<_> = self.peers.iter().map(|p| p.public_key).collect();
        keys.sort_unstable_by_key(|k| *k.as_bytes());
        keys.dedup();
        if keys.len() != self.peers.len() {
            return invalid("同じ public_key のピアが重複しています".to_string());
        }
        if let Some(name) = &self.interface.network_name {
            if !crate::names::is_dns_label(name) {
                return invalid(format!(
                    "network_name \"{name}\" が不正です(小文字英数とハイフンのみ、63 文字以内)"
                ));
            }
        }
        // ACL(ADR-0018): ホスト⇔メンバーの遮断はコントロールチャネルが
        // 壊れるため拒否。存在しないメンバーの IP は許容(効果がないだけ)
        for (a, b) in &self.acl.deny {
            if a == b {
                return invalid(format!("acl.deny の組 [{a}, {b}] が同じ IP です"));
            }
            for ip in [a, b] {
                if *ip == self.interface.address.addr() {
                    return invalid(format!(
                        "acl.deny に自分(ホスト)の IP {ip} は指定できません\
                        (ホストとの通信は遮断できません)"
                    ));
                }
                if !virtual_subnet.contains(ip) {
                    return invalid(format!(
                        "acl.deny の IP {ip} が仮想サブネット {virtual_subnet} の外です"
                    ));
                }
            }
        }
        let mut seen_records = std::collections::HashSet::new();
        for record in &self.dns_records {
            if !crate::names::is_dns_label(&record.name) {
                return invalid(format!(
                    "dns_record \"{}\" が不正です(小文字英数とハイフンのみ、63 文字以内)",
                    record.name
                ));
            }
            if !seen_records.insert(record.name.as_str()) {
                return invalid(format!("dns_record \"{}\" が重複しています", record.name));
            }
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
    fn validate_checks_subnet_overlaps() {
        // 仮想サブネットと重なる広告は拒否、外の RFC1918 は許可(ADR-0014)
        let mut config = parse(MEMBER_TOML);
        config.peers[0].subnets = vec!["100.100.42.0/28".parse().unwrap()];
        assert!(config.validate().is_err());
        config.peers[0].subnets = vec!["192.168.10.0/24".parse().unwrap()];
        assert!(config.validate().is_ok());
        // ピア間の重複も拒否
        let mut other = config.peers[0].clone();
        other.public_key = crate::keys::PrivateKey::generate().public_key();
        other.subnets = vec!["192.168.10.128/25".parse().unwrap()];
        config.peers.push(other);
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_rejects_duplicate_peers() {
        let mut config = parse(MEMBER_TOML);
        config.peers.push(config.peers[0].clone());
        assert!(config.validate().is_err());
    }

    #[test]
    fn dns_records_parse_and_validate() {
        let config = parse(
            r#"
[interface]
private_key_file = "host.key"
address = "10.100.42.1/24"

[[dns_record]]
name = "nas"
ip = "10.100.42.50"
"#,
        );
        config.validate().unwrap();
        assert_eq!(config.dns_records.len(), 1);
        assert_eq!(config.dns_records[0].name, "nas");
        assert_eq!(config.dns_records[0].ip.to_string(), "10.100.42.50");

        // 不正ラベル・重複は弾く
        let mut bad = config.clone();
        bad.dns_records[0].name = "Bad Label".to_string();
        assert!(bad.validate().is_err());
        let mut dup = config.clone();
        dup.dns_records.push(dup.dns_records[0].clone());
        assert!(dup.validate().is_err());
    }

    #[test]
    fn acl_parses_normalizes_and_validates() {
        let config = parse(
            r#"
[interface]
private_key_file = "host.key"
address = "10.100.42.1/24"

[acl]
deny = [["10.100.42.3", "10.100.42.2"], ["10.100.42.2", "10.100.42.3"]]
"#,
        );
        config.validate().unwrap();
        // 正規化: 順不同 + 重複除去
        let a: std::net::Ipv4Addr = "10.100.42.2".parse().unwrap();
        let b: std::net::Ipv4Addr = "10.100.42.3".parse().unwrap();
        assert_eq!(config.acl.normalized_deny(), vec![(a, b)]);
        assert!(config.acl.is_denied(a, b));
        assert!(config.acl.is_denied(b, a), "順不同で判定される");
        assert!(!config.acl.is_denied(a, "10.100.42.9".parse().unwrap()));

        // ホスト自身を含む組は拒否
        let mut bad = config.clone();
        bad.acl.deny = vec![("10.100.42.1".parse().unwrap(), a)];
        assert!(bad.validate().is_err());
        // サブネット外は拒否
        let mut bad = config.clone();
        bad.acl.deny = vec![("192.168.1.2".parse().unwrap(), a)];
        assert!(bad.validate().is_err());
        // 同一 IP の組は拒否
        let mut bad = config.clone();
        bad.acl.deny = vec![(a, a)];
        assert!(bad.validate().is_err());
        // 存在しないメンバーの IP は許容(効果がないだけ)
        let mut ok = config.clone();
        ok.acl.deny = vec![(a, "10.100.42.99".parse().unwrap())];
        assert!(ok.validate().is_ok());
    }

    /// [acl] が無い旧設定はそのまま読め、空の ACL はシリアライズに現れない。
    #[test]
    fn acl_defaults_to_empty_and_stays_off_the_wire() {
        let config = parse(MEMBER_TOML);
        assert!(config.acl.is_empty());
        let text = toml::to_string(&config).unwrap();
        assert!(!text.contains("acl"), "空なら書き出されない: {text}");
    }

    #[test]
    fn network_name_defaults_and_validates() {
        let config = parse(MEMBER_TOML);
        assert_eq!(config.interface.network_name, None);
        assert_eq!(config.network_name(), crate::names::DEFAULT_NETWORK_NAME);

        let mut config = parse(MEMBER_TOML);
        config.interface.network_name = Some("my-game-lan".into());
        config.validate().unwrap();
        assert_eq!(config.network_name(), "my-game-lan");

        // 正規化されていない名前は弾く
        config.interface.network_name = Some("My LAN".into());
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
