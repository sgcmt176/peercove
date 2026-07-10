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

use serde::{Deserialize, Serialize};

/// メッセージングの TCP ポート(各メンバーの仮想 IP 上)。
pub const MSG_PORT: u16 = 51822;
pub const MSG_VERSION: u32 = 1;

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
    FileOffer { id: String, name: String, size: u64 },
    /// 受信側: 受け入れる(この後に生バイト列が流れる)。
    FileAccept { id: String },
    /// 受信側: 受け取れない(不正なファイル名など)。
    FileReject { id: String, reason: String },
    /// 送信側: 本体を送り終えた。`sha256` は本体の SHA-256(16 進小文字)。
    FileHash { id: String, sha256: String },
    /// 受信側: 検証まで完了した。
    FileDone { id: String },
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
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"type":"file_offer","id":"a","name":"b.txt","size":3}"#
        );
    }
}
