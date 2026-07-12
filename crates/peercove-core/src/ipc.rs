//! デーモン制御用ローカル IPC のプロトコル型(M2-G1、ADR-0007)。
//!
//! - トランスポート: Windows = 名前付きパイプ / Linux = Unix ドメインソケット
//! - フレーミング: JSON Lines。リクエストは [`IpcEnvelope`]、応答は [`IpcReply`]
//! - 招待・削除などの設定ファイル操作は IPC に乗せない(ADR-0007)

use std::net::Ipv4Addr;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::msg::{ChatContext, ChatScope, GroupInfo};
use crate::proto::LedgerEntry;

/// Windows の名前付きパイプ名。
pub const PIPE_NAME: &str = r"\\.\pipe\peercove-daemon";

/// Linux の UDS パス(root 実行時の既定)。
pub const SOCKET_PATH_ROOT: &str = "/run/peercove.sock";

/// IPC プロトコルのバージョン。互換性を壊す変更で上げる。
/// - 1: M2-G1(単一トンネル)
/// - 2: M3-0b(複数トンネル。DaemonStatus/TunnelInfo/Stop の形が変わった)
pub const IPC_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IpcEnvelope {
    pub id: u64,
    pub req: IpcRequest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum IpcRequest {
    /// デーモンと稼働中トンネルの状態を返す。
    Status,
    /// ホストとしてトンネルを開始する(複数ネットワーク可 — ADR-0012)。
    StartHost { config: PathBuf, upnp: bool },
    /// メンバーとしてトンネルを開始する。
    StartMember { config: PathBuf },
    /// トンネルを停止する(デーモンは常駐継続)。
    /// `config` 省略時は「1 本だけ稼働中」の場合のみ止める(複数なら要指定)。
    Stop {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        config: Option<PathBuf>,
    },
    /// デーモンを終了する(トンネルが動いていればすべて停止してから)。
    Shutdown,
    /// デーモンが保持する直近のログを取り出す(M2-G5)。
    /// `after_seq` より後の行だけを返す(0 なら持っている分すべて)。
    Logs {
        #[serde(default)]
        after_seq: u64,
    },
    /// 稼働中トンネルのメンバーへファイルを送る(ADR-0015、M3-9)。
    /// 進捗は Status 応答の [`TunnelInfo::transfers`] で追う。
    /// 追加メソッドなので [`IPC_VERSION`] は上げない(旧デーモンは解析エラーを返す)。
    SendFile {
        config: PathBuf,
        /// 宛先メンバーの仮想 IP(direct のとき必須。network / group 宛の
        /// チャット内ファイル送信では省略 — M3-13d)。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        peer: Option<Ipv4Addr>,
        /// 送るファイルの絶対パス。
        path: PathBuf,
        /// チャット文脈(M3-13d)。付けると送受両側の履歴にファイルの
        /// エントリが記録される。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chat: Option<ChatContext>,
    },
    /// 稼働中トンネルのメンバーへチャットを送る(ADR-0016、M3-13)。
    /// 応答は [`IpcResponse::Chat`](送った 1 通だけを載せる)。
    /// 追加メソッドなので [`IPC_VERSION`] は上げない。
    ChatSend {
        config: PathBuf,
        scope: ChatScope,
        /// 宛先メンバーの仮想 IP(scope = direct のとき必須)。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        peer: Option<Ipv4Addr>,
        /// 宛先グループの ID(scope = group のとき必須 — M3-13c)。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group_id: Option<String>,
        text: String,
    },
    /// チャット履歴の差分を取り出す(`after_seq` より後のエントリ)。
    /// 新着の有無は Status 応答の [`TunnelInfo::chat_seq`] で判定する
    /// (新しいポーリング経路を作らない — ADR-0016)。
    ChatFetch {
        config: PathBuf,
        #[serde(default)]
        after_seq: u64,
    },
    /// グループを作る(ADR-0016、M3-13c)。`members` に自分は含めなくてよい
    /// (デーモンが必ず足す)。応答は [`IpcResponse::Group`]。
    GroupCreate {
        config: PathBuf,
        name: String,
        members: Vec<Ipv4Addr>,
    },
    /// グループの改名・メンバー追加(どちらも省略可。V1 に「他人の除外」はない
    /// — 抜けるのは本人の GroupLeave だけ)。
    GroupUpdate {
        config: PathBuf,
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        add: Vec<Ipv4Addr>,
    },
    /// 自分がグループから抜ける(履歴とグループ情報はローカルに残る)。
    GroupLeave { config: PathBuf, id: String },
    /// (member のみ)デバイス鍵のローテーションを要求する(ADR-0020、M3-11)。
    /// 応答は Done(受理のみ)。実際の更新はコントロールチャネル経由で
    /// 非同期に行われ、完了時に数秒の再接続が発生する。
    /// 追加メソッドなので [`IPC_VERSION`] は上げない。
    RotateKey { config: PathBuf },
    /// (member のみ)自分の DNS 名の変更を要求する(ADR-0021、M3-14a)。
    /// デーモンがコントロールチャネルでホストへ届け、検証・適用の結果を
    /// 待って返す(成功 = Done / 拒否・タイムアウト = Err に理由)。
    /// ホスト自身・ホストから見た各メンバーの変更は設定ファイル操作
    /// (peercove-ops)で行い、IPC には乗せない(ADR-0007)。
    /// 追加メソッドなので [`IPC_VERSION`] は上げない。
    SetDnsName { config: PathBuf, name: String },
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
    /// SendFile への応答。`id` で [`TunnelInfo::transfers`] から進捗を引ける。
    Transfer { id: String },
    /// ChatSend / ChatFetch への応答(ADR-0016、M3-13)。`seq` は履歴全体の
    /// 最新 seq。1 応答に載る件数・バイト数には上限があるため、`messages` の
    /// 末尾が `seq` に達するまで ChatFetch を繰り返して差分を取り切る。
    Chat {
        seq: u64,
        messages: Vec<ChatMessageInfo>,
    },
    /// GroupCreate / GroupUpdate への応答(作成・更新後のグループ全量)。
    Group { group: GroupInfo },
}

/// 1 応答で返すチャットの上限(IPC の 1 行上限 256 KiB に収めるため、
/// 件数と本文合計バイトの両方で打ち切る)。
pub const MAX_CHAT_MESSAGES_PER_REPLY: usize = 200;
pub const MAX_CHAT_BYTES_PER_REPLY: usize = 128 * 1024;

/// チャット履歴の 1 通(ADR-0016、M3-13)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessageInfo {
    /// 履歴内の単調増加の通し番号(ChatFetch の `after_seq` に使う)。
    pub seq: u64,
    /// メッセージ ID(送信側が発行。認証には使わない)。
    pub id: String,
    pub scope: ChatScope,
    /// (group のみ)宛先グループの ID(M3-13c)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    /// 送信者の仮想 IP(自分が送った通は自分の IP)。
    pub from: Ipv4Addr,
    /// (direct のみ)宛先の仮想 IP。network / group 宛は `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<Ipv4Addr>,
    pub text: String,
    /// 送信側時計の UNIX ミリ秒。
    pub sent_at: u64,
    /// どの宛先にも届かなかった(揮発 — デーモン再起動で消える)。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub failed: bool,
    /// チャット内ファイル送信のエントリ(M3-13d)。付いていれば `text` は空。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<ChatFileInfo>,
    /// グループ操作(作成・追加・退出・改名)のお知らせ(2026-07-11 検証 FB)。
    /// `text` が本文で、UI は吹き出しでなく中央の 1 行として表示する。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub system: bool,
}

/// チャット履歴に記録するファイル送信の情報(ADR-0016、M3-13d)。
/// 実体は従来どおり受信ボックス(`name` は受信側では保存された実ファイル名)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatFileInfo {
    pub name: String,
    pub size: u64,
    /// 対応する [`TransferInfo::id`](送信側は宛先ごとに 1 つ)。UI が
    /// 進捗バーに使う。転送一覧から流れたら進捗なしで表示する。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transfers: Vec<String>,
    /// この端末でのファイルの場所(送信側 = 元ファイル / 受信側 = 受信
    /// ボックス内)。UI が画像・動画などのインラインプレビューに使う
    /// (2026-07-11 検証 FB)。移動・削除済みならプレビューされないだけ。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
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

/// デーモンの状態モデル(ADR-0012: 複数ネットワーク同時稼働)。
/// 空 = 待機中。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DaemonStatus {
    /// デーモン側の [`IPC_VERSION`]。旧デーモン(v1)の応答には無いので 0 になる。
    /// UI/CLI はこれで**バージョン不一致を明示検出**する(旧デーモン + 新 UI は
    /// 状態が「全部停止中」に見える事故が実機で起きた)。
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub tunnels: Vec<TunnelInfo>,
}

/// トンネルの役割。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelRole {
    Host,
    Member,
}

/// メンバー間直接通信の経路状態(ADR-0013、M3-4)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectStatus {
    /// 直接ピアを張ってハンドシェイク待ち(確立中)。
    Trying,
    /// 直接通信中。
    Direct,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TunnelInfo {
    pub config: PathBuf,
    /// ネットワーク名(ADR-0012。設定の network_name、旧設定は既定名)。
    pub network: String,
    pub role: TunnelRole,
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
    /// (member のみ)相手の仮想 IP → 直接経路の状態(ADR-0013、M3-4)。
    /// 載っていない相手はホスト経由(中継)。旧デーモンの応答には無い(空)。
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub direct: std::collections::HashMap<Ipv4Addr, DirectStatus>,
    /// ファイル転送の進捗(ADR-0015、M3-9)。実行中 + 直近の完了/失敗分。
    /// 旧デーモンの応答には無い(空)。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transfers: Vec<TransferInfo>,
    /// チャット履歴の最新 seq(ADR-0016、M3-13)。0 = 履歴なし。
    /// UI/CLI はこれが進んだときだけ ChatFetch する。旧デーモンの応答には無い(0)。
    #[serde(default, skip_serializing_if = "u64_is_zero")]
    pub chat_seq: u64,
    /// 既知のグループ(ADR-0016、M3-13c)。自分が抜けた/外されたグループも
    /// 含む(UI が履歴の表示名に使う — 会話リストからは隠す)。
    /// 旧デーモンの応答には無い(空)。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<GroupInfo>,
}

fn u64_is_zero(value: &u64) -> bool {
    *value == 0
}

/// ファイル転送の向き(自分から見て)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferDirection {
    Send,
    Recv,
}

/// ファイル転送 1 件の進捗(ADR-0015、M3-9)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferInfo {
    pub id: String,
    pub direction: TransferDirection,
    /// 相手の仮想 IP。
    pub peer: Ipv4Addr,
    /// ファイル名(パスは含めない)。
    pub name: String,
    /// 全体のバイト数。
    pub size: u64,
    /// 転送済みバイト数。
    pub transferred: u64,
    /// 完了した(エラーなら `error` が入る)。
    pub done: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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
            IpcRequest::Stop { config: None },
            IpcRequest::Stop {
                config: Some(PathBuf::from("host.toml")),
            },
            IpcRequest::Shutdown,
            IpcRequest::Logs { after_seq: 42 },
            IpcRequest::SetDnsName {
                config: PathBuf::from("member.toml"),
                name: "yamada-dev".to_string(),
            },
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
                result: IpcResult::Ok(IpcResponse::Status(DaemonStatus {
                    version: IPC_VERSION,
                    tunnels: vec![],
                })),
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
            result: IpcResult::Ok(IpcResponse::Status(DaemonStatus {
                version: IPC_VERSION,
                tunnels: vec![],
            })),
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"id":7,"ok":{"type":"status","version":2,"tunnels":[]}}"#
        );

        // 旧デーモン(v1)の応答は version 欠落 → 0 として読める(不一致検出用)
        let old: IpcReply =
            serde_json::from_str(r#"{"id":7,"ok":{"type":"status","state":"idle"}}"#).unwrap();
        match old.result {
            IpcResult::Ok(IpcResponse::Status(status)) => {
                assert_eq!(status.version, 0);
                assert!(status.tunnels.is_empty());
            }
            other => panic!("Status を期待: {other:?}"),
        }

        // Stop は config 省略時に旧形式と同じワイヤ表現になる
        let json = serde_json::to_string(&IpcEnvelope {
            id: 10,
            req: IpcRequest::Stop { config: None },
        })
        .unwrap();
        assert_eq!(json, r#"{"id":10,"req":{"method":"stop"}}"#);

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

    /// SendFile / Transfer / transfers(ADR-0015、M3-9)のワイヤ表現。
    /// transfers が空なら旧デーモンの応答とワイヤ表現が一致する(互換維持)。
    #[test]
    fn send_file_wire_format() {
        let json = serde_json::to_string(&IpcEnvelope {
            id: 11,
            req: IpcRequest::SendFile {
                config: PathBuf::from("host.toml"),
                peer: Some("10.83.19.3".parse().unwrap()),
                path: PathBuf::from("a.txt"),
                chat: None,
            },
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"id":11,"req":{"method":"send_file","config":"host.toml","peer":"10.83.19.3","path":"a.txt"}}"#,
            "chat なしは M3-9 と同じワイヤ表現(旧デーモン互換)"
        );

        // チャット文脈付き(M3-13d)。group 宛は peer を省略する
        let json = serde_json::to_string(&IpcEnvelope {
            id: 19,
            req: IpcRequest::SendFile {
                config: PathBuf::from("host.toml"),
                peer: None,
                path: PathBuf::from("a.txt"),
                chat: Some(ChatContext {
                    scope: ChatScope::Group,
                    group_id: Some("g1".to_string()),
                }),
            },
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"id":19,"req":{"method":"send_file","config":"host.toml","path":"a.txt","chat":{"scope":"group","group_id":"g1"}}}"#
        );

        let json = serde_json::to_string(&IpcReply {
            id: 11,
            result: IpcResult::Ok(IpcResponse::Transfer {
                id: "ab12".to_string(),
            }),
        })
        .unwrap();
        assert_eq!(json, r#"{"id":11,"ok":{"type":"transfer","id":"ab12"}}"#);

        let info = TransferInfo {
            id: "ab12".to_string(),
            direction: TransferDirection::Recv,
            peer: "10.83.19.3".parse().unwrap(),
            name: "a.txt".to_string(),
            size: 10,
            transferred: 4,
            done: false,
            error: None,
        };
        let json = serde_json::to_string(&info).unwrap();
        assert!(!json.contains("error"), "エラーなしなら省略: {json}");
        let parsed: TransferInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, info);

        // 旧デーモンの TunnelInfo(transfers なし)も読める
        let old = r#"{"config":"a.toml","network":"n","role":"host","address":"10.83.19.1"}"#;
        let parsed: TunnelInfo = serde_json::from_str(old).unwrap();
        assert!(parsed.transfers.is_empty());
        assert_eq!(parsed.chat_seq, 0, "chat_seq も省略可(旧デーモン互換)");
        assert!(parsed.groups.is_empty(), "groups も省略可(旧デーモン互換)");
    }

    /// ChatSend / ChatFetch / Chat 応答(ADR-0016、M3-13)のワイヤ表現。
    #[test]
    fn chat_wire_format() {
        let json = serde_json::to_string(&IpcEnvelope {
            id: 12,
            req: IpcRequest::ChatSend {
                config: PathBuf::from("host.toml"),
                scope: ChatScope::Direct,
                peer: Some("10.83.19.3".parse().unwrap()),
                group_id: None,
                text: "やあ".to_string(),
            },
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"id":12,"req":{"method":"chat_send","config":"host.toml","scope":"direct","peer":"10.83.19.3","text":"やあ"}}"#
        );

        // network 宛は peer を省略する
        let json = serde_json::to_string(&IpcEnvelope {
            id: 13,
            req: IpcRequest::ChatSend {
                config: PathBuf::from("host.toml"),
                scope: ChatScope::Network,
                peer: None,
                group_id: None,
                text: "全体".to_string(),
            },
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"id":13,"req":{"method":"chat_send","config":"host.toml","scope":"network","text":"全体"}}"#
        );

        // group 宛は group_id を付ける(M3-13c)
        let json = serde_json::to_string(&IpcEnvelope {
            id: 15,
            req: IpcRequest::ChatSend {
                config: PathBuf::from("host.toml"),
                scope: ChatScope::Group,
                peer: None,
                group_id: Some("g1".to_string()),
                text: "グループ".to_string(),
            },
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"id":15,"req":{"method":"chat_send","config":"host.toml","scope":"group","group_id":"g1","text":"グループ"}}"#
        );

        let json = serde_json::to_string(&IpcEnvelope {
            id: 14,
            req: IpcRequest::ChatFetch {
                config: PathBuf::from("host.toml"),
                after_seq: 7,
            },
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"id":14,"req":{"method":"chat_fetch","config":"host.toml","after_seq":7}}"#
        );

        let message = ChatMessageInfo {
            seq: 8,
            id: "c1".to_string(),
            scope: ChatScope::Direct,
            group_id: None,
            from: "10.83.19.1".parse().unwrap(),
            to: Some("10.83.19.3".parse().unwrap()),
            text: "やあ".to_string(),
            sent_at: 1_700_000_000_000,
            failed: false,
            file: None,
            system: false,
        };
        let json = serde_json::to_string(&IpcReply {
            id: 14,
            result: IpcResult::Ok(IpcResponse::Chat {
                seq: 8,
                messages: vec![message.clone()],
            }),
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"id":14,"ok":{"type":"chat","seq":8,"messages":[{"seq":8,"id":"c1","scope":"direct","from":"10.83.19.1","to":"10.83.19.3","text":"やあ","sent_at":1700000000000}]}}"#,
            "failed = false と to = None は省略される"
        );
        let parsed: IpcReply = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed.result,
            IpcResult::Ok(IpcResponse::Chat {
                seq: 8,
                messages: vec![message]
            })
        );

        // network 宛 + 失敗フラグ
        let message = ChatMessageInfo {
            seq: 9,
            id: "c2".to_string(),
            scope: ChatScope::Network,
            group_id: None,
            from: "10.83.19.1".parse().unwrap(),
            to: None,
            text: "x".to_string(),
            sent_at: 1,
            failed: true,
            file: None,
            system: false,
        };
        let json = serde_json::to_string(&message).unwrap();
        assert!(!json.contains("\"to\""), "network 宛は to を省略: {json}");
        assert!(
            !json.contains("group_id"),
            "group 宛以外は group_id を省略: {json}"
        );
        assert!(json.contains(r#""failed":true"#), "{json}");
        let parsed: ChatMessageInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, message);

        // group 宛 + グループ操作(M3-13c)のワイヤ表現
        let message = ChatMessageInfo {
            seq: 10,
            id: "c3".to_string(),
            scope: ChatScope::Group,
            group_id: Some("g1".to_string()),
            from: "10.83.19.1".parse().unwrap(),
            to: None,
            text: "x".to_string(),
            sent_at: 1,
            failed: false,
            file: None,
            system: false,
        };
        let json = serde_json::to_string(&message).unwrap();
        assert!(json.contains(r#""group_id":"g1""#), "{json}");
        assert!(!json.contains("file"), "ファイルなしは file を省略: {json}");
        let parsed: ChatMessageInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, message);

        // チャット内ファイル送信のエントリ(M3-13d)
        let message = ChatMessageInfo {
            seq: 11,
            id: "c4".to_string(),
            scope: ChatScope::Direct,
            group_id: None,
            from: "10.83.19.1".parse().unwrap(),
            to: Some("10.83.19.3".parse().unwrap()),
            text: String::new(),
            sent_at: 1,
            failed: false,
            file: Some(ChatFileInfo {
                name: "写真.jpg".to_string(),
                size: 1024,
                transfers: vec!["t1".to_string()],
                path: Some(PathBuf::from("C:/inbox/写真.jpg")),
            }),
            system: false,
        };
        let json = serde_json::to_string(&message).unwrap();
        assert!(
            json.contains(
                r#""file":{"name":"写真.jpg","size":1024,"transfers":["t1"],"path":"C:/inbox/写真.jpg"}"#
            ),
            "{json}"
        );
        let parsed: ChatMessageInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, message);

        // グループ操作のお知らせ(system)。通常のメッセージでは省略される
        let message = ChatMessageInfo {
            seq: 12,
            id: "c5".to_string(),
            scope: ChatScope::Group,
            group_id: Some("g1".to_string()),
            from: "10.83.19.1".parse().unwrap(),
            to: None,
            text: "グループ「開発」を作成しました".to_string(),
            sent_at: 1,
            failed: false,
            file: None,
            system: true,
        };
        let json = serde_json::to_string(&message).unwrap();
        assert!(json.contains(r#""system":true"#), "{json}");
        let parsed: ChatMessageInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, message);

        let json = serde_json::to_string(&IpcEnvelope {
            id: 16,
            req: IpcRequest::GroupCreate {
                config: PathBuf::from("host.toml"),
                name: "開発".to_string(),
                members: vec!["10.83.19.3".parse().unwrap()],
            },
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"id":16,"req":{"method":"group_create","config":"host.toml","name":"開発","members":["10.83.19.3"]}}"#
        );
        let json = serde_json::to_string(&IpcEnvelope {
            id: 17,
            req: IpcRequest::GroupUpdate {
                config: PathBuf::from("host.toml"),
                id: "g1".to_string(),
                name: None,
                add: vec![],
            },
        })
        .unwrap();
        assert_eq!(
            json, r#"{"id":17,"req":{"method":"group_update","config":"host.toml","id":"g1"}}"#,
            "name / add は省略可"
        );
        let json = serde_json::to_string(&IpcEnvelope {
            id: 18,
            req: IpcRequest::GroupLeave {
                config: PathBuf::from("host.toml"),
                id: "g1".to_string(),
            },
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"id":18,"req":{"method":"group_leave","config":"host.toml","id":"g1"}}"#
        );
        let json = serde_json::to_string(&IpcReply {
            id: 16,
            result: IpcResult::Ok(IpcResponse::Group {
                group: GroupInfo {
                    id: "g1".to_string(),
                    name: "開発".to_string(),
                    members: vec!["10.83.19.1".parse().unwrap()],
                    revision: 1,
                    updated_by: "10.83.19.1".parse().unwrap(),
                },
            }),
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"id":16,"ok":{"type":"group","group":{"id":"g1","name":"開発","members":["10.83.19.1"],"revision":1,"updated_by":"10.83.19.1"}}}"#
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
