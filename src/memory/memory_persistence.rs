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

#[cfg(all(not(feature = "backend-sqlite"), feature = "backend-postgres"))]
use crate::memory::pg_pool::{connect_postgres, create_pool, pool_get, resolve_pg_dsn, PgPoolPair};
use flate2::read::MultiGzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use futures_util::stream::{self, StreamExt};
use indexmap::IndexSet;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
#[cfg(feature = "backend-sqlite")]
use std::sync::Arc;
use std::sync::Mutex;
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
}

// ===========================================================================
// L3: Cold Tier — gzip NDJSON on disk
// ===========================================================================

/// Manages cold storage: `.goon/memory/cold/YYYY-MM/*.ndjson.gz`
#[derive(Debug)]
struct ColdStorage {
    base_path: PathBuf,
    max_shard_size_bytes: u64,
    max_total_shards: usize,
    /// Cached total shard count across all months. Computed once with a
    /// single directory scan, then maintained incrementally by append/evict
    /// so the write path never re-scans the tree.
    shard_count: Mutex<Option<usize>>,
    /// Cached highest existing shard index per year-month (key: `y*100+m`),
    /// avoiding the previous per-append stat() loop from index 0.
    latest_shard_idx: Mutex<Option<(u32, i64)>>,
}

impl ColdStorage {
    fn new(base_path: &Path) -> Self {
        Self {
            base_path: base_path.to_path_buf(),
            max_shard_size_bytes: 10 * 1024 * 1024, // 10 MB default
            max_total_shards: 100,
            shard_count: Mutex::new(None),
            latest_shard_idx: Mutex::new(None),
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

    /// Find the next free shard index within the given year-month directory.
    ///
    /// Uses a per-month cache of the highest existing shard index: the first
    /// write into a month pays one directory listing, subsequent writes are
    /// O(1). Previously every append stat()ed candidate paths from index 0,
    /// making the write path linear in the shard count.
    fn next_shard_index(&self, year: i32, month: u32) -> u32 {
        let ym_key = (year as u32) * 100 + month;
        let mut cache = self.latest_shard_idx.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("cold storage shard cache lock poisoned, recovering");
            poisoned.into_inner()
        });
        let start = match *cache {
            Some((cached_ym, last)) if cached_ym == ym_key => last + 1,
            _ => {
                // First write into this month: scan the directory once and
                // cache the highest existing shard index (-1 when empty).
                let mut max_idx: i64 = -1;
                if let Ok(entries) = fs::read_dir(self.month_dir(year, month)) {
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if let Some(idx) = name
                            .strip_suffix(".ndjson.gz")
                            .and_then(|stem| stem.rsplit('-').next())
                            .and_then(|s| s.parse::<i64>().ok())
                        {
                            max_idx = max_idx.max(idx);
                        }
                    }
                }
                *cache = Some((ym_key, max_idx));
                max_idx + 1
            }
        };
        // Verify the cached guess (files may have been removed externally,
        // e.g. by shard eviction) and scan forward only when it is stale.
        let mut idx = start.max(0) as u32;
        while self
            .shard_path(year, month, &format!("{:04}-{:02}-{:03}", year, month, idx))
            .exists()
        {
            idx += 1;
        }
        *cache = Some((ym_key, idx as i64 - 1));
        idx
    }

    /// Count existing shard files under the base path.
    ///
    /// Cached: the first call performs one recursive directory scan;
    /// subsequent calls return the cached value maintained by
    /// `append_entry` / `evict_oldest_shards`.
    fn total_shard_count(&self) -> usize {
        let mut cache = self.shard_count.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("cold storage shard count lock poisoned, recovering");
            poisoned.into_inner()
        });
        if let Some(count) = *cache {
            return count;
        }
        let mut count = 0;
        if self.base_path.exists() {
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
        }
        *cache = Some(count);
        count
    }

    /// Adjust the cached shard count after a new shard file was created.
    fn note_shard_created(&self) {
        if let Ok(mut cache) = self.shard_count.lock() {
            if let Some(count) = cache.as_mut() {
                *count += 1;
            }
        }
    }

    /// Adjust the cached shard count after shard files were evicted.
    fn note_shards_removed(&self, count: usize) {
        if count == 0 {
            return;
        }
        if let Ok(mut cache) = self.shard_count.lock() {
            if let Some(current) = cache.as_mut() {
                *current = current.saturating_sub(count);
            }
        }
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
        let shard_index = self.next_shard_index(year, month);
        let shard = if shard_index == 0 {
            // No shard exists yet; start with index 0. This is also a NEW
            // shard file, so bump the cached count (previously only the
            // rollover path called note_shard_created, leaving the count one
            // behind after the cache was primed by a scan).
            self.note_shard_created();
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
                self.note_shard_created();
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
        let mut removed = 0usize;
        for shard in shards.into_iter().take(count) {
            if fs::remove_file(&shard).is_ok() {
                removed += 1;
            }
        }
        self.note_shards_removed(removed);
    }
}

/// Collect every shard file under a cold storage base path (all year-month
/// directories), sorted by path.
fn collect_shard_paths(base_path: &Path) -> Vec<PathBuf> {
    let mut shards = Vec::new();
    if let Ok(dir_iter) = fs::read_dir(base_path) {
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
    shards.sort();
    shards
}

/// Read every entry stored in one cold shard file. Shards are written as a
/// sequence of gzip members (one per append), so the whole file must be
/// decoded as a multi-member stream.
fn read_shard_entries(path: &Path) -> Result<Vec<MemoryEntry>> {
    let file = fs::File::open(path).context("failed to open cold storage shard")?;
    let decoder = MultiGzDecoder::new(file);
    let mut entries = Vec::new();
    for line in std::io::BufReader::new(decoder).lines() {
        let line = line.context("failed to read cold storage shard line")?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<MemoryEntry>(line) {
            Ok(entry) => entries.push(entry),
            Err(e) => {
                tracing::warn!(
                    "cold storage: skipping malformed entry line in {}: {}",
                    path.display(),
                    e
                );
            }
        }
    }
    Ok(entries)
}

/// Find all cold entries belonging to a session, most recently accessed first.
///
/// This is the **production** cold-tier recovery path: `search_by_session`
/// falls back here when the warm tier returns fewer than the requested limit.
/// The former single-entry precise lookup (`get_from_cold` /
/// `load_entry_from_cold`) had zero production callers — all recovery is
/// session-scoped — so it was removed. No per-entry sidecar index is
/// maintained (a previous `ColdStorageIndex` was write-only): cold shards are
/// a sequence of gzip members that cannot be seeked, so a session lookup must
/// decode the shards and scan them anyway.
fn find_cold_entries_by_session(base: &Path, session_id: &str) -> Result<Vec<MemoryEntry>> {
    let mut hits = Vec::new();
    for path in collect_shard_paths(base) {
        if !path.exists() {
            continue;
        }
        for entry in read_shard_entries(&path)? {
            if entry.session_id.as_deref() == Some(session_id) {
                hits.push(entry);
            }
        }
    }
    hits.sort_by_key(|b| std::cmp::Reverse(b.accessed_at));
    Ok(hits)
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

// ---- Shared upsert machinery -------------------------------------------------
// The warm-store upsert business logic (field preparation, SQL templates,
// over-cap eviction) is shared across backends; only the placeholder style,
// parameter binding and row reads differ. Those backend-specific bits live in
// the two `upsert_exec` helpers below.

/// Field values bound into the warm_memory upsert statement. Prepared once
/// outside `spawn_blocking` so the closure only touches plain data.
struct UpsertPayload {
    id: String,
    tier_label: String,
    class: String,
    content: String,
    created_at: i64,
    accessed_at: i64,
    usefulness: f32,
    embedding_json: Option<String>,
    access_count: i64,
    session_id: Option<String>,
    user_id: Option<String>,
}

impl UpsertPayload {
    fn from_entry(entry: &MemoryEntry) -> Self {
        Self {
            id: entry.id.clone(),
            tier_label: entry.tier.label().to_string(),
            class: entry.class.clone(),
            content: entry.content.clone(),
            created_at: entry.created_at,
            accessed_at: entry.accessed_at,
            usefulness: entry.usefulness,
            embedding_json: entry
                .embedding
                .as_ref()
                .map(|v| serde_json::to_string(v).unwrap_or_default()),
            access_count: entry.access_count,
            session_id: entry.session_id.clone(),
            user_id: entry.user_id.clone(),
        }
    }
}

/// INSERT ... ON CONFLICT(id) DO UPDATE ... — identical SQL for both
/// backends; only the placeholder prefix differs (`?` vs `$n`).
fn upsert_sql() -> String {
    format!(
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
    )
}

fn count_over_cap_sql() -> String {
    format!("SELECT COUNT(*) - {p}1 FROM warm_memory", p = PARAM_PREFIX)
}

fn evict_over_cap_sql() -> String {
    format!(
        "DELETE FROM warm_memory WHERE id IN (
        SELECT id FROM warm_memory ORDER BY accessed_at ASC \
        LIMIT {p}1
    )",
        p = PARAM_PREFIX
    )
}

/// Execute the upsert against a SQLite connection, evicting the LRU warm
/// entries only when the table actually exceeds `max_entries` (the
/// DELETE+ORDER BY full-table sort is skipped on every normal write).
#[cfg(feature = "backend-sqlite")]
fn upsert_exec(
    conn: &rusqlite::Connection,
    payload: &UpsertPayload,
    max_entries: usize,
) -> Result<()> {
    conn.execute(
        &upsert_sql(),
        rusqlite::params![
            &payload.id,
            &payload.tier_label,
            &payload.class,
            &payload.content,
            payload.created_at,
            payload.accessed_at,
            payload.usefulness,
            &payload.embedding_json,
            payload.access_count,
            &payload.session_id,
            &payload.user_id,
        ],
    )?;
    let over_cap: i64 = conn.query_row(
        &count_over_cap_sql(),
        rusqlite::params![max_entries as i64],
        |row| row.get(0),
    )?;
    if over_cap > 0 {
        conn.execute(&evict_over_cap_sql(), rusqlite::params![over_cap])?;
    }
    Ok(())
}

/// PostgreSQL variant of [`upsert_exec`]; identical business logic, only the
/// binding style (`&[...]` instead of `params![]`) and row read differ.
#[cfg(all(not(feature = "backend-sqlite"), feature = "backend-postgres"))]
fn upsert_exec(
    conn: &mut postgres::Client,
    payload: &UpsertPayload,
    max_entries: usize,
) -> Result<()> {
    conn.execute(
        &upsert_sql(),
        &[
            &payload.id,
            &payload.tier_label,
            &payload.class,
            &payload.content,
            &payload.created_at,
            &payload.accessed_at,
            &payload.usefulness,
            &payload.embedding_json,
            &payload.access_count,
            &payload.session_id,
            &payload.user_id,
        ],
    )?;
    let row = conn.query_one(&count_over_cap_sql(), &[&(max_entries as i64)])?;
    let over_cap: i64 = row.get(0);
    if over_cap > 0 {
        conn.execute(&evict_over_cap_sql(), &[&over_cap])?;
    }
    Ok(())
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
    /// `None` when no PostgreSQL connection string was configured or the
    /// connection/DDL failed — the warm tier then acts as a no-op facade so
    /// the caller chain never panics (see `WarmStore::new`).
    pool: Option<PgPoolPair>,
    max_entries: usize,
}

#[cfg(all(not(feature = "backend-sqlite"), feature = "backend-postgres"))]
impl std::fmt::Debug for WarmStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WarmStore")
            .field("max_entries", &self.max_entries)
            .field("enabled", &self.pool.is_some())
            .finish()
    }
}

impl WarmStore {
    #[cfg(feature = "backend-sqlite")]
    /// Whether the warm tier is active. Always true for the sqlite backend.
    fn is_enabled(&self) -> bool {
        true
    }

    #[cfg(all(not(feature = "backend-sqlite"), feature = "backend-postgres"))]
    /// Whether the warm tier is active (postgres pool established).
    fn is_enabled(&self) -> bool {
        self.pool.is_some()
    }

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
    /// Create the warm store under the `backend-postgres` build.
    ///
    /// The `db_conn_str` argument is a **SQLite file path** produced by the
    /// production call sites (`memory_base_path().join("warm.db")`) and is
    /// meaningless as a PostgreSQL DSN — passing it to `PgClient::connect`
    /// made warm-tier construction fail (or panic via `.expect` in the server
    /// recovery path) on every postgres build. The real connection string is
    /// resolved through the shared `pg_pool::resolve_pg_dsn` resolver (config
    /// `connection_string` → `GO_ON_PG_CONNECTION_STRING` → `DATABASE_URL` →
    /// `PG_DSN` → `GO_ON_DATABASE_URL`), the same single source the cache and
    /// vector store use.
    ///
    /// When no connection string is available, or the connection/DDL fails,
    /// the store degrades to a no-op facade (`pool: None`) with a clear
    /// warning instead of returning an error — the callers in `src/acp` retry
    /// once and then `.expect()`, so an `Err` here would panic the server.
    /// All business methods short-circuit on `pool: None` (see below).
    ///
    /// Connections do **not** come from a shared pool: the warm tier builds
    /// its own independent pool (max 4, created lazily via `create_pool`)
    /// rather than sharing the pools cache.rs / vector.rs construct. Only the
    /// DSN *resolver* (`resolve_pg_dsn`) is shared infrastructure. The
    /// independence is deliberate: warm-tier traffic (and its max-4 connection
    /// cap) stays isolated from the cache/vector pools so warm-tier
    /// backpressure or connection churn cannot starve the other stores, and
    /// warm-store startup is not coupled to whichever store boots first.
    fn new(_db_conn_str: &Path, max_entries: usize) -> Result<Self> {
        let disabled = || {
            tracing::warn!(
                "warm tier disabled under backend-postgres: no reachable PostgreSQL connection string (set GO_ON_PG_CONNECTION_STRING, DATABASE_URL, PG_DSN or GO_ON_DATABASE_URL) — warm tier is a no-op; cold tier still works"
            );
            Ok(Self {
                pool: None,
                max_entries,
            })
        };

        let Some(dsn) = resolve_pg_dsn(None) else {
            return disabled();
        };
        let write_dsn = dsn.clone();
        // The pool creates connections lazily on first `pool_get`; a failed
        // connect surfaces as a `pool_get` error below and degrades to the
        // no-op facade, preserving the previous semantics.
        let write_pool = create_pool(move || connect_postgres(&write_dsn), 4);
        let mut client = match pool_get(&write_pool) {
            Ok(client) => client,
            Err(e) => {
                tracing::warn!(
                    "warm tier disabled under backend-postgres: failed to connect to PostgreSQL: {e} — warm tier is a no-op; cold tier still works"
                );
                return disabled();
            }
        };
        if let Err(e) = client.batch_execute(
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
        ) {
            tracing::warn!(
                "warm tier disabled under backend-postgres: failed to prepare warm_memory schema: {e} — warm tier is a no-op; cold tier still works"
            );
            return disabled();
        }

        Ok(Self {
            pool: Some(PgPoolPair {
                write: write_pool.clone(),
                read: write_pool,
            }),
            max_entries,
        })
    }

    // ---- Unified business methods --------------------------------------------

    async fn upsert(&self, entry: &MemoryEntry) -> Result<()> {
        #[cfg(feature = "backend-sqlite")]
        let _permit = crate::shared::db_pool::acquire_db_permit().await;
        #[cfg(feature = "backend-sqlite")]
        let conn = self.conn.clone();
        #[cfg(all(not(feature = "backend-sqlite"), feature = "backend-postgres"))]
        let pool = match self.pool.as_ref() {
            Some(pool) => pool.write.clone(),
            // No-op facade: no reachable PostgreSQL connection (see `new`).
            None => return Ok(()),
        };
        let max_entries = self.max_entries;
        // Prepare the bound fields outside the blocking closure.
        let payload = UpsertPayload::from_entry(entry);
        spawn_blocking(move || {
            #[cfg(all(not(feature = "backend-sqlite"), feature = "backend-postgres"))]
            let mut conn = pool_get(&pool)?;
            #[cfg(feature = "backend-sqlite")]
            let conn = conn.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("warm store mutex poisoned, recovering");
                poisoned.into_inner()
            });
            // Business logic (SQL templates, insert, over-cap eviction) is
            // shared; only the backend binding/row-read differs (sqlite takes
            // `&Connection`, postgres takes `&mut Client`).
            let upsert_result: Result<()> = {
                #[cfg(feature = "backend-sqlite")]
                {
                    upsert_exec(&conn, &payload, max_entries)
                }
                #[cfg(all(not(feature = "backend-sqlite"), feature = "backend-postgres"))]
                {
                    upsert_exec(&mut conn, &payload, max_entries)
                }
            };
            upsert_result
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))?
    }

    async fn remove(&self, id: &str) -> Result<bool> {
        #[cfg(feature = "backend-sqlite")]
        let _permit = crate::shared::db_pool::acquire_db_permit().await;
        #[cfg(feature = "backend-sqlite")]
        let conn = self.conn.clone();
        #[cfg(all(not(feature = "backend-sqlite"), feature = "backend-postgres"))]
        let pool = match self.pool.as_ref() {
            Some(pool) => pool.write.clone(),
            None => return Ok(false),
        };
        let id = id.to_string();
        spawn_blocking(move || {
            #[cfg(all(not(feature = "backend-sqlite"), feature = "backend-postgres"))]
            let mut conn = pool_get(&pool)?;
            #[cfg(feature = "backend-sqlite")]
            let conn = conn.lock().unwrap_or_else(|poisoned| {
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

    /// Fetch warm entries whose `accessed_at` is older than `before_ts`
    /// (i.e. idle >= warm TTL). The filter is pushed into the query so the
    /// full table is never loaded just to discard non-expired rows (the
    /// previous `iterate_all` + in-memory filter pulled every warm row).
    async fn iterate_expiring(&self, before_ts: i64) -> Result<Vec<MemoryEntry>> {
        #[cfg(feature = "backend-sqlite")]
        let _permit = crate::shared::db_pool::acquire_db_permit().await;
        #[cfg(feature = "backend-sqlite")]
        let conn = self.conn.clone();
        #[cfg(all(not(feature = "backend-sqlite"), feature = "backend-postgres"))]
        let pool = match self.pool.as_ref() {
            Some(pool) => pool.write.clone(),
            None => return Ok(Vec::new()),
        };
        spawn_blocking(move || {
            #[cfg(all(not(feature = "backend-sqlite"), feature = "backend-postgres"))]
            let mut conn = pool_get(&pool)?;
            #[cfg(feature = "backend-sqlite")]
            let conn = conn
                .lock()
                .map_err(|e| anyhow::anyhow!("warm store mutex poisoned: {}", e))?;
            let sql = format!(
                "SELECT {} FROM warm_memory WHERE accessed_at <= {p}1",
                WARM_MEMORY_COLUMNS,
                p = PARAM_PREFIX
            );
            #[cfg(feature = "backend-sqlite")]
            {
                query_all(&conn, &sql, &[&before_ts])
            }
            #[cfg(not(feature = "backend-sqlite"))]
            {
                query_all(&mut conn, &sql, &[&before_ts])
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
        #[cfg(feature = "backend-sqlite")]
        let conn = self.conn.clone();
        #[cfg(all(not(feature = "backend-sqlite"), feature = "backend-postgres"))]
        let pool = match self.pool.as_ref() {
            Some(pool) => pool.write.clone(),
            None => return Ok(Vec::new()),
        };
        let session_id = session_id.to_string();
        spawn_blocking(move || {
            #[cfg(all(not(feature = "backend-sqlite"), feature = "backend-postgres"))]
            let mut conn = pool_get(&pool)?;
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
    /// Whether the warm tier is active (always false for the stub backend).
    fn is_enabled(&self) -> bool {
        false
    }

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

    fn search_by_session(&self, _session_id: &str, _limit: usize) -> Result<Vec<MemoryEntry>> {
        Err(anyhow::anyhow!(
            "No storage backend configured: enable backend-sqlite or backend-postgres feature"
        ))
    }

    fn iterate_expiring(&self, _before_ts: i64) -> Result<Vec<MemoryEntry>> {
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
    /// * `db_path` - Path to the SQLite warm store database. Ignored under the
    ///   `backend-postgres` build — the warm tier then resolves the real
    ///   connection string through `pg_pool::resolve_pg_dsn` (config
    ///   `connection_string` → `GO_ON_PG_CONNECTION_STRING` → `DATABASE_URL` →
    ///   `PG_DSN` → `GO_ON_DATABASE_URL`) and degrades to a no-op when none is
    ///   reachable.
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
        // When the warm tier is disabled (e.g. backend-postgres without a
        // reachable connection string — see `WarmStore::new`), degrade to the
        // cold tier instead of silently dropping the entry: the cold store is
        // file-backed and always available, so the memory is never lost.
        if !self.warm.is_enabled() {
            tracing::warn!(
                "warm tier unavailable (postgres no-op facade); promoting to cold instead"
            );
            return self.promote_to_cold(entry).await;
        }
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

    /// Search the durable warm tier — with cold-tier fallback — for entries
    /// belonging to a session, most recently accessed first. This is the read
    /// side of the persistence layer — used by `session/load` and
    /// `session/resume` to restore a session's memory context.
    ///
    /// When the warm tier returns fewer than `limit` entries, the cold
    /// archival tier is scanned so long-term memories remain recoverable
    /// after their warm-tier retention expires. The cold scan decodes every
    /// shard (gzip), so it only runs when warm results are short of the limit.
    pub async fn search_by_session(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>> {
        // Hot tier first: `store()` writes entries into the hot cache and they
        // are only promoted to warm by `auto_migrate` (TTL-based, default
        // 30 minutes). Without this merge, freshly stored memories would be
        // invisible to `session/load` until the next migration cycle.
        let mut hits: Vec<MemoryEntry> = {
            let hot = self.hot.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("hot cache mutex poisoned in 'search_by_session', recovering");
                poisoned.into_inner()
            });
            let mut entries: Vec<MemoryEntry> = hot
                .entries
                .values()
                .filter(|he| he.entry.session_id.as_deref() == Some(session_id))
                .map(|he| he.entry.clone())
                .collect();
            entries.sort_by_key(|b| std::cmp::Reverse(b.accessed_at));
            entries.truncate(limit);
            entries
        };

        // Warm tier second, deduplicating against hot results (hot wins on
        // id conflicts since it holds the freshest copy).
        if hits.len() < limit {
            let mut seen: HashSet<String> = hits.iter().map(|h| h.id.clone()).collect();
            for entry in self.warm.search_by_session(session_id, limit).await? {
                if hits.len() >= limit {
                    break;
                }
                if seen.insert(entry.id.clone()) {
                    hits.push(entry);
                }
            }
        }

        // Cold tier last, only when the upper tiers fell short of the limit.
        if hits.len() < limit {
            let base = self.cold.base_path.clone();
            let sid = session_id.to_string();
            let cold_entries = spawn_blocking(move || find_cold_entries_by_session(&base, &sid))
                .await
                .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))??;
            let mut seen: HashSet<String> = hits.iter().map(|h| h.id.clone()).collect();
            for entry in cold_entries {
                if hits.len() >= limit {
                    break;
                }
                if seen.insert(entry.id.clone()) {
                    hits.push(entry);
                }
            }
        }

        // Final global sort: hot and warm tiers (and cold when scanned) are
        // merged in tier order above; sort once here so the combined result is
        // truly most-recently-accessed-first regardless of which tiers matched.
        hits.sort_by_key(|b| std::cmp::Reverse(b.accessed_at));
        hits.truncate(limit);
        Ok(hits)
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

        // Parallelize tier promotions: each call runs spawn_blocking + a
        // SQLite write, so buffer at most 8 in flight to avoid exhausting the
        // blocking pool (previously every entry was awaited sequentially).
        let mut hot_stream = stream::iter(evicted)
            .map(|entry| {
                let promote_warm = entry.usefulness >= self.policy.hot_threshold;
                async move {
                    if promote_warm {
                        self.promote_to_warm(entry).await?;
                        Ok::<MigrationStep, anyhow::Error>(MigrationStep::PromotedToWarm)
                    } else {
                        self.promote_to_cold(entry).await?;
                        Ok::<MigrationStep, anyhow::Error>(MigrationStep::DemotedToCold)
                    }
                }
            })
            .buffer_unordered(8);
        while let Some(result) = hot_stream.next().await {
            match result? {
                MigrationStep::PromotedToWarm => report.promoted_hot_to_warm += 1,
                MigrationStep::DemotedToCold => report.demoted_hot_to_cold += 1,
                _ => {}
            }
        }

        // ── Step 2: Check warm for TTL candidates ──
        // The idle filter is pushed down into the SQL query
        // (`iterate_expiring`) instead of loading the whole warm table and
        // filtering in memory — with 100k warm entries the full scan only
        // served to throw away non-expired rows.
        let now = crate::shared::timestamps::now_ts();
        let ttl_candidates = self
            .warm
            .iterate_expiring(now.saturating_sub(self.policy.warm_ttl_secs))
            .await?;
        let mut warm_stream = stream::iter(ttl_candidates)
            .map(|entry| {
                let promote_cold = entry.usefulness >= self.policy.warm_threshold;
                async move {
                    if promote_cold {
                        // Promote to cold (archival).
                        self.promote_to_cold(entry).await?;
                        Ok::<MigrationStep, anyhow::Error>(MigrationStep::PromotedToCold)
                    } else {
                        // Low-usefulness warm entry: just remove.
                        self.warm.remove(&entry.id).await?;
                        Ok::<MigrationStep, anyhow::Error>(MigrationStep::EvictedWarm)
                    }
                }
            })
            .buffer_unordered(8);
        while let Some(result) = warm_stream.next().await {
            match result? {
                MigrationStep::PromotedToCold => report.promoted_warm_to_cold += 1,
                MigrationStep::EvictedWarm => report.evicted_warm += 1,
                _ => {}
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
        // point. Entries stored after the snapshot are NOT summarized and must
        // be preserved: we only remove the snapshot's ids below, so entries
        // written by `store()` while the LLM call is in flight survive.
        let (snapshot_ids, entries): (Vec<String>, Vec<MemoryEntry>) = {
            let hot = self.hot.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("hot cache mutex poisoned in 'summarize_hot_entries', recovering");
                poisoned.into_inner()
            });
            let ids = hot.entries.keys().cloned().collect();
            let entries = hot.entries.values().map(|he| he.entry.clone()).collect();
            (ids, entries)
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
                // Remove only the entries that were part of the snapshot; any
                // entry stored while the summarizer ran is left untouched.
                for id in &snapshot_ids {
                    hot.remove(id);
                }
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

/// Intermediate result of a single tier-migration step, used to aggregate the
/// [`MigrationReport`] after a parallel migration batch completes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MigrationStep {
    PromotedToWarm,
    DemotedToCold,
    PromotedToCold,
    EvictedWarm,
}

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
///
/// Delegates to the single canonical epoch→date conversion
/// (`crate::security::security_advisor::unix_ts_to_ymd`, Hinnant
/// civil-from-days) — the previous day-loop implementation had its own leap
/// handling that could disagree with the other two date converters at month
/// boundaries.
fn ts_to_year_month(ts: i64) -> (i32, u32) {
    let (year, month, _) = crate::security::security_advisor::unix_ts_to_ymd(ts);
    (year as i32, month as u32)
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

    /// Month-boundary coverage for the cold-storage shard partitioning. Uses
    /// the shared epoch→date conversion (Hinnant) — the old day-loop
    /// implementation could disagree at leap/month boundaries.
    #[test]
    fn test_ts_to_year_month_boundaries() {
        assert_eq!(ts_to_year_month(0), (1970, 1));
        assert_eq!(ts_to_year_month(1_704_067_200), (2024, 1));
        // 2024-02-29 00:00:00 UTC (leap day).
        assert_eq!(ts_to_year_month(1_709_164_800), (2024, 2));
        // One second before 2024-03-01 still belongs to February.
        assert_eq!(ts_to_year_month(1_709_251_199), (2024, 2));
        assert_eq!(ts_to_year_month(1_709_251_200), (2024, 3));
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
        let mut entry = make_entry("cold1", 0.7);
        entry.user_id = Some("u1".to_string());
        entry.session_id = Some("sess-arch".to_string());
        // Mirror `promote_to_cold`: the archival entry carries the Cold tier.
        entry.tier = MemoryTier::Cold;
        cold.append_entry(&entry).expect("append should succeed");

        // The write path must produce exactly one gzip shard on disk.
        assert_eq!(cold.total_shard_count(), 1);

        // The cold read path (production: session-scoped) must recover the
        // entry, so cold is a real archival tier rather than write-only
        // storage. The former user+id precise lookup was removed with
        // `get_from_cold` — session recovery is the canonical path.
        let hits = find_cold_entries_by_session(&cold.base_path, "sess-arch")
            .expect("session read should succeed");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "cold1");
        assert_eq!(hits[0].content, "content-cold1");
        assert_eq!(hits[0].tier, MemoryTier::Cold);

        // A miss (unknown session) returns nothing.
        assert!(
            find_cold_entries_by_session(&cold.base_path, "sess-unknown")
                .expect("session read should succeed")
                .is_empty()
        );
    }

    #[test]
    fn test_cold_storage_find_by_session() {
        let dir = TempDir::new().expect("temp dir creation should succeed");
        let cold = ColdStorage::new(dir.path());
        let mut entry = make_entry("sess-entry", 0.4);
        entry.session_id = Some("sess-cold".to_string());
        cold.append_entry(&entry).expect("append should succeed");

        let hits = find_cold_entries_by_session(&cold.base_path, "sess-cold")
            .expect("find_by_session should succeed");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "sess-entry");
        assert!(find_cold_entries_by_session(&cold.base_path, "other")
            .expect("find_by_session should succeed")
            .is_empty());
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
        let mut low = make_entry("low", 0.1);
        low.session_id = Some("sess-low".to_string());
        persistence.store(low).await.expect("store should succeed");

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

        // The cold-tier write must be readable back: the demoted entry is
        // recoverable via the session-scoped archival read path (the warm tier
        // has no row for this session, so search falls back to cold).
        let recovered = persistence
            .search_by_session("sess-low", 16)
            .await
            .expect("search_by_session should succeed");
        assert_eq!(recovered.len(), 1, "cold entry should be recoverable");
        assert_eq!(recovered[0].id, "low");
        assert!(persistence
            .search_by_session("sess-unknown", 16)
            .await
            .expect("search_by_session should succeed")
            .is_empty());
    }

    #[tokio::test]
    async fn test_search_by_session_falls_back_to_cold() {
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

        // Low-usefulness entry: auto-migrate demotes it straight to cold.
        let mut entry = make_entry("cold-sess", 0.1);
        entry.session_id = Some("sess-cold2".to_string());
        persistence
            .store(entry)
            .await
            .expect("store should succeed");
        persistence
            .auto_migrate()
            .await
            .expect("auto_migrate should succeed");

        // The warm tier has no row for this session; search must fall back to
        // the cold tier and still restore the memory.
        let hits = persistence
            .search_by_session("sess-cold2", 16)
            .await
            .expect("search_by_session should succeed");
        assert_eq!(hits.len(), 1, "cold fallback should restore session memory");
        assert_eq!(hits[0].id, "cold-sess");
    }
}
