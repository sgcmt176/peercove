//! デーモン(M2-G1a、ADR-0007)。
//!
//! `peercove-poc daemon run` で常駐し、ローカル IPC(Windows: 名前付きパイプ /
//! Linux: Unix ドメインソケット)でトンネルの開始・停止・状態取得を受け付ける。
//! 招待・削除などの設定ファイル操作は IPC に乗せない(UI/CLI が直接行い、
//! 実行中トンネルは 5 秒再読込で追随する)。
//!
//! トランスポート非依存の部分(`handle_connection` / `request_over`)は
//! 任意の AsyncRead+AsyncWrite で動き、テストは `tokio::io::duplex` で行う。

use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use anyhow::{bail, Context};
use peercove_core::ipc::{
    DaemonStatus, IpcEnvelope, IpcReply, IpcRequest, IpcResponse, IpcResult, PeerSummary,
    TunnelInfo,
};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::watch;

use crate::commands::tunnel::{self, ActiveTunnel, Role, SharedSnapshot};

const MAX_LINE_LEN: u64 = 256 * 1024;

/// トンネルの起動方法(テストでは差し替える)。
type BringUp = Box<dyn Fn(&Path, Role, bool) -> anyhow::Result<ActiveTunnel> + Send + Sync>;

/// デーモンの共有状態。
pub struct DaemonShared {
    active: tokio::sync::Mutex<Option<Active>>,
    bring_up: BringUp,
    shutdown_tx: watch::Sender<bool>,
}

struct Active {
    role: Role,
    config: PathBuf,
    address: Ipv4Addr,
    stop_tx: watch::Sender<bool>,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
    snapshot: SharedSnapshot,
}

impl DaemonShared {
    fn new(bring_up: BringUp) -> (Arc<Self>, watch::Receiver<bool>) {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        (
            Arc::new(Self {
                active: tokio::sync::Mutex::new(None),
                bring_up,
                shutdown_tx,
            }),
            shutdown_rx,
        )
    }

    async fn dispatch(self: &Arc<Self>, request: IpcRequest) -> anyhow::Result<IpcResponse> {
        match request {
            IpcRequest::Status => Ok(IpcResponse::Status(self.status().await)),
            IpcRequest::StartHost { config, upnp } => {
                self.start(config, Role::Host, upnp).await?;
                Ok(IpcResponse::Done)
            }
            IpcRequest::StartMember { config } => {
                self.start(config, Role::Member, false).await?;
                Ok(IpcResponse::Done)
            }
            IpcRequest::Stop => {
                self.stop().await?;
                Ok(IpcResponse::Done)
            }
            IpcRequest::Shutdown => {
                if self.active.lock().await.is_some() {
                    self.stop().await?;
                }
                let _ = self.shutdown_tx.send(true);
                Ok(IpcResponse::Done)
            }
        }
    }

    async fn start(
        self: &Arc<Self>,
        config: PathBuf,
        role: Role,
        upnp: bool,
    ) -> anyhow::Result<()> {
        let mut active = self.active.lock().await;
        if active.is_some() {
            bail!("既にトンネルが動いています。先に stop してください");
        }
        // bring_up はブロッキング処理(netlink / netsh / UPnP)なので専用スレッドで
        let shared = Arc::clone(self);
        let config_for_up = config.clone();
        let tunnel =
            tokio::task::spawn_blocking(move || (shared.bring_up)(&config_for_up, role, upnp))
                .await
                .context("起動タスクの実行に失敗しました")??;

        let address = tunnel.spec.address.addr();
        let (stop_tx, stop_rx) = watch::channel(false);
        let snapshot: SharedSnapshot = Arc::new(Mutex::new(None));
        let task_snapshot = Arc::clone(&snapshot);
        let task_config = config.clone();
        let task = tokio::spawn(async move {
            let mut tunnel = tunnel;
            let supervise_result = tunnel
                .supervise_run(&task_config, stop_rx, Some(task_snapshot))
                .await;
            // クリーンアップ(ブロッキング)は必ず実行する
            let down_result =
                tokio::task::spawn_blocking(move || tunnel::tear_down(tunnel, &task_config))
                    .await
                    .context("停止タスクの実行に失敗しました")?;
            supervise_result.and(down_result)
        });
        *active = Some(Active {
            role,
            config,
            address,
            stop_tx,
            task,
            snapshot,
        });
        tracing::info!("トンネルを開始しました");
        Ok(())
    }

    async fn stop(self: &Arc<Self>) -> anyhow::Result<()> {
        let Some(active) = self.active.lock().await.take() else {
            bail!("トンネルは動いていません");
        };
        let _ = active.stop_tx.send(true);
        active
            .task
            .await
            .context("トンネルタスクの終了待ちに失敗しました")?
            .context("トンネルの停止処理でエラーが発生しました")?;
        tracing::info!("トンネルを停止しました");
        Ok(())
    }

    async fn status(&self) -> DaemonStatus {
        let active = self.active.lock().await;
        let Some(active) = active.as_ref() else {
            return DaemonStatus::Idle;
        };
        let (peers, ledger) = active
            .snapshot
            .lock()
            .unwrap()
            .clone()
            .unwrap_or((Vec::new(), None));
        let now = SystemTime::now();
        let info = TunnelInfo {
            config: active.config.clone(),
            address: active.address,
            ledger: ledger.unwrap_or_default(),
            peers: peers
                .iter()
                .map(|p| PeerSummary {
                    public_key: p.public_key,
                    endpoint: p.endpoint,
                    last_handshake_age_secs: p
                        .last_handshake
                        .and_then(|t| now.duration_since(t).ok())
                        .map(|d| d.as_secs()),
                    rx_bytes: p.rx_bytes,
                    tx_bytes: p.tx_bytes,
                })
                .collect(),
        };
        match active.role {
            Role::Host => DaemonStatus::Hosting(info),
            Role::Member => DaemonStatus::Joined(info),
        }
    }
}

/// 1 本の IPC 接続を処理する(トランスポート非依存)。
async fn handle_connection<S>(stream: S, shared: Arc<DaemonShared>) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (read_half, mut write_half) = tokio::io::split(stream);
    let mut reader = BufReader::new(read_half).take(MAX_LINE_LEN);
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            return Ok(()); // クライアント切断
        }
        let reply = match serde_json::from_str::<IpcEnvelope>(&line) {
            Ok(envelope) => {
                let result = match shared.dispatch(envelope.req).await {
                    Ok(response) => IpcResult::Ok(response),
                    Err(e) => IpcResult::Err(format!("{e:#}")),
                };
                IpcReply {
                    id: envelope.id,
                    result,
                }
            }
            Err(e) => IpcReply {
                id: 0,
                result: IpcResult::Err(format!("リクエストを解析できません: {e}")),
            },
        };
        let mut json = serde_json::to_string(&reply).expect("IpcReply は常に直列化可能");
        json.push('\n');
        write_half.write_all(json.as_bytes()).await?;
    }
}

/// 任意のストリーム上で 1 リクエストを送る(クライアント側の共通部)。
pub async fn request_over<S>(
    stream: &mut S,
    id: u64,
    req: &IpcRequest,
) -> anyhow::Result<IpcResponse>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut json = serde_json::to_string(&IpcEnvelope {
        id,
        req: req.clone(),
    })
    .expect("IpcEnvelope は常に直列化可能");
    json.push('\n');
    stream.write_all(json.as_bytes()).await?;

    let mut reader = BufReader::new(stream).take(MAX_LINE_LEN);
    let mut line = String::new();
    if reader.read_line(&mut line).await? == 0 {
        bail!("デーモンが応答せず切断しました");
    }
    let reply: IpcReply = serde_json::from_str(&line).context("デーモンの応答を解析できません")?;
    if reply.id != id {
        bail!("応答 id が一致しません(期待 {id}、実際 {})", reply.id);
    }
    match reply.result {
        IpcResult::Ok(response) => Ok(response),
        IpcResult::Err(message) => bail!("{message}"),
    }
}

// ---- サーバー(OS 別トランスポート) ----

/// `daemon run`: IPC サーバーを起動して常駐する。
pub fn run_server() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .context("非同期ランタイムの初期化に失敗しました")?;
    let (shared, shutdown_rx) = DaemonShared::new(Box::new(tunnel::bring_up));
    println!("peercove デーモンを開始しました(Ctrl+C か shutdown 要求で終了)");
    runtime.block_on(async {
        tokio::select! {
            result = accept_loop(Arc::clone(&shared)) => result,
            result = tokio::signal::ctrl_c() => {
                result.context("シグナル待機に失敗しました")?;
                Ok(())
            }
            _ = wait_shutdown(shutdown_rx) => Ok(()),
        }
    })?;
    // 常駐終了時にトンネルが残っていれば必ず片付ける
    runtime.block_on(async {
        if shared.active.lock().await.is_some() {
            if let Err(e) = shared.stop().await {
                tracing::warn!("終了時のトンネル停止に失敗しました: {e:#}");
            }
        }
    });
    println!("peercove デーモンを終了しました");
    Ok(())
}

async fn wait_shutdown(mut rx: watch::Receiver<bool>) {
    while !*rx.borrow() {
        if rx.changed().await.is_err() {
            return;
        }
    }
}

#[cfg(windows)]
async fn accept_loop(shared: Arc<DaemonShared>) -> anyhow::Result<()> {
    use tokio::net::windows::named_pipe::ServerOptions;
    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .create(peercove_core::ipc::PIPE_NAME)
        .context("名前付きパイプの作成に失敗しました(既にデーモンが動いていませんか?)")?;
    tracing::info!("IPC: {} で待受けます", peercove_core::ipc::PIPE_NAME);
    loop {
        server
            .connect()
            .await
            .context("パイプ接続の待受に失敗しました")?;
        let stream = server;
        server = ServerOptions::new()
            .create(peercove_core::ipc::PIPE_NAME)
            .context("次のパイプインスタンスの作成に失敗しました")?;
        let shared = Arc::clone(&shared);
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, shared).await {
                tracing::debug!("IPC 接続が終了: {e:#}");
            }
        });
    }
}

#[cfg(unix)]
async fn accept_loop(shared: Arc<DaemonShared>) -> anyhow::Result<()> {
    let path = socket_path();
    let _ = std::fs::remove_file(&path); // 前回異常終了の残骸
    let listener = tokio::net::UnixListener::bind(&path)
        .with_context(|| format!("{} の bind に失敗しました", path.display()))?;
    tracing::info!("IPC: {} で待受けます", path.display());
    loop {
        let (stream, _) = listener
            .accept()
            .await
            .context("UDS の accept に失敗しました")?;
        let shared = Arc::clone(&shared);
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, shared).await {
                tracing::debug!("IPC 接続が終了: {e:#}");
            }
        });
    }
}

#[cfg(unix)]
pub fn socket_path() -> PathBuf {
    // SAFETY: geteuid は引数なし・常に成功する POSIX API。
    let euid = unsafe { libc::geteuid() };
    if euid == 0 {
        PathBuf::from(peercove_core::ipc::SOCKET_PATH_ROOT)
    } else if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(dir).join("peercove.sock")
    } else {
        std::env::temp_dir().join(format!("peercove-{euid}.sock"))
    }
}

// ---- クライアント ----

/// デーモンへ 1 リクエストを送って応答を返す(CLI / 将来の UI 用)。
pub fn request(req: IpcRequest) -> anyhow::Result<IpcResponse> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("非同期ランタイムの初期化に失敗しました")?;
    runtime.block_on(async {
        let mut stream = connect().await?;
        request_over(&mut stream, 1, &req).await
    })
}

#[cfg(windows)]
async fn connect() -> anyhow::Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    use tokio::net::windows::named_pipe::ClientOptions;
    ClientOptions::new()
        .open(peercove_core::ipc::PIPE_NAME)
        .context("デーモンに接続できません(`peercove-poc daemon run` が起動していますか?)")
}

#[cfg(unix)]
async fn connect() -> anyhow::Result<tokio::net::UnixStream> {
    let path = socket_path();
    tokio::net::UnixStream::connect(&path)
        .await
        .with_context(|| {
            format!(
                "デーモンに接続できません({} 。`peercove-poc daemon run` が起動していますか?)",
                path.display()
            )
        })
}

/// status 応答を人間向けに表示する。
pub fn print_status(status: &DaemonStatus) {
    match status {
        DaemonStatus::Idle => println!("状態: 待機中(トンネルなし)"),
        DaemonStatus::Hosting(info) | DaemonStatus::Joined(info) => {
            let role = if matches!(status, DaemonStatus::Hosting(_)) {
                "ホスト"
            } else {
                "メンバー"
            };
            println!("状態: {role}として稼働中");
            println!("  設定: {}", info.config.display());
            println!("  仮想 IP: {}", info.address);
            if !info.ledger.is_empty() {
                println!("  members:");
                for entry in &info.ledger {
                    println!(
                        "    {} {}({}){}",
                        if entry.online { "●" } else { "○" },
                        entry.name.as_deref().unwrap_or("(名前なし)"),
                        entry.ip,
                        if entry.is_host { " [host]" } else { "" }
                    );
                }
            }
            for peer in &info.peers {
                let handshake = match peer.last_handshake_age_secs {
                    Some(age) => format!("{age} 秒前"),
                    None => "なし".to_string(),
                };
                println!(
                    "  peer {}: handshake {handshake}, rx {} B, tx {} B",
                    peer.public_key, peer.rx_bytes, peer.tx_bytes
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::mock::MockBackend;
    use crate::backend::TunnelSpec;
    use peercove_core::keys::PrivateKey;

    fn test_shared() -> (Arc<DaemonShared>, watch::Receiver<bool>) {
        DaemonShared::new(Box::new(|config, role, _upnp| {
            // 実トンネルの代わりにモックを起動する
            let spec = TunnelSpec {
                private_key: PrivateKey::generate(),
                address: "10.99.0.1/24".parse().unwrap(),
                listen_port: Some(51820),
                mtu: 1420,
                forwarding: role == Role::Host,
                peers: Vec::new(),
            };
            let _ = config;
            Ok(ActiveTunnel::new_for_test(
                spec,
                role,
                Box::new(MockBackend::default()),
            ))
        }))
    }

    /// duplex ストリーム越しに start → status → stop → shutdown の一連を流す。
    #[tokio::test]
    async fn ipc_lifecycle_over_duplex() {
        let (shared, mut shutdown_rx) = test_shared();
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let server = tokio::spawn(handle_connection(server_io, Arc::clone(&shared)));
        let mut client = client_io;

        // Idle
        let response = request_over(&mut client, 1, &IpcRequest::Status)
            .await
            .unwrap();
        assert_eq!(response, IpcResponse::Status(DaemonStatus::Idle));

        // Start host → Hosting
        let response = request_over(
            &mut client,
            2,
            &IpcRequest::StartHost {
                config: PathBuf::from("host.toml"),
                upnp: false,
            },
        )
        .await
        .unwrap();
        assert_eq!(response, IpcResponse::Done);
        let response = request_over(&mut client, 3, &IpcRequest::Status)
            .await
            .unwrap();
        match response {
            IpcResponse::Status(DaemonStatus::Hosting(info)) => {
                assert_eq!(info.address.to_string(), "10.99.0.1");
            }
            other => panic!("Hosting を期待: {other:?}"),
        }

        // 二重起動は拒否
        let err = request_over(
            &mut client,
            4,
            &IpcRequest::StartMember {
                config: PathBuf::from("member.toml"),
            },
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("既にトンネル"));

        // Stop → Idle
        let response = request_over(&mut client, 5, &IpcRequest::Stop)
            .await
            .unwrap();
        assert_eq!(response, IpcResponse::Done);
        let response = request_over(&mut client, 6, &IpcRequest::Status)
            .await
            .unwrap();
        assert_eq!(response, IpcResponse::Status(DaemonStatus::Idle));

        // Shutdown シグナル
        let response = request_over(&mut client, 7, &IpcRequest::Shutdown)
            .await
            .unwrap();
        assert_eq!(response, IpcResponse::Done);
        shutdown_rx.changed().await.unwrap();
        assert!(*shutdown_rx.borrow());

        drop(client);
        server.await.unwrap().unwrap();
    }

    /// stop 時に MockBackend の down が呼ばれる(クリーンアップの対称性)。
    #[tokio::test]
    async fn stop_tears_down_backend() {
        let ops: Arc<Mutex<Vec<String>>> = Default::default();
        let ops_for_factory = Arc::clone(&ops);
        let (shared, _rx) = DaemonShared::new(Box::new(move |_config, role, _upnp| {
            let spec = TunnelSpec {
                private_key: PrivateKey::generate(),
                address: "10.99.0.2/24".parse().unwrap(),
                listen_port: None,
                mtu: 1420,
                forwarding: false,
                peers: Vec::new(),
            };
            Ok(ActiveTunnel::new_for_test(
                spec,
                role,
                Box::new(MockBackend::with_shared_ops(Arc::clone(&ops_for_factory))),
            ))
        }));
        // Host ロールにする(Member は supervise 開始時に設定を読むため実ファイルが要る)
        shared
            .start(PathBuf::from("h.toml"), Role::Host, false)
            .await
            .unwrap();
        shared.stop().await.unwrap();
        let ops = ops.lock().unwrap();
        assert!(
            ops.contains(&"down".to_string()),
            "down が呼ばれる: {ops:?}"
        );
    }
}
