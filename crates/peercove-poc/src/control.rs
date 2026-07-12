//! トンネル内コントロールチャネル(M1-G2、ADR-0005)。
//!
//! - host: 自分の仮想 IP の TCP [`CONTROL_PORT`] で待受け、接続してきたメンバーに
//!   台帳スナップショットを配布する(接続時 + 変更時)
//! - member: ホストへ接続して Hello を名乗り、届いた台帳を保持する
//!   (status ファイル経由で `status` コマンドに表示される)
//!
//! 接続はトンネル内なので WG により暗号化・認証済み。接続元の仮想 IP が
//! そのメンバーの身元となる(M1-G3 の削除通知はこの対応表を使う)。

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use peercove_core::dns::DnsRecord;
use peercove_core::proto::{ControlMessage, LedgerEntry, CONTROL_PORT, PROTO_VERSION};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};

/// 受信 **1 行** の上限。台帳が大きくても余裕を持てるサイズ。
///
/// `AsyncReadExt::take` の上限は「その reader の累計」なので、1 行読むたびに
/// `set_limit` で戻すこと(戻し忘れると、ping/pong の積み重ねで数時間後に
/// EOF 扱いになって制御接続が落ちる)。
const MAX_LINE_LEN: u64 = 64 * 1024;
const RETRY_INTERVAL: Duration = Duration::from_secs(5);
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
/// RTT 計測 ping の間隔(M2-G5)。supervisor の周期と同じにして、
/// UI の表示が 1 周期ごとに 1 回は更新されるようにする。
const PING_INTERVAL: Duration = Duration::from_secs(5);

/// 接続中メンバー(仮想 IP → 送信キュー)。削除通知(M1-G3)で使う。
pub type Connections = Arc<Mutex<HashMap<Ipv4Addr, mpsc::UnboundedSender<ControlMessage>>>>;

/// ホストが配布する内容一式(台帳 + カスタム DNS レコード — M3-1)。
/// watch チャネルでまとめて流し、どれかが変わったら再配布される。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Distribution {
    pub members: Vec<LedgerEntry>,
    pub dns_records: Vec<DnsRecord>,
    /// ACL の遮断組(ADR-0018、M3-10。仮想 IP の正規化済みペア)。
    /// ワイヤには載せず、送信時にメンバーごとのフィルタ
    /// ([`ledger_message_for`])の材料にする。
    pub deny: Vec<(Ipv4Addr, Ipv4Addr)>,
}

/// 台帳スナップショットをメンバー向けにフィルタして Ledger メッセージにする
/// (ADR-0018、M3-10)。`member_ip` と遮断関係にある相手のエントリは
/// endpoint を落とし(直接通信をさせない)、`blocked` を立てる(UI 表示用)。
fn ledger_message_for(mut dist: Distribution, member_ip: Ipv4Addr) -> ControlMessage {
    let deny = std::mem::take(&mut dist.deny);
    for entry in &mut dist.members {
        let blocked = deny
            .iter()
            .any(|&(a, b)| (a == member_ip && b == entry.ip) || (a == entry.ip && b == member_ip));
        if blocked {
            entry.endpoint = None;
            entry.endpoint_age_secs = None;
            entry.blocked = true;
        }
    }
    ControlMessage::Ledger {
        members: dist.members,
        dns_records: dist.dns_records,
    }
}

/// メンバーが受信した配布内容 + 受信時刻。エンドポイントの鮮度判定
/// (ADR-0013: 配布時の `endpoint_age_secs` + 受信からの経過)に受信時刻が要る。
#[derive(Debug, Clone)]
pub struct ReceivedDistribution {
    pub distribution: Distribution,
    pub received_at: Instant,
}

/// 受信ログの INFO/debug 判定に使う「意味のある内容」の要約(ADR-0019)。
/// エンドポイントとその観測経過(60 秒粒度)は鮮度更新のたびに変わり、
/// 台帳は最大毎分数回再配布されるため、それ**だけ**の変化は debug に落とす。
type LedgerDigest = (
    Vec<(
        Option<String>,                 // name
        Ipv4Addr,                       // ip
        peercove_core::keys::PublicKey, // 鍵の入れ替わりも「意味のある変化」
        bool,                           // online
        bool,                           // is_host
        bool,                           // blocked(ACL、ADR-0018)
        Vec<ipnet::Ipv4Net>,            // subnets(ADR-0014)
    )>,
    Vec<DnsRecord>,
);

fn ledger_digest(members: &[LedgerEntry], dns_records: &[DnsRecord]) -> LedgerDigest {
    (
        members
            .iter()
            .map(|m| {
                (
                    m.name.clone(),
                    m.ip,
                    m.public_key,
                    m.online,
                    m.is_host,
                    m.blocked,
                    m.subnets.clone(),
                )
            })
            .collect(),
        dns_records.to_vec(),
    )
}

/// 相手の仮想 IP → 直近の RTT(ミリ秒、M2-G5)。切断時にエントリを消す。
pub type RttMap = Arc<Mutex<HashMap<Ipv4Addr, f64>>>;

fn encode_line(message: &ControlMessage) -> String {
    let mut line = serde_json::to_string(message).expect("ControlMessage は常に直列化可能");
    line.push('\n');
    line
}

/// 送信済み ping の記録。ping を打つのは書き側、Pong を見るのは読み側なので共有する。
///
/// 応答が返らないまま次の周期に入った場合、その 1 回分の測定は捨てる
/// (前回値は [`RttMap`] に残るので、UI は最後に測れた値を出し続ける)。
#[derive(Default)]
struct PingState {
    next_nonce: u64,
    outstanding: Option<(u64, Instant)>,
}

impl PingState {
    /// 次に送る ping メッセージ。送信時刻を記録する。
    fn issue(&mut self) -> ControlMessage {
        self.next_nonce += 1;
        let nonce = self.next_nonce;
        self.outstanding = Some((nonce, Instant::now()));
        ControlMessage::Ping { nonce }
    }

    /// Pong を受け取ったときの RTT(未知の nonce なら None)。
    fn observe(&mut self, nonce: u64) -> Option<f64> {
        match self.outstanding {
            Some((sent, at)) if sent == nonce => {
                self.outstanding = None;
                Some(at.elapsed().as_secs_f64() * 1000.0)
            }
            _ => None,
        }
    }
}

type SharedPing = Arc<Mutex<PingState>>;
/// 相手へ送るメッセージのキュー(削除通知・Pong の配送口)。
type Outbox = mpsc::UnboundedSender<ControlMessage>;

type LineReader = tokio::io::Take<BufReader<tokio::net::tcp::OwnedReadHalf>>;

fn line_reader(read_half: tokio::net::tcp::OwnedReadHalf) -> LineReader {
    BufReader::new(read_half).take(MAX_LINE_LEN)
}

/// 1 行読む。`None` は EOF(相手が切断)。
///
/// **`read_line` はキャンセル安全でない**(途中まで読んだバイトが失われる)。
/// そのため読み側は必ず専用タスクの素直なループで回し、`select!` の分岐には
/// 置かないこと。この関数を使う側もそれを守る前提。
async fn read_line(reader: &mut LineReader, line: &mut String) -> anyhow::Result<Option<()>> {
    reader.set_limit(MAX_LINE_LEN); // 上限は累計なので 1 行ごとに戻す
    line.clear();
    if reader.read_line(line).await? == 0 {
        return Ok(None); // 相手が切断
    }
    if !line.ends_with('\n') {
        // read_line は改行か EOF まで読む。改行が無いのは上限に達したか、行の
        // 途中で切断されたかのどちらか
        if reader.limit() == 0 {
            anyhow::bail!("1 行が {MAX_LINE_LEN} バイトを超えました");
        }
        anyhow::bail!("行の途中で切断されました");
    }
    Ok(Some(()))
}

/// 読み側の共通処理: ping には pong を返し、pong からは RTT を記録する。
/// 処理したら `true`(呼び出し側はそれ以外のメッセージを自分で捌く)。
fn handle_ping_pong(
    message: &ControlMessage,
    peer_ip: Ipv4Addr,
    out: &Outbox,
    ping: &SharedPing,
    rtt: &RttMap,
) -> bool {
    match *message {
        ControlMessage::Ping { nonce } => {
            let _ = out.send(ControlMessage::Pong { nonce });
            true
        }
        ControlMessage::Pong { nonce } => {
            if let Some(ms) = ping.lock().unwrap().observe(nonce) {
                rtt.lock().unwrap().insert(peer_ip, ms);
            }
            true
        }
        _ => false,
    }
}

/// ホスト側サーバー。台帳の変更を watch チャネルで受け取り、全接続へ配布する。
pub async fn run_host_server(
    bind_ip: Ipv4Addr,
    ledger_rx: watch::Receiver<Distribution>,
    connections: Connections,
    rtt: RttMap,
) {
    // トンネル作成直後は Windows が仮想 IP を数秒間「準備中」として扱うため、
    // bind が 10049 等で失敗する。準備が整うまで 1 秒間隔でリトライする
    let listener = loop {
        match TcpListener::bind(SocketAddr::from((bind_ip, CONTROL_PORT))).await {
            Ok(listener) => break listener,
            Err(e) => {
                tracing::debug!(
                    "コントロールチャネル起動待ち(トンネルのアドレス準備中。想定内): {e}"
                );
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    };
    tracing::info!("コントロールチャネルを {bind_ip}:{CONTROL_PORT} で待受けます");

    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let member_ip = match peer.ip() {
                    IpAddr::V4(ip) => ip,
                    IpAddr::V6(_) => continue, // M0/M1 は IPv4 のみ
                };
                let ledger_rx = ledger_rx.clone();
                let connections = Arc::clone(&connections);
                let rtt = Arc::clone(&rtt);
                tokio::spawn(async move {
                    if let Err(e) =
                        handle_member(stream, member_ip, ledger_rx, &connections, &rtt).await
                    {
                        tracing::debug!("メンバー {member_ip} との制御接続が終了: {e:#}");
                    }
                    connections.lock().unwrap().remove(&member_ip);
                    rtt.lock().unwrap().remove(&member_ip);
                    tracing::info!("メンバー {member_ip} の制御接続が切断されました");
                });
            }
            Err(e) => {
                tracing::warn!("コントロールチャネルの accept に失敗: {e}");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

/// ホスト側 1 接続。読みと書きを別タスクに分ける。
///
/// 分けるのは `read_line` がキャンセル安全でないため。書き側の `select!` に
/// 混ぜると、台帳の配布や ping のタイミングで読みかけの行が捨てられてしまう。
async fn handle_member(
    stream: TcpStream,
    member_ip: Ipv4Addr,
    mut ledger_rx: watch::Receiver<Distribution>,
    connections: &Connections,
    rtt: &RttMap,
) -> anyhow::Result<()> {
    let (read_half, write_half) = stream.into_split();
    let mut reader = line_reader(read_half);

    // 最初のメッセージは Hello(名乗り)
    let mut line = String::new();
    let hello = tokio::time::timeout(HELLO_TIMEOUT, read_line(&mut reader, &mut line))
        .await
        .map_err(|_| anyhow::anyhow!("Hello がタイムアウトしました"))??;
    if hello.is_none() {
        anyhow::bail!("Hello の前に切断されました");
    }
    match serde_json::from_str::<ControlMessage>(&line) {
        Ok(ControlMessage::Hello { version, name }) => {
            if version != PROTO_VERSION {
                tracing::warn!(
                    "メンバー {member_ip} のプロトコルバージョン {version} は未対応です\
                    (こちらは {PROTO_VERSION})"
                );
            }
            tracing::info!(
                "メンバー {member_ip}({})が接続しました",
                name.as_deref().unwrap_or("名前なし")
            );
        }
        Ok(other) => anyhow::bail!("Hello 以外のメッセージが届きました: {other:?}"),
        Err(e) => anyhow::bail!("Hello の解析に失敗しました: {e}"),
    }

    // 送信キューを登録(台帳変更・削除通知・Pong の配送口)
    let (tx, rx) = mpsc::unbounded_channel::<ControlMessage>();
    connections.lock().unwrap().insert(member_ip, tx.clone());

    // 現在の台帳を即送信。borrow_and_update で「見た」ことにして、
    // 直後の changed() が同じ内容をもう一度送るのを防ぐ
    let snapshot = ledger_rx.borrow_and_update().clone();
    let ping: SharedPing = Default::default();

    let mut writer = tokio::spawn(host_writer(
        write_half,
        member_ip,
        snapshot,
        ledger_rx,
        rx,
        Arc::clone(&ping),
    ));
    let mut read_task = tokio::spawn(host_reader(
        reader,
        member_ip,
        tx,
        ping,
        Arc::clone(rtt),
        line,
    ));

    // どちらかが終わったら接続を畳む。
    // biased: 削除通知を送り終えた書き側の Ok(()) が「意味のある終了」なので先に見る。
    // (読み側が先に終わると tx が落ちて書き側も即 ready になり、順序が非決定になる)
    let result = tokio::select! {
        biased;
        joined = &mut writer => { read_task.abort(); joined }
        joined = &mut read_task => { writer.abort(); joined }
    };
    result.unwrap_or_else(|e| Err(anyhow::anyhow!("制御タスクが異常終了しました: {e}")))
}

/// 書き側。分岐はすべてキャンセル安全なものだけにすること。
async fn host_writer(
    mut write_half: tokio::net::tcp::OwnedWriteHalf,
    member_ip: Ipv4Addr,
    initial: Distribution,
    mut ledger_rx: watch::Receiver<Distribution>,
    mut rx: mpsc::UnboundedReceiver<ControlMessage>,
    ping: SharedPing,
) -> anyhow::Result<()> {
    write_half
        .write_all(encode_line(&ledger_message_for(initial, member_ip)).as_bytes())
        .await?;

    let mut ping_tick = tokio::time::interval(PING_INTERVAL);
    loop {
        tokio::select! {
            changed = ledger_rx.changed() => {
                if changed.is_err() {
                    return Ok(()); // 送信側(supervisor)終了
                }
                let snapshot = ledger_rx.borrow_and_update().clone();
                write_half
                    .write_all(encode_line(&ledger_message_for(snapshot, member_ip)).as_bytes())
                    .await?;
            }
            queued = rx.recv() => {
                match queued {
                    Some(message) => {
                        let is_removed = matches!(message, ControlMessage::Removed { .. });
                        write_half.write_all(encode_line(&message).as_bytes()).await?;
                        if is_removed {
                            write_half.flush().await?;
                            return Ok(()); // 削除通知後は切断
                        }
                    }
                    None => return Ok(()),
                }
            }
            _ = ping_tick.tick() => {
                let message = ping.lock().unwrap().issue();
                write_half.write_all(encode_line(&message).as_bytes()).await?;
            }
        }
    }
}

/// 読み側。`select!` を使わない素直なループ(read_line のキャンセル安全性のため)。
async fn host_reader(
    mut reader: LineReader,
    member_ip: Ipv4Addr,
    out: Outbox,
    ping: SharedPing,
    rtt: RttMap,
    mut line: String,
) -> anyhow::Result<()> {
    loop {
        if read_line(&mut reader, &mut line).await?.is_none() {
            return Ok(()); // メンバー側が切断
        }
        match serde_json::from_str::<ControlMessage>(&line) {
            Ok(message) => {
                // Hello 以降にメンバーから届くのは ping/pong だけ(将来拡張用に無視)
                if !handle_ping_pong(&message, member_ip, &out, &ping, &rtt) {
                    tracing::debug!("メンバー {member_ip} から: {}", line.trim_end());
                }
            }
            Err(e) => tracing::debug!("解析できないメッセージを無視: {e}"),
        }
    }
}

/// メンバー側クライアント。台帳を受信して `latest_ledger` に反映する。
/// 切断されたら自動で再接続する。
///
/// ホストから削除された(`Removed`)ら `removed` を立てて**再接続をやめる**。
/// 削除後はホストが WG ピアも消すので再接続は成功しないうえ、UI に「削除された」
/// と出したまま無駄なリトライを続けないため(M2-G6 のフィードバック)。
pub async fn run_member_client(
    host_ip: Ipv4Addr,
    display_name: Option<String>,
    latest_ledger: Arc<Mutex<Option<ReceivedDistribution>>>,
    rtt: RttMap,
    removed: Arc<AtomicBool>,
) {
    let target = SocketAddr::from((host_ip, CONTROL_PORT));
    let mut logged_wait = false;
    loop {
        match TcpStream::connect(target).await {
            Ok(stream) => {
                logged_wait = false;
                tracing::info!("コントロールチャネルに接続しました({target})");
                let session = member_session(
                    stream,
                    &display_name,
                    &latest_ledger,
                    host_ip,
                    &rtt,
                    &removed,
                );
                if let Err(e) = session.await {
                    tracing::debug!("制御接続が終了しました(再接続します): {e:#}");
                }
                rtt.lock().unwrap().remove(&host_ip);
                if removed.load(Ordering::Relaxed) {
                    tracing::info!("削除通知を受けたので再接続を停止します");
                    return;
                }
            }
            Err(e) => {
                // トンネル確立前は失敗して当然なので、最初の 1 回だけ案内する
                if !logged_wait {
                    tracing::info!("コントロールチャネル接続待ち(トンネル確立後に自動接続): {e}");
                    logged_wait = true;
                }
            }
        }
        tokio::time::sleep(RETRY_INTERVAL).await;
    }
}

/// メンバー側 1 接続。ホスト側と同じく読みと書きを分ける。
async fn member_session(
    stream: TcpStream,
    display_name: &Option<String>,
    latest_ledger: &Arc<Mutex<Option<ReceivedDistribution>>>,
    host_ip: Ipv4Addr,
    rtt: &RttMap,
    removed: &Arc<AtomicBool>,
) -> anyhow::Result<()> {
    let (read_half, write_half) = stream.into_split();
    let (tx, rx) = mpsc::unbounded_channel::<ControlMessage>();
    tx.send(ControlMessage::Hello {
        version: PROTO_VERSION,
        name: display_name.clone(),
    })
    .expect("受信側はこの後 spawn する");

    let ping: SharedPing = Default::default();
    let mut writer = tokio::spawn(member_writer(write_half, rx, Arc::clone(&ping)));
    let mut read_task = tokio::spawn(member_reader(
        line_reader(read_half),
        host_ip,
        tx,
        ping,
        Arc::clone(rtt),
        Arc::clone(latest_ledger),
        Arc::clone(removed),
    ));

    // biased: 削除通知(Removed)を検知する読み側が「意味のある終了」なので先に見る。
    // (読み側が終わると Outbox が落ちて書き側も即 ready になり、順序が非決定になる)
    let result = tokio::select! {
        biased;
        joined = &mut read_task => { writer.abort(); joined }
        joined = &mut writer => { read_task.abort(); joined }
    };
    result.unwrap_or_else(|e| Err(anyhow::anyhow!("制御タスクが異常終了しました: {e}")))
}

/// 書き側。キューされたメッセージ(Hello / Pong)と定期 ping を送る。
async fn member_writer(
    mut write_half: tokio::net::tcp::OwnedWriteHalf,
    mut rx: mpsc::UnboundedReceiver<ControlMessage>,
    ping: SharedPing,
) -> anyhow::Result<()> {
    let mut ping_tick = tokio::time::interval(PING_INTERVAL);
    loop {
        tokio::select! {
            queued = rx.recv() => {
                match queued {
                    Some(message) => write_half.write_all(encode_line(&message).as_bytes()).await?,
                    None => return Ok(()), // 読み側が終了
                }
            }
            _ = ping_tick.tick() => {
                let message = ping.lock().unwrap().issue();
                write_half.write_all(encode_line(&message).as_bytes()).await?;
            }
        }
    }
}

/// 読み側。`select!` を使わない素直なループ(read_line のキャンセル安全性のため)。
async fn member_reader(
    mut reader: LineReader,
    host_ip: Ipv4Addr,
    out: Outbox,
    ping: SharedPing,
    rtt: RttMap,
    latest_ledger: Arc<Mutex<Option<ReceivedDistribution>>>,
    removed: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    let mut line = String::new();
    let mut last_digest: Option<LedgerDigest> = None;
    loop {
        if read_line(&mut reader, &mut line).await?.is_none() {
            anyhow::bail!("ホストが切断しました");
        }
        match serde_json::from_str::<ControlMessage>(&line) {
            Ok(message) if handle_ping_pong(&message, host_ip, &out, &ping, &rtt) => {}
            Ok(ControlMessage::Ledger {
                members,
                dns_records,
            }) => {
                // 意味のある変化があったときだけ INFO(エンドポイント鮮度の
                // 定期更新による再配布は debug に落とす — ADR-0019)
                let digest = ledger_digest(&members, &dns_records);
                if last_digest.as_ref() != Some(&digest) {
                    tracing::info!(
                        "台帳を受信しました({} 名、DNS レコード {} 件)",
                        members.len(),
                        dns_records.len()
                    );
                    last_digest = Some(digest);
                } else {
                    tracing::debug!("台帳を受信しました({} 名、内容の変化なし)", members.len());
                }
                *latest_ledger.lock().unwrap() = Some(ReceivedDistribution {
                    distribution: Distribution {
                        members,
                        dns_records,
                        deny: vec![], // deny はワイヤに載らない(blocked で受ける)
                    },
                    received_at: Instant::now(),
                });
            }
            Ok(ControlMessage::Removed { message }) => {
                tracing::warn!("ホストから削除されました: {message}");
                // 台帳はクリアし、削除フラグを立てる(UI が「削除された」と表示する)
                *latest_ledger.lock().unwrap() = None;
                removed.store(true, Ordering::Relaxed);
                anyhow::bail!("削除通知を受信");
            }
            Ok(other) => tracing::debug!("未処理のメッセージ: {other:?}"),
            Err(e) => tracing::debug!("解析できないメッセージを無視: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peercove_core::keys::PrivateKey;

    fn entry(name: &str, ip: &str, online: bool) -> LedgerEntry {
        LedgerEntry {
            name: Some(name.to_string()),
            ip: ip.parse().unwrap(),
            public_key: PrivateKey::generate().public_key(),
            online,
            is_host: false,
            endpoint: None,
            endpoint_age_secs: None,
            subnets: vec![],
            blocked: false,
        }
    }

    /// 受信ログの INFO/debug 判定(ADR-0019): エンドポイントとその観測経過
    /// **だけ**の変化はダイジェスト一致(= debug)、メンバーの増減・オンライン・
    /// 遮断・DNS の変化は不一致(= INFO)。
    #[test]
    fn ledger_digest_ignores_endpoint_freshness_only() {
        let base = vec![entry("alice", "100.100.42.2", true)];
        let mut fresher = base.clone();
        fresher[0].endpoint = Some("203.0.113.9:51820".parse().unwrap());
        fresher[0].endpoint_age_secs = Some(60);
        assert_eq!(
            ledger_digest(&base, &[]),
            ledger_digest(&fresher, &[]),
            "エンドポイント鮮度だけの変化は意味のある変化ではない"
        );

        let mut offline = base.clone();
        offline[0].online = false;
        assert_ne!(ledger_digest(&base, &[]), ledger_digest(&offline, &[]));

        let mut blocked = base.clone();
        blocked[0].blocked = true;
        assert_ne!(ledger_digest(&base, &[]), ledger_digest(&blocked, &[]));

        let more = vec![base[0].clone(), entry("bob", "100.100.42.3", true)];
        assert_ne!(ledger_digest(&base, &[]), ledger_digest(&more, &[]));
    }

    /// host サーバー ↔ member クライアントを localhost で対向させ、
    /// 台帳の初回配布と変更配布がクライアントに反映されることを確認する。
    #[tokio::test]
    async fn ledger_is_distributed_and_updated() {
        // 127.0.0.1 で CONTROL_PORT が使用中でもテストが落ちないよう、
        // サーバー本体ではなく handle_member を直接テストする
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (ledger_tx, ledger_rx) = watch::channel(Distribution {
            members: vec![entry("alice", "100.100.42.2", true)],
            dns_records: vec![],
            deny: vec![],
        });
        let connections: Connections = Arc::new(Mutex::new(HashMap::new()));
        let host_rtt: RttMap = Default::default();

        let server_connections = Arc::clone(&connections);
        let server_rtt = Arc::clone(&host_rtt);
        tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.unwrap();
            let ip = match peer.ip() {
                IpAddr::V4(ip) => ip,
                _ => unreachable!(),
            };
            let _ = handle_member(stream, ip, ledger_rx, &server_connections, &server_rtt).await;
        });

        let latest: Arc<Mutex<Option<ReceivedDistribution>>> = Arc::new(Mutex::new(None));
        let client_latest = Arc::clone(&latest);
        let member_rtt: RttMap = Default::default();
        let client_rtt = Arc::clone(&member_rtt);
        let client = tokio::spawn(async move {
            let stream = TcpStream::connect(addr).await.unwrap();
            let host_ip: Ipv4Addr = "127.0.0.1".parse().unwrap();
            let _ = member_session(
                stream,
                &Some("alice".to_string()),
                &client_latest,
                host_ip,
                &client_rtt,
                &Arc::new(AtomicBool::new(false)),
            )
            .await;
        });

        // 初回スナップショットを受信
        let members_len =
            |l: &Option<ReceivedDistribution>| l.as_ref().map(|r| r.distribution.members.len());
        wait_for(&latest, |l| members_len(l) == Some(1)).await;

        // 台帳 + DNS レコードの変更が配布される
        ledger_tx
            .send(Distribution {
                members: vec![
                    entry("alice", "100.100.42.2", true),
                    entry("bob", "100.100.42.3", false),
                ],
                dns_records: vec![DnsRecord {
                    name: "nas".to_string(),
                    ip: "100.100.42.50".parse().unwrap(),
                }],
                deny: vec![],
            })
            .unwrap();
        wait_for(&latest, |l| members_len(l) == Some(2)).await;
        {
            let ledger = latest.lock().unwrap();
            let received = ledger.as_ref().unwrap();
            assert!(
                received.received_at.elapsed() < Duration::from_secs(10),
                "受信時刻が付く(エンドポイントの鮮度判定用)"
            );
            let dist = &received.distribution;
            assert_eq!(dist.members[1].name.as_deref(), Some("bob"));
            assert!(!dist.members[1].online);
            assert_eq!(dist.dns_records.len(), 1, "DNS レコードも一緒に届く");
            assert_eq!(dist.dns_records[0].name, "nas");
        }

        // 接続レジストリに登録されている(削除通知の配送口)
        assert_eq!(connections.lock().unwrap().len(), 1);

        // ping/pong は双方向。ホストはメンバーへの、メンバーはホストへの RTT を持つ
        let loopback: Ipv4Addr = "127.0.0.1".parse().unwrap();
        for (label, map) in [("host", &host_rtt), ("member", &member_rtt)] {
            let measured = wait_until(|| map.lock().unwrap().get(&loopback).copied()).await;
            assert!(
                measured.is_finite() && measured >= 0.0,
                "{label} 側の RTT が測れる: {measured}"
            );
        }
        client.abort();
    }

    /// ACL(ADR-0018): 台帳はメンバーごとにフィルタされ、遮断相手の
    /// エントリは endpoint が消え blocked が立つ。他のエントリは素通し。
    #[test]
    fn ledger_message_filters_blocked_entries_per_member() {
        let member_ip: Ipv4Addr = "100.100.42.5".parse().unwrap();
        let mut blocked_peer = entry("alice", "100.100.42.2", true);
        blocked_peer.endpoint = Some("203.0.113.9:51820".parse().unwrap());
        blocked_peer.endpoint_age_secs = Some(3);
        let mut open_peer = entry("bob", "100.100.42.3", true);
        open_peer.endpoint = Some("203.0.113.10:51820".parse().unwrap());
        let dist = Distribution {
            members: vec![blocked_peer, open_peer],
            dns_records: vec![],
            deny: vec![("100.100.42.2".parse().unwrap(), member_ip)],
        };

        let ControlMessage::Ledger { members, .. } = ledger_message_for(dist.clone(), member_ip)
        else {
            panic!("Ledger メッセージになる");
        };
        assert!(members[0].blocked, "遮断相手は blocked が立つ");
        assert_eq!(members[0].endpoint, None, "endpoint は配布されない");
        assert_eq!(members[0].endpoint_age_secs, None);
        assert!(!members[1].blocked, "無関係の相手はそのまま");
        assert!(members[1].endpoint.is_some());

        // 別のメンバー(組に含まれない)にはフィルタがかからない
        let other: Ipv4Addr = "100.100.42.9".parse().unwrap();
        let ControlMessage::Ledger { members, .. } = ledger_message_for(dist, other) else {
            panic!("Ledger メッセージになる");
        };
        assert!(!members[0].blocked);
        assert!(members[0].endpoint.is_some());
    }

    /// Removed を送るとメンバー側セッションが終了し、台帳がクリアされる。
    #[tokio::test]
    async fn removed_notification_ends_session() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (_ledger_tx, ledger_rx) = watch::channel(Distribution::default());
        let connections: Connections = Arc::new(Mutex::new(HashMap::new()));

        let server_connections = Arc::clone(&connections);
        let server_rtt: RttMap = Default::default();
        let server = tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.unwrap();
            let ip = match peer.ip() {
                IpAddr::V4(ip) => ip,
                _ => unreachable!(),
            };
            handle_member(stream, ip, ledger_rx, &server_connections, &server_rtt).await
        });

        let latest: Arc<Mutex<Option<ReceivedDistribution>>> = Arc::new(Mutex::new(None));
        let client_latest = Arc::clone(&latest);
        let client_rtt: RttMap = Default::default();
        let client_removed = Arc::new(AtomicBool::new(false));
        let removed_flag = Arc::clone(&client_removed);
        let client = tokio::spawn(async move {
            let stream = TcpStream::connect(addr).await.unwrap();
            let host_ip: Ipv4Addr = "127.0.0.1".parse().unwrap();
            member_session(
                stream,
                &None,
                &client_latest,
                host_ip,
                &client_rtt,
                &removed_flag,
            )
            .await
        });

        // 接続登録を待って Removed を送る
        let sender = loop {
            if let Some(tx) = connections.lock().unwrap().values().next().cloned() {
                break tx;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        sender
            .send(ControlMessage::Removed {
                message: "テスト削除".to_string(),
            })
            .unwrap();

        let client_result = client.await.unwrap();
        assert!(client_result.is_err(), "削除通知でセッションが終わること");
        assert!(
            client_removed.load(Ordering::Relaxed),
            "削除フラグが立つこと(UI に「削除された」と出す信号)"
        );
        assert!(server.await.unwrap().is_ok());
    }

    async fn wait_for<T>(value: &Arc<Mutex<Option<T>>>, predicate: impl Fn(&Option<T>) -> bool) {
        for _ in 0..100 {
            if predicate(&value.lock().unwrap()) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("条件が満たされませんでした");
    }

    async fn wait_until<T>(mut probe: impl FnMut() -> Option<T>) -> T {
        for _ in 0..100 {
            if let Some(value) = probe() {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("条件が満たされませんでした");
    }

    /// 送受信するだけの対向 TCP を用意し、読み側のハーフを返す。
    async fn loopback_reader(payload: Vec<u8>) -> LineReader {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut stream = TcpStream::connect(addr).await.unwrap();
            stream.write_all(&payload).await.unwrap();
            stream.shutdown().await.unwrap();
        });
        let (stream, _) = listener.accept().await.unwrap();
        line_reader(stream.into_split().0)
    }

    /// `take` の上限は reader の**累計**。1 行ごとに戻さないと、ping/pong を
    /// 積み重ねた数時間後に EOF 扱いになって制御接続が落ちる(実際に踏んだ)。
    #[tokio::test]
    async fn read_line_resets_the_cumulative_take_limit() {
        const LINES: u64 = 3000;
        let mut payload = Vec::new();
        for nonce in 0..LINES {
            payload.extend_from_slice(encode_line(&ControlMessage::Ping { nonce }).as_bytes());
        }
        assert!(
            payload.len() as u64 > MAX_LINE_LEN,
            "累計が上限を超える量を送ること: {} バイト",
            payload.len()
        );

        let mut reader = loopback_reader(payload).await;
        let mut line = String::new();
        let mut read = 0u64;
        while read_line(&mut reader, &mut line).await.unwrap().is_some() {
            read += 1;
        }
        assert_eq!(read, LINES, "累計上限で打ち切られない");
    }

    /// 1 行が上限を超えたら、EOF ではなくエラーにする(メモリを守る)。
    #[tokio::test]
    async fn read_line_rejects_a_too_long_line() {
        let payload = vec![b'x'; MAX_LINE_LEN as usize + 1];
        let mut reader = loopback_reader(payload).await;
        let error = read_line(&mut reader, &mut String::new())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("バイトを超えました"), "{error}");
    }

    /// 行の途中で切れた場合は、長すぎる行と区別して報告する。
    #[tokio::test]
    async fn read_line_reports_truncated_line() {
        let mut reader = loopback_reader(b"{\"type\":\"pi".to_vec()).await;
        let error = read_line(&mut reader, &mut String::new())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("途中で切断"), "{error}");
    }

    /// 未知の nonce の Pong では RTT を記録しない(遅れて届いた応答の混入防止)。
    #[test]
    fn ping_state_ignores_unknown_nonce() {
        let mut ping = PingState::default();
        assert_eq!(ping.observe(1), None, "未送信の nonce");
        let ControlMessage::Ping { nonce } = ping.issue() else {
            panic!("Ping を発行する");
        };
        assert_eq!(ping.observe(nonce + 1), None, "別の nonce");
        assert!(ping.observe(nonce).is_some());
        assert_eq!(ping.observe(nonce), None, "同じ Pong の二重計上はしない");
    }
}
