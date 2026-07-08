//! PeerCove デスクトップ UI の Rust 側(M2-G2)。
//!
//! UI は**非特権**で動き、管理者/root のデーモンをローカル IPC 経由で操作する
//! (ADR-0007)。ここでは IPC の応答を UI 用 DTO へ変換して frontend へ渡す。
//! serde の内部タグ表現をそのまま TypeScript に写すと、プロトコルの表現変更が
//! UI に波及するため、境界を明示的に切っている。

mod dto;

use peercove_core::ipc::{IpcRequest, IpcResponse};

use crate::dto::Status;

/// デーモンの状態を取得する。デーモンに届かない場合は Err(人間向けメッセージ)。
#[tauri::command]
async fn daemon_status() -> Result<Status, String> {
    match peercove_ipc::request_async(IpcRequest::Status).await {
        Ok(IpcResponse::Status(status)) => Ok(Status::from(status)),
        Ok(other) => Err(format!("想定外の応答です: {other:?}")),
        Err(e) => Err(format!("{e:#}")),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![daemon_status])
        .run(tauri::generate_context!())
        .expect("Tauri アプリの起動に失敗しました");
}
