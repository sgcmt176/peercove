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

use std::collections::HashSet;
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
    ChatContext, ChatScope, GroupInfo, MsgFrame, MAX_CHAT_TEXT_BYTES, MAX_GROUP_MEMBERS,
    MAX_GROUP_NAME_BYTES, MSG_VERSION,
};
use peercove_core::proto::{ControlMessage, LedgerEntry, PROTO_VERSION};
use serde::{Deserialize, Serialize};
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
/// チャットの自動再送間隔(E-E 3)。
const CHAT_RESEND_INTERVAL: Duration = Duration::from_secs(10);

/// 送信待ち(再送キュー)のチャット 1 通(E-E 3)。送達条件を満たすまで
/// [`CHAT_RESEND_INTERVAL`] ごとに再送する。同じ ID を使い続けるので、
/// 受信側の重複弾き(contains_id)と対で冪等。
pub struct PendingChat {
    pub seq: u64,
    pub id: String,
    pub scope: ChatScope,
    pub to: Option<Ipv4Addr>,
    pub group_id: Option<String>,
    pub text: String,
    pub sent_at: u64,
    /// ack が取れた宛先(network/group の部分送達を重複させない)
    delivered: HashSet<Ipv4Addr>,
    next_at: Instant,
}

/// 送信待ちキューの保存形式(<config>.chatq.json)。アプリ再起動を跨いで
/// 自動再送を続けるための永続化。next_at は保存せず、読み直したら即時再送。
#[derive(Serialize, Deserialize)]
struct PersistedChat {
    seq: u64,
    id: String,
    scope: ChatScope,
    #[serde(default)]
    to: Option<Ipv4Addr>,
    #[serde(default)]
    group_id: Option<String>,
    text: String,
    sent_at: u64,
    #[serde(default)]
    delivered: Vec<Ipv4Addr>,
}

fn chat_queue_path(config_path: &Path) -> PathBuf {
    config_path.with_extension("chatq.json")
}

/// 保存済みの送信待ちキューを読む(壊れていたら空 = 諦めて捨てる)。
fn load_chat_queue(config_path: &Path) -> Vec<PendingChat> {
    let Ok(data) = std::fs::read_to_string(chat_queue_path(config_path)) else {
        return Vec::new();
    };
    let Ok(list) = serde_json::from_str::<Vec<PersistedChat>>(&data) else {
        return Vec::new();
    };
    list.into_iter()
        .map(|p| PendingChat {
            seq: p.seq,
            id: p.id,
            scope: p.scope,
            to: p.to,
            group_id: p.group_id,
            text: p.text,
            sent_at: p.sent_at,
            delivered: p.delivered.into_iter().collect(),
            next_at: Instant::now(),
        })
        .collect()
}

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
    /// チャットの送信待ちキュー(E-E 3)
    pub chat_queue: Mutex<Vec<PendingChat>>,
    pub groups: Mutex<GroupStore>,
    pub transfers: Mutex<Vec<TransferInfo>>,
    /// listener が実際に bind したアドレス(テストが接続先を知るため)
    pub bound_listen: Mutex<Option<SocketAddr>>,
    /// コントロールチャネルへ差し込む送信キュー(表示名・DNS 名の変更依頼)
    outbox: Mutex<Vec<ControlMessage>>,
    /// SetDnsName / SetDisplayName / RotateKey の応答受け口((accepted, message))
    dns_result: Mutex<Option<(bool, String)>>,
    display_result: Mutex<Option<(bool, String)>>,
    rotate_result: Mutex<Option<(bool, String)>>,
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
            chat_queue: Mutex::new(load_chat_queue(&cfg.config_path)),
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
            rotate_result: Mutex::new(None),
            stop: AtomicBool::new(false),
            cfg,
        });
        let threads = vec![
            spawn_named("peercove-control", Arc::clone(&shared), control_loop),
            spawn_named("peercove-msg", Arc::clone(&shared), listener_loop),
            spawn_named("peercove-chatq", Arc::clone(&shared), chat_queue_loop),
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

    /// 受信ファイルサイズ上限(バイト、0 = 無制限)。申し出ごとに読む
    /// (設定変更を再起動なしで反映)。
    fn recv_limit_bytes(&self) -> u64 {
        recv_limit_mb_for(&self.cfg.config_path).saturating_mul(1024 * 1024)
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

/// member.toml の受信上限(MB)。**フィールドが書かれていない**設定(この機能より
/// 前の join で作ったもの)はモバイル既定の 10 にする — Config のデフォルト
/// (デスクトップの 100)へ落とさない(2026-07-19 依頼者指定)。
pub fn recv_limit_mb_for(config_path: &Path) -> u64 {
    let Ok(text) = std::fs::read_to_string(config_path) else {
        return MOBILE_DEFAULT_MAX_RECV_FILE_MB;
    };
    if !text.contains("max_recv_file_mb") {
        return MOBILE_DEFAULT_MAX_RECV_FILE_MB;
    }
    peercove_core::config::Config::load(config_path)
        .map(|c| c.interface.max_recv_file_mb)
        .unwrap_or(MOBILE_DEFAULT_MAX_RECV_FILE_MB)
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
            platform: Some("android".to_string()),
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
                        // メンバー再追加(別の鍵)を検知して 1:1 履歴をクリア(検証 FB)
                        shared.reconcile_identities(&members);
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
                    Ok(ControlMessage::RotateKeyResult { accepted, message }) => {
                        *shared.rotate_result.lock().unwrap() = Some((accepted, message));
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

/// チャット送信キューの巡回(E-E 3)。2 秒ごとに期限の来た送信待ちを再送する。
fn chat_queue_loop(shared: &Arc<SessionShared>) {
    while !shared.stopped() {
        shared.sleep_with_stop(Duration::from_secs(2));
        if shared.stopped() {
            return;
        }
        shared.pump_chat_queue();
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
            // モバイルは中断再開(E-E 6)未対応: 常に先頭(offset 0)から受ける
            resume: _,
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
            // 再送の重複(ack の取り損ね後に同じ ID で再送 — E-E 3)は
            // 取り込まず ack だけ返す
            if shared.chat.lock().unwrap().contains_id(&id) {
                send_json(&mut writer, &MsgFrame::ChatAck { id: id.clone() })?;
                tracing::debug!("重複したチャットを ack のみで処理しました(id={id})");
                return Ok(());
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
        send_json(
            writer,
            &MsgFrame::FileAccept {
                id: id.clone(),
                offset: 0,
            },
        )?;

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
        let (accepted, reply) = self.control_request_raw(message, slot)?;
        if accepted {
            Ok(reply)
        } else {
            bail!("{reply}")
        }
    }

    /// 応答そのもの(accepted, message)が要る版。Err はタイムアウト・未接続のみ
    /// (拒否と区別したい鍵ローテーションが使う)。
    fn control_request_raw(
        &self,
        message: ControlMessage,
        slot: &Mutex<Option<(bool, String)>>,
    ) -> anyhow::Result<(bool, String)> {
        if !self.control_connected.load(Ordering::Relaxed) {
            bail!("ホストと同期していません(接続直後は数秒待ってから再試行してください)");
        }
        *slot.lock().unwrap() = None;
        self.outbox.lock().unwrap().push(message);
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && !self.stopped() {
            if let Some(result) = slot.lock().unwrap().take() {
                return Ok(result);
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

    /// デバイス鍵の更新依頼(ADR-0020 のモバイル版)。
    /// 戻り値は (accepted, message)。Err はタイムアウト・未接続のみ
    /// (呼び出し側が「拒否 = 新鍵破棄」「応答なし = 新鍵温存」を分ける)。
    pub fn rotate_key_request(
        &self,
        new_public_key: peercove_core::keys::PublicKey,
    ) -> anyhow::Result<(bool, String)> {
        self.control_request_raw(
            ControlMessage::RotateKey { new_public_key },
            &self.rotate_result,
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

    /// チャットを送る(E-E 3: 再送キュー方式)。履歴へ追記してキューに積み、
    /// その場で 1 回配送を試みる。届かなくてもエラーにせず、失敗の印を付けて
    /// 10 秒間隔で自動再送する(同じ ID を使うので受信側の重複弾きと対で冪等)。
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
        if scope == ChatScope::Direct && to.is_none() {
            bail!("宛先がありません");
        }
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
        self.chat_queue.lock().unwrap().push(PendingChat {
            seq: entry.seq,
            id: entry.id,
            scope,
            to,
            group_id,
            text,
            sent_at: entry.sent_at,
            delivered: HashSet::new(),
            next_at: Instant::now(),
        });
        self.pump_chat_queue();
        self.save_chat_queue();
        Ok(())
    }

    /// 送信待ちキューをディスクへ反映する(空になったらファイルを消す)。
    /// 件数は高々数十なので毎回全量書き直しで足りる。
    fn save_chat_queue(&self) {
        let path = chat_queue_path(&self.cfg.config_path);
        let snapshot: Vec<PersistedChat> = self
            .chat_queue
            .lock()
            .unwrap()
            .iter()
            .map(|p| PersistedChat {
                seq: p.seq,
                id: p.id.clone(),
                scope: p.scope,
                to: p.to,
                group_id: p.group_id.clone(),
                text: p.text.clone(),
                sent_at: p.sent_at,
                delivered: p.delivered.iter().copied().collect(),
            })
            .collect();
        if snapshot.is_empty() {
            let _ = std::fs::remove_file(&path);
            return;
        }
        let Ok(json) = serde_json::to_string(&snapshot) else {
            return;
        };
        let tmp = path.with_extension("chatq.json.tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }

    /// 送信待ちキューを 1 巡処理する(期限が来たものだけ)。送信キュー
    /// スレッドと send_chat / resend_chat から呼ばれる。取り出してから
    /// 配送する(remove → 処理 → 未達なら戻す)ので並行 pump でも二重配送しない。
    pub fn pump_chat_queue(&self) {
        let due: Vec<u64> = {
            let now = Instant::now();
            self.chat_queue
                .lock()
                .unwrap()
                .iter()
                .filter(|p| p.next_at <= now)
                .map(|p| p.seq)
                .collect()
        };
        for seq in due {
            let pending = {
                let mut queue = self.chat_queue.lock().unwrap();
                queue
                    .iter()
                    .position(|p| p.seq == seq)
                    .map(|index| queue.remove(index))
            };
            let Some(mut pending) = pending else { continue };
            if self.attempt_chat(&mut pending) {
                self.chat.lock().unwrap().clear_failed(pending.seq);
                tracing::info!(
                    "チャットを送信しました(seq={} id={})",
                    pending.seq,
                    pending.id
                );
            } else {
                self.chat.lock().unwrap().mark_failed(pending.seq);
                pending.next_at = Instant::now() + CHAT_RESEND_INTERVAL;
                self.chat_queue.lock().unwrap().push(pending);
            }
            // 送達済みの除去・部分送達(delivered)の変化を保存へ反映
            self.save_chat_queue();
        }
    }

    /// 未達の宛先へ配送を試みる。戻り値 true = 送達条件を満たした
    /// (direct = 宛先本人、network/group = 1 人以上)。
    fn attempt_chat(&self, pending: &mut PendingChat) -> bool {
        let frame = MsgFrame::Chat {
            id: pending.id.clone(),
            scope: pending.scope,
            group_id: pending.group_id.clone(),
            text: pending.text.clone(),
            sent_at: pending.sent_at,
        };
        // 宛先は毎回引き直す(台帳の到着やオンライン状態の変化に追従)
        let targets = self
            .chat_targets(pending.scope, pending.to, pending.group_id.as_deref())
            .unwrap_or_default();
        for target in targets {
            if pending.delivered.contains(&target) {
                continue;
            }
            match self.deliver_chat(target, &frame, &pending.id) {
                Ok(()) => {
                    pending.delivered.insert(target);
                }
                Err(e) => tracing::debug!("{target} への送信に失敗(自動再送します): {e:#}"),
            }
        }
        match pending.scope {
            ChatScope::Direct => pending
                .to
                .is_some_and(|target| pending.delivered.contains(&target)),
            _ => !pending.delivered.is_empty(),
        }
    }

    /// 送信待ち(= まだ送達条件を満たしていない)メッセージの seq 一覧。
    pub fn sending_seqs(&self) -> Vec<u64> {
        self.chat_queue
            .lock()
            .unwrap()
            .iter()
            .map(|p| p.seq)
            .collect()
    }

    /// 手動再送(失敗した吹き出しの「再送」)。キューに居ればすぐ再試行、
    /// 居なければ(取消後・アプリ再起動後)履歴から積み直す。
    pub fn resend_chat(&self, seq: u64) -> anyhow::Result<()> {
        let requeued = {
            let mut queue = self.chat_queue.lock().unwrap();
            match queue.iter_mut().find(|p| p.seq == seq) {
                Some(pending) => {
                    pending.next_at = Instant::now();
                    true
                }
                None => false,
            }
        };
        if !requeued {
            let entry = self
                .chat
                .lock()
                .unwrap()
                .get(seq)
                .context("メッセージが見つかりません")?;
            if entry.from != self.cfg.own_ip || entry.system || entry.file.is_some() {
                bail!("このメッセージは再送できません");
            }
            self.chat_queue.lock().unwrap().push(PendingChat {
                seq: entry.seq,
                id: entry.id,
                scope: entry.scope,
                to: entry.to,
                group_id: entry.group_id,
                text: entry.text,
                sent_at: entry.sent_at,
                delivered: HashSet::new(),
                next_at: Instant::now(),
            });
        }
        self.pump_chat_queue();
        self.save_chat_queue();
        Ok(())
    }

    /// 送信の取消(自動再送をやめる)。履歴には失敗の印を付けたまま残す。
    pub fn cancel_chat_send(&self, seq: u64) {
        self.chat_queue.lock().unwrap().retain(|p| p.seq != seq);
        self.chat.lock().unwrap().mark_failed(seq);
        self.save_chat_queue();
    }

    /// メンバーの再追加(削除 → 同名・同 IP で再参加)と削除(台帳から消えた)
    /// を検知して後片付けをする(検証 FB 2026-07-20):
    /// - 1:1 履歴の消去(お知らせ行を残す = seq を巻き戻さない)
    /// - その相手宛の送信待ちの破棄
    /// - 自分がメンバーのグループから該当 IP を外して配布(自動脱退)
    ///
    /// 同一性はホストが振る **member_id(invite_id)** で判定する。公開鍵は
    /// 鍵ローテーション(ADR-0020/0044 — Android は参加直後に自動実行)でも
    /// 変わるため使えない。`<config>.chatids.json` に IP → "id:<member_id>" を
    /// 保存して台帳受信のたびに突き合わせる。初回・旧形式(公開鍵の記録)は
    /// 記録し直すだけ(既存履歴は消さない)。
    fn reconcile_identities(&self, members: &[LedgerEntry]) {
        let path = self.cfg.config_path.with_extension("chatids.json");
        let mut map: std::collections::HashMap<String, String> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        let mut replaced: Vec<Ipv4Addr> = Vec::new();
        let mut changed = false;
        for entry in members {
            if entry.ip == self.cfg.own_ip || entry.is_host {
                continue;
            }
            let Some(member_id) = entry.member_id.as_ref() else {
                continue;
            };
            let ip = entry.ip.to_string();
            let id = format!("id:{member_id}");
            match map.get(&ip) {
                Some(prev) if *prev == id => {} // 同一メンバー
                // 旧形式(公開鍵)からの移行: 記録し直すだけで消さない
                Some(prev) if !prev.starts_with("id:") => {
                    map.insert(ip, id);
                    changed = true;
                }
                Some(_) => {
                    if self.chat.lock().unwrap().clear_direct(entry.ip) {
                        tracing::info!(
                            "{} は別のメンバーに置き換わったため 1:1 履歴を消去しました",
                            entry.ip
                        );
                    }
                    // お知らせ行(履歴の末尾を残して seq の巻き戻りも防ぐ)
                    self.append_system_line(
                        ChatScope::Direct,
                        Some(entry.ip),
                        None,
                        format!(
                            "{} は新しい端末として参加しました(以前の履歴と送信待ちを破棄しました)",
                            entry.name.as_deref().unwrap_or(&ip)
                        ),
                    );
                    replaced.push(entry.ip);
                    map.insert(ip, id);
                    changed = true;
                }
                None => {
                    map.insert(ip, id);
                    changed = true;
                }
            }
        }
        // サイドカーに居るのに台帳から消えた IP = ネットワークから削除された
        let departed: Vec<Ipv4Addr> = map
            .keys()
            .filter_map(|ip| ip.parse::<Ipv4Addr>().ok())
            .filter(|ip| !members.iter().any(|e| e.ip == *ip))
            .collect();
        for ip in &departed {
            map.remove(&ip.to_string());
            changed = true;
            self.append_system_line(
                ChatScope::Network,
                None,
                None,
                format!("{ip} がネットワークから削除されました"),
            );
        }
        // 置き換わった・居なくなった相手宛の送信待ち(1:1)は破棄する
        let stale: Vec<Ipv4Addr> = replaced.iter().chain(departed.iter()).copied().collect();
        if !stale.is_empty() {
            let pruned = {
                let mut queue = self.chat_queue.lock().unwrap();
                let before = queue.len();
                queue.retain(|p| {
                    !(p.scope == ChatScope::Direct && p.to.is_some_and(|to| stale.contains(&to)))
                });
                queue.len() != before
            };
            if pruned {
                self.save_chat_queue();
            }
        }
        self.prune_groups(&stale, members);
        if changed {
            if let Ok(json) = serde_json::to_string(&map) {
                let tmp = path.with_extension("chatids.json.tmp");
                if std::fs::write(&tmp, json).is_ok() {
                    let _ = std::fs::rename(&tmp, &path);
                }
            }
        }
    }

    /// お知らせ行(system)を履歴へ 1 行足す。
    fn append_system_line(
        &self,
        scope: ChatScope,
        to: Option<Ipv4Addr>,
        group_id: Option<String>,
        text: String,
    ) {
        self.chat.lock().unwrap().append(ChatMessageInfo {
            seq: 0,
            id: new_transfer_id(),
            scope,
            group_id,
            from: self.cfg.own_ip,
            to,
            text,
            sent_at: now_unix_ms(),
            failed: false,
            file: None,
            system: true,
        });
    }

    /// ネットワークに居ない IP(置き換わった・台帳から消えた・過去の削除の
    /// 取りこぼし)を、自分がメンバーのグループから外して残りへ配る
    /// (デスクトップの reconcile と同じ規則。配布は best-effort — 各メンバーが
    /// 独立に同じ更新を作るため、届かなくても収束する)。
    fn prune_groups(&self, replaced: &[Ipv4Addr], members: &[LedgerEntry]) {
        // 台帳が不完全(自分すら居ない)なら見送る
        if !members.iter().any(|e| e.ip == self.cfg.own_ip) {
            return;
        }
        let mut stale: Vec<Ipv4Addr> = replaced.to_vec();
        for group in self.groups.lock().unwrap().list() {
            if !group.members.contains(&self.cfg.own_ip) {
                continue;
            }
            for ip in group.members {
                if ip != self.cfg.own_ip
                    && !members.iter().any(|e| e.ip == ip)
                    && !stale.contains(&ip)
                {
                    stale.push(ip);
                }
            }
        }
        let updates = self
            .groups
            .lock()
            .unwrap()
            .prune_departed(&stale, self.cfg.own_ip);
        for group in updates {
            let name_of = |ip: Ipv4Addr| -> String {
                if ip == self.cfg.own_ip {
                    return "自分".to_string();
                }
                members
                    .iter()
                    .find(|e| e.ip == ip)
                    .and_then(|e| e.name.clone())
                    .unwrap_or_else(|| ip.to_string())
            };
            let applied = self.groups.lock().unwrap().apply(group.clone());
            if let Some(update) = applied {
                for text in groups::system_messages(
                    update.previous.as_ref(),
                    &group,
                    self.cfg.own_ip,
                    &name_of,
                ) {
                    self.append_system_line(ChatScope::Group, None, Some(group.id.clone()), text);
                }
            }
            tracing::info!(
                "ネットワークに居ないメンバーをグループから外しました(id={} rev={})",
                group.id,
                group.revision
            );
            for target in group.members.iter().filter(|ip| {
                **ip != self.cfg.own_ip
                    && members
                        .iter()
                        .any(|e| e.ip == **ip && e.online && !e.blocked)
            }) {
                if let Err(e) = self.deliver_group(*target, &group) {
                    tracing::debug!("{target} へのグループ配布に失敗しました: {e:#}");
                }
            }
        }
    }

    /// チャット履歴を全消去する(E-E 10 のストレージ管理)。送信待ちも破棄。
    /// seq を保つため、削除のお知らせ行を 1 行だけ残す(再起動でも巻き戻らない)。
    pub fn clear_chat_history(&self) {
        self.chat_queue.lock().unwrap().clear();
        self.save_chat_queue();
        let mut chat = self.chat.lock().unwrap();
        chat.clear();
        chat.append(ChatMessageInfo {
            seq: 0,
            id: new_transfer_id(),
            scope: ChatScope::Network,
            group_id: None,
            from: self.cfg.own_ip,
            to: None,
            text: "チャット履歴を削除しました".to_string(),
            sent_at: now_unix_ms(),
            failed: false,
            file: None,
            system: true,
        });
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

    /// グループを作る(ADR-0016 のモバイル版)。`members` に自分は含めなくてよい。
    ///
    /// デスクトップと違いモバイルは送達再送(pending_sync)を持たないため、
    /// **オンラインのメンバー 1 人以上に届いたときだけ作成が成立**する
    /// (届いた先のデスクトップの ack ベース再送で残りのメンバーへ収束する)。
    /// 誰にも届かなければローカルにも作らずエラーを返す(幽霊グループ防止)。
    pub fn create_group(&self, name: &str, members: Vec<Ipv4Addr>) -> anyhow::Result<GroupInfo> {
        let name = name.trim();
        if name.is_empty() {
            bail!("グループ名を入力してください");
        }
        if name.len() > MAX_GROUP_NAME_BYTES {
            bail!("グループ名が長すぎます(上限 {MAX_GROUP_NAME_BYTES} バイト)");
        }
        let (member_states, mut group_members) = {
            let ledger = self.ledger.lock().unwrap();
            let snapshot = ledger.as_ref().context("台帳が未受信です(接続直後?)")?;
            let mut group_members = vec![self.cfg.own_ip];
            for ip in members {
                if ip == self.cfg.own_ip || group_members.contains(&ip) {
                    continue;
                }
                if !snapshot.members.iter().any(|m| m.ip == ip) {
                    bail!("{ip} はこのネットワークのメンバーにいません");
                }
                group_members.push(ip);
            }
            let states: Vec<(Ipv4Addr, bool)> = snapshot
                .members
                .iter()
                .map(|m| (m.ip, m.online && !m.blocked))
                .collect();
            (states, group_members)
        };
        if group_members.len() < 2 {
            bail!("グループに入れるメンバーを 1 人以上選んでください");
        }
        if group_members.len() > MAX_GROUP_MEMBERS {
            bail!("グループのメンバーが多すぎます(上限 {MAX_GROUP_MEMBERS} 人)");
        }
        group_members.sort();
        let group = GroupInfo {
            id: new_transfer_id(),
            name: name.to_string(),
            members: group_members.clone(),
            revision: 1,
            updated_by: self.cfg.own_ip,
        };
        let mut delivered = 0usize;
        for target in group_members.iter().filter(|ip| **ip != self.cfg.own_ip) {
            let online = member_states.iter().any(|(ip, ok)| ip == target && *ok);
            if !online {
                continue;
            }
            match self.deliver_group(*target, &group) {
                Ok(()) => delivered += 1,
                Err(e) => tracing::warn!("{target} へのグループ配布に失敗しました: {e:#}"),
            }
        }
        if delivered == 0 {
            bail!("オンラインのメンバーに届けられませんでした(全員オフラインの可能性)。あとでやり直してください");
        }
        // ローカルへ取り込み + 作成のお知らせ(グループ名はログへ出さない)
        self.groups.lock().unwrap().apply(group.clone());
        self.chat.lock().unwrap().append(ChatMessageInfo {
            seq: 0,
            id: new_transfer_id(),
            scope: ChatScope::Group,
            group_id: Some(group.id.clone()),
            from: self.cfg.own_ip,
            to: None,
            text: format!("グループ「{}」を作成しました", group.name),
            sent_at: now_unix_ms(),
            failed: false,
            file: None,
            system: true,
        });
        tracing::info!(
            "グループを作成しました(id={} members={} delivered={delivered})",
            group.id,
            group.members.len()
        );
        Ok(group)
    }

    /// グループの改名・メンバー追加・メンバー除外(デスクトップの GroupUpdate
    /// のモバイル版。remove = キックは 2026-07-20 検証 FB)。create_group と
    /// 同じ理由で、オンラインのメンバー 1 人以上に届いたときだけ成立する。
    pub fn update_group(
        &self,
        id: &str,
        name: Option<String>,
        add: Vec<Ipv4Addr>,
        remove: Vec<Ipv4Addr>,
    ) -> anyhow::Result<GroupInfo> {
        let previous = self
            .groups
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .context("このグループはありません")?;
        if !previous.members.contains(&self.cfg.own_ip) {
            bail!("このグループのメンバーではありません");
        }
        let mut group = previous.clone();
        if let Some(name) = name {
            let name = name.trim().to_string();
            if name.is_empty() {
                bail!("グループ名を入力してください");
            }
            if name.len() > MAX_GROUP_NAME_BYTES {
                bail!("グループ名が長すぎます(上限 {MAX_GROUP_NAME_BYTES} バイト)");
            }
            group.name = name;
        }
        {
            let ledger = self.ledger.lock().unwrap();
            let snapshot = ledger.as_ref().context("台帳が未受信です(接続直後?)")?;
            for ip in add {
                if group.members.contains(&ip) {
                    continue;
                }
                if !snapshot.members.iter().any(|m| m.ip == ip) {
                    bail!("{ip} はこのネットワークのメンバーにいません");
                }
                group.members.push(ip);
            }
        }
        for ip in &remove {
            if *ip == self.cfg.own_ip {
                bail!("自分は外せません(「退出」を使ってください)");
            }
        }
        group.members.retain(|ip| !remove.contains(ip));
        if group.members.len() > MAX_GROUP_MEMBERS {
            bail!("グループのメンバーが多すぎます(上限 {MAX_GROUP_MEMBERS} 人)");
        }
        group.revision += 1;
        group.updated_by = self.cfg.own_ip;
        // 外した本人にも配る(本人の画面からグループを引っ込める)
        self.commit_group_change(&previous, group, &remove)
    }

    /// 自分がグループから抜ける。ローカルには自分抜きの全量が残る
    /// (履歴の表示名に使う。UI は会話リストから隠す)。
    pub fn leave_group(&self, id: &str) -> anyhow::Result<()> {
        let previous = self
            .groups
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .context("このグループはありません")?;
        if !previous.members.contains(&self.cfg.own_ip) {
            bail!("このグループのメンバーではありません");
        }
        let mut group = previous.clone();
        group.members.retain(|ip| *ip != self.cfg.own_ip);
        group.revision += 1;
        group.updated_by = self.cfg.own_ip;
        self.commit_group_change(&previous, group, &[])?;
        Ok(())
    }

    /// 変更後の全量を残りのオンラインメンバー(+ `extra` = キックで外した
    /// 本人)へ配って取り込む(1 人以上に届いたときだけ成立 = create_group と
    /// 同じ収束前提)。お知らせ行も足す。
    fn commit_group_change(
        &self,
        previous: &GroupInfo,
        group: GroupInfo,
        extra: &[Ipv4Addr],
    ) -> anyhow::Result<GroupInfo> {
        let (online, names): (Vec<Ipv4Addr>, Vec<(Ipv4Addr, String)>) = {
            let ledger = self.ledger.lock().unwrap();
            let snapshot = ledger.as_ref().context("台帳が未受信です(接続直後?)")?;
            (
                snapshot
                    .members
                    .iter()
                    .filter(|m| m.online && !m.blocked)
                    .map(|m| m.ip)
                    .collect(),
                snapshot
                    .members
                    .iter()
                    .map(|m| (m.ip, m.name.clone().unwrap_or_else(|| m.ip.to_string())))
                    .collect(),
            )
        };
        let mut delivered = 0usize;
        for target in group
            .members
            .iter()
            .chain(extra.iter())
            .filter(|ip| **ip != self.cfg.own_ip && online.contains(ip))
        {
            match self.deliver_group(*target, &group) {
                Ok(()) => delivered += 1,
                Err(e) => tracing::warn!("{target} へのグループ配布に失敗しました: {e:#}"),
            }
        }
        if delivered == 0 {
            bail!("オンラインのメンバーに届けられませんでした(全員オフラインの可能性)。あとでやり直してください");
        }
        let name_of = |ip: Ipv4Addr| -> String {
            if ip == self.cfg.own_ip {
                return "自分".to_string();
            }
            names
                .iter()
                .find(|(addr, _)| *addr == ip)
                .map(|(_, name)| name.clone())
                .unwrap_or_else(|| ip.to_string())
        };
        let lines = groups::system_messages(Some(previous), &group, self.cfg.own_ip, &name_of);
        self.groups.lock().unwrap().apply(group.clone());
        for text in lines {
            self.chat.lock().unwrap().append(ChatMessageInfo {
                seq: 0,
                id: new_transfer_id(),
                scope: ChatScope::Group,
                group_id: Some(group.id.clone()),
                from: self.cfg.own_ip,
                to: None,
                text,
                sent_at: now_unix_ms(),
                failed: false,
                file: None,
                system: true,
            });
        }
        tracing::info!(
            "グループを更新しました(id={} rev={} delivered={delivered})",
            group.id,
            group.revision
        );
        Ok(group)
    }

    fn deliver_group(&self, target: Ipv4Addr, group: &GroupInfo) -> anyhow::Result<()> {
        let (mut reader, mut writer) = self.open_peer(target)?;
        send_json(
            &mut writer,
            &MsgFrame::GroupUpdate {
                group: group.clone(),
            },
        )?;
        let mut line = String::new();
        match read_msg_frame(&mut reader, &mut line)? {
            MsgFrame::GroupAck { id } if id == group.id => Ok(()),
            other => bail!("GroupAck 以外の応答: {other:?}"),
        }
    }

    /// ファイルを 1 人へ送る(チャット文脈付き = 会話にファイルバブルが出る)。
    /// 進捗は transfers に載る。完了時に自分のチャット履歴にも記録する。
    pub fn send_file(&self, target: Ipv4Addr, src: &Path) -> anyhow::Result<String> {
        self.send_file_scoped(ChatScope::Direct, Some(target), None, src)
    }

    /// スコープ付きファイル送信(2026-07-20 検証 FB — スマホからも全体/
    /// グループ宛を可能にする)。network / group はオンラインの対象メンバーへの
    /// 個別転送で、履歴には 1 エントリだけ載る。1 人以上へ届けば成功
    /// (デスクトップの send_file と同じ規則)。ブロッキング(全転送の完了まで
    /// 待つ)なので Kotlin 側は IO ディスパッチャで呼ぶ。
    pub fn send_file_scoped(
        &self,
        scope: ChatScope,
        to: Option<Ipv4Addr>,
        group_id: Option<String>,
        src: &Path,
    ) -> anyhow::Result<String> {
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

        // 宛先の決定(チャットと同じ規則。オフライン宛は V1 非対応)
        let targets: Vec<Ipv4Addr> = match scope {
            ChatScope::Direct => vec![to.context("宛先がありません")?],
            ChatScope::Network => {
                let ledger = self.ledger.lock().unwrap();
                let snapshot = ledger.as_ref().context("台帳が未受信です(接続直後?)")?;
                snapshot
                    .members
                    .iter()
                    .filter(|m| m.ip != self.cfg.own_ip && m.online && !m.blocked)
                    .map(|m| m.ip)
                    .collect()
            }
            ChatScope::Group => {
                let gid = group_id
                    .as_deref()
                    .context("宛先グループ(group_id)がありません")?;
                let members = {
                    let groups = self.groups.lock().unwrap();
                    let group = groups.get(gid).context(
                        "このグループはありません(退出したか、まだ情報が届いていません)",
                    )?;
                    if !group.members.contains(&self.cfg.own_ip) {
                        bail!("このグループのメンバーではありません");
                    }
                    group.members.clone()
                };
                let ledger = self.ledger.lock().unwrap();
                let snapshot = ledger.as_ref().context("台帳が未受信です(接続直後?)")?;
                snapshot
                    .members
                    .iter()
                    .filter(|m| {
                        m.ip != self.cfg.own_ip && m.online && !m.blocked && members.contains(&m.ip)
                    })
                    .map(|m| m.ip)
                    .collect()
            }
        };
        if targets.is_empty() {
            bail!("オンラインの宛先がいません(あとでやり直してください)");
        }
        let ids: Vec<String> = targets.iter().map(|_| new_transfer_id()).collect();

        // 履歴には 1 エントリ(先に載せ、全滅したら失敗の印を付ける)
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
            text: String::new(),
            sent_at: now_unix_ms(),
            failed: false,
            file: Some(ChatFileInfo {
                name: name.clone(),
                size,
                transfers: ids.clone(),
                path: Some(src.to_path_buf()),
            }),
            system: false,
        });

        let ctx = ChatContext { scope, group_id };
        let mut results: Vec<anyhow::Result<()>> = Vec::new();
        std::thread::scope(|s| {
            let handles: Vec<_> = targets
                .iter()
                .zip(ids.iter())
                .map(|(target, id)| {
                    let target = *target;
                    let id = id.clone();
                    let ctx = ctx.clone();
                    let name = name.clone();
                    s.spawn(move || self.send_file_one(target, src, &id, &ctx, &name, size))
                })
                .collect();
            for handle in handles {
                results.push(
                    handle
                        .join()
                        .unwrap_or_else(|_| Err(anyhow::anyhow!("送信スレッドが異常終了しました"))),
                );
            }
        });
        if !results.iter().any(|r| r.is_ok()) {
            self.chat.lock().unwrap().mark_failed(entry.seq);
            // 1 宛先(direct)なら拒否理由をそのまま返す(UI に上限などが出る)
            match results.into_iter().find_map(|r| r.err()) {
                Some(e) => return Err(e),
                None => bail!("どの宛先にも届きませんでした"),
            }
        }
        Ok(ids.first().cloned().unwrap_or_default())
    }

    /// 1 宛先へのファイル転送(進捗は transfers に反映)。
    fn send_file_one(
        &self,
        target: Ipv4Addr,
        src: &Path,
        id: &str,
        ctx: &ChatContext,
        name: &str,
        size: u64,
    ) -> anyhow::Result<()> {
        self.upsert_transfer(TransferInfo {
            id: id.to_string(),
            peer: target,
            name: name.to_string(),
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
                    id: id.to_string(),
                    name: name.to_string(),
                    size,
                    chat: Some(ctx.clone()),
                    // モバイル送信側は再開未対応(resume を立てないので相手は
                    // 常に offset 0 を返す)
                    resume: false,
                },
            )?;
            let mut line = String::new();
            match read_msg_frame(&mut reader, &mut line)? {
                MsgFrame::FileAccept {
                    id: accepted,
                    offset: 0,
                } if accepted == id => {}
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
                self.transfer_progress(id, sent);
            }
            if sent != size {
                bail!("送信中にファイルサイズが変わりました({sent} / {size})");
            }
            send_json(
                &mut writer,
                &MsgFrame::FileHash {
                    id: id.to_string(),
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
                self.transfer_state(id, "done");
                tracing::info!("{target} へファイルを送信しました({size} バイト、id={id})");
            }
            Err(e) => {
                // ファイル名はログに出さない(秘匿ルール)
                tracing::warn!("{target} へのファイル送信に失敗しました: {e:#}");
                self.transfer_state(id, &format!("failed: {e}"));
            }
        }
        result
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
            platform: None,
            capabilities: vec![],
            member_id: None,
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
        test_session_at(label, "127.0.0.1", control_addr, peer_msg_port)
    }

    /// own_ip / listen の IP を指定できる版(グループ配布テストは送り手と
    /// 受け手で別のループバック IP を使う必要がある)。
    fn test_session_at(
        label: &str,
        ip: &str,
        control_addr: SocketAddr,
        peer_msg_port: u16,
    ) -> NetSession {
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
            own_ip: ip.parse().unwrap(),
            display_name: Some(format!("test-{label}")),
            device_id: None,
            network_name: "testnet".to_string(),
            control_addr,
            listen_addr: format!("{ip}:0").parse().unwrap(),
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

        // 鍵ローテーションの依頼: rotate_key 行が届き、応答が (accepted, msg) で返る
        let new_public = PrivateKey::generate().public_key();
        let session_for_rotate = Arc::clone(&session.shared);
        let rotate = std::thread::spawn(move || session_for_rotate.rotate_key_request(new_public));
        let mut got_rotate = false;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            line.clear();
            if reader.read_line(&mut line).unwrap() == 0 {
                break;
            }
            if line.contains(r#""type":"rotate_key""#) {
                assert!(line.contains(&new_public.to_base64()), "{line}");
                got_rotate = true;
                break;
            }
        }
        assert!(got_rotate, "鍵の更新依頼が届かない");
        let result = ControlMessage::RotateKeyResult {
            accepted: true,
            message: "鍵を更新しました".to_string(),
        };
        writer
            .write_all((serde_json::to_string(&result).unwrap() + "\n").as_bytes())
            .unwrap();
        assert_eq!(
            rotate.join().unwrap().unwrap(),
            (true, "鍵を更新しました".to_string())
        );

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

    /// グループ作成(モバイル発)がメンバーへ届き、双方の GroupStore と
    /// 作成者のチャット履歴(お知らせ)に反映される。
    #[test]
    fn create_group_delivers_to_online_member() {
        // 受け手 A は 127.0.0.2、作り手 B は 127.0.0.1(別 IP でないと
        // 自分とメンバーが同一視されてグループが成立しない)
        let a = test_session_at("group-a", "127.0.0.2", "127.0.0.1:1".parse().unwrap(), 1);
        let a_addr = bound_addr(&a);
        let b = test_session("group-b", "127.0.0.1:1".parse().unwrap(), a_addr.port());
        let both = vec![
            ledger_entry("127.0.0.1", "creator", true),
            ledger_entry("127.0.0.2", "member", true),
        ];
        seed_ledger(&a, both.clone());
        seed_ledger(&b, both);

        let group = b
            .shared
            .create_group("家族", vec!["127.0.0.2".parse().unwrap()])
            .unwrap();
        assert_eq!(group.members.len(), 2);
        assert_eq!(group.revision, 1);

        // 受け手にも保存され、追加のお知らせがチャットに載る
        let received = a.shared.groups.lock().unwrap().get(&group.id).cloned();
        assert_eq!(received.map(|g| g.name).as_deref(), Some("家族"));
        let a_chat = a.shared.chat.lock().unwrap().fetch(0, 10);
        assert!(a_chat.iter().any(|m| m.system && m.text.contains("追加")));
        // 作り手側にも保存 + 作成のお知らせ
        assert!(b.shared.groups.lock().unwrap().get(&group.id).is_some());
        let b_chat = b.shared.chat.lock().unwrap().fetch(0, 10);
        assert!(b_chat.iter().any(|m| m.system && m.text.contains("作成")));

        // バリデーション: 空名・台帳外メンバー・メンバー不足
        assert!(b.shared.create_group(" ", vec![]).is_err());
        assert!(b
            .shared
            .create_group("x", vec!["10.9.9.9".parse().unwrap()])
            .is_err());

        a.stop();
        b.stop();
    }

    /// グループの改名・メンバー追加・退出がスマホから配布される(E-E 8)。
    #[test]
    fn update_and_leave_group_propagate() {
        let a = test_session_at("gedit-a", "127.0.0.2", "127.0.0.1:1".parse().unwrap(), 1);
        let a_addr = bound_addr(&a);
        let b = test_session("gedit-b", "127.0.0.1:1".parse().unwrap(), a_addr.port());
        let both = vec![
            ledger_entry("127.0.0.1", "editor", true),
            ledger_entry("127.0.0.2", "member", true),
        ];
        seed_ledger(&a, both.clone());
        seed_ledger(&b, both);

        let group = b
            .shared
            .create_group("旧名", vec!["127.0.0.2".parse().unwrap()])
            .unwrap();

        // 改名: 配布は同期(deliver → apply)なので、成功時点で相手にも届いている
        let renamed = b
            .shared
            .update_group(&group.id, Some("新名".to_string()), vec![], vec![])
            .unwrap();
        assert_eq!(renamed.revision, 2);
        assert_eq!(
            a.shared
                .groups
                .lock()
                .unwrap()
                .get(&group.id)
                .map(|g| g.name.clone())
                .as_deref(),
            Some("新名")
        );
        let b_chat = b.shared.chat.lock().unwrap().fetch(0, 20);
        assert!(b_chat.iter().any(|m| m.system && m.text.contains("新名")));

        // 台帳外メンバーの追加は拒否
        assert!(b
            .shared
            .update_group(&group.id, None, vec!["10.9.9.9".parse().unwrap()], vec![])
            .is_err());

        // 退出: 相手側では自分抜きの全量になり、自分の一覧からは消える
        b.shared.leave_group(&group.id).unwrap();
        let after = a
            .shared
            .groups
            .lock()
            .unwrap()
            .get(&group.id)
            .cloned()
            .unwrap();
        assert!(!after.members.contains(&"127.0.0.1".parse().unwrap()));
        assert!(b
            .shared
            .groups
            .lock()
            .unwrap()
            .joined(b.shared.cfg.own_ip)
            .iter()
            .all(|g| g.id != group.id));

        // 非メンバーになった後の更新は拒否される
        assert!(b
            .shared
            .update_group(&group.id, Some("x".to_string()), vec![], vec![])
            .is_err());

        a.stop();
        b.stop();
    }

    /// 送信キュー(E-E 3): 相手が受け取れない間はエラーにせず失敗の印付きで
    /// キューに残り、受け取れるようになると自動再送で届いて印が消える。
    /// 手動再送で同じ ID が二重に届いても受信側が弾く。
    #[test]
    fn chat_queue_retries_and_receiver_dedups() {
        let a = test_session("queue-a", "127.0.0.1:1".parse().unwrap(), 1);
        let a_addr = bound_addr(&a);
        let b = test_session("queue-b", "127.0.0.1:1".parse().unwrap(), a_addr.port());

        // A は台帳未受信 = 送信元不明として受信を拒否する状態。
        // B が送ってもエラーにはならず、失敗の印 + 送信待ちになる
        b.shared
            .send_chat(
                ChatScope::Direct,
                Some("127.0.0.1".parse().unwrap()),
                None,
                "遅れて届く".to_string(),
            )
            .unwrap();
        let sent = b.shared.chat.lock().unwrap().fetch(0, 10);
        assert!(sent[0].failed, "未達は失敗の印");
        assert_eq!(b.shared.sending_seqs(), vec![sent[0].seq], "送信待ちに残る");

        // 台帳が届くと自動再送(10 秒間隔)で A に届き、失敗の印が消える
        let both = vec![ledger_entry("127.0.0.1", "pair", true)];
        seed_ledger(&a, both.clone());
        seed_ledger(&b, both);
        assert!(
            wait_until(Duration::from_secs(20), || {
                a.shared.chat.lock().unwrap().fetch(0, 10).len() == 1
            }),
            "自動再送で届くはず"
        );
        assert!(wait_until(Duration::from_secs(5), || {
            !b.shared.chat.lock().unwrap().fetch(0, 10)[0].failed
                && b.shared.sending_seqs().is_empty()
        }));

        // 手動再送(送達済みの同じ ID)を受信側が弾いて履歴が増えない
        b.shared.resend_chat(sent[0].seq).unwrap();
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(a.shared.chat.lock().unwrap().fetch(0, 10).len(), 1);

        // 取消: キューから消え、失敗の印が付く
        b.shared
            .send_chat(
                ChatScope::Direct,
                Some("10.99.99.99".parse().unwrap()),
                None,
                "取消する".to_string(),
            )
            .unwrap();
        let seq = b.shared.chat.lock().unwrap().latest_seq();
        b.shared.cancel_chat_send(seq);
        assert!(b.shared.sending_seqs().is_empty());
        assert!(b.shared.chat.lock().unwrap().get(seq).unwrap().failed);

        a.stop();
        b.stop();
    }

    /// 送信待ちキューは再起動を跨いで保持される(E-E 3 残: 永続化)。
    #[test]
    fn chat_queue_survives_restart() {
        let a = test_session("queueper", "127.0.0.1:1".parse().unwrap(), 1);
        a.shared
            .send_chat(
                ChatScope::Direct,
                Some("10.99.99.98".parse().unwrap()),
                None,
                "再起動しても送る".to_string(),
            )
            .unwrap();
        let seq = a.shared.chat.lock().unwrap().latest_seq();
        assert_eq!(a.shared.sending_seqs(), vec![seq]);
        let config_path = a.shared.cfg.config_path.clone();
        assert!(
            chat_queue_path(&config_path).is_file(),
            "キューが保存される"
        );
        a.stop();

        // 同じ設定で立て直すとキューが復元され、自動再送が続く
        let b = NetSession::start(SessionConfig {
            slug: "queueper-2".to_string(),
            config_path: config_path.clone(),
            own_ip: "127.0.0.1".parse().unwrap(),
            display_name: None,
            device_id: None,
            network_name: "testnet".to_string(),
            control_addr: "127.0.0.1:1".parse().unwrap(),
            listen_addr: "127.0.0.1:0".parse().unwrap(),
            peer_msg_port: 1,
        });
        assert_eq!(b.shared.sending_seqs(), vec![seq], "キューが復元される");

        // 取消でキューとファイルが消える
        b.shared.cancel_chat_send(seq);
        assert!(b.shared.sending_seqs().is_empty());
        assert!(!chat_queue_path(&config_path).is_file());
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
