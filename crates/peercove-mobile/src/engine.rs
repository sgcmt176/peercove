//! メンバー専用の WG ユーザースペースエンジン(M4 E-B、ADR-0040)。
//!
//! デスクトップの Windows デバイス(peercove-cli backend/windows/device.rs)と
//! 同じ boringtun 0.7 の `Tunn` を使うが、こちらは**メンバー専用・単一ピア
//! (ホスト)**なので大幅に単純: ピアテーブル・リレー・roaming 学習を持たない。
//!
//! スレッド構成(device.rs と同型):
//! - TUN 読み: 平文 IP パケット → encapsulate → UDP でホストへ
//! - UDP 読み: ホストからの WG データグラム → decapsulate → TUN へ書く
//! - タイマー: 250ms ごとに `update_timers`(再送・keepalive・鍵更新)+ 統計更新
//!
//! TUN の実体は [`TunIo`] trait の背後(Android では VpnService の fd、
//! テストではチャネル)。device.rs の TunIo と同じ発想でループバックテストする。

use std::io;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use boringtun::noise::errors::WireGuardError;
use boringtun::noise::{Tunn, TunnResult};
use boringtun::x25519;
use ipnet::Ipv4Net;

/// パケットバッファ。MTU(既定 1420)+ WG ヘッダに十分(device.rs と同値)。
const BUF_SIZE: usize = 2048;
/// 読み待ちのタイムアウト = 停止フラグの確認間隔 = タイマー周期
const TICK: Duration = Duration::from_millis(250);

/// トンネル 1 本の設定(member.toml から組み立てる)。
pub struct EngineSpec {
    pub private_key: [u8; 32],
    pub peer_public_key: [u8; 32],
    pub preshared_key: Option<[u8; 32]>,
    /// ホストのエンドポイント候補(先頭から順に試す。M4 E-C: LAN → 外部 IP の
    /// フォールバック。member.toml の endpoint + endpoint_fallbacks)
    pub endpoints: Vec<SocketAddr>,
    /// ハンドシェイクが確立しないとき次の候補へ切り替えるまでの時間
    pub rotate_after: Duration,
    /// ホスト側 AllowedIPs(通常はネットワークのサブネット全体)
    pub allowed_ips: Vec<Ipv4Net>,
    pub persistent_keepalive: Option<u16>,
}

/// TUN 入出力の抽象。Android は VpnService の fd、テストはチャネル。
/// `read` は `timeout` まで待って、データが無ければ `Ok(0)` を返す
/// (停止フラグを確認できるように)。
pub trait TunIo: Send + Sync {
    fn read(&self, buf: &mut [u8], timeout: Duration) -> io::Result<usize>;
    fn write(&self, buf: &[u8]) -> io::Result<()>;
}

/// 稼働中トンネルの統計(タイマースレッドが更新、UI が読む)。
#[derive(Default, Clone)]
pub struct EngineStats {
    /// 最終ハンドシェイクからの経過秒(None = 未確立)
    pub handshake_age_secs: Option<u64>,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    /// いま使っているエンドポイント(フォールバック切替の確認用)
    pub current_endpoint: Option<SocketAddr>,
}

struct Shared {
    tunn: Mutex<Tunn>,
    /// UDP ソケット。Android の回線切替(Wi-Fi ↔ モバイル)で張り直す
    /// (rebind)ため差し替え可能にする。使う側は毎回 `socket()` で取る
    udp: RwLock<Arc<UdpSocket>>,
    tun: Arc<dyn TunIo>,
    allowed_ips: Vec<Ipv4Net>,
    /// エンドポイント候補と現在使用中の候補(タイマーがローテートする)
    endpoints: Vec<SocketAddr>,
    current_peer: Mutex<SocketAddr>,
    rotate_after: Duration,
    /// 最後に rebind(回線切替)した時刻。それ以降にハンドシェイクが
    /// 成立するまで、途絶判定(180 秒)を待たずにローテーションを回す
    rebound_at: Mutex<Option<Instant>>,
    stats: Mutex<EngineStats>,
    stop: AtomicBool,
}

impl Shared {
    fn socket(&self) -> Arc<UdpSocket> {
        Arc::clone(&self.udp.read().unwrap())
    }
}

pub struct Engine {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

impl Engine {
    /// トンネルを開始する。`udp` は bind 済み(かつ Android では protect 済み)の
    /// ソケットを受け取る(テストで自由にポートを選べるように)。
    pub fn start(spec: EngineSpec, tun: Arc<dyn TunIo>, udp: UdpSocket) -> anyhow::Result<Engine> {
        let first = *spec
            .endpoints
            .first()
            .ok_or_else(|| anyhow::anyhow!("エンドポイント候補がありません"))?;
        udp.connect(first)?;
        udp.set_read_timeout(Some(TICK))?;

        let tunn = Tunn::new(
            x25519::StaticSecret::from(spec.private_key),
            x25519::PublicKey::from(spec.peer_public_key),
            spec.preshared_key,
            spec.persistent_keepalive,
            0, // 単一ピアなので index は固定
            None,
        );
        let shared = Arc::new(Shared {
            tunn: Mutex::new(tunn),
            udp: RwLock::new(Arc::new(udp)),
            tun,
            allowed_ips: spec.allowed_ips,
            endpoints: spec.endpoints,
            current_peer: Mutex::new(first),
            rotate_after: spec.rotate_after,
            rebound_at: Mutex::new(None),
            stats: Mutex::new(EngineStats::default()),
            stop: AtomicBool::new(false),
        });

        // メンバーは endpoint を知っているので、即ハンドシェイクを開始する
        {
            let mut buf = [0u8; BUF_SIZE];
            let mut tunn = shared.tunn.lock().unwrap();
            if let TunnResult::WriteToNetwork(data) =
                tunn.format_handshake_initiation(&mut buf, false)
            {
                let _ = shared.socket().send(data);
            }
        }

        let threads = vec![
            spawn("peercove-tun", Arc::clone(&shared), tun_loop),
            spawn("peercove-udp", Arc::clone(&shared), udp_loop),
            spawn("peercove-timer", Arc::clone(&shared), timer_loop),
        ];
        Ok(Engine { shared, threads })
    }

    pub fn stats(&self) -> EngineStats {
        let mut stats = self.shared.stats.lock().unwrap().clone();
        stats.current_endpoint = Some(*self.shared.current_peer.lock().unwrap());
        stats
    }

    /// UDP ソケットを張り直して即ハンドシェイクし直す(M4 E-D)。
    /// Android の回線切替(Wi-Fi ↔ モバイル)後は旧ソケットの経路が死んで
    /// いることがあるため、NetworkCallback から protect 済みの新ソケットを
    /// もらって差し替える。送信元ポートが変わるが、ホスト側(デスクトップ)は
    /// roaming 学習で追従する。
    pub fn rebind(&self, udp: UdpSocket) -> anyhow::Result<()> {
        let peer = *self.shared.current_peer.lock().unwrap();
        udp.connect(peer)?;
        udp.set_read_timeout(Some(TICK))?;
        let udp = Arc::new(udp);
        *self.shared.udp.write().unwrap() = Arc::clone(&udp);
        *self.shared.rebound_at.lock().unwrap() = Some(Instant::now());
        let mut buf = [0u8; BUF_SIZE];
        let mut tunn = self.shared.tunn.lock().unwrap();
        if let TunnResult::WriteToNetwork(data) = tunn.format_handshake_initiation(&mut buf, true) {
            let _ = udp.send(data);
        }
        Ok(())
    }

    /// トンネルを停止してスレッドを回収する。TUN(fd)は Arc の解放で閉じる。
    pub fn stop(self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        for t in self.threads {
            let _ = t.join();
        }
    }
}

fn spawn(name: &str, shared: Arc<Shared>, f: fn(&Shared)) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || f(&shared))
        .expect("スレッド起動に失敗")
}

/// TUN → 暗号化 → UDP(ホスト宛)
fn tun_loop(shared: &Shared) {
    let mut pkt = [0u8; BUF_SIZE];
    let mut work = [0u8; BUF_SIZE];
    while !shared.stop.load(Ordering::Relaxed) {
        let n = match shared.tun.read(&mut pkt, TICK) {
            Ok(0) => continue, // タイムアウト(データなし)
            Ok(n) => n,
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut =>
            {
                continue
            }
            Err(e) => {
                tracing::warn!("TUN 読み取りに失敗: {e}");
                break;
            }
        };
        let mut tunn = shared.tunn.lock().unwrap();
        match tunn.encapsulate(&pkt[..n], &mut work) {
            TunnResult::WriteToNetwork(data) => {
                let _ = shared.socket().send(data);
            }
            TunnResult::Done => {} // セッション未確立中はキュー(ハンドシェイクは開始済み)
            TunnResult::Err(e) => tracing::warn!("暗号化に失敗: {e:?}"),
            _ => {}
        }
    }
}

/// UDP(ホスト発)→ 復号 → TUN
fn udp_loop(shared: &Shared) {
    let mut datagram = [0u8; BUF_SIZE];
    let mut work = [0u8; BUF_SIZE];
    while !shared.stop.load(Ordering::Relaxed) {
        // フォールバックのローテートや rebind で変わりうるので毎回読む
        let peer_ip = shared.current_peer.lock().unwrap().ip();
        let udp = shared.socket();
        let n = match udp.recv(&mut datagram) {
            Ok(n) => n,
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut =>
            {
                continue
            }
            // 到達不能ポートへ送った後の ICMP(ConnectionRefused/Reset)や
            // 回線切替直後の一時エラーで受信ループを終わらせない(終わらせると
            // 以後ハンドシェイク応答を読めず永久に復帰不能 = 実機で観測)。
            // スピン防止に 1 tick 待って続行する
            Err(e) => {
                tracing::debug!("UDP 受信エラー(継続): {e}");
                std::thread::sleep(TICK);
                continue;
            }
        };
        let mut tunn = shared.tunn.lock().unwrap();
        // device.rs と同じ decapsulate ループ: WriteToNetwork(ハンドシェイク応答
        // など)は送信して空 decapsulate で送信キューを掃き出す
        let mut result = tunn.decapsulate(Some(peer_ip), &datagram[..n], &mut work);
        loop {
            match result {
                TunnResult::WriteToNetwork(data) => {
                    let _ = udp.send(data);
                    result = tunn.decapsulate(None, &[], &mut work);
                }
                TunnResult::WriteToTunnelV4(packet, src) => {
                    // 暗号鍵ルーティング: ピアの AllowedIPs 内の送信元だけ通す
                    if shared.allowed_ips.iter().any(|net| net.contains(&src)) {
                        if let Err(e) = shared.tun.write(packet) {
                            tracing::warn!("TUN 書き込みに失敗: {e}");
                        }
                    }
                    break;
                }
                TunnResult::WriteToTunnelV6(..) => break, // IPv6 は対象外
                TunnResult::Done => break,
                TunnResult::Err(WireGuardError::UnderLoad) => break,
                TunnResult::Err(e) => {
                    tracing::debug!("復号に失敗: {e:?}");
                    break;
                }
            }
        }
    }
}

/// 250ms ごとの再送・keepalive・鍵更新 + 統計の更新 + フォールバック切替
fn timer_loop(shared: &Shared) {
    let mut work = [0u8; BUF_SIZE];
    let mut endpoint_index = 0usize;
    let mut last_rotate = Instant::now();
    while !shared.stop.load(Ordering::Relaxed) {
        std::thread::sleep(TICK);
        let mut tunn = shared.tunn.lock().unwrap();
        match tunn.update_timers(&mut work) {
            TunnResult::WriteToNetwork(data) => {
                let _ = shared.socket().send(data);
            }
            TunnResult::Err(WireGuardError::ConnectionExpired) => {}
            TunnResult::Err(e) => tracing::debug!("タイマー処理でエラー: {e:?}"),
            _ => {}
        }
        let (since, tx, rx, _loss, _rtt) = tunn.stats();
        {
            let mut stats = shared.stats.lock().unwrap();
            stats.handshake_age_secs = since.map(|d| d.as_secs());
            stats.tx_bytes = tx as u64;
            stats.rx_bytes = rx as u64;
        }

        // ハンドシェイク未確立・途絶(180 秒 = WG の REJECT_AFTER_TIME)時の
        // 自己回復(M4 E-C/E-D): rotate_after ごとに再ハンドシェイクを仕掛ける。
        // 候補が複数あれば次の候補へ切り替え、1 つでも同じ相手へ強制再開する
        // (放置後に boringtun が再試行を諦めて固まったままになる事例への対策)
        let mut dead = match since {
            None => true,
            Some(age) => age > Duration::from_secs(180),
        };
        // rebind(回線切替)後は、その後にハンドシェイクが成立するまで
        // 途絶扱いにする(Wi-Fi の LAN 接続先はモバイル回線から届かないため、
        // 180 秒待たずに外部 IP 候補への切替を始める)
        {
            let mut rebound = shared.rebound_at.lock().unwrap();
            if let Some(at) = *rebound {
                match since {
                    Some(age) if age < at.elapsed() => *rebound = None, // 切替後に成立
                    _ => dead = true,
                }
            }
        }
        if dead && last_rotate.elapsed() >= shared.rotate_after {
            last_rotate = Instant::now();
            if shared.endpoints.len() > 1 {
                endpoint_index = (endpoint_index + 1) % shared.endpoints.len();
            }
            let next = shared.endpoints[endpoint_index];
            let udp = shared.socket();
            if udp.connect(next).is_ok() {
                let changed = {
                    let mut current = shared.current_peer.lock().unwrap();
                    let changed = *current != next;
                    *current = next;
                    changed
                };
                if changed {
                    tracing::info!("エンドポイントを切り替えます: {next}");
                } else {
                    tracing::debug!("再ハンドシェイクを開始します: {next}");
                }
                if let TunnResult::WriteToNetwork(data) =
                    tunn.format_handshake_initiation(&mut work, true)
                {
                    let _ = udp.send(data);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// チャネルで代用する TUN(device.rs のモックと同じ発想)
    struct ChanTun {
        rx: Mutex<mpsc::Receiver<Vec<u8>>>,
        tx: mpsc::Sender<Vec<u8>>,
    }

    impl TunIo for ChanTun {
        fn read(&self, buf: &mut [u8], timeout: Duration) -> io::Result<usize> {
            match self.rx.lock().unwrap().recv_timeout(timeout) {
                Ok(pkt) => {
                    buf[..pkt.len()].copy_from_slice(&pkt);
                    Ok(pkt.len())
                }
                Err(_) => Ok(0),
            }
        }
        fn write(&self, buf: &[u8]) -> io::Result<()> {
            let _ = self.tx.send(buf.to_vec());
            Ok(())
        }
    }

    /// 最小の IPv4 ヘッダ + ペイロード(検証用)
    fn ipv4_packet(src: [u8; 4], dst: [u8; 4], payload: &[u8]) -> Vec<u8> {
        let total = 20 + payload.len();
        let mut pkt = vec![0u8; total];
        pkt[0] = 0x45; // version 4, IHL 5
        pkt[2] = (total >> 8) as u8;
        pkt[3] = (total & 0xff) as u8;
        pkt[8] = 64; // TTL
        pkt[9] = 17; // UDP(中身は見ないので何でもよい)
        pkt[12..16].copy_from_slice(&src);
        pkt[16..20].copy_from_slice(&dst);
        pkt[20..].copy_from_slice(payload);
        pkt
    }

    fn make_engine(
        private: &x25519::StaticSecret,
        peer_public: x25519::PublicKey,
        endpoint: SocketAddr,
        udp: UdpSocket,
        allowed: &str,
    ) -> (Engine, mpsc::Sender<Vec<u8>>, mpsc::Receiver<Vec<u8>>) {
        let (in_tx, in_rx) = mpsc::channel();
        let (out_tx, out_rx) = mpsc::channel();
        let tun = Arc::new(ChanTun {
            rx: Mutex::new(in_rx),
            tx: out_tx,
        });
        let spec = EngineSpec {
            private_key: private.to_bytes(),
            peer_public_key: peer_public.to_bytes(),
            preshared_key: None,
            endpoints: vec![endpoint],
            rotate_after: Duration::from_secs(10),
            allowed_ips: vec![allowed.parse().unwrap()],
            persistent_keepalive: Some(5),
        };
        let engine = Engine::start(spec, tun, udp).unwrap();
        (engine, in_tx, out_rx)
    }

    /// 2 つのエンジンを localhost UDP で対向させ、平文パケットが暗号化されて
    /// 相手の TUN 側から出てくることを確認する(WG プロトコル一式の実通し)。
    #[test]
    fn two_engines_tunnel_packets_over_loopback() {
        let a_key = x25519::StaticSecret::random_from_rng(rand_core::OsRng);
        let b_key = x25519::StaticSecret::random_from_rng(rand_core::OsRng);
        let a_pub = x25519::PublicKey::from(&a_key);
        let b_pub = x25519::PublicKey::from(&b_key);

        let a_udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        let b_udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        let a_addr = a_udp.local_addr().unwrap();
        let b_addr = b_udp.local_addr().unwrap();

        // A から見たピアは B(逆も同様)。AllowedIPs は相手の仮想 IP
        let (a, a_in, _a_out) = make_engine(&a_key, b_pub, b_addr, a_udp, "10.99.0.2/32");
        let (b, _b_in, b_out) = make_engine(&b_key, a_pub, a_addr, b_udp, "10.99.0.1/32");

        // A の TUN へ平文を入れる → B の TUN から同じものが出てくる
        let pkt = ipv4_packet([10, 99, 0, 1], [10, 99, 0, 2], b"hello peercove");
        // ハンドシェイク完了前の送信は落ちることがあるため、何度か送る
        let mut received = None;
        for _ in 0..40 {
            a_in.send(pkt.clone()).unwrap();
            if let Ok(got) = b_out.recv_timeout(Duration::from_millis(250)) {
                received = Some(got);
                break;
            }
        }
        let received = received.expect("10 秒以内にトンネル越しにパケットが届くはず");
        assert_eq!(received, pkt);

        // ハンドシェイクが確立していれば統計にも出る(更新は 250ms 周期なので待つ)
        let established = (0..20).any(|_| {
            if a.stats().handshake_age_secs.is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
            false
        });
        assert!(established, "ハンドシェイクが統計に反映されない");

        a.stop();
        b.stop();
    }

    /// 先頭エンドポイントが死んでいるとき、次の候補へフォールバックして
    /// ハンドシェイクが確立する(M4 E-C: LAN → 外部 IP の自動切替に相当)。
    #[test]
    fn engine_falls_back_to_second_endpoint() {
        let a_key = x25519::StaticSecret::random_from_rng(rand_core::OsRng);
        let b_key = x25519::StaticSecret::random_from_rng(rand_core::OsRng);
        let a_pub = x25519::PublicKey::from(&a_key);
        let b_pub = x25519::PublicKey::from(&b_key);

        // 何も応答しないダミー(死んでいる先頭候補)
        let dead = UdpSocket::bind("127.0.0.1:0").unwrap();
        let dead_addr = dead.local_addr().unwrap();

        let a_udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        let b_udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        let a_addr = a_udp.local_addr().unwrap();
        let b_addr = b_udp.local_addr().unwrap();

        // A: [死んでいる候補, B] の順。500ms で切り替え
        let (a_in, a_rx) = mpsc::channel::<Vec<u8>>();
        let (a_out_tx, _a_out) = mpsc::channel();
        let _ = a_in; // TUN 入力は使わない
        let tun = Arc::new(ChanTun {
            rx: Mutex::new(a_rx),
            tx: a_out_tx,
        });
        let a = Engine::start(
            EngineSpec {
                private_key: a_key.to_bytes(),
                peer_public_key: b_pub.to_bytes(),
                preshared_key: None,
                endpoints: vec![dead_addr, b_addr],
                rotate_after: Duration::from_millis(500),
                allowed_ips: vec!["10.99.0.2/32".parse().unwrap()],
                persistent_keepalive: Some(5),
            },
            tun,
            a_udp,
        )
        .unwrap();
        // B は通常どおり A を向く
        let (b, _b_in, _b_out) = make_engine(&b_key, a_pub, a_addr, b_udp, "10.99.0.1/32");

        let established = (0..100).any(|_| {
            if a.stats().handshake_age_secs.is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
            false
        });
        assert!(established, "フォールバック先でハンドシェイクが確立しない");

        a.stop();
        b.stop();
    }

    /// 先頭候補が「誰も居ないポート」(ICMP 到達不能 → recv がエラーを返す)
    /// でも受信ループが死なず、次の候補へフォールバックして確立する。
    /// 回線切替後に永久に復帰しなかった実機不具合の回帰テスト。
    #[test]
    fn engine_survives_recv_errors_and_falls_back() {
        let a_key = x25519::StaticSecret::random_from_rng(rand_core::OsRng);
        let b_key = x25519::StaticSecret::random_from_rng(rand_core::OsRng);
        let a_pub = x25519::PublicKey::from(&a_key);
        let b_pub = x25519::PublicKey::from(&b_key);

        // bind してすぐ閉じたポート = 送ると ICMP 到達不能が返ってくる
        let ghost_addr = {
            let s = UdpSocket::bind("127.0.0.1:0").unwrap();
            s.local_addr().unwrap()
        };

        let a_udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        let b_udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        let a_addr = a_udp.local_addr().unwrap();
        let b_addr = b_udp.local_addr().unwrap();

        let (_a_in, a_rx) = mpsc::channel::<Vec<u8>>();
        let (a_out_tx, _a_out) = mpsc::channel();
        let tun = Arc::new(ChanTun {
            rx: Mutex::new(a_rx),
            tx: a_out_tx,
        });
        let a = Engine::start(
            EngineSpec {
                private_key: a_key.to_bytes(),
                peer_public_key: b_pub.to_bytes(),
                preshared_key: None,
                endpoints: vec![ghost_addr, b_addr],
                rotate_after: Duration::from_millis(500),
                allowed_ips: vec!["10.99.0.2/32".parse().unwrap()],
                persistent_keepalive: Some(5),
            },
            tun,
            a_udp,
        )
        .unwrap();
        let (b, _b_in, _b_out) = make_engine(&b_key, a_pub, a_addr, b_udp, "10.99.0.1/32");

        let established = (0..100).any(|_| {
            if a.stats().handshake_age_secs.is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
            false
        });
        assert!(
            established,
            "recv エラー(ICMP 到達不能)後にフォールバックで確立しない"
        );

        a.stop();
        b.stop();
    }

    /// AllowedIPs 外の送信元は TUN に流さない(暗号鍵ルーティング)
    #[test]
    fn packets_from_outside_allowed_ips_are_dropped() {
        let a_key = x25519::StaticSecret::random_from_rng(rand_core::OsRng);
        let b_key = x25519::StaticSecret::random_from_rng(rand_core::OsRng);
        let a_pub = x25519::PublicKey::from(&a_key);
        let b_pub = x25519::PublicKey::from(&b_key);

        let a_udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        let b_udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        let a_addr = a_udp.local_addr().unwrap();
        let b_addr = b_udp.local_addr().unwrap();

        let (a, a_in, _a_out) = make_engine(&a_key, b_pub, b_addr, a_udp, "10.99.0.2/32");
        // B 側は「10.99.0.9 からのみ受け付ける」= A の 10.99.0.1 は範囲外
        let (b, _b_in, b_out) = make_engine(&b_key, a_pub, a_addr, b_udp, "10.99.0.9/32");

        let pkt = ipv4_packet([10, 99, 0, 1], [10, 99, 0, 2], b"should be dropped");
        for _ in 0..12 {
            a_in.send(pkt.clone()).unwrap();
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(
            b_out.recv_timeout(Duration::from_millis(300)).is_err(),
            "AllowedIPs 外のパケットが通ってしまった"
        );

        a.stop();
        b.stop();
    }
}
