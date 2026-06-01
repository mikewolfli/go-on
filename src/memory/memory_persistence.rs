//! GAP-B52-11: Memory Persistence with Hot/Warm/Cold Tiering
//!
//! Implements a three-tier memory persistence system:
//! - **Hot (L1)**: In-memory LRU cache (max 2048 entries, 5-minute TTL)
//! - **Warm (L2)**: SQLite-backed vector store (30-day retention)
//! - **Cold (L3)**: gzip-compressed NDJSON files on disk for long-term archival
//!
//! Provides automatic migration (promotion/demotion) between tiers and
//! metadata indexing on startup.

#![allow(dead_code)]


use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The memory tier an entry resides in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MemoryTier {
    /// L1: In-memory LRU cache. 2048 max entries, 5-minute TTL.
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
}

impl MemoryEntry {
    /// Create a new hot-tier memory entry.
    pub fn new_hot(
        id: impl Into<String>,
        class: impl Into<String>,
        content: impl Into<String>,
        usefulness: f32,
    ) -> Self {
        let now = now_secs();
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
        }
    }

    /// Touch the entry, updating its access timestamp and count.
    pub fn touch(&mut self) {
        self.accessed_at = now_secs();
        self.access_count += 1;
    }

    /// Returns the age of this entry in seconds.
    pub fn age_secs(&self) -> i64 {
        now_secs().saturating_sub(self.created_at)
    }

    /// Returns the time since last access in seconds.
    pub fn idle_secs(&self) -> i64 {
        now_secs().saturating_sub(self.accessed_at)
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
            hot_ttl_secs: 300,        // 5 minutes
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
    lru_order: Vec<String>,
    max_entries: usize,
    ttl: Duration,
}

impl HotCache {
    fn new(max_entries: usize, ttl_secs: i64) -> Self {
        Self {
            entries: HashMap::with_capacity(max_entries),
            lru_order: Vec::with_capacity(max_entries),
            max_entries,
            ttl: Duration::from_secs(ttl_secs.max(1) as u64),
        }
    }

    fn get(&mut self, id: &str) -> Option<&mut MemoryEntry> {
        // Promote to MRU position
        if let Some(pos) = self.lru_order.iter().position(|x| x == id) {
            self.lru_order.remove(pos);
            self.lru_order.push(id.to_string());
        }
        self.entries.get_mut(id).map(|he| {
            he.inserted_at = Instant::now();
            &mut he.entry
        })
    }

    fn insert(&mut self, mut entry: MemoryEntry) {
        entry.tier = MemoryTier::Hot;

        // If the entry already exists, just refresh it.
        if let Some(existing) = self.entries.get_mut(&entry.id) {
            existing.entry = entry;
            existing.inserted_at = Instant::now();
            // Move to MRU
            if let Some(pos) = self.lru_order.iter().position(|x| x == &existing.entry.id) {
                self.lru_order.remove(pos);
            }
            self.lru_order.push(existing.entry.id.clone());
            return;
        }

        // Evict if at capacity (LRU: remove from front).
        if self.entries.len() >= self.max_entries {
            self.evict_lru_one();
        }

        let id = entry.id.clone();
        self.lru_order.push(id.clone());
        self.entries.insert(
            id,
            HotEntry {
                entry,
                inserted_at: Instant::now(),
            },
        );
    }

    fn remove(&mut self, id: &str) -> Option<MemoryEntry> {
        if let Some(pos) = self.lru_order.iter().position(|x| x == id) {
            self.lru_order.remove(pos);
        }
        self.entries.remove(id).map(|he| he.entry)
    }

    /// Evict expired entries (TTL exceeded). Returns evicted entries.
    fn evict_expired(&mut self) -> Vec<MemoryEntry> {
        let now = Instant::now();
        let expired_ids: Vec<String> = self
            .entries
            .iter()
            .filter(|(_, he)| now.duration_since(he.inserted_at) > self.ttl)
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

    #[allow(dead_code)]
    fn contains(&self, id: &str) -> bool {
        self.entries.contains_key(id)
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    #[allow(dead_code)]
    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[allow(dead_code)]
    fn iter_entries(&self) -> impl Iterator<Item = &MemoryEntry> {
        self.entries.values().map(|he| &he.entry)
    }
}

// ===========================================================================
// L3: Cold Tier — gzip NDJSON on disk
// ===========================================================================

/// Manages cold storage: `.goon/memory/cold/YYYY-MM/*.ndjson.gz`
#[derive(Debug, Clone)]
struct ColdStorage {
    base_path: PathBuf,
}

impl ColdStorage {
    fn new(base_path: &Path) -> Self {
        Self {
            base_path: base_path.to_path_buf(),
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

    /// Append a single entry to the latest shard for the current month.
    fn append_entry(&self, entry: &MemoryEntry) -> Result<()> {
        let now_s = now_secs();
        // Compute year/month from Unix timestamp (simple divisional approach).
        // Uses 1970-01-01 as base, accounting for leap years.
        let (year, month) = ts_to_year_month(now_s);
        let dir = self.month_dir(year, month);
        fs::create_dir_all(&dir).context("failed to create cold storage month directory")?;

        // Use a fixed shard name for simplicity; rotate shards by date.
        let shard = format!("{:04}-{:02}", year, month);
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
        Ok(())
    }

    /// Read all entries from a specific shard.
    fn read_shard(&self, path: &Path) -> Result<Vec<MemoryEntry>> {
        let file = fs::File::open(path).context("failed to open cold storage shard")?;
        let decoder = GzDecoder::new(file);
        let reader = BufReader::new(decoder);
        let mut entries = Vec::new();
        for line in reader.lines() {
            let line = line.context("failed to read cold storage line")?;
            if line.trim().is_empty() {
                continue;
            }
            let entry: MemoryEntry =
                serde_json::from_str(&line).context("failed to deserialize cold entry")?;
            entries.push(entry);
        }
        Ok(entries)
    }

    /// Iterate all shards across all months, returning (path, entries).
    fn read_all(&self) -> Result<Vec<MemoryEntry>> {
        let mut all = Vec::new();
        if !self.base_path.exists() {
            return Ok(all);
        }
        for dir_entry in
            fs::read_dir(&self.base_path).context("failed to read cold storage base")?
        {
            let dir_entry = dir_entry.context("failed to read cold storage directory entry")?;
            let path = dir_entry.path();
            if path.is_dir() {
                for file_entry in
                    fs::read_dir(&path).context("failed to read cold storage month directory")?
                {
                    let file_entry =
                        file_entry.context("failed to read cold storage file entry")?;
                    let file_path = file_entry.path();
                    if file_path.extension().and_then(|e| e.to_str()) == Some("gz") {
                        let entries = self.read_shard(&file_path)?;
                        all.extend(entries);
                    }
                }
            }
        }
        Ok(all)
    }
}

// ===========================================================================
// L2: Warm Tier — SQLite-backed persistence
// ===========================================================================

/// Wrapper around the SQLite warm store.
///
/// Uses the same schema patterns as `crate::memory::vector::VectorStore`
/// but with a dedicated table for memory tier persistence.
#[cfg(feature = "backend-sqlite")]
#[derive(Debug)]
pub struct WarmStore {
    conn: Mutex<rusqlite::Connection>,
    max_entries: usize,
}

#[cfg(feature = "backend-sqlite")]
impl WarmStore {
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
                session_id TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_warm_memory_accessed_at
                ON warm_memory(accessed_at DESC);

            CREATE INDEX IF NOT EXISTS idx_warm_memory_usefulness
                ON warm_memory(usefulness DESC);

            CREATE INDEX IF NOT EXISTS idx_warm_memory_session_id
                ON warm_memory(session_id);
            ",
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
            max_entries,
        })
    }

    fn upsert(&self, entry: &MemoryEntry) -> Result<()> {
        let embedding_json = entry
            .embedding
            .as_ref()
            .map(|v| serde_json::to_string(v).unwrap_or_default());
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("warm store mutex poisoned: {}", e))?;
        conn.execute(
            "INSERT INTO warm_memory(id, tier, class, content, created_at, accessed_at, usefulness, embedding_json, access_count, session_id)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(id) DO UPDATE SET
                tier = excluded.tier,
                class = excluded.class,
                content = excluded.content,
                accessed_at = excluded.accessed_at,
                usefulness = excluded.usefulness,
                embedding_json = excluded.embedding_json,
                access_count = excluded.access_count,
                session_id = excluded.session_id",
            rusqlite::params![
                entry.id,
                entry.tier.label(),
                entry.class,
                entry.content,
                entry.created_at,
                entry.accessed_at,
                entry.usefulness,
                embedding_json,
                entry.access_count,
                entry.session_id,
            ],
        )?;
        // Enforce max entries (evict oldest by accessed_at)
        conn.execute(
            "DELETE FROM warm_memory WHERE id IN (
                SELECT id FROM warm_memory ORDER BY accessed_at ASC LIMIT MAX(0, (SELECT COUNT(*) FROM warm_memory) - ?1)
            )",
            rusqlite::params![self.max_entries as i64],
        )?;
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Option<MemoryEntry>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("warm store mutex poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT id, tier, class, content, created_at, accessed_at, usefulness, embedding_json, access_count, session_id
             FROM warm_memory WHERE id = ?1",
        )?;
        let mut rows = stmt.query(rusqlite::params![id])?;
        match rows.next()? {
            Some(row) => {
                let embedding_json: Option<String> = row.get(7)?;
                let embedding =
                    embedding_json.and_then(|j| serde_json::from_str::<Vec<f32>>(&j).ok());
                Ok(Some(MemoryEntry {
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
                }))
            }
            None => Ok(None),
        }
    }

    fn remove(&self, id: &str) -> Result<bool> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("warm store mutex poisoned: {}", e))?;
        let affected = conn.execute(
            "DELETE FROM warm_memory WHERE id = ?1",
            rusqlite::params![id],
        )?;
        Ok(affected > 0)
    }

    pub fn search_by_usefulness(
        &self,
        min_usefulness: f32,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("warm store mutex poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT id, tier, class, content, created_at, accessed_at, usefulness, embedding_json, access_count, session_id
             FROM warm_memory WHERE usefulness >= ?1 ORDER BY usefulness DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![min_usefulness, limit as i64], |row| {
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
            })
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    fn count(&self) -> Result<usize> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("warm store mutex poisoned: {}", e))?;
        let count: i64 =
            conn.query_row("SELECT COUNT(*) FROM warm_memory", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    fn iterate_all(&self) -> Result<Vec<MemoryEntry>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("warm store mutex poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT id, tier, class, content, created_at, accessed_at, usefulness, embedding_json, access_count, session_id
             FROM warm_memory",
        )?;
        let rows = stmt.query_map([], |row| {
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
            })
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    pub fn search_by_session(&self, session_id: &str, limit: usize) -> Result<Vec<MemoryEntry>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("warm store mutex poisoned: {}", e))?;
        let mut stmt = conn.prepare(
            "SELECT id, tier, class, content, created_at, accessed_at, usefulness, embedding_json, access_count, session_id
             FROM warm_memory WHERE session_id = ?1 ORDER BY accessed_at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![session_id, limit as i64], |row| {
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
            })
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }
}

// ===========================================================================
// Memory Persistence Manager (tier orchestration)
// ===========================================================================

/// The main persistence manager that orchestrates all three tiers.
///
/// Responsibilities:
/// - Insert entries into the appropriate tier.
/// - Promote hot → warm and warm → cold based on policy.
/// - Demote cold → warm and warm → hot when accessed.
/// - Load metadata index from L2 + L3 on startup.
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
    /// Monotonic sequence for ordering
    sequence: AtomicU64,
}

/// Non-SQLite placeholder for when the backend-postgres feature is active.
/// Since the core memory persistence works regardless of backend, we provide
/// an empty fallback when SQLite is not available.
#[cfg(not(feature = "backend-sqlite"))]
#[derive(Debug)]
pub struct WarmStore;

#[cfg(not(feature = "backend-sqlite"))]
impl WarmStore {
    pub fn new(_path: &Path, _max_entries: usize) -> Result<Self> {
        // Postgres backend variant — stub that logs a warning.
        tracing::warn!("WarmStore not available: backend-sqlite feature not enabled");
        Ok(Self)
    }

    pub fn upsert(&self, _entry: &MemoryEntry) -> Result<()> {
        Ok(())
    }

    pub fn get(&self, _id: &str) -> Result<Option<MemoryEntry>> {
        Ok(None)
    }

    pub fn remove(&self, _id: &str) -> Result<bool> {
        Ok(false)
    }

    pub fn search_by_usefulness(
        &self,
        _min_usefulness: f32,
        _limit: usize,
    ) -> Result<Vec<MemoryEntry>> {
        Ok(Vec::new())
    }

    pub fn count(&self) -> Result<usize> {
        Ok(0)
    }

    pub fn iterate_all(&self) -> Result<Vec<MemoryEntry>> {
        Ok(Vec::new())
    }

    pub fn search_by_session(&self, _session_id: &str, _limit: usize) -> Result<Vec<MemoryEntry>> {
        Ok(Vec::new())
    }
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

        #[cfg(feature = "backend-sqlite")]
        let warm = WarmStore::new(db_path, policy.warm_max_entries)?;
        #[cfg(not(feature = "backend-sqlite"))]
        let warm = WarmStore::new(db_path, policy.warm_max_entries)?;

        Ok(Self {
            hot: Mutex::new(HotCache::new(policy.hot_max_entries, policy.hot_ttl_secs)),
            warm,
            cold: ColdStorage::new(cold_base_path),
            policy,
            sequence: AtomicU64::new(0),
        })
    }

    /// Load metadata index for all entries from L2 (warm) and L3 (cold) tiers.
    ///
    /// Returns a summary with counts per tier.
    pub fn load_metadata_index(&self) -> Result<MetadataIndex> {
        let warm_entries = self.warm.iterate_all()?;
        let cold_entries = self.cold.read_all()?;

        Ok(MetadataIndex {
            warm_count: warm_entries.len(),
            cold_count: cold_entries.len(),
            total: warm_entries.len() + cold_entries.len(),
        })
    }

    /// Insert or update a memory entry.
    ///
    /// The entry is placed in the hot tier by default and will be promoted
    /// according to the tiering policy.
    pub fn store(&self, entry: MemoryEntry) -> Result<()> {
        let seq = self.sequence.fetch_add(1, Ordering::Relaxed);
        let _ = seq; // available for future ordering needs

        let mut hot = self
            .hot
            .lock()
            .map_err(|e| anyhow::anyhow!("hot cache mutex poisoned: {}", e))?;

        // Always insert/refresh in hot tier.
        hot.insert(entry);
        Ok(())
    }

    /// Retrieve an entry by ID, checking hot → warm → cold in order.
    pub fn retrieve(&self, id: &str) -> Result<Option<MemoryEntry>> {
        // Check hot first.
        {
            let mut hot = self
                .hot
                .lock()
                .map_err(|e| anyhow::anyhow!("hot cache mutex poisoned: {}", e))?;
            if let Some(entry) = hot.get(id) {
                entry.touch();
                return Ok(Some(entry.clone()));
            }
        }

        // Check warm.
        if let Some(mut entry) = self.warm.get(id)? {
            entry.touch();
            // Promote back to hot on access.
            self.promote_to_hot(entry.clone())?;
            return Ok(Some(entry));
        }

        // Check cold.
        // Cold is file-based; we scan all shards. For efficiency, a real
        // implementation would maintain an index. Here we do a linear scan.
        let cold_entries = self.cold.read_all()?;
        if let Some(mut entry) = cold_entries.into_iter().find(|e| e.id == id) {
            entry.touch();
            // Promote back to warm on access.
            self.promote_to_warm(entry.clone())?;
            return Ok(Some(entry));
        }

        Ok(None)
    }

    /// Remove an entry from all tiers.
    pub fn remove(&self, id: &str) -> Result<bool> {
        let mut removed = false;
        // Remove from hot.
        {
            let mut hot = self
                .hot
                .lock()
                .map_err(|e| anyhow::anyhow!("hot cache mutex poisoned: {}", e))?;
            if hot.remove(id).is_some() {
                removed = true;
            }
        }
        // Remove from warm.
        if self.warm.remove(id)? {
            removed = true;
        }
        // Remove from cold is not supported (append-only archival).
        Ok(removed)
    }

    /// Promote an entry from hot → warm tier.
    pub fn promote_to_warm(&self, entry: MemoryEntry) -> Result<()> {
        let mut entry = entry;
        entry.tier = MemoryTier::Warm;
        self.warm.upsert(&entry)?;

        // Remove from hot.
        let mut hot = self
            .hot
            .lock()
            .map_err(|e| anyhow::anyhow!("hot cache mutex poisoned: {}", e))?;
        hot.remove(&entry.id);
        Ok(())
    }

    /// Promote an entry from warm → cold (archival).
    pub fn promote_to_cold(&self, entry: MemoryEntry) -> Result<()> {
        let mut entry = entry;
        entry.tier = MemoryTier::Cold;
        self.cold.append_entry(&entry)?;

        // Remove from warm.
        self.warm.remove(&entry.id)?;
        Ok(())
    }

    /// Promote an entry to the hot tier (from warm or cold).
    pub fn promote_to_hot(&self, entry: MemoryEntry) -> Result<()> {
        let mut entry = entry;
        entry.tier = MemoryTier::Hot;
        {
            let mut hot = self
                .hot
                .lock()
                .map_err(|e| anyhow::anyhow!("hot cache mutex poisoned: {}", e))?;
            hot.insert(entry);
        }
        Ok(())
    }

    /// Run automatic tier migration based on policy.
    ///
    /// 1. Evict expired hot entries → promote useful ones to warm, discard stale ones.
    /// 2. Check warm entries approaching TTL → promote useful ones to cold.
    pub fn auto_migrate(&self) -> Result<MigrationReport> {
        let mut report = MigrationReport::default();

        // ── Step 1: Process hot cache evictions ──
        let evicted: Vec<MemoryEntry> = {
            let mut hot = self
                .hot
                .lock()
                .map_err(|e| anyhow::anyhow!("hot cache mutex poisoned: {}", e))?;
            hot.evict_expired()
        };

        for entry in evicted {
            if entry.usefulness >= self.policy.hot_threshold {
                // Promote to warm.
                self.promote_to_warm(entry)?;
                report.promoted_hot_to_warm += 1;
            } else {
                // Stale, demote to cold directly (or discard).
                self.promote_to_cold(entry)?;
                report.demoted_hot_to_cold += 1;
            }
        }

        // ── Step 2: Check warm for TTL candidates ──
        let warm_entries = self.warm.iterate_all()?;
        let now = now_secs();
        for entry in warm_entries {
            let idle = now.saturating_sub(entry.accessed_at);
            if idle >= self.policy.warm_ttl_secs {
                if entry.usefulness >= self.policy.warm_threshold {
                    // Promote to cold (archival).
                    self.promote_to_cold(entry)?;
                    report.promoted_warm_to_cold += 1;
                } else {
                    // Low-usefulness warm entry: just remove.
                    self.warm.remove(&entry.id)?;
                    report.evicted_warm += 1;
                }
            }
        }

        Ok(report)
    }

    /// Explicitly promote a single entry up one tier.
    pub fn promote(&self, entry: &MemoryEntry) -> Result<Option<MemoryEntry>> {
        match entry.tier {
            MemoryTier::Hot => {
                let mut e = entry.clone();
                e.tier = MemoryTier::Warm;
                self.warm.upsert(&e)?;
                {
                    let mut hot = self
                        .hot
                        .lock()
                        .map_err(|e| anyhow::anyhow!("hot cache mutex poisoned: {}", e))?;
                    hot.remove(&entry.id);
                }
                Ok(Some(e))
            }
            MemoryTier::Warm => {
                let mut e = entry.clone();
                e.tier = MemoryTier::Cold;
                self.cold.append_entry(&e)?;
                self.warm.remove(&entry.id)?;
                Ok(Some(e))
            }
            MemoryTier::Cold => {
                // Already at highest tier; cannot promote further.
                Ok(None)
            }
        }
    }

    /// Explicitly demote a single entry down one tier.
    pub fn demote(&self, entry: &MemoryEntry) -> Result<Option<MemoryEntry>> {
        match entry.tier {
            MemoryTier::Hot => {
                // Fall directly to warm (hot→cold would be too aggressive).
                self.promote_to_warm(entry.clone())?;
                let mut e = entry.clone();
                e.tier = MemoryTier::Warm;
                Ok(Some(e))
            }
            MemoryTier::Warm => {
                self.promote_to_cold(entry.clone())?;
                let mut e = entry.clone();
                e.tier = MemoryTier::Cold;
                Ok(Some(e))
            }
            MemoryTier::Cold => {
                // Cannot demote from cold; stays in cold.
                Ok(None)
            }
        }
    }

    /// Returns the count of entries in each tier.
    pub fn tier_counts(&self) -> Result<TierCounts> {
        let hot_count = self
            .hot
            .lock()
            .map_err(|e| anyhow::anyhow!("hot cache mutex poisoned: {}", e))?
            .len();
        let warm_count = self.warm.count().unwrap_or(0);
        Ok(TierCounts {
            hot: hot_count,
            warm: warm_count,
            cold: 0, // Cold count would require scanning all shards; omitted for perf.
        })
    }

    /// Returns a reference to the warm store for direct querying.
    pub fn warm_store(&self) -> &WarmStore {
        &self.warm
    }
}

// ===========================================================================
// Reports & Index
// ===========================================================================

/// Summary of metadata loaded at startup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataIndex {
    pub warm_count: usize,
    pub cold_count: usize,
    pub total: usize,
}

/// Count of entries in each tier.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TierCounts {
    pub hot: usize,
    pub warm: usize,
    pub cold: usize,
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

/// Current Unix timestamp in seconds.
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

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
    fn test_hot_cache_insert_and_get() {
        let mut cache = HotCache::new(10, 300);
        let entry = make_entry("e1", 0.8);
        cache.insert(entry.clone());

        let retrieved = cache.get("e1");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().id, "e1");
    }

    #[test]
    fn test_hot_cache_lru_eviction() {
        let mut cache = HotCache::new(3, 300);
        for i in 1..=3 {
            cache.insert(make_entry(&format!("e{}", i), 0.5));
        }
        assert_eq!(cache.len(), 3);

        // Insert a 4th entry → evicts LRU (e1)
        cache.insert(make_entry("e4", 0.5));
        assert_eq!(cache.len(), 3);
        assert!(cache.get("e1").is_none());
        assert!(cache.get("e4").is_some());
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
        assert_eq!(policy.hot_ttl_secs, 300);
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
        let dir = TempDir::new().unwrap();
        let cold = ColdStorage::new(dir.path());
        let entry = make_entry("cold1", 0.7);
        cold.append_entry(&entry).unwrap();

        let all = cold.read_all().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "cold1");
    }

    #[test]
    fn test_persistence_store_and_retrieve() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("warm.db");
        let cold_path = dir.path().join("cold");

        let persistence = MemoryPersistence::new(&db_path, &cold_path, None).unwrap();
        let entry = make_entry("p1", 0.8);
        persistence.store(entry).unwrap();

        let retrieved = persistence.retrieve("p1").unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().id, "p1");
    }

    #[test]
    fn test_promotion_hot_to_warm() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("warm.db");
        let cold_path = dir.path().join("cold");

        let persistence = MemoryPersistence::new(&db_path, &cold_path, None).unwrap();
        let entry = make_entry("promo1", 0.9);
        persistence.store(entry.clone()).unwrap();
        persistence.promote_to_warm(entry).unwrap();

        // Should no longer be in hot (but still retrievable from warm).
        let retrieved = persistence.retrieve("promo1").unwrap();
        assert!(retrieved.is_some());
        // Access brings it back to hot
        assert_eq!(retrieved.unwrap().tier, MemoryTier::Hot);
    }

    #[test]
    fn test_auto_migrate_evicts_expired_hot_entries() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("warm.db");
        let cold_path = dir.path().join("cold");

        let mut policy = MemoryTieringPolicy::default();
        policy.hot_ttl_secs = 0; // Instant expiry
        let persistence = MemoryPersistence::new(&db_path, &cold_path, Some(policy)).unwrap();

        // Entry with low usefulness → gets demoted to cold (not promoted to warm)
        let entry = make_entry("low", 0.1);
        persistence.store(entry).unwrap();

        // Entry with high usefulness → promoted to warm
        let entry = make_entry("high", 0.8);
        persistence.store(entry).unwrap();

        let report = persistence.auto_migrate().unwrap();
        assert_eq!(report.promoted_hot_to_warm, 1);
        assert_eq!(report.demoted_hot_to_cold, 1);
    }

    #[test]
    fn test_metadata_index_load() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("warm.db");
        let cold_path = dir.path().join("cold");

        let persistence = MemoryPersistence::new(&db_path, &cold_path, None).unwrap();

        // Seed some entries
        let entry = make_entry("idx1", 0.5);
        persistence.store(entry).unwrap();

        let index = persistence.load_metadata_index().unwrap();
        // Hot-only entries don't appear in the index (only warm + cold).
        assert_eq!(index.warm_count, 0);
        assert_eq!(index.cold_count, 0);
    }

    #[test]
    fn test_demote_entry() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("warm.db");
        let cold_path = dir.path().join("cold");

        let persistence = MemoryPersistence::new(&db_path, &cold_path, None).unwrap();

        let entry = make_entry("dem1", 0.5);
        persistence.store(entry.clone()).unwrap();
        let demoted = persistence.demote(&entry).unwrap();
        assert!(demoted.is_some());
        assert_eq!(demoted.unwrap().tier, MemoryTier::Warm);
    }

    #[test]
    fn test_promote_entry() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("warm.db");
        let cold_path = dir.path().join("cold");

        let persistence = MemoryPersistence::new(&db_path, &cold_path, None).unwrap();

        let entry = make_entry("prom2", 0.5);
        persistence.store(entry.clone()).unwrap();
        // Move to warm first, then promote to cold
        persistence.promote_to_warm(entry.clone()).unwrap();

        let entry = persistence.retrieve("prom2").unwrap().unwrap();
        // Manually set to warm for promote to work
        let promoted = persistence.promote(&entry).unwrap();
        assert!(promoted.is_some());
        assert_eq!(promoted.unwrap().tier, MemoryTier::Cold);
    }
}
