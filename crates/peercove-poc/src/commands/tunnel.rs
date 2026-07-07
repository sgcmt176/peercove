use std::path::Path;

use anyhow::{bail, Context};
use peercove_core::config::{Config, PeerConfig, DEFAULT_LISTEN_PORT};
use peercove_core::keys::{read_preshared_key_file, read_private_key_file};

use crate::backend::{create_backend, PeerSpec, TunnelSpec};

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

    wait_for_ctrl_c()?;
    println!("終了処理中…");
    backend.down()?;
    println!("クリーンアップが完了しました");
    Ok(())
}

/// down コマンド: 残骸(TUN 等)のクリーンアップ。
pub fn run_down(config_path: &Path) -> anyhow::Result<()> {
    let config = Config::load(config_path)?;
    let mut backend = create_backend(&config.interface.name)?;
    backend.down()?;
    println!("クリーンアップが完了しました({})", config.interface.name);
    Ok(())
}

fn wait_for_ctrl_c() -> anyhow::Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("非同期ランタイムの初期化に失敗しました")?
        .block_on(tokio::signal::ctrl_c())
        .context("シグナル待機に失敗しました")
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
