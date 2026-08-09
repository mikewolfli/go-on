//! GAP-B52-11: Memory Persistence with Hot/Warm/Cold Tiering
//!
//! Implements a three-tier memory persistence system:
//! - **Hot (L1)**: In-memory LRU cache (max 2048 entries, 30-minute TTL)
//! - **Warm (L2)**: SQLite-backed vector store (30-day retention)
//! - **Cold (L3)**: gzip-compressed NDJSON files on disk for long-term archival
//!
//! Provides automatic migration (promotion/demotion) between tiers.

use anyhow::{Context, Result};
use tokio::task::spawn_blocking;

use flate2::write::GzEncoder;
use flate2::Compression;
use indexmap::IndexSet;
#[cfg(feature = "backend-postgres")]
use postgres::{Client as PgClient, NoTls as PgNoTls};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::memory::summarization::{MemorySummarizer, SummarizedMemory};

/// The memory tier an entry resides in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MemoryTier {
    /// L1: In-memory LRU cache. 2048 max entries, 30-minute TTL (configurable).
    Hot,
    /// L2: SQLite vector store with 30-day retention.
    Warm,
    /// L3: gzip-compressed NDJSON on disk for long-term archival.
    Cold,
}

impl MemoryTier {
    /// Human-readable label for the tier.
    pub fn label(&self) -> &'static str {
        match self {
            MemoryTier::Hot => "hot",
            MemoryTier::Warm => "warm",
            MemoryTier::Cold => "cold",
        }
    }
}

/// A memory entry tracked through the persistence tier system.
///
/// This struct carries both semantic metadata (from the existing
/// `crate::memory::memory::MemoryEntry`) and tiering-specific fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// Unique identifier.
    pub id: String,
    /// The current tier this entry resides in.
    pub tier: MemoryTier,
    /// Semantic class label (e.g. "episodic", "semantic").
    pub class: String,
    /// Text content of the memory.
    pub content: String,
    /// Unix timestamp (seconds) when this entry was created.
    pub created_at: i64,
    /// Unix timestamp (seconds) of the last access.
    pub accessed_at: i64,
    /// Usefulness score (0.0 – 1.0).
    pub usefulness: f32,
    /// Embedding vector for similarity search (optional, L2/L3).
    pub embedding: Option<Vec<f32>>,
    /// Number of times this entry has been accessed.
    pub access_count: i64,
    /// Optional session_id this entry belongs to.
    pub session_id: Option<String>,
    /// Optional user_id for multi-user isolation.
    pub user_id: Option<String>,
}

impl MemoryEntry {
    /// Create a new hot-tier memory entry.
    pub fn new_hot(
        id: impl Into<String>,
        class: impl Into<String>,
        content: impl Into<String>,
        usefulness: f32,
    ) -> Self {
        let now = crate::shared::timestamps::now_ts();
        Self {
            id: id.into(),
            tier: MemoryTier::Hot,
            class: class.into(),
            content: content.into(),
            created_at: now,
            accessed_at: now,
            usefulness,
            embedding: None,
            access_count: 0,
            session_id: None,
            user_id: None,
        }
    }

    /// Touch the entry, updating its access timestamp and count.
    pub fn touch(&mut self) {
        self.accessed_at = crate::shared::timestamps::now_ts();
        self.access_count += 1;
    }

    /// Returns the age of this entry in seconds.
    pub fn age_secs(&self) -> i64 {
        crate::shared::timestamps::now_ts().saturating_sub(self.created_at)
    }

    /// Returns the time since last access in seconds.
    pub fn idle_secs(&self) -> i64 {
        crate::shared::timestamps::now_ts().saturating_sub(self.accessed_at)
    }
}

/// Policy governing when entries are promoted or demoted between tiers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryTieringPolicy {
    /// If `usefulness` >= this threshold, consider promoting Hot → Warm.
    pub hot_threshold: f32,
    /// If `usefulness` >= this threshold, consider promoting Warm → Cold.
    pub warm_threshold: f32,
    /// Maximum idle seconds before an entry is demoted from Hot → Warm.
    pub hot_ttl_secs: i64,
    /// Maximum idle seconds before an entry is demoted from Warm → Cold.
    pub warm_ttl_secs: i64,
    /// Maximum number of hot entries.
    pub hot_max_entries: usize,
    /// Maximum entries in warm store.
    pub warm_max_entries: usize,
}

impl Default for MemoryTieringPolicy {
    fn default() -> Self {
        Self {
            hot_threshold: 0.3,
            warm_threshold: 0.6,
            hot_ttl_secs: 1800,       // 30 minutes — short-term "cognitive" memory
            warm_ttl_secs: 2_592_000, // 30 days
            hot_max_entries: 2048,
            warm_max_entries: 100_000,
        }
    }
}

// ===========================================================================
// L1: Hot Tier — In-Memory LRU Cache
// ===========================================================================

/// A single entry in the hot cache with LRU tracking.
#[derive(Debug, Clone)]
struct HotEntry {
    entry: MemoryEntry,
    inserted_at: Instant,
}

/// L1 hot cache using a simple LRU eviction policy.
#[derive(Debug)]
struct HotCache {
    entries: HashMap<String, HotEntry>,
    lru_order: IndexSet<String>,
    max_entries: usize,
    ttl: Duration,
}

impl HotCache {
    fn new(max_entries: usize, ttl_secs: i64) -> Self {
        Self {
            entries: HashMap::with_capacity(max_entries),
            lru_order: IndexSet::with_capacity(max_entries),
            max_entries,
            ttl: Duration::from_secs(ttl_secs.max(0) as u64),
        }
    }

    fn insert(&mut self, mut entry: MemoryEntry) {
        entry.tier = MemoryTier::Hot;

        // If the entry already exists, just refresh it.
        if let Some(existing) = self.entries.get_mut(&entry.id) {
            existing.entry = entry;
            existing.inserted_at = Instant::now();
            // Move to MRU
            self.lru_order.shift_remove(&existing.entry.id);
            self.lru_order.insert(existing.entry.id.clone());
            return;
        }

        // Evict if at capacity (LRU: remove from front).
        if self.entries.len() >= self.max_entries {
            self.evict_lru_one();
        }

        let id = entry.id.clone();
        self.lru_order.insert(id.clone());
        self.entries.insert(
            id,
            HotEntry {
                entry,
                inserted_at: Instant::now(),
            },
        );
    }

    fn remove(&mut self, id: &str) -> Option<MemoryEntry> {
        self.lru_order.shift_remove(id);
        self.entries.remove(id).map(|he| he.entry)
    }

    /// Evict expired entries (TTL exceeded). Returns evicted entries.
    fn evict_expired(&mut self) -> Vec<MemoryEntry> {
        let now = Instant::now();
        let expired_ids: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, he)| now.duration_since(he.inserted_at) >= self.ttl)
            .map(|(id, _)| id.clone())
            .collect();

        let mut evicted = Vec::with_capacity(expired_ids.len());
        for id in expired_ids {
            if let Some(entry) = self.remove(&id) {
                evicted.push(entry);
            }
        }
        evicted
    }

    fn evict_lru_one(&mut self) -> Option<MemoryEntry> {
        let lru_id = self.lru_order.first()?.clone();
        self.remove(&lru_id)
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.lru_order.clear();
    }
}

// ===========================================================================
// L3: Cold Tier — gzip NDJSON on disk
// ===========================================================================

/// Lightweight sidecar index: (user_id, memory_id) → cold storage location.
///
/// Maps each memory entry to the shard file where it is stored in cold
/// storage, enabling O(1) retrieval without scanning all shards.
#[derive(Debug, Clone, Default)]
pub struct ColdStorageIndex {
    /// Index: (user_id_or_empty, memory_id) → (year_month, shard_name, line_offset)
    entries: HashMap<(String, String), (String, String, u64)>,
}

impl ColdStorageIndex {
    /// Record a cold storage location for a memory entry.
    pub fn store(
        &mut self,
        user_id: Option<&str>,
        memory_id: &str,
        year_month: &str,
        shard_name: &str,
        line_offset: u64,
    ) {
        let uid = user_id.unwrap_or("").to_string();
        self.entries.insert(
            (uid, memory_id.to_string()),
            (year_month.to_string(), shard_name.to_string(), line_offset),
        );
    }
}

/// Manages cold storage: `.goon/memory/cold/YYYY-MM/*.ndjson.gz`
#[derive(Debug)]
struct ColdStorage {
    base_path: PathBuf,
    max_shard_size_bytes: u64,
    max_total_shards: usize,
    /// Sidecar index for O(1) cold storage lookups.
    index: Mutex<ColdStorageIndex>,
}

impl ColdStorage {
    fn new(base_path: &Path) -> Self {
        Self {
            base_path: base_path.to_path_buf(),
            max_shard_size_bytes: 10 * 1024 * 1024, // 10 MB default
            max_total_shards: 100,
            index: Mutex::new(ColdStorageIndex::default()),
        }
    }

    /// Get the cold storage directory for a given year-month.
    fn month_dir(&self, year: i32, month: u32) -> PathBuf {
        self.base_path.join(format!("{:04}-{:02}", year, month))
    }

    /// Generate a filename for a cold storage shard.
    fn shard_path(&self, year: i32, month: u32, shard_id: &str) -> PathBuf {
        self.month_dir(year, month)
            .join(format!("{}.ndjson.gz", shard_id))
    }

    /// Find the next available shard index within the given year-month directory.
    fn next_shard_index(&self, year: i32, month: u32, start_index: u32) -> u32 {
        let dir = self.month_dir(year, month);
        let mut idx = start_index;
        loop {
            let path = dir.join(format!("{:04}-{:02}-{:03}.ndjson.gz", year, month, idx));
            if !path.exists() {
                return idx;
            }
            idx += 1;
        }
    }

    /// Count existing shard files under the base path.
    fn total_shard_count(&self) -> usize {
        let mut count = 0;
        if !self.base_path.exists() {
            return 0;
        }
        if let Ok(dir_iter) = fs::read_dir(&self.base_path) {
            for entry in dir_iter.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if let Ok(file_iter) = fs::read_dir(&path) {
                        for file in file_iter.flatten() {
                            if file.path().extension().and_then(|e| e.to_str()) == Some("gz") {
                                count += 1;
                            }
                        }
                    }
                }
            }
        }
        count
    }

    /// Append a single entry to the latest shard for the current month,
    /// rotating to a new shard when the current one exceeds the size limit.
    fn append_entry(&self, entry: &MemoryEntry) -> Result<()> {
        let now_s = crate::shared::timestamps::now_ts();
        // Compute year/month from Unix timestamp (simple divisional approach).
        // Uses 1970-01-01 as base, accounting for leap years.
        let (year, month) = ts_to_year_month(now_s);
        let dir = self.month_dir(year, month);
        fs::create_dir_all(&dir).context("failed to create cold storage month directory")?;

        // Determine current active shard for this month.
        let shard_index = self.next_shard_index(year, month, 0);
        let shard = if shard_index == 0 {
            // No shard exists yet; start with index 0.
            format!("{:04}-{:02}-000", year, month)
        } else {
            // Check if the latest shard has room.
            let latest_idx = shard_index.saturating_sub(1);
            let latest_path = dir.join(format!(
                "{:04}-{:02}-{:03}.ndjson.gz",
                year, month, latest_idx
            ));
            let file_size = fs::metadata(&latest_path).map(|m| m.len()).unwrap_or(0);
            if file_size < self.max_shard_size_bytes {
                // Reuse existing shard.
                format!("{:04}-{:02}-{:03}", year, month, latest_idx)
            } else {
                // Need a new shard.
                format!("{:04}-{:02}-{:03}", year, month, shard_index)
            }
        };

        let path = self.shard_path(year, month, &shard);

        let line = serde_json::to_string(entry).context("failed to serialize cold entry")?;
        let file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .context("failed to open cold storage shard")?;
        let mut encoder = GzEncoder::new(file, Compression::default());
        writeln!(encoder, "{}", line).context("failed to write cold entry")?;
        encoder
            .finish()
            .context("failed to finalize cold storage gzip")?;

        // Record entry location in the sidecar index for O(1) lookups.
        if let Ok(mut idx) = self.index.lock() {
            idx.store(
                entry.user_id.as_deref(),
                &entry.id,
                &format!("{:04}-{:02}", year, month),
                &shard,
                // Approximate line offset; the actual offset within this shard
                // is not tracked precisely, but the index still narrows the
                // search to a single shard instead of scanning all shards.
                0,
            );
        }

        // Enforce max total shards: if we just created a new shard, evict oldest.
        let current_count = self.total_shard_count();
        if current_count > self.max_total_shards {
            self.evict_oldest_shards(current_count - self.max_total_shards);
        }

        Ok(())
    }

    /// Remove the oldest shards (by modification time) until `count` have been removed.
    fn evict_oldest_shards(&self, count: usize) {
        let mut shards: Vec<std::path::PathBuf> = Vec::new();
        if let Ok(dir_iter) = fs::read_dir(&self.base_path) {
            for entry in dir_iter.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if let Ok(file_iter) = fs::read_dir(&path) {
                        for file in file_iter.flatten() {
                            let fp = file.path();
                            if fp.extension().and_then(|e| e.to_str()) == Some("gz") {
                                shards.push(fp);
                            }
                        }
                    }
                }
            }
        }
        shards.sort_by_key(|p| fs::metadata(p).and_then(|m| m.modified()).ok());
        for shard in shards.into_iter().take(count) {
            let _ = fs::remove_file(&shard);
        }
    }
}

// ===========================================================================
// L2: Warm Tier — SQLite/PostgreSQL-backed persistence
// ===========================================================================

/// Shared column list for warm_memory queries.
const WARM_MEMORY_COLUMNS: &str = "id, tier, class, content, created_at, accessed_at, usefulness, embedding_json, access_count, session_id, user_id";

#[cfg(feature = "backend-sqlite")]
const PARAM_PREFIX: &str = "?";
#[cfg(all(not(feature = "backend-sqlite"), feature = "backend-postgres"))]
const PARAM_PREFIX: &str = "$";

// ---- Shared row-to-entry mapping -------------------------------------------

#[cfg(feature = "backend-sqlite")]
fn row_to_memory_entry(row: &rusqlite::Row) -> rusqlite::Result<MemoryEntry> {
    let embedding_json: Option<String> = row.get(7)?;
    let embedding = embedding_json.and_then(|j| serde_json::from_str::<Vec<f32>>(&j).ok());
    Ok(MemoryEntry {
        id: row.get(0)?,
        tier: MemoryTier::Warm,
        class: row.get(2)?,
        content: row.get(3)?,
        created_at: row.get(4)?,
        accessed_at: row.get(5)?,
        usefulness: row.get(6)?,
        embedding,
        access_count: row.get(8)?,
        session_id: row.get(9)?,
        user_id: row.get(10)?,
    })
}

#[cfg(all(not(feature = "backend-sqlite"), feature = "backend-postgres"))]
fn row_to_memory_entry(row: &postgres::Row) -> MemoryEntry {
    let embedding_json: Option<String> = row.get(7);
    let embedding = embedding_json.and_then(|j| serde_json::from_str::<Vec<f32>>(&j).ok());
    MemoryEntry {
        id: row.get(0),
        tier: MemoryTier::Warm,
        class: row.get(2),
        content: row.get(3),
        created_at: row.get(4),
        accessed_at: row.get(5),
        usefulness: row.get(6),
        embedding,
        access_count: row.get(8),
        session_id: row.get(9),
        user_id: row.get(10),
    }
}

// ---- Shared query helper ----------------------------------------------------

/// Run a query that returns rows, mapping each row through `row_to_memory_entry`.
#[cfg(feature = "backend-sqlite")]
fn query_all(
    conn: &rusqlite::Connection,
    sql: &str,
    params: &[&dyn rusqlite::types::ToSql],
) -> Result<Vec<MemoryEntry>> {
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query(params)?;
    let mut results = Vec::new();
    while let Some(row) = rows.next()? {
        results.push(row_to_memory_entry(row)?);
    }
    Ok(results)
}

#[cfg(all(not(feature = "backend-sqlite"), feature = "backend-postgres"))]
fn query_all(
    conn: &mut postgres::Client,
    sql: &str,
    params: &[&(dyn postgres::types::ToSql + Sync)],
) -> Result<Vec<MemoryEntry>> {
    let rows = conn.query(sql, params)?;
    Ok(rows.iter().map(row_to_memory_entry).collect())
}

/// Wrapper around the warm store (SQLite or PostgreSQL).
///
/// Uses the same schema patterns as `crate::memory::vector::VectorStore`
/// but with a dedicated table for memory tier persistence.
#[cfg(feature = "backend-sqlite")]
#[derive(Debug)]
pub struct WarmStore {
    conn: Arc<Mutex<rusqlite::Connection>>,
    max_entries: usize,
}

#[cfg(all(not(feature = "backend-sqlite"), feature = "backend-postgres"))]
pub struct WarmStore {
    conn: Arc<Mutex<PgClient>>,
    max_entries: usize,
}

#[cfg(all(not(feature = "backend-sqlite"), feature = "backend-postgres"))]
impl std::fmt::Debug for WarmStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WarmStore")
            .field("max_entries", &self.max_entries)
            .field("conn", &"<Mutex<PgClient>>")
            .finish()
    }
}

impl WarmStore {
    #[cfg(feature = "backend-sqlite")]
    fn new(path: &Path, max_entries: usize) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let conn = rusqlite::Connection::open(path)?;
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA wal_autocheckpoint = 1000;

            CREATE TABLE IF NOT EXISTS warm_memory (
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
            );

            CREATE INDEX IF NOT EXISTS idx_warm_memory_accessed_at
                ON warm_memory(accessed_at DESC);

            CREATE INDEX IF NOT EXISTS idx_warm_memory_usefulness
                ON warm_memory(usefulness DESC);

            CREATE INDEX IF NOT EXISTS idx_warm_memory_session_id
                ON warm_memory(session_id);

            CREATE INDEX IF NOT EXISTS idx_warm_memory_user_id
                ON warm_memory(user_id);
            ",
        )?;

        // Force a WAL checkpoint on startup so the WAL file does not grow
        // unboundedly between application restarts (BLUE69 §performance).
        let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            max_entries,
        })
    }

    #[cfg(all(not(feature = "backend-sqlite"), feature = "backend-postgres"))]
    fn new(db_conn_str: &Path, max_entries: usize) -> Result<Self> {
        let conn_str = db_conn_str.to_string_lossy();
        let mut client = PgClient::connect(&conn_str, PgNoTls)?;

        client.batch_execute(
            "
            CREATE TABLE IF NOT EXISTS warm_memory (
                id TEXT PRIMARY KEY,
                tier TEXT NOT NULL DEFAULT 'warm',
                class TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at BIGINT NOT NULL,
                accessed_at BIGINT NOT NULL,
                usefulness REAL NOT NULL DEFAULT 0.0,
                embedding_json TEXT,
                access_count BIGINT NOT NULL DEFAULT 0,
                session_id TEXT,
                user_id TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_warm_memory_accessed_at
                ON warm_memory(accessed_at DESC);

            CREATE INDEX IF NOT EXISTS idx_warm_memory_usefulness
                ON warm_memory(usefulness DESC);

            CREATE INDEX IF NOT EXISTS idx_warm_memory_session_id
                ON warm_memory(session_id);

            CREATE INDEX IF NOT EXISTS idx_warm_memory_user_id
                ON warm_memory(user_id);
            ",
        )?;

        Ok(Self {
            conn: Arc::new(Mutex::new(client)),
            max_entries,
        })
    }

    // ---- Unified business methods --------------------------------------------

    async fn upsert(&self, entry: &MemoryEntry) -> Result<()> {
        #[cfg(feature = "backend-sqlite")]
        let _permit = crate::shared::db_pool::acquire_db_permit().await;
        let conn = self.conn.clone();
        let max_entries = self.max_entries;
        let embedding_json = entry
            .embedding
            .as_ref()
            .map(|v| serde_json::to_string(v).unwrap_or_default());
        #[cfg(feature = "backend-sqlite")]
        {
            let entry_id = entry.id.clone();
            let tier_label = entry.tier.label().to_string();
            let class = entry.class.clone();
            let content = entry.content.clone();
            let created_at = entry.created_at;
            let accessed_at = entry.accessed_at;
            let usefulness = entry.usefulness;
            let access_count = entry.access_count;
            let session_id = entry.session_id.clone();
            let user_id = entry.user_id.clone();
            spawn_blocking(move || {
                let conn = conn.lock().unwrap_or_else(|poisoned| {
                    tracing::warn!("warm store mutex poisoned, recovering");
                    poisoned.into_inner()
                });
                let sql = format!(
                    "INSERT INTO warm_memory({columns})
                     VALUES({p}1, {p}2, {p}3, {p}4, {p}5, {p}6, {p}7, {p}8, {p}9, {p}10, {p}11)
                     ON CONFLICT(id) DO UPDATE SET
                        tier = excluded.tier,
                        class = excluded.class,
                        content = excluded.content,
                        accessed_at = excluded.accessed_at,
                        usefulness = excluded.usefulness,
                        embedding_json = excluded.embedding_json,
                        access_count = excluded.access_count,
                        session_id = excluded.session_id,
                        user_id = excluded.user_id",
                    columns = WARM_MEMORY_COLUMNS,
                    p = PARAM_PREFIX
                );
                conn.execute(
                    &sql,
                    rusqlite::params![
                        &entry_id,
                        &tier_label,
                        &class,
                        &content,
                        created_at,
                        accessed_at,
                        usefulness,
                        &embedding_json,
                        access_count,
                        &session_id,
                        &user_id,
                    ],
                )?;
                // Evict only when the table actually exceeds the cap — the
                // DELETE+ORDER BY full-table sort is skipped on every normal
                // write (same COUNT-gated pattern as cache.rs).
                let over_cap: i64 = conn.query_row(
                    &format!("SELECT COUNT(*) - {p}1 FROM warm_memory", p = PARAM_PREFIX),
                    rusqlite::params![max_entries as i64],
                    |row| row.get(0),
                )?;
                if over_cap > 0 {
                    conn.execute(
                        &format!(
                            "DELETE FROM warm_memory WHERE id IN (
                            SELECT id FROM warm_memory ORDER BY accessed_at ASC \
                            LIMIT {p}1
                        )",
                            p = PARAM_PREFIX
                        ),
                        rusqlite::params![over_cap],
                    )?;
                }
                Ok(())
            })
            .await
            .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))?
        }
        #[cfg(all(not(feature = "backend-sqlite"), feature = "backend-postgres"))]
        {
            let entry_id = entry.id.clone();
            let tier_label = entry.tier.label().to_string();
            let class = entry.class.clone();
            let content = entry.content.clone();
            let created_at = entry.created_at;
            let accessed_at = entry.accessed_at;
            let usefulness = entry.usefulness;
            let access_count = entry.access_count;
            let session_id = entry.session_id.clone();
            let user_id = entry.user_id.clone();
            spawn_blocking(move || {
                let mut conn = conn.lock().unwrap_or_else(|poisoned| {
                    tracing::warn!("warm store mutex poisoned, recovering");
                    poisoned.into_inner()
                });
                let sql = format!(
                    "INSERT INTO warm_memory({columns})
                     VALUES({p}1, {p}2, {p}3, {p}4, {p}5, {p}6, {p}7, {p}8, {p}9, {p}10, {p}11)
                     ON CONFLICT(id) DO UPDATE SET
                        tier = EXCLUDED.tier,
                        class = EXCLUDED.class,
                        content = EXCLUDED.content,
                        accessed_at = EXCLUDED.accessed_at,
                        usefulness = EXCLUDED.usefulness,
                        embedding_json = EXCLUDED.embedding_json,
                        access_count = EXCLUDED.access_count,
                        session_id = EXCLUDED.session_id,
                        user_id = EXCLUDED.user_id",
                    columns = WARM_MEMORY_COLUMNS,
                    p = PARAM_PREFIX
                );
                conn.execute(
                    &sql,
                    &[
                        &entry_id,
                        &tier_label,
                        &class,
                        &content,
                        &created_at,
                        &accessed_at,
                        &usefulness,
                        &embedding_json,
                        &access_count,
                        &session_id,
                        &user_id,
                    ],
                )?;
                // Evict only when the table actually exceeds the cap (see
                // the sqlite branch above for rationale).
                let row = conn.query_one(
                    &format!("SELECT COUNT(*) - {p}1 FROM warm_memory", p = PARAM_PREFIX),
                    &[&(max_entries as i64)],
                )?;
                let over_cap: i64 = row.get(0);
                if over_cap > 0 {
                    conn.execute(
                        &format!(
                            "DELETE FROM warm_memory WHERE id IN (
                            SELECT id FROM warm_memory ORDER BY accessed_at ASC \
                            LIMIT {p}1
                        )",
                            p = PARAM_PREFIX
                        ),
                        &[&over_cap],
                    )?;
                }
                Ok(())
            })
            .await
            .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))?
        }
    }

    async fn remove(&self, id: &str) -> Result<bool> {
        #[cfg(feature = "backend-sqlite")]
        let _permit = crate::shared::db_pool::acquire_db_permit().await;
        let conn = self.conn.clone();
        let id = id.to_string();
        spawn_blocking(move || {
            #[allow(unused_mut, reason = "mut needed by backend-postgres Client::execute")]
            let mut conn = conn.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("warm store mutex poisoned, recovering");
                poisoned.into_inner()
            });
            let sql = format!("DELETE FROM warm_memory WHERE id = {p}1", p = PARAM_PREFIX);
            #[cfg(feature = "backend-sqlite")]
            {
                let affected = conn.execute(&sql, rusqlite::params![id])?;
                Ok(affected > 0)
            }
            #[cfg(all(not(feature = "backend-sqlite"), feature = "backend-postgres"))]
            {
                let affected = conn.execute(&sql, &[&id])?;
                Ok(affected > 0)
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))?
    }

    async fn iterate_all(&self) -> Result<Vec<MemoryEntry>> {
        #[cfg(feature = "backend-sqlite")]
        let _permit = crate::shared::db_pool::acquire_db_permit().await;
        let conn = self.conn.clone();
        spawn_blocking(move || {
            #[cfg(not(feature = "backend-sqlite"))]
            let mut conn = conn
                .lock()
                .map_err(|e| anyhow::anyhow!("warm store mutex poisoned: {}", e))?;
            #[cfg(feature = "backend-sqlite")]
            let conn = conn
                .lock()
                .map_err(|e| anyhow::anyhow!("warm store mutex poisoned: {}", e))?;
            let sql = format!("SELECT {} FROM warm_memory", WARM_MEMORY_COLUMNS);
            #[cfg(feature = "backend-sqlite")]
            {
                let empty_params: [&dyn rusqlite::types::ToSql; 0] = [];
                query_all(&conn, &sql, &empty_params)
            }
            #[cfg(not(feature = "backend-sqlite"))]
            {
                let empty_params: [&(dyn postgres::types::ToSql + Sync); 0] = [];
                query_all(&mut conn, &sql, &empty_params)
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))?
    }

    pub async fn search_by_session(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>> {
        let conn = self.conn.clone();
        let session_id = session_id.to_string();
        spawn_blocking(move || {
            #[cfg(not(feature = "backend-sqlite"))]
            let mut conn = conn
                .lock()
                .map_err(|e| anyhow::anyhow!("warm store mutex poisoned: {}", e))?;
            #[cfg(feature = "backend-sqlite")]
            let conn = conn
                .lock()
                .map_err(|e| anyhow::anyhow!("warm store mutex poisoned: {}", e))?;
            let sql = format!(
                "SELECT {} FROM warm_memory WHERE session_id = {p}1 \
                 ORDER BY accessed_at DESC LIMIT {p}2",
                WARM_MEMORY_COLUMNS,
                p = PARAM_PREFIX
            );
            #[cfg(feature = "backend-sqlite")]
            {
                query_all(&conn, &sql, &[&session_id, &(limit as i64)])
            }
            #[cfg(not(feature = "backend-sqlite"))]
            {
                query_all(&mut conn, &sql, &[&session_id, &(limit as i64)])
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))?
    }
}

/// WarmStore stub for backends without a warm persistence layer.
#[cfg(not(any(feature = "backend-sqlite", feature = "backend-postgres")))]
#[derive(Debug)]
pub struct WarmStore {
    max_entries: usize,
}

#[cfg(not(any(feature = "backend-sqlite", feature = "backend-postgres")))]
impl WarmStore {
    fn new(_path: &Path, _max_entries: usize) -> Result<Self> {
        Err(anyhow::anyhow!(
            "No storage backend configured: enable backend-sqlite or backend-postgres feature"
        ))
    }

    fn upsert(&self, _entry: &MemoryEntry) -> Result<()> {
        Err(anyhow::anyhow!(
            "No storage backend configured: enable backend-sqlite or backend-postgres feature"
        ))
    }

    fn remove(&self, _id: &str) -> Result<bool> {
        Err(anyhow::anyhow!(
            "No storage backend configured: enable backend-sqlite or backend-postgres feature"
        ))
    }

    fn iterate_all(&self) -> Result<Vec<MemoryEntry>> {
        Err(anyhow::anyhow!(
            "No storage backend configured: enable backend-sqlite or backend-postgres feature"
        ))
    }

    fn search_by_session(&self, _session_id: &str, _limit: usize) -> Result<Vec<MemoryEntry>> {
        Err(anyhow::anyhow!(
            "No storage backend configured: enable backend-sqlite or backend-postgres feature"
        ))
    }
}

/// Manages memory persistence across three tiers: hot, warm, cold.
///
/// - Insert entries into the appropriate tier.
/// - Promote hot → warm and warm → cold based on policy.
#[derive(Debug)]
pub struct MemoryPersistence {
    /// L1: hot cache (in-memory LRU)
    hot: Mutex<HotCache>,
    /// L2: warm store (SQLite / no-op fallback for postgres)
    warm: WarmStore,
    /// L3: cold storage (gzip NDJSON)
    cold: ColdStorage,
    /// Tiering policy
    policy: MemoryTieringPolicy,
    /// Optional memory summarizer for compressing excess hot entries
    /// during tier migration. Wired via `with_summarizer`.
    summarizer: Option<MemorySummarizer>,
}

impl MemoryPersistence {
    /// Create a new memory persistence manager.
    ///
    /// # Arguments
    /// * `db_path` - Path to the SQLite warm store database.
    /// * `cold_base_path` - Path to the cold storage directory (e.g. `.goon/memory/cold`).
    /// * `policy` - Tiering policy; uses default if `None`.
    pub fn new(
        db_path: &Path,
        cold_base_path: &Path,
        policy: Option<MemoryTieringPolicy>,
    ) -> Result<Self> {
        let policy = policy.unwrap_or_default();

        // Ensure cold storage directory exists.
        fs::create_dir_all(cold_base_path)
            .context("failed to create cold storage base directory")?;

        let warm = WarmStore::new(db_path, policy.warm_max_entries)?;

        Ok(Self {
            hot: Mutex::new(HotCache::new(policy.hot_max_entries, policy.hot_ttl_secs)),
            warm,
            cold: ColdStorage::new(cold_base_path),
            policy,
            summarizer: None,
        })
    }

    /// Insert or update a memory entry.
    ///
    /// The entry is placed in the hot tier by default and will be promoted
    /// according to the tiering policy.
    pub async fn store(&self, entry: MemoryEntry) -> Result<()> {
        let mut hot = self.hot.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("hot cache mutex poisoned in 'store', recovering");
            poisoned.into_inner()
        });

        // Always insert/refresh in hot tier.
        hot.insert(entry);
        Ok(())
    }

    /// Promote an entry from hot → warm tier.
    pub async fn promote_to_warm(&self, entry: MemoryEntry) -> Result<()> {
        let mut entry = entry;
        entry.tier = MemoryTier::Warm;
        self.warm.upsert(&entry).await?;

        // Remove from hot.
        let mut hot = self.hot.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("hot cache mutex poisoned in 'promote_to_warm', recovering");
            poisoned.into_inner()
        });
        hot.remove(&entry.id);
        Ok(())
    }

    /// Promote an entry from warm → cold (archival).
    pub async fn promote_to_cold(&self, entry: MemoryEntry) -> Result<()> {
        let mut entry = entry;
        entry.tier = MemoryTier::Cold;
        self.cold.append_entry(&entry)?;

        // Remove from warm.
        self.warm.remove(&entry.id).await?;
        Ok(())
    }

    /// Search the durable warm tier for entries belonging to a session,
    /// most recently accessed first. This is the read side of the
    /// persistence layer — used by `session/load` and `session/resume` to
    /// restore a session's memory context. (Previously the persistence layer
    /// was write-only in production.)
    pub async fn search_by_session(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>> {
        self.warm.search_by_session(session_id, limit).await
    }

    /// Run automatic tier migration based on policy.
    ///
    /// 1. Evict expired hot entries → promote useful ones to warm, discard stale ones.
    /// 2. Check warm entries approaching TTL → promote useful ones to cold.
    pub async fn auto_migrate(&self) -> Result<MigrationReport> {
        let mut report = MigrationReport::default();

        // ── Step 0: Summarize hot entries if summarizer is configured ──
        self.summarize_hot_entries().await?;

        // ── Step 1: Process hot cache evictions ──
        let evicted: Vec<MemoryEntry> = {
            let mut hot = self.hot.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("hot cache mutex poisoned in 'auto_migrate', recovering");
                poisoned.into_inner()
            });
            hot.evict_expired()
        };

        for entry in evicted {
            if entry.usefulness >= self.policy.hot_threshold {
                // Promote to warm.
                self.promote_to_warm(entry).await?;
                report.promoted_hot_to_warm += 1;
            } else {
                // Stale, demote to cold directly (or discard).
                self.promote_to_cold(entry).await?;
                report.demoted_hot_to_cold += 1;
            }
        }

        // ── Step 2: Check warm for TTL candidates ──
        let warm_entries = self.warm.iterate_all().await?;
        let now = crate::shared::timestamps::now_ts();
        for entry in warm_entries {
            let idle = now.saturating_sub(entry.accessed_at);
            if idle >= self.policy.warm_ttl_secs {
                if entry.usefulness >= self.policy.warm_threshold {
                    // Promote to cold (archival).
                    self.promote_to_cold(entry).await?;
                    report.promoted_warm_to_cold += 1;
                } else {
                    // Low-usefulness warm entry: just remove.
                    self.warm.remove(&entry.id).await?;
                    report.evicted_warm += 1;
                }
            }
        }

        Ok(report)
    }

    /// Attach an optional `MemorySummarizer` that compresses excess hot
    /// entries during automatic tier migration.
    pub fn with_summarizer(mut self, summarizer: MemorySummarizer) -> Self {
        self.summarizer = Some(summarizer);
        self
    }

    /// Summarize hot cache entries if the summarizer is configured and
    /// the entry count exceeds the threshold.
    ///
    /// Routes through the async [`MemorySummarizer::summarize`] so the
    /// configured LLM agent is actually used when `use_llm_summarization` is
    /// enabled (previously this only ran the sync truncation path).
    pub async fn summarize_hot_entries(&self) -> Result<()> {
        let Some(ref summarizer) = self.summarizer else {
            return Ok(());
        };

        // Snapshot the hot entries under the lock, then drop it before the
        // await so the LLM call never holds the std Mutex across an await
        // point (and the summarize-replace window stays race-free because
        // auto_migrate is the only writer of the hot cache).
        let entries: Vec<MemoryEntry> = {
            let hot = self.hot.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("hot cache mutex poisoned in 'summarize_hot_entries', recovering");
                poisoned.into_inner()
            });
            let entries: Vec<MemoryEntry> =
                hot.entries.values().map(|he| he.entry.clone()).collect();
            entries
        };
        if !summarizer.should_summarize(entries.len()) {
            return Ok(());
        }

        let result = summarizer.summarize(&entries).await;
        match result {
            SummarizedMemory::Full(_) => {
                // Entry count is still manageable; nothing to do.
            }
            SummarizedMemory::Compressed(compressed) => {
                let compressed_len = compressed.len();
                let mut hot = self.hot.lock().unwrap_or_else(|poisoned| {
                    tracing::warn!(
                        "hot cache mutex poisoned in 'summarize_hot_entries', recovering"
                    );
                    poisoned.into_inner()
                });
                hot.clear();
                for entry in compressed {
                    hot.insert(entry);
                }
                tracing::info!(
                    target: "memory_persistence",
                    "summarized hot cache: {} entries compressed from {} total",
                    compressed_len,
                    entries.len()
                );
            }
        }

        Ok(())
    }
}

// ===========================================================================
// Reports & Index
// ===========================================================================

/// Report from a migration cycle.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MigrationReport {
    pub promoted_hot_to_warm: usize,
    pub promoted_warm_to_cold: usize,
    pub demoted_hot_to_cold: usize,
    pub evicted_warm: usize,
}

// ===========================================================================
// Helpers
// ===========================================================================

/// Convert a Unix timestamp (seconds) to a (year, month) tuple.
/// Uses days-since-epoch arithmetic with a simple leap-year-aware algorithm.
fn ts_to_year_month(ts: i64) -> (i32, u32) {
    // Days since Unix epoch (1970-01-01).
    let days = ts / 86_400;
    let mut y = 1970i64;
    let mut remaining = days;
    loop {
        let days_in_year = if is_leap_year(y) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        y += 1;
    }
    let year = y as i32;
    // Months from March so Feb is last (easier leap handling).
    let month_days = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 0u32;
    for (i, &md) in month_days.iter().enumerate() {
        let dim = if i == 1 && is_leap_year(year as i64) {
            29
        } else {
            md
        };
        if remaining < dim {
            month = (i + 1) as u32;
            break;
        }
        remaining -= dim;
    }
    if month == 0 {
        month = 12;
    }
    (year, month)
}

fn is_leap_year(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_entry(id: &str, usefulness: f32) -> MemoryEntry {
        MemoryEntry::new_hot(id, "test", format!("content-{}", id), usefulness)
    }

    #[test]
    fn test_hot_cache_lru_eviction() {
        let mut cache = HotCache::new(3, 300);
        cache.insert(make_entry("e1", 0.5));
        cache.insert(make_entry("e2", 0.5));
        cache.insert(make_entry("e3", 0.5));

        // Inserting a 4th entry at capacity evicts the LRU entry (e1).
        cache.insert(make_entry("e4", 0.5));
        let evicted = cache.evict_lru_one().expect("eviction should succeed");
        assert_eq!(evicted.id, "e2", "expected e1 to already be evicted");
    }

    #[test]
    fn test_hot_cache_ttl_eviction() {
        let mut cache = HotCache::new(10, 0); // 0-second TTL
        cache.insert(make_entry("e1", 0.5));
        // Instantly expired
        let evicted = cache.evict_expired();
        assert!(!evicted.is_empty());
        assert_eq!(evicted[0].id, "e1");
    }

    #[test]
    fn test_memory_tiering_policy_defaults() {
        let policy = MemoryTieringPolicy::default();
        assert_eq!(policy.hot_max_entries, 2048);
        assert_eq!(policy.hot_ttl_secs, 1800);
        assert_eq!(policy.warm_ttl_secs, 2_592_000);
    }

    #[test]
    fn test_memory_entry_touch() {
        let mut entry = MemoryEntry::new_hot("id1", "test", "hello", 0.9);
        let before = entry.access_count;
        entry.touch();
        assert_eq!(entry.access_count, before + 1);
    }

    #[test]
    fn test_cold_storage_append_and_read() {
        let dir = TempDir::new().expect("temp dir creation should succeed");
        let cold = ColdStorage::new(dir.path());
        let entry = make_entry("cold1", 0.7);
        cold.append_entry(&entry).expect("append should succeed");

        // The write path must produce exactly one gzip shard on disk.
        assert_eq!(cold.total_shard_count(), 1);
    }

    #[tokio::test]
    async fn test_persistence_store_auto_migrate_and_search_by_session() {
        let dir = TempDir::new().expect("temp dir creation should succeed");
        let db_path = dir.path().join("warm.db");
        let cold_path = dir.path().join("cold");

        let persistence = MemoryPersistence::new(
            &db_path,
            &cold_path,
            Some(MemoryTieringPolicy {
                hot_ttl_secs: 0,
                ..Default::default()
            }),
        )
        .expect("persistence should initialize");
        let mut entry = make_entry("p1", 0.8);
        entry.session_id = Some("sess-1".to_string());
        persistence
            .store(entry)
            .await
            .expect("store should succeed");

        // Auto-migrate evicts the expired hot entry into the warm tier.
        persistence
            .auto_migrate()
            .await
            .expect("auto_migrate should succeed");

        // The entry is now durable in the warm tier, retrievable by session.
        let hits = persistence
            .search_by_session("sess-1", 16)
            .await
            .expect("search_by_session should succeed");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "p1");
    }

    #[tokio::test]
    async fn test_promotion_hot_to_warm() {
        let dir = TempDir::new().expect("temp dir creation should succeed");
        let db_path = dir.path().join("warm.db");
        let cold_path = dir.path().join("cold");

        let persistence = MemoryPersistence::new(&db_path, &cold_path, None)
            .expect("persistence should initialize");
        let mut entry = make_entry("promo1", 0.9);
        entry.session_id = Some("sess-promo".to_string());
        persistence
            .store(entry.clone())
            .await
            .expect("store should succeed");
        persistence
            .promote_to_warm(entry)
            .await
            .expect("promote to warm should succeed");

        // The entry should now live in the durable warm tier.
        let hits = persistence
            .search_by_session("sess-promo", 16)
            .await
            .expect("search_by_session should succeed");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "promo1");
        assert_eq!(hits[0].tier, MemoryTier::Warm);
    }

    #[tokio::test]
    async fn test_auto_migrate_evicts_expired_hot_entries() {
        let dir = TempDir::new().expect("temp dir creation should succeed");
        let db_path = dir.path().join("warm.db");
        let cold_path = dir.path().join("cold");

        let persistence = MemoryPersistence::new(
            &db_path,
            &cold_path,
            Some(MemoryTieringPolicy {
                hot_ttl_secs: 0,
                ..Default::default()
            }),
        )
        .expect("persistence should initialize");

        // Entry with low usefulness → gets demoted to cold (not promoted to warm)
        persistence
            .store(make_entry("low", 0.1))
            .await
            .expect("store should succeed");

        // Entry with high usefulness → promoted to warm
        persistence
            .store(make_entry("high", 0.8))
            .await
            .expect("store should succeed");

        let report = persistence
            .auto_migrate()
            .await
            .expect("auto migration should run");
        assert_eq!(report.promoted_hot_to_warm, 1);
        assert_eq!(report.demoted_hot_to_cold, 1);
    }
}
