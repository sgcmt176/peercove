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
use std::time::{Duration, Instant};

use anyhow::Context;
use boringtun::noise::{errors::WireGuardError, handshake, Packet, Tunn, TunnResult};
use boringtun::x25519;
use ipnet::Ipv4Net;
use peercove_core::proto::CONTROL_PORT;

use super::super::PeerSpec;

/// TUN デバイスの読み書き。wintun の他、テスト用モックを差し込めるようにする。
pub trait TunIo: Send + Sync + 'static {
    /// OS から出ていくパケットを 1 つ受け取る(ブロッキング)。Err はシャットダウン。
    fn recv(&self) -> anyhow::Result<Vec<u8>>;
    /// 復号済みパケットを OS へ渡す。
    fn send(&self, packet: &[u8]) -> anyhow::Result<()>;
    /// `recv` のブロックを解除する。
    fn shutdown(&self);
}

/// MTU 1500 + WG ヘッダに十分な作業バッファ。
const BUF_SIZE: usize = 2048;
const TIMER_TICK: Duration = Duration::from_millis(250);
/// UDP 受信のタイムアウト。shutdown フラグの確認周期を兼ねる。
const RECV_TIMEOUT: Duration = Duration::from_millis(500);

pub struct DevicePeer {
    pub public_key: [u8; 32],
    /// 直接通信の二段階 AllowedIPs(ADR-0019: プローブは空 → 確立で /32)で
    /// 実行中に書き換わるため RwLock。
    allowed_ips: RwLock<Vec<Ipv4Net>>,
    /// `Tunn::new` に渡した 24bit ピアインデックス(削除時にテーブルから引く)。
    index: u32,
    tunn: Mutex<Tunn>,
    endpoint: RwLock<Option<SocketAddr>>,
}

impl DevicePeer {
    pub fn endpoint(&self) -> Option<SocketAddr> {
        *self.endpoint.read().unwrap()
    }

    pub fn allowed_ips(&self) -> Vec<Ipv4Net> {
        self.allowed_ips.read().unwrap().clone()
    }

    /// このピアの仮想 IP(AllowedIPs の先頭 = 慣例で `<仮想IP>/32`)。
    /// ACL(ADR-0018)のリレー判定は、パケットの送信元 IP でなく
    /// **どのピアから来てどのピアへ出るか**で行うため、これを身元に使う
    /// (広告サブネット由来のトラフィックも自然に遮断される)。
    fn virtual_ip(&self) -> Option<Ipv4Addr> {
        self.allowed_ips
            .read()
            .unwrap()
            .first()
            .map(|net| net.addr())
    }

    pub fn allows(&self, ip: Ipv4Addr) -> bool {
        self.allowed_ips
            .read()
            .unwrap()
            .iter()
            .any(|net| net.contains(&ip))
    }

    /// `ip` を含む AllowedIPs のうち最長のプレフィックス長(含まなければ None)。
    fn longest_match(&self, ip: Ipv4Addr) -> Option<u8> {
        self.allowed_ips
            .read()
            .unwrap()
            .iter()
            .filter(|net| net.contains(&ip))
            .map(|net| net.prefix_len())
            .max()
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
    tun: Box<dyn TunIo>,
    peers: RwLock<PeerTable>,
    /// ピア間転送(ハブ&スポーク)。宛先が別ピアのパケットを OS を経由せず
    /// デバイス内で直接リレーする(ADR-0003)。ホストのみ true。
    relay: bool,
    /// ACL の遮断組(ADR-0018)。仮想 IP の正規化済みペア(小さい方が先)。
    /// リレー時に「来たピア × 出るピア」の組で判定して破棄する。
    acl_policy: RwLock<peercove_core::acl::AclPolicy>,
    /// 許可された新規セッションの応答方向。Linux conntrack と同じく、
    /// 片方向 deny が逆方向から開始した通信の応答まで壊さないために使う。
    acl_sessions: Mutex<peercove_core::acl::AclSessionTracker>,
    /// 承認待ち端末の仮想 IP。ホストのコントロール TCP 以外を破棄する。
    isolated: RwLock<std::collections::HashSet<Ipv4Addr>>,
    isolation_host: RwLock<Option<Ipv4Addr>>,
    shutdown: AtomicBool,
}

impl Device {
    pub fn new(
        private_key: [u8; 32],
        listen_port: Option<u16>,
        relay: bool,
        tun: Box<dyn TunIo>,
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
            tun,
            peers: RwLock::new(PeerTable::default()),
            relay,
            acl_policy: RwLock::new(peercove_core::acl::AclPolicy {
                default: peercove_core::acl::AclAction::Allow,
                rules: vec![],
            }),
            acl_sessions: Mutex::new(peercove_core::acl::AclSessionTracker::default()),
            isolated: RwLock::new(std::collections::HashSet::new()),
            isolation_host: RwLock::new(None),
            shutdown: AtomicBool::new(false),
        }))
    }

    /// ACL の遮断組を丸ごと差し替える(ADR-0018。冪等、空で全解除)。
    pub fn set_acl(&self, policy: peercove_core::acl::AclPolicy) {
        *self.acl_policy.write().unwrap() = policy;
    }

    pub fn set_isolated(&self, isolated: &[Ipv4Addr], host_ip: Ipv4Addr) {
        *self.isolated.write().unwrap() = isolated.iter().copied().collect();
        *self.isolation_host.write().unwrap() = Some(host_ip);
    }

    fn is_isolated(&self, peer: &DevicePeer) -> bool {
        peer.virtual_ip()
            .is_some_and(|ip| self.isolated.read().unwrap().contains(&ip))
    }

    /// IPv4/TCP のコントロールチャネルだけを識別する。inbound は member→host、
    /// outbound は host→member の向き。
    fn is_control_packet(packet: &[u8], inbound: bool) -> bool {
        if packet.len() < 20 || packet[0] >> 4 != 4 || packet[9] != 6 {
            return false;
        }
        let header_len = usize::from(packet[0] & 0x0f) * 4;
        if header_len < 20 || packet.len() < header_len + 4 {
            return false;
        }
        let src = u16::from_be_bytes([packet[header_len], packet[header_len + 1]]);
        let dst = u16::from_be_bytes([packet[header_len + 2], packet[header_len + 3]]);
        if inbound {
            dst == CONTROL_PORT
        } else {
            src == CONTROL_PORT
        }
    }

    /// ピアを追加する。既存ピアなら AllowedIPs の更新として働く(upsert、
    /// ADR-0019)。セッション(Tunn)と roaming 学習済みエンドポイントは
    /// 維持する。鍵・PSK・keepalive の変更は remove → add で行うこと。
    pub fn add_peer(&self, spec: &PeerSpec) -> anyhow::Result<()> {
        let mut table = self.peers.write().unwrap();
        if let Some(existing) = table.by_key.get(spec.public_key.as_bytes()) {
            *existing.allowed_ips.write().unwrap() = spec.allowed_ips.clone();
            return Ok(());
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
            allowed_ips: RwLock::new(spec.allowed_ips.clone()),
            index,
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

    /// ピアを削除する。以後このピアのパケットはテーブルで引けなくなり破棄される。
    /// 存在しない場合は何もしない(冪等)。
    pub fn remove_peer(&self, public_key: &[u8; 32]) {
        let mut table = self.peers.write().unwrap();
        if let Some(peer) = table.by_key.remove(public_key) {
            table.by_index.remove(&peer.index);
            // 直接接続の再試行(ADR-0019: 60 秒周期)でも通るため debug。
            // 意味のある削除(メンバー削除・経路の状態変化)は呼び出し側が
            // 理由つきで INFO を出す
            tracing::debug!(
                "ピア {} を削除しました",
                peercove_core::keys::PublicKey::from_bytes(*public_key)
            );
        }
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
        self.tun.shutdown();
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
        if let Err(e) = self.tun.send(packet) {
            tracing::debug!("TUN への書き込みに失敗: {e}");
        }
    }

    /// 宛先 IP からピアを選ぶ。**最長プレフィックス一致**(ADR-0013)。
    /// 直接通信ではホスト(/24)と直接ピア(/32)の AllowedIPs が重なるため、
    /// first-match(HashMap 順で不定)では経路が壊れる。
    fn find_peer_by_dst(&self, dst: Ipv4Addr) -> Option<Arc<DevicePeer>> {
        let table = self.peers.read().unwrap();
        table
            .by_key
            .values()
            .filter_map(|p| p.longest_match(dst).map(|len| (len, p)))
            .max_by_key(|(len, _)| *len)
            .map(|(_, p)| Arc::clone(p))
    }

    /// 復号済みパケットを配送する。宛先が別ピアならデバイス内で直接リレーし
    /// (ハブ&スポーク、ADR-0003)、それ以外は TUN(自分の OS)へ渡す。
    fn deliver_inbound(&self, from: &Arc<DevicePeer>, packet: &[u8]) {
        let dst = match Tunn::dst_address(packet) {
            Some(IpAddr::V4(dst)) => dst,
            _ => {
                self.write_to_tun(packet);
                return;
            }
        };
        tracing::trace!("復号 {} バイトを受信(宛先 {dst})", packet.len());
        if self.is_isolated(from)
            && (Some(dst) != *self.isolation_host.read().unwrap()
                || self.find_peer_by_dst(dst).is_some()
                || !Self::is_control_packet(packet, true))
        {
            tracing::trace!("承認待ち端末からの隔離対象パケットを破棄しました");
            return;
        }
        if self.relay {
            if let Some(target) = self.find_peer_by_dst(dst) {
                // 送信元ピア宛への折り返しはループになるため TUN 側へ落とす
                if !Arc::ptr_eq(&target, from) {
                    if self.is_isolated(&target) {
                        tracing::trace!("承認待ち端末宛のリレーパケットを破棄しました");
                        return;
                    }
                    // ACL(ADR-0018/0035): 新規セッションの開始方向を判定し、
                    // 許可された逆方向セッションの応答だけは deny 方向でも通す。
                    let policy = self.acl_policy.read().unwrap();
                    let decision = policy.evaluate_ipv4_packet(packet);
                    let mut sessions = self.acl_sessions.lock().unwrap();
                    let now = Instant::now();
                    let established_reply = decision.action == peercove_core::acl::AclAction::Deny
                        && sessions.allows_reply(packet, now);
                    if decision.action == peercove_core::acl::AclAction::Allow {
                        sessions.observe_allowed(packet, now);
                    } else if established_reply {
                        tracing::trace!(
                            "ACL で許可済みセッションの応答をリレーします(rule={:?})",
                            decision.rule_id
                        );
                    } else {
                        tracing::trace!(
                            "ACL によりリレーを破棄しました(rule={:?})",
                            decision.rule_id
                        );
                        return;
                    }
                    tracing::trace!("宛先 {dst} のピアへ直接リレーします");
                    let mut buf = [0u8; BUF_SIZE];
                    let mut tunn = target.tunn.lock().unwrap();
                    match tunn.encapsulate(packet, &mut buf) {
                        TunnResult::WriteToNetwork(data) => self.send_to_peer(&target, data),
                        TunnResult::Err(e) => tracing::warn!("リレーの暗号化に失敗: {e:?}"),
                        // セッション未確立時はキューされ、ハンドシェイク完了後に流れる
                        _ => {}
                    }
                    return;
                }
            }
        }
        self.write_to_tun(packet);
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
                // 削除済みメンバーの再ハンドシェイクなどで定期的に届く。
                // 拒否できているので debug に留める(5 秒間隔で繰り返しうる)
                tracing::debug!(
                    "{src} からの不明なパケットを破棄しました\
                     (登録されていないピアからのハンドシェイクの可能性)"
                );
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
                            self.deliver_inbound(&peer, packet);
                        } else {
                            tracing::warn!(
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
                    TunnResult::Err(WireGuardError::NoCurrentSession) => {
                        // 典型例: こちらの再起動後、相手が古いセッションで送信している。
                        // 相手は約 15 秒でデータ無応答を検知して再ハンドシェイクする
                        tracing::warn!(
                            "{src} からのパケットを復号できません(セッション不一致)。\
                             相手側の自動再接続(約 15 秒)を待っています"
                        );
                        break;
                    }
                    TunnResult::Err(e) => {
                        tracing::warn!("{src} からのパケットの復号に失敗: {e:?}");
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
            let packet = match self.tun.recv() {
                Ok(packet) => packet,
                Err(e) => {
                    if !self.is_shutdown() {
                        tracing::warn!("TUN の読み取りが停止しました: {e:#}");
                    }
                    break;
                }
            };
            let bytes = packet.as_slice();
            let Some(IpAddr::V4(dst)) = Tunn::dst_address(bytes) else {
                continue; // IPv6 等は M0 対象外
            };
            // パケット 1 個ごとのログは trace(debug でも多すぎるため)
            tracing::trace!("TUN から {} バイト受信(宛先 {dst})", bytes.len());
            // ブロードキャスト・マルチキャスト(NetBIOS、mDNS 等)は対象外。
            // ユニキャストのみピアへ転送する
            if dst.is_multicast() || dst.is_broadcast() || dst.octets()[3] == 255 {
                tracing::trace!("ブロードキャスト/マルチキャスト宛 {dst} を無視します");
                continue;
            }
            let Some(peer) = self.find_peer_by_dst(dst) else {
                tracing::warn!("宛先 {dst} に対応するピアがありません(AllowedIPs 未登録)");
                continue;
            };
            if self.is_isolated(&peer) && !Self::is_control_packet(bytes, false) {
                tracing::trace!("承認待ち端末宛の隔離対象パケットを破棄しました");
                continue;
            }
            let mut tunn = peer.tunn.lock().unwrap();
            match tunn.encapsulate(bytes, &mut work_buf) {
                TunnResult::WriteToNetwork(data) => {
                    if peer.endpoint().is_some() {
                        self.send_to_peer(&peer, data);
                    } else {
                        tracing::warn!(
                            "ピアのエンドポイントが未学習のため宛先 {dst} のパケットを破棄しました"
                        );
                    }
                }
                TunnResult::Err(e) => tracing::warn!("暗号化に失敗: {e:?}"),
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

#[cfg(test)]
mod tests {
    //! 2 つの `Device` を localhost の UDP で対向させ、モック TUN 経由で
    //! WG プロトコル一式(ハンドシェイク・暗号化データ転送・AllowedIPs 検査)を
    //! 実トンネルなしで検証するループバックテスト。

    use std::sync::mpsc;
    use std::thread::JoinHandle;
    use std::time::Instant;

    use super::*;
    use peercove_core::keys::{PrivateKey, PublicKey};

    /// チャネルで OS 側を模す TUN。
    /// - `os_out_tx` にテストが積んだパケット = 「OS が送信したパケット」
    /// - `os_in_rx` でテストが受け取るパケット = 「OS に届いたパケット」
    struct MockTun {
        os_out_rx: Mutex<mpsc::Receiver<Vec<u8>>>,
        os_in_tx: Mutex<mpsc::Sender<Vec<u8>>>,
        shutdown: AtomicBool,
    }

    struct MockTunHandles {
        os_out_tx: mpsc::Sender<Vec<u8>>,
        os_in_rx: mpsc::Receiver<Vec<u8>>,
    }

    fn mock_tun() -> (Box<MockTun>, MockTunHandles) {
        let (os_out_tx, os_out_rx) = mpsc::channel();
        let (os_in_tx, os_in_rx) = mpsc::channel();
        (
            Box::new(MockTun {
                os_out_rx: Mutex::new(os_out_rx),
                os_in_tx: Mutex::new(os_in_tx),
                shutdown: AtomicBool::new(false),
            }),
            MockTunHandles {
                os_out_tx,
                os_in_rx,
            },
        )
    }

    impl TunIo for MockTun {
        fn recv(&self) -> anyhow::Result<Vec<u8>> {
            loop {
                let packet = self
                    .os_out_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_millis(100));
                if self.shutdown.load(Ordering::SeqCst) {
                    anyhow::bail!("shutdown");
                }
                match packet {
                    Ok(packet) => return Ok(packet),
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => anyhow::bail!("closed"),
                }
            }
        }

        fn send(&self, packet: &[u8]) -> anyhow::Result<()> {
            self.os_in_tx
                .lock()
                .unwrap()
                .send(packet.to_vec())
                .map_err(|_| anyhow::anyhow!("closed"))
        }

        fn shutdown(&self) {
            self.shutdown.store(true, Ordering::SeqCst);
        }
    }

    /// 最小の IPv4 ヘッダ + ペイロードを組み立てる(チェックサムは未計算。
    /// デバイスループは検査しない)。
    fn ipv4_packet(src: Ipv4Addr, dst: Ipv4Addr, payload: &[u8]) -> Vec<u8> {
        let total_len = 20 + payload.len();
        let mut packet = vec![0u8; total_len];
        packet[0] = 0x45; // version 4, IHL 5
        packet[2..4].copy_from_slice(&(total_len as u16).to_be_bytes());
        packet[8] = 64; // TTL
        packet[9] = 17; // UDP
        packet[12..16].copy_from_slice(&src.octets());
        packet[16..20].copy_from_slice(&dst.octets());
        packet[20..].copy_from_slice(payload);
        packet
    }

    fn tcp_packet(src: Ipv4Addr, dst: Ipv4Addr, src_port: u16, dst_port: u16) -> Vec<u8> {
        tcp_packet_with_flags(src, dst, src_port, dst_port, 0)
    }

    fn tcp_packet_with_flags(
        src: Ipv4Addr,
        dst: Ipv4Addr,
        src_port: u16,
        dst_port: u16,
        flags: u8,
    ) -> Vec<u8> {
        let mut packet = ipv4_packet(src, dst, &[0; 20]);
        packet[9] = 6;
        packet[20..22].copy_from_slice(&src_port.to_be_bytes());
        packet[22..24].copy_from_slice(&dst_port.to_be_bytes());
        packet[32] = 5 << 4;
        packet[33] = flags;
        packet
    }

    #[test]
    fn invite_isolation_only_recognizes_control_tcp() {
        let member: Ipv4Addr = "10.99.0.2".parse().unwrap();
        let host: Ipv4Addr = "10.99.0.1".parse().unwrap();
        let inbound = tcp_packet(member, host, 49152, CONTROL_PORT);
        let outbound = tcp_packet(host, member, CONTROL_PORT, 49152);
        assert!(Device::is_control_packet(&inbound, true));
        assert!(Device::is_control_packet(&outbound, false));
        assert!(!Device::is_control_packet(&inbound, false));
        assert!(!Device::is_control_packet(
            &ipv4_packet(member, host, b"dns"),
            true
        ));
    }

    struct TestNode {
        device: Arc<Device>,
        tun: MockTunHandles,
        threads: Vec<JoinHandle<()>>,
    }

    impl TestNode {
        fn start(private_key: &PrivateKey) -> Self {
            Self::start_full(private_key, None, false)
        }

        fn start_with_port(private_key: &PrivateKey, port: Option<u16>) -> Self {
            Self::start_full(private_key, port, false)
        }

        fn start_full(private_key: &PrivateKey, port: Option<u16>, relay: bool) -> Self {
            let (tun, handles) = mock_tun();
            let device =
                Device::new(*private_key.as_bytes(), port, relay, tun).expect("device 起動失敗");
            let threads = vec![
                spawn_loop(&device, Device::udp_loop),
                spawn_loop(&device, Device::tun_loop),
                spawn_loop(&device, Device::timer_loop),
            ];
            Self {
                device,
                tun: handles,
                threads,
            }
        }

        fn endpoint(&self) -> SocketAddr {
            format!("127.0.0.1:{}", self.device.local_port())
                .parse()
                .unwrap()
        }

        fn stop(self) {
            self.device.shutdown();
            for thread in self.threads {
                let _ = thread.join();
            }
        }
    }

    fn spawn_loop(
        device: &Arc<Device>,
        f: impl Fn(&Arc<Device>) + Send + 'static,
    ) -> JoinHandle<()> {
        let device = Arc::clone(device);
        std::thread::spawn(move || f(&device))
    }

    fn peer_spec(
        public_key: PublicKey,
        endpoint: Option<SocketAddr>,
        allowed_ips: &[&str],
        keepalive: Option<u16>,
    ) -> PeerSpec {
        PeerSpec {
            public_key,
            endpoint,
            allowed_ips: allowed_ips.iter().map(|s| s.parse().unwrap()).collect(),
            persistent_keepalive: keepalive,
            preshared_key: None,
        }
    }

    /// 最長プレフィックス一致(ADR-0013): ホスト(/24)と直接ピア(/32)の
    /// AllowedIPs が重なったら /32 が勝ち、/32 を消せば /24(ホスト経由)へ戻る。
    #[test]
    fn find_peer_by_dst_prefers_longest_prefix() {
        let (tun, _handles) = mock_tun();
        let device =
            Device::new(*PrivateKey::generate().as_bytes(), None, false, tun).expect("device");
        let host_key = PrivateKey::generate().public_key();
        let direct_key = PrivateKey::generate().public_key();
        device
            .add_peer(&peer_spec(host_key, None, &["10.99.0.0/24"], None))
            .unwrap();
        device
            .add_peer(&peer_spec(direct_key, None, &["10.99.0.3/32"], None))
            .unwrap();

        let dst: Ipv4Addr = "10.99.0.3".parse().unwrap();
        let picked = device.find_peer_by_dst(dst).expect("/32 に一致する");
        assert_eq!(
            picked.public_key,
            *direct_key.as_bytes(),
            "/32 が /24 に勝つ"
        );

        let other = device
            .find_peer_by_dst("10.99.0.4".parse().unwrap())
            .expect("/24 に一致する");
        assert_eq!(other.public_key, *host_key.as_bytes());
        assert!(
            device
                .find_peer_by_dst("10.98.0.1".parse().unwrap())
                .is_none(),
            "どの AllowedIPs にも入らない宛先は不一致"
        );

        // 直接ピアを消せばホストへ戻る(直接通信フォールバックの前提)
        device.remove_peer(direct_key.as_bytes());
        let fallback = device.find_peer_by_dst(dst).expect("/24 が引き継ぐ");
        assert_eq!(fallback.public_key, *host_key.as_bytes());
        device.shutdown();
    }

    /// add_peer の upsert(ADR-0019): 既存ピアの AllowedIPs をその場で更新し、
    /// ピアオブジェクト(= Tunn セッション)は作り直さない。直接通信の
    /// 二段階 AllowedIPs(プローブは空 → 確立で /32)の土台。
    #[test]
    fn add_peer_upsert_updates_allowed_ips_in_place() {
        let (tun, _handles) = mock_tun();
        let device =
            Device::new(*PrivateKey::generate().as_bytes(), None, false, tun).expect("device");
        let key = PrivateKey::generate().public_key();
        let dst: Ipv4Addr = "10.99.0.3".parse().unwrap();

        // プローブ(AllowedIPs 空): どの宛先にも一致しない = 経路を奪わない
        device.add_peer(&peer_spec(key, None, &[], None)).unwrap();
        assert!(device.find_peer_by_dst(dst).is_none(), "プローブは経路なし");
        let probe = &device.peers()[0];
        assert!(probe.allowed_ips().is_empty());

        // 確立: /32 を付与 → 経路がこのピアへ切り替わる。同じオブジェクトの
        // まま(セッション維持)で、テーブルにも増えない
        device
            .add_peer(&peer_spec(key, None, &["10.99.0.3/32"], None))
            .unwrap();
        let peers = device.peers();
        assert_eq!(peers.len(), 1, "upsert でピアは増えない");
        assert!(
            Arc::ptr_eq(probe, &peers[0]),
            "既存の DevicePeer(セッション)を維持する"
        );
        let picked = device.find_peer_by_dst(dst).expect("/32 に一致する");
        assert_eq!(picked.public_key, *key.as_bytes());
        device.shutdown();
    }

    /// 送信パケットが相手の TUN から出てくるまで待つ。
    fn expect_via_tunnel(from: &TestNode, to: &TestNode, packet: Vec<u8>, what: &str) -> Vec<u8> {
        let deadline = Instant::now() + Duration::from_secs(10);
        // ハンドシェイク完了前の送信は boringtun がキューするので、1 度だけ送って待つ
        from.tun.os_out_tx.send(packet).unwrap();
        loop {
            match to.tun.os_in_rx.recv_timeout(Duration::from_millis(500)) {
                Ok(received) => return received,
                Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() < deadline => continue,
                Err(e) => panic!("{what} がトンネルを通りませんでした: {e}"),
            }
        }
    }

    #[test]
    fn loopback_host_and_member_exchange_packets() {
        let host_ip: Ipv4Addr = "100.100.42.1".parse().unwrap();
        let member_ip: Ipv4Addr = "100.100.42.2".parse().unwrap();
        let host_key = PrivateKey::generate();
        let member_key = PrivateKey::generate();

        // host: listen ポートは OS 任せ(local_port で取得)。メンバーの endpoint は不明
        let host = TestNode::start(&host_key);
        host.device
            .add_peer(&peer_spec(
                member_key.public_key(),
                None,
                &["100.100.42.2/32"],
                None,
            ))
            .unwrap();

        // member: host の endpoint を指定し、即ハンドシェイク開始
        let member = TestNode::start(&member_key);
        member
            .device
            .add_peer(&peer_spec(
                host_key.public_key(),
                Some(host.endpoint()),
                &["100.100.42.0/24"],
                Some(25),
            ))
            .unwrap();

        // member -> host(member 発の初回データがハンドシェイクを完了させる)
        let m2h = ipv4_packet(member_ip, host_ip, b"member to host");
        let received = expect_via_tunnel(&member, &host, m2h.clone(), "member->host パケット");
        assert_eq!(received, m2h);

        // host -> member(host は学習したエンドポイントへ返す)
        let h2m = ipv4_packet(host_ip, member_ip, b"host to member");
        let received = expect_via_tunnel(&host, &member, h2m.clone(), "host->member パケット");
        assert_eq!(received, h2m);

        // 統計が増えていること(G-2 の status 相当)
        let host_stats = host.device.peers();
        let (since, tx, rx) = host_stats[0].stats();
        assert!(since.is_some(), "host にハンドシェイクが記録されていない");
        assert!(rx > 0, "host の rx が 0");
        assert!(tx > 0, "host の tx が 0");

        // AllowedIPs 外の送信元は host の TUN に出てこないこと
        let spoofed = ipv4_packet("100.100.42.99".parse().unwrap(), host_ip, b"spoofed");
        member.tun.os_out_tx.send(spoofed).unwrap();
        // 少し待って、届いていないことを確認
        std::thread::sleep(Duration::from_millis(600));
        assert!(
            host.tun.os_in_rx.try_recv().is_err(),
            "AllowedIPs 外の送信元のパケットが通過した"
        );

        host.stop();
        member.stop();
    }

    /// ハブ&スポーク: Member A ↔ Member B が Host のデバイス内リレー経由で
    /// 疎通する(A・B 間に直接のピア設定はない)。G-3 に対応。
    #[test]
    fn hub_and_spoke_relays_between_members() {
        let a_ip: Ipv4Addr = "100.100.42.2".parse().unwrap();
        let b_ip: Ipv4Addr = "100.100.42.3".parse().unwrap();
        let host_key = PrivateKey::generate();
        let a_key = PrivateKey::generate();
        let b_key = PrivateKey::generate();

        // host はリレー有効(ホスト役)
        let host = TestNode::start_full(&host_key, None, true);
        host.device
            .add_peer(&peer_spec(
                a_key.public_key(),
                None,
                &["100.100.42.2/32"],
                None,
            ))
            .unwrap();
        host.device
            .add_peer(&peer_spec(
                b_key.public_key(),
                None,
                &["100.100.42.3/32"],
                None,
            ))
            .unwrap();

        let member_peer = |endpoint| {
            peer_spec(
                host_key.public_key(),
                Some(endpoint),
                &["100.100.42.0/24"],
                Some(25),
            )
        };
        let a = TestNode::start(&a_key);
        a.device.add_peer(&member_peer(host.endpoint())).unwrap();
        let b = TestNode::start(&b_key);
        b.device.add_peer(&member_peer(host.endpoint())).unwrap();

        // A -> B(Host 経由。B のセッションが未確立でもキュー・再送で届く)
        let a2b = ipv4_packet(a_ip, b_ip, b"a to b via host");
        let received = expect_via_tunnel(&a, &b, a2b.clone(), "A->B リレーパケット");
        assert_eq!(received, a2b);

        // B -> A(逆方向)
        let b2a = ipv4_packet(b_ip, a_ip, b"b to a via host");
        let received = expect_via_tunnel(&b, &a, b2a.clone(), "B->A リレーパケット");
        assert_eq!(received, b2a);

        // リレーしたパケットは host の TUN(OS)には現れない
        assert!(
            host.tun.os_in_rx.try_recv().is_err(),
            "リレー対象パケットが host の TUN に漏れた"
        );

        host.stop();
        a.stop();
        b.stop();
    }

    /// ACL(ADR-0018): 遮断組のリレーは破棄され、解除すれば再び通る。
    /// ホスト宛の通信は影響を受けない。
    #[test]
    fn acl_blocks_relay_between_denied_members() {
        let host_ip: Ipv4Addr = "100.100.42.1".parse().unwrap();
        let a_ip: Ipv4Addr = "100.100.42.2".parse().unwrap();
        let b_ip: Ipv4Addr = "100.100.42.3".parse().unwrap();
        let host_key = PrivateKey::generate();
        let a_key = PrivateKey::generate();
        let b_key = PrivateKey::generate();

        let host = TestNode::start_full(&host_key, None, true);
        host.device
            .add_peer(&peer_spec(
                a_key.public_key(),
                None,
                &["100.100.42.2/32"],
                None,
            ))
            .unwrap();
        host.device
            .add_peer(&peer_spec(
                b_key.public_key(),
                None,
                &["100.100.42.3/32"],
                None,
            ))
            .unwrap();
        let member_peer = |endpoint| {
            peer_spec(
                host_key.public_key(),
                Some(endpoint),
                &["100.100.42.0/24"],
                Some(25),
            )
        };
        let a = TestNode::start(&a_key);
        a.device.add_peer(&member_peer(host.endpoint())).unwrap();
        let b = TestNode::start(&b_key);
        b.device.add_peer(&member_peer(host.endpoint())).unwrap();

        // まず疎通を確認(両セッションを確立させる)
        let before = ipv4_packet(a_ip, b_ip, b"before acl");
        expect_via_tunnel(&a, &b, before, "遮断前の A->B パケット");

        let deny = peercove_core::acl::ResolvedRule {
            id: "test-deny".into(),
            action: peercove_core::acl::AclAction::Deny,
            source: vec![ipnet::Ipv4Net::new(a_ip, 32).unwrap()],
            destination: vec![ipnet::Ipv4Net::new(b_ip, 32).unwrap()],
            protocol: peercove_core::acl::AclProtocol::Any,
            ports: vec![],
        };
        host.device.set_acl(peercove_core::acl::AclPolicy {
            default: peercove_core::acl::AclAction::Allow,
            rules: vec![deny],
        });
        let blocked = ipv4_packet(a_ip, b_ip, b"blocked");
        a.tun.os_out_tx.send(blocked).unwrap();
        std::thread::sleep(Duration::from_millis(600));
        assert!(
            b.tun.os_in_rx.try_recv().is_err(),
            "遮断中のパケットが B に届いた"
        );
        assert!(
            host.tun.os_in_rx.try_recv().is_err(),
            "遮断したパケットが host の TUN に漏れた"
        );

        // 逆方向 B -> A から開始したセッションは、A -> B が deny でも
        // 応答パケットだけ通る。これにより方向付き ACL が接続開始方向として機能する。
        let b_syn = tcp_packet_with_flags(b_ip, a_ip, 50_000, 443, 0x02);
        let received = expect_via_tunnel(&b, &a, b_syn.clone(), "許可方向 B->A の SYN");
        assert_eq!(received, b_syn);
        let a_syn_ack = tcp_packet_with_flags(a_ip, b_ip, 443, 50_000, 0x12);
        let received =
            expect_via_tunnel(&a, &b, a_syn_ack.clone(), "deny 方向 A->B の確立済み応答");
        assert_eq!(received, a_syn_ack);

        // ホスト宛の通信は影響を受けない
        let a2h = ipv4_packet(a_ip, host_ip, b"a to host");
        let received = expect_via_tunnel(&a, &host, a2h.clone(), "遮断中の A->host パケット");
        assert_eq!(received, a2h);

        // 解除すれば再び通る
        host.device.set_acl(peercove_core::acl::AclPolicy {
            default: peercove_core::acl::AclAction::Allow,
            rules: vec![],
        });
        let after = ipv4_packet(a_ip, b_ip, b"after acl");
        let received = expect_via_tunnel(&a, &b, after.clone(), "解除後の A->B パケット");
        assert_eq!(received, after);

        host.stop();
        a.stop();
        b.stop();
    }

    /// remove_peer 後はそのピアのパケットが一切通らなくなる(M1-G3)。
    #[test]
    fn removed_peer_traffic_is_dropped() {
        let host_ip: Ipv4Addr = "100.100.42.1".parse().unwrap();
        let member_ip: Ipv4Addr = "100.100.42.2".parse().unwrap();
        let host_key = PrivateKey::generate();
        let member_key = PrivateKey::generate();

        let host = TestNode::start(&host_key);
        host.device
            .add_peer(&peer_spec(
                member_key.public_key(),
                None,
                &["100.100.42.2/32"],
                None,
            ))
            .unwrap();
        let member = TestNode::start(&member_key);
        member
            .device
            .add_peer(&peer_spec(
                host_key.public_key(),
                Some(host.endpoint()),
                &["100.100.42.0/24"],
                Some(25),
            ))
            .unwrap();

        // 疎通確立
        let packet = ipv4_packet(member_ip, host_ip, b"before removal");
        expect_via_tunnel(&member, &host, packet, "削除前のパケット");

        // 削除 → テーブルから消える
        host.device.remove_peer(member_key.public_key().as_bytes());
        assert!(host.device.peers().is_empty());

        // 以後のパケットは host の TUN に届かない
        let packet = ipv4_packet(member_ip, host_ip, b"after removal");
        member.tun.os_out_tx.send(packet).unwrap();
        std::thread::sleep(Duration::from_secs(2));
        assert!(
            host.tun.os_in_rx.try_recv().is_err(),
            "削除済みピアのパケットが通過した"
        );

        host.stop();
        member.stop();
    }

    /// ホスト再起動シナリオ: メンバーは古いセッションで送信を続けるが、
    /// データ無応答の検知(約 15 秒)で自動的に再ハンドシェイクして復帰する。
    /// G-2 実機検証で観測した NoCurrentSession の回復経路を担保する。
    #[test]
    fn member_recovers_after_host_restart() {
        let host_ip: Ipv4Addr = "100.100.42.1".parse().unwrap();
        let member_ip: Ipv4Addr = "100.100.42.2".parse().unwrap();
        let host_key = PrivateKey::generate();
        let member_key = PrivateKey::generate();
        let member_peer = || peer_spec(member_key.public_key(), None, &["100.100.42.2/32"], None);

        let host1 = TestNode::start(&host_key);
        host1.device.add_peer(&member_peer()).unwrap();
        let port = host1.device.local_port();

        let member = TestNode::start(&member_key);
        member
            .device
            .add_peer(&peer_spec(
                host_key.public_key(),
                Some(host1.endpoint()),
                &["100.100.42.0/24"],
                Some(25),
            ))
            .unwrap();

        // 初回の疎通を確立
        let m2h = ipv4_packet(member_ip, host_ip, b"before restart");
        expect_via_tunnel(&member, &host1, m2h, "再起動前の member->host パケット");

        // ホストを停止し、同じポートで新プロセス相当を起動(セッション消失)
        host1.stop();
        let host2 = TestNode::start_with_port(&host_key, Some(port));
        host2.device.add_peer(&member_peer()).unwrap();

        // メンバーは古いセッションのまま送信し続ける → 自動再ハンドシェイクで復帰
        let deadline = Instant::now() + Duration::from_secs(45);
        let payload = ipv4_packet(member_ip, host_ip, b"after restart");
        let recovered = loop {
            member.tun.os_out_tx.send(payload.clone()).unwrap();
            match host2.tun.os_in_rx.recv_timeout(Duration::from_secs(1)) {
                Ok(received) => break received,
                Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() < deadline => continue,
                Err(e) => panic!("ホスト再起動後に復帰しませんでした: {e}"),
            }
        };
        assert_eq!(recovered, payload);

        host2.stop();
        member.stop();
    }
}
