//! 招待トークン(pcv1)の発行(ADR-0005 案 B)。
//!
//! メンバーの鍵ペアと仮想 IP をこの場で生成して host.toml に登録し、
//! 参加に必要な情報一式をトークンにまとめる。
//!
//! **戻り値のトークンはメンバー秘密鍵を含む秘密情報**。ログへ出さず、
//! 表示・保存は呼び出し側が明示的に扱うこと(ADR-0008)。

use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::Path;

use anyhow::{bail, Context};
use peercove_core::config::{Config, DEFAULT_LISTEN_PORT};
use peercove_core::ipalloc::next_free_ip;
use peercove_core::keys::PresharedKey;
use peercove_core::keys::PrivateKey;
use peercove_core::token::{InviteToken, MAX_NAME_LEN};

use crate::peers::{append_peer, taken_dns_labels, used_ips, DnsExclude, NewPeer};

pub struct InviteOptions<'a> {
    pub config_path: &'a Path,
    /// 省略時 `member-<IP 第4オクテット>`
    pub name: Option<&'a str>,
    /// 省略時は空き IP を自動割当
    pub ip: Option<Ipv4Addr>,
    /// 追加のエンドポイント候補(外部 IP:ポート等)。LAN は自動で先頭に入る
    pub extra_endpoints: &'a [SocketAddrV4],
    /// メンバー用の事前共有鍵を発行する
    pub psk: bool,
    /// None は無期限。UI/CLI の既定は 7 日。
    pub expires_in_secs: Option<u64>,
}

pub struct InviteResult {
    /// **秘密情報**。pcv1 形式のトークン文字列
    pub token: String,
    pub name: String,
    pub ip: Ipv4Addr,
    pub endpoints: Vec<SocketAddrV4>,
    pub psk: bool,
    pub invite_id: String,
    pub issued_at: u64,
    pub expires_at: Option<u64>,
}

/// メンバーを host.toml に登録し、招待トークンを返す。
pub fn invite(options: &InviteOptions) -> anyhow::Result<InviteResult> {
    let config = Config::load(options.config_path)?;
    let subnet = config.interface.address.trunc();
    let listen_port = config.interface.listen_port.unwrap_or(DEFAULT_LISTEN_PORT);

    let used: Vec<Ipv4Addr> = used_ips(&config).collect();
    let ip = match options.ip {
        Some(ip) => ip, // 妥当性は append_peer が検証する
        None => next_free_ip(subnet, &used)
            .with_context(|| format!("サブネット {subnet} に空き IP がありません"))?,
    };

    let name = match options.name {
        Some(name) => name.to_string(),
        None => format!("member-{}", ip.octets()[3]),
    };
    validate_name(&name)?;
    if config
        .peers
        .iter()
        .any(|p| p.name.as_deref() == Some(name.as_str()))
    {
        bail!("名前 {name} のピアは既に存在します(別の名前を指定してください)");
    }

    // エンドポイント一覧: LAN(自動)→ 追加指定(外部など)の順
    let mut endpoints: Vec<SocketAddrV4> = Vec::new();
    if let Some(std::net::IpAddr::V4(lan_ip)) = crate::net::default_route_local_ip() {
        endpoints.push(SocketAddrV4::new(lan_ip, listen_port));
    }
    for ep in options.extra_endpoints {
        if !endpoints.contains(ep) {
            endpoints.push(*ep);
        }
    }
    if endpoints.is_empty() {
        bail!("エンドポイントを決定できませんでした。ホストへの到達先を指定してください");
    }

    // DNS 名を IP 割当と同時に確定する(ADR-0021、M3-14a)。以後この名前は
    // IP と独立に維持される。既定は表示名の正規化ラベル(空・予約語なら
    // member-<第4オクテット>)、重複は -<oct>、-<oct>-2、… で一意化
    let dns_name = {
        let taken = taken_dns_labels(&config, DnsExclude::None);
        let oct = ip.octets()[3];
        let base = peercove_core::names::normalize_dns_name(&name, false)
            .unwrap_or_else(|_| format!("member-{oct}"));
        if !taken.contains(&base) {
            base
        } else {
            let with_oct = format!("{base}-{oct}");
            if !taken.contains(&with_oct) {
                with_oct
            } else {
                (2..)
                    .map(|i| format!("{with_oct}-{i}"))
                    .find(|candidate| !taken.contains(candidate))
                    .expect("いつかは空きがある")
            }
        }
    };

    let member_private_key = PrivateKey::generate();
    let member_public_key = member_private_key.public_key();
    let preshared_key = options.psk.then(PresharedKey::generate);
    let psk_file_name = format!("peer-{ip}.psk");
    let issued_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("システム時刻が UNIX epoch より前です")?
        .as_secs();
    let expires_at = options
        .expires_in_secs
        .map(|seconds| {
            issued_at
                .checked_add(seconds)
                .context("招待期限が時刻範囲を超えます")
        })
        .transpose()?;
    let invite_id = peercove_core::token::generate_invite_id();
    if let Some(psk) = &preshared_key {
        let psk_path = options
            .config_path
            .parent()
            .unwrap_or(Path::new("."))
            .join(&psk_file_name);
        crate::secret::write_secret(&psk_path, &format!("{}\n", psk.to_base64()))
            .context("ホスト側 PSK ファイルの保存に失敗しました")?;
    }

    append_peer(
        options.config_path,
        &NewPeer {
            public_key: member_public_key,
            ip,
            name: Some(&name),
            dns_name: Some(&dns_name),
            preshared_key_file: preshared_key.as_ref().map(|_| psk_file_name.as_str()),
            invite_id: Some(&invite_id),
            invite_issued_at: Some(issued_at),
            invite_expires_at: expires_at,
        },
    )?;

    let token = InviteToken {
        member_private_key,
        host_public_key: host_public_key(&config)?,
        preshared_key,
        member_address: ipnet::Ipv4Net::new(ip, subnet.prefix_len()).expect("検証済み"),
        host_virtual_ip: config.interface.address.addr(),
        endpoints: endpoints.clone(),
        name: name.clone(),
        // 設定に名前が無い(旧設定)場合は None のまま = v1 トークン
        network: config.interface.network_name.clone(),
        invite_id: Some(invite_id.clone()),
        issued_at: Some(issued_at),
        expires_at,
    };

    Ok(InviteResult {
        token: token.encode()?,
        name,
        ip,
        endpoints,
        psk: options.psk,
        invite_id,
        issued_at,
        expires_at,
    })
}

fn host_public_key(config: &Config) -> anyhow::Result<peercove_core::keys::PublicKey> {
    let private = peercove_core::keys::read_private_key_file(&config.interface.private_key_file)
        .context("ホストの秘密鍵の読み込みに失敗しました")?;
    Ok(private.public_key())
}

/// 表示名の検証。TOML 追記と画面表示の安全のため、制御文字と引用符を拒否する。
pub fn validate_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty() || name.len() > MAX_NAME_LEN {
        bail!(
            "名前は 1〜{MAX_NAME_LEN} バイトにしてください(実際 {} バイト)",
            name.len()
        );
    }
    if name
        .chars()
        .any(|c| c.is_control() || c == '"' || c == '\\')
    {
        bail!("名前に制御文字・引用符・バックスラッシュは使えません");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use peercove_core::token::InviteToken;

    fn setup(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("peercove-ops-invite-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::init::init_host(&dir, "home", 51820, false)
            .unwrap()
            .config_path
    }

    fn options(config: &Path) -> InviteOptions<'_> {
        InviteOptions {
            config_path: config,
            name: None,
            ip: None,
            extra_endpoints: &[],
            psk: false,
            expires_in_secs: Some(7 * 24 * 60 * 60),
        }
    }

    #[test]
    fn invite_registers_peer_and_returns_token() {
        let config_path = setup("basic");
        let result = invite(&options(&config_path)).unwrap();

        let config = Config::load(&config_path).unwrap();
        assert_eq!(config.peers.len(), 1);
        assert_eq!(config.peers[0].name.as_deref(), Some(result.name.as_str()));
        assert_eq!(
            config.peers[0].dns_name.as_deref(),
            Some(result.name.as_str()),
            "DNS 名が invite 時に確定・永続化される(ADR-0021)"
        );

        let token = InviteToken::parse(&result.token).unwrap();
        assert_eq!(
            token.member_private_key.public_key(),
            config.peers[0].public_key
        );
        assert_eq!(token.member_address.addr(), result.ip);
        assert_eq!(token.host_virtual_ip, config.interface.address.addr());
        assert_eq!(token.invite_id.as_deref(), Some(result.invite_id.as_str()));
        assert_eq!(token.issued_at, Some(result.issued_at));
        assert_eq!(token.expires_at, result.expires_at);
        assert_eq!(
            config.peers[0].invite_id.as_deref(),
            Some(result.invite_id.as_str())
        );
        assert!(!token.endpoints.is_empty());
        assert_eq!(
            token.network.as_deref(),
            Some("home"),
            "ネットワーク名がトークンに載る"
        );

        // 2 人目は次の空き IP
        let second = invite(&options(&config_path)).unwrap();
        assert_ne!(second.ip, result.ip);
    }

    #[test]
    fn invite_expiry_and_first_device_are_host_enforced() {
        let config_path = setup("lifecycle");
        let mut opts = options(&config_path);
        opts.expires_in_secs = Some(60);
        let result = invite(&opts).unwrap();
        let first_device = "11111111111111111111111111111111";
        let other_device = "22222222222222222222222222222222";

        assert!(crate::peers::mark_invite_accepted_by_ip(
            &config_path,
            result.ip,
            result.issued_at + 1,
            Some(first_device),
        )
        .unwrap());
        assert!(!crate::peers::mark_invite_accepted_by_ip(
            &config_path,
            result.ip,
            result.issued_at + 2,
            Some(first_device),
        )
        .unwrap());
        assert!(crate::peers::mark_invite_accepted_by_ip(
            &config_path,
            result.ip,
            result.issued_at + 2,
            Some(other_device),
        )
        .is_err());

        let config = Config::load(&config_path).unwrap();
        assert_eq!(
            config.peers[0].invite_device_id.as_deref(),
            Some(first_device)
        );
    }

    #[test]
    fn expired_unused_invite_is_rejected() {
        let config_path = setup("expired");
        let mut opts = options(&config_path);
        opts.expires_in_secs = Some(1);
        let result = invite(&opts).unwrap();
        assert!(crate::peers::mark_invite_accepted_by_ip(
            &config_path,
            result.ip,
            result.issued_at + 2,
            Some("11111111111111111111111111111111"),
        )
        .is_err());
    }

    #[test]
    fn approval_required_invite_stays_isolated_until_approved() {
        let config_path = setup("approval");
        let text = std::fs::read_to_string(&config_path).unwrap();
        std::fs::write(
            &config_path,
            text.replacen(
                "[interface]",
                "[interface]\nrequire_invite_approval = true",
                1,
            ),
        )
        .unwrap();
        let result = invite(&options(&config_path)).unwrap();
        let device = "11111111111111111111111111111111";
        crate::peers::mark_invite_accepted_by_ip(
            &config_path,
            result.ip,
            result.issued_at + 1,
            Some(device),
        )
        .unwrap();
        let config = Config::load(&config_path).unwrap();
        assert!(config.peers[0].invite_is_isolated());
        assert_eq!(
            config.peers[0].invite_state(result.issued_at + 2),
            peercove_core::config::InviteState::AwaitingApproval
        );

        crate::peers::approve_invite(
            &config_path,
            &crate::peers::Selector::Ip(result.ip),
            result.issued_at + 2,
        )
        .unwrap();
        let config = Config::load(&config_path).unwrap();
        assert!(!config.peers[0].invite_is_isolated());
        assert_eq!(
            config.peers[0].invite_state(result.issued_at + 3),
            peercove_core::config::InviteState::Joined
        );
    }

    #[test]
    fn invite_with_psk_matches_host_side_file() {
        let config_path = setup("psk");
        let mut opts = options(&config_path);
        opts.psk = true;
        let result = invite(&opts).unwrap();

        let config = Config::load(&config_path).unwrap();
        let psk_path = config.peers[0].preshared_key_file.as_ref().unwrap();
        let host_psk = peercove_core::keys::read_preshared_key_file(psk_path).unwrap();
        let token = InviteToken::parse(&result.token).unwrap();
        assert_eq!(host_psk.as_bytes(), token.preshared_key.unwrap().as_bytes());
    }

    /// DNS 名の初期割当(ADR-0021): 日本語名は member-<oct>、
    /// 表示名由来のラベルが衝突したら -<oct> で一意化される。
    #[test]
    fn invite_assigns_default_dns_name() {
        let config_path = setup("dns-default");
        let mut opts = options(&config_path);
        opts.name = Some("たろう");
        let taro = invite(&opts).unwrap();
        let config = Config::load(&config_path).unwrap();
        assert_eq!(
            config.peers[0].dns_name.as_deref(),
            Some(format!("member-{}", taro.ip.octets()[3]).as_str()),
            "日本語名はフォールバック"
        );

        // 正規化すると同じ "alice pc" → 2 人目は -<oct> で一意化
        let mut opts = options(&config_path);
        opts.name = Some("Alice PC");
        invite(&opts).unwrap();
        let mut opts = options(&config_path);
        opts.name = Some("alice-pc");
        let second = invite(&opts).unwrap();
        let config = Config::load(&config_path).unwrap();
        assert_eq!(config.peers[1].dns_name.as_deref(), Some("alice-pc"));
        assert_eq!(
            config.peers[2].dns_name.as_deref(),
            Some(format!("alice-pc-{}", second.ip.octets()[3]).as_str())
        );
    }

    #[test]
    fn invite_rejects_duplicate_name() {
        let config_path = setup("dup");
        let mut opts = options(&config_path);
        opts.name = Some("alice");
        invite(&opts).unwrap();
        assert!(invite(&opts).is_err());
    }

    #[test]
    fn validate_name_rules() {
        assert!(validate_name("たろう").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name(&"x".repeat(65)).is_err());
        assert!(validate_name("a\"b").is_err());
        assert!(validate_name("a\nb").is_err());
    }
}
