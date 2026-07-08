//! PeerCove デスクトップ UI の Rust 側(M2-G2/G3/G4)。
//!
//! UI は**非特権**で動く。役割は 2 つ:
//!
//! 1. **デーモン操作**(要特権のトンネル操作): ローカル IPC 経由(ADR-0007)
//! 2. **設定ファイル操作**(init / invite / join / メンバー管理): `peercove-ops` を
//!    ユーザー権限で直接呼ぶ。デーモンには 5 秒の再読込で自動反映される(ADR-0008)
//!
//! IPC の応答は `dto` で UI 用 DTO(camelCase)へ変換してから frontend に渡す。

mod dto;

use std::net::SocketAddrV4;
use std::path::{Path, PathBuf};

use peercove_core::ipc::{IpcRequest, IpcResponse};
use peercove_ops::peers::Selector;
use tauri::Manager;

use crate::dto::{ConfigPaths, ConfigSlot, InitResult, InviteResult, JoinResult, Status};

/// デフォルトの設定ディレクトリ(Windows: %APPDATA%\… / Linux: ~/.config/…)。
/// UI からは「別の設定を使う」で任意のファイルも選べる(ADR-0008)。
fn config_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map_err(|e| format!("設定ディレクトリを特定できません: {e}"))
}

/// anyhow のエラーチェーンを人間向け 1 行に潰す。
fn to_message(e: anyhow::Error) -> String {
    format!("{e:#}")
}

// ---- デーモン操作(IPC) ----

/// デーモンの状態を取得する。デーモンに届かない場合は Err(人間向けメッセージ)。
#[tauri::command]
async fn daemon_status() -> Result<Status, String> {
    match peercove_ipc::request_async(IpcRequest::Status).await {
        Ok(IpcResponse::Status(status)) => Ok(Status::from(status)),
        Ok(other) => Err(format!("想定外の応答です: {other:?}")),
        Err(e) => Err(to_message(e)),
    }
}

/// トンネルを開始する(ホスト)。設定パスは絶対にしてからデーモンへ渡す。
#[tauri::command]
async fn start_host(config_path: String, upnp: bool) -> Result<(), String> {
    let config = canonical(&config_path)?;
    send(IpcRequest::StartHost { config, upnp }).await
}

/// トンネルを開始する(メンバー)。
#[tauri::command]
async fn start_member(config_path: String) -> Result<(), String> {
    let config = canonical(&config_path)?;
    send(IpcRequest::StartMember { config }).await
}

/// トンネルを停止する(デーモンは常駐継続)。
#[tauri::command]
async fn stop_tunnel() -> Result<(), String> {
    send(IpcRequest::Stop).await
}

async fn send(request: IpcRequest) -> Result<(), String> {
    peercove_ipc::request_async(request)
        .await
        .map(|_| ())
        .map_err(to_message)
}

/// デーモンとクライアントで作業ディレクトリが違うため、パスは絶対にして送る。
fn canonical(path: &str) -> Result<PathBuf, String> {
    std::fs::canonicalize(path).map_err(|e| format!("{path} が見つかりません: {e}"))
}

// ---- 設定ファイル操作(ops) ----

/// 既定の設定ファイルの所在と、存在するかどうか。
#[tauri::command]
fn config_paths(app: tauri::AppHandle) -> Result<ConfigPaths, String> {
    let dir = config_dir(&app)?;
    Ok(ConfigPaths {
        host: ConfigSlot::of(&dir.join("host.toml")),
        member: ConfigSlot::of(&dir.join("member.toml")),
        dir: dir.display().to_string(),
    })
}

/// ホストを初期化する(鍵とランダムサブネットの host.toml を生成)。
#[tauri::command]
fn init_host(app: tauri::AppHandle, force: bool) -> Result<InitResult, String> {
    let dir = config_dir(&app)?;
    let result =
        peercove_ops::init::init_host(&dir, peercove_core::config::DEFAULT_LISTEN_PORT, force)
            .map_err(to_message)?;
    Ok(InitResult {
        config_path: result.config_path.display().to_string(),
        subnet: result.subnet.to_string(),
        host_ip: result.host_ip.to_string(),
        public_key: result.public_key.to_base64(),
    })
}

/// 招待トークンを発行する。**戻り値の token は秘密情報**(発行直後のみ表示する)。
#[tauri::command]
fn create_invite(
    config_path: String,
    name: Option<String>,
    psk: bool,
    endpoints: Vec<String>,
) -> Result<InviteResult, String> {
    let extra: Vec<SocketAddrV4> = endpoints
        .iter()
        .map(|e| {
            e.trim()
                .parse::<SocketAddrV4>()
                .map_err(|_| format!("エンドポイント {e} は IP:ポート形式で指定してください"))
        })
        .collect::<Result<_, _>>()?;

    let result = peercove_ops::invite::invite(&peercove_ops::invite::InviteOptions {
        config_path: Path::new(&config_path),
        name: name.as_deref().filter(|s| !s.trim().is_empty()),
        ip: None,
        extra_endpoints: &extra,
        psk,
    })
    .map_err(to_message)?;

    let qr_svg = render_qr_svg(&result.token)?;
    Ok(InviteResult {
        token: result.token,
        qr_svg,
        name: result.name,
        ip: result.ip.to_string(),
        endpoints: result.endpoints.iter().map(|e| e.to_string()).collect(),
        psk: result.psk,
    })
}

/// トークンを画面表示用の QR(SVG)にする。ターミナル用の文字 QR とは別。
fn render_qr_svg(token: &str) -> Result<String, String> {
    use fast_qr::convert::{svg::SvgBuilder, Builder, Shape};
    let qr = fast_qr::QRBuilder::new(token)
        .build()
        .map_err(|e| format!("QR コードの生成に失敗しました: {e:?}"))?;
    Ok(SvgBuilder::default().shape(Shape::Square).to_str(&qr))
}

/// 招待トークンから参加設定(member.key / member.toml)を作る。
#[tauri::command]
fn join_network(app: tauri::AppHandle, token: String, force: bool) -> Result<JoinResult, String> {
    let dir = config_dir(&app)?;
    let result = peercove_ops::join::join(&token, &dir, force).map_err(to_message)?;
    Ok(JoinResult {
        config_path: result.config_path.display().to_string(),
        name: result.name,
        address: result.address.to_string(),
        endpoint: result.endpoint.to_string(),
        other_endpoints: result
            .other_endpoints
            .iter()
            .map(|e| e.to_string())
            .collect(),
    })
}

/// メンバーを削除する(本人へ通知後、約 10 秒でトンネルから除外される)。
#[tauri::command]
fn remove_member(config_path: String, public_key: String) -> Result<String, String> {
    let removed = peercove_ops::peers::remove_peer(
        Path::new(&config_path),
        &Selector::PublicKey(&public_key),
    )
    .map_err(to_message)?;
    Ok(removed.display)
}

/// メンバーの表示名を変更する(台帳に反映される)。
#[tauri::command]
fn rename_member(config_path: String, public_key: String, new_name: String) -> Result<(), String> {
    peercove_ops::peers::rename_peer(
        Path::new(&config_path),
        &Selector::PublicKey(&public_key),
        new_name.trim(),
    )
    .map_err(to_message)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .invoke_handler(tauri::generate_handler![
            daemon_status,
            start_host,
            start_member,
            stop_tunnel,
            config_paths,
            init_host,
            create_invite,
            join_network,
            remove_member,
            rename_member,
        ])
        .run(tauri::generate_context!())
        .expect("Tauri アプリの起動に失敗しました");
}
