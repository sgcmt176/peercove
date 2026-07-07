//! ユーザー空間 WG デバイスループ(Windows)。
//!
//! boringtun の `noise::Tunn`(プロトコルエンジン)と wintun セッションを
//! 3 本のスレッドで接続する:
//!
//! - UDP 受信: 復号してトンネルへ / ハンドシェイク応答を返送
//! - TUN 受信: AllowedIPs でピアを選び、暗号化して UDP 送信
//! - タイマー: 250ms ごとに `update_timers`(再送・keepalive・鍵更新)
//!
//! ピア判別は WG の慣例どおり: ハンドシェイク開始は静的公開鍵を復号して
//! (`parse_handshake_anon`)、それ以外は receiver_idx の上位 24bit
//! (`Tunn::new` に渡した index)で引く。

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use anyhow::Context;
use boringtun::noise::{errors::WireGuardError, handshake, Packet, Tunn, TunnResult};
use boringtun::x25519;
use ipnet::Ipv4Net;

use super::super::PeerSpec;

/// MTU 1500 + WG ヘッダに十分な作業バッファ。
const BUF_SIZE: usize = 2048;
const TIMER_TICK: Duration = Duration::from_millis(250);
/// UDP 受信のタイムアウト。shutdown フラグの確認周期を兼ねる。
const RECV_TIMEOUT: Duration = Duration::from_millis(500);

pub struct DevicePeer {
    pub public_key: [u8; 32],
    pub allowed_ips: Vec<Ipv4Net>,
    tunn: Mutex<Tunn>,
    endpoint: RwLock<Option<SocketAddr>>,
}

impl DevicePeer {
    pub fn endpoint(&self) -> Option<SocketAddr> {
        *self.endpoint.read().unwrap()
    }

    pub fn allows(&self, ip: Ipv4Addr) -> bool {
        self.allowed_ips.iter().any(|net| net.contains(&ip))
    }

    /// (最終ハンドシェイクからの経過, tx, rx)
    pub fn stats(&self) -> (Option<Duration>, usize, usize) {
        let (since, tx, rx, _loss, _rtt) = self.tunn.lock().unwrap().stats();
        (since, tx, rx)
    }
}

#[derive(Default)]
struct PeerTable {
    by_key: HashMap<[u8; 32], Arc<DevicePeer>>,
    by_index: HashMap<u32, Arc<DevicePeer>>,
    next_index: u32,
}

pub struct Device {
    private_key: x25519::StaticSecret,
    public_key: x25519::PublicKey,
    socket: UdpSocket,
    session: Arc<wintun::Session>,
    peers: RwLock<PeerTable>,
    shutdown: AtomicBool,
}

impl Device {
    pub fn new(
        private_key: [u8; 32],
        listen_port: Option<u16>,
        session: Arc<wintun::Session>,
    ) -> anyhow::Result<Arc<Self>> {
        let port = listen_port.unwrap_or(0);
        let socket = UdpSocket::bind(("0.0.0.0", port)).with_context(|| {
            format!(
                "UDP ポート {port} の bind に失敗しました。\
                他のプロセス(WireGuard クライアント等)が使用していないか確認してください"
            )
        })?;
        socket
            .set_read_timeout(Some(RECV_TIMEOUT))
            .context("UDP ソケットの設定に失敗しました")?;
        let private_key = x25519::StaticSecret::from(private_key);
        let public_key = x25519::PublicKey::from(&private_key);
        Ok(Arc::new(Self {
            private_key,
            public_key,
            socket,
            session,
            peers: RwLock::new(PeerTable::default()),
            shutdown: AtomicBool::new(false),
        }))
    }

    pub fn add_peer(&self, spec: &PeerSpec) -> anyhow::Result<()> {
        let mut table = self.peers.write().unwrap();
        if table.by_key.contains_key(spec.public_key.as_bytes()) {
            anyhow::bail!("ピア {} は登録済みです", spec.public_key);
        }
        let index = table.next_index;
        table.next_index += 1;
        let tunn = Tunn::new(
            self.private_key.clone(),
            x25519::PublicKey::from(*spec.public_key.as_bytes()),
            spec.preshared_key.as_ref().map(|k| *k.as_bytes()),
            spec.persistent_keepalive,
            index,
            None,
        );
        let peer = Arc::new(DevicePeer {
            public_key: *spec.public_key.as_bytes(),
            allowed_ips: spec.allowed_ips.clone(),
            tunn: Mutex::new(tunn),
            endpoint: RwLock::new(spec.endpoint),
        });
        table.by_key.insert(peer.public_key, Arc::clone(&peer));
        table.by_index.insert(index, Arc::clone(&peer));
        drop(table);

        // endpoint が分かっているピア(メンバー→ホスト)へは即ハンドシェイクを開始する
        if spec.endpoint.is_some() {
            let mut buf = [0u8; BUF_SIZE];
            let mut tunn = peer.tunn.lock().unwrap();
            if let TunnResult::WriteToNetwork(data) =
                tunn.format_handshake_initiation(&mut buf, false)
            {
                self.send_to_peer(&peer, data);
            }
        }
        Ok(())
    }

    pub fn peers(&self) -> Vec<Arc<DevicePeer>> {
        let table = self.peers.read().unwrap();
        let mut peers: Vec<_> = table.by_key.values().cloned().collect();
        peers.sort_by_key(|p| p.public_key);
        peers
    }

    pub fn local_port(&self) -> u16 {
        self.socket.local_addr().map(|a| a.port()).unwrap_or(0)
    }

    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        let _ = self.session.shutdown();
    }

    fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    fn send_to_peer(&self, peer: &DevicePeer, data: &[u8]) {
        if let Some(endpoint) = peer.endpoint() {
            if let Err(e) = self.socket.send_to(data, endpoint) {
                tracing::debug!("UDP 送信に失敗: {endpoint} {e}");
            }
        }
    }

    /// 復号済みパケットをトンネル(TUN)へ書き込む。
    fn write_to_tun(&self, packet: &[u8]) {
        let Ok(len) = u16::try_from(packet.len()) else {
            return;
        };
        match self.session.allocate_send_packet(len) {
            Ok(mut tun_packet) => {
                tun_packet.bytes_mut().copy_from_slice(packet);
                self.session.send_packet(tun_packet);
            }
            Err(e) => tracing::debug!("TUN への書き込みに失敗: {e}"),
        }
    }

    fn find_peer_for_datagram(&self, datagram: &[u8]) -> Option<Arc<DevicePeer>> {
        let packet = Tunn::parse_incoming_packet(datagram).ok()?;
        let table = self.peers.read().unwrap();
        match packet {
            Packet::HandshakeInit(ref init) => {
                let half =
                    handshake::parse_handshake_anon(&self.private_key, &self.public_key, init)
                        .ok()?;
                table.by_key.get(&half.peer_static_public).cloned()
            }
            Packet::HandshakeResponse(r) => table.by_index.get(&(r.receiver_idx >> 8)).cloned(),
            Packet::PacketCookieReply(r) => table.by_index.get(&(r.receiver_idx >> 8)).cloned(),
            Packet::PacketData(r) => table.by_index.get(&(r.receiver_idx >> 8)).cloned(),
        }
    }

    /// UDP 受信ループ。
    pub fn udp_loop(self: &Arc<Self>) {
        let mut recv_buf = [0u8; BUF_SIZE];
        let mut work_buf = [0u8; BUF_SIZE];
        while !self.is_shutdown() {
            let (len, src) = match self.socket.recv_from(&mut recv_buf) {
                Ok(ok) => ok,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    continue;
                }
                Err(e) => {
                    tracing::debug!("UDP 受信エラー: {e}");
                    continue;
                }
            };
            let datagram = &recv_buf[..len];
            let Some(peer) = self.find_peer_for_datagram(datagram) else {
                tracing::debug!("{src} からの不明なパケットを破棄しました");
                continue;
            };

            let mut tunn = peer.tunn.lock().unwrap();
            let mut result = tunn.decapsulate(Some(src.ip()), datagram, &mut work_buf);
            let mut authenticated = false;
            loop {
                match result {
                    TunnResult::WriteToNetwork(data) => {
                        authenticated = true;
                        if let Err(e) = self.socket.send_to(data, src) {
                            tracing::debug!("UDP 返送に失敗: {src} {e}");
                        }
                        // キューされたパケット(ハンドシェイク完了直後など)を掃き出す
                        result = tunn.decapsulate(None, &[], &mut work_buf);
                    }
                    TunnResult::WriteToTunnelV4(packet, src_ip) => {
                        authenticated = true;
                        if peer.allows(src_ip) {
                            self.write_to_tun(packet);
                        } else {
                            tracing::debug!(
                                "AllowedIPs 外の送信元 {src_ip} からのパケットを破棄しました"
                            );
                        }
                        break;
                    }
                    TunnResult::WriteToTunnelV6(..) => break, // IPv6 は M0 対象外
                    TunnResult::Done => {
                        // keepalive やハンドシェイク開始の受理もここに来る
                        authenticated = true;
                        break;
                    }
                    TunnResult::Err(WireGuardError::UnderLoad) => break,
                    TunnResult::Err(e) => {
                        tracing::debug!("{src} からのパケットの復号に失敗: {e:?}");
                        break;
                    }
                }
            }
            drop(tunn);

            // 認証済みパケットの送信元をピアの現在エンドポイントとして学習する(roaming)
            if authenticated && peer.endpoint() != Some(src) {
                tracing::info!(
                    "ピア {} のエンドポイントを {src} に更新しました",
                    &peercove_core::keys::PublicKey::from_bytes(peer.public_key)
                );
                *peer.endpoint.write().unwrap() = Some(src);
            }
        }
    }

    /// TUN 受信ループ(OS から出ていくパケットの暗号化)。
    pub fn tun_loop(self: &Arc<Self>) {
        let mut work_buf = [0u8; BUF_SIZE];
        while !self.is_shutdown() {
            let packet = match self.session.receive_blocking() {
                Ok(packet) => packet,
                Err(_) => break, // セッション shutdown
            };
            let bytes = packet.bytes();
            let Some(IpAddr::V4(dst)) = Tunn::dst_address(bytes) else {
                continue; // IPv6 等は M0 対象外
            };
            let peer = {
                let table = self.peers.read().unwrap();
                table.by_key.values().find(|p| p.allows(dst)).cloned()
            };
            let Some(peer) = peer else {
                tracing::debug!("宛先 {dst} に対応するピアがありません");
                continue;
            };
            let mut tunn = peer.tunn.lock().unwrap();
            match tunn.encapsulate(bytes, &mut work_buf) {
                TunnResult::WriteToNetwork(data) => {
                    if peer.endpoint().is_some() {
                        self.send_to_peer(&peer, data);
                    } else {
                        tracing::debug!(
                            "ピアのエンドポイントが未学習のため宛先 {dst} のパケットを破棄しました"
                        );
                    }
                }
                TunnResult::Err(e) => tracing::debug!("暗号化に失敗: {e:?}"),
                _ => {}
            }
        }
    }

    /// タイマーループ。ハンドシェイク再送・keepalive・セッション更新を駆動する。
    pub fn timer_loop(self: &Arc<Self>) {
        let mut work_buf = [0u8; BUF_SIZE];
        while !self.is_shutdown() {
            std::thread::sleep(TIMER_TICK);
            for peer in self.peers() {
                let mut tunn = peer.tunn.lock().unwrap();
                match tunn.update_timers(&mut work_buf) {
                    TunnResult::WriteToNetwork(data) => self.send_to_peer(&peer, data),
                    TunnResult::Err(WireGuardError::ConnectionExpired) => {}
                    TunnResult::Err(e) => tracing::debug!("タイマー処理でエラー: {e:?}"),
                    _ => {}
                }
            }
        }
    }
}
