//! OS 依存の WG トンネル操作の抽象化層。
//!
//! 将来 `peercove-daemon-win` / `peercove-daemon-linux` へ分離する前提で、
//! TUN 作成・ピア設定・統計取得・破棄をこの trait の背後に隠す。
//! 実装の選定理由は docs/decisions.md の ADR-0001 を参照。

#[cfg(target_os = "linux")]
mod linux;
#[cfg(test)]
pub(crate) mod mock;
#[cfg(target_os = "windows")]
mod windows;

use std::net::SocketAddr;
use std::time::SystemTime;

use ipnet::Ipv4Net;
use peercove_core::keys::{PresharedKey, PrivateKey, PublicKey};

/// トンネル 1 本の設定。設定 TOML から組み立てる。
/// インターフェース名はバックエンド構築時([`create_backend`])に渡す。
pub struct TunnelSpec {
    pub private_key: PrivateKey,
    pub address: Ipv4Net,
    /// `None` は OS 任せ(メンバー)。ホストは必ず指定する。
    pub listen_port: Option<u16>,
    pub mtu: u16,
    /// ピア間転送(ハブ&スポーク)を有効にする(ホストのみ true)。
    /// 方式は OS ごとに異なる(ADR-0003)。
    pub forwarding: bool,
    pub peers: Vec<PeerSpec>,
}

#[derive(Clone)]
pub struct PeerSpec {
    pub public_key: PublicKey,
    pub endpoint: Option<SocketAddr>,
    pub allowed_ips: Vec<Ipv4Net>,
    pub persistent_keepalive: Option<u16>,
    pub preshared_key: Option<PresharedKey>,
}

#[derive(Clone)]
pub struct PeerStats {
    pub public_key: PublicKey,
    pub endpoint: Option<SocketAddr>,
    pub last_handshake: Option<SystemTime>,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub allowed_ips: Vec<Ipv4Net>,
}

pub trait WgBackend: Send {
    /// TUN 作成・IP/MTU 設定・ピア登録・待受開始までを行う。
    fn up(&mut self, spec: &TunnelSpec) -> anyhow::Result<()>;

    /// 実行中のトンネルへピアを追加する。
    fn add_peer(&mut self, peer: &PeerSpec) -> anyhow::Result<()>;

    /// 実行中のトンネルからピアを削除する(M1-G3)。
    /// 存在しないピアの削除は成功扱い(冪等)。
    fn remove_peer(&mut self, public_key: &PublicKey) -> anyhow::Result<()>;

    /// ピアごとの統計(最終ハンドシェイク・転送量)を返す。
    fn stats(&mut self) -> anyhow::Result<Vec<PeerStats>>;

    /// トンネルと関連設定を破棄する。`up` していないインスタンスで呼んだ場合は
    /// 残骸クリーンアップとして動作する(存在しなければ成功扱い)。
    fn down(&mut self) -> anyhow::Result<()>;

    /// 広告サブネット(ADR-0014、M3-7)宛の OS ルートをトンネル IF へ向ける。
    /// トンネル IF の消滅で OS が経路も消すため、down での明示削除は保険。
    fn add_route(&mut self, subnet: Ipv4Net) -> anyhow::Result<()>;

    /// [`WgBackend::add_route`] の対。存在しない経路の削除は成功扱い(冪等)。
    fn remove_route(&mut self, subnet: Ipv4Net) -> anyhow::Result<()>;

    /// ルーター役(自分の背後 LAN を広告している)としての転送設定を同期する。
    /// 冪等で、`subnets` が空なら全解除。`snat` は将来「LAN 側静的ルート前提」
    /// モードを設定で切るための入口(ADR-0014 追加条件。現状は常に true)。
    /// V1 は Linux のみ実装で、Windows は subnets が空でない場合に警告する。
    fn sync_subnet_router(
        &mut self,
        virtual_subnet: Ipv4Net,
        subnets: &[Ipv4Net],
        snat: bool,
    ) -> anyhow::Result<()>;
}

/// 現在の OS 用のバックエンドを返す。
pub fn create_backend(if_name: &str) -> anyhow::Result<Box<dyn WgBackend>> {
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(linux::LinuxBackend::new(if_name)?))
    }
    #[cfg(target_os = "windows")]
    {
        Ok(Box::new(windows::WindowsBackend::new(if_name)))
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = if_name;
        anyhow::bail!("この OS は未対応です(対応: Windows 10/11, Ubuntu 22.04+)")
    }
}
