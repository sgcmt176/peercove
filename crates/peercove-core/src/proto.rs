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
    /// この端末が Hello で広告した追加機能。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
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
            capabilities: vec![],
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
            },
            ControlMessage::Ledger {
                members: vec![entry()],
                dns_records: vec![crate::dns::DnsRecord {
                    name: "nas".to_string(),
                    ip: "100.100.42.50".parse().unwrap(),
                    scheme: None,
                    port: None,
                    health: None,
                }],
                cname_records: vec![crate::dns::CnameRecord {
                    name: "docs".to_string(),
                    target: "example.com".to_string(),
                    resolved_ip: None,
                    scheme: None,
                    port: None,
                    health: None,
                }],
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
            }
        );

        // dns_records / cname_records が空なら旧バージョンとワイヤ表現が一致する(互換維持)
        let json = serde_json::to_string(&ControlMessage::Ledger {
            members: vec![],
            dns_records: vec![],
            cname_records: vec![],
        })
        .unwrap();
        assert_eq!(json, r#"{"type":"ledger","members":[]}"#);

        // サービス情報なしのレコードは M3-14c より前の JSON と完全に一致する。
        let json = serde_json::to_string(&crate::dns::DnsRecord {
            name: "nas".to_string(),
            ip: "100.100.42.50".parse().unwrap(),
            scheme: None,
            port: None,
            health: None,
        })
        .unwrap();
        assert_eq!(json, r#"{"name":"nas","ip":"100.100.42.50"}"#);

        // 旧ホストからの台帳(dns_records なし)も読める
        let old: ControlMessage =
            serde_json::from_str(r#"{"type":"ledger","members":[]}"#).unwrap();
        assert_eq!(
            old,
            ControlMessage::Ledger {
                members: vec![],
                dns_records: vec![],
                cname_records: vec![],
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
