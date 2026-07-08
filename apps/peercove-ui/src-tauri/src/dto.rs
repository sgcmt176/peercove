//! frontend へ渡す DTO。`apps/peercove-ui/src/ipc.ts` と対で保守すること。

use std::path::Path;

use peercove_core::ipc::{DaemonStatus, PeerSummary, TunnelInfo};
use peercove_core::proto::LedgerEntry;
use serde::Serialize;

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
}

impl From<&PeerSummary> for Peer {
    fn from(peer: &PeerSummary) -> Self {
        Self {
            public_key: peer.public_key.to_base64(),
            endpoint: peer.endpoint.map(|e| e.to_string()),
            last_handshake_age_secs: peer.last_handshake_age_secs,
            rx_bytes: peer.rx_bytes,
            tx_bytes: peer.tx_bytes,
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
}

impl From<&TunnelInfo> for Tunnel {
    fn from(info: &TunnelInfo) -> Self {
        Self {
            config: info.config.display().to_string(),
            address: info.address.to_string(),
            members: info.ledger.iter().map(Member::from).collect(),
            peers: info.peers.iter().map(Peer::from).collect(),
        }
    }
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
