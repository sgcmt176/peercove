//! UPnP IGD によるポート自動開放(G-6、ADR-0004)。
//!
//! host 起動時に一回だけ試行し、成否と外部エンドポイント(推定)をレポートする。
//! 開放したマッピングは `UpnpLease::release`(down 時)で削除する。

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

use anyhow::Context;
use igd_next::{AddPortError, Gateway, PortMappingProtocol, SearchOptions};

/// 異常終了で残ってもリース切れで消えるよう 24 時間にする(ADR-0004)。
const LEASE_SECS: u32 = 86_400;
/// igd 本探索のタイムアウト。事前プローブが通った(= ルーターは応答が速い)
/// 場合しか到達しないので短くてよい。
const SEARCH_TIMEOUT: Duration = Duration::from_secs(5);
/// 本探索の試行回数(1 回目で取りこぼしても粘る)。
const SEARCH_ATTEMPTS: usize = 2;
/// 事前プローブの総待ち時間。igd の本探索は応答が 1 件も無いと
/// タイムアウトまで待ち続け、host の起動(接続)を数十秒ブロックする
/// 事例があったため、まず短時間の自前プローブで環境を判定する。
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// 最初の応答を受けた後、追加の応答を待つ時間(複数ルーター環境向け)。
const PROBE_LINGER: Duration = Duration::from_millis(500);
/// デバイス情報(LOCATION)の HTTP 到達検査のタイムアウト。
const HTTP_CHECK_TIMEOUT: Duration = Duration::from_secs(3);
const SSDP_MULTICAST: &str = "239.255.255.250:1900";
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

    // 事前プローブ: SSDP 応答の有無と、デバイス情報(LOCATION)の HTTP 到達性を
    // 数秒で判定する。igd の本探索は「応答はあるが HTTP が失敗する」壊れ方でも
    // タイムアウトまで待つため、失敗環境の切り分けと高速化を兼ねる
    match probe_ssdp(bind_addr) {
        Ok(candidates) if candidates.is_empty() => {
            anyhow::bail!(
                "UPnP 対応ルーターが見つかりませんでした(SSDP 探索に応答なし)。\
                 次を確認してください: (1) ルーターの設定画面で UPnP が有効か \
                 (2) 無効な場合は手動でポートフォワード(UDP {listen_port} → この PC)を設定し、\
                 招待発行時に「外部の接続先」へ グローバルIP:{listen_port} を入力 \
                 (3) それでも外部から届かない場合は回線が CGNAT の可能性(ISP に \
                 グローバル IP の提供を確認)"
            );
        }
        Ok(candidates) => {
            let mut last_error: Option<(SocketAddr, String)> = None;
            let reachable = candidates
                .iter()
                .any(|(from, location)| match http_check(location) {
                    Ok(()) => true,
                    Err(e) => {
                        tracing::debug!("デバイス情報の取得検査に失敗({from} {location}): {e}");
                        last_error = Some((*from, e));
                        false
                    }
                });
            if !reachable {
                let (from, e) = last_error.expect("空でない候補が全滅した場合はエラーがある");
                anyhow::bail!(
                    "ルーター({from})は UPnP 探索に応答しましたが、デバイス情報の取得に\
                     失敗しました({e})。ルーターの UPnP 機能が正常に動作していない可能性が\
                     あります。ルーターの再起動を試してください。改善しない場合は手動で\
                     ポートフォワード(UDP {listen_port} → この PC)を設定し、招待発行時に\
                     「外部の接続先」へ グローバルIP:{listen_port} を入力してください"
                );
            }
        }
        // プローブ自体の失敗(ソケット作成不可など)は判定不能として本探索に進む
        Err(e) => tracing::debug!("SSDP プローブに失敗したため本探索のみ行います: {e}"),
    }

    // igd 本探索。何回か試す(1 回目で応答を取りこぼしても粘る)
    let mut last_error = None;
    let mut found = None;
    for attempt in 1..=SEARCH_ATTEMPTS {
        let options = SearchOptions {
            bind_addr,
            timeout: Some(SEARCH_TIMEOUT),
            ..Default::default()
        };
        match igd_next::search_gateway(options) {
            Ok(gateway) => {
                found = Some(gateway);
                break;
            }
            Err(e) => {
                tracing::debug!("UPnP 探索 {attempt}/{SEARCH_ATTEMPTS} 回目が失敗: {e}");
                last_error = Some(e);
            }
        }
    }
    let gateway = found.ok_or_else(|| {
        let e = last_error.expect("失敗時は必ずエラーがある");
        anyhow::anyhow!(
            "UPnP ルーターとの通信に失敗しました({e})。\
             ルーターの再起動を試すか、手動でポートフォワード\
             (UDP {listen_port} → この PC)を設定し、\
             招待発行時に「外部の接続先」へ グローバルIP:{listen_port} を入力してください"
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

// 経路のローカル IP を求めるヘルパは `peercove-ops::net`(招待の
// エンドポイント候補と共用)。
use peercove_ops::net::{default_route_local_ip, local_ip_towards};

/// 自前の軽量 SSDP 探索。IGD を探す M-SEARCH を送り、応答元と LOCATION
/// (デバイス情報 URL)を集める。応答が 1 件も無ければ空を返す。
fn probe_ssdp(bind_addr: SocketAddr) -> std::io::Result<Vec<(SocketAddr, String)>> {
    let socket = UdpSocket::bind(bind_addr)?;
    socket.set_read_timeout(Some(Duration::from_millis(250)))?;
    let msearch = "M-SEARCH * HTTP/1.1\r\n\
                   Host: 239.255.255.250:1900\r\n\
                   Man: \"ssdp:discover\"\r\n\
                   MX: 3\r\n\
                   ST: urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\n\r\n";
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let mut next_send = Instant::now();
    let mut first_response: Option<Instant> = None;
    let mut found: Vec<(SocketAddr, String)> = Vec::new();
    let mut buf = [0u8; 2048];
    while Instant::now() < deadline {
        // 取りこぼしに備えて 2 秒おきに再送する
        if Instant::now() >= next_send {
            socket.send_to(msearch.as_bytes(), SSDP_MULTICAST)?;
            next_send = Instant::now() + Duration::from_secs(2);
        }
        if let Some(t) = first_response {
            if t.elapsed() >= PROBE_LINGER {
                break;
            }
        }
        match socket.recv_from(&mut buf) {
            Ok((n, from)) => {
                first_response.get_or_insert_with(Instant::now);
                let text = String::from_utf8_lossy(&buf[..n]);
                if let Some(location) = header_value(&text, "location") {
                    if !found.iter().any(|(_, l)| *l == location) {
                        found.push((from, location));
                    }
                }
            }
            // read timeout は Windows では TimedOut、Unix では WouldBlock になる
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(found)
}

/// SSDP 応答(HTTP ヘッダ形式)から指定ヘッダの値を取り出す(大小無視)。
fn header_value(response: &str, name: &str) -> Option<String> {
    response.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case(name)
            .then(|| value.trim().to_string())
    })
}

/// `http://host:port/path` を (host:port, /path) に分解する。
fn split_location(location: &str) -> Result<(String, String), String> {
    let rest = location
        .strip_prefix("http://")
        .ok_or_else(|| format!("未対応の URL 形式です: {location}"))?;
    match rest.split_once('/') {
        Some((hostport, path)) => Ok((hostport.to_string(), format!("/{path}"))),
        None => Ok((rest.to_string(), "/".to_string())),
    }
}

/// LOCATION の URL へ HTTP GET を送り、HTTP 応答が返るかだけを検査する。
/// ルーターの UPnP サービスが SSDP には応答するのに HTTP を即切断する
/// 壊れ方(実地で観測)を短時間で検出するのが目的で、内容は解釈しない。
fn http_check(location: &str) -> Result<(), String> {
    let (hostport, path) = split_location(location)?;
    let addr = if hostport.contains(':') {
        hostport.to_socket_addrs()
    } else {
        (hostport.as_str(), 80).to_socket_addrs()
    }
    .map_err(|e| format!("アドレス解決に失敗: {e}"))?
    .next()
    .ok_or_else(|| format!("アドレス解決に失敗: {hostport}"))?;
    let mut stream = TcpStream::connect_timeout(&addr, HTTP_CHECK_TIMEOUT)
        .map_err(|e| format!("接続に失敗: {e}"))?;
    stream
        .set_read_timeout(Some(HTTP_CHECK_TIMEOUT))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(HTTP_CHECK_TIMEOUT))
        .map_err(|e| e.to_string())?;
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: {hostport}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .map_err(|e| format!("送信に失敗: {e}"))?;
    // 判定に必要な先頭 "HTTP/" が揃うまで読む(小刻みに届くルーター対策)
    let mut head = Vec::with_capacity(8);
    let mut buf = [0u8; 16];
    while head.len() < 5 {
        let n = stream
            .read(&mut buf)
            .map_err(|e| format!("応答の受信に失敗: {e}"))?;
        if n == 0 {
            return Err("応答なく切断されました".into());
        }
        head.extend_from_slice(&buf[..n]);
    }
    if !head.starts_with(b"HTTP/") {
        return Err("HTTP 以外の応答が返りました".into());
    }
    Ok(())
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

    #[test]
    fn ssdp_location_header_is_extracted() {
        let response = "HTTP/1.1 200 OK\r\n\
                        Cache-Control: max-age=120\r\n\
                        ST: urn:schemas-upnp-org:device:InternetGatewayDevice:1\r\n\
                        LOCATION: http://192.168.0.1:2869/upnp/0bec430.xml\r\n\r\n";
        assert_eq!(
            header_value(response, "location").as_deref(),
            Some("http://192.168.0.1:2869/upnp/0bec430.xml")
        );
        assert_eq!(header_value(response, "usn"), None);
    }

    #[test]
    fn location_url_is_split_into_hostport_and_path() {
        assert_eq!(
            split_location("http://192.168.0.1:2869/upnp/desc.xml").unwrap(),
            ("192.168.0.1:2869".to_string(), "/upnp/desc.xml".to_string())
        );
        assert_eq!(
            split_location("http://192.168.0.1").unwrap(),
            ("192.168.0.1".to_string(), "/".to_string())
        );
        assert!(split_location("https://192.168.0.1/desc.xml").is_err());
    }
}
