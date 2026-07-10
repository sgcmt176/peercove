//! トンネル内メッセージング基盤のプロトコル型(ADR-0015、M3-9/M3-13)。
//!
//! - トランスポート: 各デーモンが自分の仮想 IP の TCP [`MSG_PORT`] で待受け、
//!   送信側が相手の仮想 IP へ直接接続する(P2P)。経路選択は IP 層に任せる
//!   (ハブ&スポーク中継でも直接経路(ADR-0013)でも同じコードで動く)
//! - 認証: トンネル内なので WG により暗号化・認証済み。接続元の仮想 IP が
//!   送信者の身元になり、受信側は台帳のメンバー仮想 IP と照合する
//! - フレーミング: JSON Lines の制御フレーム + ファイル本体だけ生バイナリ
//!   (1 論理操作 = 1 短命 TCP 接続)
//! - 互換性: 旧デーモンにはリスナーが無く接続拒否になるだけ。
//!   互換性を壊す変更は [`MSG_VERSION`] を上げる

use std::net::Ipv4Addr;

use serde::{Deserialize, Serialize};

/// メッセージングの TCP ポート(各メンバーの仮想 IP 上)。
pub const MSG_PORT: u16 = 51822;
pub const MSG_VERSION: u32 = 1;

/// チャット本文の上限(バイト)。フレームの 1 行上限(64 KiB)と IPC の
/// 1 応答上限(256 KiB)に収まるよう、送信側・受信側の両方で検査する。
pub const MAX_CHAT_TEXT_BYTES: usize = 8 * 1024;

/// グループ名の上限(バイト)。フレームの 1 行上限に収める(M3-13c)。
pub const MAX_GROUP_NAME_BYTES: usize = 256;
/// グループのメンバー数上限(小規模ネットワーク前提の安全弁)。
pub const MAX_GROUP_MEMBERS: usize = 250;

/// チャットの宛先種別(ADR-0016)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatScope {
    /// 1:1(特定メンバー宛)。
    Direct,
    /// ネットワーク全体宛(送信側がオンラインメンバー全員へ個別送信)。
    Network,
    /// 任意グループ宛(M3-13c)。`group_id` で対象を特定し、送信側が
    /// オンラインのグループメンバー全員へ個別送信する。
    Group,
}

/// ファイル送信のチャット文脈(ADR-0016、M3-13d)。`file_offer` に付けると
/// 送受両側がチャット履歴にファイルのエントリを記録し、UI が会話内に
/// ファイルバブルを出す。旧デーモン(M3-9a)は未知フィールドとして無視する
/// (= チャット文脈なしの通常ファイル受信になるだけ)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatContext {
    pub scope: ChatScope,
    /// scope = group のときだけ付く。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
}

/// グループ情報(ADR-0016、M3-13c)。**全量 + リビジョン**で伝搬し、受信側は
/// revision が手元より新しければ丸ごと置換する(同値は updated_by の IP 比較で
/// 決着 = 最新リビジョン勝ち)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupInfo {
    /// グループ ID(作成者が発行。改名しても変わらない)。
    pub id: String,
    pub name: String,
    /// メンバーの仮想 IP(作成者自身を含む)。
    pub members: Vec<Ipv4Addr>,
    /// 更新のたびに +1。大きい方が勝つ。
    pub revision: u64,
    /// この revision を作ったメンバーの仮想 IP(同値の決着に使う)。
    pub updated_by: Ipv4Addr,
}

/// 1 行 1 メッセージでやり取りする制御フレーム。
///
/// ファイル転送の流れ(ADR-0015):
/// `Hello` → `FileOffer` → `FileAccept`(または `FileReject`)→ 生バイト列
/// (`size` バイト)→ `FileHash`(送信側が転送しながら計算)→ 受信側が検証して
/// `FileDone`。ハッシュを事前でなく後置にするのは、送信側が巨大ファイルを
/// 2 回読まずに済むようにするため。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MsgFrame {
    /// 接続直後に双方の前提を揃える(送信側 → 受信側)。
    Hello { version: u32 },
    /// ファイル送信の申し出。`name` はファイル名のみ(パスは含めない)。
    /// `chat` はチャット内ファイル送信の文脈(M3-13d、任意)。
    FileOffer {
        id: String,
        name: String,
        size: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        chat: Option<ChatContext>,
    },
    /// 受信側: 受け入れる(この後に生バイト列が流れる)。
    FileAccept { id: String },
    /// 受信側: 受け取れない(不正なファイル名など)。
    FileReject { id: String, reason: String },
    /// 送信側: 本体を送り終えた。`sha256` は本体の SHA-256(16 進小文字)。
    FileHash { id: String, sha256: String },
    /// 受信側: 検証まで完了した。
    FileDone { id: String },
    /// チャット 1 通(ADR-0016、M3-13)。`sent_at` は送信側時計の UNIX ミリ秒。
    /// network / group 宛も 1 通ずつ個別送信される(scope で区別する)。
    /// `group_id` は scope = group のときだけ付く(M3-13c)。
    Chat {
        id: String,
        scope: ChatScope,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        group_id: Option<String>,
        text: String,
        sent_at: u64,
    },
    /// 受信側: チャットを受け取り履歴に記録した。
    ChatAck { id: String },
    /// グループ情報の伝搬(ADR-0016、M3-13c)。作成・メンバー追加・退出・改名の
    /// いずれも全量の置換として送る。旧デーモンはこのフレームを解析できず
    /// 切断するだけ(グループ機能が使えないだけで他は壊れない)。
    GroupUpdate { group: GroupInfo },
    /// 受信側: グループ情報を取り込んだ(手元の方が新しく捨てた場合も返す)。
    GroupAck { id: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_json_roundtrip() {
        let frames = vec![
            MsgFrame::Hello {
                version: MSG_VERSION,
            },
            MsgFrame::FileOffer {
                id: "t1".to_string(),
                name: "写真.jpg".to_string(),
                size: 1024,
                chat: None,
            },
            MsgFrame::FileOffer {
                id: "t2".to_string(),
                name: "資料.pdf".to_string(),
                size: 2048,
                chat: Some(ChatContext {
                    scope: ChatScope::Group,
                    group_id: Some("g1".to_string()),
                }),
            },
            MsgFrame::FileAccept {
                id: "t1".to_string(),
            },
            MsgFrame::FileReject {
                id: "t1".to_string(),
                reason: "ファイル名が不正です".to_string(),
            },
            MsgFrame::FileHash {
                id: "t1".to_string(),
                sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                    .to_string(),
            },
            MsgFrame::FileDone {
                id: "t1".to_string(),
            },
            MsgFrame::Chat {
                id: "c1".to_string(),
                scope: ChatScope::Direct,
                group_id: None,
                text: "こんにちは 🎉".to_string(),
                sent_at: 1_700_000_000_000,
            },
            MsgFrame::Chat {
                id: "c2".to_string(),
                scope: ChatScope::Network,
                group_id: None,
                text: "全体宛".to_string(),
                sent_at: 1,
            },
            MsgFrame::Chat {
                id: "c3".to_string(),
                scope: ChatScope::Group,
                group_id: Some("g1".to_string()),
                text: "グループ宛".to_string(),
                sent_at: 2,
            },
            MsgFrame::ChatAck {
                id: "c1".to_string(),
            },
            MsgFrame::GroupUpdate {
                group: GroupInfo {
                    id: "g1".to_string(),
                    name: "開発チーム".to_string(),
                    members: vec!["10.0.0.1".parse().unwrap(), "10.0.0.2".parse().unwrap()],
                    revision: 3,
                    updated_by: "10.0.0.1".parse().unwrap(),
                },
            },
            MsgFrame::GroupAck {
                id: "g1".to_string(),
            },
        ];
        for frame in frames {
            let json = serde_json::to_string(&frame).unwrap();
            assert!(!json.contains('\n'), "JSON Lines 用に 1 行であること");
            let parsed: MsgFrame = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, frame);
        }
    }

    /// ワイヤ表現(タグ名)を固定する。変えると旧バージョンと会話できなくなる。
    #[test]
    fn wire_format_is_stable() {
        let json = serde_json::to_string(&MsgFrame::Hello { version: 1 }).unwrap();
        assert_eq!(json, r#"{"type":"hello","version":1}"#);
        let json = serde_json::to_string(&MsgFrame::FileOffer {
            id: "a".to_string(),
            name: "b.txt".to_string(),
            size: 3,
            chat: None,
        })
        .unwrap();
        assert_eq!(
            json, r#"{"type":"file_offer","id":"a","name":"b.txt","size":3}"#,
            "chat なしは M3-9a と同じワイヤ表現(旧デーモン互換)"
        );
        let json = serde_json::to_string(&MsgFrame::FileOffer {
            id: "a".to_string(),
            name: "b.txt".to_string(),
            size: 3,
            chat: Some(ChatContext {
                scope: ChatScope::Direct,
                group_id: None,
            }),
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"type":"file_offer","id":"a","name":"b.txt","size":3,"chat":{"scope":"direct"}}"#
        );
        let json = serde_json::to_string(&MsgFrame::Chat {
            id: "a".to_string(),
            scope: ChatScope::Direct,
            group_id: None,
            text: "hi".to_string(),
            sent_at: 5,
        })
        .unwrap();
        assert_eq!(
            json, r#"{"type":"chat","id":"a","scope":"direct","text":"hi","sent_at":5}"#,
            "group_id なしは M3-13a と同じワイヤ表現(旧デーモン互換)"
        );
        let json = serde_json::to_string(&MsgFrame::Chat {
            id: "a".to_string(),
            scope: ChatScope::Group,
            group_id: Some("g1".to_string()),
            text: "hi".to_string(),
            sent_at: 5,
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"type":"chat","id":"a","scope":"group","group_id":"g1","text":"hi","sent_at":5}"#
        );
        let json = serde_json::to_string(&MsgFrame::ChatAck {
            id: "a".to_string(),
        })
        .unwrap();
        assert_eq!(json, r#"{"type":"chat_ack","id":"a"}"#);
        let json = serde_json::to_string(&MsgFrame::GroupUpdate {
            group: GroupInfo {
                id: "g1".to_string(),
                name: "n".to_string(),
                members: vec!["10.0.0.1".parse().unwrap()],
                revision: 2,
                updated_by: "10.0.0.1".parse().unwrap(),
            },
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"type":"group_update","group":{"id":"g1","name":"n","members":["10.0.0.1"],"revision":2,"updated_by":"10.0.0.1"}}"#
        );
        let json = serde_json::to_string(&MsgFrame::GroupAck {
            id: "g1".to_string(),
        })
        .unwrap();
        assert_eq!(json, r#"{"type":"group_ack","id":"g1"}"#);
    }
}
