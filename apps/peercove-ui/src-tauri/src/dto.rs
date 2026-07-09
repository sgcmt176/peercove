//! frontend へ渡す DTO。`apps/peercove-ui/src/ipc.ts` と対で保守すること。

use std::path::Path;

use peercove_core::ipc::{DaemonStatus, LogLine, PeerSummary, TunnelInfo};
use peercove_core::proto::LedgerEntry;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Member {
    pub name: Option<String>,
    pub ip: String,
    pub public_key: String,
    pub online: bool,
    pub is_host: bool,
}

impl From<&LedgerEntry> for Member {
    fn from(entry: &LedgerEntry) -> Self {
        Self {
            name: entry.name.clone(),
            ip: entry.ip.to_string(),
            public_key: entry.public_key.to_base64(),
            online: entry.online,
            is_host: entry.is_host,
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tunnel {
    pub config: String,
    pub address: String,
    pub members: Vec<Member>,
    pub peers: Vec<Peer>,
    /// ホストからネットワーク削除された(M2-G6)。UI が明示して切断を促す。
    pub removed: bool,
}

impl From<&TunnelInfo> for Tunnel {
    fn from(info: &TunnelInfo) -> Self {
        Self {
            config: display_path(&info.config),
            address: info.address.to_string(),
            members: info.ledger.iter().map(Member::from).collect(),
            peers: info.peers.iter().map(Peer::from).collect(),
            removed: info.removed,
        }
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
    /// "idle" | "hosting" | "joined"(同時参加は 1 ネットワークまで)
    pub state: &'static str,
    pub tunnel: Option<Tunnel>,
}

impl From<DaemonStatus> for Status {
    fn from(status: DaemonStatus) -> Self {
        match status {
            DaemonStatus::Idle => Self {
                state: "idle",
                tunnel: None,
            },
            DaemonStatus::Hosting(info) => Self {
                state: "hosting",
                tunnel: Some(Tunnel::from(&info)),
            },
            DaemonStatus::Joined(info) => Self {
                state: "joined",
                tunnel: Some(Tunnel::from(&info)),
            },
        }
    }
}

/// UI が扱う設定ファイルの所在。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSlot {
    pub path: String,
    pub exists: bool,
}

impl ConfigSlot {
    pub fn of(path: &Path) -> Self {
        Self {
            path: path.display().to_string(),
            exists: path.exists(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigPaths {
    /// 既定のホスト設定(アプリのデータディレクトリ)
    pub host: ConfigSlot,
    /// 既定のメンバー設定
    pub member: ConfigSlot,
    /// 設定を置くディレクトリ
    pub dir: String,
}

/// ホスト初期化の結果。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitResult {
    pub config_path: String,
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
    pub address: String,
    pub listen_port: Option<u16>,
    pub mtu: u16,
    pub host_endpoint: Option<String>,
    pub is_member: bool,
    /// 既定値。UI の入力欄のプレースホルダに使う。
    pub default_mtu: u16,
    pub default_listen_port: u16,
}

impl From<peercove_ops::settings::Settings> for Settings {
    fn from(settings: peercove_ops::settings::Settings) -> Self {
        Self {
            interface_name: settings.interface_name,
            display_name: settings.display_name,
            address: settings.address,
            listen_port: settings.listen_port,
            mtu: settings.mtu,
            host_endpoint: settings.host_endpoint,
            is_member: settings.is_member,
            default_mtu: peercove_core::config::DEFAULT_MTU,
            default_listen_port: peercove_core::config::DEFAULT_LISTEN_PORT,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsUpdate {
    pub display_name: Option<String>,
    pub listen_port: Option<u16>,
    pub mtu: u16,
    pub host_endpoint: Option<String>,
}

impl From<SettingsUpdate> for peercove_ops::settings::Update {
    fn from(update: SettingsUpdate) -> Self {
        Self {
            display_name: update.display_name,
            listen_port: update.listen_port,
            mtu: update.mtu,
            host_endpoint: update.host_endpoint,
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
            address: "10.100.42.1".parse().unwrap(),
            ledger: vec![LedgerEntry {
                name: Some("alice".to_string()),
                ip: "10.100.42.2".parse().unwrap(),
                public_key: PrivateKey::generate().public_key(),
                online: true,
                is_host: false,
            }],
            peers: vec![],
            removed: false,
        };
        let json = serde_json::to_value(Status::from(DaemonStatus::Hosting(info))).unwrap();
        assert_eq!(json["state"], "hosting");
        assert_eq!(json["tunnel"]["address"], "10.100.42.1");
        assert_eq!(json["tunnel"]["members"][0]["name"], "alice");
        assert_eq!(json["tunnel"]["members"][0]["isHost"], false);
        assert!(json["tunnel"]["members"][0]["publicKey"].is_string());

        let json = serde_json::to_value(Status::from(DaemonStatus::Idle)).unwrap();
        assert_eq!(json["state"], "idle");
        assert!(json["tunnel"].is_null());
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
            address: "10.119.96.2/24".to_string(),
            listen_port: None,
            mtu: 1420,
            host_endpoint: Some("203.0.113.5:51820".to_string()),
            is_member: true,
        }))
        .unwrap();
        assert_eq!(json["hostEndpoint"], "203.0.113.5:51820");
        assert_eq!(json["isMember"], true);
        assert!(json["listenPort"].is_null());
        assert_eq!(json["defaultMtu"], 1420);

        let update: SettingsUpdate = serde_json::from_value(serde_json::json!({
            "displayName": "bob",
            "listenPort": 51900,
            "mtu": 1380,
            "hostEndpoint": null,
        }))
        .unwrap();
        let update: peercove_ops::settings::Update = update.into();
        assert_eq!(update.display_name.as_deref(), Some("bob"));
        assert_eq!(update.listen_port, Some(51900));
        assert_eq!(update.host_endpoint, None);
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
        })
        .unwrap();
        assert_eq!(json["qrSvg"], "<svg/>");
        assert_eq!(json["token"], "pcv1.xxx");
    }
}
