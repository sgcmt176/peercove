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

/// 1 行 1 メッセージでやり取りする制御メッセージ。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlMessage {
    /// member → host: 接続直後に名乗る。
    Hello {
        version: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
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
    },
    /// host → member: あなたは削除された(以後トンネルは通らない)。
    Removed { message: String },
    /// 双方向: RTT 計測(M2-G5)。受け取った側は同じ nonce で [`ControlMessage::Pong`] を返す。
    ///
    /// 追加メッセージなので [`PROTO_VERSION`] は上げない。ping を知らない旧実装は
    /// 解析に失敗して黙って無視するため、RTT が測れないだけで通信は壊れない。
    Ping { nonce: u64 },
    /// 双方向: [`ControlMessage::Ping`] への応答。
    Pong { nonce: u64 },
}

/// 台帳の 1 エントリ。ホスト自身も 1 エントリとして含める。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub ip: Ipv4Addr,
    pub public_key: PublicKey,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::PrivateKey;

    fn entry() -> LedgerEntry {
        LedgerEntry {
            name: Some("alice".to_string()),
            ip: "100.100.42.2".parse().unwrap(),
            public_key: PrivateKey::generate().public_key(),
            online: true,
            is_host: false,
            endpoint: Some("203.0.113.5:51820".parse().unwrap()),
            endpoint_age_secs: Some(3),
        }
    }

    #[test]
    fn control_message_json_roundtrip() {
        let messages = vec![
            ControlMessage::Hello {
                version: PROTO_VERSION,
                name: Some("alice".to_string()),
            },
            ControlMessage::Ledger {
                members: vec![entry()],
                dns_records: vec![crate::dns::DnsRecord {
                    name: "nas".to_string(),
                    ip: "100.100.42.50".parse().unwrap(),
                }],
            },
            ControlMessage::Removed {
                message: "ホストにより削除されました".to_string(),
            },
            ControlMessage::Ping { nonce: 7 },
            ControlMessage::Pong { nonce: 7 },
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
        })
        .unwrap();
        assert_eq!(json, r#"{"type":"hello","version":1}"#);

        // dns_records が空なら旧バージョンとワイヤ表現が一致する(互換維持)
        let json = serde_json::to_string(&ControlMessage::Ledger {
            members: vec![],
            dns_records: vec![],
        })
        .unwrap();
        assert_eq!(json, r#"{"type":"ledger","members":[]}"#);

        // 旧ホストからの台帳(dns_records なし)も読める
        let old: ControlMessage =
            serde_json::from_str(r#"{"type":"ledger","members":[]}"#).unwrap();
        assert_eq!(
            old,
            ControlMessage::Ledger {
                members: vec![],
                dns_records: vec![],
            }
        );

        let json = serde_json::to_string(&ControlMessage::Ping { nonce: 1 }).unwrap();
        assert_eq!(json, r#"{"type":"ping","nonce":1}"#);
        let json = serde_json::to_string(&ControlMessage::Pong { nonce: 1 }).unwrap();
        assert_eq!(json, r#"{"type":"pong","nonce":1}"#);
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
