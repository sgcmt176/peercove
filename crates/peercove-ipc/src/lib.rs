//! デーモン制御 IPC のクライアント(ADR-0007)。
//!
//! UI(非特権)と CLI の双方から使う。プロトコル型は [`peercove_core::ipc`]。
//! トランスポートは Windows = 名前付きパイプ / Linux = Unix ドメインソケット。
//!
//! サーバー側(デーモン本体)は `peercove-poc` の `daemon` モジュールにある。
//! crate 分割の全体像は ADR-0007 を参照。

use anyhow::{bail, Context};
use peercove_core::ipc::{IpcEnvelope, IpcReply, IpcRequest, IpcResponse, IpcResult};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

/// 受信 1 行の上限(台帳が大きくても足りるサイズ)。
pub const MAX_LINE_LEN: u64 = 256 * 1024;

#[cfg(windows)]
pub type IpcStream = tokio::net::windows::named_pipe::NamedPipeClient;
#[cfg(unix)]
pub type IpcStream = tokio::net::UnixStream;

/// 任意のストリーム上で 1 リクエストを送り、応答を返す。
///
/// デーモン側のテスト(`tokio::io::duplex`)からも使えるよう、
/// トランスポート非依存にしてある。
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

/// デーモンへ接続する。
pub async fn connect() -> anyhow::Result<IpcStream> {
    connect_impl().await
}

#[cfg(windows)]
async fn connect_impl() -> anyhow::Result<IpcStream> {
    use tokio::net::windows::named_pipe::ClientOptions;
    ClientOptions::new()
        .open(peercove_core::ipc::PIPE_NAME)
        .context("デーモンに接続できません(`peercove-poc daemon run` が起動していますか?)")
}

#[cfg(unix)]
async fn connect_impl() -> anyhow::Result<IpcStream> {
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

/// デーモンへ 1 リクエストを送って応答を返す(非同期)。
pub async fn request_async(req: IpcRequest) -> anyhow::Result<IpcResponse> {
    let mut stream = connect().await?;
    request_over(&mut stream, 1, &req).await
}

/// デーモンへ 1 リクエストを送って応答を返す(同期。CLI 用)。
pub fn request(req: IpcRequest) -> anyhow::Result<IpcResponse> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("非同期ランタイムの初期化に失敗しました")?
        .block_on(request_async(req))
}

/// Unix ドメインソケットのパス。root 実行時は `/run`、それ以外はユーザー領域。
/// サーバー(bind)とクライアント(connect)の双方が使う。
#[cfg(unix)]
pub fn socket_path() -> std::path::PathBuf {
    use std::path::PathBuf;
    // SAFETY: geteuid は引数なし・常に成功する POSIX API。OS 境界のため unsafe。
    let euid = unsafe { libc::geteuid() };
    if euid == 0 {
        PathBuf::from(peercove_core::ipc::SOCKET_PATH_ROOT)
    } else if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(dir).join("peercove.sock")
    } else {
        std::env::temp_dir().join(format!("peercove-{euid}.sock"))
    }
}
