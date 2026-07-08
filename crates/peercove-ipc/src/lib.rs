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
    // デーモンは root(サービス)で動くことが多く、そのソケットは /run にある。
    // 一方クライアントは通常ユーザーなので、自分の euid だけでパスを決めると
    // すれ違う。候補を順に試す。
    let candidates = socket_candidates();
    let mut last_error = None;
    for path in &candidates {
        match tokio::net::UnixStream::connect(path).await {
            Ok(stream) => {
                tracing::debug!("IPC: {} へ接続しました", path.display());
                return Ok(stream);
            }
            Err(e) => last_error = Some(e),
        }
    }
    let places = candidates
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    Err(last_error.expect("候補は 1 つ以上ある")).with_context(|| {
        format!(
            "デーモンに接続できません(`peercove-poc daemon run` が起動していますか?)。\
             探した場所: {places}"
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

/// 環境変数でソケットパスを上書きする(検証・複数インスタンス用)。
#[cfg(unix)]
pub const SOCKET_ENV: &str = "PEERCOVE_SOCKET";

/// **サーバー**が bind する Unix ドメインソケットのパス。
/// root(サービス)なら `/run/peercove.sock`、それ以外はユーザー領域。
#[cfg(unix)]
pub fn socket_path() -> std::path::PathBuf {
    use std::path::PathBuf;
    if let Ok(path) = std::env::var(SOCKET_ENV) {
        return PathBuf::from(path);
    }
    // SAFETY: geteuid は引数なし・常に成功する POSIX API。OS 境界のため unsafe。
    let euid = unsafe { libc::geteuid() };
    if euid == 0 {
        PathBuf::from(peercove_core::ipc::SOCKET_PATH_ROOT)
    } else {
        user_socket_path(euid)
    }
}

#[cfg(unix)]
fn user_socket_path(euid: u32) -> std::path::PathBuf {
    use std::path::PathBuf;
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        PathBuf::from(dir).join("peercove.sock")
    } else {
        std::env::temp_dir().join(format!("peercove-{euid}.sock"))
    }
}

/// **クライアント**が順に試すソケットの候補。
///
/// デーモンは root で動くのが普通(サービス化後は必ず)なので、
/// 自分の euid から決め打ちすると通常ユーザーの UI/CLI が繋がらない。
/// root のソケット → 自分のユーザー領域、の順に探す。
#[cfg(unix)]
pub fn socket_candidates() -> Vec<std::path::PathBuf> {
    use std::path::PathBuf;
    if let Ok(path) = std::env::var(SOCKET_ENV) {
        return vec![PathBuf::from(path)];
    }
    // SAFETY: geteuid は引数なし・常に成功する POSIX API。
    let euid = unsafe { libc::geteuid() };
    let mut candidates = vec![PathBuf::from(peercove_core::ipc::SOCKET_PATH_ROOT)];
    let user = user_socket_path(euid);
    if !candidates.contains(&user) {
        candidates.push(user);
    }
    candidates
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// 環境変数を共有するため 1 つのテストにまとめる(並列実行での競合を避ける)。
    #[test]
    fn socket_paths_for_server_and_client() {
        std::env::remove_var(SOCKET_ENV);

        // クライアントは root のソケットを先に試す(デーモンはサービス = root)
        let candidates = socket_candidates();
        assert_eq!(
            candidates[0],
            std::path::Path::new(peercove_core::ipc::SOCKET_PATH_ROOT)
        );
        // SAFETY: geteuid は引数なし・常に成功する POSIX API。
        let euid = unsafe { libc::geteuid() };
        if euid != 0 {
            // 非 root なら自分のユーザー領域も候補に入り、サーバーはそこへ bind する
            assert_eq!(candidates.len(), 2);
            assert_eq!(socket_path(), candidates[1]);
        } else {
            assert_eq!(socket_path(), candidates[0]);
        }

        // 環境変数の上書きはサーバー・クライアント双方に効く
        let override_path = std::path::PathBuf::from("/tmp/peercove-test.sock");
        std::env::set_var(SOCKET_ENV, &override_path);
        assert_eq!(socket_candidates(), vec![override_path.clone()]);
        assert_eq!(socket_path(), override_path);
        std::env::remove_var(SOCKET_ENV);
    }
}
