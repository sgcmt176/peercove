//! frontend へ渡す DTO。`apps/peercove-ui/src/ipc.ts` と対で保守すること。

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
}
