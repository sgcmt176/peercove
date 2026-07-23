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

// ---- 共有メモ(M5 F-2、ADR-0049) ----
//
// ホスト正本の共有メモをコントロールチャネル(`MemoReq` / `MemoResp` /
// `MemoEvent`)と IPC(`IpcRequest::SharedMemo`)の両方で操作するための型。
// 権限は member_id(= invite_id、ADR-0047)へ紐付け、フィルタは**ホストの
// 配信時**に行う(受信側フィルタに頼らない)。

/// 共有メモの権限レベル。メンバー個別の指定は「全体」より優先される
/// (`None` を個別指定すると、そのメンバーだけ除外できる)。
/// 宣言順(None < Viewer < Editor)が強さの順序と一致するため
/// `PartialOrd`/`Ord` を導出できる(複数グループ該当時の最大値判定に使う。
/// ADR-0051)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SharedPermLevel {
    /// 見えない。
    None,
    /// 閲覧のみ(既定)。
    #[default]
    Viewer,
    /// 閲覧 + 編集。
    Editor,
}

/// メンバー個別の権限指定。`name` は表示用スナップショット(正本は member_id)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedMemberPerm {
    pub member_id: String,
    #[serde(default)]
    pub name: String,
    pub level: SharedPermLevel,
}

/// グループ単位の権限指定(ADR-0051)。`name` は表示用スナップショット
/// (正本は group_id。現存するグループなら detail 生成時に現在名で上書きする)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedGroupPerm {
    pub group_id: String,
    #[serde(default)]
    pub name: String,
    pub level: SharedPermLevel,
}

/// 共有メモ一覧の絞り込み。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SharedMemoQuery {
    /// ゴミ箱を見る(ホスト管理者と所有者のみ意味を持つ)。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub trash: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<String>,
}

/// 共有メモ一覧の 1 行。`can_edit` / `can_manage` / `locked_by` は
/// **受信者視点**でホストが計算して詰める。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedMemoSummary {
    pub id: String,
    pub title: String,
    pub excerpt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<String>,
    /// 単調増加リビジョン(CAS 用)。
    pub revision: u64,
    pub created_at: u64,
    pub updated_at: u64,
    /// 最終更新者の表示名(スナップショット)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<String>,
    /// 所有者(作成者)の member_id。空 = ホスト。
    #[serde(default)]
    pub owner_id: String,
    #[serde(default)]
    pub owner_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<u64>,
    /// 受信者が編集できるか(所有者・編集者)。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub can_edit: bool,
    /// 受信者が権限変更・削除できるか(所有者・ホスト管理者)。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub can_manage: bool,
    /// いま編集ロックを握っている人の表示名(自分を含む)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locked_by: Option<String>,
    #[serde(default, skip_serializing_if = "u32_is_zero")]
    pub checklist_done: u32,
    #[serde(default, skip_serializing_if = "u32_is_zero")]
    pub checklist_total: u32,
}

/// 共有メモ 1 件の全体。`everyone` / `members` は can_manage のときだけ載る。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedMemoDetail {
    pub id: String,
    pub title: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<String>,
    pub revision: u64,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<String>,
    #[serde(default)]
    pub owner_id: String,
    #[serde(default)]
    pub owner_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<u64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub can_edit: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub can_manage: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locked_by: Option<String>,
    /// 全体(ネットワークの全メンバー)への権限。can_manage のときだけ載る。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub everyone: Option<SharedPermLevel>,
    /// メンバー個別の権限。can_manage のときだけ載る。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<SharedMemberPerm>,
    /// グループ単位の権限(ADR-0051)。can_manage のときだけ載る。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<SharedGroupPerm>,
}

/// 共有メモへの操作。メンバーは `MemoReq` に載せてホストへ送り、
/// ホスト UI は IPC からそのままサービス層へ渡す。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum SharedMemoOp {
    List {
        #[serde(default)]
        query: SharedMemoQuery,
    },
    Get {
        id: String,
    },
    Create {
        #[serde(default)]
        title: String,
        #[serde(default)]
        body: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        folder_id: Option<String>,
    },
    /// 本文・タイトルの更新(CAS)。編集ロックを握っていることが前提。
    /// `base_revision` が現在と一致しない場合は競合として拒否される。
    Update {
        id: String,
        base_revision: u64,
        title: String,
        body: String,
    },
    /// 編集ロックの取得。応答は最新の [`SharedMemoReply::Memo`]
    /// (これを土台に編集を始める)。
    AcquireLock {
        id: String,
    },
    ReleaseLock {
        id: String,
    },
    /// (ホスト管理者のみ)編集ロックの強制解除。
    ForceUnlock {
        id: String,
    },
    /// ゴミ箱へ(所有者・ホスト管理者)。
    Trash {
        id: String,
    },
    Restore {
        id: String,
    },
    DeleteForever {
        id: String,
    },
    /// 権限の設定(所有者・ホスト管理者)。`members` は全量置き換え。
    /// `groups` は `None` = グループ権限を変更しない(旧クライアントの
    /// SetPerms が既存のグループ権限を消さないための互換仕様)。
    /// `Some` は全量置き換え。
    SetPerms {
        id: String,
        everyone: SharedPermLevel,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        members: Vec<SharedMemberPerm>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        groups: Option<Vec<SharedGroupPerm>>,
    },
    /// 共有フォルダーの管理(ホスト管理者のみ — 要件 §6)。
    FolderCreate {
        name: String,
    },
    FolderRename {
        id: String,
        name: String,
    },
    FolderDelete {
        id: String,
    },
    /// 変更履歴の一覧(新しい順)。
    HistoryList {
        id: String,
    },
    /// 変更履歴 1 版の本文取得。
    HistoryGet {
        id: String,
        hid: i64,
    },
    /// 2 版間の差分。`to_hid = None` は「現在の本文と比較」。
    HistoryDiff {
        id: String,
        from_hid: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to_hid: Option<i64>,
    },
    /// 指定した版の内容へ復元する。
    HistoryRestore {
        id: String,
        hid: i64,
    },
    /// 現在の内容を手動で履歴に残す。
    SaveVersion {
        id: String,
    },
    /// 共有メモの容量・履歴上限を取得する(秘匿情報ではないため誰でも可)。
    GetLimits,
    /// (ホスト管理者のみ)共有メモの容量・履歴上限を設定する。
    SetLimits {
        limits: SharedMemoLimits,
    },
}

/// [`SharedMemoOp`] への応答。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
// 短命のワイヤ型なので Box 化しない(ワイヤ表現を単純に保つ)
#[allow(clippy::large_enum_variant)]
pub enum SharedMemoReply {
    Memos {
        memos: Vec<SharedMemoSummary>,
        folders: Vec<MemoFolder>,
        /// (メンバーのみ)ホスト未接続のキャッシュ応答 = 読み取り専用。
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        offline: bool,
    },
    Memo {
        memo: SharedMemoDetail,
    },
    /// HistoryList への応答。
    History {
        entries: Vec<SharedMemoHistoryEntry>,
    },
    /// HistoryGet への応答。
    HistoryDetail {
        detail: SharedMemoHistoryDetail,
    },
    /// HistoryDiff への応答。
    Diff {
        lines: Vec<DiffLine>,
    },
    /// GetLimits / SetLimits への応答。
    Limits {
        limits: SharedMemoLimits,
    },
    Done,
    /// 拒否・競合など(コントロールチャネル経由の応答用)。
    Err {
        message: String,
    },
}

/// 共有メモの容量・履歴の上限(ホスト設定可、M5 F-3)。
/// 既定値は要件 §14 相当(本文サイズ)+ 運用上妥当な値。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedMemoLimits {
    /// 1 メモの本文の上限(バイト)。既定 256 KiB。
    pub max_body_bytes: u64,
    /// 共有メモの件数上限(ゴミ箱含む)。既定 10,000 件。
    pub max_memo_count: u32,
    /// 本文 + 履歴本文の合計サイズ上限(バイト)。既定 100 MiB。
    pub max_total_bytes: u64,
    /// メモごとの変更履歴の保持件数上限。既定 100。
    pub max_versions: u32,
    /// 変更履歴の保持日数。既定 30 日。
    pub history_days: u32,
    /// ゴミ箱の保持日数。既定 30 日。
    pub trash_days: u32,
}

impl Default for SharedMemoLimits {
    fn default() -> Self {
        Self {
            max_body_bytes: 256 * 1024,
            max_memo_count: 10_000,
            max_total_bytes: 100 * 1024 * 1024,
            max_versions: 100,
            history_days: 30,
            trash_days: 30,
        }
    }
}

impl SharedMemoLimits {
    /// 範囲外の設定を拒否する(コントロールチャネルの 1 行 1MiB 上限に
    /// 収めるため本文サイズは 256KiB までしか許可しない)。
    pub fn validate(&self) -> Result<(), String> {
        if !(1024..=256 * 1024).contains(&self.max_body_bytes) {
            return Err("本文サイズの上限は 1KiB〜256KiB の範囲で指定してください".to_string());
        }
        if !(1..=100_000).contains(&self.max_memo_count) {
            return Err("メモ件数の上限は 1〜100,000 件の範囲で指定してください".to_string());
        }
        if !(1024 * 1024..=1024 * 1024 * 1024).contains(&self.max_total_bytes) {
            return Err("全体容量の上限は 1MiB〜1GiB の範囲で指定してください".to_string());
        }
        if !(1..=1_000).contains(&self.max_versions) {
            return Err("変更履歴の保持件数は 1〜1,000 の範囲で指定してください".to_string());
        }
        if !(1..=365).contains(&self.history_days) {
            return Err("変更履歴の保持日数は 1〜365 日の範囲で指定してください".to_string());
        }
        if !(1..=365).contains(&self.trash_days) {
            return Err("ゴミ箱の保持日数は 1〜365 日の範囲で指定してください".to_string());
        }
        Ok(())
    }
}

/// 変更履歴 1 版分の要約(本文は含まない)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedMemoHistoryEntry {
    /// 履歴行 ID(単調増加、AUTOINCREMENT)。
    pub hid: i64,
    /// その版が保存された時点のメモ revision。
    pub revision: u64,
    /// "auto" | "close" | "manual" | "restore"。
    pub kind: String,
    /// その内容を書いた人の表示名(スナップショット)。
    pub saved_by_name: String,
    pub created_at_unix_ms: u64,
    pub title: String,
    /// 本文のバイト数(UTF-8)。一覧に本文全体は載せない。
    pub body_bytes: u64,
}

/// 変更履歴 1 版分の全体(本文込み)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedMemoHistoryDetail {
    pub entry: SharedMemoHistoryEntry,
    pub body: String,
}

/// 差分の 1 行の種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffLineKind {
    Same,
    Added,
    Removed,
}

/// 差分の 1 行。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub text: String,
}

/// ホスト → メンバーのリアルタイム配信。**閲覧権限のある接続にだけ**送る。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
// 短命のワイヤ型なので Box 化しない(ワイヤ表現を単純に保つ)
#[allow(clippy::large_enum_variant)]
pub enum SharedMemoEvent {
    /// 作成・更新・権限変更(受信者視点で can_edit 等を計算済み)。
    Changed { memo: SharedMemoDetail },
    /// 削除、または権限を失って見えなくなった。キャッシュから消すこと。
    Removed { id: String },
    /// 編集ロックの変化。`holder` は表示名(None = 解放)。
    Lock {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        holder: Option<String>,
    },
    /// 共有フォルダー一覧の変化。
    Folders { folders: Vec<MemoFolder> },
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
