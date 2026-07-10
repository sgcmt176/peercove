//! トンネル内メッセージング基盤(ADR-0015、M3-9)。
//!
//! 各デーモンが自分の仮想 IP の TCP [`MSG_PORT`] で待受け、送信側が相手の
//! 仮想 IP へ直接接続する(P2P)。接続元の仮想 IP は WG の cryptokey routing
//! で偽装できないため、台帳のメンバー仮想 IP と照合すれば身元が決まる
//! (広告サブネット(ADR-0014)内の LAN 機器は台帳に無い IP なので弾かれる)。
//!
//! フレームは JSON Lines([`MsgFrame`])、ファイル本体だけ生バイナリ。
//! 1 論理操作 = 1 短命 TCP 接続なので、常設接続の管理(再接続・take 上限の
//! 罠 — ADR-0009)を持ち込まない。
//!
//! 受信ファイルは受信ボックス([`inbox_dir`])に自動保存し、UI が任意の
//! 場所へコピー・削除する(特権デーモンがユーザー領域へ直接書かない)。
//!
//! 秘匿ルール: ファイル名はチャット本文と同様にログへ出さない(id・IP・
//! サイズは出してよい)。

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context};
use peercove_core::ipc::{TransferDirection, TransferInfo};
use peercove_core::msg::{MsgFrame, MSG_PORT, MSG_VERSION};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// 受信 1 行の上限(control.rs と同じ。take の上限は累計なので 1 行ごとに戻す)。
const MAX_LINE_LEN: u64 = 64 * 1024;
/// ファイル本体の読み書き単位。
const CHUNK: usize = 64 * 1024;
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// 本体転送中も含む、1 回の読み書きの無通信タイムアウト。
const IO_TIMEOUT: Duration = Duration::from_secs(60);
/// 進捗一覧に残す「終了済み(完了/失敗)」エントリの上限。
const MAX_FINISHED: usize = 20;

/// 接続元の照合に使うピア表(仮想 IP → 表示名)。supervisor が台帳から
/// 毎周期更新する(自分自身は含めない)。
pub type SharedPeers = Arc<Mutex<HashMap<Ipv4Addr, String>>>;

/// ファイル転送の進捗一覧(IPC の status 応答に載る)。
pub type TransferRegistry = Arc<Mutex<Vec<TransferInfo>>>;

/// 受信ボックスの場所: 設定ファイルの拡張子を差し替える
/// (`networks/game.toml` → `networks/game.inbox/`。status ファイルと同じ規則)。
pub fn inbox_dir(config_path: &Path) -> PathBuf {
    config_path.with_extension("inbox")
}

/// 転送 ID。認証には使わない(身元は接続元 IP)ので、レジストリ内で
/// 一意になれば十分。
pub fn new_transfer_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}-{:x}", COUNTER.fetch_add(1, Ordering::Relaxed))
}

fn register(transfers: &TransferRegistry, info: TransferInfo) {
    let mut list = transfers.lock().unwrap();
    // 終了済みの古いエントリから捨てる(実行中は消さない)
    let finished = list.iter().filter(|t| t.done).count();
    if finished >= MAX_FINISHED {
        if let Some(pos) = list.iter().position(|t| t.done) {
            list.remove(pos);
        }
    }
    list.push(info);
}

fn update(transfers: &TransferRegistry, id: &str, apply: impl FnOnce(&mut TransferInfo)) {
    let mut list = transfers.lock().unwrap();
    if let Some(info) = list.iter_mut().find(|t| t.id == id) {
        apply(info);
    }
}

fn mark_failed(transfers: &TransferRegistry, id: &str, error: &anyhow::Error) {
    update(transfers, id, |t| {
        t.done = true;
        t.error = Some(format!("{error:#}"));
    });
}

type LineReader = tokio::io::Take<BufReader<tokio::net::tcp::OwnedReadHalf>>;

fn line_reader(read_half: tokio::net::tcp::OwnedReadHalf) -> LineReader {
    BufReader::new(read_half).take(MAX_LINE_LEN)
}

/// 1 フレーム読む。`None` は EOF(相手が切断)。
/// `read_line` はキャンセル安全でないため、`select!` の分岐に置かないこと
/// (このモジュールは 1 接続 1 タスクの素直なループだけなので問題にならない)。
async fn read_frame(
    reader: &mut LineReader,
    line: &mut String,
) -> anyhow::Result<Option<MsgFrame>> {
    reader.set_limit(MAX_LINE_LEN); // 上限は累計なので 1 行ごとに戻す
    line.clear();
    if reader.read_line(line).await? == 0 {
        return Ok(None);
    }
    if !line.ends_with('\n') {
        if reader.limit() == 0 {
            bail!("1 行が {MAX_LINE_LEN} バイトを超えました");
        }
        bail!("行の途中で切断されました");
    }
    Ok(Some(
        serde_json::from_str(line).context("フレームの解析に失敗しました")?,
    ))
}

/// タイムアウト付きで 1 フレーム読む(EOF はエラー扱い)。
async fn expect_frame(reader: &mut LineReader, line: &mut String) -> anyhow::Result<MsgFrame> {
    tokio::time::timeout(IO_TIMEOUT, read_frame(reader, line))
        .await
        .map_err(|_| anyhow::anyhow!("応答がタイムアウトしました"))??
        .context("相手が切断しました")
}

async fn send_frame<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    frame: &MsgFrame,
) -> anyhow::Result<()> {
    let mut json = serde_json::to_string(frame).expect("MsgFrame は常に直列化可能");
    json.push('\n');
    writer.write_all(json.as_bytes()).await?;
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 受信側サーバー。自分の仮想 IP で待受け、台帳のメンバーからの接続だけ受ける。
pub async fn run_server(
    bind_ip: Ipv4Addr,
    peers: SharedPeers,
    inbox: PathBuf,
    transfers: TransferRegistry,
) {
    // トンネル作成直後は Windows が仮想 IP を数秒間「準備中」として扱うため、
    // bind できるまでリトライする(control.rs と同じ事情)
    let listener = loop {
        match TcpListener::bind(SocketAddr::from((bind_ip, MSG_PORT))).await {
            Ok(listener) => break listener,
            Err(e) => {
                tracing::debug!("メッセージング待受の起動待ち(想定内): {e}");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    };
    tracing::info!("メッセージングを {bind_ip}:{MSG_PORT} で待受けます");

    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let peer_ip = match peer.ip() {
                    IpAddr::V4(ip) => ip,
                    IpAddr::V6(_) => continue, // IPv4 のみ
                };
                // 台帳照合: メンバーの仮想 IP 以外(広告サブネット内の機器など)は拒否
                let Some(sender_name) = peers.lock().unwrap().get(&peer_ip).cloned() else {
                    tracing::debug!("台帳にない {peer_ip} からのメッセージング接続を拒否しました");
                    continue;
                };
                let inbox = inbox.clone();
                let transfers = Arc::clone(&transfers);
                tokio::spawn(async move {
                    if let Err(e) =
                        handle_incoming(stream, peer_ip, sender_name, &inbox, &transfers).await
                    {
                        tracing::debug!("{peer_ip} からのメッセージング接続が終了: {e:#}");
                    }
                });
            }
            Err(e) => {
                tracing::warn!("メッセージングの accept に失敗: {e}");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

/// 受信側 1 接続: Hello → 本題のフレーム(現状はファイルの申し出のみ)。
async fn handle_incoming(
    stream: TcpStream,
    peer_ip: Ipv4Addr,
    sender_name: String,
    inbox: &Path,
    transfers: &TransferRegistry,
) -> anyhow::Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = line_reader(read_half);
    let mut line = String::new();

    let hello = tokio::time::timeout(HELLO_TIMEOUT, read_frame(&mut reader, &mut line))
        .await
        .map_err(|_| anyhow::anyhow!("Hello がタイムアウトしました"))??
        .context("Hello の前に切断されました")?;
    match hello {
        MsgFrame::Hello { version } => {
            if version != MSG_VERSION {
                tracing::warn!(
                    "{peer_ip} のメッセージングバージョン {version} は未対応です\
                    (こちらは {MSG_VERSION})"
                );
            }
        }
        other => bail!("Hello 以外のフレームが届きました: {other:?}"),
    }

    match expect_frame(&mut reader, &mut line).await? {
        MsgFrame::FileOffer { id, name, size } => {
            receive_file(
                &mut reader,
                &mut write_half,
                peer_ip,
                &sender_name,
                inbox,
                transfers,
                id,
                &name,
                size,
            )
            .await
        }
        other => bail!("想定外のフレームが届きました: {other:?}"),
    }
}

/// ファイルを受信ボックスへ保存する。書きかけは `.part`、完了時に本名へ
/// リネームし、隣に `.pcvmeta`(送信者などのメタ情報)を置く。
#[allow(clippy::too_many_arguments)]
async fn receive_file(
    reader: &mut LineReader,
    write_half: &mut tokio::net::tcp::OwnedWriteHalf,
    peer_ip: Ipv4Addr,
    sender_name: &str,
    inbox: &Path,
    transfers: &TransferRegistry,
    id: String,
    name: &str,
    size: u64,
) -> anyhow::Result<()> {
    let Some(safe_name) = sanitize_file_name(name) else {
        send_frame(
            write_half,
            &MsgFrame::FileReject {
                id,
                reason: "このファイル名は受け取れません".to_string(),
            },
        )
        .await?;
        bail!("不正なファイル名の申し出を拒否しました({peer_ip})");
    };
    tokio::fs::create_dir_all(inbox)
        .await
        .with_context(|| format!("受信ボックス {} を作成できません", inbox.display()))?;
    // 特権デーモンが作るディレクトリを、非特権の UI が読み書き(保存・削除)
    // できるようにする。IPC ソケットを 0666 にするのと同じ割り切り
    // (単一ユーザー PC 前提 — ADR-0007)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(inbox, std::fs::Permissions::from_mode(0o777));
    }
    let final_path = unique_path(inbox, &safe_name);
    let part_path = append_suffix(&final_path, ".part");

    register(
        transfers,
        TransferInfo {
            id: id.clone(),
            direction: TransferDirection::Recv,
            peer: peer_ip,
            name: safe_name.clone(),
            size,
            transferred: 0,
            done: false,
            error: None,
        },
    );
    let result = receive_body(
        reader,
        write_half,
        peer_ip,
        sender_name,
        transfers,
        &id,
        &final_path,
        &part_path,
        size,
    )
    .await;
    match &result {
        Ok(()) => update(transfers, &id, |t| t.done = true),
        Err(e) => {
            let _ = tokio::fs::remove_file(&part_path).await;
            mark_failed(transfers, &id, e);
        }
    }
    result
}

#[allow(clippy::too_many_arguments)]
async fn receive_body(
    reader: &mut LineReader,
    write_half: &mut tokio::net::tcp::OwnedWriteHalf,
    peer_ip: Ipv4Addr,
    sender_name: &str,
    transfers: &TransferRegistry,
    id: &str,
    final_path: &Path,
    part_path: &Path,
    size: u64,
) -> anyhow::Result<()> {
    let mut file = tokio::fs::File::create(part_path)
        .await
        .context("受信ファイルを作成できません")?;
    send_frame(write_half, &MsgFrame::FileAccept { id: id.to_string() }).await?;

    // 本体: take の上限を残りバイト数に切り替えて読む(超過分は読まない)
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let mut received: u64 = 0;
    reader.set_limit(size);
    while received < size {
        let n = tokio::time::timeout(IO_TIMEOUT, reader.read(&mut buf))
            .await
            .map_err(|_| anyhow::anyhow!("転送がタイムアウトしました"))??;
        if n == 0 {
            bail!("本体の途中で切断されました({received}/{size} バイト)");
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n]).await?;
        received += n as u64;
        update(transfers, id, |t| t.transferred = received);
    }
    file.flush().await?;
    drop(file);

    // 後置ハッシュで完全性を検証(ADR-0015)
    let sha256 = match expect_frame(reader, &mut String::new()).await? {
        MsgFrame::FileHash {
            id: hash_id,
            sha256,
        } if hash_id == id => sha256,
        other => bail!("FileHash を期待しましたが別のフレームが届きました: {other:?}"),
    };
    let actual = hex(&hasher.finalize());
    if !actual.eq_ignore_ascii_case(&sha256) {
        bail!("チェックサムが一致しません(転送が壊れています)");
    }

    // メタ情報(UI の受信ボックス表示用)→ 本名へリネーム → 完了通知
    let meta = serde_json::json!({
        "from_ip": peer_ip.to_string(),
        "from_name": sender_name,
        "size": size,
        "received_unix_ms": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
    });
    let meta_path = append_suffix(final_path, ".pcvmeta");
    tokio::fs::write(&meta_path, meta.to_string())
        .await
        .context("メタ情報の書き出しに失敗しました")?;
    tokio::fs::rename(part_path, final_path)
        .await
        .context("受信ファイルの確定(リネーム)に失敗しました")?;
    send_frame(write_half, &MsgFrame::FileDone { id: id.to_string() }).await?;
    tracing::info!("{sender_name}({peer_ip})からファイルを受信しました({size} バイト、id={id})");
    Ok(())
}

/// 送信側: 相手の仮想 IP のメッセージングポートへ接続してファイルを送る。
/// 進捗は `transfers` に反映される(UI / CLI は status 経由で追う)。
pub async fn send_file(
    peer_ip: Ipv4Addr,
    path: &Path,
    transfers: TransferRegistry,
    id: String,
) -> anyhow::Result<()> {
    send_file_to(
        SocketAddr::from((peer_ip, MSG_PORT)),
        peer_ip,
        path,
        transfers,
        id,
    )
    .await
}

/// 実装本体(テストではエフェメラルポートへ接続するため分離)。
async fn send_file_to(
    target: SocketAddr,
    peer_ip: Ipv4Addr,
    path: &Path,
    transfers: TransferRegistry,
    id: String,
) -> anyhow::Result<()> {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .context("ファイル名を取得できません")?
        .to_string();
    let meta = tokio::fs::metadata(path)
        .await
        .with_context(|| format!("{} を読めません", path.display()))?;
    if !meta.is_file() {
        bail!("{} はファイルではありません", path.display());
    }
    let size = meta.len();

    register(
        &transfers,
        TransferInfo {
            id: id.clone(),
            direction: TransferDirection::Send,
            peer: peer_ip,
            name,
            size,
            transferred: 0,
            done: false,
            error: None,
        },
    );
    let result = send_body(target, path, &transfers, &id, size).await;
    match &result {
        Ok(()) => update(&transfers, &id, |t| t.done = true),
        Err(e) => mark_failed(&transfers, &id, e),
    }
    result
}

async fn send_body(
    target: SocketAddr,
    path: &Path,
    transfers: &TransferRegistry,
    id: &str,
    size: u64,
) -> anyhow::Result<()> {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .expect("呼び出し元で確認済み")
        .to_string();
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(target))
        .await
        .map_err(|_| anyhow::anyhow!("接続がタイムアウトしました"))?
        .map_err(|e| {
            anyhow::anyhow!(
                "相手に接続できません(相手のトンネルが動いていないか、\
                 相手の PeerCove が旧バージョンです): {e}"
            )
        })?;
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = line_reader(read_half);
    let mut line = String::new();

    send_frame(
        &mut write_half,
        &MsgFrame::Hello {
            version: MSG_VERSION,
        },
    )
    .await?;
    send_frame(
        &mut write_half,
        &MsgFrame::FileOffer {
            id: id.to_string(),
            name,
            size,
        },
    )
    .await?;
    match expect_frame(&mut reader, &mut line).await? {
        MsgFrame::FileAccept { id: ack_id } if ack_id == id => {}
        MsgFrame::FileReject { reason, .. } => bail!("相手が受信を拒否しました: {reason}"),
        other => bail!("FileAccept を期待しましたが別のフレームが届きました: {other:?}"),
    }

    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("{} を開けません", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let mut sent: u64 = 0;
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        tokio::time::timeout(IO_TIMEOUT, write_half.write_all(&buf[..n]))
            .await
            .map_err(|_| anyhow::anyhow!("転送がタイムアウトしました"))??;
        sent += n as u64;
        update(transfers, id, |t| t.transferred = sent);
    }
    if sent != size {
        // 送信中にファイルが書き換えられた等。受信側はサイズ不一致で検出する
        bail!("ファイルサイズが途中で変わりました({sent}/{size} バイト)");
    }
    send_frame(
        &mut write_half,
        &MsgFrame::FileHash {
            id: id.to_string(),
            sha256: hex(&hasher.finalize()),
        },
    )
    .await?;
    match expect_frame(&mut reader, &mut line).await? {
        MsgFrame::FileDone { id: done_id } if done_id == id => Ok(()),
        other => bail!("FileDone を期待しましたが別のフレームが届きました: {other:?}"),
    }
}

/// 受け取ったファイル名を安全な名前にする。パス区切りを剥がし、制御文字と
/// Windows で使えない文字を置換する。受け入れられない名前は `None`。
fn sanitize_file_name(name: &str) -> Option<String> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let cleaned: String = base
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    // 先頭・末尾のドットと空白を剥がす("." ".." や Windows の末尾ドット対策)
    let cleaned = cleaned.trim().trim_matches('.').trim();
    if cleaned.is_empty() {
        return None;
    }
    // Windows の予約デバイス名(CON, NUL, COM1 など)を避ける
    let stem = cleaned.split('.').next().unwrap_or(cleaned);
    let upper = stem.to_ascii_uppercase();
    let reserved = matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.len() == 4
            && upper.as_bytes()[3].is_ascii_digit());
    let cleaned = if reserved {
        format!("_{cleaned}")
    } else {
        cleaned.to_string()
    };
    // 受信ボックスの内部ファイル(書きかけ・メタ情報)と紛れない名前にする
    if cleaned.ends_with(".part") || cleaned.ends_with(".pcvmeta") {
        Some(format!("{cleaned}.file"))
    } else {
        Some(cleaned)
    }
}

/// 同名ファイルがあれば " (1)" 等を付けて空きを探す(書きかけの `.part` も考慮)。
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let taken = |p: &Path| p.exists() || append_suffix(p, ".part").exists();
    let candidate = dir.join(name);
    if !taken(&candidate) {
        return candidate;
    }
    let (stem, ext) = match name.rfind('.') {
        // ".bashrc" のような隠しファイルは全体を stem として扱う
        Some(pos) if pos > 0 => (&name[..pos], &name[pos..]),
        _ => (name, ""),
    };
    (1..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !taken(p))
        .expect("空き番号は必ずある")
}

/// パスのファイル名末尾にサフィックスを足す(拡張子の置換はしない)。
fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    name.push(suffix);
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "peercove-msg-{label}-{}-{}",
            std::process::id(),
            new_transfer_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn sanitize_strips_paths_and_bad_names() {
        assert_eq!(
            sanitize_file_name("../../etc/passwd").as_deref(),
            Some("passwd")
        );
        assert_eq!(
            sanitize_file_name(r"C:\Users\a\写真.jpg").as_deref(),
            Some("写真.jpg")
        );
        assert_eq!(
            sanitize_file_name("a<b>:c.txt").as_deref(),
            Some("a_b__c.txt")
        );
        assert_eq!(sanitize_file_name("..").as_deref(), None);
        assert_eq!(sanitize_file_name("  .  ").as_deref(), None);
        assert_eq!(sanitize_file_name("con.txt").as_deref(), Some("_con.txt"));
        assert_eq!(sanitize_file_name("com3").as_deref(), Some("_com3"));
        assert_eq!(
            sanitize_file_name("x.part").as_deref(),
            Some("x.part.file"),
            "内部ファイルの拡張子は避ける"
        );
        assert_eq!(
            sanitize_file_name("普通の名前.zip").as_deref(),
            Some("普通の名前.zip")
        );
    }

    #[test]
    fn unique_path_avoids_collisions() {
        let dir = temp_dir("unique");
        assert_eq!(unique_path(&dir, "a.txt"), dir.join("a.txt"));
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "a.txt"), dir.join("a (1).txt"));
        std::fs::write(dir.join("a (1).txt"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "a.txt"), dir.join("a (2).txt"));
        // 書きかけ(.part)も衝突とみなす
        std::fs::write(dir.join("b.txt.part"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "b.txt"), dir.join("b (1).txt"));
        // 拡張子なし
        std::fs::write(dir.join("c"), b"x").unwrap();
        assert_eq!(unique_path(&dir, "c"), dir.join("c (1)"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 送信 → 受信ボックス保存 → メタ情報 → 双方の進捗が「完了」になる E2E。
    #[tokio::test]
    async fn file_transfer_roundtrip() {
        let inbox = temp_dir("inbox");
        let src_dir = temp_dir("src");
        let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let src = src_dir.join("データ.bin");
        std::fs::write(&src, &payload).unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let recv_transfers: TransferRegistry = Default::default();
        let server_transfers = Arc::clone(&recv_transfers);
        let server_inbox = inbox.clone();
        let server = tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.unwrap();
            let ip = match peer.ip() {
                IpAddr::V4(ip) => ip,
                _ => unreachable!(),
            };
            handle_incoming(
                stream,
                ip,
                "alice".to_string(),
                &server_inbox,
                &server_transfers,
            )
            .await
        });

        let send_transfers: TransferRegistry = Default::default();
        send_file_to(
            addr,
            "127.0.0.1".parse().unwrap(),
            &src,
            Arc::clone(&send_transfers),
            "t-1".to_string(),
        )
        .await
        .unwrap();
        server.await.unwrap().unwrap();

        // 受信ボックスに本体とメタ情報がある(.part は残らない)
        let saved = inbox.join("データ.bin");
        assert_eq!(std::fs::read(&saved).unwrap(), payload);
        let meta: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(append_suffix(&saved, ".pcvmeta")).unwrap(),
        )
        .unwrap();
        assert_eq!(meta["from_name"], "alice");
        assert_eq!(meta["size"], payload.len() as u64);
        assert!(!append_suffix(&saved, ".part").exists());

        // 双方のレジストリが「完了」
        for (label, reg) in [("send", &send_transfers), ("recv", &recv_transfers)] {
            let list = reg.lock().unwrap();
            assert_eq!(list.len(), 1, "{label}");
            assert!(
                list[0].done && list[0].error.is_none(),
                "{label}: {:?}",
                list[0]
            );
            assert_eq!(list[0].transferred, payload.len() as u64, "{label}");
        }
        let _ = std::fs::remove_dir_all(&inbox);
        let _ = std::fs::remove_dir_all(&src_dir);
    }

    /// チェックサム不一致: 受信側はエラーになり、.part も本体も残らない。
    #[tokio::test]
    async fn hash_mismatch_discards_the_file() {
        let inbox = temp_dir("badhash");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let transfers: TransferRegistry = Default::default();
        let server_transfers = Arc::clone(&transfers);
        let server_inbox = inbox.clone();
        let server = tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.unwrap();
            let ip = match peer.ip() {
                IpAddr::V4(ip) => ip,
                _ => unreachable!(),
            };
            handle_incoming(
                stream,
                ip,
                "mallory".to_string(),
                &server_inbox,
                &server_transfers,
            )
            .await
        });

        let mut stream = TcpStream::connect(addr).await.unwrap();
        for frame in [
            MsgFrame::Hello {
                version: MSG_VERSION,
            },
            MsgFrame::FileOffer {
                id: "t-2".to_string(),
                name: "x.bin".to_string(),
                size: 4,
            },
        ] {
            send_frame(&mut stream, &frame).await.unwrap();
        }
        // FileAccept を読み飛ばして本体 + 偽ハッシュ
        let mut buf = [0u8; 256];
        let _ = stream.read(&mut buf).await.unwrap();
        stream.write_all(b"abcd").await.unwrap();
        send_frame(
            &mut stream,
            &MsgFrame::FileHash {
                id: "t-2".to_string(),
                sha256: "00".repeat(32),
            },
        )
        .await
        .unwrap();

        let err = server.await.unwrap().unwrap_err();
        assert!(err.to_string().contains("チェックサム"), "{err:#}");
        assert!(!inbox.join("x.bin").exists());
        assert!(!inbox.join("x.bin.part").exists());
        let list = transfers.lock().unwrap();
        assert!(list[0].done && list[0].error.is_some());
        drop(list);
        let _ = std::fs::remove_dir_all(&inbox);
    }

    /// 受け入れられないファイル名は FileReject が返り、送信側はエラーになる。
    #[tokio::test]
    async fn invalid_name_is_rejected() {
        let inbox = temp_dir("reject");
        let src_dir = temp_dir("rejsrc");
        // 送信側の sanitize は通るが、受信側で空になる名前を直接流すため
        // 手組みのフレームでテストする
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let transfers: TransferRegistry = Default::default();
        let server_transfers = Arc::clone(&transfers);
        let server_inbox = inbox.clone();
        let server = tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.unwrap();
            let ip = match peer.ip() {
                IpAddr::V4(ip) => ip,
                _ => unreachable!(),
            };
            handle_incoming(
                stream,
                ip,
                "eve".to_string(),
                &server_inbox,
                &server_transfers,
            )
            .await
        });

        let stream = TcpStream::connect(addr).await.unwrap();
        let (read_half, mut write_half) = stream.into_split();
        let mut reader = line_reader(read_half);
        send_frame(
            &mut write_half,
            &MsgFrame::Hello {
                version: MSG_VERSION,
            },
        )
        .await
        .unwrap();
        send_frame(
            &mut write_half,
            &MsgFrame::FileOffer {
                id: "t-3".to_string(),
                name: "..".to_string(),
                size: 1,
            },
        )
        .await
        .unwrap();
        let mut line = String::new();
        let reply = read_frame(&mut reader, &mut line).await.unwrap().unwrap();
        assert!(matches!(reply, MsgFrame::FileReject { .. }), "{reply:?}");
        assert!(server.await.unwrap().is_err());
        let _ = std::fs::remove_dir_all(&inbox);
        let _ = std::fs::remove_dir_all(&src_dir);
    }
}
