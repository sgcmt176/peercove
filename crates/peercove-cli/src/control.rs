//! トンネル内コントロールチャネル(M1-G2、ADR-0005)。
//!
//! - host: 自分の仮想 IP の TCP [`CONTROL_PORT`] で待受け、接続してきたメンバーに
//!   台帳スナップショットを配布する(接続時 + 変更時)
//! - member: ホストへ接続して Hello を名乗り、届いた台帳を保持する
//!   (status ファイル経由で `status` コマンドに表示される)
//!
//! 接続はトンネル内なので WG により暗号化・認証済み。接続元の仮想 IP が
//! そのメンバーの身元となる(M1-G3 の削除通知はこの対応表を使う)。

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use peercove_core::dns::{CnameRecord, DnsRecord};
use peercove_core::proto::{ControlMessage, LedgerEntry, CONTROL_PORT, PROTO_VERSION};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch};

/// 受信 **1 行** の上限。台帳と共有メモ本文(最大 256 KiB + JSON エスケープ —
/// M5 F-2)が収まるサイズ。メモ系メッセージは `shared_memo` capability を
/// 名乗った新クライアントにしか送らないため、旧上限(64 KiB)のクライアントが
/// 巨大行で切断されることはない。
///
/// `AsyncReadExt::take` の上限は「その reader の累計」なので、1 行読むたびに
/// `set_limit` で戻すこと(戻し忘れると、ping/pong の積み重ねで数時間後に
/// EOF 扱いになって制御接続が落ちる)。
const MAX_LINE_LEN: u64 = 1024 * 1024;
const RETRY_INTERVAL: Duration = Duration::from_secs(5);
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
/// RTT 計測 ping の間隔(M2-G5)。supervisor の周期と同じにして、
/// UI の表示が 1 周期ごとに 1 回は更新されるようにする。
const PING_INTERVAL: Duration = Duration::from_secs(5);
static NEXT_CONNECTION_ID: AtomicU64 = AtomicU64::new(1);

/// 接続中メンバーの送信口と、Hello で広告された製品情報(M3-12)。
#[derive(Clone)]
pub struct ConnectionInfo {
    pub(crate) outbox: mpsc::UnboundedSender<ControlMessage>,
    connection_id: u64,
    close: mpsc::UnboundedSender<()>,
    pub app_version: Option<String>,
    pub capabilities: Vec<String>,
    /// Hello で名乗った OS(E-E 11。台帳の端末バッジに使う)
    pub platform: Option<String>,
}

impl ConnectionInfo {
    pub(crate) fn new(
        outbox: mpsc::UnboundedSender<ControlMessage>,
        app_version: Option<String>,
        capabilities: Vec<String>,
        platform: Option<String>,
    ) -> (Self, mpsc::UnboundedReceiver<()>) {
        let (close, close_rx) = mpsc::unbounded_channel();
        (
            Self {
                outbox,
                connection_id: NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed),
                close,
                app_version,
                capabilities,
                platform,
            },
            close_rx,
        )
    }

    pub fn send(&self, message: ControlMessage) -> bool {
        self.outbox.send(message).is_ok()
    }
}

/// 接続中メンバー(仮想 IP → 接続情報)。削除通知と台帳の製品情報に使う。
pub type Connections = Arc<Mutex<HashMap<Ipv4Addr, ConnectionInfo>>>;

fn register_connection(connections: &Connections, member_ip: Ipv4Addr, connection: ConnectionInfo) {
    let previous = connections.lock().unwrap().insert(member_ip, connection);
    if let Some(previous) = previous {
        // 鍵更新直後など、旧 TCP が半開きのまま新しい認証済み
        // セッションが先に到着することがある。新しい方を正本にする。
        let _ = previous.close.send(());
    }
}

fn unregister_connection(
    connections: &Connections,
    member_ip: Ipv4Addr,
    connection_id: u64,
) -> bool {
    let mut connections = connections.lock().unwrap();
    if connections
        .get(&member_ip)
        .is_some_and(|current| current.connection_id == connection_id)
    {
        connections.remove(&member_ip);
        true
    } else {
        false
    }
}

/// ホストが配布する内容一式(台帳 + カスタム DNS レコード — M3-1)。
/// watch チャネルでまとめて流し、どれかが変わったら再配布される。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Distribution {
    pub members: Vec<LedgerEntry>,
    pub dns_records: Vec<DnsRecord>,
    /// カスタム CNAME レコード(ADR-0025、M3-17)。
    pub cname_records: Vec<CnameRecord>,
    /// ACL の遮断組(ADR-0018、M3-10。仮想 IP の正規化済みペア)。
    /// ワイヤには載せず、送信時にメンバーごとのフィルタ
    /// ([`ledger_message_for`])の材料にする。
    pub deny: Vec<(Ipv4Addr, Ipv4Addr)>,
    /// 細粒度ACLにより直接通信を禁止する組と、その理由となる先頭ルールID。
    pub force_relay: Vec<(Ipv4Addr, Ipv4Addr, String)>,
}

/// 台帳スナップショットをメンバー向けにフィルタして Ledger メッセージにする
/// (ADR-0018、M3-10)。`member_ip` と遮断関係にある相手のエントリは
/// endpoint を落とし(直接通信をさせない)、`blocked` を立てる(UI 表示用)。
fn ledger_message_for(mut dist: Distribution, member_ip: Ipv4Addr) -> ControlMessage {
    let isolated_ips: Vec<Ipv4Addr> = dist
        .members
        .iter()
        .filter(|entry| entry.invite_status.as_deref() == Some("awaiting_approval"))
        .map(|entry| entry.ip)
        .collect();
    let isolated_labels: Vec<String> = dist
        .members
        .iter()
        .filter(|entry| isolated_ips.contains(&entry.ip))
        .filter_map(|entry| entry.dns_name.clone())
        .collect();
    dist.members
        .retain(|entry| !isolated_ips.contains(&entry.ip));
    dist.dns_records
        .retain(|record| !isolated_ips.contains(&record.ip));
    dist.cname_records.retain(|record| {
        !isolated_labels
            .iter()
            .any(|label| record.name.split('.').any(|part| part == label))
    });
    let deny = std::mem::take(&mut dist.deny);
    let force_relay = std::mem::take(&mut dist.force_relay);
    for entry in &mut dist.members {
        let blocked = deny
            .iter()
            .any(|&(a, b)| (a == member_ip && b == entry.ip) || (a == entry.ip && b == member_ip));
        if blocked {
            entry.endpoint = None;
            entry.endpoint_age_secs = None;
            entry.blocked = true;
        }
        if let Some((_, _, rule_id)) = force_relay.iter().find(|(a, b, _)| {
            (*a == member_ip && *b == entry.ip) || (*a == entry.ip && *b == member_ip)
        }) {
            entry.endpoint = None;
            entry.endpoint_age_secs = None;
            entry.force_relay = true;
            entry.acl_rule_id = Some(rule_id.clone());
        }
    }
    ControlMessage::Ledger {
        members: dist.members,
        dns_records: dist.dns_records,
        cname_records: dist.cname_records,
    }
}

/// メンバーが受信した配布内容 + 受信時刻。エンドポイントの鮮度判定
/// (ADR-0013: 配布時の `endpoint_age_secs` + 受信からの経過)に受信時刻が要る。
#[derive(Debug, Clone)]
pub struct ReceivedDistribution {
    pub distribution: Distribution,
    pub received_at: Instant,
}

/// 受信ログの INFO/debug 判定に使う「意味のある内容」の要約(ADR-0019)。
/// エンドポイントとその観測経過(60 秒粒度)は鮮度更新のたびに変わり、
/// 台帳は最大毎分数回再配布されるため、それ**だけ**の変化は debug に落とす。
type LedgerDigest = (
    Vec<(
        Option<String>,                 // name
        Option<String>,                 // dns_name(ADR-0021)
        Ipv4Addr,                       // ip
        peercove_core::keys::PublicKey, // 鍵の入れ替わりも「意味のある変化」
        bool,                           // online
        bool,                           // is_host
        bool,                           // blocked(ACL、ADR-0018)
        bool,                           // force_relay(ACL v2)
        Option<String>,                 // acl_rule_id
        Vec<ipnet::Ipv4Net>,            // subnets(ADR-0014)
        Option<String>,                 // app_version(M3-12)
        Vec<String>,                    // capabilities(M3-12)
    )>,
    Vec<DnsRecord>,
    Vec<CnameRecord>,
);

fn ledger_digest(
    members: &[LedgerEntry],
    dns_records: &[DnsRecord],
    cname_records: &[CnameRecord],
) -> LedgerDigest {
    (
        members
            .iter()
            .map(|m| {
                (
                    m.name.clone(),
                    m.dns_name.clone(),
                    m.ip,
                    m.public_key,
                    m.online,
                    m.is_host,
                    m.blocked,
                    m.force_relay,
                    m.acl_rule_id.clone(),
                    m.subnets.clone(),
                    m.app_version.clone(),
                    m.capabilities.clone(),
                )
            })
            .collect(),
        dns_records.to_vec(),
        cname_records.to_vec(),
    )
}

/// 1 回の supervisor 周期で回収する Ping/Pong 観測値。
#[derive(Debug, Clone, Default)]
pub struct ProbeWindow {
    pub connected: bool,
    pub sent: u32,
    pub received: u32,
    pub rtts_ms: Vec<f64>,
}

/// RTT 最新値と、品質履歴用の未回収 Ping/Pong 観測値。
#[derive(Default)]
pub struct RttState {
    latest: HashMap<Ipv4Addr, f64>,
    pending: HashMap<Ipv4Addr, ProbeWindow>,
}

impl RttState {
    #[cfg(test)]
    fn get(&self, ip: &Ipv4Addr) -> Option<&f64> {
        self.latest.get(ip)
    }

    pub fn latest(&self) -> HashMap<Ipv4Addr, f64> {
        self.latest.clone()
    }

    pub fn drain_probes(&mut self) -> HashMap<Ipv4Addr, ProbeWindow> {
        self.pending
            .iter_mut()
            .map(|(ip, window)| {
                let drained = ProbeWindow {
                    connected: window.connected,
                    sent: std::mem::take(&mut window.sent),
                    received: std::mem::take(&mut window.received),
                    rtts_ms: std::mem::take(&mut window.rtts_ms),
                };
                (*ip, drained)
            })
            .collect()
    }

    fn connected(&mut self, ip: Ipv4Addr) {
        self.pending.entry(ip).or_default().connected = true;
    }

    fn disconnected(&mut self, ip: Ipv4Addr) {
        self.latest.remove(&ip);
        self.pending.entry(ip).or_default().connected = false;
    }

    fn issued(&mut self, ip: Ipv4Addr) {
        let window = self.pending.entry(ip).or_default();
        window.connected = true;
        window.sent = window.sent.saturating_add(1);
    }

    fn observed(&mut self, ip: Ipv4Addr, ms: f64) {
        self.latest.insert(ip, ms);
        let window = self.pending.entry(ip).or_default();
        window.connected = true;
        window.received = window.received.saturating_add(1);
        window.rtts_ms.push(ms);
    }
}

pub type RttMap = Arc<Mutex<RttState>>;

/// メンバー側コントロールチャネルと supervisor の橋渡し(ADR-0020、M3-11)。
/// supervisor が鍵ローテーションの依頼を接続中の送信キューへ差し込み、
/// 読みタスクが受け取った応答をここへ置く(supervisor が周期処理で回収)。
#[derive(Default)]
pub struct MemberLink {
    inner: Mutex<MemberLinkState>,
}

/// [`ControlMessage::CreateInviteResult`] の中身(メンバー側の受け口用)。
/// `token` はメンバー秘密鍵を含む秘密情報 — ログへ出さず、永続化しない。
#[derive(Debug)]
pub struct InviteReply {
    pub accepted: bool,
    pub message: String,
    pub token: Option<String>,
    pub name: Option<String>,
    pub expires_at: Option<u64>,
}

#[derive(Default)]
struct MemberLinkState {
    /// セッション世代。接続のたびに増える(「このセッションで依頼済みか」の判定用)。
    session: u64,
    outbox: Option<Outbox>,
    rotate_result: Option<(bool, String)>,
    /// DNS 名変更(ADR-0021)の応答待ち。IPC ハンドラが受け口を握り、
    /// 読みタスクが応答を流し込む。切断(attach)で捨てられると受け口側は
    /// Err になる(= 接続が切れた)。
    dns_reply: Option<tokio::sync::oneshot::Sender<(bool, String)>>,
    /// 表示名変更(ADR-0027)の応答待ち。dns_reply と同じ仕組み。
    display_reply: Option<tokio::sync::oneshot::Sender<(bool, String)>>,
    /// メンバー招待の発行依頼(ADR-0048)の応答待ち。dns_reply と同じ仕組み。
    invite_reply: Option<tokio::sync::oneshot::Sender<InviteReply>>,
    /// 共有メモ(M5 F-2)の応答待ち(seq → 受け口)。他の依頼と違い
    /// 同時に複数飛ばせる(一覧 + 本文の取り直しなど)。
    memo_pending: HashMap<u64, tokio::sync::oneshot::Sender<peercove_core::memo::SharedMemoReply>>,
    /// MemoReq の連番(セッションをまたいで単調増加)。
    memo_seq: u64,
    /// 自動再試行では回復しない、ホストからの参加拒否理由。
    connection_error: Option<String>,
}

impl MemberLink {
    /// 接続中ならセッション世代を返す。
    pub fn session(&self) -> Option<u64> {
        let state = self.inner.lock().unwrap();
        state.outbox.is_some().then_some(state.session)
    }

    /// 接続中なら送信キューへ積む(切断済みなら false)。
    pub fn send(&self, message: ControlMessage) -> bool {
        let state = self.inner.lock().unwrap();
        match &state.outbox {
            Some(outbox) => outbox.send(message).is_ok(),
            None => false,
        }
    }

    /// 届いた rotate_key_result を取り出す(1 回限り)。
    pub fn take_rotate_result(&self) -> Option<(bool, String)> {
        self.inner.lock().unwrap().rotate_result.take()
    }

    /// ホストが参加を拒否した場合の、UI に表示する理由。
    pub fn connection_error(&self) -> Option<String> {
        self.inner.lock().unwrap().connection_error.clone()
    }

    /// 自分の DNS 名の変更をホストへ依頼し、応答の受け口を返す(ADR-0021)。
    /// 切断中なら `None`。先行する依頼の応答待ちがあれば破棄される
    /// (受け口側は Err = 打ち切り扱い)。
    pub fn request_dns_name(
        &self,
        name: String,
    ) -> Option<tokio::sync::oneshot::Receiver<(bool, String)>> {
        let mut state = self.inner.lock().unwrap();
        let outbox = state.outbox.as_ref()?;
        if outbox.send(ControlMessage::SetDnsName { name }).is_err() {
            return None;
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        state.dns_reply = Some(tx);
        Some(rx)
    }

    /// 自分の表示名の変更をホストへ依頼し、応答の受け口を返す(ADR-0027)。
    /// 切断中なら `None`。仕組みは [`Self::request_dns_name`] と同じ。
    pub fn request_display_name(
        &self,
        name: String,
    ) -> Option<tokio::sync::oneshot::Receiver<(bool, String)>> {
        let mut state = self.inner.lock().unwrap();
        let outbox = state.outbox.as_ref()?;
        if outbox
            .send(ControlMessage::SetDisplayName { name })
            .is_err()
        {
            return None;
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        state.display_reply = Some(tx);
        Some(rx)
    }

    /// メンバー招待の発行をホストへ依頼し、応答の受け口を返す(ADR-0048)。
    /// 切断中なら `None`。仕組みは [`Self::request_dns_name`] と同じ。
    pub fn request_create_invite(
        &self,
        name: Option<String>,
        expires_in_secs: Option<u64>,
    ) -> Option<tokio::sync::oneshot::Receiver<InviteReply>> {
        let mut state = self.inner.lock().unwrap();
        let outbox = state.outbox.as_ref()?;
        if outbox
            .send(ControlMessage::CreateInvite {
                name,
                expires_in_secs,
            })
            .is_err()
        {
            return None;
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        state.invite_reply = Some(tx);
        Some(rx)
    }

    /// 共有メモの操作をホストへ送り、応答の受け口を返す(M5 F-2)。
    /// 切断中なら `None`。他の依頼と違い同時に複数を待てる。
    pub fn request_memo(
        &self,
        op: peercove_core::memo::SharedMemoOp,
    ) -> Option<tokio::sync::oneshot::Receiver<peercove_core::memo::SharedMemoReply>> {
        let mut state = self.inner.lock().unwrap();
        state.memo_seq += 1;
        let seq = state.memo_seq;
        let outbox = state.outbox.as_ref()?;
        if outbox.send(ControlMessage::MemoReq { seq, op }).is_err() {
            return None;
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        state.memo_pending.insert(seq, tx);
        Some(rx)
    }

    fn put_memo_result(&self, seq: u64, reply: peercove_core::memo::SharedMemoReply) {
        if let Some(sender) = self.inner.lock().unwrap().memo_pending.remove(&seq) {
            let _ = sender.send(reply);
        }
    }

    fn attach(&self, outbox: Outbox) {
        let mut state = self.inner.lock().unwrap();
        state.session += 1;
        state.outbox = Some(outbox);
        state.rotate_result = None;
        state.dns_reply = None; // 応答待ちの受け口は Err(切断)になる
        state.display_reply = None;
        state.invite_reply = None;
        state.memo_pending.clear();
        state.connection_error = None;
    }

    fn detach(&self) {
        let mut state = self.inner.lock().unwrap();
        state.outbox = None;
        state.dns_reply = None;
        state.display_reply = None;
        state.invite_reply = None;
        state.memo_pending.clear();
    }

    fn put_rotate_result(&self, accepted: bool, message: String) {
        self.inner.lock().unwrap().rotate_result = Some((accepted, message));
    }

    fn put_dns_result(&self, accepted: bool, message: String) {
        if let Some(reply) = self.inner.lock().unwrap().dns_reply.take() {
            let _ = reply.send((accepted, message));
        }
    }

    fn put_display_result(&self, accepted: bool, message: String) {
        if let Some(reply) = self.inner.lock().unwrap().display_reply.take() {
            let _ = reply.send((accepted, message));
        }
    }

    fn put_invite_result(&self, result: InviteReply) {
        if let Some(reply) = self.inner.lock().unwrap().invite_reply.take() {
            let _ = reply.send(result);
        }
    }

    fn reject(&self, message: String) {
        self.inner.lock().unwrap().connection_error = Some(message);
    }
}

#[cfg(test)]
impl MemberLink {
    /// テスト用: 接続済み状態を作り、送信キューの受け口を返す(rotate.rs)。
    pub(crate) fn attach_for_test(&self) -> mpsc::UnboundedReceiver<ControlMessage> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.attach(tx);
        rx
    }

    /// テスト用: ホストからの応答が届いた状態を作る(rotate.rs)。
    pub(crate) fn put_rotate_result_for_test(&self, accepted: bool, message: String) {
        self.put_rotate_result(accepted, message);
    }
}

/// メンバー発の設定変更依頼をホスト側で host.toml へ適用する
/// (鍵ローテーション = ADR-0020 / DNS 名変更 = ADR-0021)。
/// 適用の反映(WG ピアの入れ替え・台帳の再配布)は supervisor の
/// 次回再読込(≤5 秒)が行うため、応答は現行セッションが生きているうちに届く。
pub struct HostRequests {
    config_path: std::path::PathBuf,
    host_public_key: peercove_core::keys::PublicKey,
    /// (host)UPnP で観測した外部エンドポイント。メンバー発行の招待
    /// (ADR-0048)のエンドポイント候補に自動で足す(ホスト UI 発行と同等)。
    external_endpoint: Option<std::net::SocketAddrV4>,
    /// 共有メモのサービス(M5 F-2)。メンバーの MemoReq をここへ流す。
    memo: Arc<crate::memoshare::MemoService>,
    /// host.toml の読み書きを直列化する(複数メンバーの同時依頼)。
    lock: tokio::sync::Mutex<()>,
}

impl HostRequests {
    pub fn new(
        config_path: std::path::PathBuf,
        host_public_key: peercove_core::keys::PublicKey,
        external_endpoint: Option<std::net::SocketAddrV4>,
        memo: Arc<crate::memoshare::MemoService>,
    ) -> Self {
        Self {
            config_path,
            host_public_key,
            external_endpoint,
            memo,
            lock: tokio::sync::Mutex::new(()),
        }
    }

    /// 鍵ローテーション依頼を適用し、(accepted, メッセージ) を返す。
    async fn apply_rotate_key(
        &self,
        member_ip: Ipv4Addr,
        new_key: peercove_core::keys::PublicKey,
    ) -> (bool, String) {
        use peercove_ops::peers::RotateOutcome;
        let _guard = self.lock.lock().await;
        let path = self.config_path.clone();
        let host_key = self.host_public_key;
        let outcome = tokio::task::spawn_blocking(move || {
            peercove_ops::peers::rotate_peer_key(&path, member_ip, &new_key, &host_key)
        })
        .await;
        match outcome {
            Ok(Ok(RotateOutcome::Applied { display: name })) => {
                tracing::info!("メンバー {member_ip}({name})の公開鍵を更新しました: {new_key}");
                (
                    true,
                    "更新を受け付けました(数秒で新しい鍵に切り替わります)".to_string(),
                )
            }
            Ok(Ok(RotateOutcome::Unchanged)) => (true, "既に更新済みです".to_string()),
            Ok(Err(e)) => {
                tracing::warn!("メンバー {member_ip} の鍵更新を拒否しました: {e:#}");
                (false, format!("{e:#}"))
            }
            Err(e) => {
                tracing::warn!("鍵更新の適用タスクが失敗しました: {e}");
                (false, "ホスト側の内部エラーです".to_string())
            }
        }
    }

    /// DNS 名の変更依頼(ADR-0021)を適用し、(accepted, メッセージ) を返す。
    async fn apply_dns_name(&self, member_ip: Ipv4Addr, name: String) -> (bool, String) {
        use peercove_ops::peers::DnsNameOutcome;
        let _guard = self.lock.lock().await;
        let path = self.config_path.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            peercove_ops::peers::set_peer_dns_name_by_ip(&path, member_ip, &name)
        })
        .await;
        match outcome {
            Ok(Ok(DnsNameOutcome::Applied { display, label })) => {
                // tracing マクロは {display} を同名関数に解決してしまうため束縛し直す
                let name = display;
                tracing::info!("メンバー {member_ip}({name})の DNS 名を {label} に変更しました");
                (
                    true,
                    format!("DNS 名を {label} に変更しました(数秒で全員に反映されます)"),
                )
            }
            Ok(Ok(DnsNameOutcome::Unchanged { label })) => {
                (true, format!("DNS 名は既に {label} です"))
            }
            Ok(Err(e)) => {
                tracing::info!("メンバー {member_ip} の DNS 名変更を受け付けませんでした: {e:#}");
                (false, format!("{e:#}"))
            }
            Err(e) => {
                tracing::warn!("DNS 名変更の適用タスクが失敗しました: {e}");
                (false, "ホスト側の内部エラーです".to_string())
            }
        }
    }

    /// 表示名の変更依頼(ADR-0027)を適用し、(accepted, メッセージ) を返す。
    /// 表示名は host.toml `[[peer]].name` が正本なので rename_peer と同型で書く。
    async fn apply_display_name(&self, member_ip: Ipv4Addr, name: String) -> (bool, String) {
        let _guard = self.lock.lock().await;
        let path = self.config_path.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            peercove_ops::peers::set_peer_display_name_by_ip(&path, member_ip, &name)
        })
        .await;
        match outcome {
            Ok(Ok(name)) => {
                tracing::info!("メンバー {member_ip} の表示名を「{name}」に変更しました");
                (
                    true,
                    format!("表示名を「{name}」に変更しました(数秒で全員に反映されます)"),
                )
            }
            Ok(Err(e)) => {
                tracing::info!("メンバー {member_ip} の表示名変更を受け付けませんでした: {e:#}");
                (false, format!("{e:#}"))
            }
            Err(e) => {
                tracing::warn!("表示名変更の適用タスクが失敗しました: {e}");
                (false, "ホスト側の内部エラーです".to_string())
            }
        }
    }

    /// メンバー招待の発行依頼(ADR-0048)を適用し、応答メッセージを返す。
    /// 権限(グローバルトグル + can_invite)はホスト正本で依頼ごとに確認する。
    /// トークンは秘密情報 — ログには発行者 IP と invite_id だけ残す。
    async fn apply_create_invite(
        &self,
        member_ip: Ipv4Addr,
        name: Option<String>,
        expires_in_secs: Option<u64>,
    ) -> ControlMessage {
        /// メンバー発行の有効期限の上限(ホスト UI の既定と同じ 7 日)。
        /// 無期限(None)は指定できず、上限に丸める(ADR-0048)。
        const MAX_EXPIRES_SECS: u64 = 7 * 24 * 60 * 60;
        let _guard = self.lock.lock().await;
        let path = self.config_path.clone();
        let external = self.external_endpoint;
        let outcome = tokio::task::spawn_blocking(move || {
            let config = peercove_core::config::Config::load(&path)?;
            if !config.interface.member_invites {
                anyhow::bail!("ホストの設定でメンバーによる招待発行が無効になっています");
            }
            let issuer = peercove_ops::peers::find_peer(
                &config,
                &peercove_ops::peers::Selector::Ip(member_ip),
            )?;
            if !issuer.can_invite {
                anyhow::bail!(
                    "この端末には招待発行の権限がありません(ホスト管理者が許可すると使えます)"
                );
            }
            let issuer_id = issuer
                .invite_id
                .clone()
                .context("発行者の識別子が無いため発行できません(旧形式の登録)")?;
            let issuer_name = issuer
                .name
                .clone()
                .unwrap_or_else(|| format!("member-{member_ip}"));
            let expires = expires_in_secs
                .unwrap_or(MAX_EXPIRES_SECS)
                .clamp(60, MAX_EXPIRES_SECS);
            let extra: Vec<std::net::SocketAddrV4> = external.into_iter().collect();
            peercove_ops::invite::invite(&peercove_ops::invite::InviteOptions {
                config_path: &path,
                name: name.as_deref().map(str::trim).filter(|s| !s.is_empty()),
                ip: None,
                extra_endpoints: &extra,
                psk: false, // メンバー発行では PSK は指定不可(ADR-0048)
                expires_in_secs: Some(expires),
                invited_by: Some((&issuer_id, &issuer_name)),
            })
        })
        .await;
        match outcome {
            Ok(Ok(result)) => {
                // 可視化(ADR-0048)。名前・トークンはログへ出さない
                tracing::info!(
                    "メンバー {member_ip} がメンバー招待を発行しました(invite_id={})",
                    result.invite_id
                );
                ControlMessage::CreateInviteResult {
                    accepted: true,
                    message: "招待を発行しました".to_string(),
                    token: Some(result.token),
                    name: Some(result.name),
                    expires_at: result.expires_at,
                }
            }
            Ok(Err(e)) => {
                tracing::info!("メンバー {member_ip} の招待発行を受け付けませんでした: {e:#}");
                ControlMessage::CreateInviteResult {
                    accepted: false,
                    message: format!("{e:#}"),
                    token: None,
                    name: None,
                    expires_at: None,
                }
            }
            Err(e) => {
                tracing::warn!("招待発行の適用タスクが失敗しました: {e}");
                ControlMessage::CreateInviteResult {
                    accepted: false,
                    message: "ホスト側の内部エラーです".to_string(),
                    token: None,
                    name: None,
                    expires_at: None,
                }
            }
        }
    }

    /// 招待 v3 の期限をホスト正本で確認し、初回 Hello を永続化する。
    async fn accept_invite(
        &self,
        member_ip: Ipv4Addr,
        device_id: Option<String>,
    ) -> anyhow::Result<bool> {
        let _guard = self.lock.lock().await;
        let path = self.config_path.clone();
        let accepted_at = unix_secs();
        tokio::task::spawn_blocking(move || {
            peercove_ops::peers::mark_invite_accepted_by_ip(
                &path,
                member_ip,
                accepted_at,
                device_id.as_deref(),
            )?;
            let config = peercove_core::config::Config::load(&path)?;
            let peer = config
                .peers
                .iter()
                .find(|peer| peer.allowed_ips.first().map(|net| net.addr()) == Some(member_ip))
                .context("参加端末が host.toml に見つかりません")?;
            Ok::<_, anyhow::Error>(peer.invite_is_isolated())
        })
        .await
        .context("招待状態の更新タスクが失敗しました")?
    }
}

fn encode_line(message: &ControlMessage) -> String {
    let mut line = serde_json::to_string(message).expect("ControlMessage は常に直列化可能");
    line.push('\n');
    line
}

/// 送信済み ping の記録。ping を打つのは書き側、Pong を見るのは読み側なので共有する。
///
/// 応答が返らないまま次の周期に入った場合、その 1 回分の測定は捨てる
/// (前回値は [`RttMap`] に残るので、UI は最後に測れた値を出し続ける)。
#[derive(Default)]
struct PingState {
    next_nonce: u64,
    outstanding: Option<(u64, Instant)>,
}

impl PingState {
    /// 次に送る ping メッセージ。送信時刻を記録する。
    fn issue(&mut self) -> ControlMessage {
        self.next_nonce += 1;
        let nonce = self.next_nonce;
        self.outstanding = Some((nonce, Instant::now()));
        ControlMessage::Ping { nonce }
    }

    /// Pong を受け取ったときの RTT(未知の nonce なら None)。
    fn observe(&mut self, nonce: u64) -> Option<f64> {
        match self.outstanding {
            Some((sent, at)) if sent == nonce => {
                self.outstanding = None;
                Some(at.elapsed().as_secs_f64() * 1000.0)
            }
            _ => None,
        }
    }
}

type SharedPing = Arc<Mutex<PingState>>;
/// 相手へ送るメッセージのキュー(削除通知・Pong の配送口)。
type Outbox = mpsc::UnboundedSender<ControlMessage>;

type LineReader = tokio::io::Take<BufReader<tokio::net::tcp::OwnedReadHalf>>;

fn line_reader(read_half: tokio::net::tcp::OwnedReadHalf) -> LineReader {
    BufReader::new(read_half).take(MAX_LINE_LEN)
}

/// 1 行読む。`None` は EOF(相手が切断)。
///
/// **`read_line` はキャンセル安全でない**(途中まで読んだバイトが失われる)。
/// そのため読み側は必ず専用タスクの素直なループで回し、`select!` の分岐には
/// 置かないこと。この関数を使う側もそれを守る前提。
async fn read_line(reader: &mut LineReader, line: &mut String) -> anyhow::Result<Option<()>> {
    reader.set_limit(MAX_LINE_LEN); // 上限は累計なので 1 行ごとに戻す
    line.clear();
    if reader.read_line(line).await? == 0 {
        return Ok(None); // 相手が切断
    }
    if !line.ends_with('\n') {
        // read_line は改行か EOF まで読む。改行が無いのは上限に達したか、行の
        // 途中で切断されたかのどちらか
        if reader.limit() == 0 {
            anyhow::bail!("1 行が {MAX_LINE_LEN} バイトを超えました");
        }
        anyhow::bail!("行の途中で切断されました");
    }
    Ok(Some(()))
}

/// 読み側の共通処理: ping には pong を返し、pong からは RTT を記録する。
/// 処理したら `true`(呼び出し側はそれ以外のメッセージを自分で捌く)。
fn handle_ping_pong(
    message: &ControlMessage,
    peer_ip: Ipv4Addr,
    out: &Outbox,
    ping: &SharedPing,
    rtt: &RttMap,
) -> bool {
    match *message {
        ControlMessage::Ping { nonce } => {
            let _ = out.send(ControlMessage::Pong { nonce });
            true
        }
        ControlMessage::Pong { nonce } => {
            if let Some(ms) = ping.lock().unwrap().observe(nonce) {
                rtt.lock().unwrap().observed(peer_ip, ms);
            }
            true
        }
        _ => false,
    }
}

/// ホスト側サーバー。台帳の変更を watch チャネルで受け取り、全接続へ配布する。
pub async fn run_host_server(
    bind_ip: Ipv4Addr,
    ledger_rx: watch::Receiver<Distribution>,
    connections: Connections,
    rtt: RttMap,
    requests: Arc<HostRequests>,
) {
    // トンネル作成直後は Windows が仮想 IP を数秒間「準備中」として扱うため、
    // bind が 10049 等で失敗する。準備が整うまで 1 秒間隔でリトライする
    let listener = loop {
        match TcpListener::bind(SocketAddr::from((bind_ip, CONTROL_PORT))).await {
            Ok(listener) => break listener,
            Err(e) => {
                tracing::debug!(
                    "コントロールチャネル起動待ち(トンネルのアドレス準備中。想定内): {e}"
                );
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    };
    tracing::info!("コントロールチャネルを {bind_ip}:{CONTROL_PORT} で待受けます");

    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let member_ip = match peer.ip() {
                    IpAddr::V4(ip) => ip,
                    IpAddr::V6(_) => continue, // M0/M1 は IPv4 のみ
                };
                let ledger_rx = ledger_rx.clone();
                let connections = Arc::clone(&connections);
                let rtt = Arc::clone(&rtt);
                let requests = Arc::clone(&requests);
                tokio::spawn(async move {
                    if let Err(e) =
                        handle_member(stream, member_ip, ledger_rx, &connections, &rtt, requests)
                            .await
                    {
                        tracing::debug!("メンバー {member_ip} との制御接続が終了: {e:#}");
                    }
                    tracing::info!("メンバー {member_ip} の制御接続が切断されました");
                });
            }
            Err(e) => {
                tracing::warn!("コントロールチャネルの accept に失敗: {e}");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

/// ホスト側 1 接続。読みと書きを別タスクに分ける。
///
/// 分けるのは `read_line` がキャンセル安全でないため。書き側の `select!` に
/// 混ぜると、台帳の配布や ping のタイミングで読みかけの行が捨てられてしまう。
async fn handle_member(
    stream: TcpStream,
    member_ip: Ipv4Addr,
    mut ledger_rx: watch::Receiver<Distribution>,
    connections: &Connections,
    rtt: &RttMap,
    requests: Arc<HostRequests>,
) -> anyhow::Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = line_reader(read_half);

    // 最初のメッセージは Hello(名乗り)
    let mut line = String::new();
    let hello = tokio::time::timeout(HELLO_TIMEOUT, read_line(&mut reader, &mut line))
        .await
        .map_err(|_| anyhow::anyhow!("Hello がタイムアウトしました"))??;
    if hello.is_none() {
        anyhow::bail!("Hello の前に切断されました");
    }
    let (name, app_version, capabilities, device_id, platform) =
        match serde_json::from_str::<ControlMessage>(&line) {
            Ok(ControlMessage::Hello {
                version,
                name,
                app_version,
                capabilities,
                device_id,
                platform,
            }) => {
                if version != PROTO_VERSION {
                    tracing::warn!(
                        "メンバー {member_ip} のプロトコルバージョン {version} は未対応です\
                    (こちらは {PROTO_VERSION})"
                    );
                }
                (name, app_version, capabilities, device_id, platform)
            }
            Ok(other) => anyhow::bail!("Hello 以外のメッセージが届きました: {other:?}"),
            Err(e) => anyhow::bail!("Hello の解析に失敗しました: {e}"),
        };

    let isolated = match requests.accept_invite(member_ip, device_id).await {
        Ok(isolated) => isolated,
        Err(error) => {
            let reason = format!("{error:#}");
            let message = format!("{reason}。ホストから新しい招待トークンを受け取ってください。");
            write_half
                .write_all(encode_line(&ControlMessage::JoinRejected { message }).as_bytes())
                .await
                .context("参加拒否理由をメンバーへ送信できません")?;
            write_half.flush().await?;
            tracing::warn!("メンバー {member_ip} の参加を拒否しました: {reason}");
            anyhow::bail!("メンバー {member_ip} の招待を受け付けられません: {reason}");
        }
    };
    tracing::info!(
        "メンバー {member_ip}({})が接続しました(version={})",
        name.as_deref().unwrap_or("名前なし"),
        app_version.as_deref().unwrap_or("不明")
    );

    // 送信キューを登録(台帳変更・削除通知・Pong の配送口)
    let (tx, rx) = mpsc::unbounded_channel::<ControlMessage>();
    let (connection, mut close_rx) =
        ConnectionInfo::new(tx.clone(), app_version, capabilities, platform);
    let connection_id = connection.connection_id;
    register_connection(connections, member_ip, connection);
    rtt.lock().unwrap().connected(member_ip);

    // 現在の台帳を即送信。borrow_and_update で「見た」ことにして、
    // 直後の changed() が同じ内容をもう一度送るのを防ぐ
    let snapshot = ledger_rx.borrow_and_update().clone();
    let ping: SharedPing = Default::default();
    let isolation = Arc::new(AtomicBool::new(isolated));

    let mut writer = tokio::spawn(host_writer(
        write_half,
        member_ip,
        snapshot,
        ledger_rx,
        rx,
        Arc::clone(&ping),
        Arc::clone(&isolation),
        Arc::clone(rtt),
    ));
    let mut read_task = tokio::spawn(host_reader(
        reader,
        member_ip,
        tx,
        ping,
        Arc::clone(rtt),
        line,
        Arc::clone(&requests),
        isolation,
    ));

    // どちらかが終わったら接続を畳む。
    // biased: 削除通知を送り終えた書き側の Ok(()) が「意味のある終了」なので先に見る。
    // (読み側が先に終わると tx が落ちて書き側も即 ready になり、順序が非決定になる)
    let result = tokio::select! {
        biased;
        _ = close_rx.recv() => {
            writer.abort();
            read_task.abort();
            Ok(Ok(()))
        }
        joined = &mut writer => { read_task.abort(); joined }
        joined = &mut read_task => { writer.abort(); joined }
    };
    let result =
        result.unwrap_or_else(|e| Err(anyhow::anyhow!("制御タスクが異常終了しました: {e}")));
    let removed_current = unregister_connection(connections, member_ip, connection_id);
    if removed_current {
        rtt.lock().unwrap().disconnected(member_ip);
        // 切断したメンバーが握っていた共有メモの編集ロックを解放する(要件 §12)
        requests.memo.release_locks_for_ip(member_ip);
    }
    result
}

fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// 書き側。分岐はすべてキャンセル安全なものだけにすること。
#[allow(clippy::too_many_arguments)]
async fn host_writer(
    mut write_half: tokio::net::tcp::OwnedWriteHalf,
    member_ip: Ipv4Addr,
    initial: Distribution,
    mut ledger_rx: watch::Receiver<Distribution>,
    mut rx: mpsc::UnboundedReceiver<ControlMessage>,
    ping: SharedPing,
    isolation: Arc<AtomicBool>,
    rtt: RttMap,
) -> anyhow::Result<()> {
    if !isolation.load(Ordering::Relaxed) {
        write_half
            .write_all(encode_line(&ledger_message_for(initial, member_ip)).as_bytes())
            .await?;
    }

    let mut ping_tick = tokio::time::interval(PING_INTERVAL);
    loop {
        tokio::select! {
            changed = ledger_rx.changed() => {
                if changed.is_err() {
                    return Ok(()); // 送信側(supervisor)終了
                }
                let snapshot = ledger_rx.borrow_and_update().clone();
                let isolated = snapshot.members.iter().any(|entry| {
                    entry.ip == member_ip
                        && entry.invite_status.as_deref() == Some("awaiting_approval")
                });
                isolation.store(isolated, Ordering::Relaxed);
                if !isolated {
                    write_half
                        .write_all(encode_line(&ledger_message_for(snapshot, member_ip)).as_bytes())
                        .await?;
                }
            }
            queued = rx.recv() => {
                match queued {
                    Some(message) => {
                        let is_removed = matches!(message, ControlMessage::Removed { .. });
                        write_half.write_all(encode_line(&message).as_bytes()).await?;
                        if is_removed {
                            write_half.flush().await?;
                            return Ok(()); // 削除通知後は切断
                        }
                    }
                    None => return Ok(()),
                }
            }
            _ = ping_tick.tick() => {
                let message = ping.lock().unwrap().issue();
                rtt.lock().unwrap().issued(member_ip);
                write_half.write_all(encode_line(&message).as_bytes()).await?;
            }
        }
    }
}

/// 読み側。`select!` を使わない素直なループ(read_line のキャンセル安全性のため)。
#[allow(clippy::too_many_arguments)]
async fn host_reader(
    mut reader: LineReader,
    member_ip: Ipv4Addr,
    out: Outbox,
    ping: SharedPing,
    rtt: RttMap,
    mut line: String,
    requests: Arc<HostRequests>,
    isolation: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    loop {
        if read_line(&mut reader, &mut line).await?.is_none() {
            return Ok(()); // メンバー側が切断
        }
        match serde_json::from_str::<ControlMessage>(&line) {
            Ok(message) if handle_ping_pong(&message, member_ip, &out, &ping, &rtt) => {}
            Ok(ControlMessage::RotateKey { .. }) if isolation.load(Ordering::Relaxed) => {
                tracing::debug!("承認待ち端末 {member_ip} からの鍵更新依頼を拒否しました");
            }
            Ok(
                ControlMessage::SetDnsName { .. }
                | ControlMessage::SetDisplayName { .. }
                | ControlMessage::CreateInvite { .. }
                | ControlMessage::MemoReq { .. },
            ) if isolation.load(Ordering::Relaxed) => {
                tracing::debug!("承認待ち端末 {member_ip} からの設定変更依頼を拒否しました");
            }
            Ok(ControlMessage::RotateKey { new_public_key }) => {
                // 鍵ローテーション(ADR-0020)。apply は host.toml への永続化のみで、
                // WG ピアの入れ替えは supervisor の次回再読込(≤5 秒)が行う。
                // 応答が先に返るので、旧鍵のセッションが生きているうちに届く
                let (accepted, message) =
                    requests.apply_rotate_key(member_ip, new_public_key).await;
                let _ = out.send(ControlMessage::RotateKeyResult { accepted, message });
            }
            Ok(ControlMessage::SetDnsName { name }) => {
                // DNS 名の変更依頼(ADR-0021)。永続化のみで、台帳への反映は
                // supervisor の次回再読込(≤5 秒)が行う
                let (accepted, message) = requests.apply_dns_name(member_ip, name).await;
                let _ = out.send(ControlMessage::SetDnsNameResult { accepted, message });
            }
            Ok(ControlMessage::SetDisplayName { name }) => {
                // 表示名の変更依頼(ADR-0027)。DNS 名と同じく永続化のみで、
                // 台帳への反映は supervisor の次回再読込(≤5 秒)が行う
                let (accepted, message) = requests.apply_display_name(member_ip, name).await;
                let _ = out.send(ControlMessage::SetDisplayNameResult { accepted, message });
            }
            Ok(ControlMessage::CreateInvite {
                name,
                expires_in_secs,
            }) => {
                // メンバー招待の発行依頼(ADR-0048)。応答(トークン入り)は
                // 依頼が来たこの接続の outbox にだけ流す
                let reply = requests
                    .apply_create_invite(member_ip, name, expires_in_secs)
                    .await;
                let _ = out.send(reply);
            }
            Ok(ControlMessage::MemoReq { seq, op }) => {
                // 共有メモの依頼(M5 F-2)。権限・ロック・リビジョンの判定は
                // サービス層(ホスト正本)。応答は依頼が来たこの接続にだけ返す。
                // メモの内容はログへ出さない(ADR-0049)
                let reply = requests.memo.handle_for_member(member_ip, op).await;
                let _ = out.send(ControlMessage::MemoResp { seq, reply });
            }
            Ok(_) => tracing::debug!("メンバー {member_ip} から: {}", line.trim_end()),
            Err(e) => tracing::debug!("解析できないメッセージを無視: {e}"),
        }
    }
}

/// メンバー側クライアント。台帳を受信して `latest_ledger` に反映する。
/// 切断されたら自動で再接続する。
///
/// ホストから削除された(`Removed`)ら `removed` を立てて**再接続をやめる**。
/// 削除後はホストが WG ピアも消すので再接続は成功しないうえ、UI に「削除された」
/// と出したまま無駄なリトライを続けないため(M2-G6 のフィードバック)。
#[allow(clippy::too_many_arguments)]
pub async fn run_member_client(
    host_ip: Ipv4Addr,
    display_name: Option<String>,
    device_id: Option<String>,
    latest_ledger: Arc<Mutex<Option<ReceivedDistribution>>>,
    rtt: RttMap,
    removed: Arc<AtomicBool>,
    link: Arc<MemberLink>,
    memo_cache: Arc<crate::memoshare::MemberMemoCache>,
) {
    let target = SocketAddr::from((host_ip, CONTROL_PORT));
    let mut logged_wait = false;
    loop {
        match TcpStream::connect(target).await {
            Ok(stream) => {
                logged_wait = false;
                tracing::info!("コントロールチャネルに接続しました({target})");
                let session = member_session(
                    stream,
                    &display_name,
                    &device_id,
                    &latest_ledger,
                    host_ip,
                    &rtt,
                    &removed,
                    &link,
                    &memo_cache,
                );
                let result = session.await;
                let connection_error = link.connection_error();
                link.detach();
                if let Some(message) = connection_error {
                    tracing::warn!("ホストが参加を拒否したため再接続を停止します: {message}");
                    rtt.lock().unwrap().disconnected(host_ip);
                    return;
                }
                if let Err(e) = result {
                    tracing::debug!("制御接続が終了しました(再接続します): {e:#}");
                }
                rtt.lock().unwrap().disconnected(host_ip);
                if removed.load(Ordering::Relaxed) {
                    tracing::info!("削除通知を受けたので再接続を停止します");
                    return;
                }
            }
            Err(e) => {
                // トンネル確立前は失敗して当然なので、最初の 1 回だけ案内する
                if !logged_wait {
                    tracing::info!("コントロールチャネル接続待ち(トンネル確立後に自動接続): {e}");
                    logged_wait = true;
                }
            }
        }
        tokio::time::sleep(RETRY_INTERVAL).await;
    }
}

/// メンバー側 1 接続。ホスト側と同じく読みと書きを分ける。
#[allow(clippy::too_many_arguments)]
async fn member_session(
    stream: TcpStream,
    display_name: &Option<String>,
    device_id: &Option<String>,
    latest_ledger: &Arc<Mutex<Option<ReceivedDistribution>>>,
    host_ip: Ipv4Addr,
    rtt: &RttMap,
    removed: &Arc<AtomicBool>,
    link: &Arc<MemberLink>,
    memo_cache: &Arc<crate::memoshare::MemberMemoCache>,
) -> anyhow::Result<()> {
    let (read_half, write_half) = stream.into_split();
    let (tx, rx) = mpsc::unbounded_channel::<ControlMessage>();
    tx.send(ControlMessage::Hello {
        version: PROTO_VERSION,
        name: display_name.clone(),
        app_version: Some(env!("CARGO_PKG_VERSION").to_string()),
        capabilities: peercove_core::proto::current_capabilities(),
        device_id: device_id.clone(),
        platform: Some(std::env::consts::OS.to_string()),
    })
    .expect("受信側はこの後 spawn する");
    // supervisor が鍵ローテーション依頼を差し込めるようにする(ADR-0020)
    link.attach(tx.clone());
    rtt.lock().unwrap().connected(host_ip);

    let ping: SharedPing = Default::default();
    let mut writer = tokio::spawn(member_writer(
        write_half,
        rx,
        Arc::clone(&ping),
        host_ip,
        Arc::clone(rtt),
    ));
    let mut read_task = tokio::spawn(member_reader(
        line_reader(read_half),
        host_ip,
        tx,
        ping,
        Arc::clone(rtt),
        Arc::clone(latest_ledger),
        Arc::clone(removed),
        Arc::clone(link),
        Arc::clone(memo_cache),
    ));
    // 共有メモの初回同期(M5 F-2)。応答は member_reader が pending へ流す
    let sync_task = tokio::spawn(memo_sync(Arc::clone(link), Arc::clone(memo_cache)));

    // biased: 削除通知(Removed)を検知する読み側が「意味のある終了」なので先に見る。
    // (読み側が終わると Outbox が落ちて書き側も即 ready になり、順序が非決定になる)
    let result = tokio::select! {
        biased;
        joined = &mut read_task => { writer.abort(); joined }
        joined = &mut writer => { read_task.abort(); joined }
    };
    sync_task.abort();
    result.unwrap_or_else(|e| Err(anyhow::anyhow!("制御タスクが異常終了しました: {e}")))
}

/// 接続時の共有メモ同期(M5 F-2)。一覧を取り、リビジョンが進んでいる・
/// 手元に無いメモだけ本文を取り直す。ホストが旧バージョンなら応答が無いまま
/// タイムアウトし、「ホスト未対応」のままにする(キャッシュは温存)。
async fn memo_sync(link: Arc<MemberLink>, cache: Arc<crate::memoshare::MemberMemoCache>) {
    use peercove_core::memo::{SharedMemoOp, SharedMemoQuery, SharedMemoReply};
    use peercove_core::schedule::{ScheduleOp, ScheduleReply};
    use peercove_core::sheet::{SheetOp, SheetReply};
    const SYNC_TIMEOUT: Duration = Duration::from_secs(20);
    // 接続時に全シートのセルまで取りに行くのは重いため、最初の数枚だけ
    // 即時同期し、残りは各シートを開いたときの Cells 取得に任せる
    // (実装簡素化の判断、ADR-0054 の作業指示書どおり)
    const SHEET_SYNC_LIMIT: usize = 10;
    let Some(rx) = link.request_memo(SharedMemoOp::List {
        query: SharedMemoQuery::default(),
    }) else {
        return;
    };
    let reply = match tokio::time::timeout(SYNC_TIMEOUT, rx).await {
        Ok(Ok(reply)) => reply,
        _ => {
            tracing::debug!("共有メモの同期応答がありません(ホスト未対応の可能性)");
            return;
        }
    };
    let SharedMemoReply::Memos { memos, folders, .. } = reply else {
        return;
    };
    let total = memos.len();
    let stale = match cache.sync_from_list(memos, folders).await {
        Ok(stale) => stale,
        Err(e) => {
            tracing::warn!("共有メモの同期に失敗しました: {e:#}");
            return;
        }
    };
    cache.set_supported(true);
    tracing::info!(
        "共有メモを同期しました({total} 件、取り直し {} 件)",
        stale.len()
    );
    for id in stale {
        let Some(rx) = link.request_memo(SharedMemoOp::Get { id }) else {
            return;
        };
        if let Ok(Ok(SharedMemoReply::Memo { memo })) = tokio::time::timeout(SYNC_TIMEOUT, rx).await
        {
            cache.upsert(memo).await;
        }
    }

    // 共有スケジュール表の初回同期(M6 G-1、ADR-0053)。メモの同期に相乗り。
    // ホストが未対応(旧バージョン)なら応答が無いままタイムアウトし、
    // 既存のキャッシュをそのまま温存する(既存の互換モデル)
    let Some(rx) = link.request_memo(SharedMemoOp::Schedule {
        schedule: ScheduleOp::List,
    }) else {
        return;
    };
    match tokio::time::timeout(SYNC_TIMEOUT, rx).await {
        Ok(Ok(SharedMemoReply::Schedule {
            reply: ScheduleReply::Events { events, .. },
        })) => {
            let total = events.len();
            match cache.schedule_sync_from_list(events).await {
                Ok(()) => tracing::info!("共有スケジュール表を同期しました({total} 件)"),
                Err(e) => tracing::warn!("共有スケジュール表の同期に失敗しました: {e:#}"),
            }
        }
        _ => tracing::debug!("共有スケジュール表の同期応答がありません(ホスト未対応の可能性)"),
    }

    // 共有シートの初回同期(M6 G-2、ADR-0054)。メモ・スケジュールの同期に
    // 相乗り。ホストが未対応(旧バージョン)なら応答が無いままタイムアウトし、
    // 既存のキャッシュをそのまま温存する(既存の互換モデル)
    let Some(rx) = link.request_memo(SharedMemoOp::Sheet {
        sheet: SheetOp::List,
    }) else {
        return;
    };
    let sheets = match tokio::time::timeout(SYNC_TIMEOUT, rx).await {
        Ok(Ok(SharedMemoReply::Sheet {
            reply: SheetReply::Sheets { sheets, .. },
        })) => sheets,
        _ => {
            tracing::debug!("共有シートの同期応答がありません(ホスト未対応の可能性)");
            return;
        }
    };
    let total = sheets.len();
    match cache.sheet_sync_from_list(sheets.clone()).await {
        Ok(()) => tracing::info!("共有シートを同期しました({total} 枚)"),
        Err(e) => {
            tracing::warn!("共有シートの同期に失敗しました: {e:#}");
            return;
        }
    }
    for sheet in sheets.into_iter().take(SHEET_SYNC_LIMIT) {
        let Some(rx) = link.request_memo(SharedMemoOp::Sheet {
            sheet: SheetOp::Cells {
                sheet_id: sheet.id.clone(),
            },
        }) else {
            return;
        };
        if let Ok(Ok(SharedMemoReply::Sheet {
            reply: SheetReply::CellsData {
                sheet_id, cells, ..
            },
        })) = tokio::time::timeout(SYNC_TIMEOUT, rx).await
        {
            if let Err(e) = cache.sheet_sync_cells(sheet_id, cells).await {
                tracing::warn!("共有シートのセル同期に失敗しました: {e:#}");
            }
        }
    }
}

/// 書き側。キューされたメッセージ(Hello / Pong)と定期 ping を送る。
async fn member_writer(
    mut write_half: tokio::net::tcp::OwnedWriteHalf,
    mut rx: mpsc::UnboundedReceiver<ControlMessage>,
    ping: SharedPing,
    host_ip: Ipv4Addr,
    rtt: RttMap,
) -> anyhow::Result<()> {
    let mut ping_tick = tokio::time::interval(PING_INTERVAL);
    loop {
        tokio::select! {
            queued = rx.recv() => {
                match queued {
                    Some(message) => write_half.write_all(encode_line(&message).as_bytes()).await?,
                    None => return Ok(()), // 読み側が終了
                }
            }
            _ = ping_tick.tick() => {
                let message = ping.lock().unwrap().issue();
                rtt.lock().unwrap().issued(host_ip);
                write_half.write_all(encode_line(&message).as_bytes()).await?;
            }
        }
    }
}

/// 読み側。`select!` を使わない素直なループ(read_line のキャンセル安全性のため)。
#[allow(clippy::too_many_arguments)]
async fn member_reader(
    mut reader: LineReader,
    host_ip: Ipv4Addr,
    out: Outbox,
    ping: SharedPing,
    rtt: RttMap,
    latest_ledger: Arc<Mutex<Option<ReceivedDistribution>>>,
    removed: Arc<AtomicBool>,
    link: Arc<MemberLink>,
    memo_cache: Arc<crate::memoshare::MemberMemoCache>,
) -> anyhow::Result<()> {
    let mut line = String::new();
    let mut last_digest: Option<LedgerDigest> = None;
    loop {
        if read_line(&mut reader, &mut line).await?.is_none() {
            anyhow::bail!("ホストが切断しました");
        }
        match serde_json::from_str::<ControlMessage>(&line) {
            Ok(message) if handle_ping_pong(&message, host_ip, &out, &ping, &rtt) => {}
            Ok(ControlMessage::Ledger {
                members,
                dns_records,
                cname_records,
            }) => {
                // 意味のある変化があったときだけ INFO(エンドポイント鮮度の
                // 定期更新による再配布は debug に落とす — ADR-0019)
                let digest = ledger_digest(&members, &dns_records, &cname_records);
                if last_digest.as_ref() != Some(&digest) {
                    tracing::info!(
                        "台帳を受信しました({} 名、DNS レコード {} 件、CNAME {} 件)",
                        members.len(),
                        dns_records.len(),
                        cname_records.len()
                    );
                    last_digest = Some(digest);
                } else {
                    tracing::debug!("台帳を受信しました({} 名、内容の変化なし)", members.len());
                }
                *latest_ledger.lock().unwrap() = Some(ReceivedDistribution {
                    distribution: Distribution {
                        members,
                        dns_records,
                        cname_records,
                        deny: vec![], // deny はワイヤに載らない(blocked で受ける)
                        force_relay: vec![],
                    },
                    received_at: Instant::now(),
                });
            }
            Ok(ControlMessage::Removed { message }) => {
                tracing::warn!("ホストから削除されました: {message}");
                // 台帳はクリアし、削除フラグを立てる(UI が「削除された」と表示する)
                *latest_ledger.lock().unwrap() = None;
                removed.store(true, Ordering::Relaxed);
                anyhow::bail!("削除通知を受信");
            }
            Ok(ControlMessage::JoinRejected { message }) => {
                tracing::warn!("ホストが参加を拒否しました: {message}");
                *latest_ledger.lock().unwrap() = None;
                link.reject(message);
                anyhow::bail!("参加拒否通知を受信");
            }
            Ok(ControlMessage::RotateKeyResult { accepted, message }) => {
                // 鍵ローテーションの応答(ADR-0020)。ファイル操作と再起動は
                // supervisor が周期処理で行う(ここでは受け渡しのみ)
                link.put_rotate_result(accepted, message);
            }
            Ok(ControlMessage::SetDnsNameResult { accepted, message }) => {
                // DNS 名変更の応答(ADR-0021)。IPC ハンドラが待つ受け口へ渡す
                link.put_dns_result(accepted, message);
            }
            Ok(ControlMessage::SetDisplayNameResult { accepted, message }) => {
                // 表示名変更の応答(ADR-0027)。IPC ハンドラが待つ受け口へ渡す
                link.put_display_result(accepted, message);
            }
            Ok(ControlMessage::CreateInviteResult {
                accepted,
                message,
                token,
                name,
                expires_at,
            }) => {
                // 招待発行の応答(ADR-0048)。IPC ハンドラが待つ受け口へ渡す。
                // token は秘密情報なのでログへ出さない
                link.put_invite_result(InviteReply {
                    accepted,
                    message,
                    token,
                    name,
                    expires_at,
                });
            }
            Ok(ControlMessage::MemoResp { seq, reply }) => {
                // 共有メモの応答(M5 F-2)。IPC ハンドラ・同期タスクの受け口へ渡す
                link.put_memo_result(seq, reply);
            }
            Ok(ControlMessage::MemoEvent { event }) => {
                // リアルタイム配信(M5 F-2)。キャッシュへ反映し、世代を進める
                // (UI は status の shared_memo_seq で変化を検知する)
                memo_cache.set_supported(true);
                memo_cache.apply_event(event).await;
            }
            Ok(other) => tracing::debug!("未処理のメッセージ: {other:?}"),
            Err(e) => tracing::debug!("解析できないメッセージを無視: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use peercove_core::keys::PrivateKey;

    fn entry(name: &str, ip: &str, online: bool) -> LedgerEntry {
        LedgerEntry {
            name: Some(name.to_string()),
            dns_name: None,
            ip: ip.parse().unwrap(),
            public_key: PrivateKey::generate().public_key(),
            app_version: None,
            platform: None,
            capabilities: vec![],
            member_id: None,
            can_invite: false,
            invited_by: None,
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

    /// テスト用のメンバー側メモキャッシュ(一時パス。DB は使われるまで作られない)。
    fn test_memo_cache() -> Arc<crate::memoshare::MemberMemoCache> {
        crate::memoshare::MemberMemoCache::new(
            &std::env::temp_dir().join(format!("peercove-memo-test-{}.toml", std::process::id())),
        )
    }

    /// 鍵ローテーションを使わないテスト用の旧招待(v1/v2)設定。
    fn test_requests() -> Arc<HostRequests> {
        let path =
            std::env::temp_dir().join(format!("peercove-control-host-{}.toml", std::process::id()));
        let peer_key = PrivateKey::generate().public_key();
        std::fs::write(
            &path,
            format!(
                "[interface]\nprivate_key_file = \"host.key\"\naddress = \"127.0.0.10/24\"\n\n[[peer]]\npublic_key = \"{peer_key}\"\nallowed_ips = [\"127.0.0.1/32\"]\n"
            ),
        )
        .unwrap();
        Arc::new(HostRequests::new(
            path.clone(),
            PrivateKey::generate().public_key(),
            None,
            crate::memoshare::MemoService::new(&path),
        ))
    }

    #[test]
    fn authenticated_reconnect_replaces_stale_session_safely() {
        let connections: Connections = Default::default();
        let member_ip = "10.100.42.2".parse().unwrap();
        let (first_outbox, _first_inbox) = mpsc::unbounded_channel();
        let (first, mut first_close) = ConnectionInfo::new(first_outbox, None, vec![], None);
        let first_id = first.connection_id;
        register_connection(&connections, member_ip, first);

        let (second_outbox, _second_inbox) = mpsc::unbounded_channel();
        let (second, _second_close) = ConnectionInfo::new(second_outbox, None, vec![], None);
        let second_id = second.connection_id;
        register_connection(&connections, member_ip, second);

        assert_eq!(first_close.try_recv(), Ok(()), "旧セッションを終了させる");
        assert!(
            !unregister_connection(&connections, member_ip, first_id),
            "旧セッションの終了で新セッションを削除しない"
        );
        assert_eq!(connections.lock().unwrap().len(), 1);
        assert!(unregister_connection(&connections, member_ip, second_id));
        assert!(connections.lock().unwrap().is_empty());
    }

    /// 受信ログの INFO/debug 判定(ADR-0019): エンドポイントとその観測経過
    /// **だけ**の変化はダイジェスト一致(= debug)、メンバーの増減・オンライン・
    /// 遮断・DNS の変化は不一致(= INFO)。
    #[test]
    fn ledger_digest_ignores_endpoint_freshness_only() {
        let base = vec![entry("alice", "100.100.42.2", true)];
        let mut fresher = base.clone();
        fresher[0].endpoint = Some("203.0.113.9:51820".parse().unwrap());
        fresher[0].endpoint_age_secs = Some(60);
        assert_eq!(
            ledger_digest(&base, &[], &[]),
            ledger_digest(&fresher, &[], &[]),
            "エンドポイント鮮度だけの変化は意味のある変化ではない"
        );

        let mut offline = base.clone();
        offline[0].online = false;
        assert_ne!(
            ledger_digest(&base, &[], &[]),
            ledger_digest(&offline, &[], &[])
        );

        let mut blocked = base.clone();
        blocked[0].blocked = true;
        assert_ne!(
            ledger_digest(&base, &[], &[]),
            ledger_digest(&blocked, &[], &[])
        );

        let more = vec![base[0].clone(), entry("bob", "100.100.42.3", true)];
        assert_ne!(
            ledger_digest(&base, &[], &[]),
            ledger_digest(&more, &[], &[])
        );
    }

    /// host サーバー ↔ member クライアントを localhost で対向させ、
    /// 台帳の初回配布と変更配布がクライアントに反映されることを確認する。
    #[tokio::test]
    async fn ledger_is_distributed_and_updated() {
        // 127.0.0.1 で CONTROL_PORT が使用中でもテストが落ちないよう、
        // サーバー本体ではなく handle_member を直接テストする
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (ledger_tx, ledger_rx) = watch::channel(Distribution {
            members: vec![entry("alice", "100.100.42.2", true)],
            dns_records: vec![],
            cname_records: vec![],
            deny: vec![],
            force_relay: vec![],
        });
        let connections: Connections = Arc::new(Mutex::new(HashMap::new()));
        let host_rtt: RttMap = Default::default();

        let server_connections = Arc::clone(&connections);
        let server_rtt = Arc::clone(&host_rtt);
        tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.unwrap();
            let ip = match peer.ip() {
                IpAddr::V4(ip) => ip,
                _ => unreachable!(),
            };
            let _ = handle_member(
                stream,
                ip,
                ledger_rx,
                &server_connections,
                &server_rtt,
                test_requests(),
            )
            .await;
        });

        let latest: Arc<Mutex<Option<ReceivedDistribution>>> = Arc::new(Mutex::new(None));
        let client_latest = Arc::clone(&latest);
        let member_rtt: RttMap = Default::default();
        let client_rtt = Arc::clone(&member_rtt);
        let client = tokio::spawn(async move {
            let stream = TcpStream::connect(addr).await.unwrap();
            let host_ip: Ipv4Addr = "127.0.0.1".parse().unwrap();
            let _ = member_session(
                stream,
                &Some("alice".to_string()),
                &None,
                &client_latest,
                host_ip,
                &client_rtt,
                &Arc::new(AtomicBool::new(false)),
                &Arc::new(MemberLink::default()),
                &test_memo_cache(),
            )
            .await;
        });

        // 初回スナップショットを受信
        let members_len =
            |l: &Option<ReceivedDistribution>| l.as_ref().map(|r| r.distribution.members.len());
        wait_for(&latest, |l| members_len(l) == Some(1)).await;

        // 台帳 + DNS レコードの変更が配布される
        ledger_tx
            .send(Distribution {
                members: vec![
                    entry("alice", "100.100.42.2", true),
                    entry("bob", "100.100.42.3", false),
                ],
                dns_records: vec![DnsRecord {
                    name: "nas".to_string(),
                    ip: "100.100.42.50".parse().unwrap(),
                    scheme: Some("https".to_string()),
                    port: Some(8443),
                    health: None,
                }],
                cname_records: vec![CnameRecord {
                    name: "docs".to_string(),
                    target: "example.com".to_string(),
                    resolved_ip: None,
                    scheme: None,
                    port: None,
                    health: None,
                }],
                deny: vec![],
                force_relay: vec![],
            })
            .unwrap();
        wait_for(&latest, |l| members_len(l) == Some(2)).await;
        {
            let ledger = latest.lock().unwrap();
            let received = ledger.as_ref().unwrap();
            assert!(
                received.received_at.elapsed() < Duration::from_secs(10),
                "受信時刻が付く(エンドポイントの鮮度判定用)"
            );
            let dist = &received.distribution;
            assert_eq!(dist.members[1].name.as_deref(), Some("bob"));
            assert!(!dist.members[1].online);
            assert_eq!(dist.dns_records.len(), 1, "DNS レコードも一緒に届く");
            assert_eq!(dist.dns_records[0].name, "nas");
            assert_eq!(dist.dns_records[0].scheme.as_deref(), Some("https"));
            assert_eq!(dist.dns_records[0].port, Some(8443));
            assert_eq!(dist.cname_records.len(), 1, "CNAME も一緒に届く");
            assert_eq!(dist.cname_records[0].target, "example.com");
        }

        // 接続レジストリに登録されている(削除通知の配送口)
        assert_eq!(connections.lock().unwrap().len(), 1);

        // ping/pong は双方向。ホストはメンバーへの、メンバーはホストへの RTT を持つ
        let loopback: Ipv4Addr = "127.0.0.1".parse().unwrap();
        for (label, map) in [("host", &host_rtt), ("member", &member_rtt)] {
            let measured = wait_until(|| map.lock().unwrap().get(&loopback).copied()).await;
            assert!(
                measured.is_finite() && measured >= 0.0,
                "{label} 側の RTT が測れる: {measured}"
            );
        }
        client.abort();
    }

    /// ACL(ADR-0018): 台帳はメンバーごとにフィルタされ、遮断相手の
    /// エントリは endpoint が消え blocked が立つ。他のエントリは素通し。
    #[test]
    fn ledger_message_filters_blocked_entries_per_member() {
        let member_ip: Ipv4Addr = "100.100.42.5".parse().unwrap();
        let mut blocked_peer = entry("alice", "100.100.42.2", true);
        blocked_peer.endpoint = Some("203.0.113.9:51820".parse().unwrap());
        blocked_peer.endpoint_age_secs = Some(3);
        let mut open_peer = entry("bob", "100.100.42.3", true);
        open_peer.endpoint = Some("203.0.113.10:51820".parse().unwrap());
        let dist = Distribution {
            members: vec![blocked_peer, open_peer],
            dns_records: vec![],
            cname_records: vec![],
            deny: vec![("100.100.42.2".parse().unwrap(), member_ip)],
            force_relay: vec![],
        };

        let ControlMessage::Ledger { members, .. } = ledger_message_for(dist.clone(), member_ip)
        else {
            panic!("Ledger メッセージになる");
        };
        assert!(members[0].blocked, "遮断相手は blocked が立つ");
        assert_eq!(members[0].endpoint, None, "endpoint は配布されない");
        assert_eq!(members[0].endpoint_age_secs, None);
        assert!(!members[1].blocked, "無関係の相手はそのまま");
        assert!(members[1].endpoint.is_some());

        // 別のメンバー(組に含まれない)にはフィルタがかからない
        let other: Ipv4Addr = "100.100.42.9".parse().unwrap();
        let ControlMessage::Ledger { members, .. } = ledger_message_for(dist, other) else {
            panic!("Ledger メッセージになる");
        };
        assert!(!members[0].blocked);
        assert!(members[0].endpoint.is_some());
    }

    #[test]
    fn ledger_message_hides_approval_pending_members_and_dns() {
        let mut pending = entry("pending", "100.100.42.2", true);
        pending.invite_status = Some("awaiting_approval".to_string());
        pending.dns_name = Some("pending".to_string());
        let dist = Distribution {
            members: vec![pending, entry("bob", "100.100.42.3", true)],
            dns_records: vec![DnsRecord {
                name: "pending-service".to_string(),
                ip: "100.100.42.2".parse().unwrap(),
                scheme: None,
                port: None,
                health: None,
            }],
            cname_records: vec![],
            deny: vec![],
            force_relay: vec![],
        };
        let ControlMessage::Ledger {
            members,
            dns_records,
            ..
        } = ledger_message_for(dist, "100.100.42.3".parse().unwrap())
        else {
            panic!("Ledger メッセージになる");
        };
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].ip, "100.100.42.3".parse::<Ipv4Addr>().unwrap());
        assert!(dns_records.is_empty());
    }

    /// Removed を送るとメンバー側セッションが終了し、台帳がクリアされる。
    #[tokio::test]
    async fn removed_notification_ends_session() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (_ledger_tx, ledger_rx) = watch::channel(Distribution::default());
        let connections: Connections = Arc::new(Mutex::new(HashMap::new()));

        let server_connections = Arc::clone(&connections);
        let server_rtt: RttMap = Default::default();
        let server = tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.unwrap();
            let ip = match peer.ip() {
                IpAddr::V4(ip) => ip,
                _ => unreachable!(),
            };
            handle_member(
                stream,
                ip,
                ledger_rx,
                &server_connections,
                &server_rtt,
                test_requests(),
            )
            .await
        });

        let latest: Arc<Mutex<Option<ReceivedDistribution>>> = Arc::new(Mutex::new(None));
        let client_latest = Arc::clone(&latest);
        let client_rtt: RttMap = Default::default();
        let client_removed = Arc::new(AtomicBool::new(false));
        let removed_flag = Arc::clone(&client_removed);
        let client = tokio::spawn(async move {
            let stream = TcpStream::connect(addr).await.unwrap();
            let host_ip: Ipv4Addr = "127.0.0.1".parse().unwrap();
            member_session(
                stream,
                &None,
                &None,
                &client_latest,
                host_ip,
                &client_rtt,
                &removed_flag,
                &Arc::new(MemberLink::default()),
                &test_memo_cache(),
            )
            .await
        });

        // 接続登録を待って Removed を送る
        let sender = loop {
            if let Some(tx) = connections.lock().unwrap().values().next().cloned() {
                break tx;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert!(
            sender.send(ControlMessage::Removed {
                message: "テスト削除".to_string(),
            }),
            "削除通知を送れること"
        );

        let client_result = client.await.unwrap();
        assert!(client_result.is_err(), "削除通知でセッションが終わること");
        assert!(
            client_removed.load(Ordering::Relaxed),
            "削除フラグが立つこと(UI に「削除された」と出す信号)"
        );
        assert!(server.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn reused_v3_invite_returns_reason_and_stops_retry_state() {
        let dir = std::env::temp_dir().join(format!(
            "peercove-control-reused-invite-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let config_path = dir.join("host.toml");
        let peer_key = PrivateKey::generate().public_key();
        std::fs::write(
            &config_path,
            format!(
                "[interface]\nprivate_key_file = \"host.key\"\naddress = \"127.0.0.10/24\"\n\n[[peer]]\nname = \"alice\"\ninvite_id = \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\"\ninvite_issued_at = 1\ninvite_accepted_at = 2\ninvite_device_id = \"11111111111111111111111111111111\"\npublic_key = \"{peer_key}\"\nallowed_ips = [\"127.0.0.1/32\"]\n"
            ),
        )
        .unwrap();
        let requests = Arc::new(HostRequests::new(
            config_path.clone(),
            PrivateKey::generate().public_key(),
            None,
            crate::memoshare::MemoService::new(&config_path),
        ));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (_ledger_tx, ledger_rx) = watch::channel(Distribution::default());
        let connections: Connections = Default::default();
        let server_connections = Arc::clone(&connections);
        let server = tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.unwrap();
            let IpAddr::V4(ip) = peer.ip() else {
                unreachable!()
            };
            handle_member(
                stream,
                ip,
                ledger_rx,
                &server_connections,
                &Default::default(),
                requests,
            )
            .await
        });

        let link = Arc::new(MemberLink::default());
        let stream = TcpStream::connect(addr).await.unwrap();
        let result = member_session(
            stream,
            &Some("alice".to_string()),
            &Some("22222222222222222222222222222222".to_string()),
            &Arc::new(Mutex::new(None)),
            "127.0.0.1".parse().unwrap(),
            &Default::default(),
            &Arc::new(AtomicBool::new(false)),
            &link,
            &test_memo_cache(),
        )
        .await;

        assert!(result.is_err(), "参加拒否でセッションを終了する");
        let message = link.connection_error().expect("拒否理由が残る");
        assert!(message.contains("別の端末で既に使用"), "{message}");
        assert!(message.contains("新しい招待トークン"), "{message}");
        assert!(server.await.unwrap().is_err());
        assert!(connections.lock().unwrap().is_empty());
    }

    async fn wait_for<T>(value: &Arc<Mutex<Option<T>>>, predicate: impl Fn(&Option<T>) -> bool) {
        for _ in 0..100 {
            if predicate(&value.lock().unwrap()) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("条件が満たされませんでした");
    }

    async fn wait_until<T>(mut probe: impl FnMut() -> Option<T>) -> T {
        for _ in 0..100 {
            if let Some(value) = probe() {
                return value;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("条件が満たされませんでした");
    }

    /// 送受信するだけの対向 TCP を用意し、読み側のハーフを返す。
    async fn loopback_reader(payload: Vec<u8>) -> LineReader {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut stream = TcpStream::connect(addr).await.unwrap();
            stream.write_all(&payload).await.unwrap();
            stream.shutdown().await.unwrap();
        });
        let (stream, _) = listener.accept().await.unwrap();
        line_reader(stream.into_split().0)
    }

    /// `take` の上限は reader の**累計**。1 行ごとに戻さないと、ping/pong を
    /// 積み重ねた数時間後に EOF 扱いになって制御接続が落ちる(実際に踏んだ)。
    #[tokio::test]
    async fn read_line_resets_the_cumulative_take_limit() {
        // 上限(1 MiB — M5 F-2 で拡大)を累計で超える行数
        const LINES: u64 = 40_000;
        let mut payload = Vec::new();
        for nonce in 0..LINES {
            payload.extend_from_slice(encode_line(&ControlMessage::Ping { nonce }).as_bytes());
        }
        assert!(
            payload.len() as u64 > MAX_LINE_LEN,
            "累計が上限を超える量を送ること: {} バイト",
            payload.len()
        );

        let mut reader = loopback_reader(payload).await;
        let mut line = String::new();
        let mut read = 0u64;
        while read_line(&mut reader, &mut line).await.unwrap().is_some() {
            read += 1;
        }
        assert_eq!(read, LINES, "累計上限で打ち切られない");
    }

    /// 1 行が上限を超えたら、EOF ではなくエラーにする(メモリを守る)。
    #[tokio::test]
    async fn read_line_rejects_a_too_long_line() {
        let payload = vec![b'x'; MAX_LINE_LEN as usize + 1];
        let mut reader = loopback_reader(payload).await;
        let error = read_line(&mut reader, &mut String::new())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("バイトを超えました"), "{error}");
    }

    /// 行の途中で切れた場合は、長すぎる行と区別して報告する。
    #[tokio::test]
    async fn read_line_reports_truncated_line() {
        let mut reader = loopback_reader(b"{\"type\":\"pi".to_vec()).await;
        let error = read_line(&mut reader, &mut String::new())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("途中で切断"), "{error}");
    }

    /// 鍵ローテーションの往復(ADR-0020): メンバーが rotate_key を送ると
    /// ホストは host.toml の public_key を差し替えて accepted を返し、
    /// メンバー側は MemberLink 経由で応答を回収できる。
    #[tokio::test]
    async fn rotate_key_roundtrip_updates_host_config() {
        // ループバック接続では member_ip = 127.0.0.1 になるため、
        // その IP のピアを持つ host.toml を用意する
        let dir = std::env::temp_dir().join("peercove-control-rotate");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let config_path = dir.join("host.toml");
        let old_key = PrivateKey::generate().public_key();
        std::fs::write(
            &config_path,
            format!(
                "[interface]\nprivate_key_file = \"host.key\"\naddress = \"127.0.0.10/24\"\nlisten_port = 51820\n\n[[peer]]\nname = \"alice\"\npublic_key = \"{old_key}\"\nallowed_ips = [\"127.0.0.1/32\"]\n"
            ),
        )
        .unwrap();
        let host_public_key = PrivateKey::generate().public_key();
        let requests = Arc::new(HostRequests::new(
            config_path.clone(),
            host_public_key,
            None,
            crate::memoshare::MemoService::new(&config_path),
        ));

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (_ledger_tx, ledger_rx) = watch::channel(Distribution::default());
        let connections: Connections = Default::default();
        let server_connections = Arc::clone(&connections);
        let server_rtt: RttMap = Default::default();
        let server = tokio::spawn(async move {
            let (stream, peer) = listener.accept().await.unwrap();
            let ip = match peer.ip() {
                IpAddr::V4(ip) => ip,
                _ => unreachable!(),
            };
            let _ = handle_member(
                stream,
                ip,
                ledger_rx,
                &server_connections,
                &server_rtt,
                requests,
            )
            .await;
        });

        let link = Arc::new(MemberLink::default());
        let client_link = Arc::clone(&link);
        let client = tokio::spawn(async move {
            let stream = TcpStream::connect(addr).await.unwrap();
            let host_ip: Ipv4Addr = "127.0.0.1".parse().unwrap();
            let _ = member_session(
                stream,
                &Some("alice".to_string()),
                &None,
                &Arc::new(Mutex::new(None)),
                host_ip,
                &Default::default(),
                &Arc::new(AtomicBool::new(false)),
                &client_link,
                &test_memo_cache(),
            )
            .await;
        });

        // 接続(attach)を待って依頼を送る
        wait_until(|| link.session()).await;
        let new_key = PrivateKey::generate().public_key();
        assert!(link.send(ControlMessage::RotateKey {
            new_public_key: new_key,
        }));

        let (accepted, message) = wait_until(|| link.take_rotate_result()).await;
        assert!(accepted, "{message}");
        let updated = peercove_core::config::Config::load(&config_path).unwrap();
        assert_eq!(updated.peers[0].public_key, new_key);
        assert_eq!(updated.peers[0].name.as_deref(), Some("alice"));

        // 同じ依頼の再送も成功扱い(冪等)。衝突する鍵は拒否される
        assert!(link.send(ControlMessage::RotateKey {
            new_public_key: new_key,
        }));
        let (accepted, _) = wait_until(|| link.take_rotate_result()).await;
        assert!(accepted, "再送は冪等に成功する");
        assert!(link.send(ControlMessage::RotateKey {
            new_public_key: host_public_key,
        }));
        let (accepted, message) = wait_until(|| link.take_rotate_result()).await;
        assert!(!accepted, "ホスト鍵との衝突は拒否: {message}");

        client.abort();
        server.abort();
    }

    /// 未知の nonce の Pong では RTT を記録しない(遅れて届いた応答の混入防止)。
    #[test]
    fn ping_state_ignores_unknown_nonce() {
        let mut ping = PingState::default();
        assert_eq!(ping.observe(1), None, "未送信の nonce");
        let ControlMessage::Ping { nonce } = ping.issue() else {
            panic!("Ping を発行する");
        };
        assert_eq!(ping.observe(nonce + 1), None, "別の nonce");
        assert!(ping.observe(nonce).is_some());
        assert_eq!(ping.observe(nonce), None, "同じ Pong の二重計上はしない");
    }
}
