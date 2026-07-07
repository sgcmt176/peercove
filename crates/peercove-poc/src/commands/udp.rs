//! G-5 検証用の UDP ツール。
//!
//! - `udp-echo`: 受け取ったデータグラムをそのまま送り返すサーバー
//! - `udp-ping`: 連番ペイロードを送って RTT を測るクライアント

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use tokio::net::UdpSocket;

const BUF_SIZE: usize = 2048;
const PING_INTERVAL: Duration = Duration::from_secs(1);
const PING_TIMEOUT: Duration = Duration::from_secs(2);

fn runtime() -> anyhow::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("非同期ランタイムの初期化に失敗しました")
}

pub fn run_echo(listen: SocketAddr) -> anyhow::Result<()> {
    runtime()?.block_on(async {
        let socket = UdpSocket::bind(listen)
            .await
            .with_context(|| format!("{listen} の bind に失敗しました"))?;
        println!(
            "UDP echo サーバーを {} で起動しました(Ctrl+C で終了)",
            socket.local_addr()?
        );
        let mut buf = [0u8; BUF_SIZE];
        let mut count: u64 = 0;
        loop {
            tokio::select! {
                result = tokio::signal::ctrl_c() => {
                    result.context("シグナル待機に失敗しました")?;
                    println!("終了します(合計 {count} パケットに応答)");
                    return Ok(());
                }
                result = socket.recv_from(&mut buf) => {
                    let (len, from) = result.context("受信に失敗しました")?;
                    count += 1;
                    println!("{from} から {len} バイト受信 -> 返送(#{count})");
                    if let Err(e) = socket.send_to(&buf[..len], from).await {
                        tracing::warn!("{from} への返送に失敗しました: {e}");
                    }
                }
            }
        }
    })
}

#[derive(Debug, Default)]
pub struct PingStats {
    pub sent: u32,
    pub received: u32,
    pub rtts: Vec<Duration>,
}

impl PingStats {
    fn loss_percent(&self) -> f64 {
        if self.sent == 0 {
            return 0.0;
        }
        f64::from(self.sent - self.received) / f64::from(self.sent) * 100.0
    }
}

pub fn run_ping(target: SocketAddr, count: u32) -> anyhow::Result<()> {
    if count == 0 {
        bail!("--count は 1 以上を指定してください");
    }
    let stats = runtime()?.block_on(ping(target, count, PING_INTERVAL, PING_TIMEOUT))?;

    println!();
    println!(
        "--- {target} の udp-ping 統計: 送信 {} / 受信 {} / 損失 {:.0}% ---",
        stats.sent,
        stats.received,
        stats.loss_percent()
    );
    if !stats.rtts.is_empty() {
        let min = stats.rtts.iter().min().unwrap();
        let max = stats.rtts.iter().max().unwrap();
        let avg = stats.rtts.iter().sum::<Duration>() / stats.rtts.len() as u32;
        println!(
            "rtt min/avg/max = {:.2}/{:.2}/{:.2} ms",
            min.as_secs_f64() * 1000.0,
            avg.as_secs_f64() * 1000.0,
            max.as_secs_f64() * 1000.0
        );
    }
    if stats.received == 0 {
        bail!(
            "応答がありません。確認: (1) 相手側で udp-echo が起動しているか \
             (2) 相手側ファイアウォールで UDP {} 番の受信が許可されているか \
             (3) トンネルの疎通(ping)が通っているか",
            target.port()
        );
    }
    Ok(())
}

async fn ping(
    target: SocketAddr,
    count: u32,
    interval: Duration,
    timeout: Duration,
) -> anyhow::Result<PingStats> {
    let socket = UdpSocket::bind(("0.0.0.0", 0))
        .await
        .context("送信用ソケットの作成に失敗しました")?;
    socket
        .connect(target)
        .await
        .with_context(|| format!("{target} への接続設定に失敗しました"))?;

    let mut stats = PingStats::default();
    let mut buf = [0u8; BUF_SIZE];
    for seq in 1..=count {
        let payload = format!("peercove-udp-ping seq={seq}");
        let started = Instant::now();
        socket
            .send(payload.as_bytes())
            .await
            .context("送信に失敗しました")?;
        stats.sent += 1;

        // タイムアウトまでの間、自分の seq の応答が返るのを待つ
        let deadline = started + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                println!("seq={seq} タイムアウト({} 秒)", timeout.as_secs());
                break;
            }
            match tokio::time::timeout(remaining, socket.recv(&mut buf)).await {
                Ok(Ok(len)) => {
                    if &buf[..len] == payload.as_bytes() {
                        let rtt = started.elapsed();
                        println!(
                            "{} バイト受信 seq={seq} rtt={:.2} ms",
                            len,
                            rtt.as_secs_f64() * 1000.0
                        );
                        stats.received += 1;
                        stats.rtts.push(rtt);
                        break;
                    }
                    // 遅れて届いた過去の seq などは読み捨てて待ち直す
                }
                Ok(Err(e)) => {
                    // ICMP port unreachable が ConnectionReset として観測されることがある
                    tracing::debug!("受信エラー(継続): {e}");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(_) => {
                    println!("seq={seq} タイムアウト({} 秒)", timeout.as_secs());
                    break;
                }
            }
        }
        if seq != count {
            tokio::time::sleep(interval.saturating_sub(started.elapsed())).await;
        }
    }
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ループバックで echo サーバーに対して ping し、全応答・RTT 記録を確認する。
    #[test]
    fn ping_against_local_echo_server() {
        let runtime = runtime().unwrap();
        runtime.block_on(async {
            let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let server_addr = server.local_addr().unwrap();
            tokio::spawn(async move {
                let mut buf = [0u8; BUF_SIZE];
                loop {
                    let (len, from) = server.recv_from(&mut buf).await.unwrap();
                    server.send_to(&buf[..len], from).await.unwrap();
                }
            });

            let stats = ping(
                server_addr,
                5,
                Duration::from_millis(10),
                Duration::from_secs(2),
            )
            .await
            .unwrap();
            assert_eq!(stats.sent, 5);
            assert_eq!(stats.received, 5);
            assert_eq!(stats.rtts.len(), 5);
        });
    }

    /// 応答が無い宛先ではタイムアウトして received=0 になる。
    #[test]
    fn ping_times_out_without_server() {
        let runtime = runtime().unwrap();
        runtime.block_on(async {
            // bind だけして応答しないソケット
            let silent = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let addr = silent.local_addr().unwrap();
            let stats = ping(
                addr,
                2,
                Duration::from_millis(10),
                Duration::from_millis(200),
            )
            .await
            .unwrap();
            assert_eq!(stats.sent, 2);
            assert_eq!(stats.received, 0);
        });
    }
}
