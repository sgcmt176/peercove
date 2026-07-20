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
use peercove_core::ipc::{ChatFileInfo, ChatMessageInfo, TransferDirection, TransferInfo};
use peercove_core::msg::{
    ChatContext, ChatScope, GroupInfo, MsgFrame, MAX_CHAT_TEXT_BYTES, MAX_GROUP_MEMBERS,
    MAX_GROUP_NAME_BYTES, MSG_PORT, MSG_VERSION,
};
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

/// 受信ファイルサイズの上限(バイト)。0 は無制限。supervisor が設定
/// (`[interface] max_recv_file_mb`)から毎周期更新する(2026-07-11 依頼者指定)。
pub type SharedLimit = Arc<Mutex<u64>>;

/// 設定値(MB)→ バイト。
pub fn limit_bytes(mb: u64) -> u64 {
    mb.saturating_mul(1024 * 1024)
}

/// 受信ボックスの場所: 設定ファイルの拡張子を差し替える
/// (`networks/game.toml` → `networks/game.inbox/`。status ファイルと同じ規則)。
pub fn inbox_dir(config_path: &Path) -> PathBuf {
    config_path.with_extension("inbox")
}

/// 現在時刻(UNIX ミリ秒)。時計が狂っていても 0 に落として続行する。
pub fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
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
    limit: SharedLimit,
    chat: crate::chat::SharedChatLog,
    groups: crate::groups::SharedGroups,
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
                let limit = *limit.lock().unwrap();
                let chat = Arc::clone(&chat);
                let groups = Arc::clone(&groups);
                let peers = Arc::clone(&peers);
                tokio::spawn(async move {
                    if let Err(e) = handle_incoming(
                        stream,
                        peer_ip,
                        bind_ip,
                        sender_name,
                        &peers,
                        &inbox,
                        &transfers,
                        limit,
                        &chat,
                        &groups,
                    )
                    .await
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

/// 受信側 1 接続: Hello → 本題のフレーム(ファイルの申し出、またはチャット)。
#[allow(clippy::too_many_arguments)]
async fn handle_incoming(
    stream: TcpStream,
    peer_ip: Ipv4Addr,
    own_ip: Ipv4Addr,
    sender_name: String,
    peers: &SharedPeers,
    inbox: &Path,
    transfers: &TransferRegistry,
    limit: u64,
    chat: &crate::chat::SharedChatLog,
    groups: &crate::groups::SharedGroups,
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
        MsgFrame::FileOffer {
            id,
            name,
            size,
            chat: chat_ctx,
            resume,
        } => {
            // 受信サイズ上限(0 は無制限)。受け取る側の設定として効く
            if limit > 0 && size > limit {
                send_frame(
                    &mut write_half,
                    &MsgFrame::FileReject {
                        id,
                        reason: format!(
                            "サイズが受信側の上限({} MB)を超えています",
                            limit / (1024 * 1024)
                        ),
                    },
                )
                .await?;
                bail!("上限を超える申し出を拒否しました({peer_ip}、{size} バイト)");
            }
            receive_file(
                &mut reader,
                &mut write_half,
                peer_ip,
                own_ip,
                &sender_name,
                inbox,
                transfers,
                chat,
                chat_ctx,
                id,
                &name,
                size,
                resume,
            )
            .await
        }
        MsgFrame::Chat {
            id,
            scope,
            group_id,
            text,
            sent_at,
        } => {
            // 送信側も検査するので、超過は不正なクライアントだけ(ack を返さず切る)
            if text.len() > MAX_CHAT_TEXT_BYTES {
                bail!("本文が上限({MAX_CHAT_TEXT_BYTES} バイト)を超えています({peer_ip})");
            }
            if scope == ChatScope::Group && group_id.is_none() {
                bail!("group 宛なのに group_id がありません({peer_ip})");
            }
            // 再送の重複(ack の取り損ね後に同じ ID で再送 — E-E 3)は
            // 取り込まず ack だけ返す
            if chat.lock().unwrap().contains_id(&id) {
                send_frame(&mut write_half, &MsgFrame::ChatAck { id: id.clone() }).await?;
                tracing::debug!("重複したチャットを ack のみで処理しました(id={id})");
                return Ok(());
            }
            let entry = ChatMessageInfo {
                seq: 0, // append が振る
                id: id.clone(),
                scope,
                group_id,
                from: peer_ip,
                to: match scope {
                    ChatScope::Direct => Some(own_ip),
                    ChatScope::Network | ChatScope::Group => None,
                },
                text,
                sent_at,
                failed: false,
                file: None,
                system: false,
            };
            chat.lock().unwrap().append(entry);
            send_frame(&mut write_half, &MsgFrame::ChatAck { id: id.clone() }).await?;
            // 本文はログに出さない(秘匿ルール)
            tracing::info!("{sender_name}({peer_ip})からチャットを受信しました(id={id})");
            Ok(())
        }
        MsgFrame::GroupUpdate { group } => {
            // グループ名はログに出さない(秘匿ルール)。上限はフレームの
            // 1 行上限に収める安全弁(正規のデーモンは送信側でも検査する)
            if group.id.is_empty()
                || group.name.is_empty()
                || group.name.len() > MAX_GROUP_NAME_BYTES
                || group.members.len() > MAX_GROUP_MEMBERS
            {
                bail!("不正なグループ更新を拒否しました({peer_ip})");
            }
            let id = group.id.clone();
            let revision = group.revision;
            let applied = {
                let mut store = groups.lock().unwrap();
                // 認可: 送信元がそのグループのメンバーでなければ取り込まない
                // (非メンバーによる改名・追放・自分の勝手な追加を防ぐ)。
                if !store.accepts_update(&group, peer_ip) {
                    bail!("権限のないグループ更新を拒否しました({peer_ip})");
                }
                // 送信者はこの revision を持っている → こちらから送り返さない
                store.mark_acked(&id, peer_ip, revision);
                store.apply(group.clone())
            };
            send_frame(&mut write_half, &MsgFrame::GroupAck { id: id.clone() }).await?;
            if let Some(update) = applied {
                // 作成・追加・退出・改名のお知らせを会話に出す(LINE 風)
                let name_of = |ip: Ipv4Addr| -> String {
                    if ip == own_ip {
                        return "自分".to_string();
                    }
                    peers
                        .lock()
                        .unwrap()
                        .get(&ip)
                        .cloned()
                        .unwrap_or_else(|| ip.to_string())
                };
                for text in crate::groups::system_messages(
                    update.previous.as_ref(),
                    &group,
                    own_ip,
                    &name_of,
                ) {
                    chat.lock().unwrap().append(ChatMessageInfo {
                        seq: 0, // append が振る
                        id: new_transfer_id(),
                        scope: ChatScope::Group,
                        group_id: Some(id.clone()),
                        from: group.updated_by,
                        to: None,
                        text,
                        sent_at: now_unix_ms(),
                        failed: false,
                        file: None,
                        system: true,
                    });
                }
                tracing::info!(
                    "{sender_name}({peer_ip})からグループ更新を受信しました(id={id} rev={revision})"
                );
            }
            Ok(())
        }
        other => bail!("想定外のフレームが届きました: {other:?}"),
    }
}

/// 送信側: 相手の仮想 IP へチャット 1 通を送り、ack を確認する。
pub async fn send_chat(
    peer_ip: Ipv4Addr,
    id: &str,
    scope: ChatScope,
    group_id: Option<&str>,
    text: &str,
    sent_at: u64,
) -> anyhow::Result<()> {
    send_chat_to(
        SocketAddr::from((peer_ip, MSG_PORT)),
        id,
        scope,
        group_id,
        text,
        sent_at,
    )
    .await
}

/// 実装本体(テストではエフェメラルポートへ接続するため分離)。
async fn send_chat_to(
    target: SocketAddr,
    id: &str,
    scope: ChatScope,
    group_id: Option<&str>,
    text: &str,
    sent_at: u64,
) -> anyhow::Result<()> {
    let (mut reader, mut write_half) = connect_and_hello(target).await?;
    let mut line = String::new();
    send_frame(
        &mut write_half,
        &MsgFrame::Chat {
            id: id.to_string(),
            scope,
            group_id: group_id.map(str::to_string),
            text: text.to_string(),
            sent_at,
        },
    )
    .await?;
    match expect_frame(&mut reader, &mut line).await? {
        MsgFrame::ChatAck { id: ack_id } if ack_id == id => Ok(()),
        other => bail!("ChatAck を期待しましたが別のフレームが届きました: {other:?}"),
    }
}

/// 送信側: 相手の仮想 IP へグループ全量を送り、ack を確認する(M3-13c)。
pub async fn send_group_update(peer_ip: Ipv4Addr, group: &GroupInfo) -> anyhow::Result<()> {
    send_group_update_to(SocketAddr::from((peer_ip, MSG_PORT)), group).await
}

/// 実装本体(テストではエフェメラルポートへ接続するため分離)。
async fn send_group_update_to(target: SocketAddr, group: &GroupInfo) -> anyhow::Result<()> {
    let (mut reader, mut write_half) = connect_and_hello(target).await?;
    let mut line = String::new();
    send_frame(
        &mut write_half,
        &MsgFrame::GroupUpdate {
            group: group.clone(),
        },
    )
    .await?;
    match expect_frame(&mut reader, &mut line).await? {
        MsgFrame::GroupAck { id } if id == group.id => Ok(()),
        other => bail!("GroupAck を期待しましたが別のフレームが届きました: {other:?}"),
    }
}

/// 接続して Hello まで送る(チャット・グループ更新の共通前半)。
async fn connect_and_hello(
    target: SocketAddr,
) -> anyhow::Result<(LineReader, tokio::net::tcp::OwnedWriteHalf)> {
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
    let reader = line_reader(read_half);
    send_frame(
        &mut write_half,
        &MsgFrame::Hello {
            version: MSG_VERSION,
        },
    )
    .await?;
    Ok((reader, write_half))
}

/// ファイルを受信ボックスへ保存する。書きかけは `.part`、完了時に本名へ
/// リネームし、隣に `.pcvmeta`(送信者などのメタ情報)を置く。
/// チャット文脈付き(M3-13d)なら履歴にもファイルのエントリを記録する。
/// 送信側が `resume` 対応なら、途中失敗した書きかけを保持し(`.pcvresume`)、
/// 同じファイルの申し出が来たら続きから受け取る(E-E 6)。
#[allow(clippy::too_many_arguments)]
async fn receive_file(
    reader: &mut LineReader,
    write_half: &mut tokio::net::tcp::OwnedWriteHalf,
    peer_ip: Ipv4Addr,
    own_ip: Ipv4Addr,
    sender_name: &str,
    inbox: &Path,
    transfers: &TransferRegistry,
    chat: &crate::chat::SharedChatLog,
    chat_ctx: Option<ChatContext>,
    id: String,
    name: &str,
    size: u64,
    resume: bool,
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
    // 中断再開(E-E 6): 同じ送信元・名前・サイズの書きかけがあれば続きから。
    // 無ければ従来どおり新しい保存先を決める
    let resumable = if resume {
        find_resumable(inbox, peer_ip, &safe_name, size).await
    } else {
        None
    };
    let (final_path, part_path, offset) = match resumable {
        Some((final_path, part_path, offset)) => {
            tracing::info!("書きかけから再開します({peer_ip}、{offset}/{size} バイト、id={id})");
            (final_path, part_path, offset)
        }
        None => {
            let final_path = unique_path(inbox, &safe_name);
            let part_path = append_suffix(&final_path, ".part");
            (final_path, part_path, 0)
        }
    };

    register(
        transfers,
        TransferInfo {
            id: id.clone(),
            direction: TransferDirection::Recv,
            peer: peer_ip,
            name: safe_name.clone(),
            size,
            transferred: offset,
            done: false,
            error: None,
        },
    );
    // チャット内ファイル送信(M3-13d): 受信開始時に履歴へ記録する
    // (UI のファイルバブルは transfers の進捗を重ねて表示する)。
    // name は実際に保存されるファイル名(同名回避の連番込み)。
    // 失敗時のお知らせ(M3-13e)のため、載せたエントリを覚えておく
    let mut chat_entry: Option<ChatMessageInfo> = None;
    if let Some(ctx) = chat_ctx {
        if ctx.scope != ChatScope::Group || ctx.group_id.is_some() {
            let saved_name = final_path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(&safe_name)
                .to_string();
            chat_entry = Some(chat.lock().unwrap().append(ChatMessageInfo {
                seq: 0, // append が振る
                id: new_transfer_id(),
                scope: ctx.scope,
                group_id: ctx.group_id,
                from: peer_ip,
                to: match ctx.scope {
                    ChatScope::Direct => Some(own_ip),
                    ChatScope::Network | ChatScope::Group => None,
                },
                text: String::new(),
                sent_at: now_unix_ms(),
                failed: false,
                file: Some(ChatFileInfo {
                    name: saved_name,
                    size,
                    transfers: vec![id.clone()],
                    // UI のインラインプレビュー用(受信完了までは UI 側が
                    // 転送の進捗を見てプレビューしない)
                    path: Some(final_path.clone()),
                }),
                system: false,
            }));
        }
    }
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
        offset,
    )
    .await;
    match &result {
        Ok(()) => update(transfers, &id, |t| t.done = true),
        Err(e) => {
            // 中断再開(E-E 6): 送信側が対応していれば書きかけを保持して
            // 次の申し出で続きから受け取る。チェックサム不一致(壊れた書き
            // かけ)は捨てて最初からやり直す
            let checksum_broken = format!("{e:#}").contains("チェックサム");
            let keep_partial = resume
                && !checksum_broken
                && tokio::fs::metadata(&part_path)
                    .await
                    .map(|m| m.len() > 0)
                    .unwrap_or(false);
            if keep_partial {
                let meta = serde_json::json!({
                    "from": peer_ip.to_string(),
                    "name": safe_name,
                    "size": size,
                });
                let _ =
                    tokio::fs::write(append_suffix(&final_path, ".pcvresume"), meta.to_string())
                        .await;
            } else {
                let _ = tokio::fs::remove_file(&part_path).await;
                let _ = tokio::fs::remove_file(append_suffix(&final_path, ".pcvresume")).await;
            }
            mark_failed(transfers, &id, e);
            // チャット内ファイルの受信失敗は会話にお知らせを出す(M3-13e)。
            // バブルの失敗表示は転送一覧との突き合わせだが、一覧は直近分しか
            // 残らないため、履歴に残るメッセージでも分かるようにする
            if let Some(entry) = chat_entry {
                let name = entry
                    .file
                    .as_ref()
                    .map(|f| f.name.clone())
                    .unwrap_or_default();
                let mut log = chat.lock().unwrap();
                log.mark_failed(entry.seq);
                log.append(ChatMessageInfo {
                    seq: 0, // append が振る
                    id: new_transfer_id(),
                    scope: entry.scope,
                    group_id: entry.group_id,
                    from: peer_ip,
                    to: entry.to,
                    text: format!("{sender_name}からのファイル「{name}」を受信できませんでした"),
                    sent_at: now_unix_ms(),
                    failed: false,
                    file: None,
                    system: true,
                });
            }
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
    offset: u64,
) -> anyhow::Result<()> {
    // ハッシュは**ファイル全体**が対象(ADR-0015)。再開時は既存の書きかけを
    // 先にハッシュへ流し込み、本体は続き(offset 以降)だけ受け取る
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let mut file = if offset > 0 {
        {
            let mut existing = tokio::fs::File::open(part_path)
                .await
                .context("書きかけを開けません")?;
            let mut hashed: u64 = 0;
            while hashed < offset {
                let n = existing.read(&mut buf).await?;
                if n == 0 {
                    bail!("書きかけが途中で読めなくなりました");
                }
                hasher.update(&buf[..n]);
                hashed += n as u64;
            }
        }
        tokio::fs::OpenOptions::new()
            .append(true)
            .open(part_path)
            .await
            .context("書きかけを追記で開けません")?
    } else {
        tokio::fs::File::create(part_path)
            .await
            .context("受信ファイルを作成できません")?
    };
    send_frame(
        write_half,
        &MsgFrame::FileAccept {
            id: id.to_string(),
            offset,
        },
    )
    .await?;

    // 本体: take の上限を残りバイト数に切り替えて読む(超過分は読まない)
    let mut received: u64 = offset;
    reader.set_limit(size - offset);
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
    // 再開用の目印は完了したら不要
    let _ = tokio::fs::remove_file(append_suffix(final_path, ".pcvresume")).await;
    send_frame(write_half, &MsgFrame::FileDone { id: id.to_string() }).await?;
    tracing::info!("{sender_name}({peer_ip})からファイルを受信しました({size} バイト、id={id})");
    Ok(())
}

/// 再開できる書きかけを探す(E-E 6)。`<保存名>.pcvresume` の
/// {from, name, size} が申し出と一致し、`.part` の実体が size 以下のもの。
/// 戻りは (最終パス, .part パス, 再開位置)。見つけた目印は消す
/// (また失敗したら書き直される)。
async fn find_resumable(
    inbox: &Path,
    from: Ipv4Addr,
    name: &str,
    size: u64,
) -> Option<(PathBuf, PathBuf, u64)> {
    let mut dir = tokio::fs::read_dir(inbox).await.ok()?;
    while let Ok(Some(entry)) = dir.next_entry().await {
        let marker = entry.path();
        let Some(fname) = marker.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(base) = fname.strip_suffix(".pcvresume") else {
            continue;
        };
        let Ok(data) = tokio::fs::read_to_string(&marker).await else {
            continue;
        };
        let Ok(meta) = serde_json::from_str::<serde_json::Value>(&data) else {
            continue;
        };
        if meta.get("from").and_then(|v| v.as_str()) != Some(from.to_string().as_str())
            || meta.get("name").and_then(|v| v.as_str()) != Some(name)
            || meta.get("size").and_then(|v| v.as_u64()) != Some(size)
        {
            continue;
        }
        let final_path = inbox.join(base);
        let part_path = append_suffix(&final_path, ".part");
        let Ok(part_meta) = tokio::fs::metadata(&part_path).await else {
            let _ = tokio::fs::remove_file(&marker).await; // 実体のない残骸
            continue;
        };
        if part_meta.len() > size {
            continue;
        }
        let _ = tokio::fs::remove_file(&marker).await;
        return Some((final_path, part_path, part_meta.len()));
    }
    None
}

/// 送信側: 相手の仮想 IP のメッセージングポートへ接続してファイルを送る。
/// 進捗は `transfers` に反映される(UI / CLI は status 経由で追う)。
/// `chat` はチャット内ファイル送信の文脈(M3-13d、任意)。
pub async fn send_file(
    peer_ip: Ipv4Addr,
    path: &Path,
    transfers: TransferRegistry,
    id: String,
    chat: Option<ChatContext>,
) -> anyhow::Result<()> {
    send_file_to(
        SocketAddr::from((peer_ip, MSG_PORT)),
        peer_ip,
        path,
        transfers,
        id,
        chat,
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
    chat: Option<ChatContext>,
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
    let result = send_body(target, path, &transfers, &id, size, chat).await;
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
    chat: Option<ChatContext>,
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
            chat,
            resume: true, // 中断再開に対応(E-E 6)。旧受信側はこの欄を無視する
        },
    )
    .await?;
    let offset = match expect_frame(&mut reader, &mut line).await? {
        MsgFrame::FileAccept { id: ack_id, offset } if ack_id == id => {
            if offset > size {
                bail!("再開位置がファイルサイズを超えています({offset}/{size})");
            }
            offset
        }
        MsgFrame::FileReject { reason, .. } => bail!("相手が受信を拒否しました: {reason}"),
        other => bail!("FileAccept を期待しましたが別のフレームが届きました: {other:?}"),
    };
    if offset > 0 {
        tracing::info!("相手の書きかけの続きから送ります({offset}/{size} バイト、id={id})");
        update(transfers, id, |t| t.transferred = offset);
    }

    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("{} を開けません", path.display()))?;
    // ハッシュは**ファイル全体**が対象なので先頭から読むが、本体として流すのは
    // offset 以降だけ(再開時)
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let mut read_total: u64 = 0;
    let mut sent: u64 = 0;
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        let chunk_start = read_total;
        read_total += n as u64;
        if read_total > offset {
            let skip = offset.saturating_sub(chunk_start) as usize;
            tokio::time::timeout(IO_TIMEOUT, write_half.write_all(&buf[skip..n]))
                .await
                .map_err(|_| anyhow::anyhow!("転送がタイムアウトしました"))??;
            sent += (n - skip) as u64;
            update(transfers, id, |t| t.transferred = offset + sent);
        }
    }
    if offset + sent != size {
        // 送信中にファイルが書き換えられた等。受信側はサイズ不一致で検出する
        bail!(
            "ファイルサイズが途中で変わりました({}/{size} バイト)",
            offset + sent
        );
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

    /// テスト用のチャット履歴(一時ディレクトリに書く)。
    fn test_chat_log() -> crate::chat::SharedChatLog {
        crate::chat::ChatLog::load(&temp_dir("chatlog").join("net.toml"))
    }

    /// テスト用のグループ保存(一時ディレクトリに書く)。
    fn test_groups() -> crate::groups::SharedGroups {
        crate::groups::GroupStore::load(&temp_dir("groups").join("net.toml"))
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
                ip,
                "alice".to_string(),
                &Default::default(),
                &server_inbox,
                &server_transfers,
                limit_bytes(100),
                &test_chat_log(),
                &test_groups(),
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
            None,
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

    /// チャット文脈付きのファイル送信(M3-13d): 受信側の履歴に
    /// kind = file のエントリ(保存された実ファイル名 + 転送 id)が残る。
    #[tokio::test]
    async fn chat_context_file_records_history() {
        let inbox = temp_dir("chatfile");
        let src_dir = temp_dir("chatfilesrc");
        let src = src_dir.join("資料.bin");
        std::fs::write(&src, b"hello").unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let chat = test_chat_log();
        let server_chat = Arc::clone(&chat);
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
                "10.9.9.9".parse().unwrap(), // 受信側(自分)の仮想 IP に見立てる
                "alice".to_string(),
                &Default::default(),
                &server_inbox,
                &server_transfers,
                0,
                &server_chat,
                &test_groups(),
            )
            .await
        });

        send_file_to(
            addr,
            "127.0.0.1".parse().unwrap(),
            &src,
            Default::default(),
            "t-5".to_string(),
            Some(ChatContext {
                scope: ChatScope::Direct,
                group_id: None,
            }),
        )
        .await
        .unwrap();
        server.await.unwrap().unwrap();

        let entry = chat.lock().unwrap().fetch(0).1.pop().unwrap();
        assert_eq!(entry.scope, ChatScope::Direct);
        assert_eq!(entry.to, Some("10.9.9.9".parse().unwrap()));
        assert!(entry.text.is_empty());
        let file = entry.file.expect("ファイルのエントリ");
        assert_eq!(file.name, "資料.bin");
        assert_eq!(file.size, 5);
        assert_eq!(file.transfers, vec!["t-5".to_string()]);
        assert!(inbox.join("資料.bin").exists(), "実体は受信ボックス");
        let _ = std::fs::remove_dir_all(&inbox);
        let _ = std::fs::remove_dir_all(&src_dir);
    }

    /// チャット内ファイルの受信が途中で失敗すると、ファイルのエントリに
    /// 失敗の印が付き、お知らせ(system)が履歴に残る(M3-13e)。
    #[tokio::test]
    async fn receive_failure_appends_system_notice() {
        let inbox = temp_dir("chatfilefail");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let chat = test_chat_log();
        let server_chat = Arc::clone(&chat);
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
                "10.9.9.9".parse().unwrap(),
                "alice".to_string(),
                &Default::default(),
                &server_inbox,
                &server_transfers,
                0,
                &server_chat,
                &test_groups(),
            )
            .await
        });

        // 手で FileOffer を送り、本体の途中で切断する
        let stream = TcpStream::connect(addr).await.unwrap();
        let (read_half, mut write_half) = stream.into_split();
        let mut reader = line_reader(read_half);
        let mut line = String::new();
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
                id: "t-9".to_string(),
                name: "資料.bin".to_string(),
                size: 10,
                chat: Some(ChatContext {
                    scope: ChatScope::Direct,
                    group_id: None,
                }),
                resume: false,
            },
        )
        .await
        .unwrap();
        match expect_frame(&mut reader, &mut line).await.unwrap() {
            MsgFrame::FileAccept { .. } => {}
            other => panic!("FileAccept を期待しましたが {other:?}"),
        }
        write_half.write_all(b"abc").await.unwrap();
        drop(write_half);
        drop(reader);
        assert!(server.await.unwrap().is_err(), "受信は失敗する");

        let log = chat.lock().unwrap();
        let (_, messages) = log.fetch(0);
        assert_eq!(
            messages.len(),
            2,
            "ファイルのエントリ + お知らせ: {messages:?}"
        );
        assert!(messages[0].file.is_some());
        assert!(messages[0].failed, "ファイルのエントリに失敗の印");
        assert!(messages[1].system);
        assert!(messages[1].text.contains("受信できませんでした"));
        assert!(messages[1].text.contains("資料.bin"));
        let _ = std::fs::remove_dir_all(&inbox);
    }

    /// 受信サイズ上限(受け取る側の設定)を超える申し出は拒否され、
    /// 送信側には上限入りの理由が返る。
    #[tokio::test]
    async fn oversize_offer_is_rejected() {
        let inbox = temp_dir("limit");
        let src_dir = temp_dir("limitsrc");
        let src = src_dir.join("big.bin");
        std::fs::write(&src, vec![0u8; 2 * 1024 * 1024]).unwrap(); // 2 MB

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
                ip,
                "alice".to_string(),
                &Default::default(),
                &server_inbox,
                &server_transfers,
                limit_bytes(1), // 上限 1 MB
                &test_chat_log(),
                &test_groups(),
            )
            .await
        });

        let send_transfers: TransferRegistry = Default::default();
        let err = send_file_to(
            addr,
            "127.0.0.1".parse().unwrap(),
            &src,
            Arc::clone(&send_transfers),
            "t-4".to_string(),
            None,
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string().contains("上限(1 MB)"),
            "上限入りの理由が送信側へ届く: {err:#}"
        );
        assert!(server.await.unwrap().is_err());
        assert!(
            std::fs::read_dir(&inbox).map(|d| d.count()).unwrap_or(0) == 0,
            "受信ボックスには何も作られない"
        );
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
                ip,
                "mallory".to_string(),
                &Default::default(),
                &server_inbox,
                &server_transfers,
                0,
                &test_chat_log(),
                &test_groups(),
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
                chat: None,
                resume: false,
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

    /// チャット送信 → 受信側の履歴に記録され、ack が返る E2E。
    #[tokio::test]
    async fn chat_roundtrip() {
        let inbox = temp_dir("chatinbox");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let chat = test_chat_log();
        let server_chat = Arc::clone(&chat);
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
                "10.9.9.9".parse().unwrap(), // 受信側(自分)の仮想 IP に見立てる
                "alice".to_string(),
                &Default::default(),
                &server_inbox,
                &Default::default(),
                0,
                &server_chat,
                &test_groups(),
            )
            .await
        });

        send_chat_to(addr, "c-1", ChatScope::Direct, None, "こんにちは 🎉", 1_234)
            .await
            .unwrap();
        server.await.unwrap().unwrap();

        let log = chat.lock().unwrap();
        let (seq, messages) = log.fetch(0);
        assert_eq!(seq, 1);
        assert_eq!(messages.len(), 1);
        let entry = &messages[0];
        assert_eq!(entry.id, "c-1");
        assert_eq!(entry.scope, ChatScope::Direct);
        assert_eq!(entry.from, "127.0.0.1".parse::<Ipv4Addr>().unwrap());
        assert_eq!(
            entry.to,
            Some("10.9.9.9".parse().unwrap()),
            "direct は宛先 = 自分"
        );
        assert_eq!(entry.text, "こんにちは 🎉");
        assert_eq!(entry.sent_at, 1_234);
        assert!(!entry.failed);
        drop(log);
        let _ = std::fs::remove_dir_all(&inbox);
    }

    /// network 宛のチャットは to が付かない(会話の区別は scope で行う)。
    #[tokio::test]
    async fn network_chat_has_no_recipient() {
        let inbox = temp_dir("chatnet");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let chat = test_chat_log();
        let server_chat = Arc::clone(&chat);
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
                ip,
                "bob".to_string(),
                &Default::default(),
                &server_inbox,
                &Default::default(),
                0,
                &server_chat,
                &test_groups(),
            )
            .await
        });
        send_chat_to(addr, "c-2", ChatScope::Network, None, "全体宛", 1)
            .await
            .unwrap();
        server.await.unwrap().unwrap();
        let entry = chat.lock().unwrap().fetch(0).1.pop().unwrap();
        assert_eq!(entry.scope, ChatScope::Network);
        assert_eq!(entry.to, None);
        let _ = std::fs::remove_dir_all(&inbox);
    }

    /// group 宛のチャットは group_id 付きで履歴に残る(M3-13c)。
    #[tokio::test]
    async fn group_chat_records_group_id() {
        let inbox = temp_dir("chatgroup");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let chat = test_chat_log();
        let server_chat = Arc::clone(&chat);
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
                ip,
                "carol".to_string(),
                &Default::default(),
                &server_inbox,
                &Default::default(),
                0,
                &server_chat,
                &test_groups(),
            )
            .await
        });
        send_chat_to(addr, "c-4", ChatScope::Group, Some("g1"), "グループ宛", 1)
            .await
            .unwrap();
        server.await.unwrap().unwrap();
        let entry = chat.lock().unwrap().fetch(0).1.pop().unwrap();
        assert_eq!(entry.scope, ChatScope::Group);
        assert_eq!(entry.group_id.as_deref(), Some("g1"));
        assert_eq!(entry.to, None);
        let _ = std::fs::remove_dir_all(&inbox);
    }

    /// グループ更新の送信 → 受信側の保存に取り込まれ、ack が返る E2E(M3-13c)。
    /// 古い revision の再送は取り込まれない(が ack は返る)。
    #[tokio::test]
    async fn group_update_roundtrip() {
        let inbox = temp_dir("groupupd");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let groups = test_groups();
        {
            let mut store = groups.lock().unwrap();
            // 送信元(loopback = 127.0.0.1)を現メンバーに含める。既知グループの
            // 更新は現メンバーからのみ受理するため(accepts_update)。
            store.apply(GroupInfo {
                id: "g1".to_string(),
                name: "旧名".to_string(),
                members: vec!["127.0.0.1".parse().unwrap(), "10.0.0.1".parse().unwrap()],
                revision: 1,
                updated_by: "10.0.0.1".parse().unwrap(),
            });
        }
        // 2 接続受ける(新しい revision → 古い revision)
        let server_groups = Arc::clone(&groups);
        let server_inbox = inbox.clone();
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (stream, peer) = listener.accept().await.unwrap();
                let ip = match peer.ip() {
                    IpAddr::V4(ip) => ip,
                    _ => unreachable!(),
                };
                handle_incoming(
                    stream,
                    ip,
                    ip,
                    "alice".to_string(),
                    &Default::default(),
                    &server_inbox,
                    &Default::default(),
                    0,
                    &test_chat_log(),
                    &server_groups,
                )
                .await?;
            }
            anyhow::Ok(())
        });

        // 送信元(このテストは loopback から接続するので peer_ip = 127.0.0.1)を
        // メンバーに含める。認可(accepts_update)は送信元がメンバーであることを
        // 要求するため、実運用でも更新を配るのはそのグループのメンバーになる。
        let newer = GroupInfo {
            id: "g1".to_string(),
            name: "新名".to_string(),
            members: vec!["127.0.0.1".parse().unwrap(), "10.0.0.2".parse().unwrap()],
            revision: 2,
            updated_by: "10.0.0.2".parse().unwrap(),
        };
        send_group_update_to(addr, &newer).await.unwrap();
        let stale = GroupInfo {
            revision: 1,
            name: "巻き戻し".to_string(),
            ..newer.clone()
        };
        send_group_update_to(addr, &stale).await.unwrap();
        server.await.unwrap().unwrap();

        let store = groups.lock().unwrap();
        let current = store.get("g1").unwrap();
        assert_eq!(current.revision, 2, "古い再送では巻き戻らない");
        assert_eq!(current.name, "新名");
        assert_eq!(current.members.len(), 2);
        drop(store);
        let _ = std::fs::remove_dir_all(&inbox);
    }

    /// 本文が上限を超えるチャットは記録されず、送信側はエラーになる。
    #[tokio::test]
    async fn oversized_chat_is_rejected() {
        let inbox = temp_dir("chatbig");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let chat = test_chat_log();
        let server_chat = Arc::clone(&chat);
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
                ip,
                "mallory".to_string(),
                &Default::default(),
                &server_inbox,
                &Default::default(),
                0,
                &server_chat,
                &test_groups(),
            )
            .await
        });
        let big = "a".repeat(MAX_CHAT_TEXT_BYTES + 1);
        let err = send_chat_to(addr, "c-3", ChatScope::Direct, None, &big, 1)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("切断"), "ack が返らない: {err:#}");
        assert!(server.await.unwrap().is_err());
        assert!(chat.lock().unwrap().fetch(0).1.is_empty(), "履歴に残らない");
        let _ = std::fs::remove_dir_all(&inbox);
    }

    /// 中断再開(E-E 6)の受信側ハーネス: 1 接続だけ受けて処理する。
    fn spawn_recv_server(
        listener: TcpListener,
        inbox: PathBuf,
        transfers: TransferRegistry,
    ) -> tokio::task::JoinHandle<anyhow::Result<()>> {
        tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.unwrap();
            let ip = match peer.ip() {
                IpAddr::V4(ip) => ip,
                _ => unreachable!(),
            };
            handle_incoming(
                stream,
                ip,
                ip,
                "alice".to_string(),
                &Default::default(),
                &inbox,
                &transfers,
                0,
                &test_chat_log(),
                &test_groups(),
            )
            .await
        })
    }

    /// resume 付きの申し出を送って FileAccept(offset)を返してもらう。
    async fn offer_resumable(
        addr: SocketAddr,
        id: &str,
        name: &str,
        size: u64,
    ) -> (LineReader, tokio::net::tcp::OwnedWriteHalf, u64) {
        let stream = TcpStream::connect(addr).await.unwrap();
        let (read_half, mut write_half) = stream.into_split();
        let mut reader = line_reader(read_half);
        let mut line = String::new();
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
                id: id.to_string(),
                name: name.to_string(),
                size,
                chat: None,
                resume: true,
            },
        )
        .await
        .unwrap();
        let offset = match expect_frame(&mut reader, &mut line).await.unwrap() {
            MsgFrame::FileAccept { offset, .. } => offset,
            other => panic!("FileAccept を期待しましたが {other:?}"),
        };
        (reader, write_half, offset)
    }

    /// 中断再開(E-E 6): 途中で切れた受信の書きかけが保持され、同じファイルの
    /// 再送では FileAccept.offset で続きから受け取り、全体ハッシュで検証される。
    #[tokio::test]
    async fn interrupted_transfer_resumes_from_partial() {
        let inbox = temp_dir("resume");
        let payload: Vec<u8> = (0..50_000u32).flat_map(|i| i.to_le_bytes()).collect();
        let size = payload.len() as u64;
        let half = payload.len() / 2;

        // 1 回目: 半分だけ送って切断 → 書きかけ + 再開の目印が残る
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_recv_server(listener, inbox.clone(), Default::default());
        let (reader, mut write_half, offset) = offer_resumable(addr, "r-1", "big.bin", size).await;
        assert_eq!(offset, 0, "初回は先頭から");
        write_half.write_all(&payload[..half]).await.unwrap();
        drop(write_half);
        drop(reader);
        assert!(server.await.unwrap().is_err(), "途中切断で受信は失敗");
        assert_eq!(
            std::fs::metadata(inbox.join("big.bin.part")).unwrap().len(),
            half as u64,
            "書きかけが保持される"
        );
        assert!(inbox.join("big.bin.pcvresume").exists(), "再開の目印");

        // 2 回目: 続き(offset = 書きかけの長さ)から送って完了する
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_recv_server(listener, inbox.clone(), Default::default());
        let (mut reader, mut write_half, offset) =
            offer_resumable(addr, "r-2", "big.bin", size).await;
        assert_eq!(offset, half as u64, "書きかけの続きを要求される");
        write_half.write_all(&payload[half..]).await.unwrap();
        let mut hasher = Sha256::new();
        hasher.update(&payload);
        send_frame(
            &mut write_half,
            &MsgFrame::FileHash {
                id: "r-2".to_string(),
                sha256: hex(&hasher.finalize()),
            },
        )
        .await
        .unwrap();
        let mut line = String::new();
        match expect_frame(&mut reader, &mut line).await.unwrap() {
            MsgFrame::FileDone { .. } => {}
            other => panic!("FileDone を期待しましたが {other:?}"),
        }
        server.await.unwrap().unwrap();
        assert_eq!(
            std::fs::read(inbox.join("big.bin")).unwrap(),
            payload,
            "全体が正しく揃う"
        );
        assert!(!inbox.join("big.bin.part").exists());
        assert!(!inbox.join("big.bin.pcvresume").exists(), "目印は消える");
        let _ = std::fs::remove_dir_all(&inbox);
    }

    /// 壊れた書きかけからの再開はハッシュ不一致で失敗し、書きかけごと捨てられる
    /// (次の申し出は最初からになる)。
    #[tokio::test]
    async fn corrupted_partial_is_discarded_on_resume() {
        let inbox = temp_dir("resumebad");
        let payload = vec![7u8; 8192];
        let size = payload.len() as u64;
        let half = payload.len() / 2;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_recv_server(listener, inbox.clone(), Default::default());
        let (reader, mut write_half, _) = offer_resumable(addr, "b-1", "x.bin", size).await;
        write_half.write_all(&payload[..half]).await.unwrap();
        drop(write_half);
        drop(reader);
        assert!(server.await.unwrap().is_err());

        // 書きかけを壊す(長さは保ったまま中身を変える)
        std::fs::write(inbox.join("x.bin.part"), vec![9u8; half]).unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = spawn_recv_server(listener, inbox.clone(), Default::default());
        let (_reader, mut write_half, offset) = offer_resumable(addr, "b-2", "x.bin", size).await;
        assert_eq!(offset, half as u64);
        write_half.write_all(&payload[half..]).await.unwrap();
        let mut hasher = Sha256::new();
        hasher.update(&payload);
        send_frame(
            &mut write_half,
            &MsgFrame::FileHash {
                id: "b-2".to_string(),
                sha256: hex(&hasher.finalize()),
            },
        )
        .await
        .unwrap();
        let err = server.await.unwrap().unwrap_err();
        assert!(format!("{err:#}").contains("チェックサム"), "{err:#}");
        assert!(!inbox.join("x.bin").exists());
        assert!(!inbox.join("x.bin.part").exists(), "壊れた書きかけは捨てる");
        assert!(!inbox.join("x.bin.pcvresume").exists());
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
                ip,
                "eve".to_string(),
                &Default::default(),
                &server_inbox,
                &server_transfers,
                0,
                &test_chat_log(),
                &test_groups(),
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
                chat: None,
                resume: false,
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
