//! Full-text search over the warm-tier memory store (SQLite backend).
//!
//! Searches the `warm_memory` table (warm.db, owned by `memory_persistence.rs`)
//! across sessions. The primary index is an FTS5 external-content virtual
//! table over `warm_memory`; a `LIKE '%...%'` substring fallback covers the
//! cases FTS5 cannot serve:
//!
//! - **CJK queries.** The bundled SQLite amalgamation (libsqlite3-sys 0.38.x,
//!   SQLite 3.53.2) is compiled with `-DSQLITE_ENABLE_FTS5`, but registers
//!   only the `unicode61`, `ascii` and `trigram` tokenizers — there is no
//!   `cjk` / `cjk_unicode61` tokenizer (verified against the vendored
//!   `sqlite3.c` and by the `fts5_is_compiled_into_the_bundled_sqlite_build`
//!   test). `unicode61` folds a run of CJK ideographs into a single token, so
//!   a two-character query would only match token-initial text. Queries
//!   containing CJK therefore run the `LIKE` substring path as a complement.
//! - **No FTS5 at all.** If the SQLite build lacks FTS5, every query goes
//!   through the `LIKE` path. Same public API either way.

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

/// Name of the FTS5 external-content index over `warm_memory`.
const FTS_TABLE: &str = "warm_memory_fts";

/// Schema of the table this module indexes (mirrors `memory_persistence.rs`).
/// The searcher never creates it — it indexes whatever the persistence layer
/// created, and returns empty results until that table exists.
const WARM_MEMORY_TABLE: &str = "warm_memory";

/// A single full-text hit against the warm memory store.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MemoryHit {
    /// Session the memory belongs to (`warm_memory.session_id`), if any.
    pub session_id: Option<String>,
    /// Semantic class label (`warm_memory.class` — the table has no `role`
    /// column; `class` is its closest analogue, e.g. "episodic", "semantic").
    pub role: String,
    /// Matching memory content.
    pub content: String,
    /// Relevance score, higher is better. FTS5 hits carry `-bm25(...)` (so a
    /// better match is a larger positive value); `LIKE`-only hits are scored
    /// `0.0` and sort after every FTS hit.
    pub score: f64,
}

/// Full-text search over the warm-tier SQLite memory store.
pub struct MemorySearcher {
    conn: Mutex<Connection>,
    fts5_available: bool,
}

impl MemorySearcher {
    /// Open (or create) the warm memory database and prepare the search index.
    ///
    /// The FTS5 external-content table is created eagerly when the underlying
    /// `warm_memory` table already exists, and on demand from [`Self::search`]
    /// otherwise (the persistence layer creates `warm_memory` lazily).
    pub fn new(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .context("failed to create memory search database directory")?;
            }
        }
        let conn = Connection::open(db_path).context("failed to open warm memory database")?;
        Self::from_connection(conn)
    }

    /// Build a searcher over an already-open connection (e.g. an in-memory
    /// database in tests, or a connection owned by a caller).
    pub fn from_connection(conn: Connection) -> Result<Self> {
        let fts5_available = fts5_compiled(&conn);
        Self::from_connection_impl(conn, fts5_available)
    }

    fn from_connection_impl(conn: Connection, fts5_available: bool) -> Result<Self> {
        let searcher = Self {
            conn: Mutex::new(conn),
            fts5_available,
        };
        {
            let mut guard = searcher.conn.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("memory search mutex poisoned in constructor, recovering");
                poisoned.into_inner()
            });
            if table_exists(&guard, WARM_MEMORY_TABLE)? {
                ensure_fts_index(&mut guard, fts5_available)?;
            }
        }
        Ok(searcher)
    }

    /// Test hook: force the `LIKE` fallback regardless of what the bundled
    /// SQLite reports, so both search paths are covered by unit tests.
    #[cfg(test)]
    pub(crate) fn from_connection_with_fts_flag(
        conn: Connection,
        fts5_available: bool,
    ) -> Result<Self> {
        Self::from_connection_impl(conn, fts5_available)
    }

    /// Search the warm memory store for entries whose content matches `query`.
    ///
    /// Ranking: FTS5 hits first (bm25), then `LIKE`-only hits (deduplicated
    /// by rowid) when the query contains CJK characters or FTS5 is absent.
    /// Returns at most `limit` hits; `limit == 0` or an empty query yields
    /// an empty result set.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<MemoryHit>> {
        let query = query.trim();
        if query.is_empty() || limit == 0 {
            return Ok(Vec::new());
        }

        let mut conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("memory search mutex poisoned: {e}"))?;

        // Nothing is indexed until the persistence layer creates the table.
        if !table_exists(&conn, WARM_MEMORY_TABLE)? {
            return Ok(Vec::new());
        }
        ensure_fts_index(&mut conn, self.fts5_available)?;

        let query_has_cjk = contains_cjk(query);
        let mut hits: Vec<MemoryHit> = Vec::new();
        let mut seen_rowids: HashSet<i64> = HashSet::new();

        if self.fts5_available {
            let expr = fts_match_expression(query);
            let mut stmt = conn
                .prepare(FTS_SELECT)
                .context("failed to prepare FTS5 memory search")?;
            let mut rows = stmt
                .query(params![expr, limit as i64])
                .context("failed to run FTS5 memory search")?;
            while let Some(row) = rows.next()? {
                let rowid: i64 = row.get(0)?;
                seen_rowids.insert(rowid);
                hits.push(MemoryHit {
                    session_id: row.get(4)?,
                    role: row.get(2)?,
                    content: row.get(3)?,
                    // bm25() ranks lower = better; negate so higher = better.
                    score: -row.get::<_, f64>(5)?,
                });
            }
        }

        if !self.fts5_available || query_has_cjk {
            // LIKE complement: substring matching (covers mid-token CJK text
            // that unicode61 folds into one token). The doubled limit leaves
            // headroom for rows already returned by the FTS path.
            let pattern = like_pattern(query);
            let mut stmt = conn
                .prepare(LIKE_SELECT)
                .context("failed to prepare LIKE memory search")?;
            let mut rows = stmt
                .query(params![pattern, limit.saturating_mul(2) as i64])
                .context("failed to run LIKE memory search")?;
            while let Some(row) = rows.next()? {
                let rowid: i64 = row.get(0)?;
                if !seen_rowids.insert(rowid) {
                    continue;
                }
                hits.push(MemoryHit {
                    session_id: row.get(4)?,
                    role: row.get(2)?,
                    content: row.get(3)?,
                    score: 0.0,
                });
                if hits.len() >= limit {
                    break;
                }
            }
        }

        hits.truncate(limit);
        Ok(hits)
    }
}

/// FTS5 query: join content and session columns; rank by bm25 (ascending =
/// best first), capped by the requested limit.
const FTS_SELECT: &str = "SELECT wm.rowid, wm.id, wm.class, wm.content, wm.session_id, \
     bm25(warm_memory_fts) \
     FROM warm_memory_fts \
     JOIN warm_memory AS wm ON wm.rowid = warm_memory_fts.rowid \
     WHERE warm_memory_fts MATCH ?1 \
     ORDER BY bm25(warm_memory_fts) ASC \
     LIMIT ?2";

/// LIKE fallback: substring match on content, newest first.
const LIKE_SELECT: &str = "SELECT rowid, id, class, content, session_id \
     FROM warm_memory \
     WHERE content LIKE ?1 ESCAPE '\\' \
     ORDER BY created_at DESC, rowid DESC \
     LIMIT ?2";

/// Build a safe FTS5 MATCH expression from a free-text query.
///
/// Each whitespace-separated token is wrapped in a double-quoted phrase with a
/// trailing `*` (prefix match), so `manag` also matches `management`. Phrase
/// quoting makes every FTS5 operator character (`AND`, `OR`, `"`, `-`, ...)
/// literal; tokens are implicitly ANDed.
fn fts_match_expression(query: &str) -> String {
    query
        .split_whitespace()
        .map(|token| format!("\"{}\" *", token.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Build a `LIKE` pattern that treats the query as a literal substring
/// (`%` / `_` / `\` in the query are escaped, not interpreted as wildcards).
fn like_pattern(query: &str) -> String {
    let mut pattern = String::with_capacity(query.len() + 2);
    pattern.push('%');
    for ch in query.chars() {
        match ch {
            '%' | '_' | '\\' => {
                pattern.push('\\');
                pattern.push(ch);
            }
            _ => pattern.push(ch),
        }
    }
    pattern.push('%');
    pattern
}

/// Whether the query contains CJK text. `unicode61` folds a CJK run into one
/// token, so substring matching for such queries needs the `LIKE` path.
fn contains_cjk(text: &str) -> bool {
    text.chars().any(|ch| {
        matches!(
            ch as u32,
            0x3400..=0x4DBF   // CJK Unified Ideographs Extension A
            | 0x4E00..=0x9FFF // CJK Unified Ideographs
            | 0x3040..=0x30FF // Hiragana / Katakana
            | 0xAC00..=0xD7AF // Hangul syllables
            | 0xF900..=0xFAFF // CJK Compatibility Ideographs
        )
    })
}

/// Whether the SQLite library was compiled with FTS5 support.
fn fts5_compiled(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM pragma_compile_options WHERE compile_options = 'ENABLE_FTS5'",
        [],
        |row| row.get::<_, i64>(0),
    )
    .map(|count| count > 0)
    .unwrap_or(false)
}

/// Whether a table (or virtual table) exists in the schema.
fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('table', 'view') AND name = ?1",
        params![table],
        |row| row.get::<_, i64>(0),
    )
    .map(|count| count > 0)
    .context("failed to probe sqlite schema")
}

/// Create the FTS5 external-content table over `warm_memory` plus the
/// triggers that keep it in sync with the persistence layer's writes, and
/// backfill any rows that existed before the index was created.
///
/// No-op when FTS5 is unavailable or the index already exists.
fn ensure_fts_index(conn: &mut Connection, fts5_available: bool) -> Result<()> {
    if !fts5_available || table_exists(conn, FTS_TABLE)? {
        return Ok(());
    }
    conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS {FTS_TABLE} USING fts5(
             content, session_id,
             content='warm_memory',
             content_rowid='rowid',
             tokenize='unicode61'
         );"
    ))
    .context("failed to create FTS5 memory index")?;
    conn.execute_batch(&format!(
        "CREATE TRIGGER IF NOT EXISTS {FTS_TABLE}_ai AFTER INSERT ON warm_memory BEGIN
             INSERT INTO {FTS_TABLE}(rowid, content, session_id)
             VALUES (new.rowid, new.content, new.session_id);
         END;
         CREATE TRIGGER IF NOT EXISTS {FTS_TABLE}_ad AFTER DELETE ON warm_memory BEGIN
             INSERT INTO {FTS_TABLE}({FTS_TABLE}, rowid, content, session_id)
             VALUES ('delete', old.rowid, old.content, old.session_id);
         END;
         CREATE TRIGGER IF NOT EXISTS {FTS_TABLE}_au AFTER UPDATE ON warm_memory BEGIN
             INSERT INTO {FTS_TABLE}({FTS_TABLE}, rowid, content, session_id)
             VALUES ('delete', old.rowid, old.content, old.session_id);
             INSERT INTO {FTS_TABLE}(rowid, content, session_id)
             VALUES (new.rowid, new.content, new.session_id);
         END;"
    ))
    .context("failed to create FTS5 sync triggers")?;
    // Backfill rows written before the index existed.
    conn.execute(
        &format!(
            "INSERT INTO {FTS_TABLE}(rowid, content, session_id)
             SELECT rowid, content, session_id FROM warm_memory"
        ),
        [],
    )
    .context("failed to backfill FTS5 memory index")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Mirrors the `warm_memory` schema owned by `memory_persistence.rs`.
    const WARM_MEMORY_DDL: &str = "CREATE TABLE warm_memory (
        id TEXT PRIMARY KEY,
        tier TEXT NOT NULL DEFAULT 'warm',
        class TEXT NOT NULL,
        content TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        accessed_at INTEGER NOT NULL,
        usefulness REAL NOT NULL DEFAULT 0.0,
        embedding_json TEXT,
        access_count INTEGER NOT NULL DEFAULT 0,
        session_id TEXT,
        user_id TEXT
    );";

    fn insert_memory(
        conn: &Connection,
        id: &str,
        class: &str,
        content: &str,
        session_id: Option<&str>,
    ) {
        conn.execute(
            "INSERT INTO warm_memory
                 (id, tier, class, content, created_at, accessed_at, usefulness,
                  embedding_json, access_count, session_id, user_id)
             VALUES (?1, 'warm', ?2, ?3, ?4, ?4, 0.8, NULL, 0, ?5, NULL)",
            params![id, class, content, 1_700_000_000_i64, session_id],
        )
        .expect("insert memory row");
    }

    /// Create a temp-db with the warm_memory schema plus the given rows
    /// (written before the searcher exists, so the backfill path is exercised).
    fn seed_db(rows: &[(&str, &str, &str)]) -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().expect("temp dir");
        let db_path = dir.path().join("warm.db");
        let conn = Connection::open(&db_path).expect("open db");
        conn.execute_batch(WARM_MEMORY_DDL).expect("create schema");
        for (i, (class, content, session)) in rows.iter().enumerate() {
            insert_memory(&conn, &format!("seed-{i}"), class, content, Some(*session));
        }
        (dir, db_path)
    }

    fn searcher_for(path: &std::path::Path) -> MemorySearcher {
        MemorySearcher::new(path).expect("build searcher")
    }

    // ── FTS5 availability evidence ─────────────────────────────────────

    /// Compile-time verification that the bundled SQLite build ships FTS5:
    /// the virtual table must instantiate and answer a MATCH query.
    #[test]
    fn fts5_is_compiled_into_the_bundled_sqlite_build() {
        let conn = Connection::open_in_memory().expect("open in-memory db");
        conn.execute_batch("CREATE VIRTUAL TABLE fts_probe USING fts5(body);")
            .expect("FTS5 virtual table should be creatable");
        conn.execute(
            "INSERT INTO fts_probe(body) VALUES (?1)",
            params!["memory search"],
        )
        .expect("insert row");
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM fts_probe WHERE fts_probe MATCH ?1",
                params!["search"],
                |row| row.get(0),
            )
            .expect("MATCH query should run");
        assert_eq!(count, 1, "FTS5 index should answer MATCH queries");
    }

    // ── Search behavior ────────────────────────────────────────────────

    #[test]
    fn search_hits_entries_kept_in_sync_by_triggers_and_backfill() {
        let dir = TempDir::new().expect("temp dir");
        let db_path = dir.path().join("warm.db");
        {
            // Rows seeded BEFORE the searcher: covered by the backfill.
            let conn = Connection::open(&db_path).expect("open db");
            conn.execute_batch(WARM_MEMORY_DDL).expect("create schema");
            insert_memory(
                &conn,
                "pre-1",
                "episodic",
                "the auth refactor plan",
                Some("s-1"),
            );
        }
        let searcher = searcher_for(&db_path);
        {
            // Rows inserted AFTER the searcher: covered by the triggers.
            let conn = Connection::open(&db_path).expect("open db");
            insert_memory(
                &conn,
                "post-1",
                "semantic",
                "user prefers rust tooling",
                Some("s-2"),
            );
        }

        let hits = searcher.search("auth refactor", 10).expect("search");
        assert_eq!(hits.len(), 1, "backfilled row should be found");
        assert_eq!(hits[0].session_id.as_deref(), Some("s-1"));
        assert_eq!(hits[0].role, "episodic");
        assert!(
            hits[0].score > 0.0,
            "FTS5 hit should carry a positive score"
        );

        let hits = searcher.search("rust tooling", 10).expect("search");
        assert_eq!(hits.len(), 1, "trigger-synced row should be found");
        assert_eq!(hits[0].session_id.as_deref(), Some("s-2"));
    }

    #[test]
    fn search_hits_chinese_two_char_keyword() {
        let dir = TempDir::new().expect("temp dir");
        let db_path = dir.path().join("warm.db");
        let conn = Connection::open(&db_path).expect("open db");
        conn.execute_batch(WARM_MEMORY_DDL).expect("create schema");
        insert_memory(
            &conn,
            "zh-1",
            "episodic",
            "今天天气不错，内存管理很重要",
            Some("s-zh"),
        );
        insert_memory(
            &conn,
            "zh-2",
            "semantic",
            "关于缓存与查询优化的讨论",
            Some("s-zh"),
        );
        drop(conn);

        let searcher = searcher_for(&db_path);
        // Two-character CJK query: the unicode61 tokenizer folds 内存管理 into
        // a single token, so the LIKE complement must catch this substring.
        let hits = searcher.search("内存", 10).expect("search");
        assert_eq!(hits.len(), 1, "CJK substring should hit exactly one row");
        assert!(hits[0].content.contains("内存"));
        assert_eq!(hits[0].session_id.as_deref(), Some("s-zh"));

        let hits = searcher.search("查询优化", 10).expect("search");
        assert_eq!(hits.len(), 1);
        assert!(hits[0].content.contains("查询优化"));
    }

    #[test]
    fn search_returns_empty_for_garbage() {
        let (dir, db_path) = seed_db(&[("episodic", "rust refactor", "s-1")]);
        let searcher = searcher_for(&db_path);
        let hits = searcher.search("zzzzzznomatchqq", 10).expect("search");
        assert!(hits.is_empty(), "garbage query should return no hits");
        drop(dir);
    }

    #[test]
    fn search_respects_limit() {
        let dir = TempDir::new().expect("temp dir");
        let db_path = dir.path().join("warm.db");
        let conn = Connection::open(&db_path).expect("open db");
        conn.execute_batch(WARM_MEMORY_DDL).expect("create schema");
        for i in 0..15 {
            insert_memory(
                &conn,
                &format!("limit-{i}"),
                "episodic",
                &format!("alpha incident number {i}"),
                Some("s-limit"),
            );
        }
        drop(conn);

        let searcher = searcher_for(&db_path);
        let hits = searcher.search("alpha incident", 3).expect("search");
        assert_eq!(hits.len(), 3, "limit must cap the result set");
        let hits = searcher.search("alpha incident", 0).expect("search");
        assert!(hits.is_empty(), "limit 0 must return nothing");
    }

    #[test]
    fn search_empty_query_returns_empty() {
        let (dir, db_path) = seed_db(&[("episodic", "anything at all", "s-1")]);
        let searcher = searcher_for(&db_path);
        assert!(searcher.search("", 10).expect("search").is_empty());
        assert!(searcher.search("   ", 10).expect("search").is_empty());
        drop(dir);
    }

    #[test]
    fn search_on_missing_table_returns_empty() {
        // A warm.db that exists but has no warm_memory table yet: the
        // persistence layer creates it lazily, so search must degrade to empty.
        let dir = TempDir::new().expect("temp dir");
        let db_path = dir.path().join("warm.db");
        let conn = Connection::open(&db_path).expect("open db");
        drop(conn);
        let searcher = searcher_for(&db_path);
        let hits = searcher.search("anything", 10).expect("search");
        assert!(hits.is_empty());
    }

    #[test]
    fn like_fallback_serves_queries_when_fts5_is_unavailable() {
        let dir = TempDir::new().expect("temp dir");
        let db_path = dir.path().join("warm.db");
        let conn = Connection::open(&db_path).expect("open db");
        conn.execute_batch(WARM_MEMORY_DDL).expect("create schema");
        insert_memory(
            &conn,
            "fb-1",
            "episodic",
            "plan the database migration",
            Some("s-fb"),
        );
        insert_memory(
            &conn,
            "fb-2",
            "semantic",
            "数据库迁移需要注意",
            Some("s-fb"),
        );
        drop(conn);

        // Force the LIKE path even though the bundled build has FTS5.
        let conn = Connection::open(&db_path).expect("reopen db");
        let searcher =
            MemorySearcher::from_connection_with_fts_flag(conn, false).expect("build searcher");

        let hits = searcher.search("migration", 10).expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session_id.as_deref(), Some("s-fb"));
        assert_eq!(hits[0].score, 0.0, "LIKE-only hits carry a zero score");

        let hits = searcher.search("数据库", 10).expect("search");
        assert_eq!(hits.len(), 1);
        assert!(hits[0].content.contains("数据库"));
    }

    #[test]
    fn like_pattern_escapes_wildcards() {
        assert_eq!(like_pattern("50%"), "%50\\%%");
        assert_eq!(like_pattern("a_b"), "%a\\_b%");
        assert_eq!(like_pattern("内存"), "%内存%");
        assert_eq!(like_pattern("plain"), "%plain%");
    }

    #[test]
    fn fts_match_expression_quotes_operators() {
        // FTS5 operator words and characters must become literal phrase text.
        assert_eq!(
            fts_match_expression("hello world"),
            "\"hello\" * \"world\" *"
        );
        assert_eq!(
            fts_match_expression("AND OR NOT"),
            "\"AND\" * \"OR\" * \"NOT\" *"
        );
        assert_eq!(
            fts_match_expression("say \"hi\""),
            "\"say\" * \"\"\"hi\"\"\" *"
        );
    }
}
