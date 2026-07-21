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

use std::path::Path;

use anyhow::{bail, Context};
use peercove_core::memo::{
    checklist_progress, excerpt, MemoDetail, MemoFolder, MemoOp, MemoPatch, MemoQuery, MemoReply,
    MemoScope, MemoSort, MemoSummary, MemoTagCount, EXCERPT_CHARS, MAX_BODY_BYTES, MAX_TITLE_CHARS,
    TRASH_RETENTION_DAYS,
};
use rusqlite::{params, Connection, OptionalExtension};

/// スキーマ世代。互換性のない変更で上げ、`migrate` に移行を足す。
/// - 2: FTS をかな折り畳み済みテキストの通常表に変更(ひらがな/カタカナ同一視)
const SCHEMA_VERSION: i64 = 2;

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
                Ok(MemoReply::Done)
            }
            MemoOp::EmptyTrash => {
                self.conn
                    .execute("DELETE FROM memos WHERE deleted_at IS NOT NULL", [])?;
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
        }
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
fn validate_text(title: &str, body: &str) -> anyhow::Result<()> {
    if title.chars().count() > MAX_TITLE_CHARS {
        bail!("タイトルが長すぎます(上限 {MAX_TITLE_CHARS} 文字)");
    }
    if body.len() > MAX_BODY_BYTES {
        bail!(
            "本文が大きすぎます(上限 {} KiB)。メモを分割してください",
            MAX_BODY_BYTES / 1024
        );
    }
    Ok(())
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
}
