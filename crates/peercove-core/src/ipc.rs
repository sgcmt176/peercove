//! デーモン制御用ローカル IPC のプロトコル型(M2-G1、ADR-0007)。
//!
//! - トランスポート: Windows = 名前付きパイプ / Linux = Unix ドメインソケット
//! - フレーミング: JSON Lines。リクエストは [`IpcEnvelope`]、応答は [`IpcReply`]
//! - 招待・削除などの設定ファイル操作は IPC に乗せない(ADR-0007)

use std::net::Ipv4Addr;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::proto::LedgerEntry;

/// Windows の名前付きパイプ名。
pub const PIPE_NAME: &str = r"\\.\pipe\peercove-daemon";

/// Linux の UDS パス(root 実行時の既定)。
pub const SOCKET_PATH_ROOT: &str = "/run/peercove.sock";

pub const IPC_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IpcEnvelope {
    pub id: u64,
    pub req: IpcRequest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum IpcRequest {
    /// デーモンと現在のトンネルの状態を返す。
    Status,
    /// ホストとしてトンネルを開始する。
    StartHost { config: PathBuf, upnp: bool },
    /// メンバーとしてトンネルを開始する。
    StartMember { config: PathBuf },
    /// トンネルを停止する(デーモンは常駐継続)。
    Stop,
    /// デーモンを終了する(トンネルが動いていれば停止してから)。
    Shutdown,
    /// デーモンが保持する直近のログを取り出す(M2-G5)。
    /// `after_seq` より後の行だけを返す(0 なら持っている分すべて)。
    Logs {
        #[serde(default)]
        after_seq: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IpcReply {
    pub id: u64,
    #[serde(flatten)]
    pub result: IpcResult,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpcResult {
    Ok(IpcResponse),
    Err(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcResponse {
    /// Status への応答。
    Status(DaemonStatus),
    /// 副作用系(start/stop/shutdown)への応答。
    Done,
    /// Logs への応答。`dropped` はリングバッファから溢れて失われた行数。
    Logs {
        lines: Vec<LogLine>,
        #[serde(default)]
        dropped: u64,
    },
}

/// デーモンが保持する直近のログ 1 行(M2-G5)。
///
/// 秘密鍵・PSK・トークンはそもそもログに出さない方針(CLAUDE.md)なので、
/// この行をそのまま UI へ渡してよい。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogLine {
    /// デーモン起動からの連番。次回取得の `after_seq` に使う。
    pub seq: u64,
    /// UNIX エポックからのミリ秒。
    pub unix_ms: u64,
    /// `ERROR` / `WARN` / `INFO` / `DEBUG` / `TRACE`。
    pub level: String,
    pub target: String,
    pub message: String,
}

/// 1 応答で返すログ行の上限([`crate::ipc`] の 1 行 JSON が
/// `peercove-ipc` の受信上限を超えないようにする)。
pub const MAX_LOG_LINES_PER_REPLY: usize = 200;

/// デーモンの状態モデル(M2 handoff Q4: 同時 1 ネットワーク)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum DaemonStatus {
    Idle,
    Hosting(TunnelInfo),
    Joined(TunnelInfo),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TunnelInfo {
    pub config: PathBuf,
    /// 自分の仮想 IP。
    pub address: Ipv4Addr,
    /// 台帳スナップショット(host: 自前構築 / member: 受信したもの)。
    #[serde(default)]
    pub ledger: Vec<LedgerEntry>,
    /// ピア統計の要約(公開鍵 base64 → (最終ハンドシェイク経過秒, rx, tx))。
    #[serde(default)]
    pub peers: Vec<PeerSummary>,
    /// (member のみ)ホストからネットワーク削除された(M2-G6)。
    /// トンネルはまだ張ったままだが通信は落ちている。UI が明示して切断を促す。
    #[serde(default)]
    pub removed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PeerSummary {
    pub public_key: crate::keys::PublicKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<std::net::SocketAddr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_handshake_age_secs: Option<u64>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    /// トンネル内コントロールチャネルの往復時間(ミリ秒、M2-G5)。
    /// 相手が旧バージョンで ping に応答しない場合や、制御接続前は `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rtt_ms: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_reply_json_roundtrip() {
        let requests = vec![
            IpcRequest::Status,
            IpcRequest::StartHost {
                config: PathBuf::from("host.toml"),
                upnp: true,
            },
            IpcRequest::StartMember {
                config: PathBuf::from("member.toml"),
            },
            IpcRequest::Stop,
            IpcRequest::Shutdown,
            IpcRequest::Logs { after_seq: 42 },
        ];
        for (i, req) in requests.into_iter().enumerate() {
            let envelope = IpcEnvelope { id: i as u64, req };
            let json = serde_json::to_string(&envelope).unwrap();
            assert!(!json.contains('\n'));
            let parsed: IpcEnvelope = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, envelope);
        }

        let replies = vec![
            IpcReply {
                id: 1,
                result: IpcResult::Ok(IpcResponse::Done),
            },
            IpcReply {
                id: 2,
                result: IpcResult::Ok(IpcResponse::Status(DaemonStatus::Idle)),
            },
            IpcReply {
                id: 3,
                result: IpcResult::Err("トンネルは動いていません".to_string()),
            },
        ];
        for reply in replies {
            let json = serde_json::to_string(&reply).unwrap();
            let parsed: IpcReply = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, reply);
        }
    }

    /// ワイヤ表現を固定する(UI 実装が依存するため)。
    #[test]
    fn wire_format_is_stable() {
        let json = serde_json::to_string(&IpcEnvelope {
            id: 7,
            req: IpcRequest::Status,
        })
        .unwrap();
        assert_eq!(json, r#"{"id":7,"req":{"method":"status"}}"#);

        let json = serde_json::to_string(&IpcReply {
            id: 7,
            result: IpcResult::Ok(IpcResponse::Status(DaemonStatus::Idle)),
        })
        .unwrap();
        assert_eq!(json, r#"{"id":7,"ok":{"type":"status","state":"idle"}}"#);

        let json = serde_json::to_string(&IpcReply {
            id: 8,
            result: IpcResult::Err("x".to_string()),
        })
        .unwrap();
        assert_eq!(json, r#"{"id":8,"err":"x"}"#);

        let json = serde_json::to_string(&IpcEnvelope {
            id: 9,
            req: IpcRequest::Logs { after_seq: 3 },
        })
        .unwrap();
        assert_eq!(json, r#"{"id":9,"req":{"method":"logs","after_seq":3}}"#);

        let json = serde_json::to_string(&IpcReply {
            id: 9,
            result: IpcResult::Ok(IpcResponse::Logs {
                lines: vec![LogLine {
                    seq: 1,
                    unix_ms: 1_700_000_000_000,
                    level: "INFO".to_string(),
                    target: "peercove_poc::daemon".to_string(),
                    message: "トンネルを開始しました".to_string(),
                }],
                dropped: 0,
            }),
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"id":9,"ok":{"type":"logs","lines":[{"seq":1,"unix_ms":1700000000000,"level":"INFO","target":"peercove_poc::daemon","message":"トンネルを開始しました"}],"dropped":0}}"#
        );
    }

    /// `rtt_ms` は後方互換のため省略可能(旧デーモンの応答も読める)。
    #[test]
    fn peer_summary_rtt_is_optional() {
        let json = r#"{"public_key":"hSDwCYkwp1R0i33ctD73Wg2/Og0mOBr06uSpB6ipTmo=","rx_bytes":1,"tx_bytes":2}"#;
        let peer: PeerSummary = serde_json::from_str(json).unwrap();
        assert_eq!(peer.rtt_ms, None);
        assert!(!serde_json::to_string(&peer).unwrap().contains("rtt_ms"));
    }
}
