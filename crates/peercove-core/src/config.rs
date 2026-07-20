//! TOML 設定ファイルの型と読み込み。
//!
//! 設定内の相対パス(鍵ファイル等)は設定ファイルのあるディレクトリ基準で解決する。

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use ipnet::Ipv4Net;
use serde::{Deserialize, Serialize};

use crate::keys::PublicKey;
use crate::{Error, Result};

pub const DEFAULT_MTU: u16 = 1420;
pub const DEFAULT_LISTEN_PORT: u16 = 51820;

/// OS ごとの既定インターフェース名。
pub const DEFAULT_IF_NAME: &str = "peercove0";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub interface: InterfaceConfig,
    #[serde(default, rename = "peer")]
    pub peers: Vec<PeerConfig>,
    /// カスタム DNS レコード(ADR-0011 §1b、ADR-0022 で拡張)。ホスト設定のみ
    /// 意味を持ち、ホストが配布時に IP へ解決してから台帳と一緒に配る。
    #[serde(default, rename = "dns_record", skip_serializing_if = "Vec::is_empty")]
    pub dns_records: Vec<DnsRecordConfig>,
    /// アクセス制御(ADR-0018、M3-10)。ホスト設定のみ意味を持つ。
    /// 注意: `deny_unknown_fields` のため、これを書いた設定は旧バージョンでは
    /// 読めない(明示エラーになる)。
    #[serde(default, skip_serializing_if = "AclConfig::is_empty")]
    pub acl: AclConfig,
}

/// アクセス制御の設定(ADR-0018、M3-10)。既定はすべて許可で、
/// `deny` に載せた仮想 IP の組(順不同)だけメンバー間通信を遮断する。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AclConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<(std::net::Ipv4Addr, std::net::Ipv4Addr)>,
    #[serde(default, skip_serializing_if = "is_default_acl_action")]
    pub default: crate::acl::AclAction,
    #[serde(default, rename = "group", skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<crate::acl::AclGroup>,
    #[serde(default, rename = "rule", skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<crate::acl::AclRule>,
}

impl AclConfig {
    pub fn is_empty(&self) -> bool {
        self.deny.is_empty()
            && self.default == crate::acl::AclAction::Allow
            && self.groups.is_empty()
            && self.rules.is_empty()
    }

    /// 順不同の組を正規化(小さい IP を先)して返す。重複は除去。
    pub fn normalized_deny(&self) -> Vec<(std::net::Ipv4Addr, std::net::Ipv4Addr)> {
        let mut pairs: Vec<_> = self
            .deny
            .iter()
            .map(|&(a, b)| if a <= b { (a, b) } else { (b, a) })
            .collect();
        pairs.sort_unstable();
        pairs.dedup();
        pairs
    }

    /// この組は遮断対象か(順不同)。
    pub fn is_denied(&self, x: std::net::Ipv4Addr, y: std::net::Ipv4Addr) -> bool {
        self.deny
            .iter()
            .any(|&(a, b)| (a == x && b == y) || (a == y && b == x))
    }
}

/// カスタム DNS レコードの設定表現(ADR-0022 / ADR-0023、M3-14b/c)。
///
/// ターゲットは `ip`(固定 IP、従来型)/ `member`(メンバー参照 = 配布時に
/// その時点の仮想 IP へ解決)の排他でどちらか必須。`under` を指定すると
/// 親メンバー配下のサブドメイン(`<name>.<親のDNSラベル>.<net>.…`)になる。
/// `ip` + `under` は LAN 機器レコードで、ip は親の広告サブネット内のみ許可。
/// 注意: `member` / `under` / `scheme` / `port` を書いた設定は旧バージョンでは読めない
/// (`deny_unknown_fields`。subnets / ACL / dns_name と同じ扱い)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DnsRecordConfig {
    /// ACL等から名前変更に影響されず参照する安定ID。新規レコードには自動付与する。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// 正規化済みラベル(単一。ドットは持たない — 階層は `under` で表す)
    pub name: String,
    /// ターゲット A: 固定 IP
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ip: Option<std::net::Ipv4Addr>,
    /// ターゲット B: メンバー参照(IP 自動追随)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<MemberRef>,
    /// ターゲット C: CNAME(別ドメインの別名。外部ドメイン可 — ADR-0025)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cname: Option<String>,
    /// 親メンバー(端末配下サブドメイン)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub under: Option<MemberRef>,
    /// URL コピー用の URI スキーム(ADR-0023)。DNS 応答自体は A レコードのまま。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<String>,
    /// URL コピー用のサービス待受ポート(ADR-0023)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// None は既定動作(A/member + scheme/port は有効、外部 CNAME は無効)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_check: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_kind: Option<crate::dns::HealthCheckKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_expect_status: Option<u16>,
    /// 外部 CNAME へ明示的に接続してよいか。既定 false。
    #[serde(default, skip_serializing_if = "is_false")]
    pub health_external: bool,
}

fn is_default_acl_action(value: &crate::acl::AclAction) -> bool {
    *value == crate::acl::AclAction::Allow
}

/// メンバー参照(ADR-0022)。ホスト自身は公開鍵が host.toml に無いため
/// 番兵文字列 `"host"`、メンバーは公開鍵 base64 で表す。
/// 鍵ローテーション(ADR-0020)時は ops::peers::rotate_peer_key が
/// `[[dns_record]]` 内の参照も併せて書き換える。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MemberRef {
    Host,
    Key(PublicKey),
}

impl MemberRef {
    /// 設定・IPC で使う文字列表現(`"host"` または公開鍵 base64)。
    pub fn to_config_string(&self) -> String {
        match self {
            MemberRef::Host => "host".to_string(),
            MemberRef::Key(key) => key.to_base64(),
        }
    }
}

impl std::str::FromStr for MemberRef {
    type Err = crate::Error;
    fn from_str(s: &str) -> Result<Self> {
        if s == "host" {
            return Ok(MemberRef::Host);
        }
        PublicKey::from_base64(s).map(MemberRef::Key)
    }
}

impl std::fmt::Display for MemberRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_config_string())
    }
}

impl Serialize for MemberRef {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_config_string())
    }
}

impl<'de> Deserialize<'de> for MemberRef {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(|_| {
            serde::de::Error::custom(
                "メンバー参照は \"host\" または公開鍵(base64)で指定してください",
            )
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InterfaceConfig {
    /// トンネルインターフェース名(省略時 `peercove0`)。
    #[serde(default = "default_if_name")]
    pub name: String,
    /// 所属ネットワーク名(ADR-0012)。正規化済みの DNS ラベル。
    /// 旧設定には無いフィールドで、省略時は [`crate::names::DEFAULT_NETWORK_NAME`]。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_name: Option<String>,
    /// 台帳・コントロールチャネルで使う自分の表示名(join で設定される)。
    /// ADR-0021 以降は表示専用(DNS 名は `dns_name` / `[[peer]].dns_name` が別に持つ)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// 招待 v3 の join 時に端末上で生成する識別子。同じトークンの二重利用防止用。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    /// (host のみ)新しく発行する招待を、ホスト管理者の承認まで隔離する。
    #[serde(default, skip_serializing_if = "is_false")]
    pub require_invite_approval: bool,
    /// (host のみ)メンバーによる招待発行(ADR-0048)を許可する(既定 true)。
    /// これは機能全体のトグルで、端末単位の許可は `[[peer]].can_invite`
    /// (既定 false)。両方が有効なメンバーだけ発行できる。
    /// 注意: `deny_unknown_fields` のため、false を書いた設定は旧バージョン
    /// では読めない(明示エラーになる)。
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub member_invites: bool,
    /// (host のみ)ホスト自身の DNS 名(ADR-0021、M3-14a)。正規化済みラベル。
    /// 省略時は従来どおり表示名から導出される(実質 `host`)。
    /// 注意: `deny_unknown_fields` のため、これを書いた設定は旧バージョンでは
    /// 読めない(明示エラーになる)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dns_name: Option<String>,
    pub private_key_file: PathBuf,
    /// 仮想 IP とサブネット(例: `100.100.42.2/24`)。
    pub address: Ipv4Net,
    /// UDP 待受ポート。ホストでは省略時 51820、メンバーでは省略時 OS 任せ。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub listen_port: Option<u16>,
    #[serde(default = "default_mtu")]
    pub mtu: u16,
    /// 受信するファイルサイズの上限(MB、ADR-0015 / M3-9)。0 で無制限。
    /// **受け取る側**の設定として効く(超える申し出は拒否する)。
    /// 注意: `deny_unknown_fields` のため、これを書いた設定は旧バージョンでは
    /// 読めない(明示エラーになる)。
    #[serde(default = "default_max_recv_file_mb")]
    pub max_recv_file_mb: u64,
    /// メンバー間直接通信(ADR-0013)を試すか(既定 true)。false なら
    /// このマシンは常にホスト経由(中継)で通信する。ADR-0013 追加条件 2:
    /// 将来 UI の設定画面から切り替えられるようにするためのフラグ。
    #[serde(default = "default_direct")]
    pub direct: bool,
    /// (member のみ)デバイス秘密鍵の出どころ(ADR-0020、M3-11)。
    /// **省略時は "token"**(招待トークン経由 = 既存設定はすべて該当)とみなし、
    /// デーモンが初回接続時に自動ローテーションを行う。完了時にデーモンが
    /// "self" へ書き換える(join はこのフィールドを書かない — 旧デーモンとの
    /// 互換維持。`deny_unknown_fields` のため、書かれた設定は旧バージョンでは
    /// 読めない)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_source: Option<KeySource>,
}

/// デバイス秘密鍵の出どころ(ADR-0020)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeySource {
    /// 招待トークンに同梱されていた鍵(ホストが生成 = 外部を経由した)。
    Token,
    /// この端末上で生成した鍵(ローテーション済み)。
    #[serde(rename = "self")]
    SelfGenerated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerConfig {
    /// 台帳用の表示名(invite で発行したメンバーに付く)。日本語・空白可。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// このピアの DNS 名(ADR-0021、M3-14a)。正規化済みラベル。invite が
    /// IP 割当と同時に確定し、以後 IP と独立に維持される(台帳経由で配布)。
    /// 省略時(アップグレード前に登録されたピア)は従来どおり表示名から
    /// 導出される。注意: `deny_unknown_fields` のため、これを書いた設定は
    /// 旧バージョンでは読めない(明示エラーになる)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dns_name: Option<String>,
    /// 招待 v3 のホスト側台帳。トークン本体や秘密鍵は保存しない(M3-22)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invite_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invite_issued_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invite_expires_at: Option<u64>,
    /// 初回の認証済み Hello を受けた時刻。設定の期限後も参加済み端末は維持する。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invite_accepted_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invite_device_id: Option<String>,
    /// 招待発行時に承認必須だったか。既存 peer へ後付けでは適用しない。
    #[serde(default, skip_serializing_if = "is_false")]
    pub invite_requires_approval: bool,
    /// ホスト管理者が承認した時刻。None の承認必須 peer は隔離する。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invite_approved_at: Option<u64>,
    /// この端末にメンバー招待の発行を許可する(ADR-0048)。既定 false。
    /// `[interface].member_invites`(既定 true)と両方が有効なときだけ
    /// 発行できる。注意: `deny_unknown_fields` のため、これを書いた設定は
    /// 旧バージョンでは読めない(明示エラーになる)。
    #[serde(default, skip_serializing_if = "is_false")]
    pub can_invite: bool,
    /// このピアを招待した発行メンバーの invite_id(= 台帳の member_id、
    /// ADR-0048)。ホスト自身が発行した招待では書かない。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invited_by_id: Option<String>,
    /// 発行時点の発行者表示名のスナップショット(発行者が削除された後の
    /// 表示用。現在名は台帳構築時に invited_by_id から解決する)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub invited_by_name: Option<String>,
    /// このピア(ホスト)の仮想 IP。メンバー側でコントロールチャネルの
    /// 接続先として使う(join が設定する)。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub control_host: Option<std::net::Ipv4Addr>,
    pub public_key: PublicKey,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<SocketAddr>,
    /// 予備のエンドポイント候補(招待トークンの複数候補、M4 E-C)。メンバー設定で
    /// join が書き、`endpoint` への接続が確立しないときのフォールバックに使う
    /// (現在はモバイルが利用。デスクトップのデーモンは先頭 endpoint のみ使用)。
    /// 注意: `deny_unknown_fields` のため、これを書いた設定は旧バージョンでは
    /// 読めない(明示エラーになる)。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub endpoint_fallbacks: Vec<SocketAddr>,
    pub allowed_ips: Vec<Ipv4Net>,
    /// このメンバーが広告する背後 LAN のサブネット(ADR-0014、M3-7)。
    /// ホスト設定が正本で、台帳経由で全メンバーへ配布される。
    /// 注意: `deny_unknown_fields` のため、これを書いた設定は旧バージョンでは
    /// 読めない(明示エラーになる)。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subnets: Vec<Ipv4Net>,
    /// 秒。NAT 維持のためメンバー→ホストでは 25 を推奨。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub persistent_keepalive: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preshared_key_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InviteState {
    Legacy,
    Pending,
    Joined,
    AwaitingApproval,
    Expired,
    ClockInvalid,
}

impl PeerConfig {
    pub fn invite_state(&self, now_unix_secs: u64) -> InviteState {
        if self.invite_accepted_at.is_some()
            && self.invite_requires_approval
            && self.invite_approved_at.is_none()
        {
            InviteState::AwaitingApproval
        } else if self.invite_accepted_at.is_some() {
            InviteState::Joined
        } else if self.invite_id.is_none() {
            InviteState::Legacy
        } else if now_unix_secs == 0 {
            InviteState::ClockInvalid
        } else if self
            .invite_expires_at
            .is_some_and(|expires| now_unix_secs >= expires)
        {
            InviteState::Expired
        } else {
            InviteState::Pending
        }
    }

    pub fn invite_allows_connection(&self, now_unix_secs: u64) -> bool {
        !matches!(
            self.invite_state(now_unix_secs),
            InviteState::Expired | InviteState::ClockInvalid
        )
    }

    pub fn invite_is_isolated(&self) -> bool {
        self.invite_requires_approval && self.invite_approved_at.is_none()
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn is_true(value: &bool) -> bool {
    *value
}

fn default_true() -> bool {
    true
}

fn default_mtu() -> u16 {
    DEFAULT_MTU
}

fn default_direct() -> bool {
    true
}

/// 受信ファイルサイズ上限の既定(100 MB、2026-07-11 依頼者指定)。
pub const DEFAULT_MAX_RECV_FILE_MB: u64 = 100;

fn default_max_recv_file_mb() -> u64 {
    DEFAULT_MAX_RECV_FILE_MB
}

fn default_if_name() -> String {
    DEFAULT_IF_NAME.to_string()
}

impl Config {
    /// 所属ネットワーク名。旧設定(フィールドなし)は既定名として扱う。
    pub fn network_name(&self) -> &str {
        self.interface
            .network_name
            .as_deref()
            .unwrap_or(crate::names::DEFAULT_NETWORK_NAME)
    }

    /// 設定ファイルを読み込み、検証し、相対パスを設定ファイル基準で解決する。
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let mut config: Config = toml::from_str(&text)?;
        let base_dir = path.parent().unwrap_or(Path::new("."));
        config.interface.private_key_file = resolve(base_dir, &config.interface.private_key_file);
        for peer in &mut config.peers {
            if let Some(psk) = &peer.preshared_key_file {
                peer.preshared_key_file = Some(resolve(base_dir, psk));
            }
        }
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        let invalid = |message: String| Err(Error::InvalidConfig(message));
        if self.interface.address.prefix_len() > 30 {
            return invalid(format!(
                "interface.address のプレフィックス長 /{} が長すぎます(/30 以下にしてください)",
                self.interface.address.prefix_len()
            ));
        }
        let ip = self.interface.address.addr();
        if ip == self.interface.address.network() || ip == self.interface.address.broadcast() {
            return invalid(format!(
                "interface.address {ip} はネットワーク/ブロードキャストアドレスです"
            ));
        }
        if self.interface.mtu < 576 {
            return invalid(format!(
                "mtu {} が小さすぎます(576 以上)",
                self.interface.mtu
            ));
        }
        let valid_id =
            |value: &str| value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit());
        if self
            .interface
            .device_id
            .as_deref()
            .is_some_and(|id| !valid_id(id))
        {
            return invalid("interface.device_id が不正です".to_string());
        }
        for (i, peer) in self.peers.iter().enumerate() {
            if peer.allowed_ips.is_empty() {
                return invalid(format!("peer[{i}] の allowed_ips が空です"));
            }
            let has_v3 = peer.invite_id.is_some() || peer.invite_issued_at.is_some();
            if has_v3 {
                let Some(id) = &peer.invite_id else {
                    return invalid(format!("peer[{i}] の invite_id がありません"));
                };
                if !valid_id(id) {
                    return invalid(format!("peer[{i}] の invite_id が不正です"));
                }
                let Some(issued) = peer.invite_issued_at else {
                    return invalid(format!("peer[{i}] の invite_issued_at がありません"));
                };
                if peer
                    .invite_expires_at
                    .is_some_and(|expires| expires <= issued)
                {
                    return invalid(format!("peer[{i}] の招待期限が発行時刻以前です"));
                }
                if peer
                    .invite_accepted_at
                    .is_some_and(|accepted| accepted < issued)
                {
                    return invalid(format!("peer[{i}] の参加時刻が発行時刻以前です"));
                }
                if peer
                    .invite_device_id
                    .as_deref()
                    .is_some_and(|id| !valid_id(id))
                {
                    return invalid(format!("peer[{i}] の invite_device_id が不正です"));
                }
                if peer.invite_device_id.is_some() != peer.invite_accepted_at.is_some() {
                    return invalid(format!("peer[{i}] の参加端末メタデータが不完全です"));
                }
                if peer.invite_approved_at.is_some() && !peer.invite_requires_approval {
                    return invalid(format!("peer[{i}] は承認不要ですが承認時刻があります"));
                }
                if peer.invite_approved_at.is_some() && peer.invite_accepted_at.is_none() {
                    return invalid(format!("peer[{i}] は参加前に承認されています"));
                }
            } else if peer.invite_expires_at.is_some()
                || peer.invite_accepted_at.is_some()
                || peer.invite_device_id.is_some()
                || peer.invite_requires_approval
                || peer.invite_approved_at.is_some()
            {
                return invalid(format!("peer[{i}] の招待メタデータが不完全です"));
            }
        }
        // 広告サブネット(ADR-0014)。仮想サブネットやピア間で重なると
        // 経路が奪い合いになるため、設定段階で拒否する
        let virtual_subnet = self.interface.address.trunc();
        let mut seen_subnets: Vec<(usize, Ipv4Net)> = Vec::new();
        for (i, peer) in self.peers.iter().enumerate() {
            for subnet in &peer.subnets {
                if subnet.prefix_len() < 8 {
                    return invalid(format!(
                        "peer[{i}] の subnet {subnet} が広すぎます(/8 以上にしてください)"
                    ));
                }
                if *subnet != subnet.trunc() {
                    return invalid(format!(
                        "peer[{i}] の subnet {subnet} はネットワークアドレスで指定してください({})",
                        subnet.trunc()
                    ));
                }
                if virtual_subnet.contains(subnet) || subnet.contains(&virtual_subnet) {
                    return invalid(format!(
                        "peer[{i}] の subnet {subnet} が仮想サブネット {virtual_subnet} と重なっています"
                    ));
                }
                if let Some((j, other)) = seen_subnets
                    .iter()
                    .find(|(_, other)| other.contains(subnet) || subnet.contains(other))
                {
                    return invalid(format!(
                        "peer[{i}] の subnet {subnet} が peer[{j}] の {other} と重なっています"
                    ));
                }
                seen_subnets.push((i, *subnet));
            }
        }
        let mut keys: Vec<_> = self.peers.iter().map(|p| p.public_key).collect();
        keys.sort_unstable_by_key(|k| *k.as_bytes());
        keys.dedup();
        if keys.len() != self.peers.len() {
            return invalid("同じ public_key のピアが重複しています".to_string());
        }
        if let Some(name) = &self.interface.network_name {
            if !crate::names::is_dns_label(name) {
                return invalid(format!(
                    "network_name \"{name}\" が不正です(小文字英数とハイフンのみ、63 文字以内)"
                ));
            }
        }
        // ACL(ADR-0018): ホスト⇔メンバーの遮断はコントロールチャネルが
        // 壊れるため拒否。存在しないメンバーの IP は許容(効果がないだけ)
        for (a, b) in &self.acl.deny {
            if a == b {
                return invalid(format!("acl.deny の組 [{a}, {b}] が同じ IP です"));
            }
            for ip in [a, b] {
                if *ip == self.interface.address.addr() {
                    return invalid(format!(
                        "acl.deny に自分(ホスト)の IP {ip} は指定できません\
                        (ホストとの通信は遮断できません)"
                    ));
                }
                if !virtual_subnet.contains(ip) {
                    return invalid(format!(
                        "acl.deny の IP {ip} が仮想サブネット {virtual_subnet} の外です"
                    ));
                }
            }
        }
        // カスタムレコード(ADR-0022): (name, under) で一意。ターゲットは
        // ip / member の排他必須。参照先メンバーの存在と LAN 機器
        // (ip + under)の広告サブネット内チェックもここで行う
        let member_exists = |member: &MemberRef| match member {
            MemberRef::Host => true,
            MemberRef::Key(key) => self.peers.iter().any(|p| p.public_key == *key),
        };
        let mut seen_records = std::collections::HashSet::new();
        let mut seen_record_ids = std::collections::HashSet::new();
        for record in &self.dns_records {
            // 相対名(サブドメイン)+ 先頭 * ワイルドカードを許す(ADR-0024)
            if !crate::names::is_custom_dns_name(&record.name) {
                return invalid(format!(
                    "dns_record \"{}\" が不正です(小文字英数とハイフン。ドットで区切り、先頭ラベルのみ * が使えます)",
                    record.name
                ));
            }
            if !seen_records.insert((record.name.as_str(), record.under)) {
                return invalid(format!("dns_record \"{}\" が重複しています", record.name));
            }
            if let Some(id) = &record.id {
                if !crate::names::is_dns_label(id) || !seen_record_ids.insert(id.as_str()) {
                    return invalid(format!("dns_record id \"{id}\" が不正または重複しています"));
                }
            }
            // ターゲットは ip / member / cname のちょうど 1 つ(ADR-0025)
            let targets = [
                record.ip.is_some(),
                record.member.is_some(),
                record.cname.is_some(),
            ]
            .iter()
            .filter(|&&set| set)
            .count();
            if targets == 0 {
                return invalid(format!(
                    "dns_record \"{}\" に ip / member / cname のいずれかを指定してください",
                    record.name
                ));
            }
            if targets > 1 {
                return invalid(format!(
                    "dns_record \"{}\" は ip / member / cname のどれか 1 つだけ指定できます",
                    record.name
                ));
            }
            if let Some(cname) = &record.cname {
                if crate::names::normalize_cname_target(cname).as_deref() != Some(cname.as_str()) {
                    return invalid(format!(
                        "dns_record \"{}\" の cname \"{}\" が不正です(ドメイン名を指定してください)",
                        record.name, cname
                    ));
                }
            }
            for reference in [&record.member, &record.under].into_iter().flatten() {
                if !member_exists(reference) {
                    return invalid(format!(
                        "dns_record \"{}\" の参照先メンバーが登録されていません",
                        record.name
                    ));
                }
            }
            // LAN 機器レコード(要望 14): ip は親メンバーの広告サブネット内のみ
            if let (Some(ip), Some(under)) = (&record.ip, &record.under) {
                let subnets: Vec<Ipv4Net> = match under {
                    MemberRef::Host => vec![], // ホストは広告サブネットを持たない
                    MemberRef::Key(key) => self
                        .peers
                        .iter()
                        .find(|p| p.public_key == *key)
                        .map(|p| p.subnets.clone())
                        .unwrap_or_default(),
                };
                if !subnets.iter().any(|subnet| subnet.contains(ip)) {
                    return invalid(format!(
                        "dns_record \"{}\" の IP {ip} が親メンバーの広告サブネットの範囲外です\
                        (LAN 機器は広告サブネット内の IP のみ登録できます)",
                        record.name
                    ));
                }
            }
            if let Some(scheme) = &record.scheme {
                if !crate::dns::is_service_scheme(scheme) {
                    return invalid(format!(
                        "dns_record \"{}\" の scheme \"{}\" が不正です\
                        (先頭は小文字英字、以降は小文字英数字と + . -、31 文字以内)",
                        record.name, scheme
                    ));
                }
            }
            if record.port == Some(0) {
                return invalid(format!(
                    "dns_record \"{}\" の port は 1〜65535 で指定してください",
                    record.name
                ));
            }
            let health_enabled = record.health_check.unwrap_or(
                record.cname.is_none() && record.scheme.is_some() && record.port.is_some(),
            );
            if health_enabled && (record.scheme.is_none() || record.port.is_none()) {
                return invalid(format!(
                    "dns_record \"{}\" のヘルスチェックには scheme と port の両方が必要です",
                    record.name
                ));
            }
            if record.health_external && record.cname.is_none() {
                return invalid(format!(
                    "dns_record \"{}\" の health_external は CNAME にだけ指定できます",
                    record.name
                ));
            }
            if record.health_kind == Some(crate::dns::HealthCheckKind::HttpHead) {
                if record.scheme.as_deref() != Some("http") {
                    return invalid(format!(
                        "dns_record \"{}\" の HTTP HEAD チェックは scheme = \"http\" の場合だけ使用できます",
                        record.name
                    ));
                }
                let path = record.health_path.as_deref().unwrap_or("/");
                if !path.starts_with('/') || path.len() > 256 || path.chars().any(char::is_control)
                {
                    return invalid(format!(
                        "dns_record \"{}\" の health_path は / で始まる 256 文字以内のパスにしてください",
                        record.name
                    ));
                }
            }
            if record
                .health_expect_status
                .is_some_and(|status| !(100..=599).contains(&status))
            {
                return invalid(format!(
                    "dns_record \"{}\" の health_expect_status は 100〜599 で指定してください",
                    record.name
                ));
            }
        }
        let mut group_ids = std::collections::HashSet::new();
        for group in &self.acl.groups {
            if !crate::names::is_dns_label(&group.id) || !group_ids.insert(group.id.as_str()) {
                return invalid(format!(
                    "acl.group id \"{}\" が不正または重複しています",
                    group.id
                ));
            }
        }
        let mut rule_ids = std::collections::HashSet::new();
        for rule in &self.acl.rules {
            if !crate::names::is_dns_label(&rule.id) || !rule_ids.insert(rule.id.as_str()) {
                return invalid(format!(
                    "acl.rule id \"{}\" が不正または重複しています",
                    rule.id
                ));
            }
        }
        if let Err(error) = crate::acl::AclPolicy::compile(self) {
            return invalid(error);
        }
        Ok(())
    }
}

fn resolve(base_dir: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// handoff 4.1 の member.toml 例(公開鍵は実在する 32 バイト値に置換)。
    const MEMBER_TOML: &str = r#"
[interface]
private_key_file = "member_a.key"
address = "100.100.42.2/24"
mtu = 1420

[[peer]]
public_key = "hSDwCYkwp1R0i33ctD73Wg2/Og0mOBr06uSpB6ipTmo="
endpoint = "203.0.113.5:51820"
allowed_ips = ["100.100.42.0/24"]
persistent_keepalive = 25
"#;

    fn parse(text: &str) -> Config {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn parses_handoff_member_example() {
        let config = parse(MEMBER_TOML);
        assert_eq!(config.interface.address.addr().to_string(), "100.100.42.2");
        assert_eq!(config.interface.address.prefix_len(), 24);
        assert_eq!(config.interface.mtu, 1420);
        assert_eq!(config.interface.name, DEFAULT_IF_NAME);
        assert_eq!(config.peers.len(), 1);
        let peer = &config.peers[0];
        assert_eq!(peer.endpoint.unwrap().to_string(), "203.0.113.5:51820");
        assert_eq!(peer.allowed_ips[0].to_string(), "100.100.42.0/24");
        assert_eq!(peer.persistent_keepalive, Some(25));
        assert!(peer.preshared_key_file.is_none());
        config.validate().unwrap();
    }

    #[test]
    fn mtu_defaults_to_1420_and_peers_default_to_empty() {
        let config = parse(
            r#"
[interface]
private_key_file = "host.key"
address = "100.100.42.1/24"
listen_port = 51820
"#,
        );
        assert_eq!(config.interface.mtu, DEFAULT_MTU);
        assert_eq!(config.interface.listen_port, Some(51820));
        assert!(config.peers.is_empty());
        config.validate().unwrap();
    }

    /// key_source(ADR-0020、M3-11): 省略時は None(= "token" 扱い)。
    /// "token" / "self" が読める。デーモンだけが "self" を書く。
    #[test]
    fn key_source_parses_and_defaults_to_none() {
        let config = parse(MEMBER_TOML);
        assert_eq!(config.interface.key_source, None, "既存設定にはない");

        let with = MEMBER_TOML.replace(
            "private_key_file = \"member_a.key\"",
            "private_key_file = \"member_a.key\"\nkey_source = \"self\"",
        );
        assert_eq!(
            parse(&with).interface.key_source,
            Some(KeySource::SelfGenerated)
        );
        let with = with.replace("key_source = \"self\"", "key_source = \"token\"");
        assert_eq!(parse(&with).interface.key_source, Some(KeySource::Token));
    }

    #[test]
    fn rejects_invalid_public_key() {
        let result: std::result::Result<Config, _> = toml::from_str(
            r#"
[interface]
private_key_file = "a.key"
address = "100.100.42.2/24"

[[peer]]
public_key = "short"
allowed_ips = ["100.100.42.0/24"]
"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn rejects_unknown_fields() {
        let result: std::result::Result<Config, _> = toml::from_str(
            r#"
[interface]
private_key_file = "a.key"
address = "100.100.42.2/24"
typo_field = 1
"#,
        );
        assert!(result.is_err());
    }

    #[test]
    fn validate_rejects_network_address() {
        let mut config = parse(MEMBER_TOML);
        config.interface.address = "100.100.42.0/24".parse().unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_checks_subnet_overlaps() {
        // 仮想サブネットと重なる広告は拒否、外の RFC1918 は許可(ADR-0014)
        let mut config = parse(MEMBER_TOML);
        config.peers[0].subnets = vec!["100.100.42.0/28".parse().unwrap()];
        assert!(config.validate().is_err());
        config.peers[0].subnets = vec!["192.168.10.0/24".parse().unwrap()];
        assert!(config.validate().is_ok());
        // ピア間の重複も拒否
        let mut other = config.peers[0].clone();
        other.public_key = crate::keys::PrivateKey::generate().public_key();
        other.subnets = vec!["192.168.10.128/25".parse().unwrap()];
        config.peers.push(other);
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_rejects_duplicate_peers() {
        let mut config = parse(MEMBER_TOML);
        config.peers.push(config.peers[0].clone());
        assert!(config.validate().is_err());
    }

    #[test]
    fn dns_records_parse_and_validate() {
        let config = parse(
            r#"
[interface]
private_key_file = "host.key"
address = "10.100.42.1/24"

[[dns_record]]
name = "nas"
ip = "10.100.42.50"
"#,
        );
        config.validate().unwrap();
        assert_eq!(config.dns_records.len(), 1);
        assert_eq!(config.dns_records[0].name, "nas");
        assert_eq!(
            config.dns_records[0].ip.unwrap().to_string(),
            "10.100.42.50"
        );
        assert_eq!(config.dns_records[0].member, None);
        assert_eq!(config.dns_records[0].under, None);
        assert_eq!(config.dns_records[0].scheme, None);
        assert_eq!(config.dns_records[0].port, None);

        // 不正ラベル・重複は弾く
        let mut bad = config.clone();
        bad.dns_records[0].name = "Bad Label".to_string();
        assert!(bad.validate().is_err());
        let mut dup = config.clone();
        dup.dns_records.push(dup.dns_records[0].clone());
        assert!(dup.validate().is_err());
    }

    /// 拡張レコード(ADR-0022): member / under の解析、排他必須、参照先の
    /// 存在確認、LAN 機器(ip + under)の広告サブネット内チェック。
    #[test]
    fn dns_records_member_targets_validate() {
        let peer_key = crate::keys::PrivateKey::generate().public_key();
        let config = parse(&format!(
            r#"
[interface]
private_key_file = "host.key"
address = "10.100.42.1/24"

[[peer]]
public_key = "{peer_key}"
allowed_ips = ["10.100.42.2/32"]
subnets = ["192.168.10.0/24"]

[[dns_record]]
name = "gamehost"
member = "{peer_key}"
scheme = "https"
port = 8443

[[dns_record]]
name = "web"
member = "host"
under = "host"

[[dns_record]]
name = "printer"
ip = "192.168.10.50"
under = "{peer_key}"
"#
        ));
        config.validate().unwrap();
        assert_eq!(config.dns_records[0].member, Some(MemberRef::Key(peer_key)));
        assert_eq!(config.dns_records[0].scheme.as_deref(), Some("https"));
        assert_eq!(config.dns_records[0].port, Some(8443));
        assert_eq!(config.dns_records[1].under, Some(MemberRef::Host));

        // ip と member の両方 / どちらも無しは弾く
        let mut bad = config.clone();
        bad.dns_records[0].ip = Some("10.100.42.9".parse().unwrap());
        assert!(bad.validate().is_err());
        let mut bad = config.clone();
        bad.dns_records[0].member = None;
        assert!(bad.validate().is_err());

        // 未登録メンバーへの参照は弾く
        let stranger = crate::keys::PrivateKey::generate().public_key();
        let mut bad = config.clone();
        bad.dns_records[0].member = Some(MemberRef::Key(stranger));
        assert!(bad.validate().is_err());

        // LAN 機器: 広告サブネット外の IP、ホスト配下の ip レコードは弾く
        let mut bad = config.clone();
        bad.dns_records[2].ip = Some("192.168.99.50".parse().unwrap());
        assert!(bad.validate().is_err());
        let mut bad = config.clone();
        bad.dns_records[2].under = Some(MemberRef::Host);
        assert!(bad.validate().is_err());

        // 親が違えば同名可、同じ親なら重複
        let mut ok = config.clone();
        ok.dns_records.push(DnsRecordConfig {
            id: None,
            name: "web".to_string(),
            ip: None,
            member: Some(MemberRef::Key(peer_key)),
            cname: None,
            under: Some(MemberRef::Key(peer_key)),
            scheme: None,
            port: None,
            health_check: None,
            health_kind: None,
            health_path: None,
            health_expect_status: None,
            health_external: false,
        });
        ok.validate().unwrap();
        let mut dup = ok.clone();
        dup.dns_records
            .push(dup.dns_records.last().unwrap().clone());
        assert!(dup.validate().is_err());

        // 不正なメンバー参照文字列は解析段階で弾く
        assert!(toml::from_str::<Config>(
            r#"
[interface]
private_key_file = "host.key"
address = "10.100.42.1/24"

[[dns_record]]
name = "x"
member = "not-a-key"
"#
        )
        .is_err());

        // scheme は正規化済みの URI スキーム、port は 1〜65535 のみ
        let mut bad = config.clone();
        bad.dns_records[0].scheme = Some("HTTP".to_string());
        assert!(bad.validate().is_err());
        let mut bad = config.clone();
        bad.dns_records[0].scheme = Some("1http".to_string());
        assert!(bad.validate().is_err());
        let mut bad = config.clone();
        bad.dns_records[0].scheme = Some("a".repeat(32));
        assert!(bad.validate().is_err());
        let mut bad = config.clone();
        bad.dns_records[0].port = Some(0);
        assert!(bad.validate().is_err());
    }

    #[test]
    fn acl_parses_normalizes_and_validates() {
        let config = parse(
            r#"
[interface]
private_key_file = "host.key"
address = "10.100.42.1/24"

[acl]
deny = [["10.100.42.3", "10.100.42.2"], ["10.100.42.2", "10.100.42.3"]]
"#,
        );
        config.validate().unwrap();
        // 正規化: 順不同 + 重複除去
        let a: std::net::Ipv4Addr = "10.100.42.2".parse().unwrap();
        let b: std::net::Ipv4Addr = "10.100.42.3".parse().unwrap();
        assert_eq!(config.acl.normalized_deny(), vec![(a, b)]);
        assert!(config.acl.is_denied(a, b));
        assert!(config.acl.is_denied(b, a), "順不同で判定される");
        assert!(!config.acl.is_denied(a, "10.100.42.9".parse().unwrap()));

        // ホスト自身を含む組は拒否
        let mut bad = config.clone();
        bad.acl.deny = vec![("10.100.42.1".parse().unwrap(), a)];
        assert!(bad.validate().is_err());
        // サブネット外は拒否
        let mut bad = config.clone();
        bad.acl.deny = vec![("192.168.1.2".parse().unwrap(), a)];
        assert!(bad.validate().is_err());
        // 同一 IP の組は拒否
        let mut bad = config.clone();
        bad.acl.deny = vec![(a, a)];
        assert!(bad.validate().is_err());
        // 存在しないメンバーの IP は許容(効果がないだけ)
        let mut ok = config.clone();
        ok.acl.deny = vec![(a, "10.100.42.99".parse().unwrap())];
        assert!(ok.validate().is_ok());
    }

    /// [acl] が無い旧設定はそのまま読め、空の ACL はシリアライズに現れない。
    #[test]
    fn acl_defaults_to_empty_and_stays_off_the_wire() {
        let config = parse(MEMBER_TOML);
        assert!(config.acl.is_empty());
        let text = toml::to_string(&config).unwrap();
        assert!(!text.contains("acl"), "空なら書き出されない: {text}");
    }

    #[test]
    fn network_name_defaults_and_validates() {
        let config = parse(MEMBER_TOML);
        assert_eq!(config.interface.network_name, None);
        assert_eq!(config.network_name(), crate::names::DEFAULT_NETWORK_NAME);

        let mut config = parse(MEMBER_TOML);
        config.interface.network_name = Some("my-game-lan".into());
        config.validate().unwrap();
        assert_eq!(config.network_name(), "my-game-lan");

        // 正規化されていない名前は弾く
        config.interface.network_name = Some("My LAN".into());
        assert!(config.validate().is_err());
    }

    #[test]
    fn load_resolves_relative_key_path() {
        let dir = std::env::temp_dir().join("peercove-core-test-config");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("member.toml");
        std::fs::write(&path, MEMBER_TOML).unwrap();
        let config = Config::load(&path).unwrap();
        assert_eq!(config.interface.private_key_file, dir.join("member_a.key"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
