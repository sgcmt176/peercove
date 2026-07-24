//! メモ帳のストレージエンジン(ADR-0049、M5 F-1)。
//!
//! SQLite(WAL)にメモ・フォルダー・タグを保存し、FTS5(trigram)で全文検索
//! する。OS 非依存で、デスクトップはデーモン、Android は `peercove-mobile` から
//! 同じ [`MemoStore`] を使う(頭脳は Rust = ADR-0039 の方針)。
//!
//! - 個人メモ DB: ネットワーク非依存の 1 ファイル(`memos.db`)
//! - 操作は [`peercove_core::memo::MemoOp`] を [`MemoStore::apply`] に渡す 1 本化
//! - **メモのタイトル・本文・タグ・フォルダー名はログへ出さない**(ADR-0049)。
//!   このモジュールの anyhow エラーは呼び出し元経由で UI に表示されるだけで、
//!   デーモンのログには載らない

pub mod shared;

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{bail, Context};
use peercove_core::memo::{
    checklist_progress, excerpt, DiffLine, DiffLineKind, MemoDetail, MemoFolder, MemoOp, MemoPatch,
    MemoQuery, MemoReminder, MemoReply, MemoScope, MemoSort, MemoSummary, MemoTagCount,
    ReminderScope, EXCERPT_CHARS, MAX_BODY_BYTES, MAX_TITLE_CHARS, TRASH_RETENTION_DAYS,
};
use rusqlite::{params, Connection, OptionalExtension};

/// スキーマ世代。互換性のない変更で上げ、`migrate` に移行を足す。
/// - 2: FTS をかな折り畳み済みテキストの通常表に変更(ひらがな/カタカナ同一視)
/// - 3: `reminders` テーブル追加(端末ローカルのリマインダー、ADR-0052 決定 6)
/// - 4: `reminders` の主キーに `remind_at` を足し 1 対象で複数件を許可、
///   `offset_minutes` 列を追加(端末ローカル、ADR-0055 決定 3)
const SCHEMA_VERSION: i64 = 4;

/// 1 対象(scope・network・memo_id)あたりのリマインダー件数上限
/// (ADR-0055 決定 3)。
const MAX_REMINDERS_PER_TARGET: usize = 10;

/// 1 メモのタグ数の上限(異常な肥大を防ぐだけの緩い値)。
const MAX_TAGS_PER_MEMO: usize = 30;

/// タグ 1 つの長さ上限(文字)。
const MAX_TAG_CHARS: usize = 50;

/// ゴミ箱の保持期間(ミリ秒)。個人・共有とも同じ(要件 §13)。
const TRASH_RETENTION_MS: i64 = TRASH_RETENTION_DAYS as i64 * 24 * 60 * 60 * 1000;

pub struct MemoStore {
    conn: Connection,
}

impl MemoStore {
    /// DB を開く(なければ作る)。親ディレクトリも作成する。
    /// 開いたときに保持期限(30 日)を過ぎたゴミ箱を自動で完全削除する。
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("フォルダーを作成できません: {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("メモデータベースを開けません: {}", path.display()))?;
        // WAL + NORMAL: ローカルディスク前提(要件 §15)。チェックポイントは
        // SQLite の自動チェックポイント(既定 1000 ページ)と接続クローズ時に走る
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        register_kana_fold(&conn)?;
        let mut store = Self { conn };
        store.migrate()?;
        store.purge_expired_trash()?;
        Ok(store)
    }

    fn migrate(&mut self) -> anyhow::Result<()> {
        let version: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version >= SCHEMA_VERSION {
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
                    pinned INTEGER NOT NULL DEFAULT 0,
                    archived INTEGER NOT NULL DEFAULT 0,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    deleted_at INTEGER
                );
                CREATE INDEX idx_memos_updated ON memos(updated_at);
                CREATE TABLE folders (
                    id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    created_at INTEGER NOT NULL
                );
                CREATE TABLE memo_tags (
                    memo_id TEXT NOT NULL REFERENCES memos(id) ON DELETE CASCADE,
                    tag TEXT NOT NULL,
                    PRIMARY KEY (memo_id, tag)
                );
                "#,
            )?;
        }
        if version < 2 {
            // v2: 全文検索(FTS5)。trigram は日本語の部分一致に分かち書きなしで
            // 対応できる(3 文字未満の検索は LIKE で代替)。索引には
            // kana_fold(カタカナ→ひらがな)済みのテキストを入れ、検索語も
            // 同じく折り畳むことで、ひらがな/カタカナを同一視する。
            // v1 の外部コンテンツ表 + 生テキストのトリガーは作り直す
            tx.execute_batch(
                r#"
                DROP TRIGGER IF EXISTS memos_fts_insert;
                DROP TRIGGER IF EXISTS memos_fts_delete;
                DROP TRIGGER IF EXISTS memos_fts_update;
                DROP TABLE IF EXISTS memo_fts;
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
                INSERT INTO memo_fts(rowid, title, body)
                    SELECT rowid, kana_fold(title), kana_fold(body) FROM memos;
                "#,
            )?;
        }
        if version < 3 {
            // v3: リマインダー(端末ローカル、ADR-0052 決定 6)。共有メモに
            // 対する「自分用リマインダー」もここへ入れる(network が識別子)。
            // 個人メモへの外部キーは張らない(共有メモの行は memos に無いため。
            // 個人メモの完全削除は呼び出し側で対の delete を行う)
            tx.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS reminders (
                    scope TEXT NOT NULL,
                    network TEXT NOT NULL DEFAULT '',
                    memo_id TEXT NOT NULL,
                    remind_at INTEGER NOT NULL,
                    fired INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY (scope, network, memo_id)
                );
                "#,
            )?;
        }
        if version < 4 {
            // v4(ADR-0055 決定 3): メモのリマインダーは UI から撤去され、
            // スケジュールの予定リマインダーへ移設。予定は「5 分前」
            // 「15 分前」等の複数オフセット + 任意日時を 1 予定に複数件
            // 設定できる必要があるため、主キーに remind_at を足して
            // 1 対象(scope・network・memo_id)で複数行を許可する。
            // offset_minutes は表示用メタ(発火判定には remind_at を使う)。
            // テーブル再作成マイグレーション(既存行は remind_at そのまま
            // 引き継ぎ、offset_minutes は v3 に無かった情報なので NULL)。
            tx.execute_batch(
                r#"
                CREATE TABLE reminders_new (
                    scope TEXT NOT NULL,
                    network TEXT NOT NULL DEFAULT '',
                    memo_id TEXT NOT NULL,
                    remind_at INTEGER NOT NULL,
                    offset_minutes INTEGER,
                    fired INTEGER NOT NULL DEFAULT 0,
                    PRIMARY KEY (scope, network, memo_id, remind_at)
                );
                INSERT INTO reminders_new (scope, network, memo_id, remind_at, fired)
                    SELECT scope, network, memo_id, remind_at, fired FROM reminders;
                DROP TABLE reminders;
                ALTER TABLE reminders_new RENAME TO reminders;
                "#,
            )?;
        }
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        tx.commit()?;
        Ok(())
    }

    /// 保持期限(30 日)を過ぎたゴミ箱のメモを完全削除する。
    fn purge_expired_trash(&mut self) -> anyhow::Result<()> {
        let cutoff = unix_ms().saturating_sub(TRASH_RETENTION_MS);
        self.conn.execute(
            "DELETE FROM memos WHERE deleted_at IS NOT NULL AND deleted_at < ?1",
            params![cutoff],
        )?;
        Ok(())
    }

    /// 操作を 1 つ適用する(それぞれ 1 トランザクション)。
    pub fn apply(&mut self, op: MemoOp) -> anyhow::Result<MemoReply> {
        match op {
            MemoOp::List { query } => self.list(&query),
            MemoOp::Get { id } => Ok(MemoReply::Memo {
                memo: self.get(&id)?,
            }),
            MemoOp::ResolveTitles { titles } => Ok(MemoReply::Titles {
                map: self.resolve_titles(&titles)?,
            }),
            MemoOp::Backlinks { id } => Ok(MemoReply::Memos {
                memos: self.backlinks(&id)?,
                folders: Vec::new(),
                tags: Vec::new(),
            }),
            MemoOp::Create {
                title,
                body,
                folder_id,
                tags,
            } => self.create(&title, &body, folder_id.as_deref(), &tags),
            MemoOp::Update { id, patch } => self.update(&id, patch),
            MemoOp::Duplicate { id } => self.duplicate(&id),
            MemoOp::Trash { id } => {
                let changed = self.conn.execute(
                    "UPDATE memos SET deleted_at = ?1 WHERE id = ?2 AND deleted_at IS NULL",
                    params![unix_ms(), id],
                )?;
                if changed == 0 {
                    bail!("メモが見つかりません(既に削除された可能性があります)");
                }
                Ok(MemoReply::Done)
            }
            MemoOp::Restore { id } => {
                let changed = self.conn.execute(
                    "UPDATE memos SET deleted_at = NULL WHERE id = ?1 AND deleted_at IS NOT NULL",
                    params![id],
                )?;
                if changed == 0 {
                    bail!("ゴミ箱にこのメモはありません");
                }
                Ok(MemoReply::Done)
            }
            MemoOp::DeleteForever { id } => {
                // 誤操作防止のため、ゴミ箱に入っているメモだけ完全削除できる
                let changed = self.conn.execute(
                    "DELETE FROM memos WHERE id = ?1 AND deleted_at IS NOT NULL",
                    params![id],
                )?;
                if changed == 0 {
                    bail!("ゴミ箱にこのメモはありません(完全削除はゴミ箱からのみ)");
                }
                // 個人メモのリマインダーも連動削除(ADR-0052 決定 6。共有メモは
                // 削除をローカルで知れないため、こちらの対象外)
                self.conn.execute(
                    "DELETE FROM reminders WHERE scope = 'personal' AND network = '' AND memo_id = ?1",
                    params![id],
                )?;
                Ok(MemoReply::Done)
            }
            MemoOp::EmptyTrash => {
                let tx = self.conn.transaction()?;
                tx.execute(
                    "DELETE FROM reminders WHERE scope = 'personal' AND network = ''
                     AND memo_id IN (SELECT id FROM memos WHERE deleted_at IS NOT NULL)",
                    [],
                )?;
                tx.execute("DELETE FROM memos WHERE deleted_at IS NOT NULL", [])?;
                tx.commit()?;
                Ok(MemoReply::Done)
            }
            MemoOp::FolderCreate { name } => self.folder_create(&name),
            MemoOp::FolderRename { id, name } => {
                let name = validate_folder_name(&name)?;
                let changed = self.conn.execute(
                    "UPDATE folders SET name = ?1 WHERE id = ?2",
                    params![name, id],
                )?;
                if changed == 0 {
                    bail!("フォルダーが見つかりません");
                }
                Ok(MemoReply::Done)
            }
            MemoOp::FolderDelete { id } => {
                let tx = self.conn.transaction()?;
                // 中のメモは消さず「フォルダーなし」へ移動する(要件 §6)
                tx.execute(
                    "UPDATE memos SET folder_id = NULL WHERE folder_id = ?1",
                    params![id],
                )?;
                let changed = tx.execute("DELETE FROM folders WHERE id = ?1", params![id])?;
                if changed == 0 {
                    bail!("フォルダーが見つかりません");
                }
                tx.commit()?;
                Ok(MemoReply::Done)
            }
            MemoOp::ReminderSet {
                scope,
                network,
                memo_id,
                remind_at,
                offset_minutes,
            } => {
                self.reminder_set(scope, &network, &memo_id, remind_at, offset_minutes)?;
                Ok(MemoReply::Done)
            }
            MemoOp::ReminderClear {
                scope,
                network,
                memo_id,
                remind_at,
            } => {
                self.reminder_clear(scope, &network, &memo_id, remind_at)?;
                Ok(MemoReply::Done)
            }
            MemoOp::ReminderList => Ok(MemoReply::Reminders {
                reminders: self.reminders_all()?,
            }),
            MemoOp::ReminderTakeDue => Ok(MemoReply::Reminders {
                reminders: self.reminders_take_due(unix_ms() as u64)?,
            }),
        }
    }

    /// リマインダーの設定(ADR-0052 決定 6 / ADR-0055 決定 3)。過去の日時は
    /// 拒否。同一 `remind_at` への再設定は上書き(fired もクリアされる)。
    /// 異なる `remind_at` は追加(1 対象につき複数件、上限
    /// [`MAX_REMINDERS_PER_TARGET`])。
    pub fn reminder_set(
        &mut self,
        scope: ReminderScope,
        network: &str,
        memo_id: &str,
        remind_at_ms: u64,
        offset_minutes: Option<u32>,
    ) -> anyhow::Result<()> {
        if (remind_at_ms as i64) <= unix_ms() {
            bail!("過去の日時は指定できません");
        }
        // 上書き(同一 remind_at)はこの件数チェックの対象外
        let existing: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM reminders
             WHERE scope = ?1 AND network = ?2 AND memo_id = ?3 AND remind_at != ?4",
            params![
                reminder_scope_text(scope),
                network,
                memo_id,
                remind_at_ms as i64
            ],
            |row| row.get(0),
        )?;
        if existing as usize >= MAX_REMINDERS_PER_TARGET {
            bail!(
                "リマインダーの件数が上限({} 件)に達しています",
                MAX_REMINDERS_PER_TARGET
            );
        }
        self.conn.execute(
            "INSERT INTO reminders (scope, network, memo_id, remind_at, offset_minutes, fired)
             VALUES (?1, ?2, ?3, ?4, ?5, 0)
             ON CONFLICT(scope, network, memo_id, remind_at)
             DO UPDATE SET offset_minutes = excluded.offset_minutes, fired = 0",
            params![
                reminder_scope_text(scope),
                network,
                memo_id,
                remind_at_ms as i64,
                offset_minutes,
            ],
        )?;
        Ok(())
    }

    /// リマインダーの解除。無くても失敗にしない。`remind_at` を省略すると
    /// その対象の全件を削除する(ADR-0055 決定 3)。
    pub fn reminder_clear(
        &mut self,
        scope: ReminderScope,
        network: &str,
        memo_id: &str,
        remind_at: Option<u64>,
    ) -> anyhow::Result<()> {
        match remind_at {
            Some(at) => {
                self.conn.execute(
                    "DELETE FROM reminders
                     WHERE scope = ?1 AND network = ?2 AND memo_id = ?3 AND remind_at = ?4",
                    params![reminder_scope_text(scope), network, memo_id, at as i64],
                )?;
            }
            None => {
                self.conn.execute(
                    "DELETE FROM reminders WHERE scope = ?1 AND network = ?2 AND memo_id = ?3",
                    params![reminder_scope_text(scope), network, memo_id],
                )?;
            }
        }
        Ok(())
    }

    /// 指定した対象の未発火のリマインダー全件(発火時刻の早い順、ADR-0055
    /// 決定 3。1 対象で複数件になり得る)。
    pub fn reminders_for(
        &self,
        scope: ReminderScope,
        network: &str,
        memo_id: &str,
    ) -> anyhow::Result<Vec<MemoReminder>> {
        let mut stmt = self.conn.prepare(
            "SELECT scope, network, memo_id, remind_at, offset_minutes FROM reminders
             WHERE scope = ?1 AND network = ?2 AND memo_id = ?3 AND fired = 0
             ORDER BY remind_at ASC",
        )?;
        let rows = stmt
            .query_map(
                params![reminder_scope_text(scope), network, memo_id],
                row_to_reminder,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// 未発火のリマインダー全件(発火時刻の早い順)。
    pub fn reminders_all(&self) -> anyhow::Result<Vec<MemoReminder>> {
        let mut stmt = self.conn.prepare(
            "SELECT scope, network, memo_id, remind_at, offset_minutes FROM reminders
             WHERE fired = 0 ORDER BY remind_at ASC",
        )?;
        let rows = stmt
            .query_map([], row_to_reminder)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// 発火時刻(`now_ms`)を過ぎた未発火のリマインダーを取り出し、fired にする。
    pub fn reminders_take_due(&mut self, now_ms: u64) -> anyhow::Result<Vec<MemoReminder>> {
        let tx = self.conn.transaction()?;
        let mut stmt = tx.prepare(
            "SELECT scope, network, memo_id, remind_at, offset_minutes FROM reminders
             WHERE fired = 0 AND remind_at <= ?1 ORDER BY remind_at ASC",
        )?;
        let due: Vec<MemoReminder> = stmt
            .query_map(params![now_ms as i64], row_to_reminder)?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        tx.execute(
            "UPDATE reminders SET fired = 1 WHERE fired = 0 AND remind_at <= ?1",
            params![now_ms as i64],
        )?;
        tx.commit()?;
        Ok(due)
    }

    fn get(&self, id: &str) -> anyhow::Result<MemoDetail> {
        let mut memo = self
            .conn
            .query_row(
                "SELECT id, title, body, folder_id, pinned, archived, created_at, updated_at,
                        deleted_at
                 FROM memos WHERE id = ?1",
                params![id],
                row_to_detail,
            )
            .optional()?
            .context("メモが見つかりません(削除された可能性があります)")?;
        memo.tags = self.tags_of(id)?;
        Ok(memo)
    }

    fn tags_of(&self, id: &str) -> anyhow::Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT tag FROM memo_tags WHERE memo_id = ?1 ORDER BY tag")?;
        let tags = stmt
            .query_map(params![id], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(tags)
    }

    /// メモ間リンク `[[タイトル]]`(ADR-0052 決定 2)の解決。タイトル →
    /// memo_id(見つかったものだけ。ゴミ箱除外、複数一致は updated_at 最新)。
    pub fn resolve_titles(&self, titles: &[String]) -> anyhow::Result<HashMap<String, String>> {
        let mut seen = HashSet::new();
        let mut out = HashMap::new();
        for title in titles {
            if title.is_empty() || !seen.insert(title.clone()) {
                continue;
            }
            let id: Option<String> = self
                .conn
                .query_row(
                    "SELECT id FROM memos WHERE title = ?1 AND deleted_at IS NULL
                     ORDER BY updated_at DESC LIMIT 1",
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

    /// バックリンク: 本文に `[[<このメモのタイトル>]]` をリテラルとして含む
    /// メモの一覧(自分自身・ゴミ箱は除く)。タイトルが空なら対象外。
    pub fn backlinks(&self, id: &str) -> anyhow::Result<Vec<MemoSummary>> {
        let memo = self.get(id)?;
        if memo.title.is_empty() {
            return Ok(Vec::new());
        }
        let pattern = format!("%[[{}]]%", escape_like(&memo.title));
        let mut stmt = self.conn.prepare(
            "SELECT id, title, body, folder_id, pinned, archived, created_at, updated_at,
                    deleted_at
             FROM memos
             WHERE deleted_at IS NULL AND id != ?1 AND body LIKE ?2 ESCAPE '\\'
             ORDER BY updated_at DESC",
        )?;
        let rows = stmt
            .query_map(params![id, pattern], row_to_detail)?
            .collect::<Result<Vec<_>, _>>()?;
        let mut out = Vec::new();
        for detail in rows {
            let tags = self.tags_of(&detail.id)?;
            let (done, total) = checklist_progress(&detail.body);
            out.push(MemoSummary {
                excerpt: excerpt(&detail.body, EXCERPT_CHARS),
                tags,
                checklist_done: done,
                checklist_total: total,
                id: detail.id,
                title: detail.title,
                folder_id: detail.folder_id,
                pinned: detail.pinned,
                archived: detail.archived,
                created_at: detail.created_at,
                updated_at: detail.updated_at,
                deleted_at: detail.deleted_at,
            });
        }
        Ok(out)
    }

    fn create(
        &mut self,
        title: &str,
        body: &str,
        folder_id: Option<&str>,
        tags: &[String],
    ) -> anyhow::Result<MemoReply> {
        validate_text(title, body)?;
        let tags = normalize_tags(tags)?;
        let tx = self.conn.transaction()?;
        if let Some(folder) = folder_id {
            ensure_folder_exists(&tx, folder)?;
        }
        let id: String = tx.query_row("SELECT lower(hex(randomblob(8)))", [], |row| row.get(0))?;
        let now = unix_ms();
        tx.execute(
            "INSERT INTO memos (id, title, body, folder_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![id, title, body, folder_id, now],
        )?;
        for tag in &tags {
            tx.execute(
                "INSERT OR IGNORE INTO memo_tags (memo_id, tag) VALUES (?1, ?2)",
                params![id, tag],
            )?;
        }
        tx.commit()?;
        Ok(MemoReply::Memo {
            memo: self.get(&id)?,
        })
    }

    fn update(&mut self, id: &str, patch: MemoPatch) -> anyhow::Result<MemoReply> {
        if let Some(title) = &patch.title {
            validate_text(title, "")?;
        }
        if let Some(body) = &patch.body {
            validate_text("", body)?;
        }
        let tags = patch.tags.as_deref().map(normalize_tags).transpose()?;
        let tx = self.conn.transaction()?;
        let exists: bool = tx
            .query_row("SELECT 1 FROM memos WHERE id = ?1", params![id], |_| Ok(()))
            .optional()?
            .is_some();
        if !exists {
            bail!("メモが見つかりません(削除された可能性があります)");
        }
        // 本文・タイトルの変更だけ updated_at を進める(ピン留めやフォルダー
        // 移動で一覧の並びが変わらないように)
        let touches_content = patch.title.is_some() || patch.body.is_some();
        if let Some(title) = &patch.title {
            tx.execute(
                "UPDATE memos SET title = ?1 WHERE id = ?2",
                params![title, id],
            )?;
        }
        if let Some(body) = &patch.body {
            tx.execute(
                "UPDATE memos SET body = ?1 WHERE id = ?2",
                params![body, id],
            )?;
        }
        if let Some(folder) = &patch.folder {
            if let Some(folder_id) = &folder.id {
                ensure_folder_exists(&tx, folder_id)?;
            }
            tx.execute(
                "UPDATE memos SET folder_id = ?1 WHERE id = ?2",
                params![folder.id, id],
            )?;
        }
        if let Some(pinned) = patch.pinned {
            tx.execute(
                "UPDATE memos SET pinned = ?1 WHERE id = ?2",
                params![pinned as i64, id],
            )?;
        }
        if let Some(archived) = patch.archived {
            tx.execute(
                "UPDATE memos SET archived = ?1 WHERE id = ?2",
                params![archived as i64, id],
            )?;
        }
        if let Some(tags) = &tags {
            tx.execute("DELETE FROM memo_tags WHERE memo_id = ?1", params![id])?;
            for tag in tags {
                tx.execute(
                    "INSERT OR IGNORE INTO memo_tags (memo_id, tag) VALUES (?1, ?2)",
                    params![id, tag],
                )?;
            }
        }
        if touches_content {
            tx.execute(
                "UPDATE memos SET updated_at = ?1 WHERE id = ?2",
                params![unix_ms(), id],
            )?;
        }
        tx.commit()?;
        Ok(MemoReply::Memo {
            memo: self.get(id)?,
        })
    }

    fn duplicate(&mut self, id: &str) -> anyhow::Result<MemoReply> {
        let source = self.get(id)?;
        let title = if source.title.is_empty() {
            "無題のコピー".to_string()
        } else {
            format!("{} のコピー", source.title)
        };
        // タイトル上限を超えないよう切り詰める(コピー接尾辞を優先)
        let title: String = title.chars().take(MAX_TITLE_CHARS).collect();
        self.create(
            &title,
            &source.body,
            source.folder_id.as_deref(),
            &source.tags,
        )
    }

    fn folder_create(&mut self, name: &str) -> anyhow::Result<MemoReply> {
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
        Ok(MemoReply::Folder {
            folder: MemoFolder {
                id,
                name,
                memo_count: 0,
            },
        })
    }

    fn list(&self, query: &MemoQuery) -> anyhow::Result<MemoReply> {
        let mut sql = String::from(
            "SELECT id, title, body, folder_id, pinned, archived, created_at, updated_at,
                    deleted_at
             FROM memos m WHERE ",
        );
        let mut clauses: Vec<String> = Vec::new();
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        match query.scope {
            MemoScope::Active => {
                clauses.push("m.deleted_at IS NULL AND m.archived = 0".to_string())
            }
            MemoScope::Archived => {
                clauses.push("m.deleted_at IS NULL AND m.archived = 1".to_string())
            }
            MemoScope::Trash => clauses.push("m.deleted_at IS NOT NULL".to_string()),
        }
        if let Some(folder) = &query.folder_id {
            args.push(Box::new(folder.clone()));
            clauses.push(format!("m.folder_id = ?{}", args.len()));
        }
        if let Some(tag) = &query.tag {
            args.push(Box::new(tag.clone()));
            clauses.push(format!(
                "m.id IN (SELECT memo_id FROM memo_tags WHERE tag = ?{})",
                args.len()
            ));
        }
        if let Some(search) = query
            .search
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            // 索引・列側は kana_fold 済みなので、検索語も折り畳んで比較する
            // (ひらがな/カタカナ同一視)
            let search = kana_fold(search);
            if search.chars().count() >= 3 {
                // FTS5 trigram: 検索語をフレーズ(引用符)として渡す。内部の
                // 引用符は 2 重化してエスケープする
                args.push(Box::new(format!("\"{}\"", search.replace('"', "\"\""))));
                clauses.push(format!(
                    "m.rowid IN (SELECT rowid FROM memo_fts WHERE memo_fts MATCH ?{})",
                    args.len()
                ));
            } else {
                // trigram は 3 文字未満に一致しないため LIKE で代替する
                let pattern = format!("%{}%", escape_like(&search));
                args.push(Box::new(pattern));
                let n = args.len();
                clauses.push(format!(
                    "(kana_fold(m.title) LIKE ?{n} ESCAPE '\\' OR kana_fold(m.body) LIKE ?{n} ESCAPE '\\')"
                ));
            }
        }
        sql.push_str(&clauses.join(" AND "));
        sql.push_str(" ORDER BY m.pinned DESC, ");
        sql.push_str(match (query.scope, query.sort) {
            // ゴミ箱は捨てた順で見たいはず(復元対象を探しやすい)
            (MemoScope::Trash, _) => "m.deleted_at DESC",
            (_, MemoSort::Updated) => "m.updated_at DESC",
            (_, MemoSort::Created) => "m.created_at DESC",
            (_, MemoSort::Title) => "m.title COLLATE NOCASE ASC, m.updated_at DESC",
        });

        let mut stmt = self.conn.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> = args.iter().map(AsRef::as_ref).collect();
        let rows = stmt.query_map(params.as_slice(), row_to_detail)?;

        // タグは全メモ分を一括で引いて突き合わせる(1 件ずつ引かない)
        let mut tag_map: std::collections::HashMap<String, Vec<String>> = Default::default();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT memo_id, tag FROM memo_tags ORDER BY tag")?;
            let pairs = stmt.query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for pair in pairs {
                let (memo_id, tag) = pair?;
                tag_map.entry(memo_id).or_default().push(tag);
            }
        }

        let mut memos = Vec::new();
        for row in rows {
            let detail = row?;
            let (done, total) = checklist_progress(&detail.body);
            memos.push(MemoSummary {
                excerpt: excerpt(&detail.body, EXCERPT_CHARS),
                tags: tag_map.remove(&detail.id).unwrap_or_default(),
                checklist_done: done,
                checklist_total: total,
                id: detail.id,
                title: detail.title,
                folder_id: detail.folder_id,
                pinned: detail.pinned,
                archived: detail.archived,
                created_at: detail.created_at,
                updated_at: detail.updated_at,
                deleted_at: detail.deleted_at,
            });
        }

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

        let mut stmt = self.conn.prepare(
            "SELECT t.tag, COUNT(*)
             FROM memo_tags t JOIN memos m ON m.id = t.memo_id
             WHERE m.deleted_at IS NULL
             GROUP BY t.tag ORDER BY t.tag",
        )?;
        let tags = stmt
            .query_map([], |row| {
                Ok(MemoTagCount {
                    tag: row.get(0)?,
                    count: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(MemoReply::Memos {
            memos,
            folders,
            tags,
        })
    }
}

fn row_to_detail(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoDetail> {
    Ok(MemoDetail {
        id: row.get(0)?,
        title: row.get(1)?,
        body: row.get(2)?,
        folder_id: row.get(3)?,
        pinned: row.get::<_, i64>(4)? != 0,
        archived: row.get::<_, i64>(5)? != 0,
        created_at: row.get::<_, i64>(6)? as u64,
        updated_at: row.get::<_, i64>(7)? as u64,
        deleted_at: row.get::<_, Option<i64>>(8)?.map(|v| v as u64),
        tags: Vec::new(),
    })
}

fn row_to_reminder(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoReminder> {
    let scope_text: String = row.get(0)?;
    Ok(MemoReminder {
        scope: reminder_scope_from_text(&scope_text),
        network: row.get(1)?,
        memo_id: row.get(2)?,
        remind_at: row.get::<_, i64>(3)? as u64,
        offset_minutes: row.get::<_, Option<i64>>(4)?.map(|v| v as u32),
    })
}

fn reminder_scope_text(scope: ReminderScope) -> &'static str {
    match scope {
        ReminderScope::Personal => "personal",
        ReminderScope::Shared => "shared",
        ReminderScope::Schedule => "schedule",
    }
}

fn reminder_scope_from_text(text: &str) -> ReminderScope {
    match text {
        "shared" => ReminderScope::Shared,
        "schedule" => ReminderScope::Schedule,
        _ => ReminderScope::Personal,
    }
}

fn ensure_folder_exists(conn: &Connection, folder_id: &str) -> anyhow::Result<()> {
    let exists: bool = conn
        .query_row(
            "SELECT 1 FROM folders WHERE id = ?1",
            params![folder_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !exists {
        bail!("指定のフォルダーが見つかりません(削除された可能性があります)");
    }
    Ok(())
}

/// タイトル・本文の上限(要件 §14)。超過は黙って失敗させず理由を返す。
/// 個人メモ(固定上限)から使う。共有メモは [`validate_title`] +
/// [`validate_body_bytes`](上限はホスト設定可)を個別に呼ぶ。
fn validate_text(title: &str, body: &str) -> anyhow::Result<()> {
    validate_title(title)?;
    validate_body_bytes(body, MAX_BODY_BYTES)
}

/// タイトルの上限(文字数)。個人・共有共通。
fn validate_title(title: &str) -> anyhow::Result<()> {
    if title.chars().count() > MAX_TITLE_CHARS {
        bail!("タイトルが長すぎます(上限 {MAX_TITLE_CHARS} 文字)");
    }
    Ok(())
}

/// 本文のバイト数上限(UTF-8)。共有メモはホスト設定可の上限を渡す。
fn validate_body_bytes(body: &str, max_bytes: usize) -> anyhow::Result<()> {
    if body.len() > max_bytes {
        bail!(
            "本文が大きすぎます(上限 {} KiB)。メモを分割してください",
            max_bytes / 1024
        );
    }
    Ok(())
}

/// 2 つの本文の行単位差分(unified diff 風)。LCS(最長共通部分列)で計算する。
///
/// 安全弁: 共通の接頭辞・接尾辞の行を先に取り除き、残りの行数の積が
/// 4,000,000 を超える場合は LCS を諦め、残り全体を削除+追加として返す
/// (巨大な本文同士の比較でメモリ・CPU を使い切らないため)。
pub fn diff_lines(old: &str, new: &str) -> Vec<DiffLine> {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    let max_common = old_lines.len().min(new_lines.len());
    let mut prefix = 0usize;
    while prefix < max_common && old_lines[prefix] == new_lines[prefix] {
        prefix += 1;
    }
    let mut suffix = 0usize;
    while suffix < max_common - prefix
        && old_lines[old_lines.len() - 1 - suffix] == new_lines[new_lines.len() - 1 - suffix]
    {
        suffix += 1;
    }

    let old_mid = &old_lines[prefix..old_lines.len() - suffix];
    let new_mid = &new_lines[prefix..new_lines.len() - suffix];

    let mut out = Vec::with_capacity(old_lines.len() + new_lines.len());
    for line in &old_lines[..prefix] {
        out.push(DiffLine {
            kind: DiffLineKind::Same,
            text: (*line).to_string(),
        });
    }

    let m = old_mid.len();
    let n = new_mid.len();
    if (m as u64) * (n as u64) > 4_000_000 {
        // 安全弁: LCS を諦め、丸ごと削除+追加として扱う
        for line in old_mid {
            out.push(DiffLine {
                kind: DiffLineKind::Removed,
                text: (*line).to_string(),
            });
        }
        for line in new_mid {
            out.push(DiffLine {
                kind: DiffLineKind::Added,
                text: (*line).to_string(),
            });
        }
    } else {
        out.extend(lcs_diff(old_mid, new_mid));
    }

    for line in &old_lines[old_lines.len() - suffix..] {
        out.push(DiffLine {
            kind: DiffLineKind::Same,
            text: (*line).to_string(),
        });
    }
    out
}

/// `old`/`new` の LCS を DP で求め、Same/Removed/Added の列へ復元する。
/// 対応が付かない区間は Removed → Added の順で並べる(unified diff 風)。
fn lcs_diff(old: &[&str], new: &[&str]) -> Vec<DiffLine> {
    let m = old.len();
    let n = new.len();
    // dp[i][j] = old[i..] と new[j..] の LCS 長。呼び出し元で m*n の上限を
    // 検査済みなので、ここでは素朴な O(m*n) 表で構わない
    let width = n + 1;
    let mut dp = vec![0u32; (m + 1) * width];
    for i in (0..m).rev() {
        for j in (0..n).rev() {
            dp[i * width + j] = if old[i] == new[j] {
                dp[(i + 1) * width + (j + 1)] + 1
            } else {
                dp[(i + 1) * width + j].max(dp[i * width + (j + 1)])
            };
        }
    }

    let mut out = Vec::with_capacity(m + n);
    let (mut i, mut j) = (0usize, 0usize);
    while i < m && j < n {
        if old[i] == new[j] {
            out.push(DiffLine {
                kind: DiffLineKind::Same,
                text: old[i].to_string(),
            });
            i += 1;
            j += 1;
        } else if dp[(i + 1) * width + j] >= dp[i * width + (j + 1)] {
            out.push(DiffLine {
                kind: DiffLineKind::Removed,
                text: old[i].to_string(),
            });
            i += 1;
        } else {
            out.push(DiffLine {
                kind: DiffLineKind::Added,
                text: new[j].to_string(),
            });
            j += 1;
        }
    }
    while i < m {
        out.push(DiffLine {
            kind: DiffLineKind::Removed,
            text: old[i].to_string(),
        });
        i += 1;
    }
    while j < n {
        out.push(DiffLine {
            kind: DiffLineKind::Added,
            text: new[j].to_string(),
        });
        j += 1;
    }
    out
}

fn validate_folder_name(name: &str) -> anyhow::Result<String> {
    let name = name.trim();
    if name.is_empty() {
        bail!("フォルダー名を入力してください");
    }
    if name.chars().count() > 60 {
        bail!("フォルダー名が長すぎます(上限 60 文字)");
    }
    Ok(name.to_string())
}

/// タグの正規化: 前後空白を落とし、空・重複を除き、上限を検査する。
fn normalize_tags(tags: &[String]) -> anyhow::Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for tag in tags {
        let tag = tag.trim();
        if tag.is_empty() || out.iter().any(|t| t == tag) {
            continue;
        }
        if tag.chars().count() > MAX_TAG_CHARS {
            bail!("タグが長すぎます(上限 {MAX_TAG_CHARS} 文字)");
        }
        out.push(tag.to_string());
    }
    if out.len() > MAX_TAGS_PER_MEMO {
        bail!("タグが多すぎます(1 メモにつき上限 {MAX_TAGS_PER_MEMO} 個)");
    }
    Ok(out)
}

/// 検索用のかな折り畳み: カタカナ(全角)をひらがなへ寄せる。
/// 索引(トリガー経由)と検索語の両方に同じ変換をかけることで、
/// 「めんて」でも「メンテ」でも同じメモに当たる。
pub fn kana_fold(text: &str) -> String {
    text.chars()
        .map(|c| {
            let code = c as u32;
            // U+30A1 ァ 〜 U+30F6 ヶ → 0x60 引くと対応するひらがな
            if (0x30A1..=0x30F6).contains(&code) {
                char::from_u32(code - 0x60).unwrap_or(c)
            } else {
                c
            }
        })
        .collect()
}

/// LIKE のメタ文字(`% _ \`)をエスケープする(`ESCAPE '\\'` と対で使う)。
/// 検索・メモ間リンクのバックリンク探索(本文中の `[[タイトル]]` の
/// リテラル一致)の両方から使う。
pub fn escape_like(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// `kana_fold` を SQL 関数として登録する(トリガーと LIKE 代替が使う)。
/// 接続ごとに必要なので open() で必ず呼ぶ。
fn register_kana_fold(conn: &Connection) -> anyhow::Result<()> {
    use rusqlite::functions::FunctionFlags;
    conn.create_scalar_function(
        "kana_fold",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let text: String = ctx.get(0)?;
            Ok(kana_fold(&text))
        },
    )?;
    Ok(())
}

/// SQLite の INTEGER(i64)に合わせる。core の型(u64)へは読み出し時に変換する。
fn unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use peercove_core::memo::MemoFolderTarget;

    fn open_temp() -> (tempfile::TempDir, MemoStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = MemoStore::open(&dir.path().join("memos.db")).unwrap();
        (dir, store)
    }

    fn create(store: &mut MemoStore, title: &str, body: &str) -> MemoDetail {
        match store
            .apply(MemoOp::Create {
                title: title.to_string(),
                body: body.to_string(),
                folder_id: None,
                tags: vec![],
            })
            .unwrap()
        {
            MemoReply::Memo { memo } => memo,
            other => panic!("Memo を期待: {other:?}"),
        }
    }

    fn list(store: &mut MemoStore, query: MemoQuery) -> Vec<MemoSummary> {
        match store.apply(MemoOp::List { query }).unwrap() {
            MemoReply::Memos { memos, .. } => memos,
            other => panic!("Memos を期待: {other:?}"),
        }
    }

    #[test]
    fn create_get_update_roundtrip() {
        let (_dir, mut store) = open_temp();
        let memo = create(&mut store, "買い物", "- [ ] 牛乳\n- [x] 卵\n");
        assert_eq!(memo.title, "買い物");
        assert!(memo.created_at > 0);

        let updated = store
            .apply(MemoOp::Update {
                id: memo.id.clone(),
                patch: MemoPatch {
                    body: Some("- [x] 牛乳\n- [x] 卵\n".to_string()),
                    tags: Some(vec![
                        "家事".to_string(),
                        " 家事 ".to_string(),
                        "".to_string(),
                    ]),
                    pinned: Some(true),
                    ..Default::default()
                },
            })
            .unwrap();
        let MemoReply::Memo { memo: updated } = updated else {
            panic!("Memo を期待");
        };
        assert_eq!(updated.tags, vec!["家事"], "タグは trim + 重複除去される");
        assert!(updated.pinned);

        let memos = list(&mut store, MemoQuery::default());
        assert_eq!(memos.len(), 1);
        assert_eq!(memos[0].checklist_done, 2);
        assert_eq!(memos[0].checklist_total, 2);
    }

    #[test]
    fn pin_and_folder_move_do_not_touch_updated_at() {
        let (_dir, mut store) = open_temp();
        let memo = create(&mut store, "a", "b");
        let before = memo.updated_at;
        let MemoReply::Memo { memo } = store
            .apply(MemoOp::Update {
                id: memo.id,
                patch: MemoPatch {
                    pinned: Some(true),
                    folder: Some(MemoFolderTarget { id: None }),
                    ..Default::default()
                },
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(memo.updated_at, before);
    }

    #[test]
    fn trash_restore_delete_flow() {
        let (_dir, mut store) = open_temp();
        let memo = create(&mut store, "捨てる", "x");

        // ゴミ箱に入れる前の完全削除は拒否される
        assert!(store
            .apply(MemoOp::DeleteForever {
                id: memo.id.clone()
            })
            .is_err());

        store
            .apply(MemoOp::Trash {
                id: memo.id.clone(),
            })
            .unwrap();
        assert!(list(&mut store, MemoQuery::default()).is_empty());
        let trash = list(
            &mut store,
            MemoQuery {
                scope: MemoScope::Trash,
                ..Default::default()
            },
        );
        assert_eq!(trash.len(), 1);
        assert!(trash[0].deleted_at.is_some());

        store
            .apply(MemoOp::Restore {
                id: memo.id.clone(),
            })
            .unwrap();
        assert_eq!(list(&mut store, MemoQuery::default()).len(), 1);

        store
            .apply(MemoOp::Trash {
                id: memo.id.clone(),
            })
            .unwrap();
        store
            .apply(MemoOp::DeleteForever {
                id: memo.id.clone(),
            })
            .unwrap();
        assert!(list(
            &mut store,
            MemoQuery {
                scope: MemoScope::Trash,
                ..Default::default()
            }
        )
        .is_empty());
    }

    #[test]
    fn folders_move_and_delete_keeps_memos() {
        let (_dir, mut store) = open_temp();
        let MemoReply::Folder { folder } = store
            .apply(MemoOp::FolderCreate {
                name: " 仕事 ".to_string(),
            })
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(folder.name, "仕事");
        // 同名は拒否
        assert!(store
            .apply(MemoOp::FolderCreate {
                name: "仕事".to_string()
            })
            .is_err());

        let memo = create(&mut store, "議事録", "x");
        store
            .apply(MemoOp::Update {
                id: memo.id.clone(),
                patch: MemoPatch {
                    folder: Some(MemoFolderTarget {
                        id: Some(folder.id.clone()),
                    }),
                    ..Default::default()
                },
            })
            .unwrap();
        let memos = list(
            &mut store,
            MemoQuery {
                folder_id: Some(folder.id.clone()),
                ..Default::default()
            },
        );
        assert_eq!(memos.len(), 1);

        // 存在しないフォルダーへの移動は拒否
        assert!(store
            .apply(MemoOp::Update {
                id: memo.id.clone(),
                patch: MemoPatch {
                    folder: Some(MemoFolderTarget {
                        id: Some("ghost".to_string()),
                    }),
                    ..Default::default()
                },
            })
            .is_err());

        store
            .apply(MemoOp::FolderDelete {
                id: folder.id.clone(),
            })
            .unwrap();
        let memos = list(&mut store, MemoQuery::default());
        assert_eq!(memos.len(), 1, "フォルダー削除でメモは消えない");
        assert_eq!(memos[0].folder_id, None);
    }

    #[test]
    fn search_japanese_fts_and_short_like() {
        let (_dir, mut store) = open_temp();
        create(&mut store, "サーバー情報", "本番環境のメンテナンス手順\n");
        create(&mut store, "買い物リスト", "牛乳と卵を買う\n");

        // 3 文字以上 → FTS5 trigram(本文の部分一致)
        let hits = list(
            &mut store,
            MemoQuery {
                search: Some("メンテナンス".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "サーバー情報");

        // 2 文字 → LIKE 代替
        let hits = list(
            &mut store,
            MemoQuery {
                search: Some("牛乳".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "買い物リスト");

        // LIKE のワイルドカードはエスケープされる(% で全件ヒットしない)
        let hits = list(
            &mut store,
            MemoQuery {
                search: Some("%".to_string()),
                ..Default::default()
            },
        );
        assert!(hits.is_empty());

        // 更新後の FTS 追随(トリガー)
        let id = list(&mut store, MemoQuery::default())
            .into_iter()
            .find(|m| m.title == "買い物リスト")
            .unwrap()
            .id;
        store
            .apply(MemoOp::Update {
                id,
                patch: MemoPatch {
                    body: Some("豆腐を買う\n".to_string()),
                    ..Default::default()
                },
            })
            .unwrap();
        assert!(list(
            &mut store,
            MemoQuery {
                search: Some("牛乳と卵".to_string()),
                ..Default::default()
            }
        )
        .is_empty());
        assert_eq!(
            list(
                &mut store,
                MemoQuery {
                    search: Some("豆腐を買".to_string()),
                    ..Default::default()
                }
            )
            .len(),
            1
        );
    }

    /// ひらがな/カタカナを同一視する(2026-07-21 実機検証フィードバック)。
    #[test]
    fn search_is_kana_insensitive() {
        let (_dir, mut store) = open_temp();
        create(&mut store, "サーバー情報", "本番環境のメンテナンス手順\n");
        create(&mut store, "ひらがなめも", "らーめんの店を探す\n");

        // ひらがな → カタカナ本文(FTS 経路: 3 文字以上)
        let hits = list(
            &mut store,
            MemoQuery {
                search: Some("めんてなんす".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "サーバー情報");

        // カタカナ → ひらがな本文(FTS 経路)
        let hits = list(
            &mut store,
            MemoQuery {
                search: Some("ラーメン".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "ひらがなめも");

        // 2 文字(LIKE 経路)でも同一視される
        let hits = list(
            &mut store,
            MemoQuery {
                search: Some("メモ".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "ひらがなめも");

        assert_eq!(kana_fold("メンテナンス手順ヴヶ"), "めんてなんす手順ゔゖ");
    }

    /// v1(生テキスト索引)の DB を開くと v2 へ移行され、かな同一視が効く。
    #[test]
    fn v1_database_migrates_to_kana_folded_fts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memos.db");
        {
            let mut store = MemoStore::open(&path).unwrap();
            create(&mut store, "移行前", "メンテナンスの記録\n");
            // v1 相当に偽装(実際の v1 とはトリガー形が違うが、v2 移行が
            // 索引を作り直すことの確認には十分)
            store.conn.pragma_update(None, "user_version", 1).unwrap();
        }
        let mut store = MemoStore::open(&path).unwrap();
        let hits = list(
            &mut store,
            MemoQuery {
                search: Some("めんてなんす".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn body_size_limit_is_enforced() {
        let (_dir, mut store) = open_temp();
        let big = "a".repeat(MAX_BODY_BYTES + 1);
        let err = store
            .apply(MemoOp::Create {
                title: String::new(),
                body: big,
                folder_id: None,
                tags: vec![],
            })
            .unwrap_err();
        assert!(err.to_string().contains("本文が大きすぎます"), "{err}");
    }

    #[test]
    fn duplicate_copies_body_and_tags() {
        let (_dir, mut store) = open_temp();
        let memo = create(&mut store, "元", "本文");
        store
            .apply(MemoOp::Update {
                id: memo.id.clone(),
                patch: MemoPatch {
                    tags: Some(vec!["t1".to_string()]),
                    ..Default::default()
                },
            })
            .unwrap();
        let MemoReply::Memo { memo: copy } =
            store.apply(MemoOp::Duplicate { id: memo.id }).unwrap()
        else {
            panic!()
        };
        assert_eq!(copy.title, "元 のコピー");
        assert_eq!(copy.body, "本文");
        assert_eq!(copy.tags, vec!["t1"]);

        let memos = list(&mut store, MemoQuery::default());
        assert_eq!(memos.len(), 2);
    }

    #[test]
    fn reopen_persists_and_purges_expired_trash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memos.db");
        let id = {
            let mut store = MemoStore::open(&path).unwrap();
            let memo = create(&mut store, "残す", "x");
            let doomed = create(&mut store, "期限切れ", "y");
            store
                .apply(MemoOp::Trash {
                    id: doomed.id.clone(),
                })
                .unwrap();
            // 31 日前にゴミ箱入りしたことにする
            store
                .conn
                .execute(
                    "UPDATE memos SET deleted_at = ?1 WHERE id = ?2",
                    params![unix_ms() - 31 * 24 * 60 * 60 * 1000, doomed.id],
                )
                .unwrap();
            memo.id
        };
        let mut store = MemoStore::open(&path).unwrap();
        let memos = list(&mut store, MemoQuery::default());
        assert_eq!(memos.len(), 1);
        assert_eq!(memos[0].id, id);
        assert!(
            list(
                &mut store,
                MemoQuery {
                    scope: MemoScope::Trash,
                    ..Default::default()
                }
            )
            .is_empty(),
            "30 日を過ぎたゴミ箱は開いたときに完全削除される"
        );
    }

    /// メモ間リンク(ADR-0052 決定 2): タイトル解決・複数一致・ゴミ箱除外。
    #[test]
    fn resolve_titles_picks_newest_and_excludes_trash() {
        let (_dir, mut store) = open_temp();
        let old = create(&mut store, "重複", "古い方");
        std::thread::sleep(std::time::Duration::from_millis(2));
        let newest = create(&mut store, "重複", "新しい方");
        let trashed = create(&mut store, "捨てた", "x");
        store
            .apply(MemoOp::Trash {
                id: trashed.id.clone(),
            })
            .unwrap();

        let map = store
            .resolve_titles(&[
                "重複".to_string(),
                "重複".to_string(), // 重複指定は 1 回だけ処理される
                "捨てた".to_string(),
                "存在しない".to_string(),
                "".to_string(),
            ])
            .unwrap();
        assert_eq!(map.get("重複"), Some(&newest.id));
        assert_ne!(map.get("重複"), Some(&old.id));
        assert_eq!(map.len(), 1, "ゴミ箱・存在しない・空は含まれない");
    }

    /// バックリンクの LIKE エスケープ: タイトルに `%` を含んでいても、
    /// SQL ワイルドカードとして働かず厳密一致だけがヒットする。
    #[test]
    fn backlinks_escapes_like_metacharacters_in_title() {
        let (_dir, mut store) = open_temp();
        let target = create(&mut store, "50%割引", "本文");
        let genuine = create(&mut store, "案内", "[[50%割引]] を見てください");
        // エスケープが効いていなければ `%` がワイルドカードとなり、
        // これも誤って一致してしまう(ダミーメモ)
        let decoy = create(&mut store, "ダミー", "[[50XY割引]] は無関係");

        let backlinks = store.backlinks(&target.id).unwrap();
        let ids: Vec<&str> = backlinks.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec![genuine.id.as_str()]);
        assert!(!ids.contains(&decoy.id.as_str()));
    }

    /// バックリンク: 自分自身・ゴミ箱を除き、`[[タイトル]]` を含むメモだけ
    /// 返る。タイトルが空のメモはバックリンク対象外(自分が呼び出し先の場合)。
    #[test]
    fn backlinks_finds_literal_wikilinks_and_excludes_self_and_trash() {
        let (_dir, mut store) = open_temp();
        let target = create(&mut store, "サーバー情報", "本文");
        let linking = create(&mut store, "議事録", "参照: [[サーバー情報]] を見ること");
        let unrelated = create(&mut store, "無関係", "特に関係ない本文");
        let trashed_link = create(&mut store, "捨てた参照", "[[サーバー情報]] だが捨てた");
        store
            .apply(MemoOp::Trash {
                id: trashed_link.id.clone(),
            })
            .unwrap();
        // 自分自身の本文に自分へのリンクがあっても自分は含まれない
        store
            .apply(MemoOp::Update {
                id: target.id.clone(),
                patch: MemoPatch {
                    body: Some("[[サーバー情報]] 自己参照".to_string()),
                    ..Default::default()
                },
            })
            .unwrap();

        let backlinks = store.backlinks(&target.id).unwrap();
        assert_eq!(backlinks.len(), 1);
        assert_eq!(backlinks[0].id, linking.id);
        assert!(!backlinks.iter().any(|m| m.id == unrelated.id));
        assert!(!backlinks.iter().any(|m| m.id == trashed_link.id));

        // タイトルが空のメモに対するバックリンクは常に空
        let untitled = create(&mut store, "", "本文だけ");
        assert!(store.backlinks(&untitled.id).unwrap().is_empty());
    }

    fn texts(lines: &[DiffLine]) -> Vec<(DiffLineKind, &str)> {
        lines.iter().map(|l| (l.kind, l.text.as_str())).collect()
    }

    #[test]
    fn diff_lines_identical_is_all_same() {
        let body = "a\nb\nc";
        let out = diff_lines(body, body);
        assert!(out.iter().all(|l| l.kind == DiffLineKind::Same));
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn diff_lines_pure_addition() {
        let out = diff_lines("a\nb", "a\nb\nc");
        assert_eq!(
            texts(&out),
            vec![
                (DiffLineKind::Same, "a"),
                (DiffLineKind::Same, "b"),
                (DiffLineKind::Added, "c"),
            ]
        );
    }

    #[test]
    fn diff_lines_pure_removal() {
        let out = diff_lines("a\nb\nc", "a\nc");
        assert_eq!(
            texts(&out),
            vec![
                (DiffLineKind::Same, "a"),
                (DiffLineKind::Removed, "b"),
                (DiffLineKind::Same, "c"),
            ]
        );
    }

    #[test]
    fn diff_lines_replace_orders_removed_before_added() {
        let out = diff_lines("a\nold\nz", "a\nnew1\nnew2\nz");
        assert_eq!(
            texts(&out),
            vec![
                (DiffLineKind::Same, "a"),
                (DiffLineKind::Removed, "old"),
                (DiffLineKind::Added, "new1"),
                (DiffLineKind::Added, "new2"),
                (DiffLineKind::Same, "z"),
            ]
        );
    }

    /// リマインダー(ADR-0052 決定 6): 設定・上書き・解除・過去日時の拒否。
    #[test]
    fn reminder_set_overwrites_and_rejects_past() {
        let (_dir, mut store) = open_temp();
        let memo = create(&mut store, "買い物", "牛乳");
        let future = unix_ms() + 60_000;

        store
            .reminder_set(ReminderScope::Personal, "", &memo.id, future as u64, None)
            .unwrap();
        assert_eq!(
            store
                .reminders_for(ReminderScope::Personal, "", &memo.id)
                .unwrap()
                .into_iter()
                .map(|r| r.remind_at)
                .collect::<Vec<_>>(),
            vec![future as u64]
        );

        // 同一 remind_at への再設定は上書き(件数は増えない)
        store
            .reminder_set(
                ReminderScope::Personal,
                "",
                &memo.id,
                future as u64,
                Some(15),
            )
            .unwrap();
        let reminders = store
            .reminders_for(ReminderScope::Personal, "", &memo.id)
            .unwrap();
        assert_eq!(reminders.len(), 1, "同一 remind_at は上書き");
        assert_eq!(reminders[0].offset_minutes, Some(15));

        // 過去日時は拒否
        let err = store
            .reminder_set(ReminderScope::Personal, "", &memo.id, 1, None)
            .unwrap_err();
        assert!(err.to_string().contains("過去の日時"), "{err}");

        // 共有メモの自分用リマインダーは network で区別される(同じ memo_id でも別行)
        store
            .reminder_set(
                ReminderScope::Shared,
                "net.toml",
                &memo.id,
                future as u64,
                None,
            )
            .unwrap();
        assert_eq!(store.reminders_all().unwrap().len(), 2);

        store
            .reminder_clear(ReminderScope::Personal, "", &memo.id, None)
            .unwrap();
        assert!(store
            .reminders_for(ReminderScope::Personal, "", &memo.id)
            .unwrap()
            .is_empty());
        // 解除は無くても失敗しない
        store
            .reminder_clear(ReminderScope::Personal, "", &memo.id, None)
            .unwrap();
        assert_eq!(store.reminders_all().unwrap().len(), 1);
    }

    /// リマインダーの複数化(ADR-0055 決定 3): 1 対象に複数件設定でき、
    /// remind_at を指定した個別削除・全件削除がそれぞれ動く。
    #[test]
    fn reminder_multiple_per_target_and_targeted_clear() {
        let (_dir, mut store) = open_temp();
        let memo = create(&mut store, "会議", "資料を送る");
        let base = unix_ms() + 60_000;
        let at1 = base as u64;
        let at2 = (base + 1_000) as u64;
        let at3 = (base + 2_000) as u64;

        store
            .reminder_set(ReminderScope::Schedule, "net.toml", &memo.id, at1, Some(15))
            .unwrap();
        store
            .reminder_set(ReminderScope::Schedule, "net.toml", &memo.id, at2, Some(5))
            .unwrap();
        store
            .reminder_set(ReminderScope::Schedule, "net.toml", &memo.id, at3, None)
            .unwrap();
        let list = store
            .reminders_for(ReminderScope::Schedule, "net.toml", &memo.id)
            .unwrap();
        assert_eq!(list.len(), 3, "1 対象に複数件");
        assert_eq!(
            list.iter().map(|r| r.remind_at).collect::<Vec<_>>(),
            vec![at1, at2, at3],
            "remind_at 昇順"
        );

        // remind_at 指定で個別削除
        store
            .reminder_clear(ReminderScope::Schedule, "net.toml", &memo.id, Some(at2))
            .unwrap();
        let list = store
            .reminders_for(ReminderScope::Schedule, "net.toml", &memo.id)
            .unwrap();
        assert_eq!(
            list.iter().map(|r| r.remind_at).collect::<Vec<_>>(),
            vec![at1, at3]
        );

        // remind_at 省略で全件削除
        store
            .reminder_clear(ReminderScope::Schedule, "net.toml", &memo.id, None)
            .unwrap();
        assert!(store
            .reminders_for(ReminderScope::Schedule, "net.toml", &memo.id)
            .unwrap()
            .is_empty());
    }

    /// リマインダーの件数上限(ADR-0055 決定 3、1 対象あたり
    /// [`MAX_REMINDERS_PER_TARGET`] 件)。
    #[test]
    fn reminder_enforces_per_target_limit() {
        let (_dir, mut store) = open_temp();
        let memo = create(&mut store, "会議", "");
        let base = unix_ms() + 60_000;
        for i in 0..MAX_REMINDERS_PER_TARGET {
            store
                .reminder_set(
                    ReminderScope::Schedule,
                    "net.toml",
                    &memo.id,
                    (base + i as i64 * 1_000) as u64,
                    None,
                )
                .unwrap();
        }
        let err = store
            .reminder_set(
                ReminderScope::Schedule,
                "net.toml",
                &memo.id,
                (base + MAX_REMINDERS_PER_TARGET as i64 * 1_000) as u64,
                None,
            )
            .unwrap_err();
        assert!(err.to_string().contains("上限"), "{err}");
    }

    /// リマインダー: 発火時刻の到来で take_due が拾い、fired 済みは一覧から消える。
    #[test]
    fn reminder_take_due_fires_once() {
        let (_dir, mut store) = open_temp();
        let memo = create(&mut store, "会議", "資料を送る");
        let due_at = unix_ms() + 100;
        store
            .reminder_set(ReminderScope::Personal, "", &memo.id, due_at as u64, None)
            .unwrap();

        // まだ到来していない
        assert!(store
            .reminders_take_due(due_at as u64 - 1)
            .unwrap()
            .is_empty());
        assert_eq!(store.reminders_all().unwrap().len(), 1);

        // 到来: 1 度だけ取れる(fired 済みは以後 all にも take_due にも出ない)
        let due = store.reminders_take_due(due_at as u64).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].memo_id, memo.id);
        assert!(store.reminders_all().unwrap().is_empty());
        assert!(store
            .reminders_take_due(due_at as u64 + 1_000)
            .unwrap()
            .is_empty());
    }

    /// 個人メモの完全削除・ゴミ箱を空にするでリマインダーも連動削除される。
    #[test]
    fn reminder_cascades_on_permanent_delete() {
        let (_dir, mut store) = open_temp();
        let memo = create(&mut store, "捨てる", "x");
        let future = (unix_ms() + 60_000) as u64;
        store
            .reminder_set(ReminderScope::Personal, "", &memo.id, future, None)
            .unwrap();
        store
            .apply(MemoOp::Trash {
                id: memo.id.clone(),
            })
            .unwrap();
        store
            .apply(MemoOp::DeleteForever {
                id: memo.id.clone(),
            })
            .unwrap();
        assert!(store.reminders_all().unwrap().is_empty());

        // EmptyTrash 経由でも連動削除される
        let memo2 = create(&mut store, "捨てる2", "y");
        store
            .reminder_set(ReminderScope::Personal, "", &memo2.id, future, None)
            .unwrap();
        store.apply(MemoOp::Trash { id: memo2.id }).unwrap();
        store.apply(MemoOp::EmptyTrash).unwrap();
        assert!(store.reminders_all().unwrap().is_empty());
    }

    /// v2(reminders 表が無い)の DB を開くと v4 へ移行され、リマインダーが使える。
    #[test]
    fn v2_database_migrates_to_reminders_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memos.db");
        let memo_id = {
            let mut store = MemoStore::open(&path).unwrap();
            let memo = create(&mut store, "移行前", "本文");
            store.conn.pragma_update(None, "user_version", 2).unwrap();
            memo.id
        };
        let mut store = MemoStore::open(&path).unwrap();
        let future = (unix_ms() + 60_000) as u64;
        store
            .reminder_set(ReminderScope::Personal, "", &memo_id, future, None)
            .unwrap();
        assert_eq!(store.reminders_all().unwrap().len(), 1);
    }

    /// v3(reminders 表はあるが主キーに remind_at を含まない旧形式)から
    /// v4 への移行で既存行が引き継がれる(ADR-0055 決定 3)。
    #[test]
    fn v3_database_migrates_reminders_primary_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memos.db");
        let future = (unix_ms() + 60_000) as u64;
        let memo_id = {
            let mut store = MemoStore::open(&path).unwrap();
            let memo = create(&mut store, "移行前3", "本文");
            store
                .reminder_set(ReminderScope::Personal, "", &memo.id, future, None)
                .unwrap();
            // v4 移行前(旧形式)まで巻き戻す
            store.conn.pragma_update(None, "user_version", 3).unwrap();
            memo.id
        };
        let mut store = MemoStore::open(&path).unwrap();
        let list = store.reminders_all().unwrap();
        assert_eq!(list.len(), 1, "既存行が引き継がれる");
        assert_eq!(list[0].memo_id, memo_id);
        assert_eq!(list[0].remind_at, future);

        // 移行後は 1 対象に複数件設定できる(新しい主キーが効いている)
        store
            .reminder_set(ReminderScope::Personal, "", &memo_id, future + 1_000, None)
            .unwrap();
        assert_eq!(store.reminders_all().unwrap().len(), 2);
    }

    /// 安全弁: 共通の接頭辞・接尾辞を除いた行数の積が閾値を超えると
    /// LCS を諦め、丸ごと削除+追加になる。
    #[test]
    fn diff_lines_huge_input_falls_back_to_whole_replace() {
        // 接頭辞・接尾辞を作らないよう、全行を一意にする
        let old: String = (0..2500)
            .map(|i| format!("old-{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let new: String = (0..2500)
            .map(|i| format!("new-{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let out = diff_lines(&old, &new);
        assert_eq!(out.len(), 5000);
        assert!(out[..2500].iter().all(|l| l.kind == DiffLineKind::Removed));
        assert!(out[2500..].iter().all(|l| l.kind == DiffLineKind::Added));
    }
}
