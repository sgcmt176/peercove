//! UPnP IGD によるポート自動開放(G-6、ADR-0004)。
//!
//! host 起動時に一回だけ試行し、成否と外部エンドポイント(推定)をレポートする。
//! 開放したマッピングは `UpnpLease::release`(down 時)で削除する。

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use anyhow::Context;
use igd_next::{AddPortError, Gateway, PortMappingProtocol, SearchOptions};

/// 異常終了で残ってもリース切れで消えるよう 24 時間にする(ADR-0004)。
const LEASE_SECS: u32 = 86_400;
const SEARCH_TIMEOUT: Duration = Duration::from_secs(5);
const MAPPING_DESCRIPTION: &str = "PeerCove";

/// 開放済みポートのハンドル。`release` で対で削除する。
pub struct UpnpLease {
    gateway: Gateway,
    external_port: u16,
}

pub struct UpnpReport {
    pub external_ip: IpAddr,
    pub external_port: u16,
    pub lease: UpnpLease,
}

impl UpnpLease {
    /// ポートマッピングを削除する(down 時に呼ぶ)。
    pub fn release(self) {
        match self
            .gateway
            .remove_port(PortMappingProtocol::UDP, self.external_port)
        {
            Ok(()) => tracing::info!(
                "UPnP ポートマッピング(UDP {})を削除しました",
                self.external_port
            ),
            Err(e) => tracing::warn!(
                "UPnP ポートマッピングの削除に失敗しました(リース切れで自動消滅します): {e}"
            ),
        }
    }
}

/// UPnP でポート開放を試行する。エラーには利用者が次に取るべき行動を含める。
///
/// トンネル作成**前**に呼ぶこと。トンネルの TUN にはマルチキャスト経路が付くため、
/// 後から呼ぶと SSDP 探索がトンネル側へ流れて失敗することがある。
pub fn setup(listen_port: u16) -> anyhow::Result<UpnpReport> {
    // 探索ソケットを物理 LAN の IP へ明示的にバインドし、仮想アダプタ
    // (VirtualBox / TUN 等)へマルチキャストが流れるのを防ぐ
    let bind_addr = default_route_local_ip()
        .map(|ip| SocketAddr::new(ip, 0))
        .unwrap_or_else(|| "0.0.0.0:0".parse().unwrap());
    tracing::debug!("SSDP 探索を {bind_addr} から送信します");
    let options = SearchOptions {
        bind_addr,
        timeout: Some(SEARCH_TIMEOUT),
        ..Default::default()
    };
    let gateway = igd_next::search_gateway(options).map_err(|e| {
        anyhow::anyhow!(
            "UPnP 対応ルーターが見つかりませんでした({e})。\
             次を確認してください: (1) ルーターの設定画面で UPnP が有効か \
             (2) 無効な場合は手動でポートフォワード(UDP {listen_port} → この PC)を設定 \
             (3) それでも外部から届かない場合は回線が CGNAT の可能性(ISP に \
             グローバル IP の提供を確認)"
        )
    })?;
    tracing::info!("UPnP ゲートウェイを発見: {}", gateway.addr);

    let external_ip = gateway
        .get_external_ip()
        .context("ルーターの外部 IP の取得に失敗しました")?;

    let local_ip = local_ip_towards(gateway.addr)
        .context("ゲートウェイへ向かうローカル IP の判定に失敗しました")?;
    let local_addr = SocketAddr::new(local_ip, listen_port);

    let add = |lease: u32| {
        gateway.add_port(
            PortMappingProtocol::UDP,
            listen_port,
            local_addr,
            lease,
            MAPPING_DESCRIPTION,
        )
    };
    match add(LEASE_SECS) {
        Ok(()) => {}
        // 一部のルーターは無期限リースしか受け付けない
        Err(AddPortError::OnlyPermanentLeasesSupported) => {
            tracing::warn!("ルーターが期限付きリース非対応のため無期限で開放します");
            add(0).map_err(add_port_error)?;
        }
        Err(e) => return Err(add_port_error(e)),
    }

    if let IpAddr::V4(v4) = external_ip {
        if !is_global_ipv4(v4) {
            tracing::warn!(
                "ルーターの外部 IP {v4} はグローバル IP ではありません。\
                 二重 NAT / CGNAT の可能性が高く、ポートを開放しても外部から\
                 届かないことがあります(上位ルーター側の設定 or ISP への確認が必要)"
            );
        }
    }

    Ok(UpnpReport {
        external_ip,
        external_port: listen_port,
        lease: UpnpLease {
            gateway,
            external_port: listen_port,
        },
    })
}

fn add_port_error(e: AddPortError) -> anyhow::Error {
    let hint = match &e {
        AddPortError::PortInUse => {
            "指定ポートは既に別のマッピングで使用されています。ルーターの設定画面で\
             既存のマッピングを削除するか、host.toml の listen_port を変更してください"
        }
        AddPortError::ActionNotAuthorized => {
            "ルーターがこの操作を許可していません。ルーターの UPnP 設定(セキュア\
             モード等)を確認するか、手動でポートフォワードを設定してください"
        }
        _ => "手動でポートフォワード(UDP をこの PC へ)を設定してください",
    };
    anyhow::anyhow!("ポート開放に失敗しました({e})。{hint}")
}

/// ゲートウェイへ向かう経路のローカル IP(= LAN 内でこの PC を指す IP)。
fn local_ip_towards(gateway: SocketAddr) -> anyhow::Result<IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0")?;
    socket.connect(gateway)?;
    Ok(socket.local_addr()?.ip())
}

/// デフォルトルート(インターネット)へ向かう経路のローカル IP。
/// UDP の connect は実際にはパケットを送らないため、外部へ通信は発生しない。
pub fn default_route_local_ip() -> Option<IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:53").ok()?;
    socket.local_addr().ok().map(|a| a.ip())
}

/// グローバル(外部から到達可能な)IPv4 かどうかの簡易判定。
/// プライベート(RFC1918)・CGNAT(RFC6598)・リンクローカル等を除外する。
fn is_global_ipv4(ip: Ipv4Addr) -> bool {
    let cgnat = (ip.octets()[0] == 100) && (64..=127).contains(&ip.octets()[1]);
    !(ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.is_unspecified() || cgnat)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_ipv4_classification() {
        let global: Ipv4Addr = "203.0.113.5".parse().unwrap();
        assert!(is_global_ipv4(global));
        for not_global in [
            "192.168.0.1",
            "10.0.0.1",
            "172.16.5.5",
            "100.64.0.1",      // CGNAT 下限
            "100.127.255.254", // CGNAT 上限
            "169.254.1.1",
            "127.0.0.1",
        ] {
            let ip: Ipv4Addr = not_global.parse().unwrap();
            assert!(!is_global_ipv4(ip), "{ip} がグローバル判定された");
        }
        // CGNAT レンジ外の 100.x はグローバル
        let ip: Ipv4Addr = "100.128.0.1".parse().unwrap();
        assert!(is_global_ipv4(ip));
    }
}
