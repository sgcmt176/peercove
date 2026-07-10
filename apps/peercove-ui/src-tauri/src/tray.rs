//! トレイ常駐(M2-G6)+ ネットワーク操作(M3-6)。
//!
//! ウィンドウを閉じてもアプリは終了せず、トレイに残る。トンネルはデーモン側で
//! 動き続けるので UI を閉じても切断されないが、UI が生きていないとメンバーの
//! 参加・切断の通知が出せないため、常駐させる意味がある。
//!
//! - 左クリック / メニュー「表示」: ウィンドウを復帰
//! - ネットワークごとの「接続」「切断」: ウィンドウを開かずに操作できる(M3-6)。
//!   メニューは 5 秒ごとの更新ループが設定一覧とデーモンの稼働状況から作り直す
//!   (内容が変わったときだけ差し替える — 開いているメニューを閉じさせないため)
//! - メニュー「終了」: アプリを終了(デーモンとトンネルはそのまま)
//!
//! Linux ではトレイに libayatana-appindicator が要る(README の前提条件参照)。
//! デスクトップ環境によってはアイコンの左クリックイベントが飛ばないため、
//! メニューからも必ず復帰できるようにしてある。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use peercove_core::ipc::{IpcRequest, IpcResponse};
use tauri::image::Image;
use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime};

/// トレイアイコン。バンドル用の PNG をそのまま埋め込む(外部ファイルに依存しない)。
const ICON_PNG: &[u8] = include_bytes!("../icons/32x32.png");

/// メニュー・ツールチップの更新間隔。UI 本体のポーリング(2 秒)より粗くてよい。
const REFRESH_INTERVAL: Duration = Duration::from_secs(5);

pub fn setup<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    // 起動直後は「表示 / 終了」だけ。ネットワーク項目は更新ループが足す
    let menu = build_menu(app, &[])?;

    TrayIconBuilder::with_id("main")
        .icon(Image::from_bytes(ICON_PNG)?)
        .tooltip("PeerCove")
        .menu(&menu)
        // 左クリックはウィンドウ復帰に使うので、メニューは右クリックだけ
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| handle_menu(app, event.id.as_ref()))
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;

    // ネットワークの接続/切断メニューと稼働状況ツールチップの更新ループ(M3-6)
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut last_signature = String::new();
        loop {
            refresh_tray(&handle, &mut last_signature).await;
            tokio::time::sleep(REFRESH_INTERVAL).await;
        }
    });
    Ok(())
}

/// メニューに載せるネットワーク項目(表示ラベルとメニュー ID)。
struct NetworkItem {
    id: String,
    label: String,
}

/// 設定一覧とデーモンの稼働状況からトレイを作り直す。
///
/// 内容のシグネチャが前回と同じなら何もしない(メニューを開いている最中に
/// 差し替えると OS によっては閉じてしまうため、無変更時の再設定を避ける)。
async fn refresh_tray<R: Runtime>(app: &AppHandle<R>, last_signature: &mut String) {
    let Ok(base) = app.path().app_config_dir() else {
        return;
    };
    let networks = peercove_ops::networks::list(&base);

    // 稼働中の設定パス。デーモンに届かない場合は全て停止扱いにする
    // (接続項目は残す — クリック時にデーモン不達のエラー通知が出る)
    let running: HashSet<String> = match peercove_ipc::request_async(IpcRequest::Status).await {
        Ok(IpcResponse::Status(status)) => status
            .tunnels
            .iter()
            .map(|tunnel| comparable_path(&tunnel.config))
            .collect(),
        _ => HashSet::new(),
    };

    let items: Vec<NetworkItem> = networks
        .iter()
        .map(|network| {
            // デーモンと同じ正規化を経由して稼働状況と突き合わせる(dto.rs と同じ理屈)
            let canonical = std::fs::canonicalize(&network.config_path)
                .unwrap_or_else(|_| network.config_path.clone());
            let is_running = running.contains(&comparable_path(&canonical));
            let path = canonical.to_string_lossy();
            if is_running {
                NetworkItem {
                    id: format!("stop:{path}"),
                    label: format!("「{}」を切断", network.name),
                }
            } else {
                let action = match network.role {
                    peercove_ops::networks::Role::Host => "start-host",
                    peercove_ops::networks::Role::Member => "start-member",
                };
                NetworkItem {
                    id: format!("{action}:{path}"),
                    label: format!("「{}」に接続", network.name),
                }
            }
        })
        .collect();

    let tooltip = if running.is_empty() {
        "PeerCove — 停止中".to_string()
    } else {
        format!("PeerCove — {} ネットワーク稼働中", running.len())
    };

    let signature = items
        .iter()
        .map(|item| item.id.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let signature = format!("{tooltip}\n{signature}");
    if signature == *last_signature {
        return;
    }

    // メニューの構築・差し替えはメインスレッドで行う(Linux の GTK 制約。
    // Windows は不問だが、共通経路にしておく)
    let handle = app.clone();
    let dispatched = app.run_on_main_thread(move || {
        let Some(tray) = handle.tray_by_id("main") else {
            return;
        };
        match build_menu(&handle, &items) {
            Ok(menu) => {
                if let Err(e) = tray.set_menu(Some(menu)) {
                    eprintln!("トレイメニューを更新できませんでした: {e}");
                    return;
                }
                let _ = tray.set_tooltip(Some(&tooltip));
            }
            Err(e) => eprintln!("トレイメニューを構築できませんでした: {e}"),
        }
    });
    if dispatched.is_ok() {
        *last_signature = signature;
    }
}

/// 突き合わせ用のパス文字列。Windows の canonicalize が付ける verbatim
/// 接頭辞(`\\?\`)の有無に左右されないよう剥がして比べる。
fn comparable_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    text.strip_prefix(r"\\?\").unwrap_or(&text).to_string()
}

fn build_menu<R: Runtime>(app: &AppHandle<R>, networks: &[NetworkItem]) -> tauri::Result<Menu<R>> {
    let show = MenuItem::with_id(app, "show", "PeerCove を表示", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "終了", true, None::<&str>)?;
    let separator_top = PredefinedMenuItem::separator(app)?;
    let separator_bottom = PredefinedMenuItem::separator(app)?;
    let network_items: Vec<MenuItem<R>> = networks
        .iter()
        .map(|item| MenuItem::with_id(app, &item.id, &item.label, true, None::<&str>))
        .collect::<Result<_, _>>()?;

    let mut refs: Vec<&dyn IsMenuItem<R>> = vec![&show];
    if !network_items.is_empty() {
        refs.push(&separator_top);
        for item in &network_items {
            refs.push(item);
        }
    }
    refs.push(&separator_bottom);
    refs.push(&quit);
    Menu::with_items(app, &refs)
}

fn handle_menu<R: Runtime>(app: &AppHandle<R>, id: &str) {
    match id {
        "show" => show_main_window(app),
        "quit" => app.exit(0),
        _ => {
            if let Some(path) = id.strip_prefix("stop:") {
                send_request(
                    app,
                    IpcRequest::Stop {
                        config: Some(PathBuf::from(path)),
                    },
                    "切断できませんでした",
                );
            } else if let Some(path) = id.strip_prefix("start-host:") {
                // UPnP は UI の接続フォームの既定値(オン)に合わせる
                send_request(
                    app,
                    IpcRequest::StartHost {
                        config: PathBuf::from(path),
                        upnp: true,
                    },
                    "接続できませんでした",
                );
            } else if let Some(path) = id.strip_prefix("start-member:") {
                send_request(
                    app,
                    IpcRequest::StartMember {
                        config: PathBuf::from(path),
                    },
                    "接続できませんでした",
                );
            } else {
                eprintln!("未知のトレイメニュー項目: {id}");
            }
        }
    }
}

/// デーモンへの要求を裏で送る。失敗はウィンドウが見えていない前提なので
/// OS 通知で知らせる(標準エラーにも残す)。
fn send_request<R: Runtime>(app: &AppHandle<R>, request: IpcRequest, context: &'static str) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = peercove_ipc::request_async(request).await {
            use tauri_plugin_notification::NotificationExt;
            eprintln!("{context}: {e:#}");
            let _ = app
                .notification()
                .builder()
                .title(context)
                .body(format!("{e:#}"))
                .show();
        }
    });
}

/// ウィンドウを表示して前面に出す(最小化されていても復帰する)。
pub fn show_main_window<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();
}
