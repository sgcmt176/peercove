//! PeerCove モバイル用コア(M4、ADR-0039 / ADR-0040)。
//!
//! Android アプリ(apps/peercove-android)から UniFFI 経由で呼ばれる。
//! 役割分担は「頭脳は Rust、OS との付き合いは Kotlin」:
//! ここに参加(join)・WG トンネル・(E-C で)台帳同期・チャット/ファイルの
//! プロトコル処理を実装し、OS 連携(VpnService・通知・保存先)は Kotlin が担う。
//!
//! E-B 時点の公開 API:
//! - `join_network` … 招待トークン(QR/貼り付け)から networks/<slug>/ を作る
//! - `list_networks` / `remove_network` … 参加済みネットワークの一覧・削除
//! - `start_tunnel` / `stop_tunnel` / `tunnel_status` … VpnService の TUN fd で
//!   WG トンネル(engine.rs)を起動・停止・監視する

uniffi::setup_scaffolding!();

pub mod engine;
#[cfg(unix)]
mod tun_fd;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use peercove_core::config::Config;
use peercove_core::keys::PrivateKey;
use peercove_core::token::InviteToken;
use peercove_ops::networks::{self, NetworkEntry, Role};

/// Kotlin へ届けるエラー(uniffi flat error = メッセージ文字列のみ)。
/// 秘密(鍵・PSK・トークン本体)をメッセージに含めないこと。
#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum MobileError {
    #[error("{msg}")]
    Failure { msg: String },
}

impl From<anyhow::Error> for MobileError {
    fn from(e: anyhow::Error) -> Self {
        MobileError::Failure {
            msg: format!("{e:#}"),
        }
    }
}

impl From<peercove_core::Error> for MobileError {
    fn from(e: peercove_core::Error) -> Self {
        MobileError::Failure { msg: e.to_string() }
    }
}

/// 参加済みネットワーク 1 件(UI の一覧・VpnService の Builder 用)。
#[derive(uniffi::Record)]
pub struct NetworkInfo {
    /// ディレクトリ名(networks/<slug>/)。API のキーとして使う
    pub slug: String,
    /// ネットワーク名
    pub name: String,
    /// 自分の表示名(台帳用)
    pub display_name: String,
    /// 自分の仮想 IP(VpnService の addAddress 用)
    pub member_ip: String,
    pub prefix_len: u8,
    /// サブネットのネットワークアドレス(addRoute 用)
    pub subnet_addr: String,
    /// ホストの仮想 IP(疎通確認・コントロールチャネルの接続先)
    pub host_ip: String,
    /// ホストの外部エンドポイント(表示用)
    pub endpoint: String,
    pub mtu: u16,
}

/// 稼働中トンネルの状態(2 秒ポーリング想定)。
#[derive(uniffi::Record)]
pub struct TunnelStatus {
    /// 最終ハンドシェイクからの経過秒。None = 未確立(接続試行中)
    pub handshake_age_secs: Option<u64>,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
}

/// Android の VpnService.protect() を Rust から呼ぶためのコールバック。
/// WG の UDP ソケットを VPN のルーティングから除外する(トンネルの中に
/// トンネル自身のパケットを吸い込ませない)。
#[uniffi::export(with_foreign)]
pub trait SocketProtector: Send + Sync {
    fn protect(&self, fd: i32) -> bool;
}

fn tunnels() -> &'static Mutex<HashMap<String, engine::Engine>> {
    static TUNNELS: OnceLock<Mutex<HashMap<String, engine::Engine>>> = OnceLock::new();
    TUNNELS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// logcat へのログ出力を初期化する(アプリ起動時に 1 回呼ぶ)。
/// tracing の "log" フィーチャ経由で Rust 側の tracing イベントも流れる。
#[uniffi::export]
pub fn init_logging() {
    #[cfg(target_os = "android")]
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("peercove"),
    );
}

/// モバイルコアのバージョン文字列(アプリの情報表示用)
#[uniffi::export]
pub fn core_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// E-A の疎通確認: peercove-core の暗号(X25519 鍵生成)が Android 上で
/// 動くことを証明する。新規生成した鍵の公開鍵 base64 を返す。
/// 秘密鍵は返さない・ログにも出さない(CLAUDE.md の秘匿規約)。
#[uniffi::export]
pub fn probe_core() -> String {
    let key = PrivateKey::generate();
    format!("core ok / pubkey {}", key.public_key().to_base64())
}

/// 貼り付け・QR・ディープリンクのどれで来ても素のトークンに揃える。
fn normalize_token(input: &str) -> String {
    let text = input.trim();
    // peercove://join?token=pcv1.… (M3-5 のディープリンク、QR に入ることもある)
    if let Some((_, query)) = text.split_once('?') {
        for pair in query.split('&') {
            if let Some(token) = pair.strip_prefix("token=") {
                return token.trim().to_string();
            }
        }
    }
    text.to_string()
}

/// 招待トークンで参加する。networks/<slug>/ に member.toml・鍵を作り、
/// 参加したネットワークの情報を返す(トンネルはまだ張らない)。
#[uniffi::export]
pub fn join_network(base_dir: String, token: String) -> Result<NetworkInfo, MobileError> {
    let base = Path::new(&base_dir);
    let token_text = normalize_token(&token);
    let parsed = InviteToken::parse(&token_text)?;
    let network_name = parsed
        .network
        .clone()
        .unwrap_or_else(|| peercove_core::names::DEFAULT_NETWORK_NAME.to_string());
    let (slug, dir) = networks::join_dir(base, &network_name)?;
    peercove_ops::join::join(&token_text, &dir, false)?;
    tracing::info!("ネットワーク {network_name} に参加しました(slug={slug})");
    list_networks(base_dir)
        .into_iter()
        .find(|n| n.slug == slug)
        .ok_or_else(|| MobileError::Failure {
            msg: "参加直後のネットワークが見つかりません(バグの可能性)".to_string(),
        })
}

/// 参加済みネットワークの一覧(メンバー役のみ。スマホはホストにならない)。
#[uniffi::export]
pub fn list_networks(base_dir: String) -> Vec<NetworkInfo> {
    networks::list(Path::new(&base_dir))
        .iter()
        .filter(|e| e.role == Role::Member)
        .filter_map(network_info)
        .collect()
}

/// ネットワークを削除する(鍵・設定ごと)。稼働中なら先に停止する。
#[uniffi::export]
pub fn remove_network(base_dir: String, slug: String) -> Result<(), MobileError> {
    stop_tunnel(slug.clone());
    networks::delete(Path::new(&base_dir), &slug)?;
    Ok(())
}

fn network_info(entry: &NetworkEntry) -> Option<NetworkInfo> {
    let config = Config::load(&entry.config_path).ok()?;
    let peer = config.peers.first()?;
    Some(NetworkInfo {
        slug: entry.slug.clone(),
        name: entry.name.clone(),
        display_name: config.interface.display_name.clone().unwrap_or_default(),
        member_ip: entry.address.addr().to_string(),
        prefix_len: entry.address.prefix_len(),
        subnet_addr: entry.address.network().to_string(),
        host_ip: peer
            .control_host
            .map(|ip| ip.to_string())
            .unwrap_or_default(),
        endpoint: peer.endpoint.map(|ep| ep.to_string()).unwrap_or_default(),
        mtu: config.interface.mtu,
    })
}

/// VpnService の TUN fd で WG トンネルを開始する。
/// `tun_fd` の所有権はこの呼び出しで Rust 側へ移る(停止時に close)。
#[uniffi::export]
pub fn start_tunnel(
    base_dir: String,
    slug: String,
    tun_fd: i32,
    protector: Arc<dyn SocketProtector>,
) -> Result<(), MobileError> {
    #[cfg(unix)]
    {
        start_tunnel_impl(&base_dir, &slug, tun_fd, protector).map_err(Into::into)
    }
    #[cfg(not(unix))]
    {
        let _ = (base_dir, slug, tun_fd, protector);
        Err(MobileError::Failure {
            msg: "この OS ではトンネルを起動できません(Android 専用)".to_string(),
        })
    }
}

#[cfg(unix)]
fn start_tunnel_impl(
    base_dir: &str,
    slug: &str,
    tun_fd: i32,
    protector: Arc<dyn SocketProtector>,
) -> anyhow::Result<()> {
    use anyhow::Context;
    use std::os::fd::AsRawFd;

    // fd の所有権は受け取った時点でこちらに移っている。以降どのエラー経路でも
    // FdTun(OwnedFd)の drop で close される
    let tun = Arc::new(tun_fd::FdTun::from_raw(tun_fd));

    let dir = networks::networks_dir(Path::new(base_dir)).join(slug);
    let config = Config::load(&dir.join(networks::MEMBER_FILE))
        .with_context(|| format!("ネットワーク {slug} の設定を読めません"))?;
    let private = peercove_core::keys::read_private_key_file(&config.interface.private_key_file)?;
    let peer = config
        .peers
        .iter()
        .find(|p| p.endpoint.is_some())
        .context("エンドポイント付きのピア(ホスト)が設定にありません")?;
    let psk = match &peer.preshared_key_file {
        Some(path) => Some(peercove_core::keys::read_preshared_key_file(path)?),
        None => None,
    };

    let spec = engine::EngineSpec {
        private_key: *private.as_bytes(),
        peer_public_key: *peer.public_key.as_bytes(),
        preshared_key: psk.map(|k| *k.as_bytes()),
        endpoint: peer.endpoint.expect("上で確認済み"),
        allowed_ips: peer.allowed_ips.clone(),
        persistent_keepalive: peer.persistent_keepalive,
    };

    let udp = std::net::UdpSocket::bind(("0.0.0.0", 0)).context("UDP ソケットを作れません")?;
    // VPN ルーティングからの除外。ハブ&スポークはサブネット split ルートなので
    // 失敗しても即座に困らないが、経路がかぶる構成への保険として警告は残す
    if !protector.protect(udp.as_raw_fd()) {
        tracing::warn!("VpnService.protect が失敗しました(トンネルのループに注意)");
    }

    let new_engine = engine::Engine::start(spec, tun, udp)?;
    let old = tunnels()
        .lock()
        .unwrap()
        .insert(slug.to_string(), new_engine);
    if let Some(old) = old {
        old.stop(); // 同じネットワークの旧トンネルは置き換え
    }
    tracing::info!("トンネルを開始しました: {slug}");
    Ok(())
}

/// トンネルを停止する(未稼働なら何もしない・冪等)。
#[uniffi::export]
pub fn stop_tunnel(slug: String) {
    let engine = tunnels().lock().unwrap().remove(&slug);
    if let Some(engine) = engine {
        engine.stop();
        tracing::info!("トンネルを停止しました: {slug}");
    }
}

/// 稼働中トンネルの状態。None = そのネットワークは稼働していない。
#[uniffi::export]
pub fn tunnel_status(slug: String) -> Option<TunnelStatus> {
    let map = tunnels().lock().unwrap();
    map.get(&slug).map(|e| {
        let s = e.stats();
        TunnelStatus {
            handshake_age_secs: s.handshake_age_secs,
            tx_bytes: s.tx_bytes,
            rx_bytes: s.rx_bytes,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use peercove_core::keys::PresharedKey;

    fn sample_token(network: &str) -> String {
        InviteToken {
            member_private_key: PrivateKey::generate(),
            host_public_key: PrivateKey::generate().public_key(),
            preshared_key: Some(PresharedKey::generate()),
            member_address: "10.77.0.5/24".parse().unwrap(),
            host_virtual_ip: "10.77.0.1".parse().unwrap(),
            endpoints: vec!["192.168.10.2:51820".parse().unwrap()],
            name: "sumaho".to_string(),
            network: Some(network.to_string()),
            invite_id: None,
            issued_at: None,
            expires_at: None,
        }
        .encode()
        .unwrap()
    }

    fn temp_base(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("peercove-mobile-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn core_version_is_semver_like() {
        let v = core_version();
        assert!(v.split('.').count() >= 3, "version = {v}");
    }

    #[test]
    fn probe_core_returns_public_key() {
        let s = probe_core();
        assert!(s.starts_with("core ok / pubkey "));
        let b64 = s.rsplit(' ').next().unwrap();
        assert_eq!(b64.len(), 44);
    }

    #[test]
    fn normalize_token_handles_deep_link_and_plain() {
        assert_eq!(normalize_token(" pcv1.abc \n"), "pcv1.abc");
        assert_eq!(
            normalize_token("peercove://join?token=pcv1.abc"),
            "pcv1.abc"
        );
        assert_eq!(
            normalize_token("peercove://join?foo=1&token=pcv1.abc"),
            "pcv1.abc"
        );
    }

    #[test]
    fn join_then_list_then_remove_roundtrip() {
        let base = temp_base("join-roundtrip");
        let base_str = base.to_string_lossy().to_string();

        let info = join_network(base_str.clone(), sample_token("sumaho-net")).unwrap();
        assert_eq!(info.name, "sumaho-net");
        assert_eq!(info.member_ip, "10.77.0.5");
        assert_eq!(info.prefix_len, 24);
        assert_eq!(info.subnet_addr, "10.77.0.0");
        assert_eq!(info.host_ip, "10.77.0.1");
        assert_eq!(info.endpoint, "192.168.10.2:51820");
        assert_eq!(info.display_name, "sumaho");

        let listed = list_networks(base_str.clone());
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].slug, info.slug);

        // 二重参加は上書きガードで失敗する(force しない)
        assert!(join_network(base_str.clone(), sample_token("sumaho-net")).is_err());

        remove_network(base_str.clone(), info.slug).unwrap();
        assert!(list_networks(base_str).is_empty());
    }

    #[test]
    fn tunnel_status_is_none_for_unknown_network() {
        assert!(tunnel_status("no-such-network".to_string()).is_none());
    }
}
