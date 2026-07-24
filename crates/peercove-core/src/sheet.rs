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
//! ADR-0054 決定 3)。セルの値は空文字への更新でセル削除を表す(ただし
//! 書式が既定でない場合はセルが存続する、ADR-0055 決定 6)。
//!
//! セル書式([`CellFormat`])・列幅/行高(`SheetOp::SetColWidth` /
//! `SetRowHeight`)は M6 H-4(ADR-0055 決定 6)で追加した additive な拡張。
//! 旧クライアントは `format` フィールドを知らないため無視するだけで壊れない
//! (`CellWrite.format` は `None` 既定 = 書式変更なし)。

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

/// 列幅・行高(既定でない分だけ、`(idx, size)` の組)。ADR-0055 決定 6。
/// `(col_widths, row_heights)`。
pub type SheetLayout = (Vec<(u32, u16)>, Vec<(u32, u16)>);

/// セル書式のフォントサイズ下限(pt、ADR-0055 決定 6)。
pub const MIN_FONT_SIZE: u8 = 8;

/// セル書式のフォントサイズ上限(pt)。
pub const MAX_FONT_SIZE: u8 = 36;

/// 列幅の下限(px)。
pub const MIN_COL_WIDTH: u16 = 20;

/// 列幅の上限(px)。
pub const MAX_COL_WIDTH: u16 = 600;

/// 行高の下限(px)。
pub const MIN_ROW_HEIGHT: u16 = 16;

/// 行高の上限(px)。
pub const MAX_ROW_HEIGHT: u16 = 400;

fn default_true() -> bool {
    true
}

fn is_true(v: &bool) -> bool {
    *v
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

/// セル書式(すべて省略可 = 既定、ADR-0055 決定 6)。`is_default()` が
/// 真の間はワイヤ上でも省略される(既存セル・旧クライアントとの互換)。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellFormat {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bold: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub italic: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub underline: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub strike: bool,
    /// pt。None = 既定(11 相当)。[`MIN_FONT_SIZE`]..=[`MAX_FONT_SIZE`]。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<u8>,
    /// 文字色("#rrggbb")。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// 背景色("#rrggbb")。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bg: Option<String>,
    /// 水平配置("left" | "center" | "right")。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub align: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub border_top: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub border_bottom: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub border_left: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub border_right: bool,
}

impl CellFormat {
    /// すべて既定値(未装飾)か。真なら「値が空ならセルも消える」判定に使う
    /// (ADR-0055 決定 6)。
    pub fn is_default(&self) -> bool {
        !self.bold
            && !self.italic
            && !self.underline
            && !self.strike
            && self.font_size.is_none()
            && self.color.is_none()
            && self.bg.is_none()
            && self.align.is_none()
            && !self.border_top
            && !self.border_bottom
            && !self.border_left
            && !self.border_right
    }
}

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
    /// 目盛線の表示(既定 true、ADR-0055 決定 6)。
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub gridlines: bool,
    /// ウインドウ枠を固定する先頭行数(既定 0 = 固定なし、ADR-0055 決定 6)。
    #[serde(default, skip_serializing_if = "is_zero")]
    pub freeze_rows: u32,
    /// ウインドウ枠を固定する先頭列数(既定 0 = 固定なし)。
    #[serde(default, skip_serializing_if = "is_zero")]
    pub freeze_cols: u32,
}

/// セル結合の 1 件(左上セル座標 + 縦横スパン、ADR-0055 決定 6)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SheetMerge {
    pub row: u32,
    pub col: u32,
    pub row_span: u32,
    pub col_span: u32,
}

impl SheetMerge {
    /// この結合が (row, col) を含むか。
    pub fn contains(&self, row: u32, col: u32) -> bool {
        row >= self.row
            && row < self.row + self.row_span
            && col >= self.col
            && col < self.col + self.col_span
    }

    /// 2 つの結合が(セル単位で)重なるか。
    pub fn overlaps(&self, other: &SheetMerge) -> bool {
        self.row < other.row + other.row_span
            && other.row < self.row + self.row_span
            && self.col < other.col + other.col_span
            && other.col < self.col + self.col_span
    }
}

/// プレゼンス 1 名分(選択セルの共有、ADR-0055 決定 6)。**DB には保存しない**
/// (揮発情報、TTL 10 秒)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SheetPresencePeer {
    pub name: String,
    pub row: u32,
    pub col: u32,
}

/// 1 セル分(疎な格納。非空セルだけが存在する。値が空でも書式が既定でなければ
/// セルは存続する、ADR-0055 決定 6)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SheetCell {
    pub row: u32,
    pub col: u32,
    pub value: String,
    /// 単調増加リビジョン(CAS 用、ADR-0054 決定 4)。書式の変更もここに含む。
    pub revision: u64,
    pub updated_by: String,
    pub updated_at: u64,
    #[serde(default, skip_serializing_if = "CellFormat::is_default")]
    pub format: CellFormat,
}

/// セル書き込み 1 件分(バッチの要素)。`base_revision` = 0 は新規セル想定
/// (既存セルに対して 0 を送ると競合扱いになる)。値が空文字 + 書式が既定
/// (省略)ならセル削除。`format` は `None` = 書式変更なし(値のみ更新)、
/// `Some` = そのセルの書式を丸ごと置き換える。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellWrite {
    pub row: u32,
    pub col: u32,
    pub value: String,
    #[serde(default)]
    pub base_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<CellFormat>,
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
    /// 列幅の変更(誰でも可。シート全体の見た目 = 共有、ADR-0055 決定 6)。
    /// `width` = `None` は既定幅へ戻す。応答は [`SheetReply::Done`]。
    SetColWidth {
        sheet_id: String,
        col: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<u16>,
    },
    /// 行高の変更(誰でも可)。`height` = `None` は既定高へ戻す。
    /// 応答は [`SheetReply::Done`]。
    SetRowHeight {
        sheet_id: String,
        row: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<u16>,
    },
    /// セル結合(誰でも可、ADR-0055 決定 6)。左上以外のセルの値・書式は
    /// 削除される。応答は [`SheetReply::Done`]。
    Merge { sheet_id: String, merge: SheetMerge },
    /// セル結合の解除(結合範囲内の任意セルの座標で指定できる)。
    /// 応答は [`SheetReply::Done`]。
    Unmerge {
        sheet_id: String,
        row: u32,
        col: u32,
    },
    /// シート設定(目盛線・固定枠、誰でも可、ADR-0055 決定 6)。
    /// 応答は [`SheetReply::Sheet`](更新後のメタ)。
    SetSheetSettings {
        sheet_id: String,
        #[serde(default = "default_true")]
        gridlines: bool,
        #[serde(default)]
        freeze_rows: u32,
        #[serde(default)]
        freeze_cols: u32,
    },
    /// 選択セルのプレゼンス共有(揮発、DB 保存なし、ADR-0055 決定 6)。
    /// 応答は [`SheetReply::Done`]。
    Presence {
        sheet_id: String,
        row: u32,
        col: u32,
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
        /// 列幅(既定でない列のみ、ADR-0055 決定 6)。
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        col_widths: Vec<(u32, u16)>,
        /// 行高(既定でない行のみ)。
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        row_heights: Vec<(u32, u16)>,
        /// セル結合(ADR-0055 決定 6)。
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        merges: Vec<SheetMerge>,
        /// 在席メンバー(自分以外、TTL 10 秒の揮発情報。ADR-0055 決定 6)。
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        presence: Vec<SheetPresencePeer>,
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
    /// セルのバッチ変更(削除セルは value = "" かつ format 既定で表現)。
    CellsChanged {
        sheet_id: String,
        cells: Vec<SheetCell>,
    },
    /// 列幅・行高の変更(全量、ADR-0055 決定 6)。
    Layout {
        sheet_id: String,
        col_widths: Vec<(u32, u16)>,
        row_heights: Vec<(u32, u16)>,
    },
    /// セル結合の変更(全量、ADR-0055 決定 6)。
    Merges {
        sheet_id: String,
        merges: Vec<SheetMerge>,
    },
    /// プレゼンス(在席セルの共有、揮発。**DB には保存しない**、
    /// ADR-0055 決定 6)。
    Presence {
        sheet_id: String,
        peers: Vec<SheetPresencePeer>,
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
                format: None,
            }],
        };
        let json = serde_json::to_string(&op).unwrap();
        // format: None は省略される(旧ワイヤ互換)
        assert!(!json.contains("format"));
        assert_eq!(serde_json::from_str::<SheetOp>(&json).unwrap(), op);

        let op = SheetOp::Write {
            sheet_id: "s1".to_string(),
            cells: vec![CellWrite {
                row: 0,
                col: 0,
                value: String::new(),
                base_revision: 1,
                format: Some(CellFormat {
                    bold: true,
                    color: Some("#ff0000".to_string()),
                    ..Default::default()
                }),
            }],
        };
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(serde_json::from_str::<SheetOp>(&json).unwrap(), op);

        let op = SheetOp::SetColWidth {
            sheet_id: "s1".to_string(),
            col: 2,
            width: Some(120),
        };
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(
            json,
            r#"{"op":"set_col_width","sheet_id":"s1","col":2,"width":120}"#
        );
        assert_eq!(serde_json::from_str::<SheetOp>(&json).unwrap(), op);

        let op = SheetOp::SetRowHeight {
            sheet_id: "s1".to_string(),
            row: 3,
            height: None,
        };
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(json, r#"{"op":"set_row_height","sheet_id":"s1","row":3}"#);
        assert_eq!(serde_json::from_str::<SheetOp>(&json).unwrap(), op);

        let op = SheetOp::Merge {
            sheet_id: "s1".to_string(),
            merge: SheetMerge {
                row: 0,
                col: 0,
                row_span: 2,
                col_span: 2,
            },
        };
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(serde_json::from_str::<SheetOp>(&json).unwrap(), op);

        let op = SheetOp::Unmerge {
            sheet_id: "s1".to_string(),
            row: 0,
            col: 0,
        };
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(json, r#"{"op":"unmerge","sheet_id":"s1","row":0,"col":0}"#);
        assert_eq!(serde_json::from_str::<SheetOp>(&json).unwrap(), op);

        let op = SheetOp::SetSheetSettings {
            sheet_id: "s1".to_string(),
            gridlines: false,
            freeze_rows: 1,
            freeze_cols: 2,
        };
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(serde_json::from_str::<SheetOp>(&json).unwrap(), op);

        let op = SheetOp::Presence {
            sheet_id: "s1".to_string(),
            row: 4,
            col: 5,
        };
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(json, r#"{"op":"presence","sheet_id":"s1","row":4,"col":5}"#);
        assert_eq!(serde_json::from_str::<SheetOp>(&json).unwrap(), op);

        let sheet = SheetMeta {
            id: "s1".to_string(),
            name: "在庫表".to_string(),
            owner_id: String::new(),
            owner_name: "ホスト".to_string(),
            created_at: 1,
            updated_at: 1,
            can_manage: true,
            gridlines: true,
            freeze_rows: 0,
            freeze_cols: 0,
        };
        // 目盛線既定 true・固定枠既定 0 はワイヤ上省略される(旧クライアント互換)
        let json = serde_json::to_string(&sheet).unwrap();
        assert!(!json.contains("gridlines"));
        assert!(!json.contains("freeze_rows"));
        assert!(!json.contains("freeze_cols"));
        assert_eq!(serde_json::from_str::<SheetMeta>(&json).unwrap(), sheet);

        let sheet_with_settings = SheetMeta {
            gridlines: false,
            freeze_rows: 2,
            freeze_cols: 1,
            ..sheet.clone()
        };
        let json = serde_json::to_string(&sheet_with_settings).unwrap();
        assert!(json.contains(r#""gridlines":false"#));
        assert!(json.contains(r#""freeze_rows":2"#));
        assert!(json.contains(r#""freeze_cols":1"#));
        assert_eq!(
            serde_json::from_str::<SheetMeta>(&json).unwrap(),
            sheet_with_settings
        );

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
            format: CellFormat::default(),
        };
        // format が既定ならワイヤ上は省略される(旧クライアント互換)
        let json = serde_json::to_string(&cell).unwrap();
        assert!(!json.contains("format"));
        assert_eq!(serde_json::from_str::<SheetCell>(&json).unwrap(), cell);

        let formatted_cell = SheetCell {
            format: CellFormat {
                bold: true,
                italic: true,
                font_size: Some(14),
                align: Some("center".to_string()),
                border_top: true,
                ..Default::default()
            },
            ..cell.clone()
        };
        let json = serde_json::to_string(&formatted_cell).unwrap();
        assert!(json.contains(r#""format""#));
        assert_eq!(
            serde_json::from_str::<SheetCell>(&json).unwrap(),
            formatted_cell
        );

        let reply = SheetReply::WriteResult {
            applied: 1,
            conflicts: vec![cell.clone()],
        };
        let json = serde_json::to_string(&reply).unwrap();
        assert_eq!(serde_json::from_str::<SheetReply>(&json).unwrap(), reply);

        let reply = SheetReply::CellsData {
            sheet_id: "s1".to_string(),
            cells: vec![cell.clone()],
            col_widths: vec![(0, 150)],
            row_heights: Vec::new(),
            merges: Vec::new(),
            presence: Vec::new(),
            offline: true,
        };
        let json = serde_json::to_string(&reply).unwrap();
        assert!(json.contains(r#""offline":true"#));
        assert!(json.contains(r#""col_widths":[[0,150]]"#));
        assert!(!json.contains("row_heights"));
        assert!(!json.contains("merges"));
        assert!(!json.contains("presence"));
        assert_eq!(serde_json::from_str::<SheetReply>(&json).unwrap(), reply);

        let reply = SheetReply::CellsData {
            sheet_id: "s1".to_string(),
            cells: vec![cell.clone()],
            col_widths: Vec::new(),
            row_heights: Vec::new(),
            merges: vec![SheetMerge {
                row: 0,
                col: 0,
                row_span: 2,
                col_span: 3,
            }],
            presence: vec![SheetPresencePeer {
                name: "アリス".to_string(),
                row: 1,
                col: 1,
            }],
            offline: false,
        };
        let json = serde_json::to_string(&reply).unwrap();
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

        let msg = SheetEventMsg::Layout {
            sheet_id: "s1".to_string(),
            col_widths: vec![(0, 150), (2, 80)],
            row_heights: vec![(1, 40)],
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(serde_json::from_str::<SheetEventMsg>(&json).unwrap(), msg);

        let msg = SheetEventMsg::Merges {
            sheet_id: "s1".to_string(),
            merges: vec![SheetMerge {
                row: 0,
                col: 0,
                row_span: 2,
                col_span: 2,
            }],
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(serde_json::from_str::<SheetEventMsg>(&json).unwrap(), msg);

        let msg = SheetEventMsg::Presence {
            sheet_id: "s1".to_string(),
            peers: vec![SheetPresencePeer {
                name: "ボブ".to_string(),
                row: 2,
                col: 3,
            }],
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"presence","sheet_id":"s1","peers":[{"name":"ボブ","row":2,"col":3}]}"#
        );
        assert_eq!(serde_json::from_str::<SheetEventMsg>(&json).unwrap(), msg);
    }

    #[test]
    fn sheet_merge_contains_and_overlaps() {
        let m = SheetMerge {
            row: 2,
            col: 3,
            row_span: 2,
            col_span: 3,
        };
        assert!(m.contains(2, 3));
        assert!(m.contains(3, 5));
        assert!(!m.contains(4, 3));
        assert!(!m.contains(2, 6));

        let overlapping = SheetMerge {
            row: 3,
            col: 4,
            row_span: 1,
            col_span: 1,
        };
        assert!(m.overlaps(&overlapping));
        assert!(overlapping.overlaps(&m));

        let disjoint = SheetMerge {
            row: 4,
            col: 3,
            row_span: 1,
            col_span: 3,
        };
        assert!(!m.overlaps(&disjoint));
    }

    #[test]
    fn cell_format_is_default() {
        assert!(CellFormat::default().is_default());
        assert!(!CellFormat {
            bold: true,
            ..Default::default()
        }
        .is_default());
        assert!(!CellFormat {
            color: Some("#000000".to_string()),
            ..Default::default()
        }
        .is_default());
    }
}
