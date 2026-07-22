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

use self::device::{Device, TunIo};
use super::{IsolatedPeer, PeerSpec, PeerStats, TunnelSpec, WgBackend};

/// wintun アダプタの「トンネル種別」表示名。
const TUNNEL_TYPE: &str = "PeerCove";

pub struct WindowsBackend {
    if_name: String,
    running: Option<Running>,
    /// ルーター役未対応の警告を出したか(毎周期のスパム防止、ADR-0014)。
    router_warned: bool,
}

struct Running {
    device: Arc<Device>,
    host_ip: Ipv4Addr,
    // ドロップ順: スレッド停止 → セッション → アダプタ(削除)
    _adapter: Arc<wintun::Adapter>,
    threads: Vec<JoinHandle<()>>,
}

impl WindowsBackend {
    pub fn new(if_name: &str) -> Self {
        Self {
            if_name: if_name.to_string(),
            running: None,
            router_warned: false,
        }
    }

    /// netsh でトンネル IF への経路を操作する(ADR-0014)。
    fn netsh_route(&self, op: &str, subnet: &ipnet::Ipv4Net) -> anyhow::Result<()> {
        let subnet = subnet.to_string();
        let args = [
            "interface",
            "ipv4",
            op,
            "route",
            &subnet,
            &self.if_name,
            "store=active",
        ];
        let output = std::process::Command::new("netsh")
            .args(args)
            .output()
            .context("netsh を実行できません")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stdout);
            // 既に無い経路の削除は成功扱い(冪等)。netsh は日本語環境だと
            // 文言がローカライズされるため、終了コードとメッセージの両方を見る
            if op == "delete" && (stderr.contains("見つかりません") || stderr.contains("not found"))
            {
                return Ok(());
            }
            bail!("netsh {} が失敗しました: {}", args.join(" "), stderr.trim());
        }
        Ok(())
    }

    fn load_wintun() -> anyhow::Result<wintun::Wintun> {
        // exe と同じフォルダの dll を最優先で読み込む。既定の DLL 検索パス任せに
        // すると、他ソフトが System32 等に置いた wintun.dll が使われて挙動が
        // 環境依存になるため(検証時に発覚)。
        let exe_dir_dll = std::env::current_exe()
            .ok()
            .and_then(|exe| Some(exe.parent()?.join("wintun.dll")))
            .filter(|path| path.exists());
        // SAFETY: wintun.dll の読み込みは FFI 境界のため unsafe。dll が差し替えられて
        // いない(正規の wintun.net 配布物である)ことは利用者の配置手順に依存する。
        let result = match &exe_dir_dll {
            Some(path) => {
                tracing::debug!("{} を読み込みます", path.display());
                unsafe { wintun::load_from_path(path) }
            }
            None => {
                tracing::warn!(
                    "exe と同じフォルダに wintun.dll が無いため、システムの DLL 検索\
                     パスから探します"
                );
                unsafe { wintun::load() }
            }
        };
        result.context(
            "wintun.dll が読み込めませんでした。https://www.wintun.net から wintun \
             をダウンロードし、bin/amd64/wintun.dll を peercove.exe と同じフォルダに\
             配置してください",
        )
    }
}

/// wintun セッションを [`TunIo`] として扱うアダプタ。
struct WintunIo {
    session: Arc<wintun::Session>,
}

impl TunIo for WintunIo {
    fn recv(&self, buf: &mut [u8]) -> anyhow::Result<usize> {
        loop {
            let packet = self
                .session
                .receive_blocking()
                .map_err(|e| anyhow::anyhow!("wintun セッションが停止しました: {e}"))?;
            let bytes = packet.bytes();
            // 毎パケットの Vec 確保を避け、呼び出し側の固定バッファへ直接コピー
            // する(C-3)。MTU 設定済みなので通常は収まる。収まらない異常サイズは
            // 切り詰めると壊れたパケットになるため破棄して次を待つ
            if bytes.len() > buf.len() {
                tracing::warn!(
                    "TUN から {} バイトの過大パケットを破棄しました",
                    bytes.len()
                );
                continue;
            }
            buf[..bytes.len()].copy_from_slice(bytes);
            return Ok(bytes.len());
        }
    }

    fn send(&self, packet: &[u8]) -> anyhow::Result<()> {
        let len = u16::try_from(packet.len()).context("パケットが大きすぎます")?;
        let mut tun_packet = self
            .session
            .allocate_send_packet(len)
            .map_err(|e| anyhow::anyhow!("TUN バッファの確保に失敗しました: {e}"))?;
        tun_packet.bytes_mut().copy_from_slice(packet);
        self.session.send_packet(tun_packet);
        Ok(())
    }

    fn shutdown(&self) {
        let _ = self.session.shutdown();
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
        // netsh が黙って失敗すると tx 経路が丸ごと死ぬため、設定を読み戻して確認する
        let assigned = adapter.get_addresses().unwrap_or_default();
        if assigned
            .iter()
            .any(|a| *a == std::net::IpAddr::V4(spec.address.addr()))
        {
            tracing::info!(
                "アダプタ {} に {} を割り当てました",
                self.if_name,
                spec.address
            );
        } else {
            tracing::warn!(
                "アダプタ {} への IP 割当が確認できませんでした(現在: {assigned:?})。\
                 `ipconfig` と `route print -4` で 100.100.42.0/24 の経路を確認してください",
                self.if_name
            );
        }

        let session = Arc::new(
            adapter
                .start_session(wintun::MAX_RING_CAPACITY)
                .context("TUN セッションの開始に失敗しました")?,
        );
        if spec.forwarding {
            tracing::info!("ピア間転送(ハブ&スポーク)を有効化しました(デバイス内リレー)");
        }
        let device = Device::new(
            *spec.private_key.as_bytes(),
            spec.listen_port,
            spec.forwarding,
            Box::new(WintunIo { session }),
        )?;
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
            host_ip: spec.address.addr(),
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

    fn remove_peer(&mut self, public_key: &peercove_core::keys::PublicKey) -> anyhow::Result<()> {
        match &self.running {
            Some(running) => {
                running.device.remove_peer(public_key.as_bytes());
                Ok(())
            }
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
                    allowed_ips: peer.allowed_ips(),
                }
            })
            .collect())
    }

    fn add_route(&mut self, subnet: ipnet::Ipv4Net) -> anyhow::Result<()> {
        self.netsh_route("add", &subnet)
            .with_context(|| format!("経路 {subnet} の追加に失敗しました"))
    }

    fn remove_route(&mut self, subnet: ipnet::Ipv4Net) -> anyhow::Result<()> {
        self.netsh_route("delete", &subnet)
            .with_context(|| format!("経路 {subnet} の削除に失敗しました"))
    }

    fn sync_acl(&mut self, policy: &peercove_core::acl::AclPolicy) -> anyhow::Result<()> {
        match &self.running {
            Some(running) => {
                // デバイス内リレーはピアの身元(仮想 IP)で判定するため、
                // サブネットは渡さなくてよい(device.rs の virtual_ip 参照)
                running.device.set_acl(policy.clone());
                Ok(())
            }
            None => bail!("トンネルが起動していません"),
        }
    }

    fn sync_subnet_router(
        &mut self,
        _virtual_subnet: ipnet::Ipv4Net,
        subnets: &[ipnet::Ipv4Net],
        _snat: bool,
    ) -> anyhow::Result<()> {
        // V1 では Windows はルーター役非対応(ADR-0014。利用側としては対応)
        if !subnets.is_empty() && !self.router_warned {
            self.router_warned = true;
            tracing::warn!(
                "このマシンにサブネット {subnets:?} が割り当てられていますが、\
                 Windows でのルーター役は未対応です(Linux のマシンを使ってください)"
            );
        }
        if subnets.is_empty() {
            self.router_warned = false;
        }
        Ok(())
    }

    fn sync_isolation(&mut self, isolated: &[IsolatedPeer]) -> anyhow::Result<()> {
        let Some(running) = &self.running else {
            bail!("トンネルが起動していません");
        };
        let ips: Vec<Ipv4Addr> = isolated.iter().map(|peer| peer.ip).collect();
        running.device.set_isolated(&ips, running.host_ip);
        Ok(())
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
