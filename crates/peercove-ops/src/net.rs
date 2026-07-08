//! ネットワークの小さなヘルパ。

use std::net::IpAddr;

/// デフォルトルート(インターネット)へ向かう経路のローカル IP。
///
/// UDP の `connect` は実際にはパケットを送らないため、外部への通信は発生しない。
/// 招待トークンの LAN エンドポイント候補や UPnP の内部アドレス判定に使う。
pub fn default_route_local_ip() -> Option<IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:53").ok()?;
    socket.local_addr().ok().map(|a| a.ip())
}

/// 指定した宛先へ向かう経路のローカル IP(UPnP のゲートウェイ向け)。
pub fn local_ip_towards(target: std::net::SocketAddr) -> anyhow::Result<IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0")?;
    socket.connect(target)?;
    Ok(socket.local_addr()?.ip())
}
