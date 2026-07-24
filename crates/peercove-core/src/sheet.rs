//! 共有シート(Excel ライク表)の共有型(ADR-0054、M6 G-2)。
//!
//! 共有メモ・共有スケジュール表([`crate::schedule`])の基盤(ホスト正本 DB・
//! コントロールチャネル配信・読み取りキャッシュ)を転用する。ストレージ実装は
//! `peercove-memo` crate。**シート名・セル値はチャット本文と同格の秘匿対象 —
//! ログ・標準出力へ出さない**(id・件数・行列番号は可)。
//!
//! [`crate::memo::SharedMemoOp`] / `SharedMemoReply` / `SharedMemoEvent` へ
//! additive な `Sheet` variant として相乗りする(capability は `shared_memo`
//! のまま。シート未対応の旧クライアントは該当行を解析失敗で無視する = 既存の
//! 互換モデル、ADR-0054 決定 6)。
//!
//! **アドレスは固定**(行・列の挿入/削除による繰り上げは V1 では持たない、
//! ADR-0054 決定 3)。セルの値は空文字への更新でセル削除を表す。

use serde::{Deserialize, Serialize};

/// シート名の上限(文字数)。
pub const MAX_SHEET_NAME_CHARS: usize = 100;

/// セル値の上限(バイト、UTF-8)。
pub const MAX_CELL_VALUE_BYTES: usize = 4 * 1024;

/// 1 ネットワークあたりのシート枚数上限。
pub const MAX_SHEETS: u32 = 100;

/// 1 シートあたりの非空セル数上限。
pub const MAX_SHEET_CELLS: u32 = 10_000;

/// 行番号の上限(0-indexed。この値未満のみ許可)。
pub const MAX_SHEET_ROWS: u32 = 1_000;

/// 列番号の上限(0-indexed。この値未満のみ許可)。
pub const MAX_SHEET_COLS: u32 = 200;

/// 共有シートのメタ情報(セルは含まない)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SheetMeta {
    pub id: String,
    pub name: String,
    /// 所有者(作成者)の member_id。空文字 = ホスト。
    #[serde(default)]
    pub owner_id: String,
    #[serde(default)]
    pub owner_name: String,
    pub created_at: u64,
    pub updated_at: u64,
    /// 受信者視点: 改名・削除できるか(作成者 + ホスト、ADR-0054 決定 5)。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub can_manage: bool,
}

/// 1 セル分(疎な格納。非空セルだけが存在する)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SheetCell {
    pub row: u32,
    pub col: u32,
    pub value: String,
    /// 単調増加リビジョン(CAS 用、ADR-0054 決定 4)。
    pub revision: u64,
    pub updated_by: String,
    pub updated_at: u64,
}

/// セル書き込み 1 件分(バッチの要素)。`base_revision` = 0 は新規セル想定
/// (既存セルに対して 0 を送ると競合扱いになる)。値が空文字ならセル削除。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellWrite {
    pub row: u32,
    pub col: u32,
    pub value: String,
    #[serde(default)]
    pub base_revision: u64,
}

/// 共有シートへの操作。全員が閲覧・セル編集できる。シートの作成・改名・
/// 削除は作成者 + ホストのみ(ADR-0054 決定 5)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SheetOp {
    /// シート一覧(メタのみ、V1。件数上限 [`MAX_SHEETS`] のため全量で良い)。
    List,
    /// 1 シートの全非空セル。
    Cells { sheet_id: String },
    /// 新規作成(全員可)。応答は作成したシートの [`SheetReply::Sheet`]。
    Create {
        #[serde(default)]
        name: String,
    },
    /// 改名(作成者・ホストのみ)。応答は [`SheetReply::Sheet`]。
    Rename {
        sheet_id: String,
        #[serde(default)]
        name: String,
    },
    /// 削除(作成者・ホストのみ。セルもまとめて削除)。
    Delete { sheet_id: String },
    /// セルのバッチ書き込み(全員可。貼り付け(TSV)も 1 バッチ)。
    /// 応答は [`SheetReply::WriteResult`](部分失敗をサポートする)。
    Write {
        sheet_id: String,
        cells: Vec<CellWrite>,
    },
}

/// [`SheetOp`] への応答。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SheetReply {
    /// List への応答。
    Sheets {
        sheets: Vec<SheetMeta>,
        /// (メンバーのみ)ホスト未接続のキャッシュ応答 = 読み取り専用。
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        offline: bool,
    },
    /// Cells への応答。
    CellsData {
        sheet_id: String,
        cells: Vec<SheetCell>,
        /// (メンバーのみ)ホスト未接続のキャッシュ応答 = 読み取り専用。
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        offline: bool,
    },
    /// Create / Rename への応答。
    Sheet { sheet: SheetMeta },
    /// Write の結果。conflicts は base_revision 不一致で適用しなかった
    /// セルの現在値(UI が最新値を提示する、ADR-0054 決定 4)。
    WriteResult {
        applied: u32,
        conflicts: Vec<SheetCell>,
    },
    /// Delete への応答。
    Done,
    /// 拒否・上限超過など(コントロールチャネル経由の応答用)。
    Err { message: String },
}

/// ホスト → メンバーのリアルタイム配信。**閲覧権限のフィルタなし**
/// (全員閲覧、ADR-0054 決定 5)。受信者ごとの `can_manage` は SheetChanged
/// の配信時に計算し直して詰める。tag は `kind`(`event` は
/// [`crate::memo::SharedMemoEvent`] 側のタグと衝突するため使えない)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SheetEventMsg {
    /// シートの作成・改名。
    SheetChanged { sheet: SheetMeta },
    /// シートの削除。キャッシュから(セルも)消すこと。
    SheetRemoved { sheet_id: String },
    /// セルのバッチ変更(削除セルは value = "" で表現)。
    CellsChanged {
        sheet_id: String,
        cells: Vec<SheetCell>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ワイヤ表現(UI・モバイルが依存)。追加フィールドはすべて省略可能で、
    /// 旧バージョンとの相互無視が成り立つことを固定する。
    #[test]
    fn sheet_op_wire_format() {
        let op = SheetOp::List;
        assert_eq!(serde_json::to_string(&op).unwrap(), r#"{"op":"list"}"#);
        assert_eq!(
            serde_json::from_str::<SheetOp>(r#"{"op":"list"}"#).unwrap(),
            op
        );

        let op = SheetOp::Cells {
            sheet_id: "s1".to_string(),
        };
        assert_eq!(
            serde_json::to_string(&op).unwrap(),
            r#"{"op":"cells","sheet_id":"s1"}"#
        );
        assert_eq!(
            serde_json::from_str::<SheetOp>(&serde_json::to_string(&op).unwrap()).unwrap(),
            op
        );

        let op = SheetOp::Create {
            name: "在庫表".to_string(),
        };
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(json, r#"{"op":"create","name":"在庫表"}"#);
        assert_eq!(serde_json::from_str::<SheetOp>(&json).unwrap(), op);

        let op = SheetOp::Rename {
            sheet_id: "s1".to_string(),
            name: "改題".to_string(),
        };
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(serde_json::from_str::<SheetOp>(&json).unwrap(), op);

        let op = SheetOp::Delete {
            sheet_id: "s1".to_string(),
        };
        assert_eq!(
            serde_json::to_string(&op).unwrap(),
            r#"{"op":"delete","sheet_id":"s1"}"#
        );
        assert_eq!(
            serde_json::from_str::<SheetOp>(&serde_json::to_string(&op).unwrap()).unwrap(),
            op
        );

        let op = SheetOp::Write {
            sheet_id: "s1".to_string(),
            cells: vec![CellWrite {
                row: 0,
                col: 0,
                value: "10".to_string(),
                base_revision: 0,
            }],
        };
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(serde_json::from_str::<SheetOp>(&json).unwrap(), op);

        let sheet = SheetMeta {
            id: "s1".to_string(),
            name: "在庫表".to_string(),
            owner_id: String::new(),
            owner_name: "ホスト".to_string(),
            created_at: 1,
            updated_at: 1,
            can_manage: true,
        };
        let reply = SheetReply::Sheet {
            sheet: sheet.clone(),
        };
        let json = serde_json::to_string(&reply).unwrap();
        assert_eq!(serde_json::from_str::<SheetReply>(&json).unwrap(), reply);

        let cell = SheetCell {
            row: 0,
            col: 0,
            value: "10".to_string(),
            revision: 1,
            updated_by: "ホスト".to_string(),
            updated_at: 1,
        };
        let reply = SheetReply::WriteResult {
            applied: 1,
            conflicts: vec![cell.clone()],
        };
        let json = serde_json::to_string(&reply).unwrap();
        assert_eq!(serde_json::from_str::<SheetReply>(&json).unwrap(), reply);

        let reply = SheetReply::CellsData {
            sheet_id: "s1".to_string(),
            cells: vec![cell.clone()],
            offline: true,
        };
        let json = serde_json::to_string(&reply).unwrap();
        assert!(json.contains(r#""offline":true"#));
        assert_eq!(serde_json::from_str::<SheetReply>(&json).unwrap(), reply);

        let msg = SheetEventMsg::SheetChanged { sheet };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(serde_json::from_str::<SheetEventMsg>(&json).unwrap(), msg);

        let msg = SheetEventMsg::SheetRemoved {
            sheet_id: "s1".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"kind":"sheet_removed","sheet_id":"s1"}"#);
        assert_eq!(serde_json::from_str::<SheetEventMsg>(&json).unwrap(), msg);

        let msg = SheetEventMsg::CellsChanged {
            sheet_id: "s1".to_string(),
            cells: vec![cell],
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(serde_json::from_str::<SheetEventMsg>(&json).unwrap(), msg);
    }
}
