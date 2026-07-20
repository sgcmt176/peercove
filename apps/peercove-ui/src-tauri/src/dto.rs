//! frontend へ渡す DTO。`apps/peercove-ui/src/ipc.ts` と対で保守すること。

use std::path::Path;

use peercove_core::ipc::{
    ChatMessageInfo, DaemonStatus, LogLine, PeerSummary, TunnelInfo, TunnelRole, IPC_VERSION,
};
use peercove_core::proto::LedgerEntry;
use peercove_core::quality::{
    QualityAvailability, QualityReport as CoreQualityReport, QualityRoute, QualitySample,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityReport {
    pub generated_at_unix_ms: u64,
    pub retention_days: u32,
    pub skipped_corrupt_lines: u32,
    pub samples: Vec<QualityPoint>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QualityPoint {
    pub window_start_unix_ms: u64,
    pub window_secs: u32,
    pub public_key: String,
    pub ip: String,
    pub name: Option<String>,
    pub availability: &'static str,
    pub rtt_latest_ms: Option<f64>,
    pub rtt_min_ms: Option<f64>,
    pub rtt_avg_ms: Option<f64>,
    pub rtt_p95_ms: Option<f64>,
    pub jitter_ms: Option<f64>,
    pub probes_sent: u32,
    pub probes_received: u32,
    pub loss_percent: Option<f64>,
    pub route: &'static str,
    pub route_switches: u32,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

impl From<CoreQualityReport> for QualityReport {
    fn from(report: CoreQualityReport) -> Self {
        Self {
            generated_at_unix_ms: report.generated_at_unix_ms,
            retention_days: report.retention_days,
            skipped_corrupt_lines: report.skipped_corrupt_lines,
            samples: report.samples.iter().map(QualityPoint::from).collect(),
        }
    }
}

impl From<&QualitySample> for QualityPoint {
    fn from(sample: &QualitySample) -> Self {
        Self {
            window_start_unix_ms: sample.window_start_unix_ms,
            window_secs: sample.window_secs,
            public_key: sample.public_key.clone(),
            ip: sample.ip.to_string(),
            name: sample.name.clone(),
            availability: match sample.availability {
                QualityAvailability::Connected => "connected",
                QualityAvailability::Disconnected => "disconnected",
                QualityAvailability::Unmeasured => "unmeasured",
            },
            rtt_latest_ms: sample.rtt_latest_ms,
            rtt_min_ms: sample.rtt_min_ms,
            rtt_avg_ms: sample.rtt_avg_ms,
            rtt_p95_ms: sample.rtt_p95_ms,
            jitter_ms: sample.jitter_ms,
            probes_sent: sample.probes_sent,
            probes_received: sample.probes_received,
            loss_percent: sample.loss_percent,
            route: match sample.route {
                QualityRoute::Direct => "direct",
                QualityRoute::Relay => "relay",
                QualityRoute::Trying => "trying",
            },
            route_switches: sample.route_switches,
            rx_bytes: sample.rx_bytes,
            tx_bytes: sample.tx_bytes,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    pub name: Option<String>,
    pub ip: String,
    pub public_key: String,
    pub app_version: Option<String>,
    pub capabilities: Vec<String>,
    pub invite_status: Option<String>,
    pub invite_expires_at: Option<u64>,
    pub online: bool,
    pub is_host: bool,
    /// このメンバーの DNS 名(M3-1)。台帳から決定的に導出される
    /// (`alice.game.peercove.internal` 等)。
    pub dns_name: Option<String>,
    /// このメンバーへの経路(ADR-0013、M3-4)。member ロールで他メンバーに
    /// 対してのみ "direct" | "trying" | "relay"。ホスト・自分・host ロールでは null。
    pub route: Option<&'static str>,
    /// この行が自分自身か(仮想 IP の一致で判定)。UI が「自分」と表示する。
    pub is_self: bool,
    /// このメンバーが広告する背後 LAN のサブネット(ADR-0014、M3-7)。
    pub subnets: Vec<String>,
    /// 自分とこのメンバーの間がホストの ACL で遮断されているか
    /// (ADR-0018、M3-10)。UI はバッジ表示 + チャット/ファイル送信の抑止に使う。
    pub blocked: bool,
    pub force_relay: bool,
    pub acl_rule_id: Option<String>,
}

impl Member {
    fn from_entry(
        entry: &LedgerEntry,
        dns_name: Option<String>,
        route: Option<&'static str>,
        is_self: bool,
    ) -> Self {
        Self {
            name: entry.name.clone(),
            ip: entry.ip.to_string(),
            public_key: entry.public_key.to_base64(),
            app_version: entry.app_version.clone(),
            capabilities: entry.capabilities.clone(),
            invite_status: entry.invite_status.clone(),
            invite_expires_at: entry.invite_expires_at,
            online: entry.online,
            is_host: entry.is_host,
            dns_name,
            route,
            is_self,
            subnets: entry.subnets.iter().map(|s| s.to_string()).collect(),
            blocked: entry.blocked,
            force_relay: entry.force_relay,
            acl_rule_id: entry.acl_rule_id.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Peer {
    pub public_key: String,
    pub endpoint: Option<String>,
    pub last_handshake_age_secs: Option<u64>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    /// トンネル内 RTT(ミリ秒)。制御接続が確立するまでは null。
    pub rtt_ms: Option<f64>,
}

impl From<&PeerSummary> for Peer {
    fn from(peer: &PeerSummary) -> Self {
        Self {
            public_key: peer.public_key.to_base64(),
            endpoint: peer.endpoint.map(|e| e.to_string()),
            last_handshake_age_secs: peer.last_handshake_age_secs,
            rx_bytes: peer.rx_bytes,
            tx_bytes: peer.tx_bytes,
            rtt_ms: peer.rtt_ms,
        }
    }
}

/// ファイル転送の進捗 1 件(ADR-0015、M3-9b)。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Transfer {
    pub id: String,
    /// "send" | "recv"(自分から見た向き)
    pub direction: &'static str,
    /// 相手の仮想 IP。
    pub peer: String,
    pub name: String,
    pub size: u64,
    pub transferred: u64,
    pub done: bool,
    pub error: Option<String>,
}

impl From<&peercove_core::ipc::TransferInfo> for Transfer {
    fn from(info: &peercove_core::ipc::TransferInfo) -> Self {
        Self {
            id: info.id.clone(),
            direction: match info.direction {
                peercove_core::ipc::TransferDirection::Send => "send",
                peercove_core::ipc::TransferDirection::Recv => "recv",
            },
            peer: info.peer.to_string(),
            name: info.name.clone(),
            size: info.size,
            transferred: info.transferred,
            done: info.done,
            error: info.error.clone(),
        }
    }
}

/// チャット履歴の 1 通(ADR-0016、M3-13b)。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    /// 履歴内の通し番号(差分フェッチに使う)。
    pub seq: u64,
    pub id: String,
    /// "direct" | "network" | "group"
    pub scope: &'static str,
    /// (group のみ)宛先グループの ID(M3-13c)。
    pub group_id: Option<String>,
    /// 送信者の仮想 IP(自分が送った通は自分の IP)。
    pub from: String,
    /// (direct のみ)宛先の仮想 IP。
    pub to: Option<String>,
    pub text: String,
    pub sent_at_ms: u64,
    /// どの宛先にも届かなかった(デーモン再起動で消える)。
    pub failed: bool,
    /// チャット内ファイル送信のエントリ(M3-13d)。付いていれば text は空。
    pub file: Option<ChatFile>,
    /// グループ操作のお知らせ(中央の 1 行として表示する)。
    pub system: bool,
}

/// チャット内ファイル送信の情報(M3-13d)。実体は受信ボックス。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatFile {
    /// ファイル名(受信側では保存された実ファイル名)。
    pub name: String,
    pub size: u64,
    /// 対応する転送 id(進捗表示用。転送一覧から流れたら進捗なし)。
    pub transfers: Vec<String>,
    /// この端末でのファイルの場所(インラインプレビュー用)。
    /// 移動・削除済みなら読み込みに失敗してプレビューされないだけ。
    pub path: Option<String>,
}

impl From<&ChatMessageInfo> for ChatMessage {
    fn from(info: &ChatMessageInfo) -> Self {
        Self {
            seq: info.seq,
            id: info.id.clone(),
            scope: match info.scope {
                peercove_core::msg::ChatScope::Direct => "direct",
                peercove_core::msg::ChatScope::Network => "network",
                peercove_core::msg::ChatScope::Group => "group",
            },
            group_id: info.group_id.clone(),
            from: info.from.to_string(),
            to: info.to.map(|ip| ip.to_string()),
            text: info.text.clone(),
            sent_at_ms: info.sent_at,
            failed: info.failed,
            file: info.file.as_ref().map(|file| ChatFile {
                name: file.name.clone(),
                size: file.size,
                transfers: file.transfers.clone(),
                // Windows の verbatim 接頭辞(\\?\)は asset URL で扱えないため剥がす
                path: file.path.as_deref().map(display_path),
            }),
            system: info.system,
        }
    }
}

/// ファイル送信のチャット文脈(M3-13d)。frontend から camelCase で受け、
/// core の [`peercove_core::msg::ChatContext`] へ変換する。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatContextDto {
    /// "direct" | "network" | "group"
    pub scope: String,
    pub group_id: Option<String>,
}

impl TryFrom<ChatContextDto> for peercove_core::msg::ChatContext {
    type Error = String;

    fn try_from(dto: ChatContextDto) -> Result<Self, String> {
        use peercove_core::msg::ChatScope;
        let scope = match dto.scope.as_str() {
            "direct" => ChatScope::Direct,
            "network" => ChatScope::Network,
            "group" => ChatScope::Group,
            other => return Err(format!("不明な宛先種別です: {other}")),
        };
        Ok(Self {
            scope,
            group_id: dto.group_id,
        })
    }
}

/// グループ(ADR-0016、M3-13c)。UI は members に自分が居るかで
/// 「参加中/退出済み」を判定する。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub id: String,
    pub name: String,
    /// メンバーの仮想 IP。
    pub members: Vec<String>,
}

impl From<&peercove_core::msg::GroupInfo> for Group {
    fn from(info: &peercove_core::msg::GroupInfo) -> Self {
        Self {
            id: info.id.clone(),
            name: info.name.clone(),
            members: info.members.iter().map(|ip| ip.to_string()).collect(),
        }
    }
}

/// ChatFetch の 1 ページ(ADR-0016)。`seq` は履歴全体の最新 seq。
/// `messages` の末尾がそこへ届くまで繰り返し取る。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatPage {
    pub seq: u64,
    pub messages: Vec<ChatMessage>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tunnel {
    pub config: String,
    /// ネットワーク名(ADR-0012)。
    pub network: String,
    /// "hosting" | "joined"
    pub role: &'static str,
    pub address: String,
    /// 実行時のインターフェース名(ADR-0012 の自動採番後）。設定ファイルの
    /// `interface.name` とは異なりうる。設定画面が接続中はこれを表示する
    /// (ADR-0028、M3-20)。旧デーモンでは空文字。
    pub interface_name: String,
    pub members: Vec<Member>,
    pub peers: Vec<Peer>,
    /// ホストからネットワーク削除された(M2-G6)。UI が明示して切断を促す。
    pub removed: bool,
    /// ホストが参加を拒否し、自動再接続を停止した理由。
    pub connection_error: Option<String>,
    /// ファイル転送の進捗(ADR-0015、M3-9)。実行中 + 直近の完了/失敗分。
    pub transfers: Vec<Transfer>,
    /// チャット履歴の最新 seq(ADR-0016、M3-13b)。これが進んだら差分フェッチする。
    pub chat_seq: u64,
    /// 送信待ち(再送キューに残っている)チャットの seq(E-E 3)。
    pub chat_sending: Vec<u64>,
    /// 既知のグループ(ADR-0016、M3-13c)。自分が抜けたグループも含む
    /// (UI が履歴の表示名に使い、会話リストからは隠す)。
    pub groups: Vec<Group>,
    /// 解決済みカスタム DNS レコード(ADR-0022)。member はここから一覧表示
    /// する(設定ファイルには無いため)。host の DNS 管理画面は編集情報つきの
    /// `list_dns_records` を使う。
    pub dns_records: Vec<DnsRecordDto>,
}

impl From<&TunnelInfo> for Tunnel {
    fn from(info: &TunnelInfo) -> Self {
        // メンバーの DNS 名を台帳から導出(カスタムレコードはメンバー名を
        // 奪えないので、ここでは台帳だけで決まる — ADR-0011 §2)
        let zone = peercove_core::dns::zone_for(&info.network, &info.ledger, &[]);
        let dns_by_key: std::collections::HashMap<&[u8; 32], &str> = zone
            .iter()
            .filter_map(|entry| {
                entry
                    .public_key
                    .as_ref()
                    .map(|key| (key.as_bytes(), entry.fqdn.as_str()))
            })
            .collect();
        Self {
            config: display_path(&info.config),
            network: info.network.clone(),
            role: role_str(info.role),
            address: info.address.to_string(),
            interface_name: info.interface_name.clone(),
            members: info
                .ledger
                .iter()
                .map(|entry| {
                    let dns_name = dns_by_key
                        .get(entry.public_key.as_bytes())
                        .map(|s| s.to_string());
                    let is_self = entry.ip == info.address;
                    // 経路(M3-4): member ロールで「ホストでも自分でもない相手」
                    // にだけ意味がある。直接経路が無ければ中継(ホスト経由)
                    let route = if info.role == TunnelRole::Member && !entry.is_host && !is_self {
                        Some(match info.direct.get(&entry.ip) {
                            Some(peercove_core::ipc::DirectStatus::Direct) => "direct",
                            Some(peercove_core::ipc::DirectStatus::Trying) => "trying",
                            None => "relay",
                        })
                    } else {
                        None
                    };
                    Member::from_entry(entry, dns_name, route, is_self)
                })
                .collect(),
            peers: info.peers.iter().map(Peer::from).collect(),
            removed: info.removed,
            connection_error: info.connection_error.clone(),
            transfers: info.transfers.iter().map(Transfer::from).collect(),
            chat_seq: info.chat_seq,
            chat_sending: info.chat_sending.clone(),
            groups: info.groups.iter().map(Group::from).collect(),
            // 配布形式は解決済み(A は {name, ip}、CNAME は {name, target})。
            // どちらもメンバー一覧に出す(参照情報は持たない)
            dns_records: info
                .dns_records
                .iter()
                .map(|record| {
                    let fqdn = format!(
                        "{}.{}.{}",
                        record.name,
                        info.network,
                        peercove_core::dns::DNS_SUFFIX
                    );
                    DnsRecordDto {
                        id: None,
                        name: record.name.clone(),
                        ip: Some(record.ip.to_string()),
                        cname: None,
                        url: peercove_core::dns::service_url(
                            &fqdn,
                            record.scheme.as_deref(),
                            record.port,
                        ),
                        fqdn,
                        member: None,
                        under: None,
                        scheme: record.scheme.clone(),
                        port: record.port,
                        health: record.health.as_ref().map(ServiceHealthDto::from),
                        health_settings: None,
                    }
                })
                .chain(info.cname_records.iter().map(|record| {
                    let fqdn = format!(
                        "{}.{}.{}",
                        record.name,
                        info.network,
                        peercove_core::dns::DNS_SUFFIX
                    );
                    DnsRecordDto {
                        id: None,
                        name: record.name.clone(),
                        ip: None,
                        cname: Some(record.target.clone()),
                        url: peercove_core::dns::service_url(
                            &fqdn,
                            record.scheme.as_deref(),
                            record.port,
                        ),
                        fqdn,
                        member: None,
                        under: None,
                        scheme: record.scheme.clone(),
                        port: record.port,
                        health: record.health.as_ref().map(ServiceHealthDto::from),
                        health_settings: None,
                    }
                }))
                .collect(),
        }
    }
}

/// 受信ボックスの 1 ファイル(ADR-0015、M3-9b)。メタ情報は受信時に
/// デーモンが置いた `.pcvmeta`(無ければ null)。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxItem {
    /// ファイル名(受信ボックス内で一意)。
    pub name: String,
    pub size: u64,
    pub from_name: Option<String>,
    pub from_ip: Option<String>,
    pub received_unix_ms: Option<u64>,
}

fn role_str(role: TunnelRole) -> &'static str {
    match role {
        TunnelRole::Host => "hosting",
        TunnelRole::Member => "joined",
    }
}

/// 表示用にパスを整える。
///
/// Windows の `canonicalize` は verbatim 接頭辞(`\\?\`)を付ける。デーモンへ渡す
/// パスとしては正しいが、画面に出すと読みづらいだけなので剥がす。
fn display_path(path: &Path) -> String {
    let text = path.display().to_string();
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{unc}");
    }
    text.strip_prefix(r"\\?\").unwrap_or(&text).to_string()
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// "idle" | "hosting" | "joined"。複数稼働時は先頭トンネルの状態
    /// (現行 UI の単一表示との互換。M3-0c で tunnels の一覧表示に移行)。
    pub state: &'static str,
    /// 互換用: 先頭のトンネル。
    pub tunnel: Option<Tunnel>,
    /// 稼働中の全トンネル(ADR-0012)。
    pub tunnels: Vec<Tunnel>,
    /// デーモンの IPC バージョンが UI と一致しない(旧デーモンが動いている)。
    /// 状態表示が信用できないため、UI は警告を出して更新を促す。
    pub daemon_outdated: bool,
    /// デーモン実行ファイルの製品バージョン。旧デーモンは null。
    pub daemon_version: Option<String>,
}

impl From<DaemonStatus> for Status {
    fn from(status: DaemonStatus) -> Self {
        let tunnels: Vec<Tunnel> = status.tunnels.iter().map(Tunnel::from).collect();
        let state = status
            .tunnels
            .first()
            .map(|info| role_str(info.role))
            .unwrap_or("idle");
        let tunnel = status.tunnels.first().map(Tunnel::from);
        Self {
            state,
            tunnel,
            tunnels,
            daemon_outdated: status.version != IPC_VERSION,
            daemon_version: status.app_version,
        }
    }
}

/// 設定済みネットワーク 1 件(M3-0c)。稼働状態は含まない
/// (frontend が daemon status の tunnels と configPath で突き合わせる)。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkDto {
    pub slug: String,
    pub name: String,
    /// "hosting" | "joined"(設定上の役割)
    pub role: &'static str,
    /// 表示・突き合わせ用の正規化済みパス(daemon status の config と一致する)
    pub config_path: String,
    pub address: String,
}

impl From<&peercove_ops::networks::NetworkEntry> for NetworkDto {
    fn from(entry: &peercove_ops::networks::NetworkEntry) -> Self {
        // daemon へ渡すパスと同じ正規化(canonicalize)を経由させることで、
        // status の config(display_path 済み)と文字列一致するようにする
        let canonical =
            std::fs::canonicalize(&entry.config_path).unwrap_or_else(|_| entry.config_path.clone());
        Self {
            slug: entry.slug.clone(),
            name: entry.name.clone(),
            role: match entry.role {
                peercove_ops::networks::Role::Host => "hosting",
                peercove_ops::networks::Role::Member => "joined",
            },
            config_path: display_path(&canonical),
            address: entry.address.to_string(),
        }
    }
}

/// カスタム DNS レコード(M3-1c、ADR-0022 で拡張)。fqdn は表示用に
/// 組み立て済み。member / under は "host" または公開鍵(フロントが
/// メンバー一覧と突き合わせて表示名にする)。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DnsRecordDto {
    pub id: Option<String>,
    pub name: String,
    /// 解決済みの現在の IP(メンバー参照が切れている場合・CNAME は None)
    pub ip: Option<String>,
    /// CNAME の転送先ドメイン(A / メンバー参照レコードは None — ADR-0025)
    pub cname: Option<String>,
    pub fqdn: String,
    /// ターゲットのメンバー参照(固定 IP / CNAME レコードは None)
    pub member: Option<String>,
    /// 親メンバー(最上位レコードは None)
    pub under: Option<String>,
    pub scheme: Option<String>,
    pub port: Option<u16>,
    pub url: Option<String>,
    pub health: Option<ServiceHealthDto>,
    /// ホストの設定ファイルを直接読んだ場合だけ付く編集情報。
    pub health_settings: Option<HealthSettingsDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceHealthDto {
    pub status: &'static str,
    pub reason: &'static str,
    pub checked_at_unix_ms: Option<u64>,
    pub response_ms: Option<u64>,
    pub http_status: Option<u16>,
}

impl From<&peercove_core::dns::ServiceHealth> for ServiceHealthDto {
    fn from(health: &peercove_core::dns::ServiceHealth) -> Self {
        use peercove_core::dns::{ServiceHealthReason as Reason, ServiceHealthStatus as Status};
        Self {
            status: match health.status {
                Status::Healthy => "healthy",
                Status::Unhealthy => "unhealthy",
                Status::Unknown => "unknown",
                Status::Disabled => "disabled",
            },
            reason: match health.reason {
                Reason::NotChecked => "not_checked",
                Reason::Offline => "offline",
                Reason::Timeout => "timeout",
                Reason::ConnectionFailed => "connection_failed",
                Reason::NameResolutionFailed => "name_resolution_failed",
                Reason::UnexpectedStatus => "unexpected_status",
                Reason::Disabled => "disabled",
            },
            checked_at_unix_ms: health.checked_at_unix_ms,
            response_ms: health.response_ms,
            http_status: health.http_status,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthSettingsDto {
    pub enabled: bool,
    pub kind: &'static str,
    pub path: String,
    pub expected_status: Option<u16>,
    pub external: bool,
}

impl From<peercove_ops::dns::RecordDetail> for DnsRecordDto {
    fn from(detail: peercove_ops::dns::RecordDetail) -> Self {
        let (member, cname) = match &detail.target {
            peercove_ops::dns::RecordTarget::Ip(_) => (None, None),
            peercove_ops::dns::RecordTarget::Member(member) => {
                (Some(member.to_config_string()), None)
            }
            peercove_ops::dns::RecordTarget::Cname(domain) => (None, Some(domain.clone())),
        };
        Self {
            id: detail.id,
            name: detail.name,
            ip: detail.resolved_ip.map(|ip| ip.to_string()),
            cname,
            fqdn: detail.fqdn,
            member,
            under: detail.under.map(|under| under.to_config_string()),
            scheme: detail.scheme,
            port: detail.port,
            url: detail.url,
            health: None,
            health_settings: Some(HealthSettingsDto {
                enabled: detail.health.enabled,
                kind: match detail.health.kind {
                    peercove_core::dns::HealthCheckKind::Tcp => "tcp",
                    peercove_core::dns::HealthCheckKind::HttpHead => "http_head",
                },
                path: detail.health.path,
                expected_status: detail.health.expected_status,
                external: detail.health.external,
            }),
        }
    }
}

/// ホスト初期化の結果。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitResult {
    pub config_path: String,
    pub network: String,
    pub subnet: String,
    pub host_ip: String,
    pub public_key: String,
}

/// 招待の結果。**token は秘密情報**で、発行直後のダイアログでのみ表示する(ADR-0008)。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteResult {
    pub token: String,
    /// ターミナル向けではなく画面表示用の QR(SVG 文字列)
    pub qr_svg: String,
    pub name: String,
    pub ip: String,
    pub endpoints: Vec<String>,
    pub psk: bool,
    pub invite_id: String,
    pub issued_at: u64,
    pub expires_at: Option<u64>,
}

/// 参加(join)の結果。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinResult {
    pub config_path: String,
    pub name: String,
    pub address: String,
    pub endpoint: String,
    pub other_endpoints: Vec<String>,
}

/// 設定編集(M2-G5)。`peercove_ops::settings` の型を camelCase で往復させる。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub interface_name: String,
    pub display_name: Option<String>,
    /// (host のみ)自分の DNS 名(ADR-0021、M3-14a)。
    pub dns_name: Option<String>,
    pub address: String,
    pub listen_port: Option<u16>,
    pub mtu: u16,
    pub host_endpoint: Option<String>,
    pub is_member: bool,
    /// メンバー間直接通信を試すか(ADR-0013)。
    pub direct: bool,
    /// 受信するファイルサイズの上限(MB、ADR-0015)。0 で無制限。
    pub max_recv_file_mb: u64,
    pub require_invite_approval: bool,
    /// 既定値。UI の入力欄のプレースホルダに使う。
    pub default_mtu: u16,
    pub default_listen_port: u16,
    pub default_max_recv_file_mb: u64,
}

impl From<peercove_ops::settings::Settings> for Settings {
    fn from(settings: peercove_ops::settings::Settings) -> Self {
        Self {
            interface_name: settings.interface_name,
            display_name: settings.display_name,
            dns_name: settings.dns_name,
            address: settings.address,
            listen_port: settings.listen_port,
            mtu: settings.mtu,
            host_endpoint: settings.host_endpoint,
            is_member: settings.is_member,
            direct: settings.direct,
            max_recv_file_mb: settings.max_recv_file_mb,
            require_invite_approval: settings.require_invite_approval,
            default_mtu: peercove_core::config::DEFAULT_MTU,
            default_listen_port: peercove_core::config::DEFAULT_LISTEN_PORT,
            default_max_recv_file_mb: peercove_core::config::DEFAULT_MAX_RECV_FILE_MB,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsUpdate {
    pub display_name: Option<String>,
    /// (host のみ)自分の DNS 名(ADR-0021)。None / 空文字で従来導出に戻す。
    #[serde(default)]
    pub dns_name: Option<String>,
    pub listen_port: Option<u16>,
    pub mtu: u16,
    pub host_endpoint: Option<String>,
    pub direct: bool,
    pub max_recv_file_mb: u64,
    #[serde(default)]
    pub require_invite_approval: bool,
}

impl From<SettingsUpdate> for peercove_ops::settings::Update {
    fn from(update: SettingsUpdate) -> Self {
        Self {
            display_name: update.display_name,
            dns_name: update.dns_name,
            listen_port: update.listen_port,
            mtu: update.mtu,
            host_endpoint: update.host_endpoint,
            direct: update.direct,
            max_recv_file_mb: update.max_recv_file_mb,
            require_invite_approval: update.require_invite_approval,
        }
    }
}

/// 設定保存の結果。トンネル再起動が要るかを UI へ伝える。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveResult {
    pub restart_required: bool,
}

/// デーモンのログ 1 行(M2-G5)。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub seq: u64,
    pub unix_ms: u64,
    pub level: String,
    pub target: String,
    pub message: String,
}

impl From<LogLine> for LogEntry {
    fn from(line: LogLine) -> Self {
        Self {
            seq: line.seq,
            unix_ms: line.unix_ms,
            level: line.level,
            target: line.target,
            message: line.message,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Logs {
    pub lines: Vec<LogEntry>,
    /// バッファから溢れて失われた行数(0 なら欠落なし)。
    pub dropped: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use peercove_core::keys::PrivateKey;

    /// frontend(src/ipc.ts)が期待する camelCase の JSON になること。
    #[test]
    fn status_serializes_to_ui_shape() {
        let info = TunnelInfo {
            config: std::path::PathBuf::from("host.toml"),
            network: "home".to_string(),
            role: TunnelRole::Host,
            address: "10.100.42.1".parse().unwrap(),
            interface_name: "peercove0".to_string(),
            external_endpoint: None,
            ledger: vec![LedgerEntry {
                name: Some("alice".to_string()),
                dns_name: None,
                ip: "10.100.42.2".parse().unwrap(),
                public_key: PrivateKey::generate().public_key(),
                app_version: Some("0.1.0".to_string()),
                capabilities: peercove_core::proto::current_capabilities(),
                invite_status: Some("joined".to_string()),
                invite_expires_at: Some(1_700_086_400),
                online: true,
                is_host: false,
                endpoint: None,
                endpoint_age_secs: None,
                subnets: vec![],
                blocked: false,
                force_relay: false,
                acl_rule_id: None,
            }],
            peers: vec![],
            removed: false,
            connection_error: None,
            direct: Default::default(),
            transfers: vec![],
            chat_seq: 0,
            chat_sending: vec![],
            groups: vec![],
            dns_records: vec![peercove_core::dns::DnsRecord {
                name: "web.alice".to_string(),
                ip: "10.100.42.2".parse().unwrap(),
                scheme: Some("http".to_string()),
                port: Some(8080),
                health: None,
            }],
            cname_records: vec![peercove_core::dns::CnameRecord {
                name: "docs".to_string(),
                target: "example.com".to_string(),
                resolved_ip: None,
                scheme: None,
                port: None,
                health: None,
            }],
        };
        let json = serde_json::to_value(Status::from(DaemonStatus {
            version: peercove_core::ipc::IPC_VERSION,
            app_version: Some("0.1.0".to_string()),
            tunnels: vec![info],
        }))
        .unwrap();
        assert_eq!(json["daemonOutdated"], false);
        assert_eq!(json["state"], "hosting");
        assert_eq!(json["tunnel"]["address"], "10.100.42.1");
        assert_eq!(
            json["tunnel"]["interfaceName"], "peercove0",
            "実行時のインターフェース名が status に載る(M3-20)"
        );
        assert_eq!(json["tunnel"]["network"], "home");
        assert_eq!(json["tunnel"]["role"], "hosting");
        assert_eq!(json["tunnel"]["members"][0]["name"], "alice");
        assert_eq!(json["tunnel"]["members"][0]["isHost"], false);
        assert!(json["tunnel"]["members"][0]["publicKey"].is_string());
        assert_eq!(
            json["tunnel"]["members"][0]["dnsName"], "alice.home.peercove.internal",
            "DNS 名が台帳から導出される(M3-1c)"
        );
        assert_eq!(
            json["tunnel"]["members"][0]["route"],
            serde_json::Value::Null,
            "host ロールでは経路バッジなし(M3-4)"
        );
        assert_eq!(
            json["tunnel"]["dnsRecords"][0]["fqdn"], "web.alice.home.peercove.internal",
            "配信されたレコードが status に載る(ADR-0022 検証 FB)"
        );
        assert_eq!(json["tunnel"]["dnsRecords"][0]["ip"], "10.100.42.2");
        assert_eq!(json["tunnel"]["dnsRecords"][0]["scheme"], "http");
        assert_eq!(json["tunnel"]["dnsRecords"][0]["port"], 8080);
        assert_eq!(
            json["tunnel"]["dnsRecords"][0]["url"], "http://web.alice.home.peercove.internal:8080/",
            "メンバー側の配布経路でも URL が組み立てられる"
        );
        assert_eq!(json["tunnels"].as_array().unwrap().len(), 1);

        let json = serde_json::to_value(Status::from(DaemonStatus {
            version: IPC_VERSION,
            app_version: Some("0.1.0".to_string()),
            tunnels: vec![],
        }))
        .unwrap();
        assert_eq!(json["state"], "idle");
        assert!(json["tunnel"].is_null());
        assert_eq!(json["tunnels"].as_array().unwrap().len(), 0);

        // 旧デーモン(version 欠落 = 0)は明示フラグで検出できる
        let json = serde_json::to_value(Status::from(DaemonStatus {
            version: 0,
            app_version: None,
            tunnels: vec![],
        }))
        .unwrap();
        assert_eq!(json["daemonOutdated"], true);
    }

    /// member ロールでは他メンバーに経路(direct / trying / relay)が付き、
    /// ホスト・自分には付かない(M3-4)。
    #[test]
    fn member_routes_are_derived_from_direct_map() {
        let entry = |name: &str, ip: &str, is_host: bool| LedgerEntry {
            name: Some(name.to_string()),
            dns_name: None,
            ip: ip.parse().unwrap(),
            public_key: PrivateKey::generate().public_key(),
            app_version: None,
            capabilities: vec![],
            invite_status: None,
            invite_expires_at: None,
            online: true,
            is_host,
            endpoint: None,
            endpoint_age_secs: None,
            subnets: vec![],
            blocked: false,
            force_relay: false,
            acl_rule_id: None,
        };
        let mut direct = std::collections::HashMap::new();
        direct.insert(
            "10.100.42.3".parse().unwrap(),
            peercove_core::ipc::DirectStatus::Direct,
        );
        direct.insert(
            "10.100.42.4".parse().unwrap(),
            peercove_core::ipc::DirectStatus::Trying,
        );
        let info = TunnelInfo {
            config: std::path::PathBuf::from("member.toml"),
            network: "home".to_string(),
            role: TunnelRole::Member,
            address: "10.100.42.2".parse().unwrap(), // 自分
            interface_name: "peercove0".to_string(),
            external_endpoint: None,
            ledger: vec![
                entry("host", "10.100.42.1", true),
                entry("me", "10.100.42.2", false),
                entry("bob", "10.100.42.3", false),
                entry("carol", "10.100.42.4", false),
                entry("dave", "10.100.42.5", false),
            ],
            peers: vec![],
            removed: false,
            connection_error: Some("この招待は別の端末で使用済みです".to_string()),
            direct,
            transfers: vec![],
            chat_seq: 0,
            chat_sending: vec![],
            groups: vec![],
            dns_records: vec![],
            cname_records: vec![],
        };
        let tunnel = Tunnel::from(&info);
        assert_eq!(
            tunnel.connection_error.as_deref(),
            Some("この招待は別の端末で使用済みです")
        );
        let routes: Vec<Option<&str>> = tunnel.members.iter().map(|m| m.route).collect();
        assert_eq!(
            routes,
            vec![
                None,           // ホスト
                None,           // 自分
                Some("direct"), // 直接通信中
                Some("trying"), // 確立中
                Some("relay"),  // ホスト経由
            ]
        );
        let selves: Vec<bool> = tunnel.members.iter().map(|m| m.is_self).collect();
        assert_eq!(
            selves,
            vec![false, true, false, false, false],
            "仮想 IP が一致する行だけ自分(UI の「自分」タグ)"
        );
    }

    /// Windows の verbatim 接頭辞は表示から取り除く。
    #[test]
    fn display_path_strips_verbatim_prefix() {
        assert_eq!(
            display_path(Path::new(r"\\?\D:\dev\peercove\host.toml")),
            r"D:\dev\peercove\host.toml"
        );
        assert_eq!(
            display_path(Path::new(r"\\?\UNC\server\share\host.toml")),
            r"\\server\share\host.toml"
        );
        assert_eq!(
            display_path(Path::new("/home/me/.config/peercove/host.toml")),
            "/home/me/.config/peercove/host.toml"
        );
    }

    /// RTT は camelCase の `rttMs` で、未測定なら null で出る(UI が判定に使う)。
    #[test]
    fn peer_rtt_serializes_as_nullable_camel_case() {
        let mut summary = PeerSummary {
            public_key: PrivateKey::generate().public_key(),
            endpoint: None,
            last_handshake_age_secs: None,
            rx_bytes: 0,
            tx_bytes: 0,
            rtt_ms: None,
        };
        let json = serde_json::to_value(Peer::from(&summary)).unwrap();
        assert!(json["rttMs"].is_null());

        summary.rtt_ms = Some(12.5);
        let json = serde_json::to_value(Peer::from(&summary)).unwrap();
        assert_eq!(json["rttMs"], 12.5);
    }

    /// 設定は camelCase で往復する(frontend の SettingsForm と対)。
    #[test]
    fn settings_round_trip_through_ui_shape() {
        let json = serde_json::to_value(Settings::from(peercove_ops::settings::Settings {
            interface_name: "peercove0".to_string(),
            display_name: Some("alice".to_string()),
            dns_name: None,
            address: "10.119.96.2/24".to_string(),
            listen_port: None,
            mtu: 1420,
            host_endpoint: Some("203.0.113.5:51820".to_string()),
            is_member: true,
            direct: true,
            max_recv_file_mb: 100,
            require_invite_approval: false,
        }))
        .unwrap();
        assert_eq!(json["hostEndpoint"], "203.0.113.5:51820");
        assert_eq!(json["isMember"], true);
        assert_eq!(json["direct"], true);
        assert!(json["listenPort"].is_null());
        assert_eq!(json["defaultMtu"], 1420);
        assert_eq!(json["maxRecvFileMb"], 100);
        assert_eq!(json["defaultMaxRecvFileMb"], 100);

        let update: SettingsUpdate = serde_json::from_value(serde_json::json!({
            "displayName": "bob",
            "listenPort": 51900,
            "mtu": 1380,
            "hostEndpoint": null,
            "direct": false,
            "maxRecvFileMb": 500,
        }))
        .unwrap();
        let update: peercove_ops::settings::Update = update.into();
        assert_eq!(update.display_name.as_deref(), Some("bob"));
        assert_eq!(update.listen_port, Some(51900));
        assert_eq!(update.host_endpoint, None);
        assert!(!update.direct);
        assert_eq!(update.max_recv_file_mb, 500);
    }

    #[test]
    fn host_record_detail_keeps_service_url() {
        let ip = "10.100.42.2".parse().unwrap();
        let dto = DnsRecordDto::from(peercove_ops::dns::RecordDetail {
            id: Some("svc-test".to_string()),
            name: "gamehost".to_string(),
            under: None,
            relative: "gamehost".to_string(),
            fqdn: "gamehost.home.peercove.internal".to_string(),
            target: peercove_ops::dns::RecordTarget::Ip(ip),
            resolved_ip: Some(ip),
            scheme: Some("https".to_string()),
            port: Some(443),
            url: Some("https://gamehost.home.peercove.internal/".to_string()),
            health: peercove_ops::dns::HealthSettings {
                enabled: true,
                kind: peercove_core::dns::HealthCheckKind::Tcp,
                path: "/".to_string(),
                expected_status: None,
                external: false,
            },
        });
        let json = serde_json::to_value(dto).unwrap();
        assert_eq!(json["scheme"], "https");
        assert_eq!(json["port"], 443);
        assert_eq!(json["url"], "https://gamehost.home.peercove.internal/");
    }

    #[test]
    fn invite_result_serializes_camel_case() {
        let json = serde_json::to_value(InviteResult {
            token: "pcv1.xxx".to_string(),
            qr_svg: "<svg/>".to_string(),
            name: "alice".to_string(),
            ip: "10.100.42.2".to_string(),
            endpoints: vec!["192.168.0.12:51820".to_string()],
            psk: true,
            invite_id: "0123456789abcdef0123456789abcdef".to_string(),
            issued_at: 1_700_000_000,
            expires_at: Some(1_700_086_400),
        })
        .unwrap();
        assert_eq!(json["qrSvg"], "<svg/>");
        assert_eq!(json["token"], "pcv1.xxx");
    }
}
