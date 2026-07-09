//! PeerCove デスクトップ UI の Rust 側(M2-G2〜G6)。
//!
//! UI は**非特権**で動く。役割は 2 つ:
//!
//! 1. **デーモン操作**(要特権のトンネル操作): ローカル IPC 経由(ADR-0007)
//! 2. **設定ファイル操作**(init / invite / join / メンバー管理 / 設定編集):
//!    `peercove-ops` をユーザー権限で直接呼ぶ。デーモンには 5 秒の再読込で
//!    自動反映される(ADR-0008)
//!
//! IPC の応答は `dto` で UI 用 DTO(camelCase)へ変換してから frontend に渡す。
//!
//! トレイ常駐(M2-G6)は `tray` モジュール。ウィンドウを閉じてもプロセスは
//! 残り、トレイから復帰・終了できる。

mod dto;
mod tray;

use std::net::SocketAddrV4;
use std::path::{Path, PathBuf};

use peercove_core::ipc::{IpcRequest, IpcResponse};
use peercove_ops::peers::Selector;
use tauri::Manager;

use crate::dto::{
    ConfigPaths, ConfigSlot, InitResult, InviteResult, JoinResult, LogEntry, Logs, SaveResult,
    Settings, SettingsUpdate, Status,
};

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

// ---- 通知(M2-G6) ----

/// OS 通知を出す(メンバーの参加・切断)。
///
/// frontend から `@tauri-apps/plugin-notification` を直接呼ばず Rust 経由にしている:
/// JS の依存を 1 つ減らせるうえ、デスクトップでは許可の問い合わせも不要なため。
/// 通知が出せない環境(通知デーモンが無い等)でも UI は止めない。
#[tauri::command]
fn notify(app: tauri::AppHandle, title: String, body: String) {
    use tauri_plugin_notification::NotificationExt;
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        eprintln!("通知を表示できませんでした: {e}");
    }
}

/// デーモンが保持する直近のログを取り出す(M2-G5)。
///
/// `after_seq` に前回の最終 seq を渡すと差分だけが返る。UI はこれを 1 秒間隔で
/// 呼び、ログビューが開いている間だけ追記していく。
#[tauri::command]
async fn daemon_logs(after_seq: u64) -> Result<Logs, String> {
    match peercove_ipc::request_async(IpcRequest::Logs { after_seq }).await {
        Ok(IpcResponse::Logs { lines, dropped }) => Ok(Logs {
            lines: lines.into_iter().map(LogEntry::from).collect(),
            dropped,
        }),
        Ok(other) => Err(format!("想定外の応答です: {other:?}")),
        Err(e) => Err(to_message(e)),
    }
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
///
/// M3-0a: 実体は `networks/<スラッグ>/` 配下(ADR-0012)。旧配置(直下の
/// host.toml / member.toml)はここで自動移行する。現在の UI は 1 ホスト +
/// 1 メンバーの 2 スロット表示なので、一覧の先頭をスロットに割り当てる
/// (一覧 UI 化は M3-0c)。
#[tauri::command]
fn config_paths(app: tauri::AppHandle) -> Result<ConfigPaths, String> {
    let dir = config_dir(&app)?;
    // 旧配置からの移行。失敗しても UI を止めない(次回また試みる)
    if let Err(e) = peercove_ops::networks::migrate_legacy(&dir) {
        eprintln!("旧設定の移行に失敗しました: {e:#}");
    }
    let networks = peercove_ops::networks::list(&dir);
    let slot = |role: peercove_ops::networks::Role, fallback: &str| {
        networks
            .iter()
            .find(|n| n.role == role)
            .map(|n| ConfigSlot::of(&n.config_path))
            .unwrap_or_else(|| {
                ConfigSlot::of(
                    &peercove_ops::networks::networks_dir(&dir)
                        .join(peercove_core::names::DEFAULT_NETWORK_NAME)
                        .join(fallback),
                )
            })
    };
    Ok(ConfigPaths {
        host: slot(peercove_ops::networks::Role::Host, "host.toml"),
        member: slot(peercove_ops::networks::Role::Member, "member.toml"),
        dir: dir.display().to_string(),
    })
}

/// ホストを初期化する(鍵とランダムサブネットの host.toml を生成)。
///
/// M3-0a: 書き込み先は `networks/<既定名>/`。名前の入力 UI は M3-0c で付ける。
#[tauri::command]
fn init_host(app: tauri::AppHandle, force: bool) -> Result<InitResult, String> {
    let base = config_dir(&app)?;
    let name = peercove_core::names::DEFAULT_NETWORK_NAME;
    let (_, dir) = peercove_ops::networks::network_dir(&base, name).map_err(to_message)?;
    let result = peercove_ops::init::init_host(
        &dir,
        name,
        peercove_core::config::DEFAULT_LISTEN_PORT,
        force,
    )
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
///
/// M3-0a: 書き込み先はトークンのネットワーク名から決まる
/// `networks/<スラッグ>/`(旧トークンは既定名)。
#[tauri::command]
fn join_network(app: tauri::AppHandle, token: String, force: bool) -> Result<JoinResult, String> {
    let base = config_dir(&app)?;
    let parsed = peercove_core::token::InviteToken::parse(&token).map_err(|e| e.to_string())?;
    let name = parsed
        .network
        .as_deref()
        .unwrap_or(peercove_core::names::DEFAULT_NETWORK_NAME);
    let (_, dir) = peercove_ops::networks::join_dir(&base, name).map_err(to_message)?;
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

/// 設定ファイルの現在値を読む(M2-G5)。
#[tauri::command]
fn read_settings(config_path: String) -> Result<Settings, String> {
    peercove_ops::settings::read(Path::new(&config_path))
        .map(Settings::from)
        .map_err(to_message)
}

/// 設定ファイルを書き戻す。`restartRequired` が true なら、再接続するまで
/// 反映されない項目(MTU / 待受ポート / ホストのエンドポイント)が変わっている。
#[tauri::command]
fn save_settings(config_path: String, update: SettingsUpdate) -> Result<SaveResult, String> {
    let path = Path::new(&config_path);
    let update: peercove_ops::settings::Update = update.into();
    let current = peercove_ops::settings::read(path).map_err(to_message)?;
    let restart_required = current.restart_required(&update);
    peercove_ops::settings::update(path, &update).map_err(to_message)?;
    Ok(SaveResult { restart_required })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            tray::setup(app.handle())?;
            Ok(())
        })
        // ウィンドウを閉じてもプロセスは残す(トレイ常駐 — M2-G6)。
        // 終了はトレイメニューの「終了」から
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            daemon_status,
            daemon_logs,
            start_host,
            start_member,
            stop_tunnel,
            notify,
            config_paths,
            init_host,
            create_invite,
            join_network,
            remove_member,
            rename_member,
            read_settings,
            save_settings,
        ])
        .run(tauri::generate_context!())
        .expect("Tauri アプリの起動に失敗しました");
}
