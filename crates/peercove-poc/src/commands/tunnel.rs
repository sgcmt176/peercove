use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context};
use peercove_core::config::{Config, PeerConfig, DEFAULT_LISTEN_PORT};
use peercove_core::keys::{read_preshared_key_file, read_private_key_file};

use crate::backend::{create_backend, PeerSpec, TunnelSpec, WgBackend};
use crate::commands::status;

/// 設定再読込とステータスファイル書き出しの周期(ADR-0002)。
const SUPERVISE_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Host,
    Member,
}

/// host / member 共通: トンネルを作成し、Ctrl+C まで維持して破棄する。
pub fn run_up(config_path: &Path, role: Role) -> anyhow::Result<()> {
    let config = Config::load(config_path)?;
    let spec = build_spec(&config, role)?;
    let mut backend = create_backend(&config.interface.name)?;

    backend.up(&spec)?;
    println!(
        "トンネル {} を作成しました(address={} mtu={} peers={})",
        config.interface.name,
        config.interface.address,
        spec.mtu,
        spec.peers.len()
    );
    if role == Role::Host {
        println!(
            "待受ポート: UDP {}(メンバーの endpoint にはこのポートを指定)",
            spec.listen_port.unwrap_or(DEFAULT_LISTEN_PORT)
        );
    }
    println!("Ctrl+C で終了します(トンネルをクリーンアップします)");

    let supervise_result = supervise_until_ctrl_c(config_path, role, backend.as_mut(), &spec);
    println!("終了処理中…");
    let _ = std::fs::remove_file(status::status_file_path(config_path));
    backend.down()?;
    println!("クリーンアップが完了しました");
    supervise_result
}

/// Ctrl+C まで 5 秒周期で以下を行う:
/// - (host のみ)設定を再読込し、追記された新規ピアを動的追加(ADR-0002)
/// - ステータスファイルの書き出し
fn supervise_until_ctrl_c(
    config_path: &Path,
    role: Role,
    backend: &mut dyn WgBackend,
    spec: &TunnelSpec,
) -> anyhow::Result<()> {
    let mut known: HashSet<[u8; 32]> = spec
        .peers
        .iter()
        .map(|p| *p.public_key.as_bytes())
        .collect();
    let status_path = status::status_file_path(config_path);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("非同期ランタイムの初期化に失敗しました")?;
    runtime.block_on(async {
        let ctrl_c = tokio::signal::ctrl_c();
        tokio::pin!(ctrl_c);
        let mut tick = tokio::time::interval(SUPERVISE_INTERVAL);
        loop {
            tokio::select! {
                result = &mut ctrl_c => {
                    return result.context("シグナル待機に失敗しました");
                }
                _ = tick.tick() => {
                    if role == Role::Host {
                        reload_new_peers(config_path, backend, &mut known);
                    }
                    match backend.stats() {
                        Ok(stats) => {
                            if let Err(e) = status::write_status_file(&status_path, &stats) {
                                tracing::debug!("ステータスファイルの書き出しに失敗: {e:#}");
                            }
                        }
                        Err(e) => tracing::debug!("統計の取得に失敗: {e:#}"),
                    }
                }
            }
        }
    })
}

/// 設定ファイルを再読込し、未登録の公開鍵のピアだけをバックエンドへ追加する。
/// 既存ピアの変更・削除は M1 で対応(ここでは無視)。
fn reload_new_peers(
    config_path: &Path,
    backend: &mut dyn WgBackend,
    known: &mut HashSet<[u8; 32]>,
) {
    let config = match Config::load(config_path) {
        Ok(config) => config,
        Err(e) => {
            tracing::warn!("設定の再読込に失敗しました(前回の設定で継続): {e:#}");
            return;
        }
    };
    for peer in &config.peers {
        if known.contains(peer.public_key.as_bytes()) {
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
                known.insert(*peer.public_key.as_bytes());
                tracing::info!("ピア {} を追加しました", peer.public_key);
            }
            Err(e) => tracing::warn!("ピア {} の追加に失敗しました: {e:#}", peer.public_key),
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
