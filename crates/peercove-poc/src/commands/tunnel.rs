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

/// host / member 共通: トンネルを作成し、Ctrl+C まで維持して破棄する。
pub fn run_up(config_path: &Path, role: Role, upnp: bool) -> anyhow::Result<()> {
    let config = Config::load(config_path)?;
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
    println!(
        "トンネル {} を作成しました(address={} mtu={} peers={})",
        config.interface.name,
        config.interface.address,
        spec.mtu,
        spec.peers.len()
    );
    if role == Role::Host {
        println!("待受ポート: UDP {listen_port}(メンバーの endpoint にはこのポートを指定)");
    }
    println!("Ctrl+C で終了します(トンネルをクリーンアップします)");

    let supervise_result = supervise_until_ctrl_c(config_path, role, backend.as_mut(), &spec);
    println!("終了処理中…");
    if let Some(lease) = upnp_lease {
        lease.release();
    }
    let _ = std::fs::remove_file(status::status_file_path(config_path));
    backend.down()?;
    println!("クリーンアップが完了しました");
    supervise_result
}

/// Ctrl+C まで 5 秒周期で以下を行う:
/// - (host のみ)設定を再読込し、追記された新規ピアを動的追加(ADR-0002)
/// - (host)台帳を更新してコントロールチャネルへ配布 /(member)受信台帳の反映
/// - ステータスファイルの書き出し
fn supervise_until_ctrl_c(
    config_path: &Path,
    role: Role,
    backend: &mut dyn WgBackend,
    spec: &TunnelSpec,
) -> anyhow::Result<()> {
    // 登録済みピア(公開鍵 → 仮想 IP)。削除通知の宛先解決にも使う
    let mut known: HashMap<[u8; 32], std::net::Ipv4Addr> = spec
        .peers
        .iter()
        .map(|p| {
            (
                *p.public_key.as_bytes(),
                p.allowed_ips
                    .first()
                    .map(|net| net.addr())
                    .unwrap_or(std::net::Ipv4Addr::UNSPECIFIED),
            )
        })
        .collect();
    // 削除通知済み・次の周期で実削除するピア
    let mut pending_removal: HashSet<[u8; 32]> = HashSet::new();
    let status_path = status::status_file_path(config_path);
    let host_public_key = spec.private_key.public_key();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("非同期ランタイムの初期化に失敗しました")?;
    runtime.block_on(async {
        // コントロールチャネル(M1-G2)
        let (ledger_tx, ledger_rx) = tokio::sync::watch::channel(Vec::<LedgerEntry>::new());
        let connections: control::Connections = Default::default();
        let member_ledger: Arc<Mutex<Option<Vec<LedgerEntry>>>> = Default::default();
        let mut tasks = Vec::new();
        match role {
            Role::Host => {
                tasks.push(tokio::spawn(control::run_host_server(
                    spec.address.addr(),
                    ledger_rx,
                    Arc::clone(&connections),
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
                        )));
                    }
                    _ => tracing::warn!(
                        "コントロールチャネルの接続先が決められないため台帳同期を行いません"
                    ),
                }
            }
        }

        let ctrl_c = tokio::signal::ctrl_c();
        tokio::pin!(ctrl_c);
        let mut tick = tokio::time::interval(SUPERVISE_INTERVAL);
        let result = loop {
            tokio::select! {
                result = &mut ctrl_c => {
                    break result.context("シグナル待機に失敗しました");
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
                }
            }
        };
        for task in tasks {
            task.abort();
        }
        result
    })
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

/// 設定とバックエンドのピアを同期する(ADR-0002 / M1-G3)。
/// - 設定に増えたピア: バックエンドへ追加
/// - 設定から消えたピア: まず削除通知を送り(1 周期目)、次の周期で実削除する
///   (通知はトンネル経由なので、先にピアを消すと届かないため)
fn sync_peers(
    config: &Config,
    backend: &mut dyn WgBackend,
    known: &mut HashMap<[u8; 32], std::net::Ipv4Addr>,
    pending_removal: &mut HashSet<[u8; 32]>,
    connections: &control::Connections,
) {
    // 追加
    for peer in &config.peers {
        if known.contains_key(peer.public_key.as_bytes()) {
            continue;
        }
        let spec = match build_peer_spec(peer, Role::Host) {
            Ok(spec) => spec,
            Err(e) => {
                tracing::warn!("ピア {} の設定が不正です: {e:#}", peer.public_key);
                continue;
            }
        };
        match backend.add_peer(&spec) {
            Ok(()) => {
                known.insert(
                    *peer.public_key.as_bytes(),
                    peer.allowed_ips
                        .first()
                        .map(|net| net.addr())
                        .unwrap_or(std::net::Ipv4Addr::UNSPECIFIED),
                );
                tracing::info!("ピア {} を追加しました", peer.public_key);
            }
            Err(e) => tracing::warn!("ピア {} の追加に失敗しました: {e:#}", peer.public_key),
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
        .map(|(key, ip)| (*key, *ip))
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
