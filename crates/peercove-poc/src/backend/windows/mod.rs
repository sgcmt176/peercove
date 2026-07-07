//! Windows バックエンド: wintun(TUN ドライバ)+ boringtun(ユーザー空間 WG)。
//!
//! wintun.dll は再配布しないため、実行ファイルと同じフォルダに手動配置する
//! (入手手順は README を参照)。アダプタ作成には管理者権限が必要。

mod device;

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::SystemTime;

use anyhow::{bail, Context};

use self::device::Device;
use super::{PeerSpec, PeerStats, TunnelSpec, WgBackend};

/// wintun アダプタの「トンネル種別」表示名。
const TUNNEL_TYPE: &str = "PeerCove";

pub struct WindowsBackend {
    if_name: String,
    running: Option<Running>,
}

struct Running {
    device: Arc<Device>,
    // ドロップ順: スレッド停止 → セッション → アダプタ(削除)
    _adapter: Arc<wintun::Adapter>,
    threads: Vec<JoinHandle<()>>,
}

impl WindowsBackend {
    pub fn new(if_name: &str) -> Self {
        Self {
            if_name: if_name.to_string(),
            running: None,
        }
    }

    fn load_wintun() -> anyhow::Result<wintun::Wintun> {
        // SAFETY: wintun.dll の読み込みは FFI 境界のため unsafe。dll が差し替えられて
        // いない(正規の wintun.net 配布物である)ことは利用者の配置手順に依存する。
        unsafe { wintun::load() }.context(
            "wintun.dll が読み込めませんでした。https://www.wintun.net から wintun \
             をダウンロードし、bin/amd64/wintun.dll を peercove-poc.exe と同じフォルダに\
             配置してください",
        )
    }
}

fn prefix_to_netmask(prefix: u8) -> Ipv4Addr {
    let bits = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(prefix))
    };
    Ipv4Addr::from(bits)
}

impl WgBackend for WindowsBackend {
    fn up(&mut self, spec: &TunnelSpec) -> anyhow::Result<()> {
        if self.running.is_some() {
            bail!("トンネルは既に起動しています");
        }
        let wintun = Self::load_wintun()?;
        let adapter = wintun::Adapter::create(&wintun, &self.if_name, TUNNEL_TYPE, None).context(
            "TUN アダプタの作成に失敗しました。管理者として実行した PowerShell / \
                 ターミナルで再実行してください",
        )?;
        adapter
            .set_address(spec.address.addr())
            .and_then(|()| adapter.set_netmask(prefix_to_netmask(spec.address.prefix_len())))
            .and_then(|()| adapter.set_mtu(usize::from(spec.mtu)))
            .context("TUN アダプタの IP/MTU 設定に失敗しました")?;

        let session = Arc::new(
            adapter
                .start_session(wintun::MAX_RING_CAPACITY)
                .context("TUN セッションの開始に失敗しました")?,
        );
        let device = Device::new(*spec.private_key.as_bytes(), spec.listen_port, session)?;
        for peer in &spec.peers {
            device.add_peer(peer)?;
        }
        tracing::info!("UDP ポート {} で待受を開始しました", device.local_port());

        let threads = vec![
            spawn("peercove-udp", Arc::clone(&device), Device::udp_loop),
            spawn("peercove-tun", Arc::clone(&device), Device::tun_loop),
            spawn("peercove-timer", Arc::clone(&device), Device::timer_loop),
        ];
        self.running = Some(Running {
            device,
            _adapter: adapter,
            threads,
        });
        Ok(())
    }

    fn add_peer(&mut self, peer: &PeerSpec) -> anyhow::Result<()> {
        match &self.running {
            Some(running) => running.device.add_peer(peer),
            None => bail!("トンネルが起動していません"),
        }
    }

    fn stats(&mut self) -> anyhow::Result<Vec<PeerStats>> {
        let Some(running) = &self.running else {
            bail!("トンネルが起動していません");
        };
        Ok(running
            .device
            .peers()
            .iter()
            .map(|peer| {
                let (since, tx, rx) = peer.stats();
                PeerStats {
                    public_key: peercove_core::keys::PublicKey::from_bytes(peer.public_key),
                    endpoint: peer.endpoint(),
                    last_handshake: since.map(|d| SystemTime::now() - d),
                    tx_bytes: tx as u64,
                    rx_bytes: rx as u64,
                    allowed_ips: peer.allowed_ips.clone(),
                }
            })
            .collect())
    }

    fn down(&mut self) -> anyhow::Result<()> {
        if let Some(running) = self.running.take() {
            running.device.shutdown();
            for thread in running.threads {
                let _ = thread.join();
            }
            // Running のドロップでセッションとアダプタのハンドルが閉じ、
            // wintun がアダプタを削除する
            return Ok(());
        }
        // 起動していないインスタンスからの呼び出し = 残骸クリーンアップ。
        // wintun アダプタはハンドルを握るプロセスが死ぬと消えるため、通常は
        // 残骸が無い。開けた場合のみハンドルを閉じて削除を促す。
        let wintun = Self::load_wintun()?;
        match wintun::Adapter::open(&wintun, &self.if_name) {
            Ok(_adapter) => {
                tracing::info!("アダプタ {} のハンドルを閉じました", self.if_name);
                Ok(())
            }
            Err(_) => {
                tracing::info!("アダプタ {} は存在しません", self.if_name);
                Ok(())
            }
        }
    }
}

fn spawn(
    name: &str,
    device: Arc<Device>,
    f: impl Fn(&Arc<Device>) + Send + 'static,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || f(&device))
        .expect("スレッドの起動に失敗しました")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn netmask_from_prefix() {
        assert_eq!(prefix_to_netmask(24), Ipv4Addr::new(255, 255, 255, 0));
        assert_eq!(prefix_to_netmask(32), Ipv4Addr::new(255, 255, 255, 255));
        assert_eq!(prefix_to_netmask(0), Ipv4Addr::new(0, 0, 0, 0));
        assert_eq!(prefix_to_netmask(30), Ipv4Addr::new(255, 255, 255, 252));
    }
}
