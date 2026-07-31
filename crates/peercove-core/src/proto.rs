//! トンネル内コントロールチャネルのプロトコル型(ADR-0005)。
//!
//! - トランスポート: ホスト仮想 IP の TCP [`CONTROL_PORT`]。トンネル内なので
//!   WG により暗号化・認証済み(接続元の仮想 IP がメンバーの身元になる)
//! - フレーミング: JSON Lines(1 行 = 1 メッセージ)
//! - 互換性: メッセージは `type` タグ付き。未知のフィールドは無視する側で許容し、
//!   互換性を壊す変更は [`PROTO_VERSION`] を上げる

use std::net::{Ipv4Addr, SocketAddr};

use serde::{Deserialize, Serialize};

use crate::keys::PublicKey;

/// コントロールチャネルの TCP ポート(ホスト仮想 IP 上)。
pub const CONTROL_PORT: u16 = 51821;
pub const PROTO_VERSION: u32 = 1;

/// 現行バイナリが対応する追加機能。文字列 ID は wire 上の安定名なので、
/// 一度公開した名前は変更・再利用しない(ADR-0029、M3-12)。
pub const CURRENT_CAPABILITIES: &[&str] = &[
    "acl_v1",
    "acl_v2",
    "chat",
    "direct",
    "dns_cname",
    "dns_health",
    "dns_service_url",
    "file_transfer",
    "key_rotation",
    // 共有メモ(M5 F-2、ADR-0049)。ホストはこの capability を名乗った接続に
    // だけメモイベントを配信する(旧クライアントの行長上限を守るため)
    "shared_memo",
    "subnet_router",
];

pub fn current_capabilities() -> Vec<String> {
    CURRENT_CAPABILITIES
        .iter()
        .map(|capability| (*capability).to_string())
        .collect()
}

/// 1 行 1 メッセージでやり取りする制御メッセージ。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlMessage {
    /// member → host: 接続直後に名乗る。
    Hello {
        version: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// 製品バージョン。旧メンバーは送らないので None。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app_version: Option<String>,
        /// 対応する追加機能。旧メンバーは空。
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        capabilities: Vec<String>,
        /// 招待 v3 の join 先端末で生成した ID。旧招待は None。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device_id: Option<String>,
        /// 端末の OS("windows" / "linux" / "android" 等)。ホスト UI の
        /// 端末バッジ表示に使う(E-E 11)。旧メンバーは送らないので None。
        /// 追加フィールドなので [`PROTO_VERSION`] は上げない。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        platform: Option<String>,
    },
    /// host → member: 台帳スナップショット(接続時と変更時に送る)。
    ///
    /// `dns_records` はカスタム DNS レコード(ADR-0011、M3-1)。後から足した
    /// フィールドなので、旧ホストからは届かず(default = 空)、旧メンバーは
    /// 未知フィールドとして無視する。[`PROTO_VERSION`] は上げない。
    Ledger {
        members: Vec<LedgerEntry>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        dns_records: Vec<crate::dns::DnsRecord>,
        /// カスタム CNAME レコード(ADR-0025、M3-17)。dns_records と同じ互換規則
        /// (旧バージョンとは互いに無視し合う)。[`PROTO_VERSION`] は上げない。
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        cname_records: Vec<crate::dns::CnameRecord>,
        /// ネットワーク表示名(ADR-0058)。ホストの `[interface].network_display_name`
        /// をそのまま載せる(未設定なら None)。メンバーは受信時に member.toml へ
        /// 永続化する(オフラインでも一覧に出るため)。追加フィールドなので
        /// [`PROTO_VERSION`] は上げない。旧メンバーは未知フィールドとして無視する。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        network_display_name: Option<String>,
    },
    /// host → member: あなたは削除された(以後トンネルは通らない)。
    Removed { message: String },
    /// host → member: Hello の招待認証を拒否した。
    ///
    /// 使用済み招待の別端末利用や期限切れなど、同じ設定での自動再試行では
    /// 回復しない理由を返す。受信側は再接続を停止し、利用者に表示する。
    JoinRejected { message: String },
    /// 双方向: RTT 計測(M2-G5)。受け取った側は同じ nonce で [`ControlMessage::Pong`] を返す。
    ///
    /// 追加メッセージなので [`PROTO_VERSION`] は上げない。ping を知らない旧実装は
    /// 解析に失敗して黙って無視するため、RTT が測れないだけで通信は壊れない。
    Ping { nonce: u64 },
    /// 双方向: [`ControlMessage::Ping`] への応答。
    Pong { nonce: u64 },
    /// member → host: デバイス鍵ローテーションの依頼(ADR-0020、M3-11)。
    /// メンバーが端末上で生成した新しい鍵ペアの**公開鍵だけ**を届ける
    /// (秘密鍵は端末から出さない)。送信元はトンネルの仮想 IP で特定済み。
    ///
    /// 追加メッセージなので [`PROTO_VERSION`] は上げない。旧ホストは解析に
    /// 失敗して黙って無視する(メンバーは応答が来ないまま現行鍵で動き続ける)。
    RotateKey { new_public_key: PublicKey },
    /// host → member: [`ControlMessage::RotateKey`] への応答。
    /// `accepted` なら host.toml へ永続化済み(適用は次回再読込 ≤5 秒)。
    /// 既に同じ鍵が登録済みの依頼も成功扱い(冪等)。
    RotateKeyResult { accepted: bool, message: String },
    /// member → host: 自分の DNS 名の変更依頼(ADR-0021、M3-14a)。
    /// ホストが検証(正規化・予約語・重複)して host.toml へ永続化し、
    /// 台帳の再配布で全員に伝わる。
    ///
    /// 追加メッセージなので [`PROTO_VERSION`] は上げない。旧ホストは解析に
    /// 失敗して黙って無視する(メンバー側はタイムアウトで未対応と案内する)。
    SetDnsName { name: String },
    /// host → member: [`ControlMessage::SetDnsName`] への応答。
    /// 拒否時は `message` に理由(重複・予約語など)が入る。
    SetDnsNameResult { accepted: bool, message: String },
    /// member → host: 自分の表示名の変更依頼(ADR-0027、M3-19)。
    /// 表示名は host.toml `[[peer]].name` が正本(ADR-0021)なので、本人からの
    /// 変更もホストが適用する(DNS 名変更 = [`ControlMessage::SetDnsName`] と
    /// 同じ「メンバー発 → ホストが host.toml へ適用」パターン)。
    ///
    /// 追加メッセージなので [`PROTO_VERSION`] は上げない。旧ホストは解析に
    /// 失敗して黙って無視する(メンバー側はタイムアウトで未対応と案内する)。
    SetDisplayName { name: String },
    /// host → member: [`ControlMessage::SetDisplayName`] への応答。
    /// 拒否時は `message` に理由(重複・空など)が入る。
    SetDisplayNameResult { accepted: bool, message: String },
    /// member → host: メンバー招待の発行依頼(ADR-0048)。ホストが権限
    /// (グローバルトグル + ピアの can_invite)を確認し、従来の invite を
    /// 実行してトークンを返す。有効期限はホストが上限(既定 7 日)を強制し、
    /// 無期限・PSK は指定できない。
    ///
    /// 追加メッセージなので [`PROTO_VERSION`] は上げない。旧ホストは解析に
    /// 失敗して黙って無視する(メンバー側はタイムアウトで未対応と案内する)。
    CreateInvite {
        /// 新メンバーの表示名(省略時はホストが自動命名)。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        /// 希望する有効期限(秒)。ホスト既定を超える指定は切り詰められる。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expires_in_secs: Option<u64>,
    },
    /// host → member: [`ControlMessage::CreateInvite`] への応答。
    /// **依頼が来た接続にだけ**返す。`token` はメンバー秘密鍵を含む
    /// 秘密情報(ADR-0008) — ログへ出さず、受信側も永続化しない。
    CreateInviteResult {
        accepted: bool,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token: Option<String>,
        /// 発行された新メンバーの表示名。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expires_at: Option<u64>,
    },
    /// member → host: 共有メモへの操作(M5 F-2、ADR-0049)。`seq` は
    /// メンバー側が応答([`ControlMessage::MemoResp`])と突き合わせる連番。
    /// 権限・編集ロック・リビジョン(CAS)の判定はすべてホスト正本で行う。
    ///
    /// 追加メッセージなので [`PROTO_VERSION`] は上げない。旧ホストは解析に
    /// 失敗して黙って無視する(メンバー側はタイムアウトで未対応と案内する)。
    MemoReq {
        seq: u64,
        op: crate::memo::SharedMemoOp,
    },
    /// host → member: [`ControlMessage::MemoReq`] への応答(依頼が来た接続にだけ)。
    MemoResp {
        seq: u64,
        reply: crate::memo::SharedMemoReply,
    },
    /// host → member: 共有メモのリアルタイム配信(変更・削除・ロック状態)。
    /// **閲覧権限があり、Hello で `shared_memo` capability を名乗った接続に
    /// だけ**送る(旧クライアントには一切流れない)。
    MemoEvent { event: crate::memo::SharedMemoEvent },
    /// member → host: DNS カスタムレコードのリモート管理依頼(ADR-0059 決定 3、
    /// J-1c)。発行者が admin(委任済み)かどうかは常にホストが host.toml 正本
    /// (`PeerConfig.admin`)で検証する(メンバー UI の出し分けは入り口の親切に
    /// すぎない)。配布(台帳の再配布)は既存の 5 秒周期に自動で乗るため、
    /// 適用そのものは `peercove_ops::dns` への委譲のみ。
    ///
    /// 追加メッセージなので [`PROTO_VERSION`] は上げない。旧ホストは解析に
    /// 失敗して黙って無視する(メンバー側はタイムアウトで未対応と案内する)。
    AdminDnsOp { op: AdminDnsRequest },
    /// host → member: [`ControlMessage::AdminDnsOp`] への応答(依頼が来た
    /// 接続にだけ返す)。
    AdminDnsResult { accepted: bool, message: String },
    /// member → host: 汎用 Admin 中継(admin 委任、ADR-0059 J-2 拡大)。
    /// メンバー管理・ネットワーク設定など、admin の付与・剥奪以外の操作を
    /// ホストへ中継する。発行者が admin かどうかは常にホストが host.toml 正本
    /// (`PeerConfig.admin`)で依頼ごとに検証する(メンバー UI の出し分けは
    /// 入り口の親切にすぎない)。変更系は既存の周期的な台帳再配布に自動で乗る。
    ///
    /// 追加メッセージなので [`PROTO_VERSION`] は上げない。旧ホストは解析に
    /// 失敗して黙って無視する(メンバー側はタイムアウトで未対応と案内する)。
    AdminOp { op: AdminRequest },
    /// host → member: [`ControlMessage::AdminOp`] への応答(依頼が来た接続に
    /// だけ返す)。`data` は読み取り系の依頼(例: [`AdminRequest::ReadNetworkSettings`])
    /// にだけ載る。
    AdminOpResult {
        accepted: bool,
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<AdminOpData>,
    },
}

/// [`ControlMessage::AdminOp`] の中身(ADR-0059 J-2 拡大)。admin の
/// 付与・剥奪(`set_peer_admin`)はここに**含めない**(権限の連鎖を防ぐため
/// 常にホスト UI 専用 — ADR-0059)。
///
/// additive なタグ付き enum なので、将来 variant を足しても旧バージョンとは
/// 互いに無視し合う。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum AdminRequest {
    /// メンバーの削除。対象にホスト自身・依頼者自身は指定できない
    /// (ホスト正本で拒否する — 事故防止)。
    RemoveMember { ip: Ipv4Addr },
    /// メンバーの表示名変更。
    RenameMember { ip: Ipv4Addr, name: String },
    /// 承認待ち端末の承認。
    ApproveMember { ip: Ipv4Addr },
    /// メンバー招待の発行許可(端末指名)の切り替え。
    SetCanInvite { ip: Ipv4Addr, allowed: bool },
    /// 他メンバーの DNS 名の変更。
    SetMemberDnsName { ip: Ipv4Addr, dns_name: String },
    /// ネットワーク設定(承認制・メンバー招待許可・表示名)の現在値を読む。
    /// 応答は [`AdminOpData::NetworkSettings`]。
    ReadNetworkSettings,
    /// 新規参加端末を承認まで隔離するか。
    SetRequireApproval { enabled: bool },
    /// メンバーによる招待発行を許可するか(グローバルトグル)。
    SetMemberInvites { enabled: bool },
    /// ネットワーク表示名(ADR-0058)。`None` で削除(既定表示に戻る)。
    SetNetworkDisplayName {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },
    /// ACL v2 ポリシー全体の現在値を読む(ADR-0059 J-2 拡大)。
    /// 応答は [`AdminOpData::AclPolicy`]。
    AclRead,
    /// ACL v2 ポリシー全体を差し替える(`peercove_ops::acl::write_policy` と同じ、
    /// 旧 deny pair は保存時に v2 へ移行される)。admin が自分自身への制限を
    /// 外せてしまう点は ADR-0059 で許容済み(最終判定は常にホスト側)。
    AclWrite { policy: AclPolicyWire },
    /// メンバーの広告サブネット(ADR-0014)を丸ごと差し替える。空で解除。
    /// CIDR のパースはホスト側で行い、不正な文字列はエラー応答になる。
    SetSubnets { ip: Ipv4Addr, subnets: Vec<String> },
    /// ホストの DNS サービス監視を間隔前でも再実行する(M3-14e と同じ処理を
    /// admin 中継で呼べるようにしたもの)。応答に `data` は載らない。
    DnsCheckNow,
    /// カスタム DNS レコードの完全な一覧を読む(id・ヘルス設定の現在値つき)。
    /// 応答は [`AdminOpData::DnsRecords`]。J-1c の既知の制限(admin のヘルス
    /// 編集が既定値から始まる / id 未付番レコードを操作できない)を解消する
    /// (`peercove_ops::dns::list_records` が id の遅延付番も行うため)。
    DnsListFull,
}

/// [`AdminRequest::AclWrite`] / [`AdminOpData::AclPolicy`] が共用する ACL v2
/// ポリシー全体のワイヤ表現。`peercove_ops::acl::PolicySettings` と同じ形
/// (default/groups/rules)だが、peercove-core は peercove-ops に依存できない
/// ためここへ最小のミラー型を置く(フィールドの型自体は peercove_core::acl の
/// 型をそのまま流用)。ops::acl::PolicySettings との変換は control.rs が行う。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AclPolicyWire {
    pub default: crate::acl::AclAction,
    #[serde(default)]
    pub groups: Vec<crate::acl::AclGroup>,
    #[serde(default)]
    pub rules: Vec<crate::acl::AclRule>,
}

/// [`AdminRequest::DnsListFull`] への応答内の 1 レコード。
/// `peercove_ops::dns::RecordDetail` を平坦化したもの(ターゲットは
/// ip / member / cname のいずれか — [`AdminDnsRequest::Add`] と同じ表現)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminDnsRecord {
    pub id: Option<String>,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip: Option<Ipv4Addr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<crate::config::MemberRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cname: Option<String>,
    /// 親メンバー(端末配下サブドメインのとき)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub under: Option<crate::config::MemberRef>,
    /// under を親ラベルへ解決したドット付き相対名(`web.alice` 等)。
    pub relative: String,
    pub fqdn: String,
    /// member ターゲットを現在の仮想 IP へ解決した値(参照切れは None)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_ip: Option<Ipv4Addr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub health_enabled: bool,
    pub health_kind: crate::dns::HealthCheckKind,
    pub health_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_expected_status: Option<u16>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub health_external: bool,
}

/// [`ControlMessage::AdminOpResult`] の `data`(読み取り系の応答のみ)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AdminOpData {
    /// [`AdminRequest::ReadNetworkSettings`] への応答。
    NetworkSettings {
        require_invite_approval: bool,
        member_invites: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        network_display_name: Option<String>,
    },
    /// [`AdminRequest::AclRead`] への応答。
    AclPolicy { policy: AclPolicyWire },
    /// [`AdminRequest::DnsListFull`] への応答。
    DnsRecords { records: Vec<AdminDnsRecord> },
}

/// [`ControlMessage::AdminDnsOp`] の中身(ADR-0059 決定 3、J-1c)。
/// `Add` のフィールドはホスト UI の Tauri コマンド `add_dns_record` と
/// `peercove_ops::dns::NewRecord` に合わせた表現(ip / member / cname の
/// いずれか 1 つ + under + scheme/port)。`Remove` / `SetHealth` は配布された
/// レコード `id`(ADR-0035)で対象を指定する — 配布データには `under` が
/// 乗らないため(name, under)の組では一意に特定できない。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum AdminDnsRequest {
    /// レコードの追加。ターゲットは ip / member / cname のいずれか 1 つ。
    Add {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ip: Option<Ipv4Addr>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        member: Option<crate::config::MemberRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cname: Option<String>,
        /// 親メンバー(端末配下サブドメインのとき)。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        under: Option<crate::config::MemberRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scheme: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<u16>,
    },
    /// レコードの削除(id 指定)。
    Remove { id: String },
    /// ヘルスチェック設定の変更(id 指定、`peercove_ops::dns::HealthSettings` と同じ表現)。
    SetHealth {
        id: String,
        enabled: bool,
        kind: crate::dns::HealthCheckKind,
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_status: Option<u16>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        external: bool,
    },
}

/// 台帳の 1 エントリ。ホスト自身も 1 エントリとして含める。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 確定済みの DNS 名(ADR-0021、M3-14a)。host.toml が正本。
    /// あればゾーン導出はこれをそのままラベルに使う。無ければ(旧ホスト・
    /// アップグレード前に登録されたピア)従来どおり `name` から導出する。
    /// 互換規則は endpoint と同じ(追加フィールド — 旧バージョンとは
    /// 互いに無視し合う)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns_name: Option<String>,
    pub ip: Ipv4Addr,
    pub public_key: PublicKey,
    /// この端末が名乗った製品バージョン。ホスト自身はローカル版を載せる。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    /// この端末の OS("windows" / "linux" / "android")。Hello 由来
    /// (E-E 11)。旧メンバー・オフライン中は None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    /// この端末が Hello で広告した追加機能。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    /// メンバーの同一性 ID(ホスト側 invite_id。2026-07-20 検証 FB)。
    /// 「削除 → 同名・同 IP で再追加」の検知に使う。公開鍵は鍵ローテーション
    /// (ADR-0020)でも変わるため同一性判定に使えない。旧版ホスト・
    /// 招待 v3 以前のメンバー・ホスト自身は None(その場合は検知しない)。
    /// 互換規則は endpoint と同じ(追加フィールド — 旧版とは互いに無視し合う)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_id: Option<String>,
    /// この端末がメンバー招待を発行できるか(ADR-0048)。ホストの
    /// グローバルトグルとピアの can_invite の両方が有効なときだけ true。
    /// 受信側は自分のエントリの値で「招待を発行」ボタンの表示を決める。
    /// 互換規則は endpoint と同じ(追加フィールド — 旧版とは互いに無視し合う)。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub can_invite: bool,
    /// このメンバーが管理を委任されているか(ADR-0059)。共有メモの全権限
    /// 管理(SetPerms・全グループ参照)を任せられる。最終判定は常にホスト側
    /// (ここは UI の出し分け用)。互換規則は endpoint と同じ(追加フィールド
    /// — 旧版とは互いに無視し合う)。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_admin: bool,
    /// このメンバーを招待した発行者の表示名(ADR-0048)。ホストが
    /// invited_by_id → 現在の表示名に解決して載せる(発行者が削除済みなら
    /// 発行時のスナップショット名)。ホスト発行・旧ピアは None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invited_by: Option<String>,
    /// ホスト正本の招待状態(M3-22)。旧版・ホスト自身は None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite_expires_at: Option<u64>,
    /// 最終ハンドシェイクが十分新しい(ホストから見て到達可能)か。
    pub online: bool,
    /// ホストかどうか。
    #[serde(default)]
    pub is_host: bool,
    /// ホストが観測したこのメンバーの外部エンドポイント(NAT 変換後の
    /// IP:port)。直接通信(ADR-0013)の宛先候補。ホストは新鮮なもの
    /// (オンライン判定の閾値内)だけを載せる。ホスト自身のエントリは None
    /// (メンバーは設定でホストの endpoint を持っている)。
    /// 後から足したフィールドなので旧バージョンとは互いに無視し合う
    /// ([`ControlMessage::Ledger`] の dns_records と同じ互換規則)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<SocketAddr>,
    /// `endpoint` をホストが観測してからの経過秒。マシン間の時計ずれを
    /// 避けるため絶対時刻ではなく相対値で運ぶ。受信側は
    /// 「この値 + 受信からの経過」で鮮度を判定する(ADR-0013 追加条件 1)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_age_secs: Option<u64>,
    /// このメンバーが広告する背後 LAN のサブネット(ADR-0014、M3-7)。
    /// ホスト設定([[peer]] の subnets)が正本。互換規則は endpoint と同じ
    /// (追加フィールド — 旧バージョンとは互いに無視し合う)。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subnets: Vec<ipnet::Ipv4Net>,
    /// **受信者とこのメンバーの間**の通信がホストの ACL で遮断されているか
    /// (ADR-0018、M3-10)。ホストが台帳をメンバーごとにフィルタして立てる
    /// (このとき endpoint も落とす)。互換規則は endpoint と同じ。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub blocked: bool,
    /// 細粒度ACLにより、この相手との直接通信を禁止してホスト中継へ固定する。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub force_relay: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acl_rule_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::PrivateKey;

    fn entry() -> LedgerEntry {
        LedgerEntry {
            name: Some("alice".to_string()),
            dns_name: None,
            ip: "100.100.42.2".parse().unwrap(),
            public_key: PrivateKey::generate().public_key(),
            app_version: None,
            platform: None,
            capabilities: vec![],
            member_id: None,
            can_invite: false,
            is_admin: false,
            invited_by: None,
            invite_status: None,
            invite_expires_at: None,
            online: true,
            is_host: false,
            endpoint: Some("203.0.113.5:51820".parse().unwrap()),
            endpoint_age_secs: Some(3),
            subnets: vec![],
            blocked: false,
            force_relay: false,
            acl_rule_id: None,
        }
    }

    /// subnets は追加フィールド(ADR-0014)。空ならワイヤに現れず、
    /// 旧バージョンの台帳(フィールドなし)も読める。
    #[test]
    fn ledger_entry_subnets_are_optional_on_the_wire() {
        let mut e = entry();
        assert!(!serde_json::to_string(&e).unwrap().contains("subnets"));
        e.subnets = vec!["192.168.10.0/24".parse().unwrap()];
        let json = serde_json::to_string(&e).unwrap();
        let back: LedgerEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.subnets, e.subnets);
    }

    #[test]
    fn control_message_json_roundtrip() {
        let messages = vec![
            ControlMessage::Hello {
                version: PROTO_VERSION,
                name: Some("alice".to_string()),
                app_version: Some("0.1.0".to_string()),
                capabilities: vec!["chat".to_string()],
                device_id: None,
                platform: Some("windows".to_string()),
            },
            ControlMessage::Ledger {
                members: vec![entry()],
                dns_records: vec![crate::dns::DnsRecord {
                    id: Some("svc-1".to_string()),
                    name: "nas".to_string(),
                    ip: "100.100.42.50".parse().unwrap(),
                    scheme: None,
                    port: None,
                    health: None,
                }],
                cname_records: vec![crate::dns::CnameRecord {
                    id: Some("svc-2".to_string()),
                    name: "docs".to_string(),
                    target: "example.com".to_string(),
                    resolved_ip: None,
                    scheme: None,
                    port: None,
                    health: None,
                }],
                network_display_name: Some("うちの LAN".to_string()),
            },
            ControlMessage::Removed {
                message: "ホストにより削除されました".to_string(),
            },
            ControlMessage::JoinRejected {
                message: "この招待は使用済みです".to_string(),
            },
            ControlMessage::Ping { nonce: 7 },
            ControlMessage::Pong { nonce: 7 },
            ControlMessage::RotateKey {
                new_public_key: PrivateKey::generate().public_key(),
            },
            ControlMessage::RotateKeyResult {
                accepted: false,
                message: "既に使われています".to_string(),
            },
            ControlMessage::SetDnsName {
                name: "yamada-dev".to_string(),
            },
            ControlMessage::SetDnsNameResult {
                accepted: true,
                message: "更新しました".to_string(),
            },
            ControlMessage::SetDisplayName {
                name: "山田のノート".to_string(),
            },
            ControlMessage::SetDisplayNameResult {
                accepted: true,
                message: "更新しました".to_string(),
            },
            ControlMessage::AdminDnsOp {
                op: AdminDnsRequest::Add {
                    name: "web".to_string(),
                    ip: Some("10.1.0.50".parse().unwrap()),
                    member: None,
                    cname: None,
                    under: Some(crate::config::MemberRef::Host),
                    scheme: Some("http".to_string()),
                    port: Some(8080),
                },
            },
            ControlMessage::AdminDnsOp {
                op: AdminDnsRequest::Remove {
                    id: "svc-1".to_string(),
                },
            },
            ControlMessage::AdminDnsOp {
                op: AdminDnsRequest::SetHealth {
                    id: "svc-1".to_string(),
                    enabled: true,
                    kind: crate::dns::HealthCheckKind::HttpHead,
                    path: "/ready".to_string(),
                    expected_status: Some(204),
                    external: false,
                },
            },
            ControlMessage::AdminDnsResult {
                accepted: false,
                message: "管理が委任されていません".to_string(),
            },
            ControlMessage::AdminOp {
                op: AdminRequest::RemoveMember {
                    ip: "100.100.42.2".parse().unwrap(),
                },
            },
            ControlMessage::AdminOp {
                op: AdminRequest::RenameMember {
                    ip: "100.100.42.2".parse().unwrap(),
                    name: "アリス".to_string(),
                },
            },
            ControlMessage::AdminOp {
                op: AdminRequest::ApproveMember {
                    ip: "100.100.42.2".parse().unwrap(),
                },
            },
            ControlMessage::AdminOp {
                op: AdminRequest::SetCanInvite {
                    ip: "100.100.42.2".parse().unwrap(),
                    allowed: true,
                },
            },
            ControlMessage::AdminOp {
                op: AdminRequest::SetMemberDnsName {
                    ip: "100.100.42.2".parse().unwrap(),
                    dns_name: "alice".to_string(),
                },
            },
            ControlMessage::AdminOp {
                op: AdminRequest::ReadNetworkSettings,
            },
            ControlMessage::AdminOp {
                op: AdminRequest::SetRequireApproval { enabled: true },
            },
            ControlMessage::AdminOp {
                op: AdminRequest::SetMemberInvites { enabled: false },
            },
            ControlMessage::AdminOp {
                op: AdminRequest::SetNetworkDisplayName {
                    name: Some("うちの LAN".to_string()),
                },
            },
            ControlMessage::AdminOp {
                op: AdminRequest::SetNetworkDisplayName { name: None },
            },
            ControlMessage::AdminOp {
                op: AdminRequest::AclRead,
            },
            ControlMessage::AdminOp {
                op: AdminRequest::AclWrite {
                    policy: AclPolicyWire {
                        default: crate::acl::AclAction::Deny,
                        groups: vec![crate::acl::AclGroup {
                            id: "servers".to_string(),
                            members: vec![PrivateKey::generate().public_key()],
                        }],
                        rules: vec![crate::acl::AclRule {
                            id: "deny-servers".to_string(),
                            action: crate::acl::AclAction::Deny,
                            source: crate::acl::AclTarget::Any("any".to_string()),
                            destination: crate::acl::AclTarget::Group {
                                group: "servers".to_string(),
                            },
                            protocol: crate::acl::AclProtocol::Tcp,
                            ports: vec!["8080".to_string()],
                            enabled: true,
                        }],
                    },
                },
            },
            ControlMessage::AdminOp {
                op: AdminRequest::SetSubnets {
                    ip: "100.100.42.2".parse().unwrap(),
                    subnets: vec!["192.168.10.0/24".to_string()],
                },
            },
            ControlMessage::AdminOp {
                op: AdminRequest::DnsCheckNow,
            },
            ControlMessage::AdminOp {
                op: AdminRequest::DnsListFull,
            },
            ControlMessage::AdminOpResult {
                accepted: true,
                message: "削除しました".to_string(),
                data: None,
            },
            ControlMessage::AdminOpResult {
                accepted: true,
                message: "現在の設定を取得しました".to_string(),
                data: Some(AdminOpData::NetworkSettings {
                    require_invite_approval: true,
                    member_invites: false,
                    network_display_name: Some("うちの LAN".to_string()),
                }),
            },
            ControlMessage::AdminOpResult {
                accepted: true,
                message: "現在の ACL ポリシーを取得しました".to_string(),
                data: Some(AdminOpData::AclPolicy {
                    policy: AclPolicyWire {
                        default: crate::acl::AclAction::Allow,
                        groups: vec![],
                        rules: vec![],
                    },
                }),
            },
            ControlMessage::AdminOpResult {
                accepted: true,
                message: "DNS レコード一覧を取得しました".to_string(),
                data: Some(AdminOpData::DnsRecords {
                    records: vec![AdminDnsRecord {
                        id: Some("svc-1".to_string()),
                        name: "web".to_string(),
                        ip: Some("10.100.42.50".parse().unwrap()),
                        member: None,
                        cname: None,
                        under: Some(crate::config::MemberRef::Host),
                        relative: "web.host".to_string(),
                        fqdn: "web.host.home.peercove.internal".to_string(),
                        resolved_ip: Some("10.100.42.50".parse().unwrap()),
                        scheme: Some("http".to_string()),
                        port: Some(8080),
                        url: Some("http://web.host.home.peercove.internal:8080/".to_string()),
                        health_enabled: true,
                        health_kind: crate::dns::HealthCheckKind::HttpHead,
                        health_path: "/ready".to_string(),
                        health_expected_status: Some(204),
                        health_external: false,
                    }],
                }),
            },
        ];
        for message in messages {
            let json = serde_json::to_string(&message).unwrap();
            assert!(!json.contains('\n'), "JSON Lines 用に 1 行であること");
            let parsed: ControlMessage = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, message);
        }
    }

    /// ワイヤ表現(タグ名)を固定する。変えると旧バージョンと会話できなくなる。
    #[test]
    fn wire_format_is_stable() {
        let json = serde_json::to_string(&ControlMessage::Hello {
            version: 1,
            name: None,
            app_version: None,
            capabilities: vec![],
            device_id: None,
            platform: None,
        })
        .unwrap();
        assert_eq!(json, r#"{"type":"hello","version":1}"#);

        // 追加フィールドが無い Hello は旧版と完全に同じ。旧 Hello も読める。
        let old: ControlMessage = serde_json::from_str(r#"{"type":"hello","version":1}"#).unwrap();
        assert_eq!(
            old,
            ControlMessage::Hello {
                version: 1,
                name: None,
                app_version: None,
                capabilities: vec![],
                device_id: None,
                platform: None,
            }
        );

        // dns_records / cname_records / network_display_name が空・未設定なら
        // 旧バージョンとワイヤ表現が一致する(互換維持)
        let json = serde_json::to_string(&ControlMessage::Ledger {
            members: vec![],
            dns_records: vec![],
            cname_records: vec![],
            network_display_name: None,
        })
        .unwrap();
        assert_eq!(json, r#"{"type":"ledger","members":[]}"#);

        // サービス情報なしのレコードは M3-14c より前の JSON と完全に一致する。
        let json = serde_json::to_string(&crate::dns::DnsRecord {
            id: None,
            name: "nas".to_string(),
            ip: "100.100.42.50".parse().unwrap(),
            scheme: None,
            port: None,
            health: None,
        })
        .unwrap();
        assert_eq!(
            json, r#"{"name":"nas","ip":"100.100.42.50"}"#,
            "id が None のときは互換ワイヤ表現(id フィールドが乗らない)"
        );

        // 旧ホストからの台帳(dns_records なし)も読める
        let old: ControlMessage =
            serde_json::from_str(r#"{"type":"ledger","members":[]}"#).unwrap();
        assert_eq!(
            old,
            ControlMessage::Ledger {
                members: vec![],
                dns_records: vec![],
                cname_records: vec![],
                network_display_name: None,
            }
        );

        let json = serde_json::to_string(&ControlMessage::Ping { nonce: 1 }).unwrap();
        assert_eq!(json, r#"{"type":"ping","nonce":1}"#);
        let json = serde_json::to_string(&ControlMessage::Pong { nonce: 1 }).unwrap();
        assert_eq!(json, r#"{"type":"pong","nonce":1}"#);

        // 鍵ローテーション(ADR-0020、M3-11)。追加メッセージなので旧実装は
        // 解析に失敗して無視する(それで壊れないことは実装側の規約)
        let key = PrivateKey::generate().public_key();
        let json = serde_json::to_string(&ControlMessage::RotateKey {
            new_public_key: key,
        })
        .unwrap();
        assert_eq!(
            json,
            format!(r#"{{"type":"rotate_key","new_public_key":"{key}"}}"#)
        );
        let json = serde_json::to_string(&ControlMessage::RotateKeyResult {
            accepted: true,
            message: "更新しました".to_string(),
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"type":"rotate_key_result","accepted":true,"message":"更新しました"}"#
        );
    }

    /// DNS 名の変更依頼(ADR-0021、M3-14a)のワイヤ表現と、
    /// `LedgerEntry.dns_name` の互換規則(None ならワイヤに現れない)。
    #[test]
    fn set_dns_name_wire_format() {
        let json = serde_json::to_string(&ControlMessage::SetDnsName {
            name: "yamada-dev".to_string(),
        })
        .unwrap();
        assert_eq!(json, r#"{"type":"set_dns_name","name":"yamada-dev"}"#);
        let json = serde_json::to_string(&ControlMessage::SetDnsNameResult {
            accepted: false,
            message: "既に使われています".to_string(),
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"type":"set_dns_name_result","accepted":false,"message":"既に使われています"}"#
        );

        // 表示名の変更依頼(ADR-0027、M3-19)。同じ「メンバー発 → ホスト適用」パターン
        let json = serde_json::to_string(&ControlMessage::SetDisplayName {
            name: "山田のノート".to_string(),
        })
        .unwrap();
        assert_eq!(json, r#"{"type":"set_display_name","name":"山田のノート"}"#);
        let json = serde_json::to_string(&ControlMessage::SetDisplayNameResult {
            accepted: false,
            message: "既に使われています".to_string(),
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"type":"set_display_name_result","accepted":false,"message":"既に使われています"}"#
        );

        // dns_name が None ならワイヤに現れず、旧バージョンの台帳も読める
        let e = entry();
        let json = serde_json::to_string(&e).unwrap();
        assert!(!json.contains("dns_name"), "{json}");
        let parsed: LedgerEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.dns_name, None);

        let mut e = entry();
        e.dns_name = Some("alice-pc".to_string());
        let parsed: LedgerEntry =
            serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(parsed.dns_name.as_deref(), Some("alice-pc"));
    }

    /// blocked(ADR-0018、M3-10)の互換規則: false ならワイヤに現れず、
    /// 旧バージョンからのエントリ(フィールドなし)は false として読める。
    #[test]
    fn ledger_entry_blocked_is_optional_on_the_wire() {
        let e = entry();
        let json = serde_json::to_string(&e).unwrap();
        assert!(!json.contains("blocked"), "false なら旧表現と一致: {json}");
        let parsed: LedgerEntry = serde_json::from_str(&json).unwrap();
        assert!(!parsed.blocked);

        let mut e = entry();
        e.blocked = true;
        let parsed: LedgerEntry =
            serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert!(parsed.blocked);
    }

    #[test]
    fn version_and_capabilities_are_optional_on_the_wire() {
        let mut e = entry();
        let old = serde_json::to_string(&e).unwrap();
        assert!(!old.contains("app_version"));
        assert!(!old.contains("capabilities"));

        e.app_version = Some("0.2.0".to_string());
        e.capabilities = vec!["chat".to_string(), "dns_service_url".to_string()];
        let parsed: LedgerEntry =
            serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(parsed.app_version.as_deref(), Some("0.2.0"));
        assert_eq!(parsed.capabilities, e.capabilities);
    }

    /// エンドポイント(ADR-0013、M3-2)の互換規則:
    /// 無ければワイヤに現れず、旧ホストからのエントリ(フィールドなし)も読める。
    #[test]
    fn ledger_entry_endpoint_is_optional_on_the_wire() {
        let mut without = entry();
        without.endpoint = None;
        without.endpoint_age_secs = None;
        let json = serde_json::to_string(&without).unwrap();
        assert!(
            !json.contains("endpoint"),
            "None なら旧バージョンとワイヤ表現が一致する: {json}"
        );

        let old_wire = json; // 旧ホストが送る形と同じ
        let parsed: LedgerEntry = serde_json::from_str(&old_wire).unwrap();
        assert_eq!(parsed, without);

        let with = entry();
        let parsed: LedgerEntry =
            serde_json::from_str(&serde_json::to_string(&with).unwrap()).unwrap();
        assert_eq!(parsed.endpoint, Some("203.0.113.5:51820".parse().unwrap()));
        assert_eq!(parsed.endpoint_age_secs, Some(3));
    }
}
