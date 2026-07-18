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

pub mod chatlog;
pub mod engine;
pub mod groups;
pub mod session;
#[cfg(unix)]
mod tun_fd;

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use peercove_core::config::Config;
use peercove_core::keys::PrivateKey;
use peercove_core::msg::ChatScope;
use peercove_core::token::InviteToken;
use peercove_ops::networks::{self, NetworkEntry, Role};

use session::SessionShared;

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
    /// 受信ファイルサイズ上限(MB、0 = 無制限。mobile.toml、既定 10)
    pub max_recv_file_mb: u64,
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

/// 稼働中ネットワーク 1 本 = WG エンジン + プロトコルセッション(E-C)。
struct Running {
    engine: engine::Engine,
    session: session::NetSession,
}

impl Running {
    fn stop(self) {
        // セッション(コントロール・メッセージ)を先に畳んでからトンネルを落とす
        self.session.stop();
        self.engine.stop();
    }
}

fn tunnels() -> &'static Mutex<HashMap<String, Running>> {
    static TUNNELS: OnceLock<Mutex<HashMap<String, Running>>> = OnceLock::new();
    TUNNELS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn session_of(slug: &str) -> Option<Arc<SessionShared>> {
    tunnels()
        .lock()
        .unwrap()
        .get(slug)
        .map(|r| Arc::clone(&r.session.shared))
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
    // スマホの受信上限の既定(10 MB)をローカル設定として書いておく
    if let Err(e) =
        session::write_mobile_recv_limit_mb(&dir, session::MOBILE_DEFAULT_MAX_RECV_FILE_MB)
    {
        tracing::warn!("モバイル設定の書き込みに失敗しました: {e:#}");
    }
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
        max_recv_file_mb: session::read_mobile_recv_limit_mb(&entry.dir),
    })
}

/// 受信ファイルサイズ上限を変更する(MB、0 = 無制限)。即時反映
/// (セッションは申し出ごとに mobile.toml を読む)。
#[uniffi::export]
pub fn set_recv_limit_mb(base_dir: String, slug: String, mb: u64) -> Result<(), MobileError> {
    let dir = networks::networks_dir(Path::new(&base_dir)).join(&slug);
    session::write_mobile_recv_limit_mb(&dir, mb)?;
    Ok(())
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

    // プロトコルセッション(E-C): コントロールチャネル + メッセージング。
    // トンネルと同時に起動し、トンネル確立後に自動で接続する
    let control_host = peer
        .control_host
        .context("設定に control_host(ホスト仮想 IP)がありません")?;
    let own_ip = config.interface.address.addr();
    let new_session = session::NetSession::start(session::SessionConfig {
        slug: slug.to_string(),
        config_path: dir.join(networks::MEMBER_FILE),
        own_ip,
        display_name: config.interface.display_name.clone(),
        device_id: config.interface.device_id.clone(),
        network_name: config.network_name().to_string(),
        control_addr: std::net::SocketAddr::from((
            control_host,
            peercove_core::proto::CONTROL_PORT,
        )),
        listen_addr: std::net::SocketAddr::from((own_ip, peercove_core::msg::MSG_PORT)),
        peer_msg_port: peercove_core::msg::MSG_PORT,
    });

    let old = tunnels().lock().unwrap().insert(
        slug.to_string(),
        Running {
            engine: new_engine,
            session: new_session,
        },
    );
    if let Some(old) = old {
        old.stop(); // 同じネットワークの旧トンネルは置き換え
    }
    tracing::info!("トンネルを開始しました: {slug}");
    Ok(())
}

/// トンネルを停止する(未稼働なら何もしない・冪等)。
#[uniffi::export]
pub fn stop_tunnel(slug: String) {
    let running = tunnels().lock().unwrap().remove(&slug);
    if let Some(running) = running {
        running.stop();
        tracing::info!("トンネルを停止しました: {slug}");
    }
}

/// 稼働中トンネルの状態。None = そのネットワークは稼働していない。
#[uniffi::export]
pub fn tunnel_status(slug: String) -> Option<TunnelStatus> {
    let map = tunnels().lock().unwrap();
    map.get(&slug).map(|r| {
        let s = r.engine.stats();
        TunnelStatus {
            handshake_age_secs: s.handshake_age_secs,
            tx_bytes: s.tx_bytes,
            rx_bytes: s.rx_bytes,
        }
    })
}

// ---- E-C: セッション情報・メンバー・DNS・チャット・ファイル ------------------

/// プロトコルセッションの状態(コントロールチャネル)。
#[derive(uniffi::Record)]
pub struct SessionState {
    /// コントロールチャネルが接続済みか(= 台帳・バージョンが同期される状態)
    pub control_connected: bool,
    /// ホストから削除された(再接続しない)
    pub removed: bool,
    /// ホストが参加を拒否した理由(使用済み招待など。再接続しない)
    pub rejected: Option<String>,
    /// ホストとの RTT(コントロールチャネルの ping-pong)
    pub rtt_ms: Option<u64>,
}

/// 台帳のメンバー 1 名(UI のメンバー一覧・チャット宛先用)。
#[derive(uniffi::Record)]
pub struct MemberInfo {
    pub name: String,
    pub fqdn: String,
    pub ip: String,
    pub online: bool,
    pub is_host: bool,
    pub is_self: bool,
    pub blocked: bool,
    pub app_version: Option<String>,
}

/// チャット 1 通(UI 表示用)。
#[derive(uniffi::Record)]
pub struct ChatMessage {
    pub seq: u64,
    pub id: String,
    /// "direct" / "network" / "group"
    pub scope: String,
    pub group_id: Option<String>,
    pub from_ip: String,
    pub from_name: String,
    pub to_ip: Option<String>,
    pub text: String,
    pub sent_at: u64,
    pub outgoing: bool,
    pub system: bool,
    pub failed: bool,
    pub file_name: Option<String>,
    pub file_size: Option<u64>,
    pub file_path: Option<String>,
}

/// 自分が入っているグループ(トーク一覧用)。
#[derive(uniffi::Record)]
pub struct GroupSummary {
    pub id: String,
    pub name: String,
    pub member_ips: Vec<String>,
}

/// トンネル内 DNS の 1 エントリ(表示用。ADR-0040: スマホは OS の DNS を
/// 向けないため、アプリ内の一覧と IP 直接の URL で代替する)。
#[derive(uniffi::Record)]
pub struct DnsEntry {
    pub fqdn: String,
    pub value: String,
    /// scheme/port 付きレコードの接続 URL(IP ベース)
    pub url: Option<String>,
}

/// ファイル転送の進捗(UI がポーリングする)。
#[derive(uniffi::Record)]
pub struct TransferStatus {
    pub id: String,
    pub peer_ip: String,
    pub name: String,
    pub size: u64,
    pub done: u64,
    pub outgoing: bool,
    /// "running" / "done" / "failed: 理由"
    pub state: String,
}

#[uniffi::export]
pub fn session_state(slug: String) -> Option<SessionState> {
    let s = session_of(&slug)?;
    let rejected = s.rejected.lock().unwrap().clone();
    let rtt_ms = *s.rtt_ms.lock().unwrap();
    Some(SessionState {
        control_connected: s
            .control_connected
            .load(std::sync::atomic::Ordering::Relaxed),
        removed: s.removed.load(std::sync::atomic::Ordering::Relaxed),
        rejected,
        rtt_ms,
    })
}

/// 台帳のメンバー一覧(台帳未受信なら空)。
#[uniffi::export]
pub fn members(slug: String) -> Vec<MemberInfo> {
    let Some(s) = session_of(&slug) else {
        return Vec::new();
    };
    let network = s.cfg.network_name.clone();
    let own_ip = s.cfg.own_ip;
    let ledger = s.ledger.lock().unwrap();
    let Some(snapshot) = ledger.as_ref() else {
        return Vec::new();
    };
    snapshot
        .members
        .iter()
        .map(|m| {
            let label = if m.is_host {
                Some(peercove_core::names::HOST_DNS_LABEL.to_string())
            } else {
                m.dns_name
                    .clone()
                    .or_else(|| m.name.as_deref().and_then(peercove_core::names::dns_label))
            };
            MemberInfo {
                name: m.name.clone().unwrap_or_else(|| m.ip.to_string()),
                fqdn: label
                    .map(|l| format!("{l}.{network}.{}", peercove_core::dns::DNS_SUFFIX))
                    .unwrap_or_default(),
                ip: m.ip.to_string(),
                online: m.online,
                is_host: m.is_host,
                is_self: m.ip == own_ip,
                blocked: m.blocked,
                app_version: m.app_version.clone(),
            }
        })
        .collect()
}

/// カスタム DNS レコード + CNAME の一覧(表示用)。
#[uniffi::export]
pub fn dns_entries(slug: String) -> Vec<DnsEntry> {
    let Some(s) = session_of(&slug) else {
        return Vec::new();
    };
    let network = s.cfg.network_name.clone();
    let ledger = s.ledger.lock().unwrap();
    let Some(snapshot) = ledger.as_ref() else {
        return Vec::new();
    };
    let suffix = peercove_core::dns::DNS_SUFFIX;
    let mut out = Vec::new();
    for record in &snapshot.dns_records {
        let url = record.scheme.as_ref().map(|scheme| {
            let port = record.port.map(|p| format!(":{p}")).unwrap_or_default();
            format!("{scheme}://{}{port}", record.ip)
        });
        out.push(DnsEntry {
            fqdn: format!("{}.{network}.{suffix}", record.name),
            value: record.ip.to_string(),
            url,
        });
    }
    for cname in &snapshot.cname_records {
        let url = cname.scheme.as_ref().and_then(|scheme| {
            cname.resolved_ip.map(|ip| {
                let port = cname.port.map(|p| format!(":{p}")).unwrap_or_default();
                format!("{scheme}://{ip}{port}")
            })
        });
        out.push(DnsEntry {
            fqdn: format!("{}.{network}.{suffix}", cname.name),
            value: format!("→ {}", cname.target),
            url,
        });
    }
    out
}

/// 自分が入っているグループの一覧(トーク一覧用)。
#[uniffi::export]
pub fn chat_groups(slug: String) -> Vec<GroupSummary> {
    let Some(s) = session_of(&slug) else {
        return Vec::new();
    };
    let own_ip = s.cfg.own_ip;
    let joined = s.groups.lock().unwrap().joined(own_ip);
    joined
        .into_iter()
        .map(|g| GroupSummary {
            id: g.id,
            name: g.name,
            member_ips: g.members.iter().map(|ip| ip.to_string()).collect(),
        })
        .collect()
}

fn scope_to_str(scope: ChatScope) -> &'static str {
    match scope {
        ChatScope::Direct => "direct",
        ChatScope::Network => "network",
        ChatScope::Group => "group",
    }
}

fn scope_from_str(scope: &str) -> Result<ChatScope, MobileError> {
    match scope {
        "direct" => Ok(ChatScope::Direct),
        "network" => Ok(ChatScope::Network),
        "group" => Ok(ChatScope::Group),
        other => Err(MobileError::Failure {
            msg: format!("不明な宛先種別: {other}"),
        }),
    }
}

/// チャット履歴(`after_seq` より後を最大 `limit` 通)。ポーリングで差分取得する。
#[uniffi::export]
pub fn chat_fetch(slug: String, after_seq: u64, limit: u32) -> Vec<ChatMessage> {
    let Some(s) = session_of(&slug) else {
        return Vec::new();
    };
    let own_ip = s.cfg.own_ip;
    let entries = s
        .chat
        .lock()
        .unwrap()
        .fetch(after_seq, limit.min(500) as usize);
    entries
        .into_iter()
        .map(|e| ChatMessage {
            seq: e.seq,
            id: e.id,
            scope: scope_to_str(e.scope).to_string(),
            group_id: e.group_id,
            from_ip: e.from.to_string(),
            from_name: s.member_display(e.from),
            to_ip: e.to.map(|ip| ip.to_string()),
            text: e.text,
            sent_at: e.sent_at,
            outgoing: e.from == own_ip,
            system: e.system,
            failed: e.failed,
            file_name: e.file.as_ref().map(|f| f.name.clone()),
            file_size: e.file.as_ref().map(|f| f.size),
            file_path: e
                .file
                .as_ref()
                .and_then(|f| f.path.as_ref())
                .map(|p| p.to_string_lossy().to_string()),
        })
        .collect()
}

/// 履歴全体の最新 seq(0 = 履歴なし)。UI の差分ポーリング用。
#[uniffi::export]
pub fn chat_latest_seq(slug: String) -> u64 {
    session_of(&slug)
        .map(|s| s.chat.lock().unwrap().latest_seq())
        .unwrap_or(0)
}

/// チャットを送る(scope: "direct" / "network" / "group")。
/// ブロッキング(ネットワーク I/O)なので Kotlin 側は IO ディスパッチャで呼ぶ。
#[uniffi::export]
pub fn send_chat_message(
    slug: String,
    scope: String,
    to_ip: Option<String>,
    group_id: Option<String>,
    text: String,
) -> Result<(), MobileError> {
    let s = session_of(&slug).ok_or_else(|| MobileError::Failure {
        msg: "接続していません".to_string(),
    })?;
    let scope = scope_from_str(&scope)?;
    let to: Option<Ipv4Addr> = match to_ip {
        Some(ip) => Some(ip.parse().map_err(|_| MobileError::Failure {
            msg: "宛先 IP が不正です".to_string(),
        })?),
        None => None,
    };
    s.send_chat(scope, to, group_id, text).map_err(Into::into)
}

/// ファイルを 1 人へ送る(チャットの文脈付き)。戻り値は転送 ID。
/// ブロッキング(転送完了まで返らない)なので Kotlin 側は IO ディスパッチャで呼ぶ。
#[uniffi::export]
pub fn send_file_to(slug: String, to_ip: String, src_path: String) -> Result<String, MobileError> {
    let s = session_of(&slug).ok_or_else(|| MobileError::Failure {
        msg: "接続していません".to_string(),
    })?;
    let target: Ipv4Addr = to_ip.parse().map_err(|_| MobileError::Failure {
        msg: "宛先 IP が不正です".to_string(),
    })?;
    s.send_file(target, Path::new(&src_path))
        .map_err(Into::into)
}

/// ファイル転送の進捗一覧。
#[uniffi::export]
pub fn transfers(slug: String) -> Vec<TransferStatus> {
    let Some(s) = session_of(&slug) else {
        return Vec::new();
    };
    let list = s.transfers.lock().unwrap().clone();
    list.into_iter()
        .map(|t| TransferStatus {
            id: t.id,
            peer_ip: t.peer.to_string(),
            name: t.name,
            size: t.size,
            done: t.done,
            outgoing: t.outgoing,
            state: t.state,
        })
        .collect()
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
