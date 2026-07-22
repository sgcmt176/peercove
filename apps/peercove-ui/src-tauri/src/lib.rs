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
mod linkmeta;
mod tray;
mod update;

use std::net::SocketAddrV4;
use std::path::{Path, PathBuf};

use peercove_core::diagnostics::DiagnosticReport;
use peercove_core::ipc::{IpcRequest, IpcResponse};
use peercove_ops::peers::Selector;
use tauri::Manager;

use crate::dto::{
    ChatMessage, ChatPage, DnsRecordDto, InboxItem, InitResult, InviteResult, JoinResult, LogEntry,
    Logs, NetworkDto, SaveResult, Settings, SettingsUpdate, Status,
};

/// デフォルトの設定ディレクトリ(Windows: %APPDATA%\… / Linux: ~/.config/…)。
/// UI からは「別の設定を使う」で任意のファイルも選べる(ADR-0008)。
fn config_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map_err(|e| format!("設定ディレクトリを特定できません: {e}"))
}

// ---- 個人メモ (M5 F-1, ADR-0049) ----

/// 個人メモ DB のパス。ネットワーク非依存なので設定ディレクトリ直下に置く。
/// DB を所有するのはデーモン(ADR-0049)— UI はパスを決めて IPC で操作するだけ。
fn memo_db(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(config_dir(app)?.join("memos.db"))
}

async fn memo_request(
    db: PathBuf,
    op: peercove_core::memo::MemoOp,
) -> Result<peercove_core::memo::MemoReply, String> {
    match peercove_ipc::request_async(IpcRequest::Memo { db, op }).await {
        Ok(IpcResponse::Memo { reply }) => Ok(reply),
        Ok(other) => Err(format!("想定外の応答です: {other:?}")),
        // 旧デーモンは Memo メソッドを解析できず Err を返す(IPC_VERSION 据え置き)
        Err(e) => Err(to_message(e)),
    }
}

#[tauri::command]
async fn memo_op(
    app: tauri::AppHandle,
    op: peercove_core::memo::MemoOp,
) -> Result<peercove_core::memo::MemoReply, String> {
    memo_request(memo_db(&app)?, op).await
}

/// メモ 1 件を `.txt` へ保存する(要件 §16。本文の受け渡しであり、タグ等は
/// 含まれない)。保存先は OS のダイアログで選ぶ。None = キャンセル。
#[tauri::command]
async fn memo_export(app: tauri::AppHandle, id: String) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let reply = memo_request(memo_db(&app)?, peercove_core::memo::MemoOp::Get { id }).await?;
    let peercove_core::memo::MemoReply::Memo { memo } = reply else {
        return Err("想定外の応答です".to_string());
    };
    let suggested = format!(
        "{}.txt",
        peercove_core::memo::sanitize_filename(&memo.title)
    );
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .add_filter("テキスト", &["txt"])
            .set_file_name(&suggested)
            .blocking_save_file()
            .map(|path| path.to_string())
    })
    .await
    .map_err(|e| format!("ダイアログの表示に失敗しました: {e}"))?;
    let Some(output) = picked else {
        return Ok(None);
    };
    std::fs::write(&output, memo.body.as_bytes())
        .map_err(|e| format!("書き込みに失敗しました: {e}"))?;
    Ok(Some(output))
}

/// `.txt` を個人メモとして取り込む(要件 §16。ファイル名がタイトル、本文が
/// メモ本文)。複数選択可。None = キャンセル、Some(n) = 取り込んだ件数。
#[tauri::command]
async fn memo_import(
    app: tauri::AppHandle,
    folder_id: Option<String>,
) -> Result<Option<u32>, String> {
    use tauri_plugin_dialog::DialogExt;
    let db = memo_db(&app)?;
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .add_filter("テキスト", &["txt"])
            .blocking_pick_files()
            .map(|paths| {
                paths
                    .into_iter()
                    .map(|path| path.to_string())
                    .collect::<Vec<_>>()
            })
    })
    .await
    .map_err(|e| format!("ダイアログの表示に失敗しました: {e}"))?;
    let Some(paths) = picked else {
        return Ok(None);
    };
    let mut imported = 0u32;
    for path in paths {
        let path = Path::new(&path);
        let bytes = std::fs::read(path).map_err(|e| format!("ファイルを読み込めません: {e}"))?;
        let body = String::from_utf8_lossy(&bytes).into_owned();
        let title = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        memo_request(
            db.clone(),
            peercove_core::memo::MemoOp::Create {
                title,
                body,
                folder_id: folder_id.clone(),
                tags: vec![],
            },
        )
        .await?;
        imported += 1;
    }
    Ok(Some(imported))
}

// ---- 共有メモ (M5 F-2, ADR-0049) ----

/// 共有メモの操作。host はデーモンが正本を直接、member は読み取りをキャッシュ、
/// 変更をコントロールチャネル経由でホストへ届ける(判定はすべてホスト正本)。
#[tauri::command]
async fn shared_memo_op(
    config_path: String,
    op: peercove_core::memo::SharedMemoOp,
) -> Result<peercove_core::memo::SharedMemoReply, String> {
    match peercove_ipc::request_async(IpcRequest::SharedMemo {
        config: PathBuf::from(&config_path),
        op,
    })
    .await
    {
        Ok(IpcResponse::SharedMemo { reply }) => Ok(reply),
        Ok(other) => Err(format!("想定外の応答です: {other:?}")),
        Err(e) => Err(to_message(e)),
    }
}

/// 共有メモ 1 件を `.txt` へ保存する(個人メモの memo_export と同型)。
#[tauri::command]
async fn shared_memo_export(
    app: tauri::AppHandle,
    config_path: String,
    id: String,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let reply = match peercove_ipc::request_async(IpcRequest::SharedMemo {
        config: PathBuf::from(&config_path),
        op: peercove_core::memo::SharedMemoOp::Get { id },
    })
    .await
    {
        Ok(IpcResponse::SharedMemo { reply }) => reply,
        Ok(other) => return Err(format!("想定外の応答です: {other:?}")),
        Err(e) => return Err(to_message(e)),
    };
    let peercove_core::memo::SharedMemoReply::Memo { memo } = reply else {
        return Err("想定外の応答です".to_string());
    };
    let suggested = format!(
        "{}.txt",
        peercove_core::memo::sanitize_filename(&memo.title)
    );
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .add_filter("テキスト", &["txt"])
            .set_file_name(&suggested)
            .blocking_save_file()
            .map(|path| path.to_string())
    })
    .await
    .map_err(|e| format!("ダイアログの表示に失敗しました: {e}"))?;
    let Some(output) = picked else {
        return Ok(None);
    };
    std::fs::write(&output, memo.body.as_bytes())
        .map_err(|e| format!("書き込みに失敗しました: {e}"))?;
    Ok(Some(output))
}

// ---- 暗号化バックアップ / 復元 (M3-24, ADR-0034) ----

#[tauri::command]
async fn create_backup(
    app: tauri::AppHandle,
    config_path: String,
    passphrase: String,
    // 共有メモ(M5 F-3)を同梱するか。既存呼び出しを壊さないよう Option 化し
    // 未指定時は同梱する(バックアップは網羅的である方が安全側)
    include_memos: Option<bool>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let network = peercove_core::config::Config::load(Path::new(&config_path))
        .map_err(|e| e.to_string())?
        .network_name()
        .to_string();
    let suggested = format!("{network}.pcvbackup");
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .add_filter("PeerCove backup", &["pcvbackup"])
            .set_file_name(&suggested)
            .blocking_save_file()
            .map(|path| path.to_string())
    })
    .await
    .map_err(|e| format!("ダイアログの表示に失敗しました: {e}"))?;
    let Some(output) = picked else {
        return Ok(None);
    };
    peercove_ops::backup::create(
        Path::new(&config_path),
        Path::new(&output),
        &passphrase,
        include_memos.unwrap_or(true),
    )
    .map_err(to_message)?;
    Ok(Some(output))
}

#[tauri::command]
async fn pick_backup(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .add_filter("PeerCove backup", &["pcvbackup"])
            .blocking_pick_file()
            .map(|path| path.to_string())
    })
    .await
    .map_err(|e| format!("ダイアログの表示に失敗しました: {e}"))
}

#[tauri::command]
fn inspect_backup(
    path: String,
    passphrase: String,
) -> Result<peercove_ops::backup::BackupPreview, String> {
    peercove_ops::backup::inspect(Path::new(&path), &passphrase).map_err(to_message)
}

#[tauri::command]
async fn restore_backup(
    app: tauri::AppHandle,
    path: String,
    passphrase: String,
    slug: String,
    replace: bool,
) -> Result<String, String> {
    let base = config_dir(&app)?;
    let target = peercove_ops::networks::networks_dir(&base).join(&slug);
    if let Ok(IpcResponse::Status(status)) = peercove_ipc::request_async(IpcRequest::Status).await {
        let target = std::fs::canonicalize(&target).unwrap_or(target);
        if status.tunnels.iter().any(|tunnel| {
            let parent = tunnel.config.parent().unwrap_or(Path::new(""));
            std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf()) == target
        }) {
            return Err("稼働中のネットワークは置換できません。先に切断してください".to_string());
        }
    }
    let mode = if replace {
        peercove_ops::backup::RestoreMode::Replace
    } else {
        peercove_ops::backup::RestoreMode::New
    };
    peercove_ops::backup::restore(Path::new(&path), &passphrase, &base, &slug, mode)
        .map(|result| result.config_path.display().to_string())
        .map_err(to_message)
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

/// GitHub Releases の最新安定版を確認する。失敗は UI の接続状態に影響させない。
#[tauri::command]
async fn check_update() -> Result<update::UpdateInfo, String> {
    update::check().await.map_err(to_message)
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

/// トンネルを停止する(デーモンは常駐継続)。複数ネットワーク対応(ADR-0012)の
/// ため停止対象の設定パスを指定する。
#[tauri::command]
async fn stop_tunnel(config_path: String) -> Result<(), String> {
    let config = canonical(&config_path)?;
    send(IpcRequest::Stop {
        config: Some(config),
    })
    .await
}

/// (member)デバイス鍵のローテーションを要求する(ADR-0020、M3-11)。
/// 応答は受理のみ。実際の更新はコントロールチャネル経由で非同期に行われ、
/// 完了時に数秒の再接続が発生する(結果はログに出る)。
#[tauri::command]
async fn rotate_key(config_path: String) -> Result<(), String> {
    let config = canonical(&config_path)?;
    send(IpcRequest::RotateKey { config }).await
}

/// (member)自分の DNS 名の変更を要求する(ADR-0021、M3-14a)。
/// デーモンがコントロールチャネルでホストへ届け、検証・適用の結果を待つ。
/// 拒否(重複・予約語)やタイムアウトはエラー文字列で返る。
#[tauri::command]
async fn set_my_dns_name(config_path: String, dns_name: String) -> Result<(), String> {
    let config = canonical(&config_path)?;
    send(IpcRequest::SetDnsName {
        config,
        name: dns_name,
    })
    .await
}

/// (member)自分の表示名の変更を要求する(ADR-0027、M3-19)。DNS 名変更と同じく
/// デーモンがコントロールチャネルでホストへ届け、検証・適用の結果を待つ。
/// 拒否(空・重複)やタイムアウトはエラー文字列で返る。
#[tauri::command]
async fn set_my_display_name(config_path: String, display_name: String) -> Result<(), String> {
    let config = canonical(&config_path)?;
    send(IpcRequest::SetDisplayName {
        config,
        name: display_name,
    })
    .await
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

/// 指定ネットワークの読み取り専用診断をデーモンに依頼する(M3-21)。
#[tauri::command]
async fn diagnose_network(config_path: String) -> Result<DiagnosticReport, String> {
    let config = canonical(&config_path)?;
    match peercove_ipc::request_async(IpcRequest::Diagnose { config }).await {
        Ok(IpcResponse::Diagnostic { report }) => Ok(report),
        Ok(other) => Err(format!("想定外の応答です: {other:?}")),
        Err(error) => Err(to_message(error)),
    }
}

#[tauri::command]
async fn quality_history(
    config_path: String,
    since_unix_ms: u64,
) -> Result<dto::QualityReport, String> {
    let config = canonical(&config_path)?;
    match peercove_ipc::request_async(IpcRequest::Quality {
        config,
        since_unix_ms,
    })
    .await
    {
        Ok(IpcResponse::Quality { report }) => Ok(report.into()),
        Ok(other) => Err(format!("想定外の応答です: {other:?}")),
        Err(e) => Err(to_message(e)),
    }
}

// ---- ファイル送信・受信ボックス(ADR-0015、M3-9b) ----

/// 送るファイルを選ぶ(OS のファイルダイアログ)。キャンセルで None。
///
/// ダイアログはブロッキングなので専用スレッドで開く(status ポーリング等の
/// 非同期コマンドを塞がない)。
#[tauri::command]
async fn pick_file(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .blocking_pick_file()
            .map(|path| path.to_string())
    })
    .await
    .map_err(|e| format!("ダイアログの表示に失敗しました: {e}"))
}

/// クリップボードから貼り付けた画像(base64)を一時ファイルへ書き出し、その
/// パスを返す。デーモンはファイルをパスで読むため、送信前に実体化が要る。
/// 一時ファイルは OS の temp 配下(peercove-paste/<ミリ秒>/)に置く。
#[tauri::command]
fn save_pasted_file(name: String, data_base64: String) -> Result<String, String> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data_base64.as_bytes())
        .map_err(|e| format!("貼り付けデータを復号できません: {e}"))?;
    // ファイル名はベース名だけにする(パス区切りを剥がす)
    let safe = Path::new(&name)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("pasted");
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let dir = std::env::temp_dir()
        .join("peercove-paste")
        .join(ms.to_string());
    std::fs::create_dir_all(&dir).map_err(|e| format!("一時フォルダを作れません: {e}"))?;
    let path = dir.join(safe);
    std::fs::write(&path, &bytes).map_err(|e| format!("一時ファイルの書き出しに失敗: {e}"))?;
    Ok(path.to_string_lossy().to_string())
}

/// メンバーへファイルを送る(デーモンが送信し、進捗は status の transfers)。
/// `chat` を付けるとチャット内ファイル送信になり、履歴にも記録される
/// (M3-13d。network / group 宛は peer 省略)。
#[tauri::command]
async fn send_file(
    config_path: String,
    peer: Option<String>,
    path: String,
    chat: Option<dto::ChatContextDto>,
) -> Result<String, String> {
    let config = canonical(&config_path)?;
    let peer = match peer {
        Some(peer) => Some(
            peer.parse::<std::net::Ipv4Addr>()
                .map_err(|_| format!("宛先 {peer} は IPv4 アドレスではありません"))?,
        ),
        None => None,
    };
    let chat = match chat {
        Some(dto) => Some(peercove_core::msg::ChatContext::try_from(dto)?),
        None => None,
    };
    match peercove_ipc::request_async(IpcRequest::SendFile {
        config,
        peer,
        path: PathBuf::from(path),
        chat,
    })
    .await
    {
        Ok(IpcResponse::Transfer { id }) => Ok(id),
        Ok(other) => Err(format!("想定外の応答です: {other:?}")),
        Err(e) => Err(to_message(e)),
    }
}

// ---- チャット(ADR-0016、M3-13b/c) ----

/// チャットを送る(ADR-0016)。`peer` 指定で 1:1、`group` 指定でグループ宛、
/// どちらも省略でネットワーク全体宛。戻り値は履歴に記録された 1 通
/// (UI が即座に吹き出しへ足す)。
#[tauri::command]
async fn chat_send(
    config_path: String,
    peer: Option<String>,
    group: Option<String>,
    text: String,
) -> Result<ChatMessage, String> {
    use peercove_core::msg::ChatScope;
    let config = canonical(&config_path)?;
    let (scope, peer, group_id) = match (peer, group) {
        (Some(peer), None) => {
            let ip: std::net::Ipv4Addr = peer
                .parse()
                .map_err(|_| format!("宛先 {peer} は IPv4 アドレスではありません"))?;
            (ChatScope::Direct, Some(ip), None)
        }
        (None, Some(group)) => (ChatScope::Group, None, Some(group)),
        (None, None) => (ChatScope::Network, None, None),
        (Some(_), Some(_)) => {
            return Err("宛先は peer か group のどちらか 1 つにしてください".to_string())
        }
    };
    match peercove_ipc::request_async(IpcRequest::ChatSend {
        config,
        scope,
        peer,
        group_id,
        text,
    })
    .await
    {
        Ok(IpcResponse::Chat { messages, .. }) => messages
            .first()
            .map(ChatMessage::from)
            .ok_or_else(|| "デーモンが送信結果を返しませんでした".to_string()),
        Ok(other) => Err(format!("想定外の応答です: {other:?}")),
        Err(e) => Err(to_message(e)),
    }
}

/// 送信待ち(または失敗した)チャットを再送する(E-E 3 のデスクトップ版)。
#[tauri::command]
async fn chat_resend(config_path: String, seq: u64) -> Result<(), String> {
    let config = canonical(&config_path)?;
    match peercove_ipc::request_async(IpcRequest::ChatResend { config, seq }).await {
        Ok(IpcResponse::Done) => Ok(()),
        Ok(other) => Err(format!("想定外の応答です: {other:?}")),
        Err(e) => Err(to_message(e)),
    }
}

/// 送信待ちチャットの自動再送をやめる(履歴には失敗の印を残す)。
#[tauri::command]
async fn chat_cancel_send(config_path: String, seq: u64) -> Result<(), String> {
    let config = canonical(&config_path)?;
    match peercove_ipc::request_async(IpcRequest::ChatCancelSend { config, seq }).await {
        Ok(IpcResponse::Done) => Ok(()),
        Ok(other) => Err(format!("想定外の応答です: {other:?}")),
        Err(e) => Err(to_message(e)),
    }
}

/// 仮想 IP 文字列の一覧を検証つきで変換する(グループ操作用)。
fn parse_ips(list: Vec<String>) -> Result<Vec<std::net::Ipv4Addr>, String> {
    list.iter()
        .map(|ip| {
            ip.parse()
                .map_err(|_| format!("{ip} は IPv4 アドレスではありません"))
        })
        .collect()
}

/// グループを作る(ADR-0016、M3-13c)。members は相手の仮想 IP(自分は不要)。
#[tauri::command]
async fn group_create(
    config_path: String,
    name: String,
    members: Vec<String>,
) -> Result<dto::Group, String> {
    let config = canonical(&config_path)?;
    let members = parse_ips(members)?;
    match peercove_ipc::request_async(IpcRequest::GroupCreate {
        config,
        name,
        members,
    })
    .await
    {
        Ok(IpcResponse::Group { group }) => Ok(dto::Group::from(&group)),
        Ok(other) => Err(format!("想定外の応答です: {other:?}")),
        Err(e) => Err(to_message(e)),
    }
}

/// グループの改名・メンバー追加・メンバー除外(すべて省略可)。
#[tauri::command]
async fn group_update(
    config_path: String,
    id: String,
    name: Option<String>,
    add: Vec<String>,
    remove: Vec<String>,
) -> Result<dto::Group, String> {
    let config = canonical(&config_path)?;
    let add = parse_ips(add)?;
    let remove = parse_ips(remove)?;
    match peercove_ipc::request_async(IpcRequest::GroupUpdate {
        config,
        id,
        name,
        add,
        remove,
    })
    .await
    {
        Ok(IpcResponse::Group { group }) => Ok(dto::Group::from(&group)),
        Ok(other) => Err(format!("想定外の応答です: {other:?}")),
        Err(e) => Err(to_message(e)),
    }
}

/// 自分がグループから抜ける(履歴はローカルに残る)。
#[tauri::command]
async fn group_leave(config_path: String, id: String) -> Result<(), String> {
    let config = canonical(&config_path)?;
    send(IpcRequest::GroupLeave { config, id }).await
}

/// チャット履歴の差分を取る(`after_seq` より後)。1 応答には上限があるため、
/// 返ったページの末尾 seq が `seq` に届くまで繰り返し呼ぶ。
#[tauri::command]
async fn chat_fetch(config_path: String, after_seq: u64) -> Result<ChatPage, String> {
    let config = canonical(&config_path)?;
    match peercove_ipc::request_async(IpcRequest::ChatFetch { config, after_seq }).await {
        Ok(IpcResponse::Chat { seq, messages }) => Ok(ChatPage {
            seq,
            messages: messages.iter().map(ChatMessage::from).collect(),
        }),
        Ok(other) => Err(format!("想定外の応答です: {other:?}")),
        Err(e) => Err(to_message(e)),
    }
}

/// 受信ボックスのディレクトリ(`networks/<net>.inbox/`。デーモン側と同じ規則)。
fn inbox_dir_for(config_path: &str) -> PathBuf {
    Path::new(config_path).with_extension("inbox")
}

/// 受信ボックス内のファイルを名前で引く。パス区切りや `..` は拒否する
/// (名前は list_inbox が返したものに限る)。
fn inbox_file(config_path: &str, name: &str) -> Result<PathBuf, String> {
    if name.is_empty() || name == ".." || name.contains('/') || name.contains('\\') {
        return Err("ファイル名が不正です".to_string());
    }
    Ok(inbox_dir_for(config_path).join(name))
}

/// 受信ボックスの一覧(新しい順)。ディレクトリが無ければ空。
#[tauri::command]
fn list_inbox(config_path: String) -> Result<Vec<InboxItem>, String> {
    let dir = inbox_dir_for(&config_path);
    let mut items = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(items); // まだ何も受信していない
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        // 書きかけ(.part)・受信メタ(.pcvmeta)・再開の目印(.pcvresume)は
        // 一覧に出さない
        if name.ends_with(".part") || name.ends_with(".pcvmeta") || name.ends_with(".pcvresume") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let sidecar = std::fs::read_to_string(dir.join(format!("{name}.pcvmeta")))
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok());
        let field = |key: &str| {
            sidecar
                .as_ref()
                .and_then(|v| v[key].as_str())
                .map(String::from)
        };
        items.push(InboxItem {
            size: meta.len(),
            from_name: field("from_name"),
            from_ip: field("from_ip"),
            received_unix_ms: sidecar
                .as_ref()
                .and_then(|v| v["received_unix_ms"].as_u64()),
            name,
        });
    }
    items.sort_by_key(|item| std::cmp::Reverse(item.received_unix_ms));
    Ok(items)
}

/// 受信ボックスのファイルを保存する(保存先ダイアログ → コピー →
/// 受信ボックスから削除)。キャンセルで None。
#[tauri::command]
async fn save_inbox_file(
    app: tauri::AppHandle,
    config_path: String,
    name: String,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let source = inbox_file(&config_path, &name)?;
    let suggested = name.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_file_name(&suggested)
            .blocking_save_file()
            .map(|path| path.to_string())
    })
    .await
    .map_err(|e| format!("ダイアログの表示に失敗しました: {e}"))?;
    let Some(dest) = picked else {
        return Ok(None);
    };
    std::fs::copy(&source, Path::new(&dest)).map_err(|e| format!("保存に失敗しました: {e}"))?;
    // 保存できたら受信ボックスから消す(メタ情報も対で)
    let _ = std::fs::remove_file(&source);
    let _ = std::fs::remove_file(inbox_dir_for(&config_path).join(format!("{name}.pcvmeta")));
    Ok(Some(dest))
}

/// チャットのテキストプレビュー(M3-13e)。先頭だけ読んで返す。
/// NUL を含むファイルはテキストとみなさない(Err → UI は通常のファイル表示)。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TextPreview {
    text: String,
    truncated: bool,
}

#[tauri::command]
fn read_text_preview(path: String) -> Result<TextPreview, String> {
    use std::io::Read as _;
    const MAX_PREVIEW_BYTES: u64 = 256 * 1024;
    let file = std::fs::File::open(&path).map_err(|e| format!("ファイルを開けません: {e}"))?;
    let len = file
        .metadata()
        .map_err(|e| format!("ファイルを読めません: {e}"))?
        .len();
    let mut buf = Vec::new();
    file.take(MAX_PREVIEW_BYTES)
        .read_to_end(&mut buf)
        .map_err(|e| format!("ファイルを読めません: {e}"))?;
    if buf.contains(&0) {
        return Err("テキストファイルではありません".to_string());
    }
    Ok(TextPreview {
        text: String::from_utf8_lossy(&buf).into_owned(),
        truncated: len > MAX_PREVIEW_BYTES,
    })
}

/// 受信ボックスのファイルを削除する(メタ情報も対で)。
#[tauri::command]
fn delete_inbox_file(config_path: String, name: String) -> Result<(), String> {
    let source = inbox_file(&config_path, &name)?;
    if let Err(e) = std::fs::remove_file(&source) {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(format!("削除に失敗しました: {e}"));
        }
    }
    let _ = std::fs::remove_file(inbox_dir_for(&config_path).join(format!("{name}.pcvmeta")));
    Ok(())
}

// ---- チャットのリンク対応(M3-13e、ADR-0017) ----

/// チャット本文の URL を既定ブラウザで開く。http/https だけを許す
/// (チャット由来の文字列で他のスキームを起動させない)。
#[tauri::command]
fn open_link(app: tauri::AppHandle, url: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err("http/https 以外の URL は開けません".to_string());
    }
    app.opener()
        .open_url(&url, None::<&str>)
        .map_err(|e| format!("URL を開けません: {e}"))
}

/// リンクプレビューの結果。`image` は og:image を data URI にしたもの
/// (CSP で外部画像の直読みを許していないため、Rust 側で取得して埋め込む)。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LinkPreviewDto {
    title: Option<String>,
    description: Option<String>,
    site_name: Option<String>,
    image: Option<String>,
}

/// HTML はメタデータが取れれば十分なので先頭だけ読む。
const PREVIEW_HTML_LIMIT: usize = 512 * 1024;
/// プレビュー画像の上限。超えるものは画像なしで返す。
const PREVIEW_IMAGE_LIMIT: usize = 2 * 1024 * 1024;

/// 取得先として禁止する IPv4 か(ループバック・プライベート・CGNAT 等)。
fn ipv4_forbidden(ip: std::net::Ipv4Addr) -> bool {
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.octets()[0] == 0
        // CGNAT(100.64.0.0/10)。PeerCove の既定レンジもここ
        || (ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]))
}

/// 取得先として禁止する IPv6 か。ループバック・未指定・マルチキャストに加え、
/// ULA(fc00::/7)・リンクローカル(fe80::/10)・IPv4-mapped(中の v4 で判定)を弾く。
fn ipv6_forbidden(ip: std::net::Ipv6Addr) -> bool {
    if ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() {
        return true;
    }
    if let Some(v4) = ip.to_ipv4_mapped() {
        return ipv4_forbidden(v4);
    }
    let seg0 = ip.segments()[0];
    (seg0 & 0xfe00) == 0xfc00 // ULA fc00::/7
        || (seg0 & 0xffc0) == 0xfe80 // link-local fe80::/10
}

fn ip_forbidden(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => ipv4_forbidden(v4),
        std::net::IpAddr::V6(v6) => ipv6_forbidden(v6),
    }
}

/// 自動取得してよい URL か(ADR-0017)。チャットに URL を書くだけで相手の
/// 端末が取得しに行くため、ループバック・プライベート等の内部アドレスと
/// 内部向けドメインは拒否する(トンネル内サービスへの意図しないアクセス防止)。
///
/// IP リテラルだけでなく、**ホスト名は解決先 IP も検査**する(内部 IP へ解決する
/// ドメインを使った回避を塞ぐ)。なお、ここで解決した IP と reqwest が接続時に
/// 再解決する IP がずれる DNS リバインディングの残余リスクはある(結果は表示者に
/// しか返らないブラインド SSRF なので、主要な内部到達経路を塞ぐことを主眼とする)。
fn previewable(url: &reqwest::Url) -> Result<(), String> {
    use std::net::ToSocketAddrs;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err("http/https 以外は取得しません".to_string());
    }
    let Some(host) = url.host_str() else {
        return Err("ホスト名がありません".to_string());
    };
    let host = host.trim_matches(['[', ']']);
    if let Ok(ip) = host.parse::<std::net::Ipv4Addr>() {
        return if ipv4_forbidden(ip) {
            Err("内部アドレスは取得しません".to_string())
        } else {
            Ok(())
        };
    }
    if let Ok(ip) = host.parse::<std::net::Ipv6Addr>() {
        return if ipv6_forbidden(ip) {
            Err("内部アドレスは取得しません".to_string())
        } else {
            Ok(())
        };
    }
    // ホスト名: 内部向けの名前を弾いたうえで、解決先 IP を全て検査する。
    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".internal") || lower.ends_with(".local") {
        return Err("内部向けの名前は取得しません".to_string());
    }
    let port = url.port_or_known_default().unwrap_or(80);
    let mut resolved = 0usize;
    for addr in (host, port)
        .to_socket_addrs()
        .map_err(|_| "名前を解決できません".to_string())?
    {
        resolved += 1;
        if ip_forbidden(addr.ip()) {
            return Err("内部アドレスへ解決される URL は取得しません".to_string());
        }
    }
    if resolved == 0 {
        return Err("名前を解決できません".to_string());
    }
    Ok(())
}

/// 応答本体を上限付きで読む。戻りは (読めた分, 上限で打ち切ったか)。
async fn read_capped(mut resp: reqwest::Response, limit: usize) -> Result<(Vec<u8>, bool), String> {
    let mut buf = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("読み込みに失敗しました: {e}"))?
    {
        buf.extend_from_slice(&chunk);
        if buf.len() > limit {
            buf.truncate(limit);
            return Ok((buf, true));
        }
    }
    Ok((buf, false))
}

/// og:image を取得して data URI にする。画像でない・大きすぎる・失敗は None
/// (プレビューは画像なしで出す)。
async fn fetch_preview_image(client: &reqwest::Client, url: reqwest::Url) -> Option<String> {
    let resp = client.get(url).send().await.ok()?;
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)?
        .to_str()
        .ok()?
        .split(';')
        .next()?
        .trim()
        .to_ascii_lowercase();
    if !content_type.starts_with("image/") {
        return None;
    }
    let (bytes, truncated) = read_capped(resp, PREVIEW_IMAGE_LIMIT).await.ok()?;
    if truncated || bytes.is_empty() {
        return None;
    }
    use base64::Engine as _;
    Some(format!(
        "data:{content_type};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    ))
}

/// チャットの URL のページ情報(OGP)を取る(M3-13e、ADR-0017)。
/// サーバーレスのため各端末が表示時に自分で取得する。アプリ設定で
/// オフにできる(呼び出し自体を frontend が抑止する)。
#[tauri::command]
async fn link_preview(url: String) -> Result<LinkPreviewDto, String> {
    let parsed = reqwest::Url::parse(&url).map_err(|_| "URL を解釈できません".to_string())?;
    previewable(&parsed)?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .user_agent("PeerCove/0.1 (link preview)")
        // リダイレクト先にも同じ制限をかける(内部アドレスへ誘導させない)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() > 5 {
                return attempt.error("リダイレクトが多すぎます");
            }
            match previewable(attempt.url()) {
                Ok(()) => attempt.follow(),
                Err(_) => attempt.stop(),
            }
        }))
        .build()
        .map_err(|e| format!("HTTP クライアントを初期化できません: {e}"))?;

    let resp = client
        .get(parsed)
        .send()
        .await
        .map_err(|e| format!("取得できません: {e}"))?;
    let final_url = resp.url().clone();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !content_type.starts_with("text/html") && !content_type.starts_with("application/xhtml") {
        return Err("HTML のページではありません".to_string());
    }
    let (bytes, _) = read_capped(resp, PREVIEW_HTML_LIMIT).await?;
    let meta = linkmeta::extract(&String::from_utf8_lossy(&bytes));

    let mut image = None;
    if let Some(src) = &meta.image {
        if let Ok(img_url) = final_url.join(src) {
            if previewable(&img_url).is_ok() {
                image = fetch_preview_image(&client, img_url).await;
            }
        }
    }
    if meta.title.is_none() && meta.description.is_none() && image.is_none() {
        return Err("プレビューできる情報がありません".to_string());
    }
    Ok(LinkPreviewDto {
        title: meta.title,
        description: meta.description,
        site_name: meta.site_name,
        image,
    })
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

/// 設定済みネットワークの一覧(M3-0c)。
///
/// 実体は `networks/<スラッグ>/` 配下(ADR-0012)。旧配置(直下の
/// host.toml / member.toml)はここで自動移行する。
#[tauri::command]
fn list_networks(app: tauri::AppHandle) -> Result<Vec<NetworkDto>, String> {
    let dir = config_dir(&app)?;
    // 旧配置からの移行。失敗しても UI を止めない(次回また試みる)
    if let Err(e) = peercove_ops::networks::migrate_legacy(&dir) {
        eprintln!("旧設定の移行に失敗しました: {e:#}");
    }
    Ok(peercove_ops::networks::list(&dir)
        .iter()
        .map(NetworkDto::from)
        .collect())
}

/// ネットワークを削除する(ディレクトリごと。鍵・PSK も消える)。
/// 稼働中でないことは frontend 側が確認する(削除ボタンを無効化)。
#[tauri::command]
fn delete_network(app: tauri::AppHandle, slug: String) -> Result<(), String> {
    let base = config_dir(&app)?;
    peercove_ops::networks::delete(&base, &slug).map_err(to_message)
}

/// 新しいネットワークをホストとして作る(鍵とランダムサブネットの host.toml)。
///
/// `name` はネットワーク名(M3-0c で UI から入力)。待受ポートは既存の
/// ホストと被らないよう自動選択(ADR-0012)。
#[tauri::command]
fn init_host(app: tauri::AppHandle, name: String, force: bool) -> Result<InitResult, String> {
    let base = config_dir(&app)?;
    let (_, dir) = peercove_ops::networks::network_dir(&base, &name).map_err(to_message)?;
    let port = peercove_ops::networks::next_listen_port(&base);
    let result = peercove_ops::init::init_host(&dir, &name, port, force).map_err(to_message)?;
    Ok(InitResult {
        config_path: result.config_path.display().to_string(),
        network: result.network,
        subnet: result.subnet.to_string(),
        host_ip: result.host_ip.to_string(),
        public_key: result.public_key.to_base64(),
    })
}

/// 招待トークンを発行する。**戻り値の token は秘密情報**(発行直後のみ表示する)。
#[tauri::command]
async fn create_invite(
    config_path: String,
    name: Option<String>,
    psk: bool,
    endpoints: Vec<String>,
    expires_in_secs: Option<u64>,
) -> Result<InviteResult, String> {
    let mut extra: Vec<SocketAddrV4> = endpoints
        .iter()
        .map(|e| {
            e.trim()
                .parse::<SocketAddrV4>()
                .map_err(|_| format!("エンドポイント {e} は IP:ポート形式で指定してください"))
        })
        .collect::<Result<_, _>>()?;

    // 稼働中デーモンが UPnP で観測した外部エンドポイントを自動で候補に足す
    // (M4 E-C)。デーモン停止・UPnP 無効なら黙って何もしない(手動指定で代替可)
    if let Ok(IpcResponse::Status(status)) = peercove_ipc::request_async(IpcRequest::Status).await {
        let canonical = Path::new(&config_path)
            .canonicalize()
            .unwrap_or_else(|_| std::path::PathBuf::from(&config_path));
        let external = status
            .tunnels
            .iter()
            .find(|t| {
                t.config == canonical
                    || t.config.canonicalize().ok().as_deref() == Some(canonical.as_path())
            })
            .and_then(|t| t.external_endpoint);
        if let Some(external) = external {
            if !extra.contains(&external) {
                extra.push(external);
            }
        }
    }

    let result = peercove_ops::invite::invite(&peercove_ops::invite::InviteOptions {
        config_path: Path::new(&config_path),
        name: name.as_deref().filter(|s| !s.trim().is_empty()),
        ip: None,
        extra_endpoints: &extra,
        psk,
        expires_in_secs,
        invited_by: None,
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
        invite_id: result.invite_id,
        issued_at: result.issued_at,
        expires_at: result.expires_at,
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

/// 承認待ち端末を承認し、次のデーモン同期で隔離を解除する。
#[tauri::command]
fn approve_member(config_path: String, public_key: String) -> Result<(), String> {
    let approved_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_secs();
    peercove_ops::peers::approve_invite(
        Path::new(&config_path),
        &Selector::PublicKey(&public_key),
        approved_at,
    )
    .map_err(to_message)
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

/// メンバー招待の発行許可(ADR-0048)を端末単位で切り替える(host.toml)。
/// 反映は次のデーモン同期(約 5 秒)で台帳に載る。
#[tauri::command]
fn set_member_can_invite(
    config_path: String,
    public_key: String,
    allowed: bool,
) -> Result<(), String> {
    peercove_ops::peers::set_peer_can_invite(
        Path::new(&config_path),
        &Selector::PublicKey(&public_key),
        allowed,
    )
    .map(|_| ())
    .map_err(to_message)
}

/// (member)メンバー招待の発行結果。**token は秘密情報**(発行直後のみ表示)。
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct MemberInviteResult {
    token: String,
    qr_svg: String,
    name: String,
    expires_at: Option<u64>,
}

/// (member)メンバー招待の発行をホストへ依頼する(ADR-0048)。
/// 権限確認・割当・記録はすべてホスト側で行われる。
#[tauri::command]
async fn member_create_invite(
    config_path: String,
    name: Option<String>,
    expires_in_secs: Option<u64>,
) -> Result<MemberInviteResult, String> {
    match peercove_ipc::request_async(IpcRequest::CreateInvite {
        config: PathBuf::from(&config_path),
        name,
        expires_in_secs,
    })
    .await
    {
        Ok(IpcResponse::InviteIssued {
            token,
            name,
            expires_at,
        }) => {
            let qr_svg = render_qr_svg(&token)?;
            Ok(MemberInviteResult {
                token,
                qr_svg,
                name,
                expires_at,
            })
        }
        Ok(other) => Err(format!("想定外の応答です: {other:?}")),
        Err(e) => Err(to_message(e)),
    }
}

/// メンバーの DNS 名を変更する(ADR-0021、M3-14a)。ホスト設定(host.toml)に
/// 対してのみ有効。正規化後のラベルを返す(約 5 秒で全メンバーへ配布される)。
#[tauri::command]
fn set_member_dns_name(
    config_path: String,
    public_key: String,
    dns_name: String,
) -> Result<String, String> {
    use peercove_ops::peers::DnsNameOutcome;
    let outcome = peercove_ops::peers::set_peer_dns_name(
        Path::new(&config_path),
        &Selector::PublicKey(&public_key),
        dns_name.trim(),
    )
    .map_err(to_message)?;
    Ok(match outcome {
        DnsNameOutcome::Applied { label, .. } | DnsNameOutcome::Unchanged { label } => label,
    })
}

/// ホスト自身の DNS 名を変更する(ADR-0021、M3-14a)。ホスト設定のみ有効。
/// 正規化後のラベルを返す。
#[tauri::command]
fn set_host_dns_name(config_path: String, dns_name: String) -> Result<String, String> {
    peercove_ops::peers::set_host_dns_name(Path::new(&config_path), dns_name.trim())
        .map_err(to_message)
}

/// ホスト自身の表示名を変更する(ADR-0027、M3-19)。ホスト設定のみ有効。
/// `[interface].display_name` を書く。確定した表示名を返す。
#[tauri::command]
fn set_host_display_name(config_path: String, display_name: String) -> Result<String, String> {
    peercove_ops::peers::set_host_display_name(Path::new(&config_path), display_name.trim())
        .map_err(to_message)
}

/// メンバーの広告サブネット(ADR-0014、M3-7)を設定する。空配列で解除。
/// ホスト設定(host.toml)に対してのみ有効。約 10 秒で全メンバーへ配布される。
#[tauri::command]
fn set_member_subnets(
    config_path: String,
    public_key: String,
    subnets: Vec<String>,
) -> Result<(), String> {
    let parsed: Vec<ipnet::Ipv4Net> = subnets
        .iter()
        .map(|s| {
            s.trim()
                .parse()
                .map_err(|_| format!("\"{s}\" は CIDR(例 192.168.10.0/24)として解釈できません"))
        })
        .collect::<Result<_, _>>()?;
    peercove_ops::peers::set_subnets(
        Path::new(&config_path),
        &Selector::PublicKey(&public_key),
        &parsed,
    )
    .map(|_| ())
    .map_err(to_message)
}

/// ACL の遮断組(ADR-0018、M3-10)。正規化済みの仮想 IP 組を返す。
#[tauri::command]
fn list_acl(config_path: String) -> Result<Vec<[String; 2]>, String> {
    let deny = peercove_ops::acl::list_deny(Path::new(&config_path)).map_err(to_message)?;
    Ok(deny
        .into_iter()
        .map(|(a, b)| [a.to_string(), b.to_string()])
        .collect())
}

/// ACL の遮断組を丸ごと差し替える(空で全許可)。ホスト設定(host.toml)に
/// 対してのみ有効。実行中のデーモンは約 5 秒で追随する(リレー遮断 + 台帳配布)。
#[tauri::command]
fn set_acl(config_path: String, deny: Vec<[String; 2]>) -> Result<(), String> {
    let parsed: Vec<(std::net::Ipv4Addr, std::net::Ipv4Addr)> = deny
        .iter()
        .map(|[a, b]| {
            let a = a
                .trim()
                .parse()
                .map_err(|_| format!("\"{a}\" は IP アドレスとして解釈できません"))?;
            let b = b
                .trim()
                .parse()
                .map_err(|_| format!("\"{b}\" は IP アドレスとして解釈できません"))?;
            Ok::<_, String>((a, b))
        })
        .collect::<Result<_, _>>()?;
    peercove_ops::acl::set_deny(Path::new(&config_path), &parsed).map_err(to_message)
}

#[tauri::command]
fn read_acl_policy(config_path: String) -> Result<peercove_ops::acl::PolicySettings, String> {
    peercove_ops::acl::read_policy(Path::new(&config_path)).map_err(to_message)
}

#[tauri::command]
fn write_acl_policy(
    config_path: String,
    policy: peercove_ops::acl::PolicySettings,
) -> Result<(), String> {
    peercove_ops::acl::write_policy(Path::new(&config_path), &policy).map_err(to_message)
}

// ---- カスタム DNS レコード(M3-1c、ADR-0011 §1b、ADR-0022) ----

/// メンバー参照("host" または公開鍵 base64)を解析する。
fn parse_member_ref(input: &str) -> Result<peercove_core::config::MemberRef, String> {
    input
        .trim()
        .parse()
        .map_err(|_| "メンバーの指定が不正です".to_string())
}

/// カスタム DNS レコードの一覧(fqdn は表示用に組み立て済み)。
#[tauri::command]
fn list_dns_records(config_path: String) -> Result<Vec<DnsRecordDto>, String> {
    Ok(peercove_ops::dns::list_records(Path::new(&config_path))
        .map_err(to_message)?
        .into_iter()
        .map(DnsRecordDto::from)
        .collect())
}

/// カスタム DNS レコードを追加する(ホストの設定のみ、ADR-0022)。
/// ターゲットは ip(固定 IP)/ member(メンバー参照 = IP 自動追随)の
/// どちらか。under で親メンバー配下のサブドメインになる。実行中のホストは
/// 5 秒の再読込で拾い、解決してから台帳と一緒に全メンバーへ配布される。
// tauri コマンドは名前付き引数(フロントから個別に渡る)なので構造体化しない
#[allow(clippy::too_many_arguments)]
#[tauri::command]
fn add_dns_record(
    config_path: String,
    name: String,
    ip: Option<String>,
    member: Option<String>,
    cname: Option<String>,
    under: Option<String>,
    scheme: Option<String>,
    port: Option<u16>,
) -> Result<(), String> {
    let ip = ip.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let cname = cname.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let target = match (ip, &member, cname) {
        (Some(ip), None, None) => peercove_ops::dns::RecordTarget::Ip(
            ip.parse()
                .map_err(|_| format!("\"{ip}\" は IPv4 アドレスとして解釈できません"))?,
        ),
        (None, Some(member), None) => {
            peercove_ops::dns::RecordTarget::Member(parse_member_ref(member)?)
        }
        (None, None, Some(cname)) => peercove_ops::dns::RecordTarget::Cname(cname.to_string()),
        _ => {
            return Err(
                "転送先には IP / メンバー / ドメイン(CNAME)のいずれか 1 つを指定してください"
                    .to_string(),
            )
        }
    };
    let under = under
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(parse_member_ref)
        .transpose()?;
    peercove_ops::dns::add_record(
        Path::new(&config_path),
        &peercove_ops::dns::NewRecord {
            name: name.trim(),
            target,
            under,
            scheme: scheme.as_deref().map(str::trim).filter(|s| !s.is_empty()),
            port,
        },
    )
    .map(|_| ())
    .map_err(to_message)
}

/// カスタム DNS レコードを (name, under) で削除する(ADR-0022)。
#[tauri::command]
fn remove_dns_record(
    config_path: String,
    name: String,
    under: Option<String>,
) -> Result<(), String> {
    let under = under
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(parse_member_ref)
        .transpose()?;
    peercove_ops::dns::remove_record(Path::new(&config_path), &name, under).map_err(to_message)
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
fn set_dns_health(
    config_path: String,
    name: String,
    under: Option<String>,
    enabled: bool,
    kind: String,
    path: String,
    expected_status: Option<u16>,
    external: bool,
) -> Result<(), String> {
    let under = under
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(parse_member_ref)
        .transpose()?;
    let kind = match kind.as_str() {
        "tcp" => peercove_core::dns::HealthCheckKind::Tcp,
        "http_head" => peercove_core::dns::HealthCheckKind::HttpHead,
        _ => return Err("ヘルスチェック方式が不正です".to_string()),
    };
    peercove_ops::dns::set_health(
        Path::new(&config_path),
        &name,
        under,
        &peercove_ops::dns::HealthSettings {
            enabled,
            kind,
            path,
            expected_status,
            external,
        },
    )
    .map_err(to_message)
}

#[tauri::command]
async fn check_dns_health(config_path: String) -> Result<(), String> {
    let config = canonical(&config_path)?;
    match peercove_ipc::request_async(IpcRequest::CheckDnsHealth { config }).await {
        Ok(IpcResponse::Done) => Ok(()),
        Ok(other) => Err(format!("想定外の応答です: {other:?}")),
        Err(error) => Err(to_message(error)),
    }
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
        // 単一インスタンス(M3-5)。**必ず最初に登録する**(公式ドキュメントの指定)。
        // 二重起動されたら既存ウィンドウを前面に出す。deep-link feature により、
        // 二重起動側が受けた peercove:// URL は既存インスタンスの
        // onOpenUrl イベントとして配送される
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            use tauri::Manager;
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            tray::setup(app.handle())?;
            // peercove:// スキームを OS に登録(M3-5)。インストーラも登録するが、
            // 開発ビルド・手動配置でも動くようランタイムでも登録する
            // (Windows: HKCU レジストリ / Linux: ~/.local の .desktop)。
            // 失敗してもディープリンクが使えないだけなので起動は続ける
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                if let Err(e) = app.deep_link().register_all() {
                    #[cfg(target_os = "linux")]
                    eprintln!(
                        "URL スキームの登録に失敗しました(ディープリンクは無効)。\
                         desktop-file-utils と xdg-utils がインストールされているか\
                         確認してください: {e}"
                    );
                    #[cfg(not(target_os = "linux"))]
                    eprintln!("URL スキームの登録に失敗しました(ディープリンクは無効): {e}");
                }
            }
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
            memo_op,
            memo_export,
            memo_import,
            shared_memo_op,
            shared_memo_export,
            create_backup,
            pick_backup,
            inspect_backup,
            restore_backup,
            daemon_status,
            check_update,
            daemon_logs,
            diagnose_network,
            quality_history,
            start_host,
            start_member,
            stop_tunnel,
            rotate_key,
            notify,
            list_networks,
            delete_network,
            init_host,
            create_invite,
            join_network,
            remove_member,
            approve_member,
            rename_member,
            set_member_can_invite,
            member_create_invite,
            set_member_dns_name,
            set_my_dns_name,
            set_host_dns_name,
            set_my_display_name,
            set_host_display_name,
            set_member_subnets,
            list_acl,
            set_acl,
            read_acl_policy,
            write_acl_policy,
            pick_file,
            save_pasted_file,
            send_file,
            chat_send,
            chat_resend,
            chat_cancel_send,
            chat_fetch,
            group_create,
            group_update,
            group_leave,
            list_inbox,
            save_inbox_file,
            delete_inbox_file,
            read_text_preview,
            open_link,
            link_preview,
            list_dns_records,
            add_dns_record,
            remove_dns_record,
            set_dns_health,
            check_dns_health,
            read_settings,
            save_settings,
        ])
        .run(tauri::generate_context!())
        .expect("Tauri アプリの起動に失敗しました");
}

#[cfg(test)]
mod preview_ssrf_tests {
    use super::previewable;

    fn ok(url: &str) -> bool {
        previewable(&reqwest::Url::parse(url).unwrap()).is_ok()
    }

    #[test]
    fn rejects_internal_ip_literals() {
        // IPv4 の内部レンジ
        assert!(!ok("http://127.0.0.1/"));
        assert!(!ok("http://10.1.2.3/"));
        assert!(!ok("http://192.168.0.1/"));
        assert!(!ok("http://169.254.1.1/"));
        assert!(!ok("http://100.100.42.1/")); // CGNAT(PeerCove 既定)
                                              // IPv6 の内部レンジ(以前は loopback/unspecified しか弾いていなかった)
        assert!(!ok("http://[::1]/"));
        assert!(!ok("http://[fd00::1]/")); // ULA
        assert!(!ok("http://[fe80::1]/")); // link-local
        assert!(!ok("http://[::ffff:127.0.0.1]/")); // IPv4-mapped loopback
        assert!(!ok("http://[::ffff:10.0.0.1]/")); // IPv4-mapped private
    }

    #[test]
    fn rejects_internal_names_and_schemes() {
        assert!(!ok("http://localhost/"));
        assert!(!ok("http://foo.internal/"));
        assert!(!ok("http://printer.local/"));
        assert!(!ok("ftp://example.com/")); // http/https 以外
        assert!(!ok("file:///etc/passwd"));
    }

    #[test]
    fn allows_public_ip_literals() {
        assert!(ok("http://1.1.1.1/"));
        assert!(ok("https://[2606:4700:4700::1111]/")); // 公開 IPv6(Cloudflare)
    }
}
