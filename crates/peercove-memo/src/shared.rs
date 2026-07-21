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
use std::path::Path;

use anyhow::{bail, Context};
use peercove_core::memo::{
    checklist_progress, excerpt, MemoFolder, SharedMemberPerm, SharedMemoDetail, SharedMemoQuery,
    SharedMemoSummary, SharedPermLevel, EXCERPT_CHARS,
};
use rusqlite::{params, Connection, OptionalExtension};

use crate::{
    kana_fold, register_kana_fold, unix_ms, validate_folder_name, validate_text, TRASH_RETENTION_MS,
};

/// 操作の主体。権限判定に使う(ホスト管理者はすべて可)。
#[derive(Debug, Clone)]
pub struct Actor {
    /// member_id(= invite_id、ADR-0047)。None = ホスト管理者。
    pub member_id: Option<String>,
    /// 表示名(更新者・所有者のスナップショットに使う)。
    pub name: String,
}

impl Actor {
    pub fn host(name: impl Into<String>) -> Self {
        Self {
            member_id: None,
            name: name.into(),
        }
    }

    pub fn member(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            member_id: Some(id.into()),
            name: name.into(),
        }
    }

    fn is_host(&self) -> bool {
        self.member_id.is_none()
    }

    /// 所有者 ID としての表現(ホスト = 空文字)。
    fn owner_id(&self) -> &str {
        self.member_id.as_deref().unwrap_or("")
    }
}

const SHARED_SCHEMA_VERSION: i64 = 1;

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
}

impl Row {
    fn visible_to(&self, actor: &Actor, perms: &HashMap<String, SharedPermLevel>) -> bool {
        if actor.is_host() || self.owner_id == actor.owner_id() {
            return true;
        }
        if self.deleted_at.is_some() {
            return false; // ゴミ箱は所有者・ホストのみ
        }
        match perms.get(actor.owner_id()) {
            Some(level) => *level != SharedPermLevel::None,
            None => self.everyone != SharedPermLevel::None,
        }
    }

    fn can_edit(&self, actor: &Actor, perms: &HashMap<String, SharedPermLevel>) -> bool {
        if self.deleted_at.is_some() {
            return false; // ゴミ箱は読み取り専用
        }
        if actor.is_host() || self.owner_id == actor.owner_id() {
            return true;
        }
        match perms.get(actor.owner_id()) {
            Some(level) => *level == SharedPermLevel::Editor,
            None => self.everyone == SharedPermLevel::Editor,
        }
    }

    fn can_manage(&self, actor: &Actor) -> bool {
        actor.is_host() || self.owner_id == actor.owner_id()
    }

    fn summary(
        &self,
        actor: &Actor,
        perms: &HashMap<String, SharedPermLevel>,
    ) -> SharedMemoSummary {
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
            can_edit: self.can_edit(actor, perms),
            can_manage: self.can_manage(actor),
            locked_by: None, // サービス層(ロック保持者)が詰める
            checklist_done: done,
            checklist_total: total,
        }
    }

    fn detail(
        &self,
        actor: &Actor,
        perms: &HashMap<String, SharedPermLevel>,
        perm_names: &HashMap<String, String>,
    ) -> SharedMemoDetail {
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
            can_edit: self.can_edit(actor, perms),
            can_manage: manage,
            locked_by: None,
            everyone: manage.then_some(self.everyone),
            members: if manage {
                let mut members: Vec<SharedMemberPerm> = perms
                    .iter()
                    .map(|(member_id, level)| SharedMemberPerm {
                        member_id: member_id.clone(),
                        name: perm_names.get(member_id).cloned().unwrap_or_default(),
                        level: *level,
                    })
                    .collect();
                members.sort_by(|a, b| a.name.cmp(&b.name).then(a.member_id.cmp(&b.member_id)));
                members
            } else {
                Vec::new()
            },
        }
    }
}

/// ホスト正本の共有メモ DB。
pub struct SharedStore {
    conn: Connection,
}

impl SharedStore {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let conn = open_db(path)?;
        let mut store = Self { conn };
        store.migrate()?;
        // 保持期限(30 日)を過ぎたゴミ箱を自動で完全削除(要件 §13/§17)
        let cutoff = unix_ms().saturating_sub(TRASH_RETENTION_MS);
        store.conn.execute(
            "DELETE FROM memos WHERE deleted_at IS NOT NULL AND deleted_at < ?1",
            params![cutoff],
        )?;
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
        tx.pragma_update(None, "user_version", SHARED_SCHEMA_VERSION)?;
        tx.commit()?;
        Ok(())
    }

    fn row(&self, id: &str) -> anyhow::Result<Row> {
        self.conn
            .query_row(
                "SELECT id, title, body, folder_id, revision, owner_id, owner_name,
                        created_at, updated_at, updated_by, everyone, deleted_at
                 FROM memos WHERE id = ?1",
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

    /// 一覧(受信者視点)。ゴミ箱は所有者・ホストの分だけ。
    pub fn list(
        &self,
        actor: &Actor,
        query: &SharedMemoQuery,
    ) -> anyhow::Result<(Vec<SharedMemoSummary>, Vec<MemoFolder>)> {
        let mut sql = String::from(
            "SELECT id, title, body, folder_id, revision, owner_id, owner_name,
                    created_at, updated_at, updated_by, everyone, deleted_at
             FROM memos m WHERE ",
        );
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
            let (perms, _) = self.perms_of(&row.id)?;
            if row.visible_to(actor, &perms) {
                memos.push(row.summary(actor, &perms));
            }
        }
        Ok((memos, self.folders()?))
    }

    /// 1 件取得(受信者視点)。見えないメモはエラー。
    pub fn get(&self, actor: &Actor, id: &str) -> anyhow::Result<SharedMemoDetail> {
        let row = self.row(id)?;
        let (perms, names) = self.perms_of(id)?;
        if !row.visible_to(actor, &perms) {
            bail!("このメモを閲覧する権限がありません");
        }
        Ok(row.detail(actor, &perms, &names))
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
                "SELECT id, title, body, folder_id, revision, owner_id, owner_name,
                        created_at, updated_at, updated_by, everyone, deleted_at
                 FROM memos WHERE id = ?1",
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
        let (perms, names) = self.perms_of(id)?;
        if !row.visible_to(actor, &perms) {
            return Ok(None);
        }
        Ok(Some(row.detail(actor, &perms, &names)))
    }

    pub fn create(
        &mut self,
        actor: &Actor,
        title: &str,
        body: &str,
        folder_id: Option<&str>,
    ) -> anyhow::Result<SharedMemoDetail> {
        validate_text(title, body)?;
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
        validate_text(title, body)?;
        let row = self.row(id)?;
        let (perms, _) = self.perms_of(id)?;
        if !row.visible_to(actor, &perms) {
            bail!("このメモを閲覧する権限がありません");
        }
        if !row.can_edit(actor, &perms) {
            bail!("このメモを編集する権限がありません(閲覧のみ)");
        }
        if row.revision as u64 != base_revision {
            bail!("competing_edit: 他の端末の変更が先に保存されています(最新を読み込み直してください)");
        }
        self.conn.execute(
            "UPDATE memos SET title = ?1, body = ?2, revision = revision + 1,
                    updated_at = ?3, updated_by = ?4
             WHERE id = ?5",
            params![title, body, unix_ms(), actor.name, id],
        )?;
        self.get(actor, id)
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
        self.conn
            .execute("DELETE FROM memos WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// 権限の設定(所有者・ホスト管理者)。`members` は全量置き換え。
    pub fn set_perms(
        &mut self,
        actor: &Actor,
        id: &str,
        everyone: SharedPermLevel,
        members: &[SharedMemberPerm],
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
        tx.commit()?;
        self.get(actor, id)
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
}

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
        let pattern = format!(
            "%{}%",
            search
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
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

const CACHE_SCHEMA_VERSION: i64 = 1;

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
        tx.pragma_update(None, "user_version", CACHE_SCHEMA_VERSION)?;
        tx.commit()?;
        Ok(())
    }

    /// ホストからの詳細(Changed イベント / Get 応答)を反映する。
    pub fn upsert(&mut self, memo: &SharedMemoDetail) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO memos (id, title, body, folder_id, revision, owner_id, owner_name,
                                created_at, updated_at, updated_by, can_edit, can_manage,
                                locked_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(id) DO UPDATE SET
                title = ?2, body = ?3, folder_id = ?4, revision = ?5, owner_id = ?6,
                owner_name = ?7, created_at = ?8, updated_at = ?9, updated_by = ?10,
                can_edit = ?11, can_manage = ?12, locked_by = ?13",
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
            ],
        )?;
        Ok(())
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
                    created_at, updated_at, updated_by, can_edit, can_manage, locked_by
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
                        created_at, updated_at, updated_by, can_edit, can_manage, locked_by
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
                    })
                },
            )
            .optional()?
            .context("共有メモが見つかりません(削除された可能性があります)")
    }
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
            )
            .unwrap();
        assert!(store.get(&bob, &memo.id).unwrap().can_edit);

        // 全体 none + 個別なし = 第三者には見えない
        store
            .set_perms(&alice, &memo.id, SharedPermLevel::None, &[])
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
            .set_perms(&bob, &memo.id, SharedPermLevel::Editor, &[])
            .is_err());
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
            .set_perms(&host, &memo.id, SharedPermLevel::Editor, &[])
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
}
