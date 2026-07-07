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
use std::sync::{Arc, Mutex};
use std::time::Duration;

use peercove_core::proto::{ControlMessage, LedgerEntry, CONTROL_PORT, PROTO_VERSION};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};

/// 受信 1 行の上限。台帳が大きくても余裕を持てるサイズ。
const MAX_LINE_LEN: usize = 64 * 1024;
const RETRY_INTERVAL: Duration = Duration::from_secs(5);
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

/// 接続中メンバー(仮想 IP → 送信キュー)。削除通知(M1-G3)で使う。
pub type Connections = Arc<Mutex<HashMap<Ipv4Addr, mpsc::UnboundedSender<ControlMessage>>>>;

fn encode_line(message: &ControlMessage) -> String {
    let mut line = serde_json::to_string(message).expect("ControlMessage は常に直列化可能");
    line.push('\n');
    line
}

/// ホスト側サーバー。台帳の変更を watch チャネルで受け取り、全接続へ配布する。
pub async fn run_host_server(
    bind_ip: Ipv4Addr,
    ledger_rx: watch::Receiver<Vec<LedgerEntry>>,
    connections: Connections,
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
                tokio::spawn(async move {
                    if let Err(e) = handle_member(stream, member_ip, ledger_rx, &connections).await
                    {
                        tracing::debug!("メンバー {member_ip} との制御接続が終了: {e:#}");
                    }
                    connections.lock().unwrap().remove(&member_ip);
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

async fn handle_member(
    stream: TcpStream,
    member_ip: Ipv4Addr,
    mut ledger_rx: watch::Receiver<Vec<LedgerEntry>>,
    connections: &Connections,
) -> anyhow::Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half).take(MAX_LINE_LEN as u64);

    // 最初のメッセージは Hello(名乗り)
    let mut line = String::new();
    tokio::time::timeout(HELLO_TIMEOUT, reader.read_line(&mut line))
        .await
        .map_err(|_| anyhow::anyhow!("Hello がタイムアウトしました"))??;
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

    // 送信キューを登録(台帳変更・削除通知の配送口)
    let (tx, mut rx) = mpsc::unbounded_channel::<ControlMessage>();
    connections.lock().unwrap().insert(member_ip, tx);

    // 現在の台帳を即送信
    let snapshot = ledger_rx.borrow().clone();
    write_half
        .write_all(encode_line(&ControlMessage::Ledger { members: snapshot }).as_bytes())
        .await?;

    // 以後: 台帳の変更 or 個別メッセージを配送。読み側は EOF 検知のため読み続ける
    let mut line = String::new();
    loop {
        tokio::select! {
            changed = ledger_rx.changed() => {
                if changed.is_err() {
                    return Ok(()); // 送信側(supervisor)終了
                }
                let snapshot = ledger_rx.borrow_and_update().clone();
                write_half
                    .write_all(encode_line(&ControlMessage::Ledger { members: snapshot }).as_bytes())
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
            read = reader.read_line({ line.clear(); &mut line }) => {
                if read? == 0 {
                    return Ok(()); // メンバー側が切断
                }
                // v1 では Hello 以降にメンバーから届くメッセージはない(将来拡張用に無視)
                tracing::debug!("メンバー {member_ip} から: {}", line.trim_end());
            }
        }
    }
}

/// メンバー側クライアント。台帳を受信して `latest_ledger` に反映する。
/// 切断されたら自動で再接続する。
pub async fn run_member_client(
    host_ip: Ipv4Addr,
    display_name: Option<String>,
    latest_ledger: Arc<Mutex<Option<Vec<LedgerEntry>>>>,
) {
    let target = SocketAddr::from((host_ip, CONTROL_PORT));
    let mut logged_wait = false;
    loop {
        match TcpStream::connect(target).await {
            Ok(stream) => {
                logged_wait = false;
                tracing::info!("コントロールチャネルに接続しました({target})");
                if let Err(e) = member_session(stream, &display_name, &latest_ledger).await {
                    tracing::debug!("制御接続が終了しました(再接続します): {e:#}");
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

async fn member_session(
    stream: TcpStream,
    display_name: &Option<String>,
    latest_ledger: &Arc<Mutex<Option<Vec<LedgerEntry>>>>,
) -> anyhow::Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    write_half
        .write_all(
            encode_line(&ControlMessage::Hello {
                version: PROTO_VERSION,
                name: display_name.clone(),
            })
            .as_bytes(),
        )
        .await?;

    let mut reader = BufReader::new(read_half).take(MAX_LINE_LEN as u64);
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            anyhow::bail!("ホストが切断しました");
        }
        match serde_json::from_str::<ControlMessage>(&line) {
            Ok(ControlMessage::Ledger { members }) => {
                tracing::info!("台帳を受信しました({} 名)", members.len());
                *latest_ledger.lock().unwrap() = Some(members);
            }
            Ok(ControlMessage::Removed { message }) => {
                tracing::warn!("ホストから削除されました: {message}");
                *latest_ledger.lock().unwrap() = None;
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
        }
    }

    /// host サーバー ↔ member クライアントを localhost で対向させ、
    /// 台帳の初回配布と変更配布がクライアントに反映されることを確認する。
    #[tokio::test]
    async fn ledger_is_distributed_and_updated() {
        // 127.0.0.1 で CONTROL_PORT が使用中でもテストが落ちないよう、
        // サーバー本体ではなく handle_member を直接テストする
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (ledger_tx, ledger_rx) = watch::channel(vec![entry("alice", "100.100.42.2", true)]);
        let connections: Connections = Arc::new(Mutex::new(HashMap::new()));

        let server_connections = Arc::clone(&connections);
        tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.unwrap();
            let ip = match peer.ip() {
                IpAddr::V4(ip) => ip,
                _ => unreachable!(),
            };
            let _ = handle_member(stream, ip, ledger_rx, &server_connections).await;
        });

        let latest: Arc<Mutex<Option<Vec<LedgerEntry>>>> = Arc::new(Mutex::new(None));
        let client_latest = Arc::clone(&latest);
        let client = tokio::spawn(async move {
            let stream = TcpStream::connect(addr).await.unwrap();
            let _ = member_session(stream, &Some("alice".to_string()), &client_latest).await;
        });

        // 初回スナップショットを受信
        wait_for(&latest, |l| l.as_ref().map(|m| m.len()) == Some(1)).await;

        // 台帳の変更が配布される
        ledger_tx
            .send(vec![
                entry("alice", "100.100.42.2", true),
                entry("bob", "100.100.42.3", false),
            ])
            .unwrap();
        wait_for(&latest, |l| l.as_ref().map(|m| m.len()) == Some(2)).await;
        {
            let ledger = latest.lock().unwrap();
            let members = ledger.as_ref().unwrap();
            assert_eq!(members[1].name.as_deref(), Some("bob"));
            assert!(!members[1].online);
        }

        // 接続レジストリに登録されている(削除通知の配送口)
        assert_eq!(connections.lock().unwrap().len(), 1);
        client.abort();
    }

    /// Removed を送るとメンバー側セッションが終了し、台帳がクリアされる。
    #[tokio::test]
    async fn removed_notification_ends_session() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (_ledger_tx, ledger_rx) = watch::channel(vec![]);
        let connections: Connections = Arc::new(Mutex::new(HashMap::new()));

        let server_connections = Arc::clone(&connections);
        let server = tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.unwrap();
            let ip = match peer.ip() {
                IpAddr::V4(ip) => ip,
                _ => unreachable!(),
            };
            handle_member(stream, ip, ledger_rx, &server_connections).await
        });

        let latest: Arc<Mutex<Option<Vec<LedgerEntry>>>> = Arc::new(Mutex::new(None));
        let client_latest = Arc::clone(&latest);
        let client = tokio::spawn(async move {
            let stream = TcpStream::connect(addr).await.unwrap();
            member_session(stream, &None, &client_latest).await
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
}
