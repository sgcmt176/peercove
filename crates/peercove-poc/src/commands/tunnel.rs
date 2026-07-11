use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use anyhow::{bail, Context};
use peercove_core::config::{Config, PeerConfig, DEFAULT_LISTEN_PORT};
use peercove_core::keys::{read_preshared_key_file, read_private_key_file};
use peercove_core::proto::LedgerEntry;

use crate::backend::{create_backend, PeerSpec, PeerStats, TunnelSpec, WgBackend};
use crate::commands::status;
use crate::control;

/// 設定再読込とステータスファイル書き出しの周期(ADR-0002)。
const SUPERVISE_INTERVAL: Duration = Duration::from_secs(5);
/// 最終ハンドシェイクがこれ以内なら「オンライン」とみなす(WG の
/// セッション有効期限 180 秒に合わせる)。
const ONLINE_THRESHOLD: Duration = Duration::from_secs(180);
/// 台帳に載せるエンドポイント観測経過秒の粒度(ADR-0013)。
/// 生の経過秒だと毎周期(5 秒)値が変わり、台帳が「変更あり」として
/// 全メンバーへ再配布され続けてしまう。60 秒粒度なら鮮度判定には十分で、
/// 再配布も最大毎分 1 回に抑えられる。
const ENDPOINT_AGE_STEP_SECS: u64 = 60;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Host,
    Member,
}

/// 起動済みトンネル一式(バックエンド + UPnP リース)。
/// [`bring_up`] で作り、[`tear_down`] で必ず対で破棄する。
pub struct ActiveTunnel {
    pub backend: Box<dyn WgBackend>,
    pub spec: TunnelSpec,
    pub role: Role,
    /// 実際に使ったインターフェース名(複数ネットワーク時は自動採番 — ADR-0012)。
    pub if_name: String,
    /// ネットワーク名(設定の network_name、旧設定は既定名)。
    pub network: String,
    upnp_lease: Option<crate::upnp::UpnpLease>,
}

/// 稼働中トンネルとの衝突検査に使う情報(ADR-0012 §3)。
/// 単発 CLI(host / member コマンド)ではデーモンが居ないので空 = 検査なし。
#[derive(Default)]
pub struct StartLimits {
    /// 稼働中の (サブネット, ネットワーク名)。重複はエラー。
    pub used_subnets: Vec<(ipnet::Ipv4Net, String)>,
    /// 稼働中のインターフェース名。既定名の場合は空き番号へ避ける。
    pub used_if_names: Vec<String>,
}

/// 使うインターフェース名を決め、サブネットの重複を検査する(純関数)。
///
/// - サブネットが稼働中ネットワークと重なる → エラー(黙って壊れない)
/// - 設定で明示されたインターフェース名が使用中 → エラー
/// - 既定名(`peercove0`)が使用中 → `peercove1`, `peercove2`, …の空き番号
pub fn plan_interface(config: &Config, limits: &StartLimits) -> anyhow::Result<String> {
    let subnet = config.interface.address.trunc();
    for (used, network) in &limits.used_subnets {
        if used.contains(&subnet.addr()) || subnet.contains(&used.addr()) {
            bail!(
                "サブネット {subnet} は稼働中のネットワーク \"{network}\"({used})と重なっています。\
                 どちらかを別のサブネットで作り直してください"
            );
        }
    }
    let name = &config.interface.name;
    let in_use = |name: &str| limits.used_if_names.iter().any(|used| used == name);
    if name != peercove_core::config::DEFAULT_IF_NAME {
        if in_use(name) {
            bail!("インターフェース名 {name} は稼働中のトンネルが使用しています");
        }
        return Ok(name.clone());
    }
    Ok((0..)
        .map(|i| format!("peercove{i}"))
        .find(|candidate| !in_use(candidate))
        .expect("空き番号は必ずある"))
}

/// 設定を読み、UPnP(任意)→ トンネル作成までを行う(CLI / daemon 共通)。
pub fn bring_up(
    config_path: &Path,
    role: Role,
    upnp: bool,
    limits: &StartLimits,
) -> anyhow::Result<ActiveTunnel> {
    let config = Config::load(config_path)?;
    if peercove_core::ipalloc::overlaps_cgnat(config.interface.address.trunc()) {
        tracing::warn!(
            "トンネルのサブネット {} は CGNAT レンジ(100.64.0.0/10)内です。\
             Tailscale が動作しているマシンではパケットが破棄されます。\
             `peercove-poc init` で生成した設定への移行を推奨します(ADR-0006)",
            config.interface.address.trunc()
        );
    }
    let if_name = plan_interface(&config, limits)?;
    let spec = build_spec(&config, role)?;
    let mut backend = create_backend(&if_name)?;

    // UPnP はトンネル作成前に試行する(TUN のマルチキャスト経路が SSDP 探索を
    // 妨げないように)。失敗してもトンネルは起動する(手動ポートフォワードで代替可能)
    let listen_port = spec.listen_port.unwrap_or(DEFAULT_LISTEN_PORT);
    let upnp_lease = if upnp && role == Role::Host {
        match crate::upnp::setup(listen_port) {
            Ok(report) => {
                println!("UPnP ポート開放に成功しました(UDP {listen_port}、リース 24 時間)");
                println!(
                    "外部エンドポイント(推定): {}:{}",
                    report.external_ip, report.external_port
                );
                println!("→ 別 NAT のメンバーは endpoint にこれを指定してください");
                Some(report.lease)
            }
            Err(e) => {
                tracing::warn!("UPnP: {e:#}");
                println!("UPnP ポート開放は失敗しました(トンネルは起動します)");
                None
            }
        }
    } else {
        None
    };

    backend.up(&spec)?;
    tracing::info!(
        "トンネル {if_name} を作成しました(network={} address={} mtu={} peers={})",
        config.network_name(),
        config.interface.address,
        spec.mtu,
        spec.peers.len()
    );
    Ok(ActiveTunnel {
        backend,
        spec,
        role,
        if_name,
        network: config.network_name().to_string(),
        upnp_lease,
    })
}

impl ActiveTunnel {
    /// 停止シグナルまで supervisor を回す(daemon 用の入り口)。
    pub async fn supervise_run(
        &mut self,
        config_path: &Path,
        stop: tokio::sync::watch::Receiver<bool>,
        snapshot: Option<SharedSnapshot>,
        transfers: crate::msg::TransferRegistry,
        chat: crate::chat::SharedChatLog,
        groups: crate::groups::SharedGroups,
    ) -> anyhow::Result<()> {
        supervise(
            config_path,
            self.role,
            self.backend.as_mut(),
            &self.spec,
            stop,
            snapshot,
            transfers,
            chat,
            groups,
        )
        .await
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(spec: TunnelSpec, role: Role, backend: Box<dyn WgBackend>) -> Self {
        Self {
            backend,
            spec,
            role,
            if_name: "peercove-test".to_string(),
            network: "test".to_string(),
            upnp_lease: None,
        }
    }
}

/// トンネルと関連リソースを対で破棄する。
pub fn tear_down(mut tunnel: ActiveTunnel, config_path: &Path) -> anyhow::Result<()> {
    if let Some(lease) = tunnel.upnp_lease.take() {
        lease.release();
    }
    let _ = std::fs::remove_file(status::status_file_path(config_path));
    tunnel.backend.down()
}

/// host / member 共通: トンネルを作成し、Ctrl+C まで維持して破棄する(CLI モード)。
pub fn run_up(config_path: &Path, role: Role, upnp: bool) -> anyhow::Result<()> {
    // 単発 CLI はデーモンの稼働状況を知らないため衝突検査なし(従来どおり)
    let mut tunnel = bring_up(config_path, role, upnp, &StartLimits::default())?;
    println!(
        "トンネルを作成しました(address={} peers={})",
        tunnel.spec.address,
        tunnel.spec.peers.len()
    );
    if role == Role::Host {
        println!(
            "待受ポート: UDP {}(メンバーの endpoint にはこのポートを指定)",
            tunnel.spec.listen_port.unwrap_or(DEFAULT_LISTEN_PORT)
        );
    }
    println!("Ctrl+C で終了します(トンネルをクリーンアップします)");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("非同期ランタイムの初期化に失敗しました")?;
    let supervise_result = runtime.block_on(async {
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        let ctrl_c = tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            let _ = stop_tx.send(true);
        });
        let result = supervise(
            config_path,
            role,
            tunnel.backend.as_mut(),
            &tunnel.spec,
            stop_rx,
            None,
            Default::default(),
            crate::chat::ChatLog::load(config_path),
            crate::groups::GroupStore::load(config_path),
        )
        .await;
        ctrl_c.abort();
        result
    });
    println!("終了処理中…");
    tear_down(tunnel, config_path)?;
    println!("クリーンアップが完了しました");
    supervise_result
}

/// supervise が周期的に更新する状態(daemon の status 応答用)。
#[derive(Default)]
pub struct Snapshot {
    pub peers: Vec<PeerStats>,
    /// host は自前構築した台帳、member は受信済みの台帳(未受信なら None)。
    pub ledger: Option<Vec<LedgerEntry>>,
    /// カスタム DNS レコード(M3-1)。host は自分の設定、member は受信したもの。
    pub dns_records: Vec<peercove_core::dns::DnsRecord>,
    /// 相手の仮想 IP → コントロールチャネルで測った RTT(ミリ秒、M2-G5)。
    pub rtt_ms: HashMap<std::net::Ipv4Addr, f64>,
    /// (member のみ)ホストからネットワーク削除された(M2-G6)。UI が明示する。
    pub removed: bool,
    /// (member のみ)相手の仮想 IP → 直接経路の状態(ADR-0013、M3-4)。
    pub direct: HashMap<std::net::Ipv4Addr, peercove_core::ipc::DirectStatus>,
}

pub type SharedSnapshot = Arc<Mutex<Option<Snapshot>>>;

/// 停止シグナルまで 5 秒周期で以下を行う(CLI = Ctrl+C / daemon = stop 要求):
/// - (host のみ)設定を再読込し、ピアの追加・変更・削除を同期(ADR-0002 / M1-G3/7)
/// - (host)台帳を更新してコントロールチャネルへ配布 /(member)受信台帳の反映
/// - ステータスファイルと共有スナップショットの書き出し
#[allow(clippy::too_many_arguments)]
pub async fn supervise(
    config_path: &Path,
    role: Role,
    backend: &mut dyn WgBackend,
    spec: &TunnelSpec,
    mut stop: tokio::sync::watch::Receiver<bool>,
    snapshot: Option<SharedSnapshot>,
    transfers: crate::msg::TransferRegistry,
    chat: crate::chat::SharedChatLog,
    groups: crate::groups::SharedGroups,
) -> anyhow::Result<()> {
    // 登録済みピア(公開鍵 → 設定のフィンガープリント)。変更検知と
    // 削除通知の宛先解決に使う
    let mut known: HashMap<[u8; 32], PeerFingerprint> = spec
        .peers
        .iter()
        .map(|p| (*p.public_key.as_bytes(), PeerFingerprint::of(p)))
        .collect();
    // 削除通知済み・次の周期で実削除するピア
    let mut pending_removal: HashSet<[u8; 32]> = HashSet::new();
    let status_path = status::status_file_path(config_path);
    let host_public_key = spec.private_key.public_key();
    {
        // コントロールチャネル(M1-G2)
        let (ledger_tx, ledger_rx) = tokio::sync::watch::channel(control::Distribution::default());
        let connections: control::Connections = Default::default();
        let member_ledger: Arc<Mutex<Option<control::ReceivedDistribution>>> = Default::default();
        // (member)直接ピアの管理(ADR-0013、M3-3)。host_public_key は
        // member ロールでは「自分の公開鍵」(台帳の自エントリ除外に使う)
        let mut direct =
            crate::direct::DirectManager::new(*host_public_key.as_bytes(), spec.address.trunc());
        let rtt: control::RttMap = Default::default();
        // (member)ホストから削除されたら立つ。status に載せて UI が表示する
        let removed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        // サブネットルーター(ADR-0014): 適用済みの OS ルート、member の
        // ホストピアに足した AllowedIPs、ルーター役エラーのスパム防止フラグ
        let mut routes: std::collections::BTreeSet<ipnet::Ipv4Net> = Default::default();
        let mut member_extra: Vec<ipnet::Ipv4Net> = Vec::new();
        let mut router_error_logged = false;
        let mut tasks = Vec::new();
        // メッセージング基盤(ADR-0015、M3-9): 両ロールとも自分の仮想 IP で
        // 待受ける。接続元の照合に使うピア表と受信サイズ上限は毎周期更新する
        let msg_peers: crate::msg::SharedPeers = Default::default();
        let msg_limit: crate::msg::SharedLimit = Arc::new(Mutex::new(crate::msg::limit_bytes(
            Config::load(config_path)
                .map(|c| c.interface.max_recv_file_mb)
                .unwrap_or(peercove_core::config::DEFAULT_MAX_RECV_FILE_MB),
        )));
        tasks.push(tokio::spawn(crate::msg::run_server(
            spec.address.addr(),
            Arc::clone(&msg_peers),
            crate::msg::inbox_dir(config_path),
            Arc::clone(&transfers),
            Arc::clone(&msg_limit),
            Arc::clone(&chat),
            Arc::clone(&groups),
        )));
        match role {
            Role::Host => {
                tasks.push(tokio::spawn(control::run_host_server(
                    spec.address.addr(),
                    ledger_rx,
                    Arc::clone(&connections),
                    Arc::clone(&rtt),
                )));
            }
            Role::Member => {
                // 接続先: join が書いた control_host。無ければ慣例(サブネット先頭)
                let config = Config::load(config_path)?;
                let host_ip = config
                    .peers
                    .first()
                    .and_then(|p| p.control_host)
                    .or_else(|| spec.address.trunc().hosts().next());
                match host_ip {
                    Some(host_ip) if host_ip != spec.address.addr() => {
                        tasks.push(tokio::spawn(control::run_member_client(
                            host_ip,
                            config.interface.display_name.clone(),
                            Arc::clone(&member_ledger),
                            Arc::clone(&rtt),
                            Arc::clone(&removed),
                        )));
                    }
                    _ => tracing::warn!(
                        "コントロールチャネルの接続先が決められないため台帳同期を行いません"
                    ),
                }
            }
        }

        let mut tick = tokio::time::interval(SUPERVISE_INTERVAL);
        let result = loop {
            tokio::select! {
                _ = stop.changed() => {
                    break Ok(());
                }
                _ = tick.tick() => {
                    let config = match Config::load(config_path) {
                        Ok(config) => Some(config),
                        Err(e) => {
                            tracing::warn!("設定の再読込に失敗しました(前回の設定で継続): {e:#}");
                            None
                        }
                    };
                    // 受信サイズ上限(ADR-0015)を設定に追随させる(両ロール)
                    if let Some(config) = &config {
                        *msg_limit.lock().unwrap() =
                            crate::msg::limit_bytes(config.interface.max_recv_file_mb);
                    }
                    if role == Role::Host {
                        if let Some(config) = &config {
                            sync_peers(
                                config,
                                backend,
                                &mut known,
                                &mut pending_removal,
                                &connections,
                            );
                            // ホスト自身が広告サブネットへ届くための OS ルート
                            // (メンバー間のリレーは AllowedIPs / デバイス内
                            // 最長一致で成立する — ADR-0014)
                            let desired = config
                                .peers
                                .iter()
                                .flat_map(|p| p.subnets.iter().copied())
                                .collect();
                            sync_routes(backend, &desired, &mut routes);
                        }
                    }
                    let stats = match backend.stats() {
                        Ok(stats) => stats,
                        Err(e) => {
                            tracing::debug!("統計の取得に失敗: {e:#}");
                            continue;
                        }
                    };
                    // 台帳 + DNS レコード: host は設定+統計から構築して配布、
                    // member は受信済みを表示(M3-1 で dns_records が加わった)
                    let mut direct_routes = HashMap::new();
                    let distribution = match role {
                        Role::Host => config.as_ref().map(|config| {
                            let dist = control::Distribution {
                                members: build_ledger(config, &host_public_key, &stats),
                                dns_records: config.dns_records.clone(),
                            };
                            ledger_tx.send_if_modified(|current| {
                                if *current != dist {
                                    *current = dist.clone();
                                    true
                                } else {
                                    false
                                }
                            });
                            dist
                        }),
                        Role::Member => {
                            let received = member_ledger.lock().unwrap().clone();
                            // 直接ピアの追加・確認・除去(ADR-0013、M3-3)。
                            // 設定が読めない周期は現状維持(触らない)
                            let enabled =
                                config.as_ref().map(|c| c.interface.direct).unwrap_or(true);
                            direct.tick(
                                std::time::Instant::now(),
                                enabled,
                                received.as_ref(),
                                &stats,
                                backend,
                            );
                            direct_routes = direct.routes();
                            // サブネットルーター(ADR-0014): 他メンバーの広告は
                            // ホスト経由の経路として反映し、自分の広告は
                            // ルーター役(転送 + SNAT)として反映する
                            if let Some(received) = &received {
                                let my_key = host_public_key.as_bytes();
                                let mut extra: Vec<ipnet::Ipv4Net> = Vec::new();
                                let mut mine: Vec<ipnet::Ipv4Net> = Vec::new();
                                for entry in &received.distribution.members {
                                    if entry.public_key.as_bytes() == my_key {
                                        mine.extend(entry.subnets.iter().copied());
                                    } else {
                                        extra.extend(entry.subnets.iter().copied());
                                    }
                                }
                                extra.sort_unstable();
                                extra.dedup();
                                mine.sort_unstable();
                                mine.dedup();
                                if extra != member_extra {
                                    if let Some(config) = &config {
                                        match rebind_host_peer(config, &extra, backend) {
                                            Ok(()) => {
                                                tracing::info!(
                                                    "ホスト経由のサブネット経路を更新しました: {extra:?}"
                                                );
                                                member_extra = extra.clone();
                                            }
                                            Err(e) => tracing::warn!(
                                                "サブネットの AllowedIPs 反映に失敗しました: {e:#}"
                                            ),
                                        }
                                    }
                                }
                                let desired = extra.iter().copied().collect();
                                sync_routes(backend, &desired, &mut routes);
                                match backend.sync_subnet_router(spec.address.trunc(), &mine, true)
                                {
                                    Ok(()) => router_error_logged = false,
                                    Err(e) => {
                                        if !router_error_logged {
                                            tracing::warn!("ルーター役の設定に失敗しました: {e:#}");
                                            router_error_logged = true;
                                        }
                                    }
                                }
                            }
                            received.map(|r| r.distribution)
                        }
                    };
                    let ledger = distribution.as_ref().map(|dist| dist.members.clone());
                    // メッセージングのピア表(自分以外の 仮想 IP → 表示名)を台帳から更新
                    if let Some(ledger) = &ledger {
                        let map: HashMap<std::net::Ipv4Addr, String> = ledger
                            .iter()
                            .filter(|e| e.ip != spec.address.addr())
                            .map(|e| {
                                (e.ip, e.name.clone().unwrap_or_else(|| e.ip.to_string()))
                            })
                            .collect();
                        *msg_peers.lock().unwrap() = map;
                        // グループの送達同期(ADR-0016、M3-13c)。ack が取れる
                        // まで 30 秒間隔で送り直す。「オンライン」判定には最終
                        // ハンドシェイクの猶予(180 秒)があり、短時間の
                        // オフライン→オンラインは遷移として見えないため、
                        // 遷移検知でなく ack ベースにしている(検証 FB)
                        let online: HashSet<std::net::Ipv4Addr> = ledger
                            .iter()
                            .filter(|e| e.online && e.ip != spec.address.addr())
                            .map(|e| e.ip)
                            .collect();
                        let pending = groups.lock().unwrap().pending_sync(
                            spec.address.addr(),
                            &online,
                            Duration::from_secs(30),
                        );
                        for (ip, group) in pending {
                            let groups = Arc::clone(&groups);
                            tokio::spawn(async move {
                                match crate::msg::send_group_update(ip, &group).await {
                                    Ok(()) => groups.lock().unwrap().mark_acked(
                                        &group.id,
                                        ip,
                                        group.revision,
                                    ),
                                    Err(e) => tracing::debug!(
                                        "{ip} へのグループ同期に失敗(30 秒後に再試行): {e:#}"
                                    ),
                                }
                            });
                        }
                    }
                    if let Err(e) =
                        status::write_status_file(&status_path, &stats, ledger.as_deref())
                    {
                        tracing::debug!("ステータスファイルの書き出しに失敗: {e:#}");
                    }
                    if let Some(snapshot) = &snapshot {
                        *snapshot.lock().unwrap() = Some(Snapshot {
                            peers: stats,
                            ledger,
                            dns_records: distribution
                                .map(|dist| dist.dns_records)
                                .unwrap_or_default(),
                            rtt_ms: rtt.lock().unwrap().clone(),
                            removed: removed.load(std::sync::atomic::Ordering::Relaxed),
                            direct: direct_routes,
                        });
                    }
                }
            }
        };
        for task in tasks {
            task.abort();
        }
        result
    }
}

/// 台帳を構築する(ホスト自身 + 全ピア)。online は最終ハンドシェイクで判定。
fn build_ledger(
    config: &Config,
    host_public_key: &peercove_core::keys::PublicKey,
    stats: &[PeerStats],
) -> Vec<LedgerEntry> {
    let by_key: HashMap<&[u8; 32], &PeerStats> =
        stats.iter().map(|s| (s.public_key.as_bytes(), s)).collect();
    let now = SystemTime::now();
    let mut ledger = vec![LedgerEntry {
        name: config
            .interface
            .display_name
            .clone()
            .or_else(|| Some("host".to_string())),
        ip: config.interface.address.addr(),
        public_key: *host_public_key,
        online: true,
        is_host: true,
        // ホスト自身のエンドポイントは載せない(メンバーは設定で持っている)
        endpoint: None,
        endpoint_age_secs: None,
        subnets: vec![],
    }];
    for peer in &config.peers {
        let stats = by_key.get(peer.public_key.as_bytes());
        let handshake_age = stats
            .and_then(|s| s.last_handshake)
            .and_then(|t| now.duration_since(t).ok());
        let online = handshake_age.is_some_and(|age| age <= ONLINE_THRESHOLD);
        // 直接通信(ADR-0013)用の外部エンドポイント。オンライン(= 新鮮)な
        // ときだけ載せる。endpoint は認証済みパケットのたびに最新化されるため、
        // 観測経過はハンドシェイク経過で近似する(実際より古く見積もる側に安全)
        let endpoint = if online {
            stats.and_then(|s| s.endpoint)
        } else {
            None
        };
        ledger.push(LedgerEntry {
            name: peer.name.clone(),
            ip: peer
                .allowed_ips
                .first()
                .map(|net| net.addr())
                .unwrap_or(std::net::Ipv4Addr::UNSPECIFIED),
            public_key: peer.public_key,
            online,
            is_host: false,
            endpoint,
            endpoint_age_secs: endpoint
                .and(handshake_age)
                .map(|age| age.as_secs() / ENDPOINT_AGE_STEP_SECS * ENDPOINT_AGE_STEP_SECS),
            subnets: peer.subnets.clone(),
        });
    }
    ledger
}

/// ピア設定の変更検知用フィンガープリント(M1-7)。
#[derive(Clone, PartialEq, Eq)]
struct PeerFingerprint {
    ip: std::net::Ipv4Addr,
    endpoint: Option<std::net::SocketAddr>,
    allowed_ips: Vec<ipnet::Ipv4Net>,
    keepalive: Option<u16>,
    psk: Option<[u8; 32]>,
}

impl PeerFingerprint {
    fn of(spec: &PeerSpec) -> Self {
        Self {
            ip: spec
                .allowed_ips
                .first()
                .map(|net| net.addr())
                .unwrap_or(std::net::Ipv4Addr::UNSPECIFIED),
            endpoint: spec.endpoint,
            allowed_ips: spec.allowed_ips.clone(),
            keepalive: spec.persistent_keepalive,
            psk: spec.preshared_key.as_ref().map(|k| *k.as_bytes()),
        }
    }
}

/// 設定とバックエンドのピアを同期する(ADR-0002 / M1-G3 / M1-7)。
/// - 設定に増えたピア: バックエンドへ追加
/// - 設定が変わったピア(endpoint / allowed_ips / keepalive / PSK): 削除→再追加で反映
///   (再ハンドシェイクが走るため数秒の断がある)
/// - 設定から消えたピア: まず削除通知を送り(1 周期目)、次の周期で実削除する
///   (通知はトンネル経由なので、先にピアを消すと届かないため)
fn sync_peers(
    config: &Config,
    backend: &mut dyn WgBackend,
    known: &mut HashMap<[u8; 32], PeerFingerprint>,
    pending_removal: &mut HashSet<[u8; 32]>,
    connections: &control::Connections,
) {
    // 追加・変更
    for peer in &config.peers {
        let key = *peer.public_key.as_bytes();
        let spec = match build_peer_spec(peer, Role::Host) {
            Ok(spec) => spec,
            Err(e) => {
                tracing::warn!("ピア {} の設定が不正です: {e:#}", peer.public_key);
                continue;
            }
        };
        let fingerprint = PeerFingerprint::of(&spec);
        match known.get(&key) {
            None => match backend.add_peer(&spec) {
                Ok(()) => {
                    known.insert(key, fingerprint);
                    tracing::info!("ピア {} を追加しました", peer.public_key);
                }
                Err(e) => {
                    tracing::warn!("ピア {} の追加に失敗しました: {e:#}", peer.public_key)
                }
            },
            Some(current) if *current != fingerprint => {
                // 変更 = 削除して再追加(両 OS 共通の確実な反映方法)
                let result = backend
                    .remove_peer(&peer.public_key)
                    .and_then(|()| backend.add_peer(&spec));
                match result {
                    Ok(()) => {
                        known.insert(key, fingerprint);
                        tracing::info!("ピア {} の設定変更を反映しました", peer.public_key);
                    }
                    Err(e) => tracing::warn!(
                        "ピア {} の設定変更の反映に失敗しました: {e:#}",
                        peer.public_key
                    ),
                }
            }
            Some(_) => {}
        }
    }

    // 削除(2 段階)
    let config_keys: HashSet<[u8; 32]> = config
        .peers
        .iter()
        .map(|p| *p.public_key.as_bytes())
        .collect();
    let removed: Vec<([u8; 32], std::net::Ipv4Addr)> = known
        .iter()
        .filter(|(key, _)| !config_keys.contains(*key))
        .map(|(key, fingerprint)| (*key, fingerprint.ip))
        .collect();
    for (key, ip) in removed {
        let public_key = peercove_core::keys::PublicKey::from_bytes(key);
        if pending_removal.insert(key) {
            // 1 周期目: 本人へ削除通知(接続していなければ何もしない)
            if let Some(tx) = connections.lock().unwrap().get(&ip) {
                let _ = tx.send(peercove_core::proto::ControlMessage::Removed {
                    message: "ホストによってこのネットワークから削除されました".to_string(),
                });
                tracing::info!("ピア {public_key}({ip})へ削除通知を送りました");
            }
            continue;
        }
        // 2 周期目: バックエンドから実削除
        match backend.remove_peer(&public_key) {
            Ok(()) => {
                known.remove(&key);
                pending_removal.remove(&key);
                tracing::info!("ピア {public_key}({ip})を削除しました");
            }
            Err(e) => tracing::warn!("ピア {public_key} の削除に失敗しました: {e:#}"),
        }
    }
}

/// down コマンド: 残骸(TUN 等)のクリーンアップ。
pub fn run_down(config_path: &Path) -> anyhow::Result<()> {
    let config = Config::load(config_path)?;
    let mut backend = create_backend(&config.interface.name)?;
    backend.down()?;
    println!("クリーンアップが完了しました({})", config.interface.name);
    Ok(())
}

pub fn build_spec(config: &Config, role: Role) -> anyhow::Result<TunnelSpec> {
    let private_key = read_private_key_file(&config.interface.private_key_file)
        .context("秘密鍵ファイルの読み込みに失敗しました(peercove-poc keygen で生成できます)")?;

    let listen_port = match role {
        Role::Host => Some(config.interface.listen_port.unwrap_or(DEFAULT_LISTEN_PORT)),
        Role::Member => config.interface.listen_port,
    };

    if role == Role::Member {
        if config.peers.is_empty() {
            bail!("member 設定には [[peer]](ホスト)が 1 つ必要です");
        }
        if config.peers.len() > 1 {
            bail!(
                "M0 の member はホブ&スポーク構成のため [[peer]] はホスト 1 つだけにしてください"
            );
        }
        if config.peers[0].endpoint.is_none() {
            bail!("member の peer には endpoint(ホストの IP:ポート)が必要です");
        }
    }

    let peers = config
        .peers
        .iter()
        .map(|peer| build_peer_spec(peer, role))
        .collect::<anyhow::Result<Vec<_>>>()?;

    // 広告サブネット(peer.subnets)は意図的にサブネット外なので対象にしない
    for peer in &config.peers {
        for net in &peer.allowed_ips {
            if !config.interface.address.trunc().contains(net) {
                tracing::warn!(
                    "allowed_ips {net} は interface.address のサブネット外です(M0 では未検証の構成)"
                );
            }
        }
    }

    Ok(TunnelSpec {
        private_key,
        address: config.interface.address,
        listen_port,
        mtu: config.interface.mtu,
        forwarding: role == Role::Host,
        peers,
    })
}

fn build_peer_spec(peer: &PeerConfig, role: Role) -> anyhow::Result<PeerSpec> {
    let preshared_key = peer
        .preshared_key_file
        .as_deref()
        .map(read_preshared_key_file)
        .transpose()
        .context("preshared_key_file の読み込みに失敗しました")?;
    let persistent_keepalive = match (role, peer.persistent_keepalive) {
        // NAT 越え維持のため、メンバー→ホストは keepalive 必須(未指定なら 25 秒)
        (Role::Member, None) => {
            tracing::info!("persistent_keepalive 未指定のため 25 秒を使用します");
            Some(25)
        }
        (_, value) => value,
    };
    // 広告サブネット(ADR-0014)は AllowedIPs に足して cryptokey routing に
    // 載せる(ホスト側のみ。member のホストピアは台帳受信時にランタイムで足す)
    let mut allowed_ips = peer.allowed_ips.clone();
    if role == Role::Host {
        allowed_ips.extend(peer.subnets.iter().copied());
    }
    Ok(PeerSpec {
        public_key: peer.public_key,
        endpoint: peer.endpoint,
        allowed_ips,
        persistent_keepalive,
        preshared_key,
    })
}

/// member のホストピアへ広告サブネット分の AllowedIPs を足して再登録する
/// (ADR-0014)。削除→再追加のため再ハンドシェイクの数秒間は断がある
/// (コントロールチャネルの TCP は再送で生き残る)。
fn rebind_host_peer(
    config: &Config,
    extra: &[ipnet::Ipv4Net],
    backend: &mut dyn WgBackend,
) -> anyhow::Result<()> {
    let peer = config
        .peers
        .first()
        .context("member 設定に [[peer]] がありません")?;
    let mut spec = build_peer_spec(peer, Role::Member)?;
    spec.allowed_ips.extend(extra.iter().copied());
    backend.remove_peer(&peer.public_key)?;
    backend.add_peer(&spec)
}

/// OS ルートを望ましい集合へ同期する(ADR-0014)。失敗した分は集合に
/// 反映せず、次の周期で再試行される。
fn sync_routes(
    backend: &mut dyn WgBackend,
    desired: &std::collections::BTreeSet<ipnet::Ipv4Net>,
    current: &mut std::collections::BTreeSet<ipnet::Ipv4Net>,
) {
    for subnet in desired.difference(current).copied().collect::<Vec<_>>() {
        match backend.add_route(subnet) {
            Ok(()) => {
                current.insert(subnet);
                tracing::info!("サブネット {subnet} への経路を追加しました");
            }
            Err(e) => tracing::warn!("経路 {subnet} の追加に失敗しました: {e:#}"),
        }
    }
    for subnet in current.difference(desired).copied().collect::<Vec<_>>() {
        match backend.remove_route(subnet) {
            Ok(()) => {
                current.remove(&subnet);
                tracing::info!("サブネット {subnet} への経路を削除しました");
            }
            Err(e) => tracing::warn!("経路 {subnet} の削除に失敗しました: {e:#}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::mock::MockBackend;
    use peercove_core::keys::PrivateKey;

    /// 広告サブネット(ADR-0014)はホスト側の AllowedIPs にだけ足される
    /// (member のホストピアは台帳受信時にランタイムで足す)。
    #[test]
    fn host_peer_spec_includes_subnets() {
        let key = PrivateKey::generate().public_key().to_base64();
        let config = host_config(&format!(
            "[[peer]]\nname = \"alice\"\npublic_key = \"{key}\"\nallowed_ips = [\"10.100.42.2/32\"]\nsubnets = [\"192.168.10.0/24\"]\n"
        ));
        let subnet: ipnet::Ipv4Net = "192.168.10.0/24".parse().unwrap();
        let host = build_peer_spec(&config.peers[0], Role::Host).unwrap();
        assert!(host.allowed_ips.contains(&subnet));
        let member = build_peer_spec(&config.peers[0], Role::Member).unwrap();
        assert!(!member.allowed_ips.contains(&subnet));
    }

    fn host_config(peers_toml: &str) -> Config {
        let text = format!(
            "[interface]\nprivate_key_file = \"host.key\"\naddress = \"10.100.42.1/24\"\nlisten_port = 51820\n{peers_toml}"
        );
        toml::from_str(&text).unwrap()
    }

    #[test]
    fn plan_interface_checks_subnet_overlap_and_allocates_names() {
        let config = host_config("");
        // 制約なし: 既定名のまま
        assert_eq!(
            plan_interface(&config, &StartLimits::default()).unwrap(),
            "peercove0"
        );

        // 既定名が使用中: 空き番号へ
        let limits = StartLimits {
            used_subnets: vec![("10.200.0.0/24".parse().unwrap(), "other".to_string())],
            used_if_names: vec!["peercove0".to_string(), "peercove1".to_string()],
        };
        assert_eq!(plan_interface(&config, &limits).unwrap(), "peercove2");

        // サブネット重複: ネットワーク名入りのエラー
        let limits = StartLimits {
            used_subnets: vec![("10.100.42.0/24".parse().unwrap(), "game".to_string())],
            used_if_names: vec![],
        };
        let err = plan_interface(&config, &limits).unwrap_err().to_string();
        assert!(
            err.contains("game"),
            "重複相手のネットワーク名を含む: {err}"
        );

        // 明示したインターフェース名が使用中: エラー(黙って別名にしない)
        let mut config = host_config("");
        config.interface.name = "mytun".to_string();
        let limits = StartLimits {
            used_subnets: vec![],
            used_if_names: vec!["mytun".to_string()],
        };
        assert!(plan_interface(&config, &limits).is_err());
    }

    /// 台帳のエンドポイント(ADR-0013、M3-2): オンラインのピアだけ
    /// 外部エンドポイント + 観測経過秒が載り、オフラインでは載らない。
    #[test]
    fn build_ledger_publishes_endpoints_only_when_fresh() {
        let online_key = PrivateKey::generate().public_key();
        let stale_key = PrivateKey::generate().public_key();
        let config = host_config(&format!(
            "[[peer]]\nname = \"alice\"\npublic_key = \"{online_key}\"\nallowed_ips = [\"10.100.42.2/32\"]\n\
             [[peer]]\nname = \"bob\"\npublic_key = \"{stale_key}\"\nallowed_ips = [\"10.100.42.3/32\"]\n"
        ));
        let stats_for = |key: &peercove_core::keys::PublicKey, age: Duration| PeerStats {
            public_key: *key,
            endpoint: Some("203.0.113.9:12345".parse().unwrap()),
            last_handshake: Some(SystemTime::now() - age),
            tx_bytes: 0,
            rx_bytes: 0,
            allowed_ips: vec![],
        };
        let host_key = PrivateKey::generate().public_key();
        let ledger = build_ledger(
            &config,
            &host_key,
            &[
                stats_for(&online_key, Duration::from_secs(90)),
                stats_for(&stale_key, ONLINE_THRESHOLD + Duration::from_secs(60)),
            ],
        );

        assert!(ledger[0].is_host);
        assert_eq!(ledger[0].endpoint, None, "ホスト自身には載せない");

        let alice = &ledger[1];
        assert!(alice.online);
        assert_eq!(alice.endpoint, Some("203.0.113.9:12345".parse().unwrap()));
        assert_eq!(
            alice.endpoint_age_secs,
            Some(60),
            "ハンドシェイク経過(90 秒)を 60 秒粒度へ量子化(再配布の抑制)"
        );

        let bob = &ledger[2];
        assert!(!bob.online);
        assert_eq!(bob.endpoint, None, "古い観測は配布しない");
        assert_eq!(bob.endpoint_age_secs, None);
    }

    #[test]
    fn sync_peers_adds_updates_and_removes() {
        let member_key = PrivateKey::generate().public_key();
        let peer_toml = |endpoint: &str| {
            format!(
                "[[peer]]\nname = \"alice\"\npublic_key = \"{member_key}\"\nendpoint = \"{endpoint}\"\nallowed_ips = [\"10.100.42.2/32\"]\n"
            )
        };
        let mut backend = MockBackend::default();
        let mut known = HashMap::new();
        let mut pending = HashSet::new();
        let connections: control::Connections = Default::default();

        // 1. 追加
        let config = host_config(&peer_toml("192.168.0.12:51820"));
        sync_peers(
            &config,
            &mut backend,
            &mut known,
            &mut pending,
            &connections,
        );
        assert_eq!(backend.ops, vec![format!("add:{member_key}")]);
        assert_eq!(known.len(), 1);

        // 2. 変更なし → 何もしない
        backend.ops.clear();
        sync_peers(
            &config,
            &mut backend,
            &mut known,
            &mut pending,
            &connections,
        );
        assert!(backend.ops.is_empty());

        // 3. endpoint 変更 → remove + add
        let config = host_config(&peer_toml("203.0.113.9:51820"));
        sync_peers(
            &config,
            &mut backend,
            &mut known,
            &mut pending,
            &connections,
        );
        assert_eq!(
            backend.ops,
            vec![format!("remove:{member_key}"), format!("add:{member_key}")]
        );

        // 4. 設定から削除 → 1 周期目は通知のみ(バックエンド操作なし)
        backend.ops.clear();
        let config = host_config("");
        sync_peers(
            &config,
            &mut backend,
            &mut known,
            &mut pending,
            &connections,
        );
        assert!(backend.ops.is_empty(), "1 周期目は実削除しない");
        assert!(pending.contains(member_key.as_bytes()));

        // 5. 2 周期目に実削除
        sync_peers(
            &config,
            &mut backend,
            &mut known,
            &mut pending,
            &connections,
        );
        assert_eq!(backend.ops, vec![format!("remove:{member_key}")]);
        assert!(known.is_empty());
        assert!(pending.is_empty());
    }
}
