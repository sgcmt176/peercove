//! 個人メモの UniFFI 公開(M5 F-1、ADR-0049)。
//!
//! ストレージはデスクトップと同じ `peercove-memo`(SQLite)。DB はアプリの
//! filesDir 直下の `memos.db`(ネットワーク非依存)。Kotlin 側はここで返す
//! Record を表示するだけで、メモのロジックは持たない(ADR-0039)。
//! ファイル入出力(SAF)は Kotlin が行い、本文の受け渡しだけをここで担う。
//! **メモのタイトル・本文・タグはログへ出さない。**

use std::collections::HashMap;
use std::path::Path;

use peercove_core::memo::{
    MemoDetail, MemoFolder, MemoOp, MemoPatch, MemoQuery, MemoReminder, MemoReply, MemoScope,
    MemoSort, MemoSummary, ReminderScope,
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

/// メモ間リンク `[[タイトル]]`(ADR-0052 決定 2)の解決。タイトル → memo_id
/// (見つかったものだけ)。
#[uniffi::export]
pub fn memo_resolve_titles(
    base_dir: String,
    titles: Vec<String>,
) -> Result<HashMap<String, String>, MobileError> {
    match apply(&base_dir, MemoOp::ResolveTitles { titles })? {
        MemoReply::Titles { map } => Ok(map),
        _ => Err(MobileError::Failure {
            msg: "想定外の応答です".to_string(),
        }),
    }
}

/// バックリンク(本文に `[[このメモのタイトル]]` を含むメモの一覧)。
#[uniffi::export]
pub fn memo_backlinks(base_dir: String, id: String) -> Result<Vec<MemoSummaryInfo>, MobileError> {
    match apply(&base_dir, MemoOp::Backlinks { id })? {
        MemoReply::Memos { memos, .. } => Ok(memos.into_iter().map(Into::into).collect()),
        _ => Err(MobileError::Failure {
            msg: "想定外の応答です".to_string(),
        }),
    }
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

// ---- リマインダー(端末ローカル、M5 F-5 Stage 5、ADR-0052 決定 6) --------
//
// 個人・共有どちらのメモに対するリマインダーも、この端末の個人メモ DB
// (memos.db)で管理する(他人には見えない)。発火は AlarmManager が担い、
// 発火時に `memo_reminder_take_due` を呼んで fired へ遷移させたうえで
// 通知を組み立てる(Kotlin 側、`ReminderReceiver`)。

#[derive(uniffi::Enum, Clone, Copy)]
pub enum ReminderScopeArg {
    Personal,
    Shared,
}

impl From<ReminderScopeArg> for ReminderScope {
    fn from(scope: ReminderScopeArg) -> Self {
        match scope {
            ReminderScopeArg::Personal => ReminderScope::Personal,
            ReminderScopeArg::Shared => ReminderScope::Shared,
        }
    }
}

#[derive(uniffi::Record)]
pub struct MemoReminderInfo {
    pub scope: ReminderScopeArg,
    /// 共有メモのときだけ意味を持つネットワーク slug(個人メモは空文字)。
    pub network: String,
    pub memo_id: String,
    /// 発火時刻(UNIX ミリ秒)。
    pub remind_at: u64,
}

impl From<MemoReminder> for MemoReminderInfo {
    fn from(reminder: MemoReminder) -> Self {
        Self {
            scope: match reminder.scope {
                ReminderScope::Personal => ReminderScopeArg::Personal,
                ReminderScope::Shared => ReminderScopeArg::Shared,
            },
            network: reminder.network,
            memo_id: reminder.memo_id,
            remind_at: reminder.remind_at,
        }
    }
}

fn expect_reminders(reply: MemoReply) -> Result<Vec<MemoReminderInfo>, MobileError> {
    match reply {
        MemoReply::Reminders { reminders } => Ok(reminders.into_iter().map(Into::into).collect()),
        _ => Err(MobileError::Failure {
            msg: "想定外の応答です".to_string(),
        }),
    }
}

/// リマインダーの設定(過去日時は拒否。既存があれば上書き)。
#[uniffi::export]
pub fn memo_reminder_set(
    base_dir: String,
    scope: ReminderScopeArg,
    network: String,
    memo_id: String,
    remind_at: u64,
) -> Result<(), MobileError> {
    apply(
        &base_dir,
        MemoOp::ReminderSet {
            scope: scope.into(),
            network,
            memo_id,
            remind_at,
        },
    )
    .map(|_| ())
}

/// リマインダーの解除。無くても失敗しない。
#[uniffi::export]
pub fn memo_reminder_clear(
    base_dir: String,
    scope: ReminderScopeArg,
    network: String,
    memo_id: String,
) -> Result<(), MobileError> {
    apply(
        &base_dir,
        MemoOp::ReminderClear {
            scope: scope.into(),
            network,
            memo_id,
        },
    )
    .map(|_| ())
}

/// 未発火のリマインダー一覧(この端末のすべて)。再起動後の
/// AlarmManager 再登録(BOOT_COMPLETED)にも使う。
#[uniffi::export]
pub fn memo_reminder_list(base_dir: String) -> Result<Vec<MemoReminderInfo>, MobileError> {
    expect_reminders(apply(&base_dir, MemoOp::ReminderList)?)
}

/// 発火時刻を過ぎたリマインダーを取り出す(呼ぶと fired 済みになる)。
/// `ReminderReceiver`(AlarmManager の発火)から呼び、返ってきた分だけ
/// 通知を組み立てる。
#[uniffi::export]
pub fn memo_reminder_take_due(base_dir: String) -> Result<Vec<MemoReminderInfo>, MobileError> {
    expect_reminders(apply(&base_dir, MemoOp::ReminderTakeDue)?)
}

// ---- 共有メモ(M5 F-2、ADR-0049)-------------------------------------------
//
// 読み取りは常にキャッシュ(オフラインでも閲覧可)。変更は稼働中セッションの
// コントロールチャネル経由でホストへ届き、権限・編集ロック・リビジョン(CAS)
// はすべてホスト正本で判定される。

use peercove_core::memo::{
    DiffLine, DiffLineKind, SharedMemoComment, SharedMemoDetail, SharedMemoHistoryDetail,
    SharedMemoHistoryEntry, SharedMemoOp, SharedMemoQuery, SharedMemoReply, SharedMemoSummary,
};
use peercove_core::schedule::{ScheduleEvent, ScheduleOp, ScheduleReply};
use peercove_core::sheet::{CellWrite, SheetCell, SheetMeta, SheetOp, SheetReply};

#[derive(uniffi::Record)]
pub struct SharedMemoSummaryInfo {
    pub id: String,
    pub title: String,
    pub excerpt: String,
    pub folder_id: Option<String>,
    pub revision: u64,
    pub created_at: u64,
    pub updated_at: u64,
    pub updated_by: Option<String>,
    pub owner_name: String,
    pub can_edit: bool,
    pub can_manage: bool,
    pub locked_by: Option<String>,
    pub checklist_done: u32,
    pub checklist_total: u32,
    /// コメント件数(ADR-0052 決定 4)。一覧の 💬 バッジ用。
    pub comment_count: u32,
}

impl From<SharedMemoSummary> for SharedMemoSummaryInfo {
    fn from(memo: SharedMemoSummary) -> Self {
        Self {
            id: memo.id,
            title: memo.title,
            excerpt: memo.excerpt,
            folder_id: memo.folder_id,
            revision: memo.revision,
            created_at: memo.created_at,
            updated_at: memo.updated_at,
            updated_by: memo.updated_by,
            owner_name: memo.owner_name,
            can_edit: memo.can_edit,
            can_manage: memo.can_manage,
            locked_by: memo.locked_by,
            checklist_done: memo.checklist_done,
            checklist_total: memo.checklist_total,
            comment_count: memo.comment_count,
        }
    }
}

#[derive(uniffi::Record)]
pub struct SharedMemoDetailInfo {
    pub id: String,
    pub title: String,
    pub body: String,
    pub folder_id: Option<String>,
    pub revision: u64,
    pub created_at: u64,
    pub updated_at: u64,
    pub updated_by: Option<String>,
    pub owner_name: String,
    pub can_edit: bool,
    pub can_manage: bool,
    pub locked_by: Option<String>,
    /// コメント件数(ADR-0052 決定 4)。
    pub comment_count: u32,
}

impl From<SharedMemoDetail> for SharedMemoDetailInfo {
    fn from(memo: SharedMemoDetail) -> Self {
        Self {
            id: memo.id,
            title: memo.title,
            body: memo.body,
            folder_id: memo.folder_id,
            revision: memo.revision,
            created_at: memo.created_at,
            updated_at: memo.updated_at,
            updated_by: memo.updated_by,
            owner_name: memo.owner_name,
            can_edit: memo.can_edit,
            can_manage: memo.can_manage,
            locked_by: memo.locked_by,
            comment_count: memo.comment_count,
        }
    }
}

/// 共有メモのコメント 1 件(ADR-0052 決定 4)。**本文はメモ本文と同格の
/// 秘匿対象**(ログへ出さない)。Android は常にメンバー役割(ADR-0039)なので
/// `can_manage`(= オーナー判定)と組み合わせて「自分のメモ」を判定できる。
#[derive(uniffi::Record)]
pub struct SharedMemoCommentInfo {
    pub comment_id: String,
    pub memo_id: String,
    pub author_name: String,
    pub body: String,
    pub created_at_unix_ms: u64,
}

impl From<SharedMemoComment> for SharedMemoCommentInfo {
    fn from(comment: SharedMemoComment) -> Self {
        Self {
            comment_id: comment.comment_id,
            memo_id: comment.memo_id,
            author_name: comment.author_name,
            body: comment.body,
            created_at_unix_ms: comment.created_at_unix_ms,
        }
    }
}

#[derive(uniffi::Record)]
pub struct SharedMemoListResult {
    pub memos: Vec<SharedMemoSummaryInfo>,
    pub folders: Vec<MemoFolderInfo>,
    /// セッション未接続(キャッシュ表示 = 読み取り専用)。
    pub offline: bool,
    /// ホストが共有メモに応答済みか(false = 未対応 or 未同期)。
    pub supported: bool,
    /// キャッシュの変更世代。進んだら再取得する。
    pub generation: u64,
}

/// 変更履歴 1 版分の要約(本文は含まない、M5 F-3)。
#[derive(uniffi::Record)]
pub struct SharedMemoHistoryEntryInfo {
    pub hid: i64,
    pub revision: u64,
    /// "auto" | "close" | "manual" | "restore"。
    pub kind: String,
    pub saved_by_name: String,
    pub created_at_unix_ms: u64,
    pub title: String,
    pub body_bytes: u64,
}

impl From<SharedMemoHistoryEntry> for SharedMemoHistoryEntryInfo {
    fn from(entry: SharedMemoHistoryEntry) -> Self {
        Self {
            hid: entry.hid,
            revision: entry.revision,
            kind: entry.kind,
            saved_by_name: entry.saved_by_name,
            created_at_unix_ms: entry.created_at_unix_ms,
            title: entry.title,
            body_bytes: entry.body_bytes,
        }
    }
}

/// 変更履歴 1 版分の全体(本文込み)。
#[derive(uniffi::Record)]
pub struct SharedMemoHistoryDetailInfo {
    pub entry: SharedMemoHistoryEntryInfo,
    pub body: String,
}

impl From<SharedMemoHistoryDetail> for SharedMemoHistoryDetailInfo {
    fn from(detail: SharedMemoHistoryDetail) -> Self {
        Self {
            entry: detail.entry.into(),
            body: detail.body,
        }
    }
}

/// 差分の 1 行。`kind` は "same" | "added" | "removed"。
#[derive(uniffi::Record)]
pub struct DiffLineInfo {
    pub kind: String,
    pub text: String,
}

impl From<DiffLine> for DiffLineInfo {
    fn from(line: DiffLine) -> Self {
        Self {
            kind: match line.kind {
                DiffLineKind::Same => "same",
                DiffLineKind::Added => "added",
                DiffLineKind::Removed => "removed",
            }
            .to_string(),
            text: line.text,
        }
    }
}

fn cache_path(base_dir: &str, slug: &str) -> std::path::PathBuf {
    peercove_ops::networks::networks_dir(Path::new(base_dir))
        .join(slug)
        .join(peercove_ops::networks::MEMBER_FILE)
        .with_extension("memocache.db")
}

fn open_cache(
    base_dir: &str,
    slug: &str,
) -> Result<peercove_memo::shared::CacheStore, MobileError> {
    Ok(peercove_memo::shared::CacheStore::open(&cache_path(
        base_dir, slug,
    ))?)
}

fn session_request(slug: &str, op: SharedMemoOp) -> Result<SharedMemoReply, MobileError> {
    let session = crate::session_of(slug).ok_or_else(|| MobileError::Failure {
        msg: "接続していません(共有メモの変更には接続が必要です)".to_string(),
    })?;
    Ok(session.memo_request(op)?)
}

fn expect_shared_memo(reply: SharedMemoReply) -> Result<SharedMemoDetailInfo, MobileError> {
    match reply {
        SharedMemoReply::Memo { memo } => Ok(memo.into()),
        _ => Err(MobileError::Failure {
            msg: "想定外の応答です".to_string(),
        }),
    }
}

/// 一覧(キャッシュから。オフラインでも使える)。
#[uniffi::export]
pub fn shared_memo_list(
    base_dir: String,
    slug: String,
    folder_id: Option<String>,
    search: Option<String>,
) -> Result<SharedMemoListResult, MobileError> {
    let cache = open_cache(&base_dir, &slug)?;
    let (memos, folders) = cache.list(&SharedMemoQuery {
        trash: false,
        folder_id,
        search: search.filter(|s| !s.trim().is_empty()),
    })?;
    let session = crate::session_of(&slug);
    let online = session.as_ref().is_some_and(|s| {
        s.control_connected
            .load(std::sync::atomic::Ordering::Relaxed)
    });
    let mut memos: Vec<SharedMemoSummaryInfo> = memos.into_iter().map(Into::into).collect();
    if !online {
        // オフライン中は読み取り専用(要件 §3.2)
        for memo in &mut memos {
            memo.can_edit = false;
            memo.can_manage = false;
        }
    }
    Ok(SharedMemoListResult {
        memos,
        folders: folders.into_iter().map(Into::into).collect(),
        offline: !online,
        supported: session
            .as_ref()
            .is_some_and(|s| s.memo_supported.load(std::sync::atomic::Ordering::Relaxed)),
        generation: session
            .as_ref()
            .map(|s| s.memo_generation.load(std::sync::atomic::Ordering::Relaxed))
            .unwrap_or(0),
    })
}

/// キャッシュの変更世代(UI のポーリング用。セッション無しは 0)。
#[uniffi::export]
pub fn shared_memo_generation(slug: String) -> u64 {
    crate::session_of(&slug)
        .map(|s| s.memo_generation.load(std::sync::atomic::Ordering::Relaxed))
        .unwrap_or(0)
}

/// 1 件取得(キャッシュから)。
#[uniffi::export]
pub fn shared_memo_get(
    base_dir: String,
    slug: String,
    id: String,
) -> Result<SharedMemoDetailInfo, MobileError> {
    Ok(open_cache(&base_dir, &slug)?.get(&id)?.into())
}

/// メモ間リンク `[[タイトル]]`(ADR-0052 決定 2)の解決(キャッシュから。
/// List/Get と同じくオフラインでも使える)。
#[uniffi::export]
pub fn shared_memo_resolve_titles(
    base_dir: String,
    slug: String,
    titles: Vec<String>,
) -> Result<HashMap<String, String>, MobileError> {
    Ok(open_cache(&base_dir, &slug)?.resolve_titles(&titles)?)
}

/// バックリンク(キャッシュから。List/Get と同じくオフラインでも使える)。
#[uniffi::export]
pub fn shared_memo_backlinks(
    base_dir: String,
    slug: String,
    id: String,
) -> Result<Vec<SharedMemoSummaryInfo>, MobileError> {
    let memos = open_cache(&base_dir, &slug)?.backlinks(&id)?;
    Ok(memos.into_iter().map(Into::into).collect())
}

#[uniffi::export]
pub fn shared_memo_create(
    slug: String,
    title: String,
    body: String,
) -> Result<SharedMemoDetailInfo, MobileError> {
    expect_shared_memo(session_request(
        &slug,
        SharedMemoOp::Create {
            title,
            body,
            folder_id: None,
        },
    )?)
}

/// 編集ロックの取得(応答が編集の土台になる最新内容)。
#[uniffi::export]
pub fn shared_memo_acquire(slug: String, id: String) -> Result<SharedMemoDetailInfo, MobileError> {
    expect_shared_memo(session_request(&slug, SharedMemoOp::AcquireLock { id })?)
}

#[uniffi::export]
pub fn shared_memo_release(slug: String, id: String) -> Result<(), MobileError> {
    session_request(&slug, SharedMemoOp::ReleaseLock { id }).map(|_| ())
}

/// 保存(CAS)。`base_revision` が最新でなければ競合として拒否される。
#[uniffi::export]
pub fn shared_memo_save(
    slug: String,
    id: String,
    base_revision: u64,
    title: String,
    body: String,
) -> Result<SharedMemoDetailInfo, MobileError> {
    expect_shared_memo(session_request(
        &slug,
        SharedMemoOp::Update {
            id,
            base_revision,
            title,
            body,
        },
    )?)
}

/// ゴミ箱へ(所有者のみ。復元・完全削除はホスト UI から)。
#[uniffi::export]
pub fn shared_memo_trash(slug: String, id: String) -> Result<(), MobileError> {
    session_request(&slug, SharedMemoOp::Trash { id }).map(|_| ())
}

// ---- 共有スケジュール表(M6 G-1、ADR-0053)-----------------------------------
//
// 共有メモと同じキャッシュ・generation を共用する(`<config>.memocache.db`、
// `memo_generation`)。ネットワーク全員が閲覧・追加でき、編集・削除は
// 作成者 + ホストのみ(権限判定はホスト正本の store 層)。読み取りは常に
// キャッシュ、変更はオンライン専用(コントロールチャネル経由)。
// **予定のタイトル・詳細はログへ出さない。**

#[derive(uniffi::Record)]
pub struct ScheduleEventInfo {
    pub id: String,
    pub title: String,
    pub note: String,
    pub start_unix_ms: u64,
    pub end_unix_ms: Option<u64>,
    pub all_day: bool,
    pub owner_name: String,
    pub updated_by: String,
    pub revision: u64,
    pub created_at: u64,
    pub updated_at: u64,
    /// 受信者視点: 編集・削除できるか(作成者 + ホスト)。
    pub can_edit: bool,
}

impl From<ScheduleEvent> for ScheduleEventInfo {
    fn from(event: ScheduleEvent) -> Self {
        Self {
            id: event.id,
            title: event.title,
            note: event.note,
            start_unix_ms: event.start_unix_ms,
            end_unix_ms: event.end_unix_ms,
            all_day: event.all_day,
            owner_name: event.owner_name,
            updated_by: event.updated_by,
            revision: event.revision,
            created_at: event.created_at,
            updated_at: event.updated_at,
            can_edit: event.can_edit,
        }
    }
}

#[derive(uniffi::Record)]
pub struct ScheduleListResult {
    pub events: Vec<ScheduleEventInfo>,
    /// セッション未接続(キャッシュ表示 = 読み取り専用)。
    pub offline: bool,
    /// ホストが共有スケジュール表に応答済みか(共有メモと共通の判定)。
    pub supported: bool,
    /// キャッシュの変更世代(共有メモと共通。進んだら再取得する)。
    pub generation: u64,
}

fn expect_schedule_event(reply: SharedMemoReply) -> Result<ScheduleEventInfo, MobileError> {
    match reply {
        SharedMemoReply::Schedule {
            reply: ScheduleReply::Event { event },
        } => Ok(event.into()),
        SharedMemoReply::Schedule {
            reply: ScheduleReply::Err { message },
        } => Err(MobileError::Failure { msg: message }),
        _ => Err(MobileError::Failure {
            msg: "想定外の応答です".to_string(),
        }),
    }
}

/// 一覧(キャッシュから。オフラインでも使える。全員閲覧可 = ADR-0053 決定 3)。
#[uniffi::export]
pub fn schedule_list(base_dir: String, slug: String) -> Result<ScheduleListResult, MobileError> {
    let cache = open_cache(&base_dir, &slug)?;
    let events = cache.schedule_list()?;
    let session = crate::session_of(&slug);
    let online = session.as_ref().is_some_and(|s| {
        s.control_connected
            .load(std::sync::atomic::Ordering::Relaxed)
    });
    let mut events: Vec<ScheduleEventInfo> = events.into_iter().map(Into::into).collect();
    if !online {
        // オフライン中は読み取り専用(共有メモの List と同じ扱い)
        for event in &mut events {
            event.can_edit = false;
        }
    }
    Ok(ScheduleListResult {
        events,
        offline: !online,
        supported: session
            .as_ref()
            .is_some_and(|s| s.memo_supported.load(std::sync::atomic::Ordering::Relaxed)),
        generation: session
            .as_ref()
            .map(|s| s.memo_generation.load(std::sync::atomic::Ordering::Relaxed))
            .unwrap_or(0),
    })
}

/// 新規作成(全員可、オンライン専用)。
#[uniffi::export]
#[allow(clippy::too_many_arguments)]
pub fn schedule_create(
    slug: String,
    title: String,
    note: String,
    start_unix_ms: u64,
    end_unix_ms: Option<u64>,
    all_day: bool,
) -> Result<ScheduleEventInfo, MobileError> {
    expect_schedule_event(session_request(
        &slug,
        SharedMemoOp::Schedule {
            schedule: ScheduleOp::Create {
                title,
                note,
                start_unix_ms,
                end_unix_ms,
                all_day,
            },
        },
    )?)
}

/// 更新(CAS)。`base_revision` が最新でなければ競合として拒否される
/// (作成者 + ホストのみ、オンライン専用)。
#[uniffi::export]
#[allow(clippy::too_many_arguments)]
pub fn schedule_update(
    slug: String,
    id: String,
    base_revision: u64,
    title: String,
    note: String,
    start_unix_ms: u64,
    end_unix_ms: Option<u64>,
    all_day: bool,
) -> Result<ScheduleEventInfo, MobileError> {
    expect_schedule_event(session_request(
        &slug,
        SharedMemoOp::Schedule {
            schedule: ScheduleOp::Update {
                id,
                base_revision,
                title,
                note,
                start_unix_ms,
                end_unix_ms,
                all_day,
            },
        },
    )?)
}

/// 削除(物理削除、ゴミ箱なし。作成者 + ホストのみ、オンライン専用)。
#[uniffi::export]
pub fn schedule_delete(slug: String, id: String) -> Result<(), MobileError> {
    session_request(
        &slug,
        SharedMemoOp::Schedule {
            schedule: ScheduleOp::Delete { id },
        },
    )
    .map(|_| ())
}

// ---- 共有シート(M6 G-2、ADR-0054)-------------------------------------------
//
// 共有メモ・共有スケジュール表と同じキャッシュ・generation を共用する
// (`<config>.memocache.db`、`memo_generation`)。ネットワーク全員が閲覧・
// セル編集でき、シートの作成・改名・削除は作成者 + ホストのみ(権限判定は
// ホスト正本の store 層)。読み取りは常にキャッシュ、変更はオンライン専用
// (コントロールチャネル経由)。**シート名・セル値はログへ出さない。**

#[derive(uniffi::Record)]
pub struct SheetMetaInfo {
    pub id: String,
    pub name: String,
    pub owner_name: String,
    pub created_at: u64,
    pub updated_at: u64,
    /// 受信者視点: 改名・削除できるか(作成者 + ホスト)。
    pub can_manage: bool,
}

impl From<SheetMeta> for SheetMetaInfo {
    fn from(sheet: SheetMeta) -> Self {
        Self {
            id: sheet.id,
            name: sheet.name,
            owner_name: sheet.owner_name,
            created_at: sheet.created_at,
            updated_at: sheet.updated_at,
            can_manage: sheet.can_manage,
        }
    }
}

#[derive(uniffi::Record)]
pub struct SheetCellInfo {
    pub row: u32,
    pub col: u32,
    pub value: String,
    pub revision: u64,
    pub updated_by: String,
    pub updated_at: u64,
}

impl From<SheetCell> for SheetCellInfo {
    fn from(cell: SheetCell) -> Self {
        Self {
            row: cell.row,
            col: cell.col,
            value: cell.value,
            revision: cell.revision,
            updated_by: cell.updated_by,
            updated_at: cell.updated_at,
        }
    }
}

/// セル書き込み 1 件分(Kotlin → Rust)。`base_revision` = 0 は新規セル想定。
#[derive(uniffi::Record)]
pub struct SheetCellWriteArg {
    pub row: u32,
    pub col: u32,
    pub value: String,
    pub base_revision: u64,
}

impl From<SheetCellWriteArg> for CellWrite {
    fn from(write: SheetCellWriteArg) -> Self {
        Self {
            row: write.row,
            col: write.col,
            value: write.value,
            base_revision: write.base_revision,
        }
    }
}

#[derive(uniffi::Record)]
pub struct SheetListResult {
    pub sheets: Vec<SheetMetaInfo>,
    /// セッション未接続(キャッシュ表示 = 読み取り専用)。
    pub offline: bool,
    /// ホストが共有シートに応答済みか(共有メモと共通の判定)。
    pub supported: bool,
    /// キャッシュの変更世代(共有メモと共通。進んだら再取得する)。
    pub generation: u64,
}

#[derive(uniffi::Record)]
pub struct SheetCellsResult {
    pub cells: Vec<SheetCellInfo>,
    pub offline: bool,
    pub generation: u64,
}

/// Write の結果(部分失敗をサポートする、ADR-0054 決定 4)。
#[derive(uniffi::Record)]
pub struct SheetWriteResult {
    pub applied: u32,
    /// base_revision 不一致で適用しなかったセルの現在値。
    pub conflicts: Vec<SheetCellInfo>,
}

/// シート一覧(キャッシュから。オフラインでも使える。全員閲覧可 = ADR-0054 決定 5)。
#[uniffi::export]
pub fn sheet_list(base_dir: String, slug: String) -> Result<SheetListResult, MobileError> {
    let cache = open_cache(&base_dir, &slug)?;
    let sheets = cache.sheet_list()?;
    let session = crate::session_of(&slug);
    let online = session.as_ref().is_some_and(|s| {
        s.control_connected
            .load(std::sync::atomic::Ordering::Relaxed)
    });
    let mut sheets: Vec<SheetMetaInfo> = sheets.into_iter().map(Into::into).collect();
    if !online {
        // オフライン中は読み取り専用(共有メモの List と同じ扱い)
        for sheet in &mut sheets {
            sheet.can_manage = false;
        }
    }
    Ok(SheetListResult {
        sheets,
        offline: !online,
        supported: session
            .as_ref()
            .is_some_and(|s| s.memo_supported.load(std::sync::atomic::Ordering::Relaxed)),
        generation: session
            .as_ref()
            .map(|s| s.memo_generation.load(std::sync::atomic::Ordering::Relaxed))
            .unwrap_or(0),
    })
}

/// 1 シートの全非空セル(キャッシュから。オフラインでも使える)。
#[uniffi::export]
pub fn sheet_cells(
    base_dir: String,
    slug: String,
    sheet_id: String,
) -> Result<SheetCellsResult, MobileError> {
    let cache = open_cache(&base_dir, &slug)?;
    let cells = cache.sheet_cells(&sheet_id)?;
    let session = crate::session_of(&slug);
    let online = session.as_ref().is_some_and(|s| {
        s.control_connected
            .load(std::sync::atomic::Ordering::Relaxed)
    });
    Ok(SheetCellsResult {
        cells: cells.into_iter().map(Into::into).collect(),
        offline: !online,
        generation: session
            .as_ref()
            .map(|s| s.memo_generation.load(std::sync::atomic::Ordering::Relaxed))
            .unwrap_or(0),
    })
}

fn expect_sheet(reply: SharedMemoReply) -> Result<SheetMetaInfo, MobileError> {
    match reply {
        SharedMemoReply::Sheet {
            reply: SheetReply::Sheet { sheet },
        } => Ok(sheet.into()),
        SharedMemoReply::Sheet {
            reply: SheetReply::Err { message },
        } => Err(MobileError::Failure { msg: message }),
        _ => Err(MobileError::Failure {
            msg: "想定外の応答です".to_string(),
        }),
    }
}

/// 新規作成(全員可、オンライン専用)。
#[uniffi::export]
pub fn sheet_create(slug: String, name: String) -> Result<SheetMetaInfo, MobileError> {
    expect_sheet(session_request(
        &slug,
        SharedMemoOp::Sheet {
            sheet: SheetOp::Create { name },
        },
    )?)
}

/// 改名(作成者 + ホストのみ、オンライン専用)。
#[uniffi::export]
pub fn sheet_rename(
    slug: String,
    sheet_id: String,
    name: String,
) -> Result<SheetMetaInfo, MobileError> {
    expect_sheet(session_request(
        &slug,
        SharedMemoOp::Sheet {
            sheet: SheetOp::Rename { sheet_id, name },
        },
    )?)
}

/// 削除(作成者 + ホストのみ、オンライン専用)。
#[uniffi::export]
pub fn sheet_delete(slug: String, sheet_id: String) -> Result<(), MobileError> {
    session_request(
        &slug,
        SharedMemoOp::Sheet {
            sheet: SheetOp::Delete { sheet_id },
        },
    )
    .map(|_| ())
}

/// セルのバッチ書き込み(全員可、オンライン専用)。競合セルは
/// `conflicts` に現在値が入って返る(部分失敗、ADR-0054 決定 4)。
#[uniffi::export]
pub fn sheet_write(
    slug: String,
    sheet_id: String,
    cells: Vec<SheetCellWriteArg>,
) -> Result<SheetWriteResult, MobileError> {
    let reply = session_request(
        &slug,
        SharedMemoOp::Sheet {
            sheet: SheetOp::Write {
                sheet_id,
                cells: cells.into_iter().map(Into::into).collect(),
            },
        },
    )?;
    match reply {
        SharedMemoReply::Sheet {
            reply: SheetReply::WriteResult { applied, conflicts },
        } => Ok(SheetWriteResult {
            applied,
            conflicts: conflicts.into_iter().map(Into::into).collect(),
        }),
        SharedMemoReply::Sheet {
            reply: SheetReply::Err { message },
        } => Err(MobileError::Failure { msg: message }),
        _ => Err(MobileError::Failure {
            msg: "想定外の応答です".to_string(),
        }),
    }
}

// ---- 変更履歴(M5 F-3、ADR-0049)---------------------------------------------
//
// 履歴の閲覧は「そのメモが見えるメンバー」、復元・版保存は「編集権限のある
// メンバー」にホスト側で検査される(権限エラーは message にホストの理由が
// 入って返る)。

/// 変更履歴の一覧(新しい順)。
#[uniffi::export]
pub fn shared_memo_history_list(
    slug: String,
    memo_id: String,
) -> Result<Vec<SharedMemoHistoryEntryInfo>, MobileError> {
    match session_request(&slug, SharedMemoOp::HistoryList { id: memo_id })? {
        SharedMemoReply::History { entries } => Ok(entries.into_iter().map(Into::into).collect()),
        _ => Err(MobileError::Failure {
            msg: "想定外の応答です".to_string(),
        }),
    }
}

/// 変更履歴 1 版の本文取得。
#[uniffi::export]
pub fn shared_memo_history_get(
    slug: String,
    memo_id: String,
    hid: i64,
) -> Result<SharedMemoHistoryDetailInfo, MobileError> {
    match session_request(&slug, SharedMemoOp::HistoryGet { id: memo_id, hid })? {
        SharedMemoReply::HistoryDetail { detail } => Ok(detail.into()),
        _ => Err(MobileError::Failure {
            msg: "想定外の応答です".to_string(),
        }),
    }
}

/// 2 版間の差分。`to_hid = None` は「現在の本文と比較」。
#[uniffi::export]
pub fn shared_memo_history_diff(
    slug: String,
    memo_id: String,
    from_hid: i64,
    to_hid: Option<i64>,
) -> Result<Vec<DiffLineInfo>, MobileError> {
    match session_request(
        &slug,
        SharedMemoOp::HistoryDiff {
            id: memo_id,
            from_hid,
            to_hid,
        },
    )? {
        SharedMemoReply::Diff { lines } => Ok(lines.into_iter().map(Into::into).collect()),
        _ => Err(MobileError::Failure {
            msg: "想定外の応答です".to_string(),
        }),
    }
}

/// 指定した版の内容へ復元する(編集権限が必要。他人が編集中なら拒否される)。
#[uniffi::export]
pub fn shared_memo_history_restore(
    slug: String,
    memo_id: String,
    hid: i64,
) -> Result<(), MobileError> {
    match session_request(&slug, SharedMemoOp::HistoryRestore { id: memo_id, hid })? {
        SharedMemoReply::Memo { .. } | SharedMemoReply::Done => Ok(()),
        _ => Err(MobileError::Failure {
            msg: "想定外の応答です".to_string(),
        }),
    }
}

/// 現在の内容を手動で履歴に残す(編集権限が必要)。
#[uniffi::export]
pub fn shared_memo_save_version(slug: String, memo_id: String) -> Result<(), MobileError> {
    session_request(&slug, SharedMemoOp::SaveVersion { id: memo_id }).map(|_| ())
}

// ---- コメント(M5 F-5、ADR-0052 決定 4)-------------------------------------
//
// 閲覧・追加・削除はすべてホスト正本で権限判定されるため、常時オンライン
// 専用(session_request)。キャッシュには保存しない(一覧は都度取得)。

/// コメント一覧(古い順、全件)。
#[uniffi::export]
pub fn shared_memo_comment_list(
    slug: String,
    memo_id: String,
) -> Result<Vec<SharedMemoCommentInfo>, MobileError> {
    match session_request(&slug, SharedMemoOp::CommentList { id: memo_id })? {
        SharedMemoReply::Comments { comments } => {
            Ok(comments.into_iter().map(Into::into).collect())
        }
        _ => Err(MobileError::Failure {
            msg: "想定外の応答です".to_string(),
        }),
    }
}

/// コメント追加(本文上限 4KiB・1 メモ 500 件)。
#[uniffi::export]
pub fn shared_memo_comment_add(
    slug: String,
    memo_id: String,
    body: String,
) -> Result<SharedMemoCommentInfo, MobileError> {
    match session_request(&slug, SharedMemoOp::CommentAdd { id: memo_id, body })? {
        SharedMemoReply::Comment { comment } => Ok(comment.into()),
        _ => Err(MobileError::Failure {
            msg: "想定外の応答です".to_string(),
        }),
    }
}

/// コメント削除(本人・メモ所有者・ホスト管理者)。
#[uniffi::export]
pub fn shared_memo_comment_delete(
    slug: String,
    memo_id: String,
    comment_id: String,
) -> Result<(), MobileError> {
    session_request(
        &slug,
        SharedMemoOp::CommentDelete {
            id: memo_id,
            comment_id,
        },
    )
    .map(|_| ())
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

    /// リマインダー(ADR-0052 決定 6)の UniFFI 層の疎通。
    #[test]
    fn reminder_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().to_string_lossy().into_owned();
        let memo = memo_create(base.clone(), "会議".to_string(), "".to_string(), None).unwrap();
        let future = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            + 60_000;

        memo_reminder_set(
            base.clone(),
            ReminderScopeArg::Personal,
            String::new(),
            memo.id.clone(),
            future,
        )
        .unwrap();
        let list = memo_reminder_list(base.clone()).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].memo_id, memo.id);

        assert!(
            memo_reminder_take_due(base.clone()).unwrap().is_empty(),
            "まだ発火時刻前"
        );

        memo_reminder_clear(
            base.clone(),
            ReminderScopeArg::Personal,
            String::new(),
            memo.id,
        )
        .unwrap();
        assert!(memo_reminder_list(base).unwrap().is_empty());
    }

    /// 履歴・差分の wire → UniFFI Record 変換(M5 F-3)。ネットワークは使わない。
    #[test]
    fn history_and_diff_conversion_preserves_fields() {
        let entry = SharedMemoHistoryEntry {
            hid: 7,
            revision: 3,
            kind: "manual".to_string(),
            saved_by_name: "太郎".to_string(),
            created_at_unix_ms: 1_000,
            title: "件名".to_string(),
            body_bytes: 42,
        };
        let info: SharedMemoHistoryEntryInfo = entry.clone().into();
        assert_eq!(info.hid, entry.hid);
        assert_eq!(info.revision, entry.revision);
        assert_eq!(info.kind, entry.kind);
        assert_eq!(info.saved_by_name, entry.saved_by_name);
        assert_eq!(info.created_at_unix_ms, entry.created_at_unix_ms);
        assert_eq!(info.title, entry.title);
        assert_eq!(info.body_bytes, entry.body_bytes);

        let detail = SharedMemoHistoryDetail {
            entry: entry.clone(),
            body: "本文".to_string(),
        };
        let detail_info: SharedMemoHistoryDetailInfo = detail.into();
        assert_eq!(detail_info.entry.hid, entry.hid);
        assert_eq!(detail_info.body, "本文");

        let added: DiffLineInfo = DiffLine {
            kind: DiffLineKind::Added,
            text: "x".to_string(),
        }
        .into();
        assert_eq!(added.kind, "added");
        assert_eq!(added.text, "x");
        let removed: DiffLineInfo = DiffLine {
            kind: DiffLineKind::Removed,
            text: "y".to_string(),
        }
        .into();
        assert_eq!(removed.kind, "removed");
        let same: DiffLineInfo = DiffLine {
            kind: DiffLineKind::Same,
            text: "z".to_string(),
        }
        .into();
        assert_eq!(same.kind, "same");
    }
}
