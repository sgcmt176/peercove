//! メモ帳機能の共有型(ADR-0049、M5 F-1)。
//!
//! ストレージ実装は `peercove-memo` crate。ここには IPC(デスクトップ)と
//! UniFFI(Android)の両方から使う操作・応答・データ型と、UI 補助の純関数だけを
//! 置く。**メモのタイトル・本文・タグ・フォルダー名はチャット本文と同格の
//! 秘匿対象 — ログ・標準出力へ出さない**(memo_id・件数は可)。

use serde::{Deserialize, Serialize};

/// 1 メモの本文の上限(要件 §14)。超過は保存を拒否して理由を返す。
pub const MAX_BODY_BYTES: usize = 256 * 1024;

/// タイトルの上限(文字数)。一覧・ファイル名に使うため常識的な長さに抑える。
pub const MAX_TITLE_CHARS: usize = 200;

/// ゴミ箱の保持日数(要件 §13)。超えた分は開いたときに完全削除される。
pub const TRASH_RETENTION_DAYS: u64 = 30;

/// 一覧の抜粋(本文先頭)の最大文字数。
pub const EXCERPT_CHARS: usize = 120;

/// メモ一覧の表示対象。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoScope {
    /// 通常(アーカイブ・ゴミ箱以外)。
    #[default]
    Active,
    /// アーカイブ済み。
    Archived,
    /// ゴミ箱。
    Trash,
}

/// メモ一覧の並び順。ピン留めは常に先頭に来る。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoSort {
    /// 更新日時の新しい順(既定)。
    #[default]
    Updated,
    /// 作成日時の新しい順。
    Created,
    /// タイトル順。
    Title,
}

/// メモ一覧の絞り込み(要件 §7)。すべて省略可 = 全件。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MemoQuery {
    #[serde(default)]
    pub scope: MemoScope,
    /// フォルダーで絞る(ゴミ箱以外で有効)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<String>,
    /// タグで絞る。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// タイトル・本文の全文検索(FTS5。3 文字未満は部分一致)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<String>,
    #[serde(default)]
    pub sort: MemoSort,
}

/// フォルダー(要件 §6)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoFolder {
    pub id: String,
    pub name: String,
    /// フォルダー内のメモ数(ゴミ箱を除く)。
    pub memo_count: u32,
}

/// タグと使用数(一覧サイドバー用)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoTagCount {
    pub tag: String,
    pub count: u32,
}

/// 一覧 1 行分の要約(本文全体は載せない)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoSummary {
    pub id: String,
    pub title: String,
    /// 本文の先頭部分([`excerpt`] で整形済み)。
    pub excerpt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archived: bool,
    /// UNIX ミリ秒。
    pub created_at: u64,
    pub updated_at: u64,
    /// ゴミ箱に入れた時刻(UNIX ミリ秒)。None = ゴミ箱ではない。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<u64>,
    /// チェックリストの完了数([`checklist_progress`])。
    #[serde(default, skip_serializing_if = "u32_is_zero")]
    pub checklist_done: u32,
    /// チェックリストの項目数。0 = チェックリストなし。
    #[serde(default, skip_serializing_if = "u32_is_zero")]
    pub checklist_total: u32,
}

fn u32_is_zero(value: &u32) -> bool {
    *value == 0
}

/// メモ 1 件の全体(編集画面用)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoDetail {
    pub id: String,
    pub title: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archived: bool,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<u64>,
}

/// フォルダー移動の指定。`Some(target)` のときだけ移動する
/// (`target.id = None` は「フォルダーなし」へ)。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MemoFolderTarget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// 部分更新(自動保存・属性変更)。`None` のフィールドは変更しない。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MemoPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<MemoFolderTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    /// タグ全量の置き換え(空 Vec = すべて外す)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

/// メモストアへの操作。IPC はこれを 1 メソッド
/// (`IpcRequest::Memo`)に載せ、応答は [`MemoReply`]。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum MemoOp {
    /// 一覧(+フォルダー・タグの集計)。
    List {
        #[serde(default)]
        query: MemoQuery,
    },
    /// 1 件取得。
    Get { id: String },
    /// 新規作成。応答は作成されたメモの [`MemoReply::Memo`]。
    Create {
        #[serde(default)]
        title: String,
        #[serde(default)]
        body: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        folder_id: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        tags: Vec<String>,
    },
    /// 部分更新(自動保存を含む)。応答は更新後の [`MemoReply::Memo`]。
    Update { id: String, patch: MemoPatch },
    /// 複製(タイトルに「のコピー」を付ける)。
    Duplicate { id: String },
    /// ゴミ箱へ移動。
    Trash { id: String },
    /// ゴミ箱から復元。
    Restore { id: String },
    /// 完全削除(ゴミ箱内のメモのみ)。
    DeleteForever { id: String },
    /// ゴミ箱を空にする。
    EmptyTrash,
    /// フォルダー作成。応答は [`MemoReply::Folder`]。
    FolderCreate { name: String },
    /// フォルダー改名。
    FolderRename { id: String, name: String },
    /// フォルダー削除(中のメモは「フォルダーなし」へ移動する)。
    FolderDelete { id: String },
}

/// [`MemoOp`] への応答。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MemoReply {
    /// List への応答。
    Memos {
        memos: Vec<MemoSummary>,
        folders: Vec<MemoFolder>,
        tags: Vec<MemoTagCount>,
    },
    /// Get / Create / Update / Duplicate への応答。
    Memo { memo: MemoDetail },
    /// FolderCreate への応答。
    Folder { folder: MemoFolder },
    /// 副作用のみの操作への応答。
    Done,
}

/// チェックリスト(`- [ ]` / `- [x]`)の進捗を数える。戻り値は (完了, 総数)。
/// Markdown のタスクリスト記法(`-` / `*` / `+` / 番号付き)に対応する。
pub fn checklist_progress(body: &str) -> (u32, u32) {
    let mut done = 0u32;
    let mut total = 0u32;
    for line in body.lines() {
        let trimmed = line.trim_start();
        let rest = if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
            .or_else(|| trimmed.strip_prefix("+ "))
        {
            rest
        } else {
            // 番号付きリスト(`1. ` など)
            let digits = trimmed.chars().take_while(|c| c.is_ascii_digit()).count();
            if digits == 0 {
                continue;
            }
            match trimmed[digits..].strip_prefix(". ") {
                Some(rest) => rest,
                None => continue,
            }
        };
        if rest.starts_with("[ ] ") || rest == "[ ]" {
            total += 1;
        } else if rest.starts_with("[x] ")
            || rest.starts_with("[X] ")
            || rest == "[x]"
            || rest == "[X]"
        {
            total += 1;
            done += 1;
        }
    }
    (done, total)
}

/// 一覧用の抜粋。空行を飛ばして本文の先頭を最大 `max_chars` 文字返す。
/// Markdown 記号は最小限(見出し `#`・引用 `>`)だけ剥がす。
pub fn excerpt(body: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for line in body.lines() {
        let mut text = line.trim();
        if text.is_empty() {
            continue;
        }
        text = text.trim_start_matches(['#', '>']).trim_start();
        if text.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(text);
        if out.chars().count() >= max_chars {
            break;
        }
    }
    if out.chars().count() > max_chars {
        let mut truncated: String = out.chars().take(max_chars).collect();
        truncated.push('…');
        return truncated;
    }
    out
}

/// タイトルをエクスポートのファイル名に使える形へ変換する(要件 §16)。
/// OS で使えない文字を `_` に置換し、空なら「メモ」。拡張子は付けない。
pub fn sanitize_filename(title: &str) -> String {
    let mut out: String = title
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    out = out.trim().trim_end_matches('.').trim().to_string();
    if out.is_empty() {
        return "メモ".to_string();
    }
    // Windows の予約名(CON, PRN, AUX, NUL, COM1-9, LPT1-9)を避ける
    let upper = out.to_ascii_uppercase();
    let reserved = matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (upper.len() == 4
            && (upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper[3..].chars().all(|c| c.is_ascii_digit()));
    if reserved {
        out.push('_');
    }
    // ファイル名としては長すぎる場合に切り詰める(バイトでなく文字で)
    if out.chars().count() > 80 {
        out = out.chars().take(80).collect();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checklist_progress_counts_task_items() {
        let body = "# 買い物\n- [x] 牛乳\n- [ ] 卵\n  - [X] たまご焼き\n* [ ] パン\n1. [x] 米\n2. ただの番号\n- 普通の箇条書き\n";
        assert_eq!(checklist_progress(body), (3, 5));
        assert_eq!(checklist_progress("メモ本文だけ"), (0, 0));
    }

    #[test]
    fn excerpt_skips_blank_and_markdown_prefix() {
        assert_eq!(excerpt("\n\n# 見出し\n\n> 引用文\n", 120), "見出し 引用文");
        let long = "あ".repeat(200);
        let cut = excerpt(&long, 10);
        assert_eq!(cut.chars().count(), 11, "10 文字 + 省略記号");
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn sanitize_filename_replaces_invalid_chars() {
        assert_eq!(
            sanitize_filename("a/b\\c:d*e?f\"g<h>i|j"),
            "a_b_c_d_e_f_g_h_i_j"
        );
        assert_eq!(sanitize_filename("  "), "メモ");
        assert_eq!(sanitize_filename("CON"), "CON_");
        assert_eq!(sanitize_filename("com1"), "com1_");
        assert_eq!(sanitize_filename("普通のタイトル"), "普通のタイトル");
        assert_eq!(sanitize_filename("末尾のドット..."), "末尾のドット");
    }

    /// ワイヤ表現(UI・モバイルが依存)。追加フィールドはすべて省略可能で、
    /// 旧バージョンとの相互無視が成り立つことを固定する。
    #[test]
    fn memo_op_wire_format() {
        let op = MemoOp::List {
            query: MemoQuery::default(),
        };
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(
            json,
            r#"{"op":"list","query":{"scope":"active","sort":"updated"}}"#
        );
        // query 省略でも読める
        let parsed: MemoOp = serde_json::from_str(r#"{"op":"list"}"#).unwrap();
        assert_eq!(parsed, op);

        let op = MemoOp::Update {
            id: "m1".to_string(),
            patch: MemoPatch {
                body: Some("本文".to_string()),
                folder: Some(MemoFolderTarget { id: None }),
                ..Default::default()
            },
        };
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(
            json, r#"{"op":"update","id":"m1","patch":{"body":"本文","folder":{}}}"#,
            "未指定フィールドは省略、folder = {{}} は「フォルダーなしへ移動」"
        );
        let parsed: MemoOp = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, op);

        let reply = MemoReply::Memo {
            memo: MemoDetail {
                id: "m1".to_string(),
                title: "t".to_string(),
                body: "b".to_string(),
                folder_id: None,
                tags: vec![],
                pinned: false,
                archived: false,
                created_at: 1,
                updated_at: 2,
                deleted_at: None,
            },
        };
        let json = serde_json::to_string(&reply).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"memo","memo":{"id":"m1","title":"t","body":"b","created_at":1,"updated_at":2}}"#
        );
        let parsed: MemoReply = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, reply);
    }
}
