//! ネットワークセッション(M4 E-C)。トンネル上で動く PeerCove のプロトコル層:
//!
//! - コントロールチャネル(TCP 51821、client): hello / 台帳受信 / ping-pong /
//!   削除・参加拒否通知。デスクトップの control.rs メンバー側と同じワイヤ動作を
//!   ブロッキング I/O + スレッドで実装する(モバイルは tokio を持ち込まない)
//! - メッセージング(TCP 51822): 自分の仮想 IP で待受け(チャット受信・
//!   ファイル受信・グループ更新受信)+ 送信(チャット・ファイル)。
//!   ワイヤは peercove-core::msg(デスクトップの msg.rs と互換)
//!
//! 認証はどちらも「トンネル内 = WG が暗号化・認証済み。接続元の仮想 IP が
//! 身元」。メッセージ受信は台帳のメンバー仮想 IP と照合する。
//!
//! 秘匿ルール: チャット本文・グループ名・ファイル名の中身はログへ出さない
//! (seq・id・IP・サイズは可)。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use peercove_core::dns::{CnameRecord, DnsRecord};
use peercove_core::ipc::{ChatFileInfo, ChatMessageInfo};
use peercove_core::msg::{
    ChatContext, ChatScope, MsgFrame, MAX_CHAT_TEXT_BYTES, MAX_GROUP_MEMBERS, MAX_GROUP_NAME_BYTES,
    MSG_VERSION,
};
use peercove_core::proto::{ControlMessage, LedgerEntry, PROTO_VERSION};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::chatlog::ChatLog;
use crate::groups::{self, GroupStore};

/// 受信 1 行の上限(desktop control.rs / msg.rs と同値)。
/// take の上限は**累計**なので 1 行ごとに set_limit で戻す(roadmap §4-10 の罠)。
const MAX_LINE: u64 = 64 * 1024;
/// コントロールチャネルの再接続間隔・ping 間隔(desktop と同値)。
const RETRY_INTERVAL: Duration = Duration::from_secs(5);
const PING_INTERVAL: Duration = Duration::from_secs(5);
/// 短命接続(メッセージング)の各種タイムアウト。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const IO_TIMEOUT: Duration = Duration::from_secs(15);
/// スマホがホストへ名乗る追加機能(実装済みのものだけ)。
const MOBILE_CAPABILITIES: &[&str] = &["chat", "file_transfer"];
/// スマホの受信ファイルサイズ上限の既定(MB)。デスクトップは 100 MB だが
/// スマホはストレージが限られるため小さくする(2026-07-19 依頼者指定)。
pub const MOBILE_DEFAULT_MAX_RECV_FILE_MB: u64 = 10;

/// セッション 1 本の設定。アドレスを差し替え可能にしてあるのはテストのため
/// (本番は control = ホスト仮想 IP:51821、listen = 自分の仮想 IP:51822)。
pub struct SessionConfig {
    pub slug: String,
    /// networks/<slug>/member.toml(履歴・グループ・受信ボックスの基準)
    pub config_path: PathBuf,
    pub own_ip: Ipv4Addr,
    pub display_name: Option<String>,
    pub device_id: Option<String>,
    pub network_name: String,
    pub control_addr: SocketAddr,
    pub listen_addr: SocketAddr,
    /// 相手のメッセージングポート(本番は 51822。テストで曲げる)
    pub peer_msg_port: u16,
}

/// 台帳のスナップショット(コントロールチャネルの受信で更新)。
#[derive(Default)]
pub struct LedgerSnapshot {
    pub members: Vec<LedgerEntry>,
    pub dns_records: Vec<DnsRecord>,
    pub cname_records: Vec<CnameRecord>,
}

/// ファイル転送の進捗(UI がポーリングする)。
#[derive(Clone)]
pub struct TransferInfo {
    pub id: String,
    pub peer: Ipv4Addr,
    pub name: String,
    pub size: u64,
    pub done: u64,
    pub outgoing: bool,
    /// "running" / "done" / "failed: 理由"
    pub state: String,
}

pub struct SessionShared {
    pub cfg: SessionConfig,
    pub ledger: Mutex<Option<LedgerSnapshot>>,
    pub control_connected: AtomicBool,
    pub removed: AtomicBool,
    pub rejected: Mutex<Option<String>>,
    pub rtt_ms: Mutex<Option<u64>>,
    pub chat: Mutex<ChatLog>,
    pub groups: Mutex<GroupStore>,
    pub transfers: Mutex<Vec<TransferInfo>>,
    /// listener が実際に bind したアドレス(テストが接続先を知るため)
    pub bound_listen: Mutex<Option<SocketAddr>>,
    /// コントロールチャネルへ差し込む送信キュー(表示名・DNS 名の変更依頼)
    outbox: Mutex<Vec<ControlMessage>>,
    /// SetDnsName / SetDisplayName の応答受け口((accepted, message))
    dns_result: Mutex<Option<(bool, String)>>,
    display_result: Mutex<Option<(bool, String)>>,
    stop: AtomicBool,
}

pub struct NetSession {
    pub shared: Arc<SessionShared>,
    threads: Vec<JoinHandle<()>>,
}

impl NetSession {
    pub fn start(cfg: SessionConfig) -> NetSession {
        let shared = Arc::new(SessionShared {
            chat: Mutex::new(ChatLog::load(&cfg.config_path)),
            groups: Mutex::new(GroupStore::load(&cfg.config_path)),
            ledger: Mutex::new(None),
            control_connected: AtomicBool::new(false),
            removed: AtomicBool::new(false),
            rejected: Mutex::new(None),
            rtt_ms: Mutex::new(None),
            transfers: Mutex::new(Vec::new()),
            bound_listen: Mutex::new(None),
            outbox: Mutex::new(Vec::new()),
            dns_result: Mutex::new(None),
            display_result: Mutex::new(None),
            stop: AtomicBool::new(false),
            cfg,
        });
        let threads = vec![
            spawn_named("peercove-control", Arc::clone(&shared), control_loop),
            spawn_named("peercove-msg", Arc::clone(&shared), listener_loop),
        ];
        NetSession { shared, threads }
    }

    pub fn stop(self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        for t in self.threads {
            let _ = t.join();
        }
    }
}

fn spawn_named(
    name: &str,
    shared: Arc<SessionShared>,
    f: fn(&Arc<SessionShared>),
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name(name.to_string())
        .spawn(move || f(&shared))
        .expect("スレッド起動に失敗")
}

impl SessionShared {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn sleep_with_stop(&self, total: Duration) {
        let step = Duration::from_millis(200);
        let mut waited = Duration::ZERO;
        while waited < total && !self.stopped() {
            std::thread::sleep(step);
            waited += step;
        }
    }

    /// 台帳から送信元メンバーを引く(メッセージ受信の認証)。
    pub(crate) fn ledger_member(&self, ip: Ipv4Addr) -> Option<LedgerEntry> {
        self.ledger
            .lock()
            .unwrap()
            .as_ref()?
            .members
            .iter()
            .find(|m| m.ip == ip)
            .cloned()
    }

    pub(crate) fn member_display(&self, ip: Ipv4Addr) -> String {
        if ip == self.cfg.own_ip {
            return "自分".to_string();
        }
        self.ledger_member(ip)
            .and_then(|m| m.name)
            .unwrap_or_else(|| ip.to_string())
    }

    /// 受信ファイルサイズ上限(バイト、0 = 無制限)。member.toml の
    /// `max_recv_file_mb`(デスクトップと同じ設定)を申し出ごとに読む
    /// (設定変更を再起動なしで反映)。
    fn recv_limit_bytes(&self) -> u64 {
        let mb = peercove_core::config::Config::load(&self.cfg.config_path)
            .map(|c| c.interface.max_recv_file_mb)
            .unwrap_or(MOBILE_DEFAULT_MAX_RECV_FILE_MB);
        mb.saturating_mul(1024 * 1024)
    }

    fn inbox_dir(&self) -> PathBuf {
        self.cfg.config_path.with_extension("inbox")
    }

    fn upsert_transfer(&self, info: TransferInfo) {
        let mut list = self.transfers.lock().unwrap();
        if let Some(existing) = list.iter_mut().find(|t| t.id == info.id) {
            *existing = info;
        } else {
            list.push(info);
            // 終了済みを増やしすぎない(直近 50 件)
            let len = list.len();
            if len > 50 {
                let running: Vec<TransferInfo> = list
                    .iter()
                    .filter(|t| t.state == "running")
                    .cloned()
                    .collect();
                let mut finished: Vec<TransferInfo> = list
                    .iter()
                    .filter(|t| t.state != "running")
                    .cloned()
                    .collect();
                let keep = 50usize.saturating_sub(running.len());
                if finished.len() > keep {
                    finished.drain(..finished.len() - keep);
                }
                *list = finished;
                list.extend(running);
            }
        }
    }

    fn transfer_progress(&self, id: &str, done: u64) {
        if let Some(t) = self
            .transfers
            .lock()
            .unwrap()
            .iter_mut()
            .find(|t| t.id == id)
        {
            t.done = done;
        }
    }

    fn transfer_state(&self, id: &str, state: &str) {
        if let Some(t) = self
            .transfers
            .lock()
            .unwrap()
            .iter_mut()
            .find(|t| t.id == id)
        {
            t.state = state.to_string();
        }
    }
}

// ---- 行フレーミング(JSON Lines)---------------------------------------------

type LineReader = BufReader<std::io::Take<TcpStream>>;

fn line_reader(stream: TcpStream) -> LineReader {
    BufReader::new(stream.take(MAX_LINE))
}

fn send_json<W: Write, T: Serialize>(writer: &mut W, value: &T) -> anyhow::Result<()> {
    let mut line = serde_json::to_string(value).context("メッセージの直列化に失敗しました")?;
    line.push('\n');
    writer
        .write_all(line.as_bytes())
        .context("送信に失敗しました")?;
    Ok(())
}

enum ReadOutcome {
    /// 完全な 1 行が `line` に入った
    Line,
    Eof,
    /// read timeout(呼び出し側は fresh=false で継続して部分行を保持する)
    Timeout,
}

/// タイムアウト付きソケットから 1 行読む。`fresh` のときだけ行バッファと
/// take 上限をリセットする(タイムアウト継続で部分行を失わないため)。
fn read_line_step(
    reader: &mut LineReader,
    line: &mut String,
    fresh: bool,
) -> anyhow::Result<ReadOutcome> {
    if fresh {
        line.clear();
        reader.get_mut().set_limit(MAX_LINE);
    }
    match reader.read_line(line) {
        Ok(0) => {
            if line.is_empty() {
                Ok(ReadOutcome::Eof)
            } else if reader.get_ref().limit() == 0 {
                bail!("1 行が上限({MAX_LINE} バイト)を超えました")
            } else {
                Ok(ReadOutcome::Eof) // 行の途中で切断
            }
        }
        Ok(_) => {
            if line.ends_with('\n') {
                Ok(ReadOutcome::Line)
            } else if reader.get_ref().limit() == 0 {
                bail!("1 行が上限({MAX_LINE} バイト)を超えました")
            } else {
                Ok(ReadOutcome::Eof)
            }
        }
        Err(e)
            if e.kind() == std::io::ErrorKind::WouldBlock
                || e.kind() == std::io::ErrorKind::TimedOut =>
        {
            Ok(ReadOutcome::Timeout)
        }
        Err(e) => Err(e).context("受信に失敗しました"),
    }
}

/// ブロッキングで 1 行読む(タイムアウトはエラー扱い。短命接続用)。
fn read_line_blocking(reader: &mut LineReader, line: &mut String) -> anyhow::Result<()> {
    let mut fresh = true;
    loop {
        match read_line_step(reader, line, fresh)? {
            ReadOutcome::Line => return Ok(()),
            ReadOutcome::Eof => bail!("相手が切断しました"),
            ReadOutcome::Timeout => {
                // 短命接続では read_timeout(15 秒)まるごと待って良い
                fresh = false;
            }
        }
    }
}

fn read_msg_frame(reader: &mut LineReader, line: &mut String) -> anyhow::Result<MsgFrame> {
    read_line_blocking(reader, line)?;
    serde_json::from_str::<MsgFrame>(line.trim_end()).context("フレームを解析できません")
}

// ---- コントロールチャネル(client)------------------------------------------

fn control_loop(shared: &Arc<SessionShared>) {
    let mut logged_wait = false;
    while !shared.stopped() {
        match TcpStream::connect_timeout(&shared.cfg.control_addr, Duration::from_secs(3)) {
            Ok(stream) => {
                logged_wait = false;
                tracing::info!(
                    "コントロールチャネルに接続しました({})",
                    shared.cfg.control_addr
                );
                shared.control_connected.store(true, Ordering::Relaxed);
                let result = control_session(shared, stream);
                shared.control_connected.store(false, Ordering::Relaxed);
                *shared.rtt_ms.lock().unwrap() = None;
                if shared.removed.load(Ordering::Relaxed) {
                    tracing::info!("削除通知を受けたので再接続を停止します");
                    return;
                }
                if shared.rejected.lock().unwrap().is_some() {
                    tracing::warn!("ホストが参加を拒否したため再接続を停止します");
                    return;
                }
                if let Err(e) = result {
                    tracing::debug!("制御接続が終了しました(再接続します): {e:#}");
                }
            }
            Err(e) => {
                if !logged_wait {
                    tracing::info!("コントロールチャネル接続待ち(トンネル確立後に自動接続): {e}");
                    logged_wait = true;
                }
            }
        }
        shared.sleep_with_stop(RETRY_INTERVAL);
    }
}

fn control_session(shared: &Arc<SessionShared>, stream: TcpStream) -> anyhow::Result<()> {
    stream.set_nodelay(true).ok();
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .ok();
    let mut writer = stream.try_clone().context("ソケット複製に失敗しました")?;
    send_json(
        &mut writer,
        &ControlMessage::Hello {
            version: PROTO_VERSION,
            name: shared.cfg.display_name.clone(),
            app_version: Some(env!("CARGO_PKG_VERSION").to_string()),
            capabilities: MOBILE_CAPABILITIES.iter().map(|c| c.to_string()).collect(),
            device_id: shared.cfg.device_id.clone(),
        },
    )?;

    let mut reader = line_reader(stream);
    let mut line = String::new();
    let mut fresh = true;
    let mut last_ping = Instant::now();
    let mut nonce: u64 = 0;
    let mut pending_ping: Option<(u64, Instant)> = None;

    loop {
        if shared.stopped() {
            return Ok(());
        }
        match read_line_step(&mut reader, &mut line, fresh)? {
            ReadOutcome::Timeout => fresh = false,
            ReadOutcome::Eof => bail!("ホストが切断しました"),
            ReadOutcome::Line => {
                fresh = true;
                match serde_json::from_str::<ControlMessage>(line.trim_end()) {
                    Ok(ControlMessage::Ledger {
                        members,
                        dns_records,
                        cname_records,
                    }) => {
                        tracing::debug!("台帳を受信しました({} 名)", members.len());
                        *shared.ledger.lock().unwrap() = Some(LedgerSnapshot {
                            members,
                            dns_records,
                            cname_records,
                        });
                    }
                    Ok(ControlMessage::Ping { nonce }) => {
                        send_json(&mut writer, &ControlMessage::Pong { nonce })?;
                    }
                    Ok(ControlMessage::Pong { nonce: got }) => {
                        if let Some((expected, at)) = pending_ping.take() {
                            if got == expected {
                                *shared.rtt_ms.lock().unwrap() =
                                    Some(at.elapsed().as_millis() as u64);
                            }
                        }
                    }
                    Ok(ControlMessage::Removed { message }) => {
                        tracing::warn!("ホストから削除されました: {message}");
                        *shared.ledger.lock().unwrap() = None;
                        shared.removed.store(true, Ordering::Relaxed);
                        bail!("削除通知を受信");
                    }
                    Ok(ControlMessage::JoinRejected { message }) => {
                        tracing::warn!("ホストが参加を拒否しました: {message}");
                        *shared.ledger.lock().unwrap() = None;
                        *shared.rejected.lock().unwrap() = Some(message);
                        bail!("参加拒否通知を受信");
                    }
                    Ok(ControlMessage::SetDnsNameResult { accepted, message }) => {
                        *shared.dns_result.lock().unwrap() = Some((accepted, message));
                    }
                    Ok(ControlMessage::SetDisplayNameResult { accepted, message }) => {
                        *shared.display_result.lock().unwrap() = Some((accepted, message));
                    }
                    Ok(other) => tracing::debug!("未処理のメッセージ: {other:?}"),
                    Err(e) => tracing::debug!("解析できないメッセージを無視: {e}"),
                }
            }
        }
        // 変更依頼(表示名・DNS 名)の差し込み送信
        let queued: Vec<ControlMessage> = shared.outbox.lock().unwrap().drain(..).collect();
        for message in queued {
            send_json(&mut writer, &message)?;
        }
        if last_ping.elapsed() >= PING_INTERVAL {
            last_ping = Instant::now();
            nonce += 1;
            pending_ping = Some((nonce, Instant::now()));
            send_json(&mut writer, &ControlMessage::Ping { nonce })?;
        }
    }
}

// ---- メッセージング(listener)-----------------------------------------------

fn listener_loop(shared: &Arc<SessionShared>) {
    let mut logged_bind_error = false;
    while !shared.stopped() {
        let listener = match TcpListener::bind(shared.cfg.listen_addr) {
            Ok(l) => l,
            Err(e) => {
                // トンネル確立前(仮想 IP がまだ無い)は失敗して当然
                if !logged_bind_error {
                    tracing::info!("メッセージ待受の準備待ち: {e}");
                    logged_bind_error = true;
                }
                shared.sleep_with_stop(Duration::from_secs(1));
                continue;
            }
        };
        logged_bind_error = false;
        if listener.set_nonblocking(true).is_err() {
            shared.sleep_with_stop(Duration::from_secs(1));
            continue;
        }
        if let Ok(addr) = listener.local_addr() {
            tracing::info!("メッセージ待受を開始しました({addr})");
            *shared.bound_listen.lock().unwrap() = Some(addr);
        }
        while !shared.stopped() {
            match listener.accept() {
                Ok((stream, addr)) => {
                    let shared = Arc::clone(shared);
                    std::thread::spawn(move || {
                        if let Err(e) = handle_conn(&shared, stream, addr) {
                            tracing::debug!("メッセージ接続を終了({addr}): {e:#}");
                        }
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(200));
                }
                Err(e) => {
                    tracing::warn!("メッセージ待受が失敗しました(再作成します): {e}");
                    break;
                }
            }
        }
    }
}

fn handle_conn(
    shared: &Arc<SessionShared>,
    stream: TcpStream,
    addr: SocketAddr,
) -> anyhow::Result<()> {
    let std::net::IpAddr::V4(peer_ip) = addr.ip() else {
        bail!("IPv4 以外の接続元");
    };
    // 認証: 台帳のメンバー仮想 IP と照合(ADR-0015)。遮断中の相手も拒否
    let sender = shared
        .ledger_member(peer_ip)
        .with_context(|| format!("台帳に無い送信元({peer_ip})"))?;
    if sender.blocked {
        bail!("遮断中の相手からの接続を拒否({peer_ip})");
    }
    let sender_name = sender.name.unwrap_or_else(|| peer_ip.to_string());

    stream.set_nodelay(true).ok();
    stream.set_read_timeout(Some(IO_TIMEOUT)).ok();
    let mut writer = stream.try_clone().context("ソケット複製に失敗しました")?;
    let mut reader = line_reader(stream);
    let mut line = String::new();

    match read_msg_frame(&mut reader, &mut line)? {
        MsgFrame::Hello { version } => {
            if version != MSG_VERSION {
                tracing::warn!(
                    "{peer_ip} のメッセージングバージョン {version} は未対応です(こちらは {MSG_VERSION})"
                );
            }
        }
        other => bail!("Hello 以外のフレームが届きました: {other:?}"),
    }

    match read_msg_frame(&mut reader, &mut line)? {
        MsgFrame::FileOffer {
            id,
            name,
            size,
            chat: chat_ctx,
        } => {
            let limit = shared.recv_limit_bytes();
            if limit > 0 && size > limit {
                send_json(
                    &mut writer,
                    &MsgFrame::FileReject {
                        id,
                        reason: format!(
                            "サイズが受信側の上限({} MB)を超えています",
                            limit / (1024 * 1024)
                        ),
                    },
                )?;
                bail!("上限を超える申し出を拒否しました({peer_ip}、{size} バイト)");
            }
            receive_file(
                shared,
                &mut reader,
                &mut writer,
                peer_ip,
                &sender_name,
                chat_ctx,
                id,
                &name,
                size,
            )
        }
        MsgFrame::Chat {
            id,
            scope,
            group_id,
            text,
            sent_at,
        } => {
            if text.len() > MAX_CHAT_TEXT_BYTES {
                bail!("本文が上限({MAX_CHAT_TEXT_BYTES} バイト)を超えています({peer_ip})");
            }
            if scope == ChatScope::Group && group_id.is_none() {
                bail!("group 宛なのに group_id がありません({peer_ip})");
            }
            let entry = ChatMessageInfo {
                seq: 0,
                id: id.clone(),
                scope,
                group_id,
                from: peer_ip,
                to: match scope {
                    ChatScope::Direct => Some(shared.cfg.own_ip),
                    ChatScope::Network | ChatScope::Group => None,
                },
                text,
                sent_at,
                failed: false,
                file: None,
                system: false,
            };
            shared.chat.lock().unwrap().append(entry);
            send_json(&mut writer, &MsgFrame::ChatAck { id: id.clone() })?;
            tracing::info!("{sender_name}({peer_ip})からチャットを受信しました(id={id})");
            Ok(())
        }
        MsgFrame::GroupUpdate { group } => {
            if group.id.is_empty()
                || group.name.is_empty()
                || group.name.len() > MAX_GROUP_NAME_BYTES
                || group.members.len() > MAX_GROUP_MEMBERS
            {
                bail!("不正なグループ更新を拒否しました({peer_ip})");
            }
            let id = group.id.clone();
            let applied = {
                let mut store = shared.groups.lock().unwrap();
                if !store.accepts_update(&group, peer_ip) {
                    bail!("権限のないグループ更新を拒否しました({peer_ip})");
                }
                store.apply(group.clone())
            };
            send_json(&mut writer, &MsgFrame::GroupAck { id: id.clone() })?;
            if let Some(update) = applied {
                let name_of = |ip: Ipv4Addr| shared.member_display(ip);
                for text in groups::system_messages(
                    update.previous.as_ref(),
                    &group,
                    shared.cfg.own_ip,
                    &name_of,
                ) {
                    shared.chat.lock().unwrap().append(ChatMessageInfo {
                        seq: 0,
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
                tracing::info!("{sender_name}({peer_ip})からグループ更新を受信しました(id={id})");
            }
            Ok(())
        }
        other => bail!("未対応のフレーム: {other:?}"),
    }
}

#[allow(clippy::too_many_arguments)]
fn receive_file(
    shared: &Arc<SessionShared>,
    reader: &mut LineReader,
    writer: &mut TcpStream,
    peer_ip: Ipv4Addr,
    sender_name: &str,
    chat_ctx: Option<ChatContext>,
    id: String,
    name: &str,
    size: u64,
) -> anyhow::Result<()> {
    let Some(clean) = sanitize_file_name(name) else {
        send_json(
            writer,
            &MsgFrame::FileReject {
                id,
                reason: "ファイル名が不正です".to_string(),
            },
        )?;
        bail!("不正なファイル名を拒否しました({peer_ip})");
    };
    let inbox = shared.inbox_dir();
    std::fs::create_dir_all(&inbox)
        .with_context(|| format!("{} を作成できません", inbox.display()))?;
    let dest = unique_path(&inbox, &clean);
    let part = append_suffix(&dest, ".part");

    shared.upsert_transfer(TransferInfo {
        id: id.clone(),
        peer: peer_ip,
        name: clean.clone(),
        size,
        done: 0,
        outgoing: false,
        state: "running".to_string(),
    });

    let result = (|| -> anyhow::Result<()> {
        send_json(writer, &MsgFrame::FileAccept { id: id.clone() })?;

        // 本体: take の上限を本体サイズに切り替えて読む(超過分は読まない)
        reader.get_mut().set_limit(size);
        let mut file = std::fs::File::create(&part)
            .with_context(|| format!("{} を作成できません", part.display()))?;
        let mut hasher = Sha256::new();
        let mut remaining = size;
        let mut buf = [0u8; 64 * 1024];
        while remaining > 0 {
            let want = remaining.min(buf.len() as u64) as usize;
            let n = reader
                .read(&mut buf[..want])
                .context("本体の受信に失敗しました")?;
            if n == 0 {
                bail!("本体の途中で切断されました(残り {remaining} バイト)");
            }
            file.write_all(&buf[..n]).context("保存に失敗しました")?;
            hasher.update(&buf[..n]);
            remaining -= n as u64;
            shared.transfer_progress(&id, size - remaining);
        }
        file.flush().ok();
        drop(file);

        // 後置ハッシュの検証(ADR-0015)
        let mut line = String::new();
        let frame = read_msg_frame(reader, &mut line)?;
        let MsgFrame::FileHash {
            id: hash_id,
            sha256,
        } = frame
        else {
            bail!("FileHash 以外のフレームが届きました");
        };
        if hash_id != id {
            bail!("転送 ID が一致しません");
        }
        let actual = format!("{:x}", hasher.finalize());
        if !actual.eq_ignore_ascii_case(&sha256) {
            bail!("SHA-256 が一致しません(破損の可能性)");
        }
        std::fs::rename(&part, &dest)
            .with_context(|| format!("{} への確定に失敗しました", dest.display()))?;
        send_json(writer, &MsgFrame::FileDone { id: id.clone() })?;
        Ok(())
    })();

    match &result {
        Ok(()) => {
            shared.transfer_state(&id, "done");
            let saved_name = dest
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or(clean);
            tracing::info!(
                "{sender_name}({peer_ip})からファイルを受信しました({size} バイト、id={id})"
            );
            if let Some(ctx) = chat_ctx {
                shared.chat.lock().unwrap().append(ChatMessageInfo {
                    seq: 0,
                    id: id.clone(),
                    scope: ctx.scope,
                    group_id: ctx.group_id,
                    from: peer_ip,
                    to: match ctx.scope {
                        ChatScope::Direct => Some(shared.cfg.own_ip),
                        _ => None,
                    },
                    text: String::new(),
                    sent_at: now_unix_ms(),
                    failed: false,
                    file: Some(ChatFileInfo {
                        name: saved_name,
                        size,
                        transfers: vec![id.clone()],
                        path: Some(dest.clone()),
                    }),
                    system: false,
                });
            }
        }
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            shared.transfer_state(&id, &format!("failed: {e}"));
        }
    }
    result
}

// ---- コントロールチャネル経由の変更依頼 --------------------------------------

impl SessionShared {
    /// 依頼を送って応答を待つ(SetDisplayName / SetDnsName 共通)。
    /// 拒否・タイムアウト(旧ホスト未対応)はエラー。成功はホストのメッセージ。
    fn control_request(
        &self,
        message: ControlMessage,
        slot: &Mutex<Option<(bool, String)>>,
    ) -> anyhow::Result<String> {
        if !self.control_connected.load(Ordering::Relaxed) {
            bail!("ホストと同期していません(接続直後は数秒待ってから再試行してください)");
        }
        *slot.lock().unwrap() = None;
        self.outbox.lock().unwrap().push(message);
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && !self.stopped() {
            if let Some((accepted, reply)) = slot.lock().unwrap().take() {
                if accepted {
                    return Ok(reply);
                }
                bail!("{reply}");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        bail!("ホストから応答がありません(ホストが旧バージョンの可能性)");
    }

    /// 自分の表示名の変更依頼(ADR-0027)。正本は host.toml でホストが適用する。
    pub fn set_display_name(&self, name: &str) -> anyhow::Result<String> {
        self.control_request(
            ControlMessage::SetDisplayName {
                name: name.to_string(),
            },
            &self.display_result,
        )
    }

    /// 自分の DNS 名の変更依頼(ADR-0021)。正本は host.toml。
    pub fn set_dns_name(&self, name: &str) -> anyhow::Result<String> {
        self.control_request(
            ControlMessage::SetDnsName {
                name: name.to_string(),
            },
            &self.dns_result,
        )
    }
}

// ---- 送信(チャット・ファイル)----------------------------------------------

impl SessionShared {
    /// 短命接続を開いて Hello 済みの reader/writer を返す。
    fn open_peer(&self, target: Ipv4Addr) -> anyhow::Result<(LineReader, TcpStream)> {
        let addr = SocketAddr::from((target, self.cfg.peer_msg_port));
        let stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
            .with_context(|| format!("{target} へ接続できません"))?;
        stream.set_nodelay(true).ok();
        stream.set_read_timeout(Some(IO_TIMEOUT)).ok();
        let mut writer = stream.try_clone().context("ソケット複製に失敗しました")?;
        send_json(
            &mut writer,
            &MsgFrame::Hello {
                version: MSG_VERSION,
            },
        )?;
        Ok((line_reader(stream), writer))
    }

    /// チャットを送る。宛先ごとに個別送信し、1 件も届かなければ失敗の印を付けて
    /// エラーを返す(デスクトップと同じ振る舞い)。
    pub fn send_chat(
        &self,
        scope: ChatScope,
        to: Option<Ipv4Addr>,
        group_id: Option<String>,
        text: String,
    ) -> anyhow::Result<()> {
        if text.is_empty() {
            bail!("本文が空です");
        }
        if text.len() > MAX_CHAT_TEXT_BYTES {
            bail!("本文が上限({MAX_CHAT_TEXT_BYTES} バイト)を超えています");
        }
        if scope == ChatScope::Group && group_id.is_none() {
            bail!("グループ宛なのにグループ ID がありません");
        }
        let targets = self.chat_targets(scope, to, group_id.as_deref())?;
        let entry = self.chat.lock().unwrap().append(ChatMessageInfo {
            seq: 0,
            id: new_transfer_id(),
            scope,
            group_id: group_id.clone(),
            from: self.cfg.own_ip,
            to: match scope {
                ChatScope::Direct => to,
                _ => None,
            },
            text: text.clone(),
            sent_at: now_unix_ms(),
            failed: false,
            file: None,
            system: false,
        });
        let frame = MsgFrame::Chat {
            id: entry.id.clone(),
            scope,
            group_id,
            text,
            sent_at: entry.sent_at,
        };
        let mut delivered = 0usize;
        for target in &targets {
            match self.deliver_chat(*target, &frame, &entry.id) {
                Ok(()) => delivered += 1,
                Err(e) => tracing::warn!("{target} への送信に失敗しました: {e:#}"),
            }
        }
        if delivered == 0 {
            self.chat.lock().unwrap().mark_failed(entry.seq);
            bail!("どの宛先にも届きませんでした({} 宛先)", targets.len());
        }
        Ok(())
    }

    fn chat_targets(
        &self,
        scope: ChatScope,
        to: Option<Ipv4Addr>,
        group_id: Option<&str>,
    ) -> anyhow::Result<Vec<Ipv4Addr>> {
        match scope {
            ChatScope::Direct => Ok(vec![to.context("宛先がありません")?]),
            ChatScope::Network => {
                let ledger = self.ledger.lock().unwrap();
                let snapshot = ledger.as_ref().context("台帳が未受信です(接続直後?)")?;
                let targets: Vec<Ipv4Addr> = snapshot
                    .members
                    .iter()
                    .filter(|m| m.online && !m.blocked && m.ip != self.cfg.own_ip)
                    .map(|m| m.ip)
                    .collect();
                if targets.is_empty() {
                    bail!("オンラインのメンバーがいません");
                }
                Ok(targets)
            }
            ChatScope::Group => {
                let gid = group_id.context("グループ ID がありません")?;
                let members = self
                    .groups
                    .lock()
                    .unwrap()
                    .get(gid)
                    .map(|g| g.members.clone())
                    .context("グループが見つかりません")?;
                let ledger = self.ledger.lock().unwrap();
                let snapshot = ledger.as_ref().context("台帳が未受信です(接続直後?)")?;
                let online: Vec<Ipv4Addr> = members
                    .into_iter()
                    .filter(|ip| *ip != self.cfg.own_ip)
                    .filter(|ip| {
                        snapshot
                            .members
                            .iter()
                            .any(|m| m.ip == *ip && m.online && !m.blocked)
                    })
                    .collect();
                if online.is_empty() {
                    bail!("オンラインのグループメンバーがいません");
                }
                Ok(online)
            }
        }
    }

    fn deliver_chat(&self, target: Ipv4Addr, frame: &MsgFrame, id: &str) -> anyhow::Result<()> {
        let (mut reader, mut writer) = self.open_peer(target)?;
        send_json(&mut writer, frame)?;
        let mut line = String::new();
        match read_msg_frame(&mut reader, &mut line)? {
            MsgFrame::ChatAck { id: acked } if acked == id => Ok(()),
            other => bail!("ChatAck 以外の応答: {other:?}"),
        }
    }

    /// ファイルを 1 人へ送る(チャット文脈付き = 会話にファイルバブルが出る)。
    /// 進捗は transfers に載る。完了時に自分のチャット履歴にも記録する。
    pub fn send_file(&self, target: Ipv4Addr, src: &Path) -> anyhow::Result<String> {
        let meta =
            std::fs::metadata(src).with_context(|| format!("{} を読めません", src.display()))?;
        if !meta.is_file() {
            bail!("ファイルではありません: {}", src.display());
        }
        let size = meta.len();
        let name = src
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .context("ファイル名がありません")?;
        let id = new_transfer_id();
        self.upsert_transfer(TransferInfo {
            id: id.clone(),
            peer: target,
            name: name.clone(),
            size,
            done: 0,
            outgoing: true,
            state: "running".to_string(),
        });

        let result = (|| -> anyhow::Result<()> {
            let (mut reader, mut writer) = self.open_peer(target)?;
            send_json(
                &mut writer,
                &MsgFrame::FileOffer {
                    id: id.clone(),
                    name: name.clone(),
                    size,
                    chat: Some(ChatContext {
                        scope: ChatScope::Direct,
                        group_id: None,
                    }),
                },
            )?;
            let mut line = String::new();
            match read_msg_frame(&mut reader, &mut line)? {
                MsgFrame::FileAccept { id: accepted } if accepted == id => {}
                MsgFrame::FileReject { reason, .. } => bail!("受信側が拒否しました: {reason}"),
                other => bail!("FileAccept 以外の応答: {other:?}"),
            }

            let mut file = std::fs::File::open(src)
                .with_context(|| format!("{} を開けません", src.display()))?;
            let mut hasher = Sha256::new();
            let mut sent: u64 = 0;
            let mut buf = [0u8; 64 * 1024];
            loop {
                let n = file.read(&mut buf).context("読み取りに失敗しました")?;
                if n == 0 {
                    break;
                }
                writer.write_all(&buf[..n]).context("送信に失敗しました")?;
                hasher.update(&buf[..n]);
                sent += n as u64;
                self.transfer_progress(&id, sent);
            }
            if sent != size {
                bail!("送信中にファイルサイズが変わりました({sent} / {size})");
            }
            send_json(
                &mut writer,
                &MsgFrame::FileHash {
                    id: id.clone(),
                    sha256: format!("{:x}", hasher.finalize()),
                },
            )?;
            match read_msg_frame(&mut reader, &mut line)? {
                MsgFrame::FileDone { id: done } if done == id => Ok(()),
                other => bail!("FileDone 以外の応答: {other:?}"),
            }
        })();

        match &result {
            Ok(()) => {
                self.transfer_state(&id, "done");
                self.chat.lock().unwrap().append(ChatMessageInfo {
                    seq: 0,
                    id: id.clone(),
                    scope: ChatScope::Direct,
                    group_id: None,
                    from: self.cfg.own_ip,
                    to: Some(target),
                    text: String::new(),
                    sent_at: now_unix_ms(),
                    failed: false,
                    file: Some(ChatFileInfo {
                        name,
                        size,
                        transfers: vec![id.clone()],
                        path: Some(src.to_path_buf()),
                    }),
                    system: false,
                });
                tracing::info!("{target} へファイルを送信しました({size} バイト、id={id})");
            }
            Err(e) => {
                self.transfer_state(&id, &format!("failed: {e}"));
            }
        }
        result.map(|_| id)
    }
}

// ---- 小物 -------------------------------------------------------------------

pub fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 転送・メッセージ ID。認証には使わない(身元は接続元 IP)ので一意なら十分。
pub fn new_transfer_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    format!(
        "m{}-{}",
        now_unix_ms(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// 受信ファイル名のサニタイズ(desktop msg.rs の移植)。パス区切り・制御文字・
/// Windows 予約文字を除去し、危険な名前は None。
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
    let cleaned = cleaned.trim().trim_matches('.').trim();
    if cleaned.is_empty() {
        return None;
    }
    // Windows の予約デバイス名(CON など)を避ける(受信ボックスを PC へ
    // コピーしても安全なように、モバイルでも同じ規則にする)
    let stem = cleaned.split('.').next().unwrap_or(cleaned);
    const RESERVED: &[&str] = &[
        "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
        "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];
    if RESERVED.contains(&stem.to_ascii_lowercase().as_str()) {
        return Some(format!("_{cleaned}"));
    }
    Some(cleaned.to_string())
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

/// 重複しない保存先を選ぶ(`名前 (1).ext` 方式、desktop msg.rs の移植)。
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let taken = |p: &Path| p.exists() || append_suffix(p, ".part").exists();
    let candidate = dir.join(name);
    if !taken(&candidate) {
        return candidate;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(pos) if pos > 0 => (&name[..pos], &name[pos..]),
        _ => (name, ""),
    };
    (1..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !taken(p))
        .expect("空き番号は必ずある")
}

#[cfg(test)]
mod tests {
    use super::*;
    use peercove_core::keys::PrivateKey;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "peercove-mobile-session-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn ledger_entry(ip: &str, name: &str, online: bool) -> LedgerEntry {
        LedgerEntry {
            name: Some(name.to_string()),
            dns_name: None,
            ip: ip.parse().unwrap(),
            public_key: PrivateKey::generate().public_key(),
            app_version: None,
            capabilities: vec![],
            invite_status: None,
            invite_expires_at: None,
            online,
            is_host: false,
            endpoint: None,
            endpoint_age_secs: None,
            subnets: vec![],
            blocked: false,
            force_relay: false,
            acl_rule_id: None,
        }
    }

    /// テスト用セッション。listen は 127.0.0.1 のエフェメラルポート。
    /// member.toml は本物(ops::join で生成)なので、設定読み出し(受信上限
    /// など)も実物の経路で動く。
    fn test_session(label: &str, control_addr: SocketAddr, peer_msg_port: u16) -> NetSession {
        let dir = temp_dir(label);
        let token = peercove_core::token::InviteToken {
            member_private_key: PrivateKey::generate(),
            host_public_key: PrivateKey::generate().public_key(),
            preshared_key: None,
            member_address: "10.77.0.5/24".parse().unwrap(),
            host_virtual_ip: "10.77.0.1".parse().unwrap(),
            endpoints: vec!["192.0.2.1:51820".parse().unwrap()],
            name: format!("test-{label}"),
            network: Some("testnet".to_string()),
            invite_id: None,
            issued_at: None,
            expires_at: None,
        }
        .encode()
        .unwrap();
        peercove_ops::join::join(&token, &dir, true).unwrap();
        NetSession::start(SessionConfig {
            slug: label.to_string(),
            config_path: dir.join("member.toml"),
            own_ip: "127.0.0.1".parse().unwrap(),
            display_name: Some(format!("test-{label}")),
            device_id: None,
            network_name: "testnet".to_string(),
            control_addr,
            listen_addr: "127.0.0.1:0".parse().unwrap(),
            peer_msg_port,
        })
    }

    fn wait_until(timeout: Duration, mut f: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    fn seed_ledger(session: &NetSession, members: Vec<LedgerEntry>) {
        *session.shared.ledger.lock().unwrap() = Some(LedgerSnapshot {
            members,
            dns_records: vec![],
            cname_records: vec![],
        });
    }

    fn bound_addr(session: &NetSession) -> SocketAddr {
        assert!(
            wait_until(Duration::from_secs(5), || session
                .shared
                .bound_listen
                .lock()
                .unwrap()
                .is_some()),
            "listener が bind しない"
        );
        session.shared.bound_listen.lock().unwrap().unwrap()
    }

    /// コントロールチャネル: hello 送信 → 台帳受信 → ping に pong → removed で停止。
    #[test]
    fn control_client_talks_to_fake_host() {
        let host = TcpListener::bind("127.0.0.1:0").unwrap();
        let control_addr = host.local_addr().unwrap();
        let session = test_session("control", control_addr, 1);

        let (stream, _) = host.accept().unwrap();
        stream.set_nodelay(true).ok();
        let mut writer = stream.try_clone().unwrap();
        let mut reader = std::io::BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert!(line.contains(r#""type":"hello""#), "{line}");
        assert!(
            line.contains(r#""capabilities":["chat","file_transfer"]"#),
            "{line}"
        );
        assert!(line.contains("test-control"), "表示名を名乗る: {line}");

        // 台帳を配る → セッションに反映される
        let ledger = ControlMessage::Ledger {
            members: vec![
                ledger_entry("10.9.0.1", "host", true),
                ledger_entry("10.9.0.5", "sumaho", true),
            ],
            dns_records: vec![],
            cname_records: vec![],
        };
        writer
            .write_all((serde_json::to_string(&ledger).unwrap() + "\n").as_bytes())
            .unwrap();
        assert!(wait_until(Duration::from_secs(5), || {
            session
                .shared
                .ledger
                .lock()
                .unwrap()
                .as_ref()
                .is_some_and(|l| l.members.len() == 2)
        }));
        assert!(session.shared.control_connected.load(Ordering::Relaxed));

        // ping に pong が返る
        writer
            .write_all(
                (serde_json::to_string(&ControlMessage::Ping { nonce: 9 }).unwrap() + "\n")
                    .as_bytes(),
            )
            .unwrap();
        let mut got_pong = false;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            line.clear();
            if reader.read_line(&mut line).unwrap() == 0 {
                break;
            }
            if line.contains(r#""type":"pong""#) && line.contains("9") {
                got_pong = true;
                break;
            }
            // セッション発の ping は無視してよい
        }
        assert!(got_pong, "pong が返らない");

        // 表示名の変更依頼: 依頼行が届き、応答が呼び出し元へ返る
        let session_for_req = Arc::clone(&session.shared);
        let request = std::thread::spawn(move || session_for_req.set_display_name("新しい名前"));
        let mut got_request = false;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            line.clear();
            if reader.read_line(&mut line).unwrap() == 0 {
                break;
            }
            if line.contains(r#""type":"set_display_name""#) {
                assert!(line.contains("新しい名前"), "{line}");
                got_request = true;
                break;
            }
        }
        assert!(got_request, "変更依頼が届かない");
        let result = ControlMessage::SetDisplayNameResult {
            accepted: true,
            message: "更新しました".to_string(),
        };
        writer
            .write_all((serde_json::to_string(&result).unwrap() + "\n").as_bytes())
            .unwrap();
        assert_eq!(request.join().unwrap().unwrap(), "更新しました");

        // removed で再接続をやめる
        let removed = ControlMessage::Removed {
            message: "テスト削除".to_string(),
        };
        writer
            .write_all((serde_json::to_string(&removed).unwrap() + "\n").as_bytes())
            .unwrap();
        assert!(wait_until(Duration::from_secs(5), || session
            .shared
            .removed
            .load(Ordering::Relaxed)));
        session.stop();
    }

    /// チャット受信: 台帳のメンバーからの Chat を履歴に記録して ack を返す。
    /// 台帳に無い送信元は拒否する。
    #[test]
    fn listener_receives_chat_and_rejects_unknown_sender() {
        // control は誰も居ないポートへ(接続失敗で待つだけ)
        let session = test_session("recv-chat", "127.0.0.1:1".parse().unwrap(), 1);
        let addr = bound_addr(&session);

        // 台帳に無い送信元(まだ台帳が空)→ 応答なしで切断される
        {
            let stream = TcpStream::connect(addr).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(3))).ok();
            let mut writer = stream.try_clone().unwrap();
            writer
                .write_all(b"{\"type\":\"hello\",\"version\":1}\n")
                .unwrap();
            writer
                .write_all(
                    b"{\"type\":\"chat\",\"id\":\"x\",\"scope\":\"direct\",\"text\":\"hi\",\"sent_at\":1}\n",
                )
                .unwrap();
            let mut reader = std::io::BufReader::new(stream);
            let mut line = String::new();
            let n = reader.read_line(&mut line).unwrap_or(0);
            assert_eq!(n, 0, "台帳に無い送信元に応答してはいけない: {line}");
        }

        // 台帳へ 127.0.0.1 を載せると受信できる
        seed_ledger(&session, vec![ledger_entry("127.0.0.1", "sender", true)]);
        let stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let mut writer = stream.try_clone().unwrap();
        writer
            .write_all(b"{\"type\":\"hello\",\"version\":1}\n")
            .unwrap();
        writer
            .write_all(
                b"{\"type\":\"chat\",\"id\":\"c9\",\"scope\":\"direct\",\"text\":\"\xe3\x81\x93\xe3\x82\x93\xe3\x81\xab\xe3\x81\xa1\xe3\x81\xaf\",\"sent_at\":5}\n",
            )
            .unwrap();
        let mut reader = std::io::BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert!(line.contains(r#""type":"chat_ack""#), "{line}");
        let messages = session.shared.chat.lock().unwrap().fetch(0, 10);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text, "こんにちは");
        session.stop();
    }

    /// 送受一体: セッション B から A へ chat + ファイルを送る(実プロトコル通し)。
    #[test]
    fn chat_and_file_roundtrip_between_sessions() {
        let a = test_session("pair-a", "127.0.0.1:1".parse().unwrap(), 1);
        let a_addr = bound_addr(&a);
        // B の peer_msg_port は A の実ポート
        let b = test_session("pair-b", "127.0.0.1:1".parse().unwrap(), a_addr.port());

        let both = vec![ledger_entry("127.0.0.1", "pair", true)];
        seed_ledger(&a, both.clone());
        seed_ledger(&b, both);

        // チャット送信(B → A)
        b.shared
            .send_chat(
                ChatScope::Direct,
                Some("127.0.0.1".parse().unwrap()),
                None,
                "やあ".to_string(),
            )
            .unwrap();
        let received = a.shared.chat.lock().unwrap().fetch(0, 10);
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].text, "やあ");
        // B 側にも自分の送信が履歴として残る
        let sent = b.shared.chat.lock().unwrap().fetch(0, 10);
        assert_eq!(sent.len(), 1);
        assert!(!sent[0].failed);

        // ファイル送信(B → A)。A の受信ボックスに同じ内容で保存される
        let src = temp_dir("pair-src").join("挨拶.txt");
        std::fs::write(&src, "file over peercove").unwrap();
        b.shared
            .send_file("127.0.0.1".parse().unwrap(), &src)
            .unwrap();
        let inbox = a.shared.inbox_dir();
        let saved = std::fs::read_dir(&inbox)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(
            std::fs::read_to_string(&saved).unwrap(),
            "file over peercove"
        );
        // 双方のチャット履歴にファイルエントリが載る(LINE 風のファイルバブル用)
        let a_chat = a.shared.chat.lock().unwrap().fetch(0, 10);
        assert!(a_chat.iter().any(|m| m.file.is_some()));
        let b_chat = b.shared.chat.lock().unwrap().fetch(0, 10);
        assert!(b_chat.iter().any(|m| m.file.is_some()));

        a.stop();
        b.stop();
    }

    /// 受信サイズ上限(mobile.toml、既定 10 MB)を超える申し出は拒否される。
    #[test]
    fn file_over_limit_is_rejected() {
        let a = test_session("limit-a", "127.0.0.1:1".parse().unwrap(), 1);
        let a_addr = bound_addr(&a);
        let b = test_session("limit-b", "127.0.0.1:1".parse().unwrap(), a_addr.port());
        let both = vec![ledger_entry("127.0.0.1", "pair", true)];
        seed_ledger(&a, both.clone());
        seed_ledger(&b, both);

        // A の上限を 1 MB に設定(デスクトップと同じ member.toml の設定を使う)
        let config_path = a.shared.cfg.config_path.clone();
        let current = peercove_ops::settings::read(&config_path).unwrap();
        peercove_ops::settings::update(
            &config_path,
            &peercove_ops::settings::Update {
                display_name: current.display_name.clone(),
                dns_name: current.dns_name.clone(),
                listen_port: current.listen_port,
                mtu: current.mtu,
                host_endpoint: current.host_endpoint.clone(),
                direct: current.direct,
                max_recv_file_mb: 1,
                require_invite_approval: current.require_invite_approval,
            },
        )
        .unwrap();

        // 2 MB のファイルは拒否される(理由に上限が入る)
        let src = temp_dir("limit-src").join("big.bin");
        std::fs::write(&src, vec![0u8; 2 * 1024 * 1024]).unwrap();
        let err = b
            .shared
            .send_file("127.0.0.1".parse().unwrap(), &src)
            .unwrap_err();
        assert!(err.to_string().contains("1 MB"), "{err:#}");

        a.stop();
        b.stop();
    }

    #[test]
    fn sanitize_and_unique_path_are_safe() {
        assert_eq!(
            sanitize_file_name("../../etc/passwd").as_deref(),
            Some("passwd")
        );
        assert_eq!(
            sanitize_file_name(r"C:\Users\a\写真.jpg").as_deref(),
            Some("写真.jpg")
        );
        assert_eq!(sanitize_file_name("..").as_deref(), None);
        assert_eq!(sanitize_file_name("con.txt").as_deref(), Some("_con.txt"));

        let dir = temp_dir("unique");
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        assert_eq!(unique_path(&dir, "a.txt"), dir.join("a (1).txt"));
        assert_eq!(unique_path(&dir, "b.txt"), dir.join("b.txt"));
    }
}
