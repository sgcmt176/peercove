//! トレイ常駐(M2-G6)。
//!
//! ウィンドウを閉じてもアプリは終了せず、トレイに残る。トンネルはデーモン側で
//! 動き続けるので UI を閉じても切断されないが、UI が生きていないとメンバーの
//! 参加・切断の通知が出せないため、常駐させる意味がある。
//!
//! - 左クリック / メニュー「表示」: ウィンドウを復帰
//! - メニュー「終了」: アプリを終了(デーモンとトンネルはそのまま)
//!
//! Linux ではトレイに libayatana-appindicator が要る(README の前提条件参照)。
//! デスクトップ環境によってはアイコンの左クリックイベントが飛ばないため、
//! メニューからも必ず復帰できるようにしてある。

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Runtime};

/// トレイアイコン。バンドル用の PNG をそのまま埋め込む(外部ファイルに依存しない)。
const ICON_PNG: &[u8] = include_bytes!("../icons/32x32.png");

pub fn setup<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "PeerCove を表示", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "終了", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    TrayIconBuilder::with_id("main")
        .icon(Image::from_bytes(ICON_PNG)?)
        .tooltip("PeerCove")
        .menu(&menu)
        // 左クリックはウィンドウ復帰に使うので、メニューは右クリックだけ
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main_window(app),
            "quit" => app.exit(0),
            id => eprintln!("未知のトレイメニュー項目: {id}"),
        })
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
    Ok(())
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
