//! 共有メモのストレージ(M5 F-2、ADR-0049)。
//!
//! - [`SharedStore`] — ホスト正本。`<config>.memos.db`(ネットワーク単位)。
//!   権限(member_id 紐付け)・リビジョン(CAS)・ゴミ箱を持つ。
//!   編集ロックは揮発情報なのでここには置かない(デーモンのサービス層が持つ)。
//! - [`CacheStore`] — メンバーの読み取りキャッシュ。`<config>.memocache.db`。
//!   ホストからの配信(一覧・Get・イベント)を書き込み、オフライン時は
//!   ここから読み取り専用で表示する。
//!
//! **メモのタイトル・本文・フォルダー名はログへ出さない**(ADR-0049)。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context};
use peercove_core::memo::{
    checklist_progress, excerpt, DiffLine, MemoFolder, SharedGroupPerm, SharedMemberPerm,
    SharedMemoComment, SharedMemoDetail, SharedMemoHistoryDetail, SharedMemoHistoryEntry,
    SharedMemoLimits, SharedMemoQuery, SharedMemoSummary, SharedPermLevel, EXCERPT_CHARS,
    MAX_COMMENTS_PER_MEMO, MAX_COMMENT_BODY_BYTES,
};
use peercove_core::schedule::{
    ScheduleEvent, ScheduleParticipant, MAX_SCHEDULE_EVENTS, MAX_SCHEDULE_NOTE_BYTES,
    MAX_SCHEDULE_PARTICIPANTS, MAX_SCHEDULE_TITLE_CHARS,
};
use peercove_core::sheet::{
    CellWrite, SheetCell, SheetMeta, MAX_CELL_VALUE_BYTES, MAX_SHEETS, MAX_SHEET_CELLS,
    MAX_SHEET_COLS, MAX_SHEET_NAME_CHARS, MAX_SHEET_ROWS,
};
use rusqlite::{params, Connection, OptionalExtension};

use crate::{
    diff_lines, escape_like, kana_fold, register_kana_fold, unix_ms, validate_body_bytes,
    validate_folder_name, validate_title,
};

/// 変更履歴の自動保存の間隔(ミリ秒)。編集セッション中はこの間隔でしか
/// "auto" 版を残さない(頻繁な自動保存で履歴が埋まらないように)。
const SHARED_HISTORY_INTERVAL_MS: i64 = 10 * 60 * 1000;

/// 削除済み ID 台帳の保持期間(ミリ秒)。これを過ぎたら台帳からも消す。
const TOMBSTONE_RETENTION_MS: i64 = 90 * 24 * 60 * 60 * 1000;

/// メンバー側キャッシュの本文合計サイズ上限(バイト)。超えたら古いメモから
/// 削除する(ホストは常に正本を持つのでキャッシュは失っても再同期できる)。
pub const CACHE_MAX_BYTES: u64 = 50 * 1024 * 1024;

/// 操作の主体。権限判定に使う(ホスト管理者はすべて可)。
#[derive(Debug, Clone)]
pub struct Actor {
    /// member_id(= invite_id、ADR-0047)。None = ホスト管理者。
    pub member_id: Option<String>,
    /// 表示名(更新者・所有者のスナップショットに使う)。
    pub name: String,
    /// 現在この操作主体が属しているグループの id 一覧(ADR-0051)。
    /// 解決はサービス層(ホスト)が「対象メンバーの現在の仮想 IP が
    /// GroupInfo.members に含まれるか」で行い、ここには解決済みの
    /// id を渡す(ストア層はグループの実体を知らない)。
    pub group_ids: Vec<String>,
}

impl Actor {
    pub fn host(name: impl Into<String>) -> Self {
        Self {
            member_id: None,
            name: name.into(),
            group_ids: Vec::new(),
        }
    }

    pub fn member(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            member_id: Some(id.into()),
            name: name.into(),
            group_ids: Vec::new(),
        }
    }

    /// 所属グループの id 一覧を付与する(ホスト側サービス層が解決して渡す)。
    pub fn with_groups(mut self, ids: Vec<String>) -> Self {
        self.group_ids = ids;
        self
    }

    fn is_host(&self) -> bool {
        self.member_id.is_none()
    }

    /// 所有者 ID としての表現(ホスト = 空文字)。
    fn owner_id(&self) -> &str {
        self.member_id.as_deref().unwrap_or("")
    }
}

/// `ALTER TABLE ... ADD COLUMN` は非冪等(2 回実行すると `duplicate column`
/// エラー)なので、テストのようにスキーマバージョンを巻き戻して migrate を
/// 再実行するケースに備えて事前に列の有無を確認する。
fn table_has_column(conn: &Connection, table: &str, column: &str) -> anyhow::Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let has_column = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(Result::ok)
        .any(|name| name == column);
    Ok(has_column)
}

const SHARED_SCHEMA_VERSION: i64 = 7;

fn level_to_str(level: SharedPermLevel) -> &'static str {
    match level {
        SharedPermLevel::None => "none",
        SharedPermLevel::Viewer => "viewer",
        SharedPermLevel::Editor => "editor",
    }
}

fn level_from_str(value: &str) -> SharedPermLevel {
    match value {
        "editor" => SharedPermLevel::Editor,
        "none" => SharedPermLevel::None,
        _ => SharedPermLevel::Viewer,
    }
}

/// 1 メモ分の権限計算の材料。
struct Row {
    id: String,
    title: String,
    body: String,
    folder_id: Option<String>,
    revision: i64,
    owner_id: String,
    owner_name: String,
    created_at: i64,
    updated_at: i64,
    updated_by: Option<String>,
    everyone: SharedPermLevel,
    deleted_at: Option<i64>,
    comment_count: u32,
}

/// 1 メモ分の権限計算に要る材料(メンバー個別 + グループ)。ホスト 1 回の
/// 問い合わせで両方まとめて取るための入れ物(`SharedStore::perm_context`)。
struct PermContext {
    /// member_id → 個別権限。
    levels: HashMap<String, SharedPermLevel>,
    /// member_id → 表示名スナップショット。
    names: HashMap<String, String>,
    /// group_id → グループ権限。
    group_levels: HashMap<String, SharedPermLevel>,
    /// グループ権限の全量(表示用。名前順)。
    groups: Vec<SharedGroupPerm>,
}

impl Row {
    /// 判定の優先順位は**メンバー個別 > グループ(該当する複数グループの
    /// 最大)> 全体**(ADR-0051)。個別指定があれば(None も含めて)それを
    /// 使い切る。個別指定が無く、所属グループに 1 つでも明示的な権限が
    /// あれば(None を含む)その最大値を使う。どちらも無ければ全体権限。
    fn effective_level(&self, actor: &Actor, ctx: &PermContext) -> SharedPermLevel {
        if let Some(level) = ctx.levels.get(actor.owner_id()) {
            return *level;
        }
        let group_max = actor
            .group_ids
            .iter()
            .filter_map(|id| ctx.group_levels.get(id))
            .copied()
            .max();
        if let Some(level) = group_max {
            return level;
        }
        self.everyone
    }

    fn visible_to(&self, actor: &Actor, ctx: &PermContext) -> bool {
        if actor.is_host() || self.owner_id == actor.owner_id() {
            return true;
        }
        if self.deleted_at.is_some() {
            return false; // ゴミ箱は所有者・ホストのみ
        }
        self.effective_level(actor, ctx) != SharedPermLevel::None
    }

    fn can_edit(&self, actor: &Actor, ctx: &PermContext) -> bool {
        if self.deleted_at.is_some() {
            return false; // ゴミ箱は読み取り専用
        }
        if actor.is_host() || self.owner_id == actor.owner_id() {
            return true;
        }
        self.effective_level(actor, ctx) == SharedPermLevel::Editor
    }

    fn can_manage(&self, actor: &Actor) -> bool {
        actor.is_host() || self.owner_id == actor.owner_id()
    }

    fn summary(&self, actor: &Actor, ctx: &PermContext) -> SharedMemoSummary {
        let (done, total) = checklist_progress(&self.body);
        SharedMemoSummary {
            id: self.id.clone(),
            title: self.title.clone(),
            excerpt: excerpt(&self.body, EXCERPT_CHARS),
            folder_id: self.folder_id.clone(),
            revision: self.revision as u64,
            created_at: self.created_at as u64,
            updated_at: self.updated_at as u64,
            updated_by: self.updated_by.clone(),
            owner_id: self.owner_id.clone(),
            owner_name: self.owner_name.clone(),
            deleted_at: self.deleted_at.map(|v| v as u64),
            can_edit: self.can_edit(actor, ctx),
            can_manage: self.can_manage(actor),
            locked_by: None, // サービス層(ロック保持者)が詰める
            checklist_done: done,
            checklist_total: total,
            comment_count: self.comment_count,
        }
    }

    fn detail(&self, actor: &Actor, ctx: &PermContext) -> SharedMemoDetail {
        let manage = self.can_manage(actor);
        SharedMemoDetail {
            id: self.id.clone(),
            title: self.title.clone(),
            body: self.body.clone(),
            folder_id: self.folder_id.clone(),
            revision: self.revision as u64,
            created_at: self.created_at as u64,
            updated_at: self.updated_at as u64,
            updated_by: self.updated_by.clone(),
            owner_id: self.owner_id.clone(),
            owner_name: self.owner_name.clone(),
            deleted_at: self.deleted_at.map(|v| v as u64),
            can_edit: self.can_edit(actor, ctx),
            can_manage: manage,
            locked_by: None,
            everyone: manage.then_some(self.everyone),
            members: if manage {
                let mut members: Vec<SharedMemberPerm> = ctx
                    .levels
                    .iter()
                    .map(|(member_id, level)| SharedMemberPerm {
                        member_id: member_id.clone(),
                        name: ctx.names.get(member_id).cloned().unwrap_or_default(),
                        level: *level,
                    })
                    .collect();
                members.sort_by(|a, b| a.name.cmp(&b.name).then(a.member_id.cmp(&b.member_id)));
                members
            } else {
                Vec::new()
            },
            groups: if manage {
                ctx.groups.clone()
            } else {
                Vec::new()
            },
            comment_count: self.comment_count,
        }
    }
}

/// [`SharedStore::db_stats`] の戻り値(診断用。ワイヤ型ではないので
/// serde 派生は不要)。
#[derive(Debug, Clone)]
pub struct SharedMemoDbStats {
    pub db_bytes: u64,
    pub wal_bytes: u64,
    pub memo_count: u64,
    pub trashed_count: u64,
    pub history_count: u64,
    pub total_body_bytes: u64,
    pub limits: SharedMemoLimits,
    /// 共有スケジュール表の予定件数(ADR-0053)。
    pub schedule_count: u64,
    /// 共有シートの枚数(ADR-0054)。
    pub sheet_count: u64,
}

/// ホスト正本の共有メモ DB。
pub struct SharedStore {
    conn: Connection,
    path: PathBuf,
}

impl SharedStore {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let conn = open_db(path)?;
        let mut store = Self {
            conn,
            path: path.to_path_buf(),
        };
        store.migrate()?;
        // 保持期限を過ぎたゴミ箱を自動で完全削除(要件 §13/§17。M5 F-3 で
        // 保持日数はホスト設定可になったため purge_expired() へ一本化)
        store.purge_expired()?;
        Ok(store)
    }

    fn migrate(&mut self) -> anyhow::Result<()> {
        let version: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version >= SHARED_SCHEMA_VERSION {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        if version < 1 {
            tx.execute_batch(
                r#"
                CREATE TABLE memos (
                    id TEXT PRIMARY KEY,
                    title TEXT NOT NULL DEFAULT '',
                    body TEXT NOT NULL DEFAULT '',
                    folder_id TEXT,
                    revision INTEGER NOT NULL DEFAULT 1,
                    owner_id TEXT NOT NULL DEFAULT '',
                    owner_name TEXT NOT NULL DEFAULT '',
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    updated_by TEXT,
                    everyone TEXT NOT NULL DEFAULT 'viewer',
                    deleted_at INTEGER
                );
                CREATE TABLE folders (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    created_at INTEGER NOT NULL
                );
                CREATE TABLE memo_perms (
                    memo_id TEXT NOT NULL REFERENCES memos(id) ON DELETE CASCADE,
                    member_id TEXT NOT NULL,
                    name TEXT NOT NULL DEFAULT '',
                    level TEXT NOT NULL,
                    PRIMARY KEY (memo_id, member_id)
                );
                CREATE VIRTUAL TABLE memo_fts USING fts5(title, body, tokenize='trigram');
                CREATE TRIGGER memos_fts_insert AFTER INSERT ON memos BEGIN
                    INSERT INTO memo_fts(rowid, title, body)
                        VALUES (new.rowid, kana_fold(new.title), kana_fold(new.body));
                END;
                CREATE TRIGGER memos_fts_delete AFTER DELETE ON memos BEGIN
                    DELETE FROM memo_fts WHERE rowid = old.rowid;
                END;
                CREATE TRIGGER memos_fts_update AFTER UPDATE OF title, body ON memos BEGIN
                    DELETE FROM memo_fts WHERE rowid = old.rowid;
                    INSERT INTO memo_fts(rowid, title, body)
                        VALUES (new.rowid, kana_fold(new.title), kana_fold(new.body));
                END;
                "#,
            )?;
        }
        if version < 2 {
            // v2(M5 F-3): 変更履歴・容量上限(settings)・削除済み ID 台帳。
            // 既存テーブルはそのまま、追加のみ
            tx.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS settings (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );
                CREATE TABLE IF NOT EXISTS memo_history (
                    hid INTEGER PRIMARY KEY AUTOINCREMENT,
                    memo_id TEXT NOT NULL,
                    revision INTEGER NOT NULL,
                    kind TEXT NOT NULL,
                    saved_by_id TEXT NOT NULL DEFAULT '',
                    saved_by_name TEXT NOT NULL,
                    title TEXT NOT NULL,
                    body TEXT NOT NULL,
                    created_at INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_memo_history ON memo_history(memo_id, created_at);
                CREATE TABLE IF NOT EXISTS deleted_memos (
                    memo_id TEXT PRIMARY KEY,
                    deleted_at INTEGER NOT NULL
                );
                "#,
            )?;
        }
        if version < 3 {
            // v3(M5 F-4、ADR-0051): グループ単位の権限。既存テーブルは
            // そのまま、追加のみ(FK は memo_perms と違って持たない —
            // 完全削除は purge_memo が手動で対で消す)
            tx.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS memo_group_perms (
                    memo_id TEXT NOT NULL,
                    group_id TEXT NOT NULL,
                    name TEXT NOT NULL,
                    level TEXT NOT NULL,
                    PRIMARY KEY (memo_id, group_id)
                );
                "#,
            )?;
        }
        if version < 4 {
            // v4(M5 F-5、ADR-0052 決定 4): 共有メモのコメント(単層)。
            // 容量計算(total_body_bytes)にはコメントを含めない
            // (上限 4KiB×500 = 高々 2MiB/メモで別勘定にしても実害がないため)。
            tx.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS memo_comments (
                    comment_id TEXT PRIMARY KEY,
                    memo_id TEXT NOT NULL,
                    author_id TEXT NOT NULL DEFAULT '',
                    author_name TEXT NOT NULL DEFAULT '',
                    body TEXT NOT NULL,
                    created_at INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_memo_comments ON memo_comments(memo_id, created_at);
                "#,
            )?;
        }
        if version < 5 {
            // v5(M6 G-1、ADR-0053): 共有スケジュール表。ゴミ箱なし(物理削除)、
            // 編集ロックも持たない(revision CAS のみ)。
            tx.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS schedule_events (
                    id TEXT PRIMARY KEY,
                    title TEXT NOT NULL DEFAULT '',
                    note TEXT NOT NULL DEFAULT '',
                    start_at INTEGER NOT NULL,
                    end_at INTEGER,
                    all_day INTEGER NOT NULL DEFAULT 0,
                    owner_id TEXT NOT NULL DEFAULT '',
                    owner_name TEXT NOT NULL DEFAULT '',
                    updated_by TEXT NOT NULL DEFAULT '',
                    revision INTEGER NOT NULL DEFAULT 1,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_schedule_events_start ON schedule_events(start_at);
                "#,
            )?;
        }
        if version < 6 {
            // v6(M6 G-2、ADR-0054): 共有シート(Excel ライク表)。アドレスは
            // 固定(挿入/削除による繰り上げは持たない)。セルは疎な格納
            // (非空セルだけが存在する。空文字への更新 = セル削除)。
            tx.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS sheets (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL DEFAULT '',
                    owner_id TEXT NOT NULL DEFAULT '',
                    owner_name TEXT NOT NULL DEFAULT '',
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS sheet_cells (
                    sheet_id TEXT NOT NULL,
                    row INTEGER NOT NULL,
                    col INTEGER NOT NULL,
                    value TEXT NOT NULL DEFAULT '',
                    revision INTEGER NOT NULL DEFAULT 1,
                    updated_by TEXT NOT NULL DEFAULT '',
                    updated_at INTEGER NOT NULL,
                    PRIMARY KEY (sheet_id, row, col)
                );
                CREATE INDEX IF NOT EXISTS idx_sheet_cells_sheet ON sheet_cells(sheet_id);
                "#,
            )?;
        }
        if version < 7 {
            // v7(M6 H-3、ADR-0055 決定 5): 予定の参加メンバー。JSON 文字列で
            // 格納する(ScheduleParticipant の配列)。旧クライアントはこの
            // 列を知らないため additive フィールドとして無視するだけで
            // 互換動作する。ALTER TABLE ADD COLUMN は非冪等なので、テストの
            // ようにバージョンを巻き戻して再実行される場合に備えて存在確認
            // してから実行する。
            if !table_has_column(&tx, "schedule_events", "participants")? {
                tx.execute_batch(
                    "ALTER TABLE schedule_events ADD COLUMN participants TEXT NOT NULL DEFAULT '[]';",
                )?;
            }
        }
        tx.pragma_update(None, "user_version", SHARED_SCHEMA_VERSION)?;
        tx.commit()?;
        Ok(())
    }

    fn row(&self, id: &str) -> anyhow::Result<Row> {
        self.conn
            .query_row(
                &format!("SELECT {ROW_COLUMNS} FROM memos WHERE id = ?1"),
                params![id],
                row_from_sql,
            )
            .optional()?
            .context("共有メモが見つかりません(削除された可能性があります)")
    }

    fn perms_of(
        &self,
        id: &str,
    ) -> anyhow::Result<(HashMap<String, SharedPermLevel>, HashMap<String, String>)> {
        let mut stmt = self
            .conn
            .prepare("SELECT member_id, name, level FROM memo_perms WHERE memo_id = ?1")?;
        let mut levels = HashMap::new();
        let mut names = HashMap::new();
        let rows = stmt.query_map(params![id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (member_id, name, level) = row?;
            levels.insert(member_id.clone(), level_from_str(&level));
            names.insert(member_id, name);
        }
        Ok((levels, names))
    }

    /// グループ権限(group_id → level、名前順の全量)。
    fn group_perms_of(
        &self,
        id: &str,
    ) -> anyhow::Result<(HashMap<String, SharedPermLevel>, Vec<SharedGroupPerm>)> {
        let mut stmt = self
            .conn
            .prepare("SELECT group_id, name, level FROM memo_group_perms WHERE memo_id = ?1")?;
        let mut levels = HashMap::new();
        let mut list = Vec::new();
        let rows = stmt.query_map(params![id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (group_id, name, level) = row?;
            let level = level_from_str(&level);
            levels.insert(group_id.clone(), level);
            list.push(SharedGroupPerm {
                group_id,
                name,
                level,
            });
        }
        list.sort_by(|a, b| a.name.cmp(&b.name).then(a.group_id.cmp(&b.group_id)));
        Ok((levels, list))
    }

    /// メンバー個別 + グループの権限材料を 1 か所で取得する。
    fn perm_context(&self, id: &str) -> anyhow::Result<PermContext> {
        let (levels, names) = self.perms_of(id)?;
        let (group_levels, groups) = self.group_perms_of(id)?;
        Ok(PermContext {
            levels,
            names,
            group_levels,
            groups,
        })
    }

    /// グループ権限を持つメモの id 一覧(重複なし)。グループの改名・
    /// メンバー増減・削除に追従した再配信の対象探索に使う(ADR-0051、
    /// `MemoService::watch_groups`)。
    pub fn memo_ids_with_group_perms(&self) -> anyhow::Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT memo_id FROM memo_group_perms")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// 一覧(受信者視点)。ゴミ箱は所有者・ホストの分だけ。
    pub fn list(
        &self,
        actor: &Actor,
        query: &SharedMemoQuery,
    ) -> anyhow::Result<(Vec<SharedMemoSummary>, Vec<MemoFolder>)> {
        let mut sql = format!("SELECT {ROW_COLUMNS_ALIASED} FROM memos m WHERE ");
        let mut clauses: Vec<String> = Vec::new();
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if query.trash {
            clauses.push("m.deleted_at IS NOT NULL".to_string());
        } else {
            clauses.push("m.deleted_at IS NULL".to_string());
        }
        if let Some(folder) = &query.folder_id {
            args.push(Box::new(folder.clone()));
            clauses.push(format!("m.folder_id = ?{}", args.len()));
        }
        if let Some(search) = query
            .search
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            push_search_clause(&mut clauses, &mut args, search);
        }
        sql.push_str(&clauses.join(" AND "));
        sql.push_str(match query.trash {
            true => " ORDER BY m.deleted_at DESC",
            false => " ORDER BY m.updated_at DESC",
        });

        let mut stmt = self.conn.prepare(&sql)?;
        let params_ref: Vec<&dyn rusqlite::ToSql> = args.iter().map(AsRef::as_ref).collect();
        let rows = stmt
            .query_map(params_ref.as_slice(), row_from_sql)?
            .collect::<Result<Vec<_>, _>>()?;

        let mut memos = Vec::new();
        for row in rows {
            let ctx = self.perm_context(&row.id)?;
            if row.visible_to(actor, &ctx) {
                memos.push(row.summary(actor, &ctx));
            }
        }
        Ok((memos, self.folders()?))
    }

    /// 1 件取得(受信者視点)。見えないメモはエラー。
    pub fn get(&self, actor: &Actor, id: &str) -> anyhow::Result<SharedMemoDetail> {
        let row = self.row(id)?;
        let ctx = self.perm_context(id)?;
        if !row.visible_to(actor, &ctx) {
            bail!("このメモを閲覧する権限がありません");
        }
        Ok(row.detail(actor, &ctx))
    }

    /// 受信者視点の詳細(見えなければ None)。イベント配信のフィルタ用。
    pub fn detail_if_visible(
        &self,
        actor: &Actor,
        id: &str,
    ) -> anyhow::Result<Option<SharedMemoDetail>> {
        let Some(row) = self
            .conn
            .query_row(
                &format!("SELECT {ROW_COLUMNS} FROM memos WHERE id = ?1"),
                params![id],
                row_from_sql,
            )
            .optional()?
        else {
            return Ok(None);
        };
        // ゴミ箱入りは配信対象外(Removed イベントで消す)
        if row.deleted_at.is_some() {
            return Ok(None);
        }
        let ctx = self.perm_context(id)?;
        if !row.visible_to(actor, &ctx) {
            return Ok(None);
        }
        Ok(Some(row.detail(actor, &ctx)))
    }

    /// メモ間リンク `[[タイトル]]`(ADR-0052 決定 2)の解決。タイトル →
    /// memo_id(見つかったものだけ。ゴミ箱除外、複数一致は updated_at
    /// 最新)。**可視性フィルタあり**(actor に見えないメモは結果から除く。
    /// 次に新しい候補へフォールバックはしない = シンプル優先)。
    pub fn resolve_titles(
        &self,
        actor: &Actor,
        titles: &[String],
    ) -> anyhow::Result<HashMap<String, String>> {
        let mut seen = std::collections::HashSet::new();
        let mut out = HashMap::new();
        for title in titles {
            if title.is_empty() || !seen.insert(title.clone()) {
                continue;
            }
            let row: Option<Row> = self
                .conn
                .query_row(
                    &format!(
                        "SELECT {ROW_COLUMNS} FROM memos WHERE title = ?1 AND deleted_at IS NULL
                         ORDER BY updated_at DESC LIMIT 1"
                    ),
                    params![title],
                    row_from_sql,
                )
                .optional()?;
            let Some(row) = row else { continue };
            let ctx = self.perm_context(&row.id)?;
            if row.visible_to(actor, &ctx) {
                out.insert(title.clone(), row.id.clone());
            }
        }
        Ok(out)
    }

    /// バックリンク: 本文に `[[<このメモのタイトル>]]` をリテラルとして含む、
    /// actor に見えるメモの一覧(自分自身・ゴミ箱は除く)。タイトルが空なら
    /// 対象外。
    pub fn backlinks(&self, actor: &Actor, id: &str) -> anyhow::Result<Vec<SharedMemoSummary>> {
        let target = self.row(id)?;
        let target_ctx = self.perm_context(id)?;
        if !target.visible_to(actor, &target_ctx) {
            bail!("このメモを閲覧する権限がありません");
        }
        if target.title.is_empty() {
            return Ok(Vec::new());
        }
        let pattern = format!("%[[{}]]%", escape_like(&target.title));
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {ROW_COLUMNS} FROM memos
             WHERE deleted_at IS NULL AND id != ?1 AND body LIKE ?2 ESCAPE '\\'
             ORDER BY updated_at DESC"
        ))?;
        let rows = stmt
            .query_map(params![id, pattern], row_from_sql)?
            .collect::<Result<Vec<_>, _>>()?;
        let mut out = Vec::new();
        for row in rows {
            let ctx = self.perm_context(&row.id)?;
            if row.visible_to(actor, &ctx) {
                out.push(row.summary(actor, &ctx));
            }
        }
        Ok(out)
    }

    pub fn create(
        &mut self,
        actor: &Actor,
        title: &str,
        body: &str,
        folder_id: Option<&str>,
    ) -> anyhow::Result<SharedMemoDetail> {
        validate_title(title)?;
        let limits = self.limits()?;
        validate_body_bytes(body, limits.max_body_bytes as usize)?;
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM memos", [], |row| row.get(0))?;
        if count as u32 >= limits.max_memo_count {
            bail!(
                "共有メモの件数が上限({} 件)に達しています。不要なメモを完全削除するか、ホストの上限設定を変更してください",
                limits.max_memo_count
            );
        }
        self.check_total_capacity(&limits, body.len() as u64)?;
        if let Some(folder) = folder_id {
            let exists: bool = self
                .conn
                .query_row(
                    "SELECT 1 FROM folders WHERE id = ?1",
                    params![folder],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if !exists {
                bail!("指定のフォルダーが見つかりません");
            }
        }
        let id: String = self
            .conn
            .query_row("SELECT lower(hex(randomblob(8)))", [], |row| row.get(0))?;
        let now = unix_ms();
        self.conn.execute(
            "INSERT INTO memos (id, title, body, folder_id, owner_id, owner_name,
                                created_at, updated_at, updated_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?6)",
            params![
                id,
                title,
                body,
                folder_id,
                actor.owner_id(),
                actor.name,
                now
            ],
        )?;
        self.get(actor, &id)
    }

    /// 本文・タイトルの更新(CAS)。権限とリビジョンを検査する。
    /// 編集ロックの検査は呼び出し側(サービス層)が行う。
    pub fn update(
        &mut self,
        actor: &Actor,
        id: &str,
        base_revision: u64,
        title: &str,
        body: &str,
    ) -> anyhow::Result<SharedMemoDetail> {
        validate_title(title)?;
        let limits = self.limits()?;
        validate_body_bytes(body, limits.max_body_bytes as usize)?;
        let row = self.row(id)?;
        let ctx = self.perm_context(id)?;
        if !row.visible_to(actor, &ctx) {
            bail!("このメモを閲覧する権限がありません");
        }
        if !row.can_edit(actor, &ctx) {
            bail!("このメモを編集する権限がありません(閲覧のみ)");
        }
        if row.revision as u64 != base_revision {
            bail!("competing_edit: 他の端末の変更が先に保存されています(最新を読み込み直してください)");
        }
        self.check_total_capacity(&limits, body.len() as u64)?;
        // 本文を上書きする前に、必要なら現在の内容を自動履歴として残す
        self.maybe_auto_snapshot(id)?;
        self.conn.execute(
            "UPDATE memos SET title = ?1, body = ?2, revision = revision + 1,
                    updated_at = ?3, updated_by = ?4
             WHERE id = ?5",
            params![title, body, unix_ms(), actor.name, id],
        )?;
        self.get(actor, id)
    }

    /// 変更履歴を 1 版残す(内部用)。「最終更新者」は現在行の updated_by
    /// (無ければ owner)のスナップショット。
    fn snapshot_history(&self, memo_id: &str, kind: &str) -> anyhow::Result<()> {
        let row = self.row(memo_id)?;
        let saved_by_name = row
            .updated_by
            .clone()
            .unwrap_or_else(|| row.owner_name.clone());
        self.conn.execute(
            "INSERT INTO memo_history (memo_id, revision, kind, saved_by_name, title, body, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![memo_id, row.revision, kind, saved_by_name, row.title, row.body, unix_ms()],
        )?;
        let limits = self.limits()?;
        self.trim_history(memo_id, &limits)?;
        Ok(())
    }

    /// メモごとの変更履歴を、件数上限・保持日数の両方で刈り込む。
    fn trim_history(&self, memo_id: &str, limits: &SharedMemoLimits) -> anyhow::Result<()> {
        self.conn.execute(
            "DELETE FROM memo_history WHERE memo_id = ?1 AND hid NOT IN (
                SELECT hid FROM memo_history WHERE memo_id = ?1
                ORDER BY created_at DESC, hid DESC LIMIT ?2
             )",
            params![memo_id, limits.max_versions as i64],
        )?;
        let cutoff = unix_ms() - (limits.history_days as i64) * 24 * 60 * 60 * 1000;
        self.conn.execute(
            "DELETE FROM memo_history WHERE memo_id = ?1 AND created_at < ?2",
            params![memo_id, cutoff],
        )?;
        Ok(())
    }

    /// そのメモの最新履歴が無い、または直近の自動保存間隔より古い場合だけ
    /// "auto" 版を残す。編集セッション中は約 10 分おきに 1 版残る想定。
    pub fn maybe_auto_snapshot(&self, memo_id: &str) -> anyhow::Result<()> {
        let latest: Option<i64> = self
            .conn
            .query_row(
                "SELECT created_at FROM memo_history WHERE memo_id = ?1
                 ORDER BY created_at DESC, hid DESC LIMIT 1",
                params![memo_id],
                |row| row.get(0),
            )
            .optional()?;
        let stale = match latest {
            None => true,
            Some(created_at) => unix_ms() - created_at > SHARED_HISTORY_INTERVAL_MS,
        };
        if stale {
            self.snapshot_history(memo_id, "auto")?;
        }
        Ok(())
    }

    /// 現在 revision が `since_revision` と異なる時だけ "close" 版を残す
    /// (編集ロック解放時に呼ばれる想定。サービス層がロック取得時の
    /// revision を渡す)。
    pub fn snapshot_if_revision_changed(
        &self,
        memo_id: &str,
        since_revision: u64,
    ) -> anyhow::Result<()> {
        let row = self.row(memo_id)?;
        if row.revision as u64 != since_revision {
            self.snapshot_history(memo_id, "close")?;
        }
        Ok(())
    }

    /// 現在の内容を手動で履歴に残す(can_edit 必須)。直近の履歴が現在
    /// revision と同じなら何もしない(重複防止)。
    pub fn save_version(&self, actor: &Actor, id: &str) -> anyhow::Result<()> {
        let row = self.row(id)?;
        let ctx = self.perm_context(id)?;
        if !row.can_edit(actor, &ctx) {
            bail!("このメモを編集する権限がありません(閲覧のみ)");
        }
        let latest_revision: Option<i64> = self
            .conn
            .query_row(
                "SELECT revision FROM memo_history WHERE memo_id = ?1
                 ORDER BY created_at DESC, hid DESC LIMIT 1",
                params![id],
                |row| row.get(0),
            )
            .optional()?;
        if latest_revision == Some(row.revision) {
            return Ok(());
        }
        self.snapshot_history(id, "manual")
    }

    /// 変更履歴の一覧(新しい順)。そのメモが見える人なら閲覧可。
    pub fn history_list(
        &self,
        actor: &Actor,
        id: &str,
    ) -> anyhow::Result<Vec<SharedMemoHistoryEntry>> {
        let row = self.row(id)?;
        let ctx = self.perm_context(id)?;
        if !row.visible_to(actor, &ctx) {
            bail!("このメモを閲覧する権限がありません");
        }
        let mut stmt = self.conn.prepare(
            "SELECT hid, revision, kind, saved_by_name, created_at, title,
                    length(CAST(body AS BLOB))
             FROM memo_history WHERE memo_id = ?1 ORDER BY created_at DESC, hid DESC",
        )?;
        let entries = stmt
            .query_map(params![id], |row| {
                Ok(SharedMemoHistoryEntry {
                    hid: row.get(0)?,
                    revision: row.get::<_, i64>(1)? as u64,
                    kind: row.get(2)?,
                    saved_by_name: row.get(3)?,
                    created_at_unix_ms: row.get::<_, i64>(4)? as u64,
                    title: row.get(5)?,
                    body_bytes: row.get::<_, i64>(6)? as u64,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(entries)
    }

    /// 変更履歴 1 版の本文取得。
    pub fn history_get(
        &self,
        actor: &Actor,
        id: &str,
        hid: i64,
    ) -> anyhow::Result<SharedMemoHistoryDetail> {
        let row = self.row(id)?;
        let ctx = self.perm_context(id)?;
        if !row.visible_to(actor, &ctx) {
            bail!("このメモを閲覧する権限がありません");
        }
        let (entry, body) = self
            .conn
            .query_row(
                "SELECT hid, revision, kind, saved_by_name, created_at, title, body
                 FROM memo_history WHERE hid = ?1 AND memo_id = ?2",
                params![hid, id],
                |row| {
                    let body: String = row.get(6)?;
                    Ok((
                        SharedMemoHistoryEntry {
                            hid: row.get(0)?,
                            revision: row.get::<_, i64>(1)? as u64,
                            kind: row.get(2)?,
                            saved_by_name: row.get(3)?,
                            created_at_unix_ms: row.get::<_, i64>(4)? as u64,
                            title: row.get(5)?,
                            body_bytes: body.len() as u64,
                        },
                        body,
                    ))
                },
            )
            .optional()?
            .context("履歴が見つかりません(保持期限切れの可能性があります)")?;
        Ok(SharedMemoHistoryDetail { entry, body })
    }

    /// `from_hid` の本文 →(`to_hid` の本文 or 現在の本文)の行差分。
    pub fn history_diff(
        &self,
        actor: &Actor,
        id: &str,
        from_hid: i64,
        to_hid: Option<i64>,
    ) -> anyhow::Result<Vec<DiffLine>> {
        let row = self.row(id)?;
        let ctx = self.perm_context(id)?;
        if !row.visible_to(actor, &ctx) {
            bail!("このメモを閲覧する権限がありません");
        }
        let from_body: String = self
            .conn
            .query_row(
                "SELECT body FROM memo_history WHERE hid = ?1 AND memo_id = ?2",
                params![from_hid, id],
                |row| row.get(0),
            )
            .optional()?
            .context("履歴が見つかりません(保持期限切れの可能性があります)")?;
        let to_body = match to_hid {
            Some(hid) => self
                .conn
                .query_row(
                    "SELECT body FROM memo_history WHERE hid = ?1 AND memo_id = ?2",
                    params![hid, id],
                    |row| row.get(0),
                )
                .optional()?
                .context("履歴が見つかりません(保持期限切れの可能性があります)")?,
            None => row.body.clone(),
        };
        Ok(diff_lines(&from_body, &to_body))
    }

    /// 指定した版の内容へ復元する(can_edit 必須)。復元前の内容は
    /// "restore" 版として保全してから書き戻す。編集ロックの検査は
    /// 呼び出し側(サービス層)が行う。
    pub fn history_restore(
        &self,
        actor: &Actor,
        id: &str,
        hid: i64,
    ) -> anyhow::Result<SharedMemoDetail> {
        let row = self.row(id)?;
        let ctx = self.perm_context(id)?;
        if !row.visible_to(actor, &ctx) {
            bail!("このメモを閲覧する権限がありません");
        }
        if !row.can_edit(actor, &ctx) {
            bail!("このメモを編集する権限がありません(閲覧のみ)");
        }
        let (hist_title, hist_body): (String, String) = self
            .conn
            .query_row(
                "SELECT title, body FROM memo_history WHERE hid = ?1 AND memo_id = ?2",
                params![hid, id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .context("履歴が見つかりません(保持期限切れの可能性があります)")?;
        validate_title(&hist_title)?;
        let limits = self.limits()?;
        validate_body_bytes(&hist_body, limits.max_body_bytes as usize)?;
        self.check_total_capacity(&limits, hist_body.len() as u64)?;
        // 復元前の内容を保全してから書き戻す
        self.snapshot_history(id, "restore")?;
        self.conn.execute(
            "UPDATE memos SET title = ?1, body = ?2, revision = revision + 1,
                    updated_at = ?3, updated_by = ?4
             WHERE id = ?5",
            params![hist_title, hist_body, unix_ms(), actor.name, id],
        )?;
        self.get(actor, id)
    }

    /// 本文 + 履歴本文の合計サイズ(バイト)。
    fn total_body_bytes(&self) -> anyhow::Result<u64> {
        let memos_bytes: i64 = self.conn.query_row(
            "SELECT COALESCE(SUM(length(CAST(body AS BLOB))), 0) FROM memos",
            [],
            |row| row.get(0),
        )?;
        let history_bytes: i64 = self.conn.query_row(
            "SELECT COALESCE(SUM(length(CAST(body AS BLOB))), 0) FROM memo_history",
            [],
            |row| row.get(0),
        )?;
        Ok((memos_bytes + history_bytes).max(0) as u64)
    }

    /// 合計使用量 + 追加分が上限を超えないか検査する(黙って失敗させない)。
    /// 更新の場合の「元の本文が減る分」までは厳密に差し引かず、常に
    /// 追加分をそのまま加算する側に倒す(安全側の見積もり)。
    fn check_total_capacity(
        &self,
        limits: &SharedMemoLimits,
        added_bytes: u64,
    ) -> anyhow::Result<()> {
        let current = self.total_body_bytes()?;
        if current + added_bytes > limits.max_total_bytes {
            bail!(
                "共有メモの全体容量が上限({} MiB)に達しています。不要なメモや履歴を整理するか、ホストの上限設定を変更してください",
                limits.max_total_bytes / (1024 * 1024)
            );
        }
        Ok(())
    }

    /// 共有メモの容量・履歴上限を取得する(未設定のキーは既定値)。
    pub fn limits(&self) -> anyhow::Result<SharedMemoLimits> {
        let mut limits = SharedMemoLimits::default();
        let mut stmt = self.conn.prepare(
            "SELECT key, value FROM settings WHERE key IN (
                'max_body_bytes', 'max_memo_count', 'max_total_bytes',
                'max_versions', 'history_days', 'trash_days'
             )",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (key, value) = row?;
            match key.as_str() {
                "max_body_bytes" => {
                    if let Ok(v) = value.parse() {
                        limits.max_body_bytes = v;
                    }
                }
                "max_memo_count" => {
                    if let Ok(v) = value.parse() {
                        limits.max_memo_count = v;
                    }
                }
                "max_total_bytes" => {
                    if let Ok(v) = value.parse() {
                        limits.max_total_bytes = v;
                    }
                }
                "max_versions" => {
                    if let Ok(v) = value.parse() {
                        limits.max_versions = v;
                    }
                }
                "history_days" => {
                    if let Ok(v) = value.parse() {
                        limits.history_days = v;
                    }
                }
                "trash_days" => {
                    if let Ok(v) = value.parse() {
                        limits.trash_days = v;
                    }
                }
                _ => {}
            }
        }
        Ok(limits)
    }

    /// 共有メモの容量・履歴上限を設定する(ホスト管理者のみ)。
    pub fn set_limits(&self, actor: &Actor, limits: &SharedMemoLimits) -> anyhow::Result<()> {
        if !actor.is_host() {
            bail!("共有メモの上限設定を変更できるのはホスト管理者だけです");
        }
        limits
            .validate()
            .map_err(|reason| anyhow::anyhow!(reason))?;
        for (key, value) in [
            ("max_body_bytes", limits.max_body_bytes.to_string()),
            ("max_memo_count", limits.max_memo_count.to_string()),
            ("max_total_bytes", limits.max_total_bytes.to_string()),
            ("max_versions", limits.max_versions.to_string()),
            ("history_days", limits.history_days.to_string()),
            ("trash_days", limits.trash_days.to_string()),
        ] {
            self.conn.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, value],
            )?;
        }
        Ok(())
    }

    /// ゴミ箱保持期限を過ぎたメモを完全削除し、削除済み ID 台帳を掃除する。
    /// 戻り値は完全削除したメモ件数。`open()` からも呼ばれる。
    pub fn purge_expired(&self) -> anyhow::Result<u64> {
        let limits = self.limits()?;
        let trash_cutoff = unix_ms() - (limits.trash_days as i64) * 24 * 60 * 60 * 1000;
        let ids: Vec<String> = {
            let mut stmt = self
                .conn
                .prepare("SELECT id FROM memos WHERE deleted_at IS NOT NULL AND deleted_at < ?1")?;
            let rows = stmt.query_map(params![trash_cutoff], |row| row.get(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let now = unix_ms();
        for id in &ids {
            self.purge_memo(id, now)?;
        }
        let tombstone_cutoff = now - TOMBSTONE_RETENTION_MS;
        self.conn.execute(
            "DELETE FROM deleted_memos WHERE deleted_at < ?1",
            params![tombstone_cutoff],
        )?;
        Ok(ids.len() as u64)
    }

    /// メモ本体 + 履歴を完全に消し、削除済み ID 台帳へ記録する(内部用)。
    fn purge_memo(&self, id: &str, now: i64) -> anyhow::Result<()> {
        self.conn
            .execute("DELETE FROM memo_history WHERE memo_id = ?1", params![id])?;
        // memo_group_perms / memo_comments は memo_perms と違って FK CASCADE を
        // 持たないため(v3/v4 マイグレーション、ADR-0051/ADR-0052)手動で対で消す
        self.conn.execute(
            "DELETE FROM memo_group_perms WHERE memo_id = ?1",
            params![id],
        )?;
        self.conn
            .execute("DELETE FROM memo_comments WHERE memo_id = ?1", params![id])?;
        self.conn
            .execute("DELETE FROM memos WHERE id = ?1", params![id])?;
        self.conn.execute(
            "INSERT INTO deleted_memos (memo_id, deleted_at) VALUES (?1, ?2)
             ON CONFLICT(memo_id) DO UPDATE SET deleted_at = excluded.deleted_at",
            params![id, now],
        )?;
        Ok(())
    }

    /// WAL の未反映分をメイン DB ファイルへ書き戻す(バックアップ前など)。
    pub fn checkpoint(&self) -> anyhow::Result<()> {
        self.conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        Ok(())
    }

    /// 診断用の統計情報。
    pub fn db_stats(&self) -> anyhow::Result<SharedMemoDbStats> {
        let db_bytes = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        let wal_bytes = std::fs::metadata(wal_path_for(&self.path))
            .map(|m| m.len())
            .unwrap_or(0);
        let memo_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM memos WHERE deleted_at IS NULL",
            [],
            |row| row.get(0),
        )?;
        let trashed_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM memos WHERE deleted_at IS NOT NULL",
            [],
            |row| row.get(0),
        )?;
        let history_count: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM memo_history", [], |row| row.get(0))?;
        let schedule_count: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM schedule_events", [], |row| row.get(0))?;
        let sheet_count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM sheets", [], |row| row.get(0))?;
        Ok(SharedMemoDbStats {
            db_bytes,
            wal_bytes,
            memo_count: memo_count as u64,
            trashed_count: trashed_count as u64,
            history_count: history_count as u64,
            total_body_bytes: self.total_body_bytes()?,
            limits: self.limits()?,
            schedule_count: schedule_count as u64,
            sheet_count: sheet_count as u64,
        })
    }

    /// ゴミ箱へ(所有者・ホスト管理者)。
    pub fn trash(&mut self, actor: &Actor, id: &str) -> anyhow::Result<()> {
        let row = self.row(id)?;
        if !row.can_manage(actor) {
            bail!("このメモを削除できるのは所有者とホスト管理者だけです");
        }
        self.conn.execute(
            "UPDATE memos SET deleted_at = ?1 WHERE id = ?2 AND deleted_at IS NULL",
            params![unix_ms(), id],
        )?;
        Ok(())
    }

    pub fn restore(&mut self, actor: &Actor, id: &str) -> anyhow::Result<()> {
        let row = self.row(id)?;
        if !row.can_manage(actor) {
            bail!("このメモを復元できるのは所有者とホスト管理者だけです");
        }
        self.conn.execute(
            "UPDATE memos SET deleted_at = NULL WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }

    pub fn delete_forever(&mut self, actor: &Actor, id: &str) -> anyhow::Result<()> {
        let row = self.row(id)?;
        if !row.can_manage(actor) {
            bail!("このメモを完全削除できるのは所有者とホスト管理者だけです");
        }
        if row.deleted_at.is_none() {
            bail!("完全削除はゴミ箱のメモに対してのみ行えます");
        }
        self.purge_memo(id, unix_ms())?;
        Ok(())
    }

    /// 権限の設定(所有者・ホスト管理者)。`members` は全量置き換え。
    /// `groups` は `None` = グループ権限を変更しない(旧クライアントの
    /// SetPerms が既存のグループ権限を消さないための互換仕様、ADR-0051)。
    /// `Some` は全量置き換え。
    pub fn set_perms(
        &mut self,
        actor: &Actor,
        id: &str,
        everyone: SharedPermLevel,
        members: &[SharedMemberPerm],
        groups: Option<&[SharedGroupPerm]>,
    ) -> anyhow::Result<SharedMemoDetail> {
        let row = self.row(id)?;
        if !row.can_manage(actor) {
            bail!("権限を変更できるのは所有者とホスト管理者だけです");
        }
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE memos SET everyone = ?1 WHERE id = ?2",
            params![level_to_str(everyone), id],
        )?;
        tx.execute("DELETE FROM memo_perms WHERE memo_id = ?1", params![id])?;
        for member in members {
            if member.member_id.is_empty() {
                continue;
            }
            tx.execute(
                "INSERT OR REPLACE INTO memo_perms (memo_id, member_id, name, level)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    id,
                    member.member_id,
                    member.name,
                    level_to_str(member.level)
                ],
            )?;
        }
        if let Some(groups) = groups {
            tx.execute(
                "DELETE FROM memo_group_perms WHERE memo_id = ?1",
                params![id],
            )?;
            for group in groups {
                if group.group_id.is_empty() {
                    continue;
                }
                tx.execute(
                    "INSERT OR REPLACE INTO memo_group_perms (memo_id, group_id, name, level)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![id, group.group_id, group.name, level_to_str(group.level)],
                )?;
            }
        }
        tx.commit()?;
        self.get(actor, id)
    }

    /// コメント一覧(古い順)。閲覧権限があれば可(ADR-0052 決定 4)。
    pub fn comment_list(&self, actor: &Actor, id: &str) -> anyhow::Result<Vec<SharedMemoComment>> {
        let row = self.row(id)?;
        let ctx = self.perm_context(id)?;
        if !row.visible_to(actor, &ctx) {
            bail!("このメモを閲覧する権限がありません");
        }
        let mut stmt = self.conn.prepare(
            "SELECT comment_id, memo_id, author_id, author_name, body, created_at
             FROM memo_comments WHERE memo_id = ?1 ORDER BY created_at ASC, comment_id ASC",
        )?;
        let comments = stmt
            .query_map(params![id], comment_from_sql)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(comments)
    }

    /// コメント追加。閲覧権限があれば可(本文上限・件数上限あり、ADR-0052 決定 4)。
    pub fn comment_add(
        &mut self,
        actor: &Actor,
        id: &str,
        body: &str,
    ) -> anyhow::Result<SharedMemoComment> {
        let row = self.row(id)?;
        let ctx = self.perm_context(id)?;
        if !row.visible_to(actor, &ctx) {
            bail!("このメモを閲覧する権限がありません");
        }
        let body = body.trim();
        if body.is_empty() {
            bail!("コメントの本文が空です");
        }
        if body.len() > MAX_COMMENT_BODY_BYTES {
            bail!(
                "コメントの本文が長すぎます(上限 {} KiB)",
                MAX_COMMENT_BODY_BYTES / 1024
            );
        }
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM memo_comments WHERE memo_id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        if count as u32 >= MAX_COMMENTS_PER_MEMO {
            bail!(
                "このメモのコメントが上限({} 件)に達しています。不要なコメントを削除してください",
                MAX_COMMENTS_PER_MEMO
            );
        }
        let comment_id: String =
            self.conn
                .query_row("SELECT lower(hex(randomblob(8)))", [], |row| row.get(0))?;
        let now = unix_ms();
        self.conn.execute(
            "INSERT INTO memo_comments (comment_id, memo_id, author_id, author_name, body, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![comment_id, id, actor.owner_id(), actor.name, body, now],
        )?;
        Ok(SharedMemoComment {
            comment_id,
            memo_id: id.to_string(),
            author_id: actor.owner_id().to_string(),
            author_name: actor.name.clone(),
            body: body.to_string(),
            created_at_unix_ms: now as u64,
        })
    }

    /// コメント削除(本人・メモ所有者・ホスト管理者、ADR-0052 決定 4)。
    pub fn comment_delete(
        &mut self,
        actor: &Actor,
        id: &str,
        comment_id: &str,
    ) -> anyhow::Result<()> {
        let row = self.row(id)?;
        let ctx = self.perm_context(id)?;
        if !row.visible_to(actor, &ctx) {
            bail!("このメモを閲覧する権限がありません");
        }
        let author_id: String = self
            .conn
            .query_row(
                "SELECT author_id FROM memo_comments WHERE comment_id = ?1 AND memo_id = ?2",
                params![comment_id, id],
                |row| row.get(0),
            )
            .optional()?
            .context("コメントが見つかりません")?;
        let is_author = author_id == actor.owner_id();
        if !is_author && !row.can_manage(actor) {
            bail!("このコメントを削除できるのは本人・メモ所有者・ホスト管理者だけです");
        }
        self.conn.execute(
            "DELETE FROM memo_comments WHERE comment_id = ?1 AND memo_id = ?2",
            params![comment_id, id],
        )?;
        Ok(())
    }

    pub fn folder_create(&mut self, actor: &Actor, name: &str) -> anyhow::Result<MemoFolder> {
        self.require_host(actor)?;
        let name = validate_folder_name(name)?;
        let exists: bool = self
            .conn
            .query_row(
                "SELECT 1 FROM folders WHERE name = ?1",
                params![name],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if exists {
            bail!("同じ名前のフォルダーが既にあります");
        }
        let id: String = self
            .conn
            .query_row("SELECT lower(hex(randomblob(8)))", [], |row| row.get(0))?;
        self.conn.execute(
            "INSERT INTO folders (id, name, created_at) VALUES (?1, ?2, ?3)",
            params![id, name, unix_ms()],
        )?;
        Ok(MemoFolder {
            id,
            name,
            memo_count: 0,
        })
    }

    pub fn folder_rename(&mut self, actor: &Actor, id: &str, name: &str) -> anyhow::Result<()> {
        self.require_host(actor)?;
        let name = validate_folder_name(name)?;
        let changed = self.conn.execute(
            "UPDATE folders SET name = ?1 WHERE id = ?2",
            params![name, id],
        )?;
        if changed == 0 {
            bail!("フォルダーが見つかりません");
        }
        Ok(())
    }

    pub fn folder_delete(&mut self, actor: &Actor, id: &str) -> anyhow::Result<()> {
        self.require_host(actor)?;
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE memos SET folder_id = NULL WHERE folder_id = ?1",
            params![id],
        )?;
        let changed = tx.execute("DELETE FROM folders WHERE id = ?1", params![id])?;
        if changed == 0 {
            bail!("フォルダーが見つかりません");
        }
        tx.commit()?;
        Ok(())
    }

    fn require_host(&self, actor: &Actor) -> anyhow::Result<()> {
        if !actor.is_host() {
            bail!("共有フォルダーを管理できるのはホスト管理者だけです");
        }
        Ok(())
    }

    pub fn folders(&self) -> anyhow::Result<Vec<MemoFolder>> {
        let mut stmt = self.conn.prepare(
            "SELECT f.id, f.name, COUNT(m.id)
             FROM folders f
             LEFT JOIN memos m ON m.folder_id = f.id AND m.deleted_at IS NULL
             GROUP BY f.id ORDER BY f.name COLLATE NOCASE",
        )?;
        let folders = stmt
            .query_map([], |row| {
                Ok(MemoFolder {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    memo_count: row.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(folders)
    }

    // ---- 共有スケジュール表(M6 G-1、ADR-0053) ----
    //
    // 権限はメモよりずっと単純: ネットワーク全員が閲覧・追加でき、編集・
    // 削除は作成者 + ホストだけ(決定 3)。ゴミ箱は持たず物理削除、編集
    // ロックも持たず revision CAS のみで競合を検知する(決定 4)。

    fn schedule_row(&self, id: &str) -> anyhow::Result<ScheduleRow> {
        self.conn
            .query_row(
                &format!("SELECT {SCHEDULE_ROW_COLUMNS} FROM schedule_events WHERE id = ?1"),
                params![id],
                schedule_row_from_sql,
            )
            .optional()?
            .context("予定が見つかりません(削除された可能性があります)")
    }

    /// 全件一覧(start_at 昇順)。V1 は 10,000 件上限のため全量で良い
    /// (ADR-0053 決定 1)。
    pub fn schedule_list(&self, actor: &Actor) -> anyhow::Result<Vec<ScheduleEvent>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {SCHEDULE_ROW_COLUMNS} FROM schedule_events ORDER BY start_at ASC, id ASC"
        ))?;
        let rows = stmt
            .query_map([], schedule_row_from_sql)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows.into_iter().map(|row| row.into_event(actor)).collect())
    }

    pub fn schedule_get(&self, actor: &Actor, id: &str) -> anyhow::Result<ScheduleEvent> {
        Ok(self.schedule_row(id)?.into_event(actor))
    }

    /// 新規作成(全員可)。
    #[allow(clippy::too_many_arguments)]
    pub fn schedule_create(
        &mut self,
        actor: &Actor,
        title: &str,
        note: &str,
        start_unix_ms: u64,
        end_unix_ms: Option<u64>,
        all_day: bool,
        participants: &[ScheduleParticipant],
    ) -> anyhow::Result<ScheduleEvent> {
        validate_schedule_title(title)?;
        validate_schedule_note(note)?;
        validate_schedule_range(start_unix_ms, end_unix_ms)?;
        validate_schedule_participants(participants)?;
        let count: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM schedule_events", [], |row| row.get(0))?;
        if count as u32 >= MAX_SCHEDULE_EVENTS {
            bail!(
                "予定の件数が上限({} 件)に達しています。不要な予定を削除してください",
                MAX_SCHEDULE_EVENTS
            );
        }
        let id: String = self
            .conn
            .query_row("SELECT lower(hex(randomblob(8)))", [], |row| row.get(0))?;
        let now = unix_ms();
        let participants_json =
            serde_json::to_string(participants).context("participants の直列化に失敗しました")?;
        self.conn.execute(
            "INSERT INTO schedule_events
                (id, title, note, start_at, end_at, all_day, owner_id, owner_name,
                 updated_by, participants, revision, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9, 1, ?10, ?10)",
            params![
                id,
                title,
                note,
                start_unix_ms as i64,
                end_unix_ms.map(|v| v as i64),
                all_day as i64,
                actor.owner_id(),
                actor.name,
                participants_json,
                now,
            ],
        )?;
        self.schedule_get(actor, &id)
    }

    /// 更新(CAS)。作成者 + ホストのみ(ADR-0053 決定 3・4)。
    #[allow(clippy::too_many_arguments)]
    pub fn schedule_update(
        &mut self,
        actor: &Actor,
        id: &str,
        base_revision: u64,
        title: &str,
        note: &str,
        start_unix_ms: u64,
        end_unix_ms: Option<u64>,
        all_day: bool,
        participants: &[ScheduleParticipant],
    ) -> anyhow::Result<ScheduleEvent> {
        validate_schedule_title(title)?;
        validate_schedule_note(note)?;
        validate_schedule_range(start_unix_ms, end_unix_ms)?;
        validate_schedule_participants(participants)?;
        let row = self.schedule_row(id)?;
        if !(actor.is_host() || row.owner_id == actor.owner_id()) {
            bail!("この予定を編集できるのは作成者とホスト管理者だけです");
        }
        if row.revision as u64 != base_revision {
            bail!("competing_edit: 他の端末の変更が先に保存されています(最新を読み込み直してください)");
        }
        let participants_json =
            serde_json::to_string(participants).context("participants の直列化に失敗しました")?;
        self.conn.execute(
            "UPDATE schedule_events SET title = ?1, note = ?2, start_at = ?3, end_at = ?4,
                    all_day = ?5, updated_by = ?6, participants = ?7,
                    revision = revision + 1, updated_at = ?8
             WHERE id = ?9",
            params![
                title,
                note,
                start_unix_ms as i64,
                end_unix_ms.map(|v| v as i64),
                all_day as i64,
                actor.name,
                participants_json,
                unix_ms(),
                id,
            ],
        )?;
        self.schedule_get(actor, id)
    }

    /// 削除(物理削除、ゴミ箱なし = V1)。作成者 + ホストのみ。
    pub fn schedule_delete(&mut self, actor: &Actor, id: &str) -> anyhow::Result<()> {
        let row = self.schedule_row(id)?;
        if !(actor.is_host() || row.owner_id == actor.owner_id()) {
            bail!("この予定を削除できるのは作成者とホスト管理者だけです");
        }
        self.conn
            .execute("DELETE FROM schedule_events WHERE id = ?1", params![id])?;
        Ok(())
    }

    // ---- 共有シート(M6 G-2、ADR-0054) ----
    //
    // シート(メタ)の作成は全員可。改名・削除は作成者 + ホストのみ。
    // セルの読み取り・書き込みは全員可(決定 5)。アドレスは固定(行・列の
    // 挿入/削除による繰り上げは持たない、決定 3)。競合はセル単位の
    // revision CAS のみ(決定 4)。

    fn sheet_row(&self, id: &str) -> anyhow::Result<SheetRow> {
        self.conn
            .query_row(
                &format!("SELECT {SHEET_ROW_COLUMNS} FROM sheets WHERE id = ?1"),
                params![id],
                sheet_row_from_sql,
            )
            .optional()?
            .context("シートが見つかりません(削除された可能性があります)")
    }

    /// シート一覧(メタのみ、V1。件数上限 [`MAX_SHEETS`] のため全量で良い)。
    pub fn sheet_list(&self, actor: &Actor) -> anyhow::Result<Vec<SheetMeta>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {SHEET_ROW_COLUMNS} FROM sheets ORDER BY created_at ASC, id ASC"
        ))?;
        let rows = stmt
            .query_map([], sheet_row_from_sql)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows.into_iter().map(|row| row.into_meta(actor)).collect())
    }

    pub fn sheet_get(&self, actor: &Actor, id: &str) -> anyhow::Result<SheetMeta> {
        Ok(self.sheet_row(id)?.into_meta(actor))
    }

    /// 1 シートの全非空セル(行・列昇順)。
    pub fn sheet_cells(&self, id: &str) -> anyhow::Result<Vec<SheetCell>> {
        // シートの存在確認(削除済みなら理由を返す)
        self.sheet_row(id)?;
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {SHEET_CELL_COLUMNS} FROM sheet_cells
             WHERE sheet_id = ?1 ORDER BY row ASC, col ASC"
        ))?;
        let cells = stmt
            .query_map(params![id], sheet_cell_from_sql)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(cells)
    }

    /// 新規作成(全員可、決定 5)。
    pub fn sheet_create(&mut self, actor: &Actor, name: &str) -> anyhow::Result<SheetMeta> {
        validate_sheet_name(name)?;
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM sheets", [], |row| row.get(0))?;
        if count as u32 >= MAX_SHEETS {
            bail!(
                "シートの枚数が上限({} 枚)に達しています。不要なシートを削除してください",
                MAX_SHEETS
            );
        }
        let id: String = self
            .conn
            .query_row("SELECT lower(hex(randomblob(8)))", [], |row| row.get(0))?;
        let now = unix_ms();
        self.conn.execute(
            "INSERT INTO sheets (id, name, owner_id, owner_name, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![id, name, actor.owner_id(), actor.name, now],
        )?;
        self.sheet_get(actor, &id)
    }

    /// 改名(作成者 + ホストのみ、決定 5)。
    pub fn sheet_rename(
        &mut self,
        actor: &Actor,
        id: &str,
        name: &str,
    ) -> anyhow::Result<SheetMeta> {
        validate_sheet_name(name)?;
        let row = self.sheet_row(id)?;
        if !(actor.is_host() || row.owner_id == actor.owner_id()) {
            bail!("このシートを改名できるのは作成者とホスト管理者だけです");
        }
        self.conn.execute(
            "UPDATE sheets SET name = ?1, updated_at = ?2 WHERE id = ?3",
            params![name, unix_ms(), id],
        )?;
        self.sheet_get(actor, id)
    }

    /// 削除(作成者 + ホストのみ、決定 5)。セルもまとめて削除する。
    pub fn sheet_delete(&mut self, actor: &Actor, id: &str) -> anyhow::Result<()> {
        let row = self.sheet_row(id)?;
        if !(actor.is_host() || row.owner_id == actor.owner_id()) {
            bail!("このシートを削除できるのは作成者とホスト管理者だけです");
        }
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM sheet_cells WHERE sheet_id = ?1", params![id])?;
        tx.execute("DELETE FROM sheets WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    /// セルのバッチ書き込み(全員可、決定 5)。競合セルは部分失敗として
    /// `conflicts` へ現在値を返す(決定 4)。戻り値は
    /// `(適用件数, 競合セル, 適用済みセルの最新形(配信用))`。
    pub fn sheet_write(
        &mut self,
        actor: &Actor,
        id: &str,
        cells: &[CellWrite],
    ) -> anyhow::Result<(u32, Vec<SheetCell>, Vec<SheetCell>)> {
        self.sheet_row(id)?; // シートの存在確認
        if cells.is_empty() {
            return Ok((0, Vec::new(), Vec::new()));
        }
        let tx = self.conn.transaction()?;
        let mut current_count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM sheet_cells WHERE sheet_id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        let now = unix_ms();
        let mut applied = 0u32;
        let mut conflicts = Vec::new();
        let mut changed = Vec::new();
        for write in cells {
            if write.row >= MAX_SHEET_ROWS || write.col >= MAX_SHEET_COLS {
                bail!(
                    "セルの位置が上限を超えています(行は {} 未満、列は {} 未満にしてください)",
                    MAX_SHEET_ROWS,
                    MAX_SHEET_COLS
                );
            }
            if write.value.len() > MAX_CELL_VALUE_BYTES {
                bail!(
                    "セルの値が長すぎます(上限 {} KiB)",
                    MAX_CELL_VALUE_BYTES / 1024
                );
            }
            let existing: Option<(i64, String, String, i64)> = tx
                .query_row(
                    "SELECT revision, value, updated_by, updated_at FROM sheet_cells
                     WHERE sheet_id = ?1 AND row = ?2 AND col = ?3",
                    params![id, write.row, write.col],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()?;
            match existing {
                Some((revision, value, updated_by, updated_at)) => {
                    if revision as u64 != write.base_revision {
                        conflicts.push(SheetCell {
                            row: write.row,
                            col: write.col,
                            value,
                            revision: revision as u64,
                            updated_by,
                            updated_at: updated_at as u64,
                        });
                        continue;
                    }
                    let new_revision = revision as u64 + 1;
                    if write.value.is_empty() {
                        tx.execute(
                            "DELETE FROM sheet_cells WHERE sheet_id = ?1 AND row = ?2 AND col = ?3",
                            params![id, write.row, write.col],
                        )?;
                        current_count -= 1;
                    } else {
                        tx.execute(
                            "UPDATE sheet_cells SET value = ?1, revision = ?2, updated_by = ?3,
                                    updated_at = ?4
                             WHERE sheet_id = ?5 AND row = ?6 AND col = ?7",
                            params![
                                write.value,
                                new_revision as i64,
                                actor.name,
                                now,
                                id,
                                write.row,
                                write.col
                            ],
                        )?;
                    }
                    changed.push(SheetCell {
                        row: write.row,
                        col: write.col,
                        value: write.value.clone(),
                        revision: new_revision,
                        updated_by: actor.name.clone(),
                        updated_at: now as u64,
                    });
                    applied += 1;
                }
                None => {
                    if write.base_revision != 0 {
                        // 存在しないセルに対する非 0 の base_revision =
                        // 他の端末が既に削除済み(競合として現在値=不在を返す)
                        conflicts.push(SheetCell {
                            row: write.row,
                            col: write.col,
                            value: String::new(),
                            revision: 0,
                            updated_by: String::new(),
                            updated_at: 0,
                        });
                        continue;
                    }
                    if write.value.is_empty() {
                        // 存在しないセルへの削除要求は無害な no-op
                        applied += 1;
                        continue;
                    }
                    if current_count as u32 >= MAX_SHEET_CELLS {
                        bail!(
                            "セルの件数が上限({} 件)に達しています。不要なセルを削除してください",
                            MAX_SHEET_CELLS
                        );
                    }
                    tx.execute(
                        "INSERT INTO sheet_cells
                            (sheet_id, row, col, value, revision, updated_by, updated_at)
                         VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6)",
                        params![id, write.row, write.col, write.value, actor.name, now],
                    )?;
                    current_count += 1;
                    changed.push(SheetCell {
                        row: write.row,
                        col: write.col,
                        value: write.value.clone(),
                        revision: 1,
                        updated_by: actor.name.clone(),
                        updated_at: now as u64,
                    });
                    applied += 1;
                }
            }
        }
        if !changed.is_empty() {
            tx.execute(
                "UPDATE sheets SET updated_at = ?1 WHERE id = ?2",
                params![now, id],
            )?;
        }
        tx.commit()?;
        Ok((applied, conflicts, changed))
    }
}

/// スケジュール 1 件分の生データ(受信者視点の計算前)。
struct ScheduleRow {
    id: String,
    title: String,
    note: String,
    start_at: i64,
    end_at: Option<i64>,
    all_day: bool,
    owner_id: String,
    owner_name: String,
    updated_by: String,
    /// JSON 文字列(`Vec<ScheduleParticipant>`)。壊れていれば空扱い
    /// (表示上の問題にとどめ、一覧取得自体は失敗させない)。
    participants: String,
    revision: i64,
    created_at: i64,
    updated_at: i64,
}

impl ScheduleRow {
    /// can_edit は作成者本人かホストなら真(ADR-0053 決定 3)。
    fn into_event(self, actor: &Actor) -> ScheduleEvent {
        let can_edit = actor.is_host() || self.owner_id == actor.owner_id();
        let participants: Vec<ScheduleParticipant> =
            serde_json::from_str(&self.participants).unwrap_or_default();
        ScheduleEvent {
            id: self.id,
            title: self.title,
            note: self.note,
            start_unix_ms: self.start_at as u64,
            end_unix_ms: self.end_at.map(|v| v as u64),
            all_day: self.all_day,
            owner_id: self.owner_id,
            owner_name: self.owner_name,
            updated_by: self.updated_by,
            participants,
            revision: self.revision as u64,
            created_at: self.created_at as u64,
            updated_at: self.updated_at as u64,
            can_edit,
        }
    }
}

const SCHEDULE_ROW_COLUMNS: &str = "id, title, note, start_at, end_at, all_day, owner_id,
        owner_name, updated_by, participants, revision, created_at, updated_at";

fn schedule_row_from_sql(row: &rusqlite::Row<'_>) -> rusqlite::Result<ScheduleRow> {
    Ok(ScheduleRow {
        id: row.get(0)?,
        title: row.get(1)?,
        note: row.get(2)?,
        start_at: row.get(3)?,
        end_at: row.get(4)?,
        all_day: row.get::<_, i64>(5)? != 0,
        owner_id: row.get(6)?,
        owner_name: row.get(7)?,
        updated_by: row.get(8)?,
        participants: row.get(9)?,
        revision: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

/// 参加メンバーの検証(件数上限のみ、ADR-0055 決定 5。member_id は検査しない)。
fn validate_schedule_participants(participants: &[ScheduleParticipant]) -> anyhow::Result<()> {
    if participants.len() > MAX_SCHEDULE_PARTICIPANTS {
        bail!(
            "参加メンバーが多すぎます(上限 {} 名)",
            MAX_SCHEDULE_PARTICIPANTS
        );
    }
    Ok(())
}

/// タイトルの検証(必須 + 上限文字数、ADR-0053 決定 6)。
fn validate_schedule_title(title: &str) -> anyhow::Result<()> {
    if title.trim().is_empty() {
        bail!("タイトルを入力してください");
    }
    if title.chars().count() > MAX_SCHEDULE_TITLE_CHARS {
        bail!("タイトルが長すぎます(上限 {MAX_SCHEDULE_TITLE_CHARS} 文字)");
    }
    Ok(())
}

/// 詳細(note)の検証(上限バイト数、ADR-0053 決定 6)。
fn validate_schedule_note(note: &str) -> anyhow::Result<()> {
    if note.len() > MAX_SCHEDULE_NOTE_BYTES {
        bail!(
            "詳細が長すぎます(上限 {} KiB)",
            MAX_SCHEDULE_NOTE_BYTES / 1024
        );
    }
    Ok(())
}

/// 終了日時は開始日時以降であること。
fn validate_schedule_range(start_unix_ms: u64, end_unix_ms: Option<u64>) -> anyhow::Result<()> {
    if let Some(end) = end_unix_ms {
        if end < start_unix_ms {
            bail!("終了日時は開始日時より後にしてください");
        }
    }
    Ok(())
}

/// シート 1 枚分の生データ(受信者視点の計算前)。
struct SheetRow {
    id: String,
    name: String,
    owner_id: String,
    owner_name: String,
    created_at: i64,
    updated_at: i64,
}

impl SheetRow {
    /// can_manage は作成者本人かホストなら真(ADR-0054 決定 5)。
    fn into_meta(self, actor: &Actor) -> SheetMeta {
        let can_manage = actor.is_host() || self.owner_id == actor.owner_id();
        SheetMeta {
            id: self.id,
            name: self.name,
            owner_id: self.owner_id,
            owner_name: self.owner_name,
            created_at: self.created_at as u64,
            updated_at: self.updated_at as u64,
            can_manage,
        }
    }
}

const SHEET_ROW_COLUMNS: &str = "id, name, owner_id, owner_name, created_at, updated_at";

fn sheet_row_from_sql(row: &rusqlite::Row<'_>) -> rusqlite::Result<SheetRow> {
    Ok(SheetRow {
        id: row.get(0)?,
        name: row.get(1)?,
        owner_id: row.get(2)?,
        owner_name: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

/// [`SheetCell`] の共通 SELECT 列(ホスト正本・キャッシュ両方で使う)。
const SHEET_CELL_COLUMNS: &str = "row, col, value, revision, updated_by, updated_at";

fn sheet_cell_from_sql(row: &rusqlite::Row<'_>) -> rusqlite::Result<SheetCell> {
    Ok(SheetCell {
        row: row.get::<_, i64>(0)? as u32,
        col: row.get::<_, i64>(1)? as u32,
        value: row.get(2)?,
        revision: row.get::<_, i64>(3)? as u64,
        updated_by: row.get(4)?,
        updated_at: row.get::<_, i64>(5)? as u64,
    })
}

/// シート名の検証(必須 + 上限文字数、ADR-0054 決定 7)。
fn validate_sheet_name(name: &str) -> anyhow::Result<()> {
    if name.trim().is_empty() {
        bail!("シート名を入力してください");
    }
    if name.chars().count() > MAX_SHEET_NAME_CHARS {
        bail!("シート名が長すぎます(上限 {MAX_SHEET_NAME_CHARS} 文字)");
    }
    Ok(())
}

/// メモ行の共通 SELECT 列(この順で `row_from_sql` が読む)。呼び出し側は
/// `FROM memos ...`(または `FROM memos m ...`)に続けて使う。comment_count は
/// 相関サブクエリで数える(索引テーブルの二重管理を避ける — 500 件上限なので
/// 十分速い)。
const ROW_COLUMNS: &str = "id, title, body, folder_id, revision, owner_id, owner_name,
        created_at, updated_at, updated_by, everyone, deleted_at,
        (SELECT COUNT(*) FROM memo_comments c WHERE c.memo_id = memos.id) AS comment_count";

/// [`ROW_COLUMNS`] の別名(`m`)版。`FROM memos m` で使う。
const ROW_COLUMNS_ALIASED: &str = "id, title, body, folder_id, revision, owner_id, owner_name,
        created_at, updated_at, updated_by, everyone, deleted_at,
        (SELECT COUNT(*) FROM memo_comments c WHERE c.memo_id = m.id) AS comment_count";

fn row_from_sql(row: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    Ok(Row {
        id: row.get(0)?,
        title: row.get(1)?,
        body: row.get(2)?,
        folder_id: row.get(3)?,
        revision: row.get(4)?,
        owner_id: row.get(5)?,
        owner_name: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
        updated_by: row.get(9)?,
        everyone: level_from_str(&row.get::<_, String>(10)?),
        deleted_at: row.get(11)?,
        comment_count: row.get::<_, i64>(12)? as u32,
    })
}

fn comment_from_sql(row: &rusqlite::Row<'_>) -> rusqlite::Result<SharedMemoComment> {
    Ok(SharedMemoComment {
        comment_id: row.get(0)?,
        memo_id: row.get(1)?,
        author_id: row.get(2)?,
        author_name: row.get(3)?,
        body: row.get(4)?,
        created_at_unix_ms: row.get::<_, i64>(5)? as u64,
    })
}

fn open_db(path: &Path) -> anyhow::Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("フォルダーを作成できません: {}", parent.display()))?;
    }
    let conn = Connection::open(path)
        .with_context(|| format!("メモデータベースを開けません: {}", path.display()))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    register_kana_fold(&conn)?;
    Ok(conn)
}

/// メイン DB ファイルパスから WAL ファイルのパスを組み立てる。
fn wal_path_for(path: &Path) -> PathBuf {
    let mut os = path.as_os_str().to_os_string();
    os.push("-wal");
    PathBuf::from(os)
}

/// DB の一貫スナップショットをメモリ上のバイト列として取得する(平文の
/// 一時ファイルを作らないバックアップ用。WAL の未チェックポイント分も含む)。
pub fn snapshot_db_bytes(path: &Path) -> anyhow::Result<Vec<u8>> {
    let src = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("メモデータベースを開けません: {}", path.display()))?;
    let mut dst = Connection::open_in_memory().context("一時 DB を作成できません")?;
    {
        let backup = rusqlite::backup::Backup::new(&src, &mut dst)?;
        backup.run_to_completion(100, std::time::Duration::from_millis(0), None)?;
    }
    let data = dst.serialize(rusqlite::MAIN_DB)?;
    Ok(data.to_vec())
}

/// 検索条件(FTS trigram / 3 文字未満は LIKE)を組み立てる。かな折り畳み済み。
fn push_search_clause(
    clauses: &mut Vec<String>,
    args: &mut Vec<Box<dyn rusqlite::ToSql>>,
    search: &str,
) {
    let search = kana_fold(search);
    if search.chars().count() >= 3 {
        args.push(Box::new(format!("\"{}\"", search.replace('"', "\"\""))));
        clauses.push(format!(
            "m.rowid IN (SELECT rowid FROM memo_fts WHERE memo_fts MATCH ?{})",
            args.len()
        ));
    } else {
        let pattern = format!("%{}%", escape_like(&search));
        args.push(Box::new(pattern));
        let n = args.len();
        clauses.push(format!(
            "(kana_fold(m.title) LIKE ?{n} ESCAPE '\\' OR kana_fold(m.body) LIKE ?{n} ESCAPE '\\')"
        ));
    }
}

/// メンバー側の読み取りキャッシュ。ホストからの配信内容をそのまま保持する
/// (権限計算は済んでいる)。オフライン時はここから読み取り専用で表示する。
pub struct CacheStore {
    conn: Connection,
}

const CACHE_SCHEMA_VERSION: i64 = 5;

impl CacheStore {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let conn = open_db(path)?;
        let mut store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&mut self) -> anyhow::Result<()> {
        let version: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version >= CACHE_SCHEMA_VERSION {
            return Ok(());
        }
        let tx = self.conn.transaction()?;
        if version < 1 {
            tx.execute_batch(
                r#"
                CREATE TABLE memos (
                    id TEXT PRIMARY KEY,
                    title TEXT NOT NULL DEFAULT '',
                    body TEXT NOT NULL DEFAULT '',
                    folder_id TEXT,
                    revision INTEGER NOT NULL,
                    owner_id TEXT NOT NULL DEFAULT '',
                    owner_name TEXT NOT NULL DEFAULT '',
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    updated_by TEXT,
                    can_edit INTEGER NOT NULL DEFAULT 0,
                    can_manage INTEGER NOT NULL DEFAULT 0,
                    locked_by TEXT
                );
                CREATE TABLE folders (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    sort INTEGER NOT NULL DEFAULT 0
                );
                CREATE VIRTUAL TABLE memo_fts USING fts5(title, body, tokenize='trigram');
                CREATE TRIGGER memos_fts_insert AFTER INSERT ON memos BEGIN
                    INSERT INTO memo_fts(rowid, title, body)
                        VALUES (new.rowid, kana_fold(new.title), kana_fold(new.body));
                END;
                CREATE TRIGGER memos_fts_delete AFTER DELETE ON memos BEGIN
                    DELETE FROM memo_fts WHERE rowid = old.rowid;
                END;
                CREATE TRIGGER memos_fts_update AFTER UPDATE OF title, body ON memos BEGIN
                    DELETE FROM memo_fts WHERE rowid = old.rowid;
                    INSERT INTO memo_fts(rowid, title, body)
                        VALUES (new.rowid, kana_fold(new.title), kana_fold(new.body));
                END;
                "#,
            )?;
        }
        if version < 2 {
            // v2(M5 F-5、ADR-0052 決定 4): コメント件数(一覧の 💬 バッジ用)。
            // コメント本文自体はキャッシュに保存しない(一覧は都度取得)。
            tx.execute_batch(
                "ALTER TABLE memos ADD COLUMN comment_count INTEGER NOT NULL DEFAULT 0;",
            )?;
        }
        if version < 3 {
            // v3(M6 G-1、ADR-0053): 共有スケジュール表のキャッシュ
            // (can_edit はホストが計算済みの値をそのまま保存する)。
            tx.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS schedule_events (
                    id TEXT PRIMARY KEY,
                    title TEXT NOT NULL DEFAULT '',
                    note TEXT NOT NULL DEFAULT '',
                    start_at INTEGER NOT NULL,
                    end_at INTEGER,
                    all_day INTEGER NOT NULL DEFAULT 0,
                    owner_id TEXT NOT NULL DEFAULT '',
                    owner_name TEXT NOT NULL DEFAULT '',
                    updated_by TEXT NOT NULL DEFAULT '',
                    revision INTEGER NOT NULL DEFAULT 1,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    can_edit INTEGER NOT NULL DEFAULT 0
                );
                CREATE INDEX IF NOT EXISTS idx_schedule_events_start ON schedule_events(start_at);
                "#,
            )?;
        }
        if version < 4 {
            // v4(M6 G-2、ADR-0054): 共有シートのキャッシュ(can_manage は
            // ホストが計算済みの値をそのまま保存する)。
            tx.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS sheets (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL DEFAULT '',
                    owner_id TEXT NOT NULL DEFAULT '',
                    owner_name TEXT NOT NULL DEFAULT '',
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    can_manage INTEGER NOT NULL DEFAULT 0
                );
                CREATE TABLE IF NOT EXISTS sheet_cells (
                    sheet_id TEXT NOT NULL,
                    row INTEGER NOT NULL,
                    col INTEGER NOT NULL,
                    value TEXT NOT NULL DEFAULT '',
                    revision INTEGER NOT NULL DEFAULT 1,
                    updated_by TEXT NOT NULL DEFAULT '',
                    updated_at INTEGER NOT NULL,
                    PRIMARY KEY (sheet_id, row, col)
                );
                CREATE INDEX IF NOT EXISTS idx_sheet_cells_sheet ON sheet_cells(sheet_id);
                "#,
            )?;
        }
        if version < 5 {
            // v5(M6 H-3、ADR-0055 決定 5): 予定の参加メンバーのキャッシュ
            // (ホスト正本と同じく JSON 文字列で格納)。存在確認してから
            // ALTER する理由は SharedStore 側と同じ(非冪等対策)。
            if !table_has_column(&tx, "schedule_events", "participants")? {
                tx.execute_batch(
                    "ALTER TABLE schedule_events ADD COLUMN participants TEXT NOT NULL DEFAULT '[]';",
                )?;
            }
        }
        tx.pragma_update(None, "user_version", CACHE_SCHEMA_VERSION)?;
        tx.commit()?;
        Ok(())
    }

    /// ホストからの詳細(Changed イベント / Get 応答)を反映する。
    pub fn upsert(&mut self, memo: &SharedMemoDetail) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO memos (id, title, body, folder_id, revision, owner_id, owner_name,
                                created_at, updated_at, updated_by, can_edit, can_manage,
                                locked_by, comment_count)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(id) DO UPDATE SET
                title = ?2, body = ?3, folder_id = ?4, revision = ?5, owner_id = ?6,
                owner_name = ?7, created_at = ?8, updated_at = ?9, updated_by = ?10,
                can_edit = ?11, can_manage = ?12, locked_by = ?13, comment_count = ?14",
            params![
                memo.id,
                memo.title,
                memo.body,
                memo.folder_id,
                memo.revision as i64,
                memo.owner_id,
                memo.owner_name,
                memo.created_at as i64,
                memo.updated_at as i64,
                memo.updated_by,
                memo.can_edit as i64,
                memo.can_manage as i64,
                memo.locked_by,
                memo.comment_count as i64,
            ],
        )?;
        self.enforce_limit()?;
        Ok(())
    }

    /// 本文合計サイズが `max_bytes` を超える間、`updated_at` が古いメモ
    /// から削除する(最新 1 件は必ず残す)。戻り値は削除件数。
    fn enforce_limit_with(&self, max_bytes: u64) -> anyhow::Result<u64> {
        let mut deleted = 0u64;
        loop {
            let total: i64 = self.conn.query_row(
                "SELECT COALESCE(SUM(length(CAST(body AS BLOB))), 0) FROM memos",
                [],
                |row| row.get(0),
            )?;
            if total as u64 <= max_bytes {
                break;
            }
            let count: i64 = self
                .conn
                .query_row("SELECT COUNT(*) FROM memos", [], |row| row.get(0))?;
            if count <= 1 {
                break;
            }
            let oldest_id: Option<String> = self
                .conn
                .query_row(
                    "SELECT id FROM memos ORDER BY updated_at ASC LIMIT 1",
                    [],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(oldest_id) = oldest_id else {
                break;
            };
            self.conn
                .execute("DELETE FROM memos WHERE id = ?1", params![oldest_id])?;
            deleted += 1;
        }
        Ok(deleted)
    }

    /// [`CACHE_MAX_BYTES`] を上限としてキャッシュを刈り込む。
    pub fn enforce_limit(&self) -> anyhow::Result<u64> {
        self.enforce_limit_with(CACHE_MAX_BYTES)
    }

    pub fn remove(&mut self, id: &str) -> anyhow::Result<()> {
        self.conn
            .execute("DELETE FROM memos WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// 同期(List 応答)で見えているメモ ID の集合外を落とす。
    pub fn retain(&mut self, ids: &[String]) -> anyhow::Result<()> {
        let tx = self.conn.transaction()?;
        let existing: Vec<String> = {
            let mut stmt = tx.prepare("SELECT id FROM memos")?;
            let rows = stmt.query_map([], |row| row.get(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        for id in existing {
            if !ids.contains(&id) {
                tx.execute("DELETE FROM memos WHERE id = ?1", params![id])?;
            }
        }
        tx.commit()?;
        self.enforce_limit()?;
        Ok(())
    }

    pub fn set_lock(&mut self, id: &str, holder: Option<&str>) -> anyhow::Result<()> {
        self.conn.execute(
            "UPDATE memos SET locked_by = ?1 WHERE id = ?2",
            params![holder, id],
        )?;
        Ok(())
    }

    pub fn replace_folders(&mut self, folders: &[MemoFolder]) -> anyhow::Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM folders", [])?;
        for (index, folder) in folders.iter().enumerate() {
            tx.execute(
                "INSERT INTO folders (id, name, sort) VALUES (?1, ?2, ?3)",
                params![folder.id, folder.name, index as i64],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn revision(&self, id: &str) -> anyhow::Result<Option<u64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT revision FROM memos WHERE id = ?1",
                params![id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .map(|v| v as u64))
    }

    pub fn list(
        &self,
        query: &SharedMemoQuery,
    ) -> anyhow::Result<(Vec<SharedMemoSummary>, Vec<MemoFolder>)> {
        let mut sql = String::from(
            "SELECT id, title, body, folder_id, revision, owner_id, owner_name,
                    created_at, updated_at, updated_by, can_edit, can_manage, locked_by,
                    comment_count
             FROM memos m WHERE 1 = 1",
        );
        let mut clauses: Vec<String> = Vec::new();
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(folder) = &query.folder_id {
            args.push(Box::new(folder.clone()));
            clauses.push(format!("m.folder_id = ?{}", args.len()));
        }
        if let Some(search) = query
            .search
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            push_search_clause(&mut clauses, &mut args, search);
        }
        for clause in &clauses {
            sql.push_str(" AND ");
            sql.push_str(clause);
        }
        sql.push_str(" ORDER BY m.updated_at DESC");
        let mut stmt = self.conn.prepare(&sql)?;
        let params_ref: Vec<&dyn rusqlite::ToSql> = args.iter().map(AsRef::as_ref).collect();
        let memos = stmt
            .query_map(params_ref.as_slice(), |row| {
                Ok(cache_summary(
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get::<_, i64>(10)? != 0,
                    row.get::<_, i64>(11)? != 0,
                    row.get(12)?,
                    row.get::<_, i64>(13)? as u32,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        let mut stmt = self
            .conn
            .prepare("SELECT id, name FROM folders ORDER BY sort")?;
        let folders = stmt
            .query_map([], |row| {
                Ok(MemoFolder {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    memo_count: 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok((memos, folders))
    }

    pub fn get(&self, id: &str) -> anyhow::Result<SharedMemoDetail> {
        self.conn
            .query_row(
                "SELECT id, title, body, folder_id, revision, owner_id, owner_name,
                        created_at, updated_at, updated_by, can_edit, can_manage, locked_by,
                        comment_count
                 FROM memos WHERE id = ?1",
                params![id],
                |row| {
                    Ok(SharedMemoDetail {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        body: row.get(2)?,
                        folder_id: row.get(3)?,
                        revision: row.get::<_, i64>(4)? as u64,
                        owner_id: row.get(5)?,
                        owner_name: row.get(6)?,
                        created_at: row.get::<_, i64>(7)? as u64,
                        updated_at: row.get::<_, i64>(8)? as u64,
                        updated_by: row.get(9)?,
                        can_edit: row.get::<_, i64>(10)? != 0,
                        can_manage: row.get::<_, i64>(11)? != 0,
                        locked_by: row.get(12)?,
                        deleted_at: None,
                        everyone: None,
                        members: Vec::new(),
                        groups: Vec::new(),
                        comment_count: row.get::<_, i64>(13)? as u32,
                    })
                },
            )
            .optional()?
            .context("共有メモが見つかりません(削除された可能性があります)")
    }

    /// メモ間リンク `[[タイトル]]` の解決(キャッシュ版。オフラインでも使える。
    /// キャッシュには actor が見えるメモしか同期されないため可視性フィルタは
    /// 不要)。タイトル → memo_id(見つかったものだけ。複数一致は updated_at
    /// 最新)。
    pub fn resolve_titles(&self, titles: &[String]) -> anyhow::Result<HashMap<String, String>> {
        let mut seen = std::collections::HashSet::new();
        let mut out = HashMap::new();
        for title in titles {
            if title.is_empty() || !seen.insert(title.clone()) {
                continue;
            }
            let id: Option<String> = self
                .conn
                .query_row(
                    "SELECT id FROM memos WHERE title = ?1 ORDER BY updated_at DESC LIMIT 1",
                    params![title],
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(id) = id {
                out.insert(title.clone(), id);
            }
        }
        Ok(out)
    }

    /// バックリンク(キャッシュ版)。本文に `[[<このメモのタイトル>]]` を
    /// リテラルとして含むメモの一覧(自分自身は除く)。
    pub fn backlinks(&self, id: &str) -> anyhow::Result<Vec<SharedMemoSummary>> {
        let target = self.get(id)?;
        if target.title.is_empty() {
            return Ok(Vec::new());
        }
        let pattern = format!("%[[{}]]%", escape_like(&target.title));
        let mut stmt = self.conn.prepare(
            "SELECT id, title, body, folder_id, revision, owner_id, owner_name,
                    created_at, updated_at, updated_by, can_edit, can_manage, locked_by,
                    comment_count
             FROM memos WHERE id != ?1 AND body LIKE ?2 ESCAPE '\\'
             ORDER BY updated_at DESC",
        )?;
        let memos = stmt
            .query_map(params![id, pattern], |row| {
                Ok(cache_summary(
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get::<_, i64>(10)? != 0,
                    row.get::<_, i64>(11)? != 0,
                    row.get(12)?,
                    row.get::<_, i64>(13)? as u32,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(memos)
    }

    // ---- 共有スケジュール表のキャッシュ(M6 G-1、ADR-0053) ----

    /// 接続時の同期(List 応答)で全量を置き換える。
    pub fn schedule_replace_all(&mut self, events: &[ScheduleEvent]) -> anyhow::Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM schedule_events", [])?;
        for event in events {
            insert_cache_schedule_event(&tx, event)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// 単発のイベント反映(Changed)。
    pub fn schedule_upsert(&mut self, event: &ScheduleEvent) -> anyhow::Result<()> {
        insert_cache_schedule_event(&self.conn, event)
    }

    /// 単発のイベント反映(Removed)。
    pub fn schedule_remove(&mut self, id: &str) -> anyhow::Result<()> {
        self.conn
            .execute("DELETE FROM schedule_events WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// オフライン時の唯一のソース(start_at 昇順)。
    pub fn schedule_list(&self) -> anyhow::Result<Vec<ScheduleEvent>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {CACHE_SCHEDULE_COLUMNS} FROM schedule_events ORDER BY start_at ASC, id ASC"
        ))?;
        let events = stmt
            .query_map([], cache_schedule_from_sql)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(events)
    }

    pub fn schedule_get(&self, id: &str) -> anyhow::Result<ScheduleEvent> {
        self.conn
            .query_row(
                &format!("SELECT {CACHE_SCHEDULE_COLUMNS} FROM schedule_events WHERE id = ?1"),
                params![id],
                cache_schedule_from_sql,
            )
            .optional()?
            .context("予定が見つかりません(削除された可能性があります)")
    }

    // ---- 共有シートのキャッシュ(M6 G-2、ADR-0054) ----

    /// 接続時の同期(List 応答)でメタの全量を置き換える。List に含まれない
    /// (= 削除された)シートのセルも一緒に消す。
    pub fn sheet_replace_all(&mut self, sheets: &[SheetMeta]) -> anyhow::Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM sheets", [])?;
        for sheet in sheets {
            insert_cache_sheet(&tx, sheet)?;
        }
        tx.execute(
            "DELETE FROM sheet_cells WHERE sheet_id NOT IN (SELECT id FROM sheets)",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// 単発のメタ反映(SheetChanged = 作成・改名)。
    pub fn sheet_upsert(&mut self, sheet: &SheetMeta) -> anyhow::Result<()> {
        insert_cache_sheet(&self.conn, sheet)
    }

    /// 単発のメタ反映(SheetRemoved = 削除)。セルもまとめて消す。
    pub fn sheet_remove(&mut self, id: &str) -> anyhow::Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM sheet_cells WHERE sheet_id = ?1", params![id])?;
        tx.execute("DELETE FROM sheets WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    /// オフライン時の唯一のソース(作成順)。
    pub fn sheet_list(&self) -> anyhow::Result<Vec<SheetMeta>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {CACHE_SHEET_COLUMNS} FROM sheets ORDER BY created_at ASC, id ASC"
        ))?;
        let sheets = stmt
            .query_map([], cache_sheet_from_sql)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(sheets)
    }

    /// 接続時の同期(Cells 応答)でそのシートのセル全量を置き換える。
    pub fn sheet_cells_replace_all(
        &mut self,
        sheet_id: &str,
        cells: &[SheetCell],
    ) -> anyhow::Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM sheet_cells WHERE sheet_id = ?1",
            params![sheet_id],
        )?;
        for cell in cells {
            insert_cache_cell(&tx, sheet_id, cell)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// イベント反映(CellsChanged)。value = "" のセルは削除。
    pub fn sheet_cells_apply(&mut self, sheet_id: &str, cells: &[SheetCell]) -> anyhow::Result<()> {
        let tx = self.conn.transaction()?;
        for cell in cells {
            if cell.value.is_empty() {
                tx.execute(
                    "DELETE FROM sheet_cells WHERE sheet_id = ?1 AND row = ?2 AND col = ?3",
                    params![sheet_id, cell.row, cell.col],
                )?;
            } else {
                insert_cache_cell(&tx, sheet_id, cell)?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// 1 シートの全非空セル(行・列昇順)。
    pub fn sheet_cells(&self, sheet_id: &str) -> anyhow::Result<Vec<SheetCell>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {SHEET_CELL_COLUMNS} FROM sheet_cells
             WHERE sheet_id = ?1 ORDER BY row ASC, col ASC"
        ))?;
        let cells = stmt
            .query_map(params![sheet_id], sheet_cell_from_sql)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(cells)
    }
}

const CACHE_SCHEDULE_COLUMNS: &str = "id, title, note, start_at, end_at, all_day, owner_id,
        owner_name, updated_by, revision, created_at, updated_at, can_edit, participants";

fn insert_cache_schedule_event(conn: &Connection, event: &ScheduleEvent) -> anyhow::Result<()> {
    let participants_json = serde_json::to_string(&event.participants)
        .context("participants の直列化に失敗しました")?;
    conn.execute(
        "INSERT INTO schedule_events
            (id, title, note, start_at, end_at, all_day, owner_id, owner_name,
             updated_by, revision, created_at, updated_at, can_edit, participants)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
         ON CONFLICT(id) DO UPDATE SET
            title = ?2, note = ?3, start_at = ?4, end_at = ?5, all_day = ?6, owner_id = ?7,
            owner_name = ?8, updated_by = ?9, revision = ?10, created_at = ?11,
            updated_at = ?12, can_edit = ?13, participants = ?14",
        params![
            event.id,
            event.title,
            event.note,
            event.start_unix_ms as i64,
            event.end_unix_ms.map(|v| v as i64),
            event.all_day as i64,
            event.owner_id,
            event.owner_name,
            event.updated_by,
            event.revision as i64,
            event.created_at as i64,
            event.updated_at as i64,
            event.can_edit as i64,
            participants_json,
        ],
    )?;
    Ok(())
}

fn cache_schedule_from_sql(row: &rusqlite::Row<'_>) -> rusqlite::Result<ScheduleEvent> {
    let participants_json: String = row.get(13)?;
    let participants: Vec<ScheduleParticipant> =
        serde_json::from_str(&participants_json).unwrap_or_default();
    Ok(ScheduleEvent {
        id: row.get(0)?,
        title: row.get(1)?,
        note: row.get(2)?,
        start_unix_ms: row.get::<_, i64>(3)? as u64,
        end_unix_ms: row.get::<_, Option<i64>>(4)?.map(|v| v as u64),
        all_day: row.get::<_, i64>(5)? != 0,
        owner_id: row.get(6)?,
        owner_name: row.get(7)?,
        updated_by: row.get(8)?,
        revision: row.get::<_, i64>(9)? as u64,
        created_at: row.get::<_, i64>(10)? as u64,
        updated_at: row.get::<_, i64>(11)? as u64,
        can_edit: row.get::<_, i64>(12)? != 0,
        participants,
    })
}

const CACHE_SHEET_COLUMNS: &str =
    "id, name, owner_id, owner_name, created_at, updated_at, can_manage";

fn insert_cache_sheet(conn: &Connection, sheet: &SheetMeta) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO sheets (id, name, owner_id, owner_name, created_at, updated_at, can_manage)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(id) DO UPDATE SET
            name = ?2, owner_id = ?3, owner_name = ?4, created_at = ?5, updated_at = ?6,
            can_manage = ?7",
        params![
            sheet.id,
            sheet.name,
            sheet.owner_id,
            sheet.owner_name,
            sheet.created_at as i64,
            sheet.updated_at as i64,
            sheet.can_manage as i64,
        ],
    )?;
    Ok(())
}

fn cache_sheet_from_sql(row: &rusqlite::Row<'_>) -> rusqlite::Result<SheetMeta> {
    Ok(SheetMeta {
        id: row.get(0)?,
        name: row.get(1)?,
        owner_id: row.get(2)?,
        owner_name: row.get(3)?,
        created_at: row.get::<_, i64>(4)? as u64,
        updated_at: row.get::<_, i64>(5)? as u64,
        can_manage: row.get::<_, i64>(6)? != 0,
    })
}

fn insert_cache_cell(conn: &Connection, sheet_id: &str, cell: &SheetCell) -> anyhow::Result<()> {
    conn.execute(
        "INSERT INTO sheet_cells (sheet_id, row, col, value, revision, updated_by, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(sheet_id, row, col) DO UPDATE SET
            value = ?4, revision = ?5, updated_by = ?6, updated_at = ?7",
        params![
            sheet_id,
            cell.row,
            cell.col,
            cell.value,
            cell.revision as i64,
            cell.updated_by,
            cell.updated_at as i64,
        ],
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn cache_summary(
    id: String,
    title: String,
    body: String,
    folder_id: Option<String>,
    revision: i64,
    owner_id: String,
    owner_name: String,
    created_at: i64,
    updated_at: i64,
    updated_by: Option<String>,
    can_edit: bool,
    can_manage: bool,
    locked_by: Option<String>,
    comment_count: u32,
) -> SharedMemoSummary {
    let (done, total) = checklist_progress(&body);
    SharedMemoSummary {
        id,
        title,
        excerpt: excerpt(&body, EXCERPT_CHARS),
        folder_id,
        revision: revision as u64,
        created_at: created_at as u64,
        updated_at: updated_at as u64,
        updated_by,
        owner_id,
        owner_name,
        deleted_at: None,
        can_edit,
        can_manage,
        locked_by,
        checklist_done: done,
        checklist_total: total,
        comment_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_temp() -> (tempfile::TempDir, SharedStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = SharedStore::open(&dir.path().join("host.memos.db")).unwrap();
        (dir, store)
    }

    #[test]
    fn permissions_owner_everyone_and_override() {
        let (_dir, mut store) = open_temp();
        let alice = Actor::member("id-alice", "アリス");
        let bob = Actor::member("id-bob", "ボブ");
        let host = Actor::host("ホスト");

        let memo = store.create(&alice, "共有", "本文", None).unwrap();
        assert!(memo.can_edit && memo.can_manage, "所有者は編集・管理できる");

        // 既定(everyone = viewer): ボブは見えるが編集不可
        let seen = store.get(&bob, &memo.id).unwrap();
        assert!(!seen.can_edit && !seen.can_manage);
        assert!(
            seen.everyone.is_none(),
            "管理権限が無ければ権限情報は載らない"
        );

        // ボブだけ編集者へ
        store
            .set_perms(
                &alice,
                &memo.id,
                SharedPermLevel::Viewer,
                &[SharedMemberPerm {
                    member_id: "id-bob".to_string(),
                    name: "ボブ".to_string(),
                    level: SharedPermLevel::Editor,
                }],
                None,
            )
            .unwrap();
        assert!(store.get(&bob, &memo.id).unwrap().can_edit);

        // 全体 none + 個別なし = 第三者には見えない
        store
            .set_perms(&alice, &memo.id, SharedPermLevel::None, &[], None)
            .unwrap();
        assert!(store.get(&bob, &memo.id).is_err());
        assert!(store.detail_if_visible(&bob, &memo.id).unwrap().is_none());
        // ホストは常に見える
        assert!(store.get(&host, &memo.id).is_ok());

        // ボブの一覧には載らない
        let (memos, _) = store.list(&bob, &SharedMemoQuery::default()).unwrap();
        assert!(memos.is_empty());
        // 権限変更はボブにはできない
        assert!(store
            .set_perms(&bob, &memo.id, SharedPermLevel::Editor, &[], None)
            .is_err());
    }

    /// グループ権限(ADR-0051): 優先順位「メンバー個別 > グループ(該当する
    /// 複数グループの最大)> 全体」と、複数グループ該当時の最大採用を確認する。
    #[test]
    fn group_permissions_resolve_priority_and_max() {
        let (_dir, mut store) = open_temp();
        let alice = Actor::member("id-alice", "アリス");
        // キャロルは g1(viewer)だけ、デイブは g1(viewer)+g2(editor)、
        // イブは g3(none)だけに所属
        let carol = Actor::member("id-carol", "キャロル").with_groups(vec!["g1".to_string()]);
        let dave = Actor::member("id-dave", "デイブ")
            .with_groups(vec!["g1".to_string(), "g2".to_string()]);
        let eve = Actor::member("id-eve", "イブ").with_groups(vec!["g3".to_string()]);

        let memo = store.create(&alice, "共有", "本文", None).unwrap();
        // 全体を none にし、グループ権限だけで可視性を決める
        store
            .set_perms(
                &alice,
                &memo.id,
                SharedPermLevel::None,
                &[],
                Some(&[
                    SharedGroupPerm {
                        group_id: "g1".to_string(),
                        name: "チームA".to_string(),
                        level: SharedPermLevel::Viewer,
                    },
                    SharedGroupPerm {
                        group_id: "g2".to_string(),
                        name: "チームB".to_string(),
                        level: SharedPermLevel::Editor,
                    },
                    SharedGroupPerm {
                        group_id: "g3".to_string(),
                        name: "チームC".to_string(),
                        level: SharedPermLevel::None,
                    },
                ]),
            )
            .unwrap();

        // g1(viewer)だけの所属: 見えるが編集不可
        let seen = store.get(&carol, &memo.id).unwrap();
        assert!(!seen.can_edit, "viewer のグループでは編集できない");

        // g1(viewer) + g2(editor)所属: 複数該当は最大(editor)を採る
        assert!(
            store.get(&dave, &memo.id).unwrap().can_edit,
            "複数グループ該当時は最も高い権限になる"
        );

        // g3(none)だけの所属: 見えない
        assert!(store.get(&eve, &memo.id).is_err());

        // 個別指定はグループより優先: キャロルへ個別 none を追加する
        // (groups は None なので直前のグループ権限は維持される)
        store
            .set_perms(
                &alice,
                &memo.id,
                SharedPermLevel::None,
                &[SharedMemberPerm {
                    member_id: "id-carol".to_string(),
                    name: "キャロル".to_string(),
                    level: SharedPermLevel::None,
                }],
                None,
            )
            .unwrap();
        assert!(
            store.get(&carol, &memo.id).is_err(),
            "個別 none がグループ viewer に勝つ"
        );
        // デイブ(グループ editor のまま)には影響しない
        assert!(store.get(&dave, &memo.id).unwrap().can_edit);
    }

    /// グループの明示的な none は、全体(everyone)が viewer のままでも
    /// 優先される(判定は「個別 > グループ > 全体」の段階評価)。
    #[test]
    fn group_none_overrides_everyone_viewer() {
        let (_dir, mut store) = open_temp();
        let host = Actor::host("ホスト");
        let bob = Actor::member("id-bob", "ボブ").with_groups(vec!["g1".to_string()]);
        let memo = store.create(&host, "t", "v", None).unwrap();
        // everyone は既定の viewer のまま。ボブの所属グループだけ none にする
        store
            .set_perms(
                &host,
                &memo.id,
                SharedPermLevel::Viewer,
                &[],
                Some(&[SharedGroupPerm {
                    group_id: "g1".to_string(),
                    name: "チームA".to_string(),
                    level: SharedPermLevel::None,
                }]),
            )
            .unwrap();
        assert!(
            store.get(&bob, &memo.id).is_err(),
            "所属グループの明示的 none は全体 viewer より優先される"
        );
    }

    /// SetPerms の groups: None は既存のグループ権限を変更しない
    /// (旧クライアント互換、ADR-0051)。
    #[test]
    fn set_perms_groups_none_keeps_existing() {
        let (_dir, mut store) = open_temp();
        let host = Actor::host("ホスト");
        let carol = Actor::member("id-carol", "キャロル").with_groups(vec!["g1".to_string()]);
        let memo = store.create(&host, "t", "v", None).unwrap();
        store
            .set_perms(
                &host,
                &memo.id,
                SharedPermLevel::None,
                &[],
                Some(&[SharedGroupPerm {
                    group_id: "g1".to_string(),
                    name: "チームA".to_string(),
                    level: SharedPermLevel::Viewer,
                }]),
            )
            .unwrap();
        assert!(store.get(&carol, &memo.id).is_ok());
        // 旧クライアント相当の SetPerms(groups なし)を送っても既存は残る
        store
            .set_perms(&host, &memo.id, SharedPermLevel::None, &[], None)
            .unwrap();
        assert!(
            store.get(&carol, &memo.id).is_ok(),
            "groups: None は既存のグループ権限を変更しない"
        );
        // can_manage の受信者(host)には detail.groups が引き続き載る
        let detail = store.get(&host, &memo.id).unwrap();
        assert_eq!(detail.groups.len(), 1);
        assert_eq!(detail.groups[0].group_id, "g1");
    }

    #[test]
    fn update_cas_rejects_stale_revision() {
        let (_dir, mut store) = open_temp();
        let host = Actor::host("ホスト");
        let memo = store.create(&host, "t", "v1", None).unwrap();
        assert_eq!(memo.revision, 1);
        let updated = store.update(&host, &memo.id, 1, "t", "v2").unwrap();
        assert_eq!(updated.revision, 2);
        let err = store.update(&host, &memo.id, 1, "t", "v3").unwrap_err();
        assert!(err.to_string().contains("先に保存"), "{err}");
    }

    #[test]
    fn viewer_cannot_edit_or_trash() {
        let (_dir, mut store) = open_temp();
        let host = Actor::host("ホスト");
        let bob = Actor::member("id-bob", "ボブ");
        let memo = store.create(&host, "t", "v", None).unwrap();
        assert!(store.update(&bob, &memo.id, 1, "t", "x").is_err());
        assert!(store.trash(&bob, &memo.id).is_err());

        // everyone = editor なら編集はできるが削除・権限変更は不可
        store
            .set_perms(&host, &memo.id, SharedPermLevel::Editor, &[], None)
            .unwrap();
        assert!(store.update(&bob, &memo.id, 1, "t", "x").is_ok());
        assert!(store.trash(&bob, &memo.id).is_err());
    }

    #[test]
    fn trash_flow_and_visibility() {
        let (_dir, mut store) = open_temp();
        let alice = Actor::member("id-alice", "アリス");
        let bob = Actor::member("id-bob", "ボブ");
        let memo = store.create(&alice, "捨てる", "x", None).unwrap();
        store.trash(&alice, &memo.id).unwrap();
        // 通常一覧から消え、配信対象からも外れる
        let (memos, _) = store.list(&bob, &SharedMemoQuery::default()).unwrap();
        assert!(memos.is_empty());
        assert!(store.detail_if_visible(&bob, &memo.id).unwrap().is_none());
        // 所有者はゴミ箱で見える
        let (trash, _) = store
            .list(
                &alice,
                &SharedMemoQuery {
                    trash: true,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(trash.len(), 1);
        store.restore(&alice, &memo.id).unwrap();
        assert!(store.get(&bob, &memo.id).is_ok());
        // 完全削除はゴミ箱からのみ
        assert!(store.delete_forever(&alice, &memo.id).is_err());
        store.trash(&alice, &memo.id).unwrap();
        store.delete_forever(&alice, &memo.id).unwrap();
    }

    #[test]
    fn folders_are_host_managed() {
        let (_dir, mut store) = open_temp();
        let host = Actor::host("ホスト");
        let alice = Actor::member("id-alice", "アリス");
        assert!(store.folder_create(&alice, "共有").is_err());
        let folder = store.folder_create(&host, "共有").unwrap();
        let memo = store.create(&alice, "中身", "x", Some(&folder.id)).unwrap();
        assert_eq!(memo.folder_id.as_deref(), Some(folder.id.as_str()));
        store.folder_delete(&host, &folder.id).unwrap();
        assert_eq!(store.get(&alice, &memo.id).unwrap().folder_id, None);
    }

    #[test]
    fn limits_validate_and_permission() {
        let (_dir, store) = open_temp();
        let bob = Actor::member("id-bob", "ボブ");
        let host = Actor::host("ホスト");
        // メンバーは変更不可
        assert!(store
            .set_limits(&bob, &SharedMemoLimits::default())
            .is_err());
        // 範囲外は拒否(本文上限は 256KiB まで)
        let bad = SharedMemoLimits {
            max_body_bytes: 300 * 1024,
            ..Default::default()
        };
        assert!(store.set_limits(&host, &bad).is_err());
        // 正常値は反映される
        let ok = SharedMemoLimits {
            trash_days: 7,
            ..Default::default()
        };
        store.set_limits(&host, &ok).unwrap();
        assert_eq!(store.limits().unwrap().trash_days, 7);
    }

    #[test]
    fn limits_enforce_count_body_and_total_bytes() {
        let (_dir, mut store) = open_temp();
        let host = Actor::host("ホスト");

        // 件数上限
        let count_limit = SharedMemoLimits {
            max_memo_count: 1,
            ..Default::default()
        };
        store.set_limits(&host, &count_limit).unwrap();
        store.create(&host, "1件目", "本文", None).unwrap();
        let err = store.create(&host, "2件目", "本文", None).unwrap_err();
        assert!(err.to_string().contains("件数が上限"), "{err}");

        // 本文サイズ上限
        let body_limit = SharedMemoLimits {
            max_memo_count: 100,
            max_body_bytes: 1024,
            ..Default::default()
        };
        store.set_limits(&host, &body_limit).unwrap();
        let big = "a".repeat(2000);
        let err = store.create(&host, "大きい", &big, None).unwrap_err();
        assert!(err.to_string().contains("本文が大きすぎます"), "{err}");

        // 全体容量上限(最小値 1MiB)。既存の「1件目」(6 バイト)を土台に、
        // 256KiB のメモを積み上げて超過させる
        let total_limit = SharedMemoLimits {
            max_memo_count: 100,
            max_body_bytes: 256 * 1024,
            max_total_bytes: 1024 * 1024,
            ..Default::default()
        };
        store.set_limits(&host, &total_limit).unwrap();
        let chunk = "a".repeat(256 * 1024);
        for _ in 0..3 {
            store.create(&host, "chunk", &chunk, None).unwrap();
        }
        let err = store.create(&host, "chunk", &chunk, None).unwrap_err();
        assert!(err.to_string().contains("全体容量"), "{err}");
    }

    #[test]
    fn history_auto_snapshot_is_rate_limited() {
        let (_dir, mut store) = open_temp();
        let host = Actor::host("ホスト");
        let memo = store.create(&host, "t", "v1", None).unwrap();
        store.update(&host, &memo.id, 1, "t", "v2").unwrap();
        // 直後の 2 回目の更新では auto 版は増えない(間隔内)
        store.update(&host, &memo.id, 2, "t", "v3").unwrap();
        let history = store.history_list(&host, &memo.id).unwrap();
        assert_eq!(history.len(), 1, "間隔内の連続更新では auto は 1 件だけ");
        assert_eq!(history[0].kind, "auto");

        // 履歴を 11 分前に見せかけてから更新すると、また 1 件増える
        store
            .conn
            .execute(
                "UPDATE memo_history SET created_at = ?1 WHERE memo_id = ?2",
                params![unix_ms() - 11 * 60 * 1000, memo.id],
            )
            .unwrap();
        store.update(&host, &memo.id, 3, "t", "v4").unwrap();
        assert_eq!(store.history_list(&host, &memo.id).unwrap().len(), 2);
    }

    #[test]
    fn snapshot_if_revision_changed_only_on_change() {
        let (_dir, mut store) = open_temp();
        let host = Actor::host("ホスト");
        let memo = store.create(&host, "t", "v1", None).unwrap();
        store
            .snapshot_if_revision_changed(&memo.id, memo.revision)
            .unwrap();
        assert!(
            store.history_list(&host, &memo.id).unwrap().is_empty(),
            "変更なしなら記録しない"
        );

        store
            .update(&host, &memo.id, memo.revision, "t", "v2")
            .unwrap();
        let before = store.history_list(&host, &memo.id).unwrap().len();
        // ロック取得時点の revision(古い 1)のまま呼ぶ = 現在(2)と異なる
        store
            .snapshot_if_revision_changed(&memo.id, memo.revision)
            .unwrap();
        let after = store.history_list(&host, &memo.id).unwrap();
        assert_eq!(after.len(), before + 1);
        assert_eq!(after[0].kind, "close");
    }

    #[test]
    fn save_version_dedups_and_requires_edit() {
        let (_dir, mut store) = open_temp();
        let host = Actor::host("ホスト");
        let bob = Actor::member("id-bob", "ボブ");
        let memo = store.create(&host, "t", "v1", None).unwrap();
        assert!(
            store.save_version(&bob, &memo.id).is_err(),
            "閲覧者は保存できない"
        );
        store.save_version(&host, &memo.id).unwrap();
        assert_eq!(store.history_list(&host, &memo.id).unwrap().len(), 1);
        // 同じ revision で連続保存しても増えない
        store.save_version(&host, &memo.id).unwrap();
        assert_eq!(store.history_list(&host, &memo.id).unwrap().len(), 1);
    }

    #[test]
    fn history_trims_by_max_versions() {
        let (_dir, mut store) = open_temp();
        let host = Actor::host("ホスト");
        let limits = SharedMemoLimits {
            max_versions: 3,
            ..Default::default()
        };
        store.set_limits(&host, &limits).unwrap();
        let memo = store.create(&host, "t", "v0", None).unwrap();
        let mut revision = memo.revision;
        for i in 0..6 {
            let updated = store
                .update(&host, &memo.id, revision, "t", &format!("v{i}"))
                .unwrap();
            revision = updated.revision;
            store.save_version(&host, &memo.id).unwrap();
        }
        let history = store.history_list(&host, &memo.id).unwrap();
        assert_eq!(history.len(), 3, "max_versions=3 を超えた分は刈り込まれる");
    }

    #[test]
    fn history_trims_by_history_days() {
        let (_dir, mut store) = open_temp();
        let host = Actor::host("ホスト");
        let memo = store.create(&host, "t", "v0", None).unwrap();
        store.save_version(&host, &memo.id).unwrap();
        assert_eq!(store.history_list(&host, &memo.id).unwrap().len(), 1);
        // 31 日前の履歴に見せかける(既定 history_days = 30)
        store
            .conn
            .execute(
                "UPDATE memo_history SET created_at = ?1",
                params![unix_ms() - 31 * 24 * 60 * 60 * 1000],
            )
            .unwrap();
        // 次の自動保存(間隔超過なので発火)が古い履歴を刈り込む
        store
            .update(&host, &memo.id, memo.revision, "t", "v1")
            .unwrap();
        let history = store.history_list(&host, &memo.id).unwrap();
        assert_eq!(history.len(), 1, "history_days を超えた履歴は刈り込まれる");
        assert_eq!(history[0].kind, "auto");
    }

    #[test]
    fn history_restore_restores_content_and_preserves_previous() {
        let (_dir, mut store) = open_temp();
        let host = Actor::host("ホスト");
        let bob = Actor::member("id-bob", "ボブ"); // 既定 everyone = viewer
        let memo = store.create(&host, "元題", "v1本文", None).unwrap();
        let after_v2 = store
            .update(&host, &memo.id, memo.revision, "元題", "v2本文")
            .unwrap();
        let history = store.history_list(&host, &memo.id).unwrap();
        assert_eq!(history.len(), 1);
        let target_hid = history[0].hid; // 更新前(v1本文)のスナップショット

        // 閲覧者は復元できない
        assert!(store.history_restore(&bob, &memo.id, target_hid).is_err());

        let restored = store.history_restore(&host, &memo.id, target_hid).unwrap();
        assert_eq!(restored.body, "v1本文");
        assert_eq!(restored.revision, after_v2.revision + 1, "revision が進む");

        // 復元前(v2本文)の内容が "restore" 種別として履歴に残る
        let history = store.history_list(&host, &memo.id).unwrap();
        let restore_entry = history.iter().find(|e| e.kind == "restore").unwrap();
        let detail = store
            .history_get(&host, &memo.id, restore_entry.hid)
            .unwrap();
        assert_eq!(detail.body, "v2本文");
    }

    #[test]
    fn history_diff_reports_changed_lines() {
        let (_dir, mut store) = open_temp();
        let host = Actor::host("ホスト");
        let memo = store.create(&host, "t", "a\nb\nc", None).unwrap();
        store
            .update(&host, &memo.id, memo.revision, "t", "a\nb2\nc")
            .unwrap();
        let history = store.history_list(&host, &memo.id).unwrap();
        let from_hid = history[0].hid; // "a\nb\nc" のスナップショット
        let lines = store.history_diff(&host, &memo.id, from_hid, None).unwrap();
        assert!(lines
            .iter()
            .any(|l| l.kind == peercove_core::memo::DiffLineKind::Removed && l.text == "b"));
        assert!(lines
            .iter()
            .any(|l| l.kind == peercove_core::memo::DiffLineKind::Added && l.text == "b2"));
    }

    #[test]
    fn purge_expired_removes_after_trash_days_and_tombstone_after_90_days() {
        let (_dir, mut store) = open_temp();
        let host = Actor::host("ホスト");
        let memo = store.create(&host, "捨てる", "x", None).unwrap();
        store.trash(&host, &memo.id).unwrap();
        // まだ期限内は残る
        assert_eq!(store.purge_expired().unwrap(), 0);

        // trash_days(既定 30 日)を過ぎたことにする
        store
            .conn
            .execute(
                "UPDATE memos SET deleted_at = ?1 WHERE id = ?2",
                params![unix_ms() - 31 * 24 * 60 * 60 * 1000, memo.id],
            )
            .unwrap();
        let purged = store.purge_expired().unwrap();
        assert_eq!(purged, 1);
        assert!(store.get(&host, &memo.id).is_err());
        // 削除済み ID 台帳に載る
        let tomb_count: i64 = store
            .conn
            .query_row(
                "SELECT COUNT(*) FROM deleted_memos WHERE memo_id = ?1",
                params![memo.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tomb_count, 1);

        // 台帳の保持期限(90 日)を過ぎると台帳からも消える
        store
            .conn
            .execute(
                "UPDATE deleted_memos SET deleted_at = ?1 WHERE memo_id = ?2",
                params![unix_ms() - 91 * 24 * 60 * 60 * 1000, memo.id],
            )
            .unwrap();
        store.purge_expired().unwrap();
        let tomb_count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM deleted_memos", [], |row| row.get(0))
            .unwrap();
        assert_eq!(tomb_count, 0);
    }

    #[test]
    fn snapshot_db_bytes_produces_valid_sqlite_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("host.memos.db");
        {
            let mut store = SharedStore::open(&path).unwrap();
            let host = Actor::host("ホスト");
            store.create(&host, "タイトル", "本文", None).unwrap();
        }
        let bytes = snapshot_db_bytes(&path).unwrap();
        assert!(!bytes.is_empty());
        let snap_path = dir.path().join("snapshot.db");
        std::fs::write(&snap_path, &bytes).unwrap();
        let reopened = SharedStore::open(&snap_path).unwrap();
        let host = Actor::host("ホスト");
        let (memos, _) = reopened.list(&host, &SharedMemoQuery::default()).unwrap();
        assert_eq!(memos.len(), 1);
        assert_eq!(memos[0].title, "タイトル");
    }

    #[test]
    fn v1_database_migrates_to_v2_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("host.memos.db");
        let memo_id = {
            let mut store = SharedStore::open(&path).unwrap();
            let host = Actor::host("ホスト");
            let memo = store.create(&host, "移行前", "本文", None).unwrap();
            // v1 相当に偽装
            store.conn.pragma_update(None, "user_version", 1).unwrap();
            memo.id
        };
        let store = SharedStore::open(&path).unwrap();
        // v2 のテーブル(settings)が使え、既存メモも読める
        assert!(store.limits().is_ok());
        let host = Actor::host("ホスト");
        assert_eq!(store.get(&host, &memo_id).unwrap().title, "移行前");
    }

    /// v2 → v3(M5 F-4、ADR-0051): memo_group_perms テーブルが追加され、
    /// 既存メモも読め、グループ権限を新規に設定できる。
    #[test]
    fn v2_database_migrates_to_v3_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("host.memos.db");
        let memo_id = {
            let mut store = SharedStore::open(&path).unwrap();
            let host = Actor::host("ホスト");
            let memo = store.create(&host, "移行前", "本文", None).unwrap();
            // v2 相当に偽装
            store.conn.pragma_update(None, "user_version", 2).unwrap();
            memo.id
        };
        let mut store = SharedStore::open(&path).unwrap();
        let host = Actor::host("ホスト");
        assert_eq!(store.get(&host, &memo_id).unwrap().title, "移行前");
        store
            .set_perms(
                &host,
                &memo_id,
                SharedPermLevel::Viewer,
                &[],
                Some(&[SharedGroupPerm {
                    group_id: "g1".to_string(),
                    name: "チーム".to_string(),
                    level: SharedPermLevel::Editor,
                }]),
            )
            .unwrap();
        assert_eq!(store.memo_ids_with_group_perms().unwrap(), vec![memo_id]);
    }

    /// コメント(ADR-0052 決定 4): 閲覧者(viewer 以上)が閲覧・追加でき、
    /// 削除は本人・所有者・ホストだけ。全体権限を失った第三者は閲覧も追加も
    /// できない。
    #[test]
    fn comment_add_list_delete_permissions() {
        let (_dir, mut store) = open_temp();
        let alice = Actor::member("id-alice", "アリス"); // 所有者
        let bob = Actor::member("id-bob", "ボブ"); // 既定 viewer
        let host = Actor::host("ホスト");

        let memo = store.create(&alice, "共有", "本文", None).unwrap();
        let comment = store.comment_add(&bob, &memo.id, "了解です").unwrap();
        assert_eq!(comment.author_id, "id-bob");
        assert_eq!(comment.author_name, "ボブ");
        assert_eq!(comment.memo_id, memo.id);

        let comments = store.comment_list(&alice, &memo.id).unwrap();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].comment_id, comment.comment_id);
        // 一覧・詳細の comment_count に反映される
        assert_eq!(store.get(&alice, &memo.id).unwrap().comment_count, 1);
        let (memos, _) = store.list(&alice, &SharedMemoQuery::default()).unwrap();
        assert_eq!(memos[0].comment_count, 1);

        // 第三者(閲覧権限なし)は閲覧・追加ともに拒否
        store
            .set_perms(&alice, &memo.id, SharedPermLevel::None, &[], None)
            .unwrap();
        let carol = Actor::member("id-carol", "キャロル");
        assert!(store.comment_list(&carol, &memo.id).is_err());
        assert!(store.comment_add(&carol, &memo.id, "x").is_err());
        // 全体権限を戻す
        store
            .set_perms(&alice, &memo.id, SharedPermLevel::Viewer, &[], None)
            .unwrap();

        // 削除: 本人(ボブ)は自分のコメントを削除できる
        let second = store.comment_add(&carol, &memo.id, "追加").unwrap();
        assert!(
            store
                .comment_delete(&carol, &memo.id, &comment.comment_id)
                .is_err(),
            "他人のコメントは削除できない"
        );
        // 所有者(アリス)・ホストは誰のコメントでも削除できる
        store
            .comment_delete(&alice, &memo.id, &comment.comment_id)
            .unwrap();
        store
            .comment_delete(&host, &memo.id, &second.comment_id)
            .unwrap();
        assert!(store.comment_list(&alice, &memo.id).unwrap().is_empty());
        assert_eq!(store.get(&alice, &memo.id).unwrap().comment_count, 0);
    }

    /// コメントの本文上限(4KiB)・件数上限(500 件、ADR-0052 決定 4)。
    #[test]
    fn comment_add_enforces_body_and_count_limits() {
        let (_dir, mut store) = open_temp();
        let host = Actor::host("ホスト");
        let memo = store.create(&host, "共有", "本文", None).unwrap();

        let too_long = "a".repeat(4 * 1024 + 1);
        assert!(store.comment_add(&host, &memo.id, &too_long).is_err());
        let just_fits = "a".repeat(4 * 1024);
        assert!(store.comment_add(&host, &memo.id, &just_fits).is_ok());

        assert!(store.comment_add(&host, &memo.id, "").is_err(), "空は拒否");

        // 件数上限(1 件は既に追加済みなので残り 499 件を追加して満杯にする)
        for _ in 0..499 {
            store.comment_add(&host, &memo.id, "x").unwrap();
        }
        assert_eq!(store.comment_list(&host, &memo.id).unwrap().len(), 500);
        assert!(
            store.comment_add(&host, &memo.id, "over").is_err(),
            "500 件を超えるコメントは拒否"
        );
    }

    /// メモの完全削除でコメントも消える(ADR-0052 決定 4)。
    #[test]
    fn delete_forever_cascades_comments() {
        let (_dir, mut store) = open_temp();
        let host = Actor::host("ホスト");
        let memo = store.create(&host, "共有", "本文", None).unwrap();
        store.comment_add(&host, &memo.id, "残る?").unwrap();
        fn count_in_db(store: &SharedStore, memo_id: &str) -> i64 {
            store
                .conn
                .query_row(
                    "SELECT COUNT(*) FROM memo_comments WHERE memo_id = ?1",
                    params![memo_id],
                    |row| row.get(0),
                )
                .unwrap()
        }
        assert_eq!(count_in_db(&store, &memo.id), 1);
        store.trash(&host, &memo.id).unwrap();
        store.delete_forever(&host, &memo.id).unwrap();
        assert_eq!(count_in_db(&store, &memo.id), 0);
    }

    /// v3(ADR-0051 まで)→ v4(ADR-0052 決定 4、コメント)への移行。
    #[test]
    fn v3_database_migrates_to_v4_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("host.memos.db");
        let memo_id = {
            let mut store = SharedStore::open(&path).unwrap();
            let host = Actor::host("ホスト");
            let memo = store.create(&host, "移行前", "本文", None).unwrap();
            // v3 相当に偽装
            store.conn.pragma_update(None, "user_version", 3).unwrap();
            memo.id
        };
        let mut store = SharedStore::open(&path).unwrap();
        let host = Actor::host("ホスト");
        let comment = store.comment_add(&host, &memo_id, "移行後の1件目").unwrap();
        assert_eq!(store.comment_list(&host, &memo_id).unwrap().len(), 1);
        assert_eq!(store.get(&host, &memo_id).unwrap().comment_count, 1);
        store
            .comment_delete(&host, &memo_id, &comment.comment_id)
            .unwrap();
    }

    /// メモ間リンク(ADR-0052 決定 2): タイトル解決に可視性フィルタがかかる
    /// (見えないメモは他に候補があっても解決結果から除外される)。
    #[test]
    fn resolve_titles_applies_visibility_and_excludes_trash() {
        let (_dir, mut store) = open_temp();
        let alice = Actor::member("id-alice", "アリス");
        let bob = Actor::member("id-bob", "ボブ");

        let old = store.create(&alice, "重複", "古い方", None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let newest = store.create(&alice, "重複", "新しい方", None).unwrap();
        let trashed = store.create(&alice, "捨てた", "x", None).unwrap();
        store.trash(&alice, &trashed.id).unwrap();

        // 既定(everyone = viewer)ではボブにも見える
        let map = store
            .resolve_titles(&bob, &["重複".to_string(), "捨てた".to_string()])
            .unwrap();
        assert_eq!(map.get("重複"), Some(&newest.id));
        assert_ne!(map.get("重複"), Some(&old.id));
        assert!(!map.contains_key("捨てた"), "ゴミ箱は解決対象外");

        // 見えなくすると解決結果から消える(古い方へフォールバックしない)
        store
            .set_perms(&alice, &newest.id, SharedPermLevel::None, &[], None)
            .unwrap();
        let map = store.resolve_titles(&bob, &["重複".to_string()]).unwrap();
        assert!(!map.contains_key("重複"));
        // ホストには常に見える
        let host = Actor::host("ホスト");
        assert_eq!(
            store.resolve_titles(&host, &["重複".to_string()]).unwrap()["重複"],
            newest.id
        );
    }

    /// バックリンク(共有): 可視性フィルタがかかり、自分自身・ゴミ箱は除く。
    #[test]
    fn shared_backlinks_applies_visibility() {
        let (_dir, mut store) = open_temp();
        let alice = Actor::member("id-alice", "アリス");
        let bob = Actor::member("id-bob", "ボブ");

        let target = store.create(&alice, "共有資料", "本文", None).unwrap();
        let linking = store
            .create(&alice, "議事録", "[[共有資料]] を参照", None)
            .unwrap();
        // ボブだけが見えるリンク元(アリスから見ても構わないが、可視性の
        // 対称性は問わない仕様。ここではボブ視点で検証する)
        let backlinks = store.backlinks(&bob, &target.id).unwrap();
        assert_eq!(backlinks.len(), 1);
        assert_eq!(backlinks[0].id, linking.id);

        // リンク元をボブに見せなくすると、ボブの結果から消える
        store
            .set_perms(&alice, &linking.id, SharedPermLevel::None, &[], None)
            .unwrap();
        assert!(store.backlinks(&bob, &target.id).unwrap().is_empty());
        // 所有者(アリス)には常に見える
        assert_eq!(store.backlinks(&alice, &target.id).unwrap().len(), 1);

        // ターゲット自体が見えなければバックリンク取得は拒否される
        store
            .set_perms(&alice, &target.id, SharedPermLevel::None, &[], None)
            .unwrap();
        assert!(store.backlinks(&bob, &target.id).is_err());
    }

    /// メモ間リンク(キャッシュ版、ADR-0052 決定 2): List/Get と同じく
    /// オフライン(ホスト未接続)でもキャッシュだけで解決・バックリンクが働く。
    #[test]
    fn cache_resolve_titles_and_backlinks() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = CacheStore::open(&dir.path().join("m.memocache.db")).unwrap();
        let make = |id: &str, title: &str, body: &str| SharedMemoDetail {
            id: id.to_string(),
            title: title.to_string(),
            body: body.to_string(),
            folder_id: None,
            revision: 1,
            created_at: 1,
            updated_at: 1,
            updated_by: None,
            owner_id: String::new(),
            owner_name: "ホスト".to_string(),
            deleted_at: None,
            can_edit: false,
            can_manage: false,
            locked_by: None,
            everyone: None,
            members: vec![],
            groups: vec![],
            comment_count: 0,
        };
        cache.upsert(&make("m1", "共有資料", "本文")).unwrap();
        cache
            .upsert(&make("m2", "議事録", "[[共有資料]] を参照"))
            .unwrap();

        let map = cache
            .resolve_titles(&["共有資料".to_string(), "存在しない".to_string()])
            .unwrap();
        assert_eq!(map.get("共有資料"), Some(&"m1".to_string()));
        assert!(!map.contains_key("存在しない"));

        let backlinks = cache.backlinks("m1").unwrap();
        assert_eq!(backlinks.len(), 1);
        assert_eq!(backlinks[0].id, "m2");
        assert!(cache.backlinks("m2").unwrap().is_empty());
    }

    #[test]
    fn cache_roundtrip_and_search() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = CacheStore::open(&dir.path().join("m.memocache.db")).unwrap();
        let detail = SharedMemoDetail {
            id: "m1".to_string(),
            title: "サーバー情報".to_string(),
            body: "メンテナンス手順".to_string(),
            folder_id: None,
            revision: 3,
            created_at: 1,
            updated_at: 2,
            updated_by: Some("ホスト".to_string()),
            owner_id: String::new(),
            owner_name: "ホスト".to_string(),
            deleted_at: None,
            can_edit: true,
            can_manage: false,
            locked_by: None,
            everyone: None,
            members: vec![],
            groups: vec![],
            comment_count: 0,
        };
        cache.upsert(&detail).unwrap();
        assert_eq!(cache.revision("m1").unwrap(), Some(3));
        // かな同一視の検索(キャッシュでも効く)
        let (memos, _) = cache
            .list(&SharedMemoQuery {
                search: Some("めんてなんす".to_string()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(memos.len(), 1);
        assert!(memos[0].can_edit);
        cache.set_lock("m1", Some("アリス")).unwrap();
        assert_eq!(
            cache.get("m1").unwrap().locked_by.as_deref(),
            Some("アリス")
        );
        cache.retain(&[]).unwrap();
        assert!(cache.get("m1").is_err());
    }

    #[test]
    fn cache_enforce_limit_removes_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = CacheStore::open(&dir.path().join("m.memocache.db")).unwrap();
        let make = |id: &str, updated_at: u64, body: String| SharedMemoDetail {
            id: id.to_string(),
            title: "t".to_string(),
            body,
            folder_id: None,
            revision: 1,
            created_at: updated_at,
            updated_at,
            updated_by: None,
            owner_id: String::new(),
            owner_name: "ホスト".to_string(),
            deleted_at: None,
            can_edit: false,
            can_manage: false,
            locked_by: None,
            everyone: None,
            members: vec![],
            groups: vec![],
            comment_count: 0,
        };
        cache.upsert(&make("m1", 1, "a".repeat(1000))).unwrap();
        cache.upsert(&make("m2", 2, "b".repeat(1000))).unwrap();
        cache.upsert(&make("m3", 3, "c".repeat(1000))).unwrap();
        let deleted = cache.enforce_limit_with(1500).unwrap();
        assert_eq!(deleted, 2, "古い 2 件が消え、最新 1 件が残る");
        assert!(cache.get("m1").is_err());
        assert!(cache.get("m2").is_err());
        assert!(cache.get("m3").is_ok());
    }

    /// キャッシュの comment_count(ADR-0052 決定 4): Changed イベントの
    /// upsert・List・Get のいずれでも反映される(コメント本文自体は
    /// キャッシュに保存しない — 一覧は都度ホストへ取得する)。
    #[test]
    fn cache_upsert_carries_comment_count() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = CacheStore::open(&dir.path().join("m.memocache.db")).unwrap();
        let detail = SharedMemoDetail {
            id: "m1".to_string(),
            title: "t".to_string(),
            body: "本文".to_string(),
            folder_id: None,
            revision: 1,
            created_at: 1,
            updated_at: 1,
            updated_by: None,
            owner_id: String::new(),
            owner_name: "ホスト".to_string(),
            deleted_at: None,
            can_edit: false,
            can_manage: false,
            locked_by: None,
            everyone: None,
            members: vec![],
            groups: vec![],
            comment_count: 3,
        };
        cache.upsert(&detail).unwrap();
        assert_eq!(cache.get("m1").unwrap().comment_count, 3);
        let (memos, _) = cache.list(&SharedMemoQuery::default()).unwrap();
        assert_eq!(memos[0].comment_count, 3);
    }

    // ---- 共有スケジュール表(M6 G-1、ADR-0053) ----

    /// CRUD の基本動作と、全員閲覧・全員追加可(決定 3)を確認する。
    #[test]
    fn schedule_crud_basic() {
        let (_dir, mut store) = open_temp();
        let alice = Actor::member("id-alice", "アリス");
        let bob = Actor::member("id-bob", "ボブ");

        let created = store
            .schedule_create(
                &alice,
                "定例会議",
                "議題未定",
                1_000,
                Some(2_000),
                false,
                &[],
            )
            .unwrap();
        assert_eq!(created.title, "定例会議");
        assert_eq!(created.owner_id, "id-alice");
        assert_eq!(created.revision, 1);
        assert!(created.can_edit, "作成者は編集できる");
        assert!(created.participants.is_empty());

        // 全員が閲覧できる(ボブから見ても can_edit は偽)
        let list = store.schedule_list(&bob).unwrap();
        assert_eq!(list.len(), 1);
        assert!(!list[0].can_edit, "作成者以外は編集不可");

        // ボブも作成できる(決定 3: 追加は全員可)。参加メンバー付き。
        let participants = vec![
            ScheduleParticipant {
                member_id: "id-alice".to_string(),
                name: "アリス".to_string(),
            },
            ScheduleParticipant {
                member_id: "id-bob".to_string(),
                name: "ボブ".to_string(),
            },
        ];
        let bob_event = store
            .schedule_create(&bob, "ボブの予定", "", 3_000, None, true, &participants)
            .unwrap();
        assert!(bob_event.can_edit);
        assert_eq!(bob_event.participants, participants);
        assert_eq!(store.schedule_list(&alice).unwrap().len(), 2);
        // 一覧経由でも参加メンバーが復元される
        assert_eq!(
            store
                .schedule_list(&alice)
                .unwrap()
                .into_iter()
                .find(|e| e.id == bob_event.id)
                .unwrap()
                .participants,
            participants
        );

        let updated = store
            .schedule_update(
                &alice,
                &created.id,
                created.revision,
                "改題した定例会議",
                "詳細を追加",
                1_500,
                Some(2_500),
                false,
                &participants,
            )
            .unwrap();
        assert_eq!(updated.title, "改題した定例会議");
        assert_eq!(updated.revision, 2);
        assert_eq!(updated.participants, participants);

        store.schedule_delete(&alice, &created.id).unwrap();
        assert_eq!(store.schedule_list(&alice).unwrap().len(), 1);
        assert!(store.schedule_get(&alice, &created.id).is_err());
    }

    /// 参加メンバーの件数上限(決定 5)。
    #[test]
    fn schedule_enforces_participant_limit() {
        let (_dir, mut store) = open_temp();
        let alice = Actor::member("id-alice", "アリス");
        let too_many: Vec<ScheduleParticipant> = (0..MAX_SCHEDULE_PARTICIPANTS + 1)
            .map(|i| ScheduleParticipant {
                member_id: format!("m{i}"),
                name: format!("name{i}"),
            })
            .collect();
        let err = store
            .schedule_create(&alice, "t", "", 1_000, None, false, &too_many)
            .unwrap_err();
        assert!(err.to_string().contains("上限"));
    }

    /// 編集・削除は作成者 + ホストのみ(決定 3)。他人は拒否される。
    #[test]
    fn schedule_edit_delete_restricted_to_owner_and_host() {
        let (_dir, mut store) = open_temp();
        let alice = Actor::member("id-alice", "アリス");
        let bob = Actor::member("id-bob", "ボブ");
        let host = Actor::host("ホスト");

        let event = store
            .schedule_create(&alice, "アリスの予定", "", 1_000, None, false, &[])
            .unwrap();

        // ボブは編集も削除もできない
        assert!(store
            .schedule_update(
                &bob,
                &event.id,
                event.revision,
                "改ざん",
                "",
                1_000,
                None,
                false,
                &[],
            )
            .is_err());
        assert!(store.schedule_delete(&bob, &event.id).is_err());

        // ホストはできる
        let by_host = store
            .schedule_update(
                &host,
                &event.id,
                event.revision,
                "ホストが編集",
                "",
                1_000,
                None,
                false,
                &[],
            )
            .unwrap();
        assert_eq!(by_host.title, "ホストが編集");
        assert!(store.schedule_delete(&host, &event.id).is_ok());
    }

    /// revision CAS(決定 4): 古い revision での更新は拒否される。
    #[test]
    fn schedule_update_rejects_stale_revision() {
        let (_dir, mut store) = open_temp();
        let alice = Actor::member("id-alice", "アリス");
        let event = store
            .schedule_create(&alice, "予定", "", 1_000, None, false, &[])
            .unwrap();
        store
            .schedule_update(
                &alice,
                &event.id,
                event.revision,
                "更新1",
                "",
                1_000,
                None,
                false,
                &[],
            )
            .unwrap();
        // event.revision はもう古い(1 →更新後は 2)
        let err = store
            .schedule_update(
                &alice,
                &event.id,
                event.revision,
                "更新2",
                "",
                1_000,
                None,
                false,
                &[],
            )
            .unwrap_err();
        assert!(err.to_string().contains("competing_edit"));
    }

    /// 上限の検証(決定 6): タイトル必須・上限文字数、詳細のバイト上限、
    /// 終了日時の整合性。
    #[test]
    fn schedule_validates_input() {
        let (_dir, mut store) = open_temp();
        let alice = Actor::member("id-alice", "アリス");

        assert!(
            store
                .schedule_create(&alice, "  ", "", 1_000, None, false, &[])
                .is_err(),
            "空タイトルは拒否"
        );
        assert!(
            store
                .schedule_create(&alice, &"あ".repeat(201), "", 1_000, None, false, &[])
                .is_err(),
            "201 文字は上限超過"
        );
        assert!(
            store
                .schedule_create(
                    &alice,
                    "t",
                    &"x".repeat(4 * 1024 + 1),
                    1_000,
                    None,
                    false,
                    &[]
                )
                .is_err(),
            "詳細 4KiB 超過"
        );
        assert!(
            store
                .schedule_create(&alice, "t", "", 2_000, Some(1_000), false, &[])
                .is_err(),
            "終了が開始より前"
        );
        assert!(
            store
                .schedule_create(&alice, "t", "", 1_000, Some(1_000), false, &[])
                .is_ok(),
            "終了 = 開始は許容"
        );
    }

    /// 件数上限(決定 6)。
    #[test]
    fn schedule_enforces_event_count_limit() {
        let (_dir, mut store) = open_temp();
        let alice = Actor::member("id-alice", "アリス");
        // 上限に達するまで直接 INSERT して高速化する(create の 1 件ずつは重い)
        {
            let tx = store.conn.transaction().unwrap();
            for i in 0..MAX_SCHEDULE_EVENTS {
                tx.execute(
                    "INSERT INTO schedule_events
                        (id, title, note, start_at, owner_id, owner_name, updated_by,
                         revision, created_at, updated_at)
                     VALUES (?1, 't', '', 0, '', '', '', 1, 0, 0)",
                    params![format!("e{i}")],
                )
                .unwrap();
            }
            tx.commit().unwrap();
        }
        let err = store
            .schedule_create(&alice, "溢れる予定", "", 1_000, None, false, &[])
            .unwrap_err();
        assert!(err.to_string().contains("上限"));
    }

    /// v4 → v5 移行(ADR-0053): 既存の memos.db に schedule_events が
    /// 追加され、以後の操作が問題なく行える。
    #[test]
    fn v4_database_migrates_to_v5_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("host.memos.db");
        {
            let mut store = SharedStore::open(&path).unwrap();
            let host = Actor::host("ホスト");
            store.create(&host, "既存メモ", "本文", None).unwrap();
            // v4 相当に偽装
            store.conn.pragma_update(None, "user_version", 4).unwrap();
        }
        let mut store = SharedStore::open(&path).unwrap();
        let host = Actor::host("ホスト");
        let event = store
            .schedule_create(&host, "移行後の予定", "", 1_000, None, false, &[])
            .unwrap();
        assert_eq!(store.schedule_list(&host).unwrap().len(), 1);
        assert_eq!(store.db_stats().unwrap().schedule_count, 1);
        store.schedule_delete(&host, &event.id).unwrap();
    }

    /// v6 → v7 移行(M6 H-3、ADR-0055 決定 5): 既存の schedule_events に
    /// participants 列が additive に足され、以後の操作が問題なく行える
    /// (旧行は participants = '[]' として読める)。
    #[test]
    fn v6_database_migrates_to_v7_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("host.memos.db");
        let host = Actor::host("ホスト");
        let event_id = {
            let mut store = SharedStore::open(&path).unwrap();
            let event = store
                .schedule_create(&host, "移行前の予定", "", 1_000, None, false, &[])
                .unwrap();
            // v6 相当に偽装(participants 列が無かった頃)
            store.conn.pragma_update(None, "user_version", 6).unwrap();
            event.id
        };
        let mut store = SharedStore::open(&path).unwrap();
        let event = store.schedule_get(&host, &event_id).unwrap();
        assert!(event.participants.is_empty(), "旧行は participants 空");
        let participants = vec![ScheduleParticipant {
            member_id: "id-alice".to_string(),
            name: "アリス".to_string(),
        }];
        let updated = store
            .schedule_update(
                &host,
                &event_id,
                event.revision,
                "移行後に編集",
                "",
                1_000,
                None,
                false,
                &participants,
            )
            .unwrap();
        assert_eq!(updated.participants, participants);
    }

    /// キャッシュ往復(メンバー側): replace_all・upsert・remove・list・get。
    #[test]
    fn schedule_cache_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = CacheStore::open(&dir.path().join("m.memocache.db")).unwrap();
        let make = |id: &str, start: u64| ScheduleEvent {
            id: id.to_string(),
            title: format!("予定{id}"),
            note: String::new(),
            start_unix_ms: start,
            end_unix_ms: None,
            all_day: false,
            owner_id: String::new(),
            owner_name: "ホスト".to_string(),
            updated_by: "ホスト".to_string(),
            participants: vec![ScheduleParticipant {
                member_id: "id-alice".to_string(),
                name: "アリス".to_string(),
            }],
            revision: 1,
            created_at: 1,
            updated_at: 1,
            can_edit: true,
        };
        cache
            .schedule_replace_all(&[make("e1", 2_000), make("e2", 1_000)])
            .unwrap();
        let list = cache.schedule_list().unwrap();
        assert_eq!(list.len(), 2);
        // start_at 昇順
        assert_eq!(list[0].id, "e2");
        assert_eq!(list[1].id, "e1");
        assert_eq!(list[0].participants.len(), 1, "participants も往復する");

        let mut updated = make("e1", 2_000);
        updated.title = "改題".to_string();
        updated.revision = 2;
        cache.schedule_upsert(&updated).unwrap();
        assert_eq!(cache.schedule_get("e1").unwrap().title, "改題");

        cache.schedule_remove("e2").unwrap();
        assert_eq!(cache.schedule_list().unwrap().len(), 1);
        assert!(cache.schedule_get("e2").is_err());
    }

    // ---- 共有シート(M6 G-2、ADR-0054) ----

    /// シート CRUD の基本動作。作成は全員可、改名・削除は作成者 + ホスト
    /// (決定 5)。
    #[test]
    fn sheet_crud_basic() {
        let (_dir, mut store) = open_temp();
        let alice = Actor::member("id-alice", "アリス");
        let bob = Actor::member("id-bob", "ボブ");

        let sheet = store.sheet_create(&alice, "在庫表").unwrap();
        assert_eq!(sheet.name, "在庫表");
        assert_eq!(sheet.owner_id, "id-alice");
        assert!(sheet.can_manage, "作成者は管理できる");

        // 全員が閲覧できる(ボブから見ても can_manage は偽)
        let list = store.sheet_list(&bob).unwrap();
        assert_eq!(list.len(), 1);
        assert!(!list[0].can_manage, "作成者以外は管理不可");

        // ボブも作成できる(決定 5: 作成は全員可)
        let bob_sheet = store.sheet_create(&bob, "ボブの表").unwrap();
        assert!(bob_sheet.can_manage);
        assert_eq!(store.sheet_list(&alice).unwrap().len(), 2);

        let renamed = store
            .sheet_rename(&alice, &sheet.id, "改題した在庫表")
            .unwrap();
        assert_eq!(renamed.name, "改題した在庫表");

        store.sheet_delete(&alice, &sheet.id).unwrap();
        assert_eq!(store.sheet_list(&alice).unwrap().len(), 1);
        assert!(store.sheet_get(&alice, &sheet.id).is_err());
    }

    /// 改名・削除は作成者 + ホストのみ(決定 5)。他人は拒否される。
    #[test]
    fn sheet_manage_restricted_to_owner_and_host() {
        let (_dir, mut store) = open_temp();
        let alice = Actor::member("id-alice", "アリス");
        let bob = Actor::member("id-bob", "ボブ");
        let host = Actor::host("ホスト");

        let sheet = store.sheet_create(&alice, "アリスの表").unwrap();

        assert!(store.sheet_rename(&bob, &sheet.id, "改ざん").is_err());
        assert!(store.sheet_delete(&bob, &sheet.id).is_err());

        let by_host = store
            .sheet_rename(&host, &sheet.id, "ホストが改名")
            .unwrap();
        assert_eq!(by_host.name, "ホストが改名");
        assert!(store.sheet_delete(&host, &sheet.id).is_ok());
    }

    /// セル書き込みは全員可(決定 5)。新規セルは revision 1、
    /// 更新は revision+1。空文字書き込みでセル削除。
    #[test]
    fn sheet_write_basic_and_delete() {
        let (_dir, mut store) = open_temp();
        let alice = Actor::member("id-alice", "アリス");
        let bob = Actor::member("id-bob", "ボブ");
        let sheet = store.sheet_create(&alice, "表").unwrap();

        let (applied, conflicts, changed) = store
            .sheet_write(
                &bob,
                &sheet.id,
                &[CellWrite {
                    row: 0,
                    col: 0,
                    value: "10".to_string(),
                    base_revision: 0,
                }],
            )
            .unwrap();
        assert_eq!(applied, 1);
        assert!(conflicts.is_empty());
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].revision, 1);

        let cells = store.sheet_cells(&sheet.id).unwrap();
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].value, "10");

        // 更新(base_revision 一致)
        let (applied, conflicts, changed) = store
            .sheet_write(
                &alice,
                &sheet.id,
                &[CellWrite {
                    row: 0,
                    col: 0,
                    value: "20".to_string(),
                    base_revision: 1,
                }],
            )
            .unwrap();
        assert_eq!(applied, 1);
        assert!(conflicts.is_empty());
        assert_eq!(changed[0].revision, 2);
        assert_eq!(store.sheet_cells(&sheet.id).unwrap()[0].value, "20");

        // 空文字 = セル削除
        let (applied, conflicts, changed) = store
            .sheet_write(
                &alice,
                &sheet.id,
                &[CellWrite {
                    row: 0,
                    col: 0,
                    value: String::new(),
                    base_revision: 2,
                }],
            )
            .unwrap();
        assert_eq!(applied, 1);
        assert!(conflicts.is_empty());
        assert_eq!(changed[0].value, "");
        assert!(store.sheet_cells(&sheet.id).unwrap().is_empty());
    }

    /// バッチ書き込みの部分失敗: 競合セルだけ conflicts に載り、他は適用される。
    #[test]
    fn sheet_write_partial_conflict() {
        let (_dir, mut store) = open_temp();
        let alice = Actor::member("id-alice", "アリス");
        let sheet = store.sheet_create(&alice, "表").unwrap();
        store
            .sheet_write(
                &alice,
                &sheet.id,
                &[
                    CellWrite {
                        row: 0,
                        col: 0,
                        value: "A".to_string(),
                        base_revision: 0,
                    },
                    CellWrite {
                        row: 0,
                        col: 1,
                        value: "B".to_string(),
                        base_revision: 0,
                    },
                ],
            )
            .unwrap();

        // (0,0) は古い base_revision(競合)、(0,1) は new(別セル、成功)、
        // (1,0) は新規セル(成功)
        let (applied, conflicts, changed) = store
            .sheet_write(
                &alice,
                &sheet.id,
                &[
                    CellWrite {
                        row: 0,
                        col: 0,
                        value: "A2".to_string(),
                        base_revision: 0, // 本当は 1 のはずが古いまま
                    },
                    CellWrite {
                        row: 0,
                        col: 1,
                        value: "B2".to_string(),
                        base_revision: 1,
                    },
                    CellWrite {
                        row: 1,
                        col: 0,
                        value: "C".to_string(),
                        base_revision: 0,
                    },
                ],
            )
            .unwrap();
        assert_eq!(applied, 2);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].row, 0);
        assert_eq!(conflicts[0].col, 0);
        assert_eq!(conflicts[0].value, "A", "競合セルの現在値が返る");
        assert_eq!(changed.len(), 2);

        let cells = store.sheet_cells(&sheet.id).unwrap();
        assert_eq!(cells.len(), 3, "競合した (0,0) は元の値のまま残る");
        let a = cells.iter().find(|c| c.row == 0 && c.col == 0).unwrap();
        assert_eq!(a.value, "A");
    }

    /// 存在しないセルに非 0 の base_revision を送ると競合扱い(既に削除済み扱い)。
    #[test]
    fn sheet_write_stale_delete_is_conflict() {
        let (_dir, mut store) = open_temp();
        let alice = Actor::member("id-alice", "アリス");
        let sheet = store.sheet_create(&alice, "表").unwrap();
        let (applied, conflicts, _) = store
            .sheet_write(
                &alice,
                &sheet.id,
                &[CellWrite {
                    row: 0,
                    col: 0,
                    value: "X".to_string(),
                    base_revision: 5,
                }],
            )
            .unwrap();
        assert_eq!(applied, 0);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].revision, 0);
    }

    /// 上限の検証(決定 7): シート名必須・上限文字数、セル値のバイト上限、
    /// セルの行・列上限。
    #[test]
    fn sheet_validates_input() {
        let (_dir, mut store) = open_temp();
        let alice = Actor::member("id-alice", "アリス");

        assert!(store.sheet_create(&alice, "  ").is_err(), "空名は拒否");
        assert!(
            store.sheet_create(&alice, &"あ".repeat(101)).is_err(),
            "101 文字は上限超過"
        );

        let sheet = store.sheet_create(&alice, "表").unwrap();
        assert!(
            store
                .sheet_write(
                    &alice,
                    &sheet.id,
                    &[CellWrite {
                        row: 0,
                        col: 0,
                        value: "x".repeat(4 * 1024 + 1),
                        base_revision: 0,
                    }],
                )
                .is_err(),
            "セル値 4KiB 超過"
        );
        assert!(
            store
                .sheet_write(
                    &alice,
                    &sheet.id,
                    &[CellWrite {
                        row: MAX_SHEET_ROWS,
                        col: 0,
                        value: "x".to_string(),
                        base_revision: 0,
                    }],
                )
                .is_err(),
            "行の上限超過"
        );
        assert!(
            store
                .sheet_write(
                    &alice,
                    &sheet.id,
                    &[CellWrite {
                        row: 0,
                        col: MAX_SHEET_COLS,
                        value: "x".to_string(),
                        base_revision: 0,
                    }],
                )
                .is_err(),
            "列の上限超過"
        );
    }

    /// シート枚数の上限(決定 7)。
    #[test]
    fn sheet_enforces_sheet_count_limit() {
        let (_dir, mut store) = open_temp();
        let alice = Actor::member("id-alice", "アリス");
        {
            let tx = store.conn.transaction().unwrap();
            for i in 0..MAX_SHEETS {
                tx.execute(
                    "INSERT INTO sheets (id, name, owner_id, owner_name, created_at, updated_at)
                     VALUES (?1, 't', '', '', 0, 0)",
                    params![format!("s{i}")],
                )
                .unwrap();
            }
            tx.commit().unwrap();
        }
        let err = store.sheet_create(&alice, "溢れるシート").unwrap_err();
        assert!(err.to_string().contains("上限"));
    }

    /// 非空セル数の上限(決定 7)。
    #[test]
    fn sheet_enforces_cell_count_limit() {
        let (_dir, mut store) = open_temp();
        let alice = Actor::member("id-alice", "アリス");
        let sheet = store.sheet_create(&alice, "表").unwrap();
        {
            let tx = store.conn.transaction().unwrap();
            for i in 0..MAX_SHEET_CELLS {
                tx.execute(
                    "INSERT INTO sheet_cells (sheet_id, row, col, value, revision, updated_by, updated_at)
                     VALUES (?1, ?2, 0, 'v', 1, '', 0)",
                    params![sheet.id, i as i64],
                )
                .unwrap();
            }
            tx.commit().unwrap();
        }
        let err = store
            .sheet_write(
                &alice,
                &sheet.id,
                &[CellWrite {
                    row: 999,
                    col: 1,
                    value: "溢れるセル".to_string(),
                    base_revision: 0,
                }],
            )
            .unwrap_err();
        assert!(err.to_string().contains("上限"));
    }

    /// シート削除でセルもまとめて消える。
    #[test]
    fn sheet_delete_cascades_cells() {
        let (_dir, mut store) = open_temp();
        let alice = Actor::member("id-alice", "アリス");
        let sheet = store.sheet_create(&alice, "表").unwrap();
        store
            .sheet_write(
                &alice,
                &sheet.id,
                &[CellWrite {
                    row: 0,
                    col: 0,
                    value: "10".to_string(),
                    base_revision: 0,
                }],
            )
            .unwrap();
        store.sheet_delete(&alice, &sheet.id).unwrap();
        let count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM sheet_cells", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0, "削除したシートのセルは残らない");
    }

    /// v5 → v6 移行(ADR-0054): 既存の memos.db に sheets / sheet_cells が
    /// 追加され、以後の操作が問題なく行える。
    #[test]
    fn v5_database_migrates_to_v6_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("host.memos.db");
        {
            let mut store = SharedStore::open(&path).unwrap();
            let host = Actor::host("ホスト");
            store
                .schedule_create(&host, "既存予定", "", 1_000, None, false, &[])
                .unwrap();
            // v5 相当に偽装
            store.conn.pragma_update(None, "user_version", 5).unwrap();
        }
        let mut store = SharedStore::open(&path).unwrap();
        let host = Actor::host("ホスト");
        let sheet = store.sheet_create(&host, "移行後の表").unwrap();
        assert_eq!(store.sheet_list(&host).unwrap().len(), 1);
        assert_eq!(store.db_stats().unwrap().sheet_count, 1);
        store.sheet_delete(&host, &sheet.id).unwrap();
    }

    /// キャッシュ往復(メンバー側): メタの replace_all・upsert・remove・list、
    /// セルの replace_all・apply(削除込み)・list。
    #[test]
    fn sheet_cache_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = CacheStore::open(&dir.path().join("m.memocache.db")).unwrap();
        let make_sheet = |id: &str, created: u64| SheetMeta {
            id: id.to_string(),
            name: format!("表{id}"),
            owner_id: String::new(),
            owner_name: "ホスト".to_string(),
            created_at: created,
            updated_at: created,
            can_manage: true,
        };
        cache
            .sheet_replace_all(&[make_sheet("s1", 1_000), make_sheet("s2", 2_000)])
            .unwrap();
        let list = cache.sheet_list().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, "s1");

        let cell = SheetCell {
            row: 0,
            col: 0,
            value: "10".to_string(),
            revision: 1,
            updated_by: "ホスト".to_string(),
            updated_at: 1,
        };
        cache
            .sheet_cells_replace_all("s1", std::slice::from_ref(&cell))
            .unwrap();
        assert_eq!(cache.sheet_cells("s1").unwrap().len(), 1);

        let mut updated = cell.clone();
        updated.value = "20".to_string();
        updated.revision = 2;
        cache.sheet_cells_apply("s1", &[updated]).unwrap();
        assert_eq!(cache.sheet_cells("s1").unwrap()[0].value, "20");

        // value = "" は削除
        let deleted = SheetCell {
            value: String::new(),
            ..cell
        };
        cache.sheet_cells_apply("s1", &[deleted]).unwrap();
        assert!(cache.sheet_cells("s1").unwrap().is_empty());

        // メタ削除でセルも消える(sheet_remove)
        cache
            .sheet_cells_replace_all(
                "s2",
                &[SheetCell {
                    row: 0,
                    col: 0,
                    value: "x".to_string(),
                    revision: 1,
                    updated_by: "ホスト".to_string(),
                    updated_at: 1,
                }],
            )
            .unwrap();
        cache.sheet_remove("s2").unwrap();
        assert_eq!(cache.sheet_list().unwrap().len(), 1);
        assert!(cache.sheet_cells("s2").unwrap().is_empty());

        // メタ upsert(改名)
        let mut renamed = make_sheet("s1", 1_000);
        renamed.name = "改題".to_string();
        cache.sheet_upsert(&renamed).unwrap();
        assert_eq!(cache.sheet_list().unwrap()[0].name, "改題");

        // replace_all で消えたシートの孤児セルも掃除される
        cache
            .sheet_cells_replace_all(
                "s1",
                &[SheetCell {
                    row: 1,
                    col: 1,
                    value: "残る?".to_string(),
                    revision: 1,
                    updated_by: "ホスト".to_string(),
                    updated_at: 1,
                }],
            )
            .unwrap();
        cache.sheet_replace_all(&[]).unwrap();
        assert!(cache.sheet_list().unwrap().is_empty());
        assert!(
            cache.sheet_cells("s1").unwrap().is_empty(),
            "孤児セルは掃除される"
        );
    }
}
