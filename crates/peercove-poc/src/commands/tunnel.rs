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
    upnp_lease: Option<crate::upnp::UpnpLease>,
}

/// 設定を読み、UPnP(任意)→ トンネル作成までを行う(CLI / daemon 共通)。
pub fn bring_up(config_path: &Path, role: Role, upnp: bool) -> anyhow::Result<ActiveTunnel> {
    let config = Config::load(config_path)?;
    if peercove_core::ipalloc::overlaps_cgnat(config.interface.address.trunc()) {
        tracing::warn!(
            "トンネルのサブネット {} は CGNAT レンジ(100.64.0.0/10)内です。\
             Tailscale が動作しているマシンではパケットが破棄されます。\
             `peercove-poc init` で生成した設定への移行を推奨します(ADR-0006)",
            config.interface.address.trunc()
        );
    }
    let spec = build_spec(&config, role)?;
    let mut backend = create_backend(&config.interface.name)?;

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
        "トンネル {} を作成しました(address={} mtu={} peers={})",
        config.interface.name,
        config.interface.address,
        spec.mtu,
        spec.peers.len()
    );
    Ok(ActiveTunnel {
        backend,
        spec,
        role,
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
    ) -> anyhow::Result<()> {
        supervise(
            config_path,
            self.role,
            self.backend.as_mut(),
            &self.spec,
            stop,
            snapshot,
        )
        .await
    }

    #[cfg(test)]
    pub(crate) fn new_for_test(spec: TunnelSpec, role: Role, backend: Box<dyn WgBackend>) -> Self {
        Self {
            backend,
            spec,
            role,
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
    let mut tunnel = bring_up(config_path, role, upnp)?;
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
    /// 相手の仮想 IP → コントロールチャネルで測った RTT(ミリ秒、M2-G5)。
    pub rtt_ms: HashMap<std::net::Ipv4Addr, f64>,
}

pub type SharedSnapshot = Arc<Mutex<Option<Snapshot>>>;

/// 停止シグナルまで 5 秒周期で以下を行う(CLI = Ctrl+C / daemon = stop 要求):
/// - (host のみ)設定を再読込し、ピアの追加・変更・削除を同期(ADR-0002 / M1-G3/7)
/// - (host)台帳を更新してコントロールチャネルへ配布 /(member)受信台帳の反映
/// - ステータスファイルと共有スナップショットの書き出し
pub async fn supervise(
    config_path: &Path,
    role: Role,
    backend: &mut dyn WgBackend,
    spec: &TunnelSpec,
    mut stop: tokio::sync::watch::Receiver<bool>,
    snapshot: Option<SharedSnapshot>,
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
        let (ledger_tx, ledger_rx) = tokio::sync::watch::channel(Vec::<LedgerEntry>::new());
        let connections: control::Connections = Default::default();
        let member_ledger: Arc<Mutex<Option<Vec<LedgerEntry>>>> = Default::default();
        let rtt: control::RttMap = Default::default();
        let mut tasks = Vec::new();
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
                    if role == Role::Host {
                        if let Some(config) = &config {
                            sync_peers(
                                config,
                                backend,
                                &mut known,
                                &mut pending_removal,
                                &connections,
                            );
                        }
                    }
                    let stats = match backend.stats() {
                        Ok(stats) => stats,
                        Err(e) => {
                            tracing::debug!("統計の取得に失敗: {e:#}");
                            continue;
                        }
                    };
                    // 台帳: host は設定+統計から構築して配布、member は受信済みを表示
                    let ledger = match role {
                        Role::Host => config.as_ref().map(|config| {
                            let ledger = build_ledger(config, &host_public_key, &stats);
                            ledger_tx.send_if_modified(|current| {
                                if *current != ledger {
                                    *current = ledger.clone();
                                    true
                                } else {
                                    false
                                }
                            });
                            ledger
                        }),
                        Role::Member => member_ledger.lock().unwrap().clone(),
                    };
                    if let Err(e) =
                        status::write_status_file(&status_path, &stats, ledger.as_deref())
                    {
                        tracing::debug!("ステータスファイルの書き出しに失敗: {e:#}");
                    }
                    if let Some(snapshot) = &snapshot {
                        *snapshot.lock().unwrap() = Some(Snapshot {
                            peers: stats,
                            ledger,
                            rtt_ms: rtt.lock().unwrap().clone(),
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
    }];
    for peer in &config.peers {
        let online = by_key
            .get(peer.public_key.as_bytes())
            .and_then(|s| s.last_handshake)
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age <= ONLINE_THRESHOLD);
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

    for peer in &peers {
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
    Ok(PeerSpec {
        public_key: peer.public_key,
        endpoint: peer.endpoint,
        allowed_ips: peer.allowed_ips.clone(),
        persistent_keepalive,
        preshared_key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::mock::MockBackend;
    use peercove_core::keys::PrivateKey;

    fn host_config(peers_toml: &str) -> Config {
        let text = format!(
            "[interface]\nprivate_key_file = \"host.key\"\naddress = \"10.100.42.1/24\"\nlisten_port = 51820\n{peers_toml}"
        );
        toml::from_str(&text).unwrap()
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
