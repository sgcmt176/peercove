//! Linux バックエンド: カーネル WireGuard を netlink(defguard_wireguard_rs)で制御する。
//!
//! ルーティングは interface.address のプレフィックス(例 /24)の connected route に
//! 任せるため、`configure_peer_routing` は呼ばない(M0 の allowed_ips はすべて
//! そのサブネット内に収まる前提。G-3 のハブ&スポークもこれで成立する)。

use anyhow::{bail, Context};
use defguard_wireguard_rs::error::WireguardInterfaceError;
use defguard_wireguard_rs::key::Key;
use defguard_wireguard_rs::net::IpAddrMask;
use defguard_wireguard_rs::peer::Peer;
use defguard_wireguard_rs::{InterfaceConfiguration, Kernel, WGApi, WireguardInterfaceApi};

use super::{PeerSpec, PeerStats, TunnelSpec, WgBackend};

pub struct LinuxBackend {
    if_name: String,
    api: WGApi<Kernel>,
}

impl LinuxBackend {
    pub fn new(if_name: &str) -> anyhow::Result<Self> {
        let api = WGApi::<Kernel>::new(if_name.to_string())
            .with_context(|| format!("WG API の初期化に失敗しました({if_name})"))?;
        Ok(Self {
            if_name: if_name.to_string(),
            api,
        })
    }

    fn ensure_root() -> anyhow::Result<()> {
        // SAFETY: geteuid は引数なし・常に成功する POSIX API。OS 境界のため unsafe。
        let euid = unsafe { libc::geteuid() };
        if euid != 0 {
            bail!("root 権限が必要です。sudo を付けて再実行してください");
        }
        Ok(())
    }
}

fn to_peer(spec: &PeerSpec) -> Peer {
    let mut peer = Peer::new(Key::new(*spec.public_key.as_bytes()));
    peer.endpoint = spec.endpoint;
    peer.persistent_keepalive_interval = spec.persistent_keepalive;
    peer.preshared_key = spec
        .preshared_key
        .as_ref()
        .map(|psk| Key::new(*psk.as_bytes()));
    peer.allowed_ips = spec
        .allowed_ips
        .iter()
        .map(|net| IpAddrMask::new(net.addr().into(), net.prefix_len()))
        .collect();
    peer
}

impl WgBackend for LinuxBackend {
    fn up(&mut self, spec: &TunnelSpec) -> anyhow::Result<()> {
        Self::ensure_root()?;
        self.api.create_interface().map_err(|e| {
            anyhow::anyhow!(e).context(format!(
                "インターフェース {} の作成に失敗しました。既に存在する場合は \
                 `peercove-poc down` で残骸を掃除してください。カーネル WireGuard \
                 モジュールが無い場合は `sudo modprobe wireguard` を試してください",
                self.if_name
            ))
        })?;

        let config = InterfaceConfiguration {
            name: self.if_name.clone(),
            prvkey: spec.private_key.to_base64(),
            addresses: vec![IpAddrMask::new(
                spec.address.addr().into(),
                spec.address.prefix_len(),
            )],
            port: spec.listen_port.unwrap_or(0),
            peers: spec.peers.iter().map(to_peer).collect(),
            mtu: Some(u32::from(spec.mtu)),
            fwmark: None,
        };
        if let Err(e) = self.api.configure_interface(&config) {
            // 設定に失敗したら作りかけの TUN を残さない
            let _ = self.api.remove_interface();
            return Err(anyhow::anyhow!(e).context("インターフェースの設定に失敗しました"));
        }
        Ok(())
    }

    fn add_peer(&mut self, peer: &PeerSpec) -> anyhow::Result<()> {
        Self::ensure_root()?;
        self.api
            .configure_peer(&to_peer(peer))
            .with_context(|| format!("ピア {} の追加に失敗しました", peer.public_key))
    }

    fn stats(&mut self) -> anyhow::Result<Vec<PeerStats>> {
        let host = self
            .api
            .read_interface_data()
            .with_context(|| format!("{} の情報取得に失敗しました", self.if_name))?;
        let mut stats: Vec<PeerStats> = host
            .peers
            .values()
            .map(|peer| PeerStats {
                public_key: peercove_core::keys::PublicKey::from_bytes(peer.public_key.as_array()),
                endpoint: peer.endpoint,
                last_handshake: peer.last_handshake,
                tx_bytes: peer.tx_bytes,
                rx_bytes: peer.rx_bytes,
                allowed_ips: peer
                    .allowed_ips
                    .iter()
                    .filter_map(|mask| format!("{mask}").parse().ok())
                    .collect(),
            })
            .collect();
        stats.sort_by_key(|s| *s.public_key.as_bytes());
        Ok(stats)
    }

    fn down(&mut self) -> anyhow::Result<()> {
        Self::ensure_root()?;
        match self.api.remove_interface() {
            Ok(()) => Ok(()),
            // 存在しない場合は成功扱い(クリーンアップの冪等性)
            Err(WireguardInterfaceError::NetlinkError(msg)) if msg.contains("No such") => {
                tracing::info!("インターフェース {} は存在しません", self.if_name);
                Ok(())
            }
            Err(e) => Err(anyhow::anyhow!(e).context(format!(
                "インターフェース {} の削除に失敗しました",
                self.if_name
            ))),
        }
    }
}
