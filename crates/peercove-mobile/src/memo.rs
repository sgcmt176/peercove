//! 個人メモの UniFFI 公開(M5 F-1、ADR-0049)。
//!
//! ストレージはデスクトップと同じ `peercove-memo`(SQLite)。DB はアプリの
//! filesDir 直下の `memos.db`(ネットワーク非依存)。Kotlin 側はここで返す
//! Record を表示するだけで、メモのロジックは持たない(ADR-0039)。
//! ファイル入出力(SAF)は Kotlin が行い、本文の受け渡しだけをここで担う。
//! **メモのタイトル・本文・タグはログへ出さない。**

use std::path::Path;

use peercove_core::memo::{
    MemoDetail, MemoFolder, MemoOp, MemoPatch, MemoQuery, MemoReply, MemoScope, MemoSort,
    MemoSummary,
};
use peercove_memo::MemoStore;

use crate::MobileError;

fn open(base_dir: &str) -> Result<MemoStore, MobileError> {
    Ok(MemoStore::open(&Path::new(base_dir).join("memos.db"))?)
}

fn apply(base_dir: &str, op: MemoOp) -> Result<MemoReply, MobileError> {
    Ok(open(base_dir)?.apply(op)?)
}

fn expect_memo(reply: MemoReply) -> Result<MemoDetailInfo, MobileError> {
    match reply {
        MemoReply::Memo { memo } => Ok(memo.into()),
        _ => Err(MobileError::Failure {
            msg: "想定外の応答です".to_string(),
        }),
    }
}

#[derive(uniffi::Enum)]
pub enum MemoScopeArg {
    Active,
    Archived,
    Trash,
}

#[derive(uniffi::Enum)]
pub enum MemoSortArg {
    Updated,
    Created,
    Title,
}

#[derive(uniffi::Record)]
pub struct MemoSummaryInfo {
    pub id: String,
    pub title: String,
    pub excerpt: String,
    pub folder_id: Option<String>,
    pub tags: Vec<String>,
    pub pinned: bool,
    pub archived: bool,
    pub created_at: u64,
    pub updated_at: u64,
    pub deleted_at: Option<u64>,
    pub checklist_done: u32,
    pub checklist_total: u32,
}

impl From<MemoSummary> for MemoSummaryInfo {
    fn from(memo: MemoSummary) -> Self {
        Self {
            id: memo.id,
            title: memo.title,
            excerpt: memo.excerpt,
            folder_id: memo.folder_id,
            tags: memo.tags,
            pinned: memo.pinned,
            archived: memo.archived,
            created_at: memo.created_at,
            updated_at: memo.updated_at,
            deleted_at: memo.deleted_at,
            checklist_done: memo.checklist_done,
            checklist_total: memo.checklist_total,
        }
    }
}

#[derive(uniffi::Record)]
pub struct MemoDetailInfo {
    pub id: String,
    pub title: String,
    pub body: String,
    pub folder_id: Option<String>,
    pub tags: Vec<String>,
    pub pinned: bool,
    pub archived: bool,
    pub created_at: u64,
    pub updated_at: u64,
    pub deleted_at: Option<u64>,
}

impl From<MemoDetail> for MemoDetailInfo {
    fn from(memo: MemoDetail) -> Self {
        Self {
            id: memo.id,
            title: memo.title,
            body: memo.body,
            folder_id: memo.folder_id,
            tags: memo.tags,
            pinned: memo.pinned,
            archived: memo.archived,
            created_at: memo.created_at,
            updated_at: memo.updated_at,
            deleted_at: memo.deleted_at,
        }
    }
}

#[derive(uniffi::Record)]
pub struct MemoFolderInfo {
    pub id: String,
    pub name: String,
    pub memo_count: u32,
}

impl From<MemoFolder> for MemoFolderInfo {
    fn from(folder: MemoFolder) -> Self {
        Self {
            id: folder.id,
            name: folder.name,
            memo_count: folder.memo_count,
        }
    }
}

#[derive(uniffi::Record)]
pub struct MemoTagInfo {
    pub tag: String,
    pub count: u32,
}

#[derive(uniffi::Record)]
pub struct MemoListResult {
    pub memos: Vec<MemoSummaryInfo>,
    pub folders: Vec<MemoFolderInfo>,
    pub tags: Vec<MemoTagInfo>,
}

#[uniffi::export]
pub fn memo_list(
    base_dir: String,
    scope: MemoScopeArg,
    folder_id: Option<String>,
    tag: Option<String>,
    search: Option<String>,
    sort: MemoSortArg,
) -> Result<MemoListResult, MobileError> {
    let query = MemoQuery {
        scope: match scope {
            MemoScopeArg::Active => MemoScope::Active,
            MemoScopeArg::Archived => MemoScope::Archived,
            MemoScopeArg::Trash => MemoScope::Trash,
        },
        folder_id,
        tag,
        search: search.filter(|s| !s.trim().is_empty()),
        sort: match sort {
            MemoSortArg::Updated => MemoSort::Updated,
            MemoSortArg::Created => MemoSort::Created,
            MemoSortArg::Title => MemoSort::Title,
        },
    };
    match apply(&base_dir, MemoOp::List { query })? {
        MemoReply::Memos {
            memos,
            folders,
            tags,
        } => Ok(MemoListResult {
            memos: memos.into_iter().map(Into::into).collect(),
            folders: folders.into_iter().map(Into::into).collect(),
            tags: tags
                .into_iter()
                .map(|t| MemoTagInfo {
                    tag: t.tag,
                    count: t.count,
                })
                .collect(),
        }),
        _ => Err(MobileError::Failure {
            msg: "想定外の応答です".to_string(),
        }),
    }
}

#[uniffi::export]
pub fn memo_get(base_dir: String, id: String) -> Result<MemoDetailInfo, MobileError> {
    expect_memo(apply(&base_dir, MemoOp::Get { id })?)
}

#[uniffi::export]
pub fn memo_create(
    base_dir: String,
    title: String,
    body: String,
    folder_id: Option<String>,
) -> Result<MemoDetailInfo, MobileError> {
    expect_memo(apply(
        &base_dir,
        MemoOp::Create {
            title,
            body,
            folder_id,
            tags: vec![],
        },
    )?)
}

/// 自動保存(タイトル・本文)。
#[uniffi::export]
pub fn memo_save_text(
    base_dir: String,
    id: String,
    title: String,
    body: String,
) -> Result<MemoDetailInfo, MobileError> {
    expect_memo(apply(
        &base_dir,
        MemoOp::Update {
            id,
            patch: MemoPatch {
                title: Some(title),
                body: Some(body),
                ..Default::default()
            },
        },
    )?)
}

/// ピン留め・アーカイブの切り替え(None = 変更しない)。
#[uniffi::export]
pub fn memo_set_flags(
    base_dir: String,
    id: String,
    pinned: Option<bool>,
    archived: Option<bool>,
) -> Result<MemoDetailInfo, MobileError> {
    expect_memo(apply(
        &base_dir,
        MemoOp::Update {
            id,
            patch: MemoPatch {
                pinned,
                archived,
                ..Default::default()
            },
        },
    )?)
}

/// フォルダー移動(None = 「フォルダーなし」へ)。
#[uniffi::export]
pub fn memo_set_folder(
    base_dir: String,
    id: String,
    folder_id: Option<String>,
) -> Result<MemoDetailInfo, MobileError> {
    expect_memo(apply(
        &base_dir,
        MemoOp::Update {
            id,
            patch: MemoPatch {
                folder: Some(peercove_core::memo::MemoFolderTarget { id: folder_id }),
                ..Default::default()
            },
        },
    )?)
}

/// タグ全量の置き換え(空 = すべて外す)。
#[uniffi::export]
pub fn memo_set_tags(
    base_dir: String,
    id: String,
    tags: Vec<String>,
) -> Result<MemoDetailInfo, MobileError> {
    expect_memo(apply(
        &base_dir,
        MemoOp::Update {
            id,
            patch: MemoPatch {
                tags: Some(tags),
                ..Default::default()
            },
        },
    )?)
}

#[uniffi::export]
pub fn memo_duplicate(base_dir: String, id: String) -> Result<MemoDetailInfo, MobileError> {
    expect_memo(apply(&base_dir, MemoOp::Duplicate { id })?)
}

#[uniffi::export]
pub fn memo_trash(base_dir: String, id: String) -> Result<(), MobileError> {
    apply(&base_dir, MemoOp::Trash { id }).map(|_| ())
}

#[uniffi::export]
pub fn memo_restore(base_dir: String, id: String) -> Result<(), MobileError> {
    apply(&base_dir, MemoOp::Restore { id }).map(|_| ())
}

#[uniffi::export]
pub fn memo_delete_forever(base_dir: String, id: String) -> Result<(), MobileError> {
    apply(&base_dir, MemoOp::DeleteForever { id }).map(|_| ())
}

#[uniffi::export]
pub fn memo_empty_trash(base_dir: String) -> Result<(), MobileError> {
    apply(&base_dir, MemoOp::EmptyTrash).map(|_| ())
}

#[uniffi::export]
pub fn memo_folder_create(base_dir: String, name: String) -> Result<MemoFolderInfo, MobileError> {
    match apply(&base_dir, MemoOp::FolderCreate { name })? {
        MemoReply::Folder { folder } => Ok(folder.into()),
        _ => Err(MobileError::Failure {
            msg: "想定外の応答です".to_string(),
        }),
    }
}

#[uniffi::export]
pub fn memo_folder_rename(base_dir: String, id: String, name: String) -> Result<(), MobileError> {
    apply(&base_dir, MemoOp::FolderRename { id, name }).map(|_| ())
}

#[uniffi::export]
pub fn memo_folder_delete(base_dir: String, id: String) -> Result<(), MobileError> {
    apply(&base_dir, MemoOp::FolderDelete { id }).map(|_| ())
}

/// エクスポート用のファイル名(拡張子なし)。OS で使えない文字を除いたもの。
#[uniffi::export]
pub fn memo_export_name(title: String) -> String {
    peercove_core::memo::sanitize_filename(&title)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// UniFFI 層の疎通(ストレージ本体のテストは peercove-memo 側)。
    #[test]
    fn create_list_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().to_string_lossy().into_owned();
        let memo =
            memo_create(base.clone(), "題".to_string(), "- [ ] a".to_string(), None).unwrap();
        let result = memo_list(
            base.clone(),
            MemoScopeArg::Active,
            None,
            None,
            None,
            MemoSortArg::Updated,
        )
        .unwrap();
        assert_eq!(result.memos.len(), 1);
        assert_eq!(result.memos[0].id, memo.id);
        assert_eq!(result.memos[0].checklist_total, 1);
        memo_trash(base.clone(), memo.id.clone()).unwrap();
        memo_delete_forever(base.clone(), memo.id).unwrap();
        assert_eq!(memo_export_name("a/b".to_string()), "a_b");
    }
}
