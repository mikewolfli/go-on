//! Vector storage and search
//!
//! Conditionally compiled:
//! - `backend-sqlite` (local, simple-server): rusqlite-backed, sync API
//! - `backend-postgres` (multi-users-server): postgres + pgvector-backed sync API
//!
//! # Backend-pair contract (debt #13 verdict: keep, do not trait-ify)
//!
//! The two backend impls mirror each other method-for-method with aligned
//! signatures (`upsert`, `search`, `get_phase_summary`, `upsert_phase_summary`,
//! `memory_entry_count`, `summary_entry_count`, `clear_all`). Semantics are
//! kept identical: eviction keeps the newest `max_entries` by `updated_at`;
//! scoring / recency blending / `min_similarity` match. Only SQL dialect
//! differs (`PARAM_PREFIX`, distance operator, DELETE/LIMIT shapes) and is
//! already confined via shared helpers (`embed_with_check`, `build_memory_key`,
//! `scored_to_hits`, `blend_similarity_with_recency`, `spawn_blocking_vec!`).
//! Trait-ifying adds no value under the mutually-exclusive feature gates
//! (compile_error! below) — the contract is enforced by convention: NEW
//! methods must be implemented in BOTH backends with identical semantics.

// Ensure features are mutually exclusive
#[cfg(all(feature = "backend-sqlite", feature = "backend-postgres"))]
compile_error!("features 'backend-sqlite' and 'backend-postgres' cannot be enabled simultaneously");

use crate::acp::prelude::now_ts;
#[cfg(not(feature = "backend-postgres"))]
use std::path::Path;
use std::sync::Arc;
#[cfg(not(feature = "backend-postgres"))]
use std::sync::Mutex;
#[cfg(not(feature = "backend-postgres"))]
use std::sync::Mutex as StdMutex;
#[cfg(not(feature = "backend-postgres"))]
use std::sync::Once;
use tokio::task::spawn_blocking;

/// Parameter placeholder prefix for the active backend.
#[cfg(not(feature = "backend-postgres"))]
const PARAM_PREFIX: &str = "?";
#[cfg(feature = "backend-postgres")]
const PARAM_PREFIX: &str = "$";

/// Column list for `phase_summary` (shared between backends).
const PHASE_SUMMARY_COLUMNS: &str = "phase, summary_text, updated_at";

#[cfg(not(feature = "backend-postgres"))]
use crate::memory::embedding_provider::{
    local_hash_embed, ConfigurableEmbeddingProvider, EmbeddingProvider,
};
#[cfg(not(feature = "backend-postgres"))]
use crate::shared::math::cosine_similarity_f32;
use anyhow::Result;
#[cfg(not(feature = "backend-postgres"))]
use fastrand;
#[cfg(feature = "backend-postgres")]
use pgvector::Vector;
#[cfg(not(feature = "backend-postgres"))]
use rusqlite::{ffi::sqlite3_auto_extension, params, Connection, OptionalExtension};
#[cfg(not(feature = "backend-postgres"))]
use sqlite_vec::sqlite3_vec_init;

/// Vector search hit
#[derive(Debug, Clone)]
pub struct VectorHit {
    /// Response snippet
    pub response_snippet: String,
    /// Similarity score (0.0-1.0)
    pub similarity: f32,
}

/// Precision feedback from a vector search: average similarity of returned hits.
/// Used by autotune to adjust min_query_chars and other parameters.
#[derive(Debug, Clone, Copy)]
pub struct VectorPrecisionFeedback {
    /// Average similarity of returned hits (0.0-1.0).
    pub avg_similarity: f32,
    /// Number of hits returned.
    pub hit_count: usize,
}

impl VectorPrecisionFeedback {
    pub fn new(hits: &[VectorHit]) -> Self {
        if hits.is_empty() {
            return Self {
                avg_similarity: 0.0,
                hit_count: 0,
            };
        }
        let sum: f32 = hits.iter().map(|h| h.similarity).sum();
        let avg = sum / hits.len() as f32;
        Self {
            avg_similarity: avg,
            hit_count: hits.len(),
        }
    }
}

/// HNSW node metadata
#[cfg(not(feature = "backend-postgres"))]
#[derive(Debug, Clone)]
struct HnswNodeMeta {
    memory_key: String,
    phase: String,
    response_text: String,
    updated_at: i64,
}

/// A (node index, distance) pair with ordering so that smaller distance sorts first.
#[cfg(not(feature = "backend-postgres"))]
#[derive(Debug, Clone, Copy, PartialEq)]
struct HnswNodeDist {
    idx: usize,
    dist: f32,
}

#[cfg(not(feature = "backend-postgres"))]
impl Eq for HnswNodeDist {}

#[cfg(not(feature = "backend-postgres"))]
impl PartialOrd for HnswNodeDist {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(not(feature = "backend-postgres"))]
impl Ord for HnswNodeDist {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.dist
            .partial_cmp(&other.dist)
            .unwrap_or(std::cmp::Ordering::Equal)
    }
}

/// Hierarchical Navigable Small World index for approximate nearest neighbor search.
///
/// Provides O(log N) search time for high-dimensional vectors.
/// Standard parameters: M=16, ef_construction=200, ef_search=50.
#[cfg(not(feature = "backend-postgres"))]
#[derive(Debug)]
struct HnswIndex {
    /// Stored vectors (index in this vec == node id)
    vectors: Vec<Vec<f32>>,
    /// Per-node metadata
    metadata: Vec<HnswNodeMeta>,
    /// Adjacency lists per layer: layers[layer][node_id] = Vec<neighbor_id>
    layers: Vec<Vec<Vec<usize>>>,
    /// Per-node random level (parallel to `vectors`/`metadata`), used to
    /// re-point `entry_point` when the current one is removed.
    node_levels: Vec<usize>,
    /// Current entry point (node id at the topmost layer)
    entry_point: Option<usize>,
    /// Highest layer that has any element
    max_level: usize,
    // ── HNSW parameters (constant after construction) ──
    /// Max number of connections per node on layer > 0
    m: usize,
    /// Max number of connections per node on layer 0
    m_max0: usize,
    /// Size of dynamic candidate list during construction
    ef_construction: usize,
    /// Size of dynamic candidate list during search
    ef_search: usize,
    /// Normalisation factor for level generation: mL = 1.0 / ln(M)
    m_l: f64,
}

#[cfg(not(feature = "backend-postgres"))]
impl HnswIndex {
    fn new(m: usize, ef_construction: usize, ef_search: usize) -> Self {
        let m_max0 = m * 2;
        let m_l = 1.0 / (m as f64).ln();
        Self {
            vectors: Vec::new(),
            metadata: Vec::new(),
            layers: vec![Vec::new()], // layer 0 exists and is empty
            node_levels: Vec::new(),
            entry_point: None,
            max_level: 0,
            m,
            m_max0,
            ef_construction,
            ef_search,
            m_l,
        }
    }

    fn random_level(&self) -> usize {
        let r: f64 = fastrand::f64(); // uniform in [0, 1)
        if r <= 0.0 {
            return 0;
        }
        (-r.ln() * self.m_l).floor() as usize
    }

    /// Distance between a query vector and a stored node.
    fn distance(&self, query: &[f32], node: usize) -> f32 {
        let v = &self.vectors[node];
        1.0 - cosine_similarity_f32(query, v)
    }

    /// Greedy search at a single layer, returning up to `ef` nearest neighbours.
    ///
    /// `entry` is the starting node id on this layer.
    fn search_layer(&self, query: &[f32], entry: usize, lc: usize, ef: usize) -> Vec<HnswNodeDist> {
        // Min-heap of candidates (closest first)
        let mut candidates: std::collections::BinaryHeap<std::cmp::Reverse<HnswNodeDist>> =
            std::collections::BinaryHeap::new();
        // Max-heap of results (furthest first — we track the worst distance)
        let mut results: std::collections::BinaryHeap<HnswNodeDist> =
            std::collections::BinaryHeap::new();

        let entry_dist = self.distance(query, entry);
        let entry_nd = HnswNodeDist {
            idx: entry,
            dist: entry_dist,
        };
        candidates.push(std::cmp::Reverse(entry_nd));
        results.push(entry_nd);

        let mut visited = std::collections::HashSet::new();
        visited.insert(entry);

        while let Some(std::cmp::Reverse(closest)) = candidates.pop() {
            // The furthest result is the top of the max-heap
            if let Some(furthest) = results.peek() {
                if closest.dist > furthest.dist {
                    break; // Cannot improve
                }
            }
            for &neighbor in &self.layers[lc][closest.idx] {
                if visited.contains(&neighbor) {
                    continue;
                }
                visited.insert(neighbor);
                let neighbor_dist = self.distance(query, neighbor);
                let furthest_dist = results.peek().map(|r| r.dist).unwrap_or(f32::MAX);
                if neighbor_dist < furthest_dist || results.len() < ef {
                    let nd = HnswNodeDist {
                        idx: neighbor,
                        dist: neighbor_dist,
                    };
                    candidates.push(std::cmp::Reverse(nd));
                    results.push(nd);
                    if results.len() > ef {
                        results.pop(); // Remove furthest
                    }
                }
            }
        }

        // Convert to sorted (closest-first) vec
        let mut sorted: Vec<HnswNodeDist> = results.into_sorted_vec();
        sorted.reverse(); // into_sorted_vec gives ascending; we want descending for .pop()
        sorted
    }

    /// Select the M closest neighbours from a candidate set (simple heuristic).
    fn select_neighbors_simple(
        &self,
        _q_idx: usize,
        candidates: &[HnswNodeDist],
        m: usize,
    ) -> Vec<HnswNodeDist> {
        let k = m.min(candidates.len());
        let mut sorted = candidates.to_vec();
        sorted.sort();
        sorted.truncate(k);
        sorted
    }

    /// Shrink connections for a node on a given layer, keeping only the M closest.
    fn shrink_connections(&mut self, node: usize, lc: usize, max_conn: usize) {
        let neighbors = &self.layers[lc][node];
        if neighbors.len() <= max_conn {
            return;
        }
        // Sort neighbors by distance to `node`
        let node_vec = &self.vectors[node];
        let mut dists: Vec<HnswNodeDist> = neighbors
            .iter()
            .map(|&n| HnswNodeDist {
                idx: n,
                dist: 1.0 - cosine_similarity_f32(node_vec, &self.vectors[n]),
            })
            .collect();
        dists.sort();
        dists.truncate(max_conn);
        self.layers[lc][node] = dists.into_iter().map(|nd| nd.idx).collect();
    }

    /// Insert a single vector with its metadata into the index.
    fn insert(&mut self, vector: Vec<f32>, meta: HnswNodeMeta) {
        let q_idx = self.vectors.len();
        let level = self.random_level();

        // Ensure layers exist up to `level`
        while self.layers.len() <= level {
            self.layers.push(Vec::new());
        }
        // Ensure each layer has adjacency entries for all existing nodes
        for lc in 0..self.layers.len() {
            while self.layers[lc].len() <= q_idx {
                self.layers[lc].push(Vec::new());
            }
        }

        self.vectors.push(vector.clone());
        self.metadata.push(meta);
        self.node_levels.push(level);

        if self.entry_point.is_none() {
            // First element
            self.entry_point = Some(q_idx);
            self.max_level = level;
            return;
        }

        let ep = self
            .entry_point
            .expect("HNSW entry_point must be set before insert");

        // Phase 1: traverse from top layer down to level+1 greedily (ef=1)
        let mut curr_ep = ep;
        for lc in (level + 1..=self.max_level).rev() {
            if lc < self.layers.len() && self.layers[lc].len() > curr_ep {
                let result = self.search_layer(&vector, curr_ep, lc, 1);
                if let Some(nearest) = result.first() {
                    curr_ep = nearest.idx;
                }
            }
        }

        // Phase 2: insert on each layer from min(level, max_level) down to 0
        let top = level.min(self.max_level);
        for lc in (0..=top).rev() {
            let candidates = self.search_layer(&vector, curr_ep, lc, self.ef_construction);
            let m_lc = if lc == 0 { self.m_max0 } else { self.m };
            let neighbors = self.select_neighbors_simple(q_idx, &candidates, m_lc);

            // Connect q → neighbors
            self.layers[lc][q_idx] = neighbors.iter().map(|nd| nd.idx).collect();

            // Connect neighbors → q (bidirectional)
            for nd in &neighbors {
                let n_idx = nd.idx;
                if lc < self.layers.len() && self.layers[lc].len() > n_idx {
                    self.layers[lc][n_idx].push(q_idx);
                    // Shrink if needed
                    let m_shrink = if lc == 0 { self.m_max0 } else { self.m };
                    self.shrink_connections(n_idx, lc, m_shrink);
                }
            }
        }

        // Update global entry point if the new element has a higher level
        if level > self.max_level {
            self.max_level = level;
            self.entry_point = Some(q_idx);
        }
    }

    /// Remove all nodes matching a memory_key from the index.
    ///
    /// Zeroes the vectors and clears metadata so they are filtered out during
    /// distance computations (`search` skips entries with empty memory_key).
    /// Removing ALL matches (not just the first) is required: `upsert` re-inserts
    /// a node per memory_key, and eviction can hit the same key twice in a row —
    /// leaving any match behind would let the search fast path return stale
    /// content or duplicate keys that the SQLite path would not return.
    fn remove(&mut self, memory_key: &str) {
        let mut removed_entry_point = false;
        for (pos, meta) in self.metadata.iter_mut().enumerate() {
            if meta.memory_key == memory_key {
                // Zero out the vector (distance will be ~1.0, effectively invisible)
                self.vectors[pos].fill(0.0);
                // Clear metadata so the node won't be matched again
                *meta = HnswNodeMeta {
                    memory_key: String::new(),
                    phase: String::new(),
                    response_text: String::new(),
                    updated_at: 0,
                };
                if self.entry_point == Some(pos) {
                    removed_entry_point = true;
                }
            }
        }
        // If the entry point itself was removed, point to the highest-level
        // remaining live node so search does not start from a dead (zeroed)
        // node. Searches still filter dead nodes, but starting from a live one
        // avoids navigating from a vector of zeros.
        if removed_entry_point {
            self.repair_entry_point();
        }
    }

    /// Re-point `entry_point`/`max_level` to the highest-level live node.
    ///
    /// If no live node remains, the index is empty and `entry_point` is cleared
    /// (search checks `vectors.is_empty()` and the valid set before use).
    fn repair_entry_point(&mut self) {
        let mut best: Option<(usize, usize)> = None; // (level, idx)
        for (idx, meta) in self.metadata.iter().enumerate() {
            if meta.memory_key.is_empty() {
                continue;
            }
            let level = self.node_levels.get(idx).copied().unwrap_or(0);
            if best.is_none_or(|(bl, _)| level > bl) {
                best = Some((level, idx));
            }
        }
        match best {
            Some((level, idx)) => {
                self.entry_point = Some(idx);
                self.max_level = level;
            }
            None => {
                self.entry_point = None;
                self.max_level = 0;
            }
        }
    }

    /// Search the index, returning up to `ef` nearest neighbours sorted by distance.
    ///
    /// Filters out removed entries (those with empty memory_key metadata).
    fn search(&self, query: &[f32], ef: usize) -> Vec<HnswNodeDist> {
        if self.vectors.is_empty() {
            return Vec::new();
        }
        // Build a set of valid (non-removed) node indices for post-filtering.
        let valid: std::collections::HashSet<usize> = self
            .metadata
            .iter()
            .enumerate()
            .filter(|(_, m)| !m.memory_key.is_empty())
            .map(|(i, _)| i)
            .collect();
        if valid.is_empty() {
            return Vec::new();
        }
        let ep = self
            .entry_point
            .expect("HNSW entry_point must be set before search; check vectors.is_empty()");

        // Greedy search from top layer down to layer 1 (ef=1 per layer)
        let mut curr_ep = ep;
        for lc in (1..=self.max_level).rev() {
            if lc < self.layers.len() && self.layers[lc].len() > curr_ep {
                let result = self.search_layer(query, curr_ep, lc, 1);
                if let Some(nearest) = result.first() {
                    curr_ep = nearest.idx;
                }
            }
        }

        // Search layer 0 with ef
        let ef_actual = ef.max(self.ef_search);
        let results = self.search_layer(query, curr_ep, 0, ef_actual);

        // Filter out removed entries (empty memory_key)
        results
            .into_iter()
            .filter(|nd| valid.contains(&nd.idx))
            .collect()
    }
}

/// Shared spawn_blocking wrapper for vector store async methods.
/// Eliminates the duplicated `spawn_blocking().await.map_err()` pattern.
macro_rules! spawn_blocking_vec {
    ($block:expr) => {
        spawn_blocking($block)
            .await
            .map_err(|e| anyhow::anyhow!("VectorStore blocking thread panicked: {e}"))?
    };
}
/// Vector store for similarity search
#[cfg(not(feature = "backend-postgres"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SqliteVectorMode {
    SqliteVec,
    // In profiles where the JSON fallback is compile-time disabled (anything
    // except `local` without simple-server/multi-users-server) the variant is
    // never constructed — keep the match arm in `search` reachable without a
    // dead_code warning instead of faking a construction at runtime.
    #[cfg_attr(
        not(all(
            feature = "local",
            not(feature = "simple-server"),
            not(feature = "multi-users-server")
        )),
        allow(dead_code)
    )]
    JsonFallback,
}

/// Vector store for similarity search
#[cfg(not(feature = "backend-postgres"))]
#[derive(Debug)]
pub struct VectorStore {
    /// SQLite connection (mutex-protected)
    conn: Arc<Mutex<Connection>>,
    /// Embedding dimensions
    dimensions: usize,
    /// Maximum number of entries to keep
    max_entries: usize,
    /// Selected sqlite vector implementation mode.
    mode: SqliteVectorMode,
    /// Optional embedding provider — overrides the built-in `embed_text()`.
    ///
    /// In production, inject via [`VectorStore::with_embedding_provider`].
    /// When `None`, the built-in minhash fallback (`embed_text()`) is used,
    /// which is only suitable for development/testing.
    embedding_provider: Option<ConfigurableEmbeddingProvider>,
    /// Optional in-memory HNSW index for approximate nearest neighbor search.
    /// Built lazily on first search; updated on upsert when present.
    ///
    /// Lock order convention: `conn` -> `hnsw` (never the reverse). Every
    /// path that touches both guards acquires `conn` first, so a concurrent
    /// `upsert` and a lazy first build can never form a lock-order cycle.
    hnsw: Arc<StdMutex<Option<HnswIndex>>>,
}

#[cfg(not(feature = "backend-postgres"))]
impl VectorStore {
    /// Create a new vector store with the built-in minhash fallback for embeddings.
    ///
    /// ⚠️  The minhash fallback is only suitable for development/testing.
    ///     Production deployments should call [`Self::new_with_env`] or
    ///     [`Self::with_embedding_provider`] to use real embeddings.
    ///
    /// # Arguments
    /// * `path` - Path to the SQLite database file
    /// * `dimensions` - Embedding dimensions
    /// * `max_entries` - Maximum number of entries to keep
    ///
    /// # Returns
    /// * `Result<Self>` - Returns Ok(Self) if the store is created successfully, or an error if something goes wrong
    pub fn new(path: &Path, dimensions: usize, max_entries: usize) -> Result<Self> {
        if dimensions == 0 {
            anyhow::bail!("vector.dimensions must be greater than 0");
        }

        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }

        register_sqlite_vec_auto_extension();
        let conn = Connection::open(path)?;

        // Step 1: Initialize schema — PRAGMAs and table creation.
        // CREATE TABLE IF NOT EXISTS is safe regardless of whether the table exists.
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;

            CREATE TABLE IF NOT EXISTS vector_memory (
                memory_key TEXT PRIMARY KEY,
                phase TEXT NOT NULL,
                query_text TEXT NOT NULL,
                response_text TEXT NOT NULL,
                embedding_json TEXT,
                embedding_blob BLOB,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                hit_count INTEGER NOT NULL DEFAULT 0,
                last_hit_at INTEGER,
                user_id TEXT
            );
            ",
        )?;

        // Step 2: Schema migration — add user_id column if it was missing from an
        // older version of the database (CREATE TABLE IF NOT EXISTS won't alter an
        // existing table). Placed BEFORE index creation so indexes on user_id will
        // succeed even on legacy databases.
        if let Err(e) = conn.execute_batch("ALTER TABLE vector_memory ADD COLUMN user_id TEXT;") {
            // "duplicate column name" is expected when the column already exists.
            tracing::debug!("vector store: user_id column migration check ({e})");
        }

        // Step 3: Create indexes and auxiliary tables.
        // These must run after the migration so that indexes on user_id don't fail
        // on legacy databases that were missing the column.
        conn.execute_batch(
            "
            CREATE INDEX IF NOT EXISTS idx_vector_memory_phase_updated_at
                ON vector_memory(phase, updated_at DESC);

            CREATE INDEX IF NOT EXISTS idx_vector_memory_user_id
                ON vector_memory(user_id);

            CREATE TABLE IF NOT EXISTS phase_summary (
                phase TEXT PRIMARY KEY,
                summary_text TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );
            ",
        )?;

        let mode = resolve_sqlite_vector_mode(&conn)?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            dimensions,
            max_entries,
            mode,
            embedding_provider: None,
            hnsw: Arc::new(StdMutex::new(None)),
        })
    }

    /// Create a new vector store with an embedding provider.
    ///
    /// When a provider is supplied it will be used for all embedding;
    /// otherwise the built-in `embed_text()` fallback is used.
    pub fn with_embedding_provider(mut self, provider: ConfigurableEmbeddingProvider) -> Self {
        self.dimensions = provider.dimensions();
        self.embedding_provider = Some(provider);
        self
    }

    /// Create a new vector store configured from environment variables.
    ///
    /// Reads `GO_ON_EMBEDDING_BACKEND` (and provider-specific env vars)
    /// via [`crate::memory::embedding_provider::embedding_provider_from_env`] and
    /// passes the result to [`Self::with_embedding_provider`].
    ///
    /// This is the recommended entry point for production deployments.
    pub fn new_with_env(path: &Path, max_entries: usize) -> Result<Self> {
        let provider = crate::memory::embedding_provider::embedding_provider_from_env();
        let dimensions = provider.dimensions();
        Self::new(path, dimensions, max_entries).map(|s| s.with_embedding_provider(provider))
    }

    /// Upsert a memory entry
    ///
    /// # Arguments
    /// * `phase` - Phase name
    /// * `query_text` - Query text
    /// * `response_text` - Response text
    ///
    /// # Returns
    /// * `Result<()>` - Returns Ok(()) if the entry is upserted successfully, or an error if something goes wrong
    pub async fn upsert(
        self: Arc<Self>,
        phase: &str,
        query_text: &str,
        response_text: &str,
    ) -> Result<()> {
        let phase = phase.to_string();
        let query_text = query_text.to_string();
        let response_text = response_text.to_string();
        spawn_blocking_vec!(move || {
            let query = query_text.trim();
            let response = response_text.trim();
            if query.is_empty() || response.is_empty() {
                return Ok(());
            }
            let embedding = embed_with_check(query, self.dimensions, &self.embedding_provider)?;
            let embedding_json = serde_json::to_string(&embedding)?;
            let embedding_blob = embedding_blob(&embedding);
            let memory_key = build_memory_key(&phase, query);
            let now = now_ts();

            let conn = self.conn.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("vector mutex poisoned in 'upsert', recovering");
                poisoned.into_inner()
            });

            let (json_value, blob_value): (Option<String>, Option<Vec<u8>>) = match self.mode {
                SqliteVectorMode::SqliteVec => (None, Some(embedding_blob)),
                SqliteVectorMode::JsonFallback => (Some(embedding_json), None),
            };

            conn.execute(
                "
            INSERT INTO vector_memory(
                memory_key,
                phase,
                query_text,
                response_text,
                embedding_json,
                embedding_blob,
                created_at,
                updated_at,
                hit_count,
                last_hit_at
            )
            VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, 0, NULL)
            ON CONFLICT(memory_key) DO UPDATE SET
                response_text = excluded.response_text,
                embedding_json = excluded.embedding_json,
                embedding_blob = excluded.embedding_blob,
                updated_at = excluded.updated_at
                ",
                params![memory_key, phase, query, response, json_value, blob_value, now,],
            )?;

            // Evict only when the table actually exceeds the cap. The
            // COUNT gate avoids the full-table ORDER BY sort on every normal
            // write (same pattern as cache.rs / warm_memory).
            let over_cap: i64 = conn.query_row(
                "SELECT COUNT(*) - ?1 FROM vector_memory",
                [self.max_entries as i64],
                |row| row.get(0),
            )?;
            let evicted_keys: Vec<String> = if over_cap > 0 {
                let mut stmt = conn.prepare(
                    "DELETE FROM vector_memory WHERE memory_key IN (
                        SELECT memory_key FROM vector_memory
                        ORDER BY updated_at DESC
                        LIMIT ?2 OFFSET ?1
                    ) RETURNING memory_key",
                )?;
                let rows = stmt.query_map(params![self.max_entries as i64, over_cap], |row| {
                    row.get::<_, String>(0)
                })?;
                rows.filter_map(|r| r.ok()).collect()
            } else {
                Vec::new()
            };

            // Update HNSW index if it exists
            if let Ok(mut hnsw_guard) = self.hnsw.lock() {
                if let Some(ref mut hnsw) = *hnsw_guard {
                    for key in &evicted_keys {
                        hnsw.remove(key);
                    }
                    // Upsert semantics: a re-insert of the same memory_key must
                    // not leave the previous node behind. The SQLite layer
                    // upserts via ON CONFLICT(memory_key); the HNSW mirror must
                    // do the same, otherwise the fast path returns the same
                    // memory_key twice (stale + fresh content) while the
                    // SQLite path returns one row.
                    hnsw.remove(&memory_key);
                    hnsw.insert(
                        embedding,
                        HnswNodeMeta {
                            memory_key,
                            phase: phase.clone(),
                            response_text: response.to_string(),
                            updated_at: now,
                        },
                    );
                }
            }

            Ok(())
        })
    }

    /// Search for similar entries
    ///
    /// # Arguments
    /// * `phase` - Phase name
    /// * `query_text` - Query text
    /// * `top_k` - Maximum number of results to return
    /// * `min_similarity` - Minimum similarity threshold (0.0-1.0)
    /// * `max_snippet_chars` - Maximum number of characters in response snippets
    ///
    /// # Returns
    /// * `Result<(Vec<VectorHit>, VectorPrecisionFeedback)>` - Returns `Ok((Vec<VectorHit>, VectorPrecisionFeedback))` with the search results and precision feedback, or an error if something goes wrong
    pub async fn search(
        self: Arc<Self>,
        phase: &str,
        query_text: &str,
        top_k: usize,
        min_similarity: f32,
        max_snippet_chars: usize,
    ) -> Result<(Vec<VectorHit>, VectorPrecisionFeedback)> {
        let phase = phase.to_string();
        let query_text = query_text.to_string();
        spawn_blocking_vec!(move || {
            if top_k == 0 {
                return Ok((Vec::new(), VectorPrecisionFeedback::new(&[])));
            }

            let query = query_text.trim();
            if query.is_empty() {
                return Ok((Vec::new(), VectorPrecisionFeedback::new(&[])));
            }

            let query_embedding =
                embed_with_check(query, self.dimensions, &self.embedding_provider)?;
            let now = now_ts();
            let limit = self.max_entries.max(top_k);

            // Try HNSW fast path
            if self.ensure_hnsw_index().is_ok() {
                let hnsw_guard = self.hnsw.lock().unwrap_or_else(|poisoned| {
                    tracing::warn!("vector hnsw mutex poisoned in 'search', recovering");
                    poisoned.into_inner()
                });
                if hnsw_guard.is_some() {
                    drop(hnsw_guard);
                    return self.hnsw_search(
                        &query_embedding,
                        &phase,
                        top_k,
                        min_similarity,
                        max_snippet_chars,
                        now,
                    );
                }
            }

            // Collect results within a locked scope, then release the lock
            // before doing sorting/processing (minimizes lock duration).
            let scored = {
                let conn = self.conn.lock().unwrap_or_else(|poisoned| {
                    tracing::warn!("vector mutex poisoned in 'search', recovering");
                    poisoned.into_inner()
                });

                let scored = match self.mode {
                    SqliteVectorMode::SqliteVec => {
                        let query_blob = embedding_blob(&query_embedding);
                        let mut stmt = conn.prepare(
                            "
                        SELECT memory_key, response_text, updated_at,
                               vec_distance_cosine(embedding_blob, ?2) AS distance
                        FROM vector_memory
                        WHERE phase = ?1
                          AND embedding_blob IS NOT NULL
                        ORDER BY distance ASC, updated_at DESC
                        LIMIT ?3
                        ",
                        )?;
                        let mut rows = stmt.query(params![phase, query_blob, limit as i64])?;
                        let mut scored: Vec<(String, f32, String)> = Vec::new();

                        while let Some(row) = rows.next()? {
                            let memory_key: String = row.get(0)?;
                            let response_text: String = row.get(1)?;
                            let updated_at: i64 = row.get(2)?;
                            let distance: f64 = row.get(3)?;
                            let similarity = (1.0_f32 - distance as f32).clamp(0.0, 1.0);
                            if similarity < min_similarity {
                                continue;
                            }
                            let blended =
                                blend_similarity_with_recency(similarity, now, updated_at);
                            scored.push((memory_key, blended, response_text));
                        }
                        scored
                    }
                    SqliteVectorMode::JsonFallback => {
                        let mut stmt = conn.prepare(
                            "
                        SELECT memory_key, response_text, embedding_json, updated_at
                        FROM vector_memory
                        WHERE phase = ?1
                        ORDER BY updated_at DESC
                        LIMIT ?2
                        ",
                        )?;

                        let mut rows = stmt.query(params![phase, limit as i64])?;
                        let mut scored: Vec<(String, f32, String)> = Vec::new();

                        while let Some(row) = rows.next()? {
                            let memory_key: String = row.get(0)?;
                            let response_text: String = row.get(1)?;
                            let embedding_json: Option<String> = row.get(2)?;
                            let updated_at: i64 = row.get(3)?;

                            let Some(embedding_json) = embedding_json else {
                                continue;
                            };
                            let memory_embedding: Vec<f32> =
                                match serde_json::from_str(&embedding_json) {
                                    Ok(v) => v,
                                    Err(_) => continue,
                                };
                            if memory_embedding.len() != query_embedding.len() {
                                continue;
                            }

                            let similarity =
                                cosine_similarity_f32(&query_embedding, &memory_embedding);
                            if similarity < min_similarity {
                                continue;
                            }

                            let blended =
                                blend_similarity_with_recency(similarity, now, updated_at);
                            scored.push((memory_key, blended, response_text));
                        }

                        scored
                    }
                };

                // Update hit counts while still holding the lock
                // BLUE69: Batch all hit count updates into a single SQL statement
                // to avoid N individual UPDATE round-trips per search result.
                if !scored.is_empty() {
                    bump_hit_counts(&conn, &scored, now)?;
                }

                scored
            }; // conn is dropped here, releasing the lock

            let (hits, feedback) = scored_to_hits(scored, top_k, max_snippet_chars);
            Ok((hits, feedback))
        })
    }

    /// Get phase summary
    ///
    /// # Arguments
    /// * `phase` - Phase name
    ///
    /// # Returns
    /// * `Result<Option<String>>` - Returns Ok(Some(String)) if a summary exists, Ok(None) if not, or an error if something goes wrong
    pub async fn get_phase_summary(&self, phase: &str) -> Result<Option<String>> {
        let conn = self.conn.clone();
        let phase = phase.to_string();
        spawn_blocking_vec!(move || {
            let conn = conn.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("vector mutex poisoned in 'get_phase_summary', recovering");
                poisoned.into_inner()
            });

            let summary = conn
                .query_row(
                    &format!(
                        "SELECT summary_text FROM phase_summary WHERE phase = {p}1",
                        p = PARAM_PREFIX
                    ),
                    params![phase],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;

            Ok(summary)
        })
    }

    /// Upsert phase summary
    ///
    /// # Arguments
    /// * `phase` - Phase name
    /// * `summary_text` - Summary text
    ///
    /// # Returns
    /// * `Result<()>` - Returns Ok(()) if the summary is upserted successfully, or an error if something goes wrong
    pub async fn upsert_phase_summary(&self, phase: &str, summary_text: &str) -> Result<()> {
        let conn = self.conn.clone();
        let phase = phase.to_string();
        let text = summary_text.trim().to_string();
        spawn_blocking_vec!(move || {
            if text.is_empty() {
                return Ok(());
            }

            let now = now_ts();
            let conn = conn.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("vector mutex poisoned in 'upsert_phase_summary', recovering");
                poisoned.into_inner()
            });

            conn.execute(
                &format!(
                    "INSERT INTO phase_summary({cols})
                     VALUES({p}1, {p}2, {p}3)
                     ON CONFLICT(phase) DO UPDATE SET
                         summary_text = excluded.summary_text,
                         updated_at    = excluded.updated_at",
                    cols = PHASE_SUMMARY_COLUMNS,
                    p = PARAM_PREFIX,
                ),
                params![phase, text, now],
            )?;

            Ok(())
        })
    }

    /// Get memory entry count
    ///
    /// # Returns
    /// * `Result<u64>` - Returns Ok(u64) with the number of memory entries, or an error if something goes wrong
    pub async fn memory_entry_count(&self) -> Result<u64> {
        let conn = self.conn.clone();
        spawn_blocking_vec!(move || {
            let conn = conn.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("vector mutex poisoned in 'memory_entry_count', recovering");
                poisoned.into_inner()
            });
            let count = conn.query_row("SELECT COUNT(*) FROM vector_memory", [], |row| {
                row.get::<_, i64>(0)
            })?;
            Ok(count.max(0) as u64)
        })
    }

    /// Get summary entry count
    ///
    /// # Returns
    /// * `Result<u64>` - Returns Ok(u64) with the number of summary entries, or an error if something goes wrong
    pub async fn summary_entry_count(&self) -> Result<u64> {
        let conn = self.conn.clone();
        spawn_blocking_vec!(move || {
            let conn = conn.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("vector mutex poisoned in 'summary_entry_count', recovering");
                poisoned.into_inner()
            });
            let count = conn.query_row("SELECT COUNT(*) FROM phase_summary", [], |row| {
                row.get::<_, i64>(0)
            })?;
            Ok(count.max(0) as u64)
        })
    }

    /// Clear all entries
    ///
    /// # Returns
    /// * `Result<(usize, usize)>` - Returns Ok((usize, usize)) with the number of memory entries and summary entries deleted, or an error if something goes wrong
    pub async fn clear_all(&self) -> Result<(usize, usize)> {
        let conn = self.conn.clone();
        let hnsw = self.hnsw.clone();
        spawn_blocking_vec!(move || {
            let conn = conn.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("vector mutex poisoned in 'clear', recovering");
                poisoned.into_inner()
            });
            let memory_deleted = conn.execute("DELETE FROM vector_memory", [])?;
            let summaries_deleted = conn.execute("DELETE FROM phase_summary", [])?;
            drop(conn);

            // Reset the in-memory HNSW index: it is the search fast path, so if
            // it stays populated after the SQLite tables are cleared, the next
            // `search` returns stale entries from before the clear. Setting it
            // to None makes the next search rebuild from the (now empty) tables.
            *hnsw.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("vector hnsw mutex poisoned in 'clear_all', recovering");
                poisoned.into_inner()
            }) = None;
            Ok((memory_deleted, summaries_deleted))
        })
    }

    /// Ensure the in-memory HNSW index is built from SQLite data.
    ///
    /// Reads all vectors from the database and constructs the HNSW graph.
    /// Called lazily on first search when no HNSW index exists yet.
    /// Returns true if the index was built, false if it already existed.
    ///
    /// Lock order convention: `conn` -> `hnsw` (never the reverse), matching
    /// [`VectorStore::upsert`] and [`VectorStore::clear_all`]. The `conn`
    /// guard is held through the build and publication so that a concurrent
    /// `upsert` cannot commit a row between the snapshot and the index
    /// publication (which would silently drop that entry from the search fast
    /// path), and a concurrent `clear_all` cannot wipe the rows the snapshot
    /// was taken from and leave a stale index published afterwards.
    fn ensure_hnsw_index(&self) -> Result<bool> {
        // Lock order: acquire `conn` first. The nested scope only ends the
        // Statement / Rows borrows — the `conn` guard itself stays alive
        // until the index is published (see the convention note above).
        let conn = self.conn.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("vector mutex poisoned in 'ensure_hnsw_index', recovering");
            poisoned.into_inner()
        });

        // Read all entries from SQLite into a Vec.
        let entries: Vec<(Vec<f32>, HnswNodeMeta)> = {
            let mut stmt = conn.prepare(
                "SELECT memory_key, phase, response_text, updated_at, embedding_blob, embedding_json
                 FROM vector_memory
                 ORDER BY updated_at ASC",
            )?;

            let mut entries: Vec<(Vec<f32>, HnswNodeMeta)> = Vec::new();
            let mut rows = stmt.query([])?;

            while let Some(row) = rows.next()? {
                let memory_key: String = row.get(0)?;
                let phase: String = row.get(1)?;
                let response_text: String = row.get(2)?;
                let updated_at: i64 = row.get(3)?;
                let embedding_blob: Option<Vec<u8>> = row.get(4)?;
                let embedding_json: Option<String> = row.get(5)?;

                let embedding: Vec<f32> = match (embedding_blob, embedding_json) {
                    (Some(blob), _) => blob
                        .chunks_exact(4)
                        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                        .collect(),
                    (None, Some(json)) => match serde_json::from_str(&json) {
                        Ok(v) => v,
                        Err(_) => continue,
                    },
                    (None, None) => continue,
                };

                if embedding.len() != self.dimensions {
                    continue;
                }

                entries.push((
                    embedding,
                    HnswNodeMeta {
                        memory_key,
                        phase,
                        response_text,
                        updated_at,
                    },
                ));
            }
            entries
        };

        // Acquire the `hnsw` lock only while already holding `conn`
        // (conn -> hnsw order). Double-check: another thread may have built
        // the index while this thread was snapshotting.
        let mut hnsw_guard = self.hnsw.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("vector hnsw mutex poisoned in 'ensure_hnsw_index', recovering");
            poisoned.into_inner()
        });
        if hnsw_guard.is_some() {
            return Ok(false);
        }

        if entries.is_empty() {
            *hnsw_guard = Some(HnswIndex::new(16, 200, 50));
            return Ok(true);
        }

        let mut hnsw = HnswIndex::new(16, 200, 50);
        for (vector, meta) in entries {
            hnsw.insert(vector, meta);
        }

        *hnsw_guard = Some(hnsw);
        Ok(true)
    }

    /// Search using the HNSW index (internal helper, assumes HNSW exists).
    fn hnsw_search(
        &self,
        query_embedding: &[f32],
        phase: &str,
        top_k: usize,
        min_similarity: f32,
        max_snippet_chars: usize,
        now: i64,
    ) -> Result<(Vec<VectorHit>, VectorPrecisionFeedback)> {
        let hnsw_guard = self.hnsw.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("vector hnsw mutex poisoned in 'hnsw_search', recovering");
            poisoned.into_inner()
        });

        let Some(ref hnsw) = *hnsw_guard else {
            anyhow::bail!("HNSW index not available");
        };

        // Prefetch more candidates than top_k because recency blending may re-rank.
        // `top_k` is user-controlled (request/phase option) with no upper clamp,
        // so the multiply must saturate instead of overflowing (debug panic /
        // release ef corruption for top_k >= 2^62).
        let ef = top_k.saturating_mul(4).max(hnsw.ef_search);
        let results = hnsw.search(query_embedding, ef);

        // Do NOT clone the entire metadata Vec (each entry carries the full
        // response_text — O(n) deep copy on every search). Only the top-k
        // candidates' metadata is touched, so access it while the guard is
        // held instead of cloning it out.
        let mut scored: Vec<(String, f32, String)> = Vec::with_capacity(results.len());
        for nd in &results {
            if nd.dist > 1.0 - min_similarity {
                continue;
            }
            let meta = &hnsw.metadata[nd.idx];
            // The SQLite paths filter by phase (`WHERE phase = ?1`); the HNSW
            // path must do the same so the two paths never disagree on which
            // memories are visible.
            if meta.phase != phase {
                continue;
            }
            let similarity = (1.0_f32 - nd.dist).clamp(0.0, 1.0);
            let blended = blend_similarity_with_recency(similarity, now, meta.updated_at);
            scored.push((meta.memory_key.clone(), blended, meta.response_text.clone()));
        }
        drop(hnsw_guard);

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);

        // Update hit counts in SQLite (batched: single IN-clause query)
        let conn = self.conn.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("vector mutex poisoned in 'hnsw_search::hit_count', recovering");
            poisoned.into_inner()
        });
        if !scored.is_empty() {
            bump_hit_counts(&conn, &scored, now)?;
        }
        drop(conn);

        let (hits, feedback) = scored_to_hits(scored, top_k, max_snippet_chars);
        Ok((hits, feedback))
    }
}

/// Wrapper around the `extern "C"` sqlite3_vec_init symbol that matches
/// the signature expected by sqlite3_auto_extension.
///
/// This avoids undefined behaviour from transmuting a function pointer
/// with one ABI signature to another (Rust ABI vs C ABI).
#[cfg(not(feature = "backend-postgres"))]
// SAFETY: This FFI function is called by SQLite during `sqlite3_auto_extension`.
//
// Why this `unsafe` block is sound:
//
// 1. **Pointer validity** — `_db`, `_pz_err_msg`, and `_p_err_msg` are raw
//    pointers passed by the SQLite runtime which guarantees they are valid
//    at the call site.  The implementation ignores all three anyway.
// 2. **ABI correctness** — The `extern "C"` ABI matches what
//    `sqlite3_auto_extension` expects.  Without this wrapper, casting
//    `sqlite3_vec_init` (which is `extern "C" fn()`) to the
//    `sqlite3_auto_extension` callback signature would be undefined
//    behaviour (calling a function through a pointer with incompatible type).
// 3. **Delegation** — The body calls `sqlite3_vec_init()`, which is the
//    upstream sqlite-vec initialiser re-exported by the `sqlite-vec` Rust
//    crate.  That symbol is statically linked from the vendored C extension.
// 4. **Feature gate** — This function is compiled only when
//    `cfg(not(feature = "backend-postgres"))` is true, which implies the
//    sqlite-vec C extension is linked into the binary.  Without the feature,
//    this module is dead code.
// 5. **Return value** — Returning `0` (SQLITE_OK) is required: any non-zero
//    value causes SQLite to permanently skip this auto-extension, silently
//    disabling vector search.
unsafe extern "C" fn sqlite3_vec_init_auto_extension(
    _db: *mut rusqlite::ffi::sqlite3,
    _pz_err_msg: *mut *mut std::os::raw::c_char,
    _p_err_msg: *const rusqlite::ffi::sqlite3_api_routines,
) -> std::ffi::c_int {
    // The underlying C symbol (declared at the top of sqlite_vec::lib.rs
    // as `extern "C" fn sqlite3_vec_init()`) takes no arguments and
    // returns void.  SQLite calls the auto-extension entry point and
    // ignores the wrapper's return; any non-zero value returned by this
    // wrapper would cause SQLite to skip the extension entirely, so we
    // return 0 (SQLITE_OK) to indicate success.
    sqlite3_vec_init();
    0
}

#[cfg(not(feature = "backend-postgres"))]
fn register_sqlite_vec_auto_extension() {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        // SAFETY: `sqlite3_auto_extension` is a SQLite C API that registers a
        // callback to run on every future `sqlite3_open`. The callback function
        // pointer (`sqlite3_vec_init_auto_extension`) is a valid C ABI function.
        // This is called at most once due to `Once`, before any database connections
        // are opened, so there is no race on the internal SQLite data structures.
        //
        // The `unsafe` block is required because:
        // - `sqlite3_auto_extension` is a foreign (C) function; calling any
        //   FFI function is inherently `unsafe` in Rust.
        // - `sqlite3_vec_init_auto_extension` is the `extern "C"` wrapper
        //   defined above with a signature that matches exactly what
        //   `sqlite3_auto_extension` expects (see its SAFETY comment
        //   for why the wrapper itself is sound).
        // - This code path is gated on `#[cfg(not(feature = "backend-postgres"))]`,
        //   which implies the `sqlite-vec` feature is active and the C extension
        //   is linked into the binary.  Without that feature, this entire
        //   function is dead code.
        // - `Once::call_once` guarantees single-threaded initialisation, so
        //   there is no data race on SQLite's internal auto-extension list.
        unsafe {
            sqlite3_auto_extension(Some(sqlite3_vec_init_auto_extension));
        }
    });
}

#[cfg(not(feature = "backend-postgres"))]
fn resolve_sqlite_vector_mode(conn: &Connection) -> Result<SqliteVectorMode> {
    let probe = conn.query_row("SELECT vec_version()", [], |row| row.get::<_, String>(0));
    match probe {
        Ok(_) => Ok(SqliteVectorMode::SqliteVec),
        Err(err) => {
            #[cfg(all(
                feature = "local",
                not(feature = "simple-server"),
                not(feature = "multi-users-server")
            ))]
            {
                tracing::warn!(
                    "sqlite-vec unavailable, falling back to JSON embedding table for local: {}",
                    err
                );
                Ok(SqliteVectorMode::JsonFallback)
            }

            #[cfg(not(all(
                feature = "local",
                not(feature = "simple-server"),
                not(feature = "multi-users-server")
            )))]
            {
                Err(anyhow::anyhow!(
                    "sqlite-vec is required for this build profile but failed to initialize: {}",
                    err
                ))
            }
        }
    }
}

#[cfg(not(feature = "backend-postgres"))]
fn embedding_blob(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(values));
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

fn blend_similarity_with_recency(similarity: f32, now: i64, updated_at: i64) -> f32 {
    // Recency blending: similarity carries 70% weight, recency 30%. The
    // decay factor 0.05 halves the recency term after ~20 days of age.
    const DECAY_FACTOR: f64 = 0.05;
    let age_secs = (now - updated_at).max(0) as f64;
    let age_days = age_secs / 86_400.0;
    let recency_weight = (1.0 / (1.0 + age_days * DECAY_FACTOR)) as f32;
    similarity * 0.70 + recency_weight * 0.30
}

/// Embed text using the canonical minhash implementation (avoids code duplication).
fn embed_text(text: &str, dimensions: usize) -> Vec<f32> {
    local_hash_embed(text, dimensions)
}

/// Shared embedding helper: dispatches to the configured provider or the minhash
/// fallback, and validates that the returned vector has the expected dimension.
///
/// Used by both the SQLite and PostgreSQL backends to eliminate the identical
/// 12-line dimension-checking pattern that was duplicated across every method.
fn embed_with_check(
    query: &str,
    dimensions: usize,
    provider: &Option<ConfigurableEmbeddingProvider>,
) -> Result<Vec<f32>> {
    if let Some(ref provider) = provider {
        let vec = provider.embed(query);
        if vec.len() != dimensions {
            anyhow::bail!(
                "Embedding dimension mismatch: got {} but store expects {} dimensions",
                vec.len(),
                dimensions,
            );
        }
        // Zero-vector guard: the OpenAI provider returns `vec![0.0; dims]` to
        // signal an API failure, and the Ollama/Qwen3 zero-signal path is
        // reachable when `fallback_to_hash` is disabled. A zero vector has
        // cosine similarity NaN against every other vector (0/0), silently
        // polluting semantic matching — reject it instead of storing/searching
        // with it. (Dimensions are always > 0 in production, so an all-zero
        // vector here is a failure signal, not a degenerate-but-legit embed.)
        if vec.iter().all(|v| *v == 0.0) {
            anyhow::bail!(
                "Embedding provider returned an all-zero vector ({} dims) — treating it as an embedding failure; refusing to store/search",
                vec.len(),
            );
        }
        Ok(vec)
    } else {
        Ok(embed_text(query, dimensions))
    }
}

fn trim_chars(input: &str, max_chars: usize) -> String {
    input.chars().take(max_chars).collect()
}

/// Convert `(memory_key, blended_score, response_text)` tuples into sorted,
/// truncated hits with precision feedback.
fn scored_to_hits(
    mut scored: Vec<(String, f32, String)>,
    top_k: usize,
    max_snippet_chars: usize,
) -> (Vec<VectorHit>, VectorPrecisionFeedback) {
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(top_k);
    let hits: Vec<VectorHit> = scored
        .into_iter()
        .map(|(_, blended_score, text)| VectorHit {
            similarity: blended_score,
            response_snippet: trim_chars(&text, max_snippet_chars),
        })
        .collect();
    let feedback = VectorPrecisionFeedback::new(&hits);
    (hits, feedback)
}

/// Batch-increment `hit_count`/`last_hit_at` for the given scored keys in a
/// single SQL `IN` statement. Shared by the SQLite and HNSW search paths —
/// previously each had a verbatim copy of the placeholder-building loop.
#[cfg(not(feature = "backend-postgres"))]
fn bump_hit_counts(
    conn: &rusqlite::Connection,
    scored: &[(String, f32, String)],
    now: i64,
) -> anyhow::Result<()> {
    let placeholders: Vec<String> = (0..scored.len()).map(|i| format!("?{}", i + 1)).collect();
    let sql = format!(
        "UPDATE vector_memory SET hit_count = hit_count + 1, last_hit_at = ?{n_plus_1} WHERE memory_key IN ({})",
        placeholders.join(", "),
        n_plus_1 = scored.len() + 1
    );
    let mut params: Vec<&dyn rusqlite::types::ToSql> = Vec::with_capacity(scored.len() + 1);
    for (memory_key, _, _) in scored {
        params.push(memory_key);
    }
    params.push(&now);
    conn.execute(&sql, params.as_slice())?;
    Ok(())
}

fn build_memory_key(phase: &str, query_text: &str) -> String {
    let payload = format!("{}|{}", phase, query_text.trim());
    crate::shared::sha256_hex(payload.as_bytes())
}

#[cfg(all(test, not(feature = "backend-postgres")))]
mod tests {
    use super::{HnswIndex, HnswNodeMeta, VectorPrecisionFeedback, VectorStore};
    use std::sync::Arc;

    #[tokio::test]
    async fn vector_store_upsert_and_search() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let db_path = dir.path().join("vector.sqlite3");

        let store =
            Arc::new(VectorStore::new(&db_path, 64, 200).expect("vector store init should work"));
        Arc::clone(&store)
            .upsert(
                "coding",
                "optimize rust async cache",
                "Use sqlite cache and tune ttl for repeated requests.",
            )
            .await
            .expect("upsert should work");

        let (hits, feedback) = Arc::clone(&store)
            .search("coding", "how to optimize async cache", 2, 0.1, 200)
            .await
            .expect("search should work");
        assert!(!hits.is_empty());
        assert!(feedback.hit_count > 0);
        assert!(feedback.avg_similarity > 0.0);
    }

    #[tokio::test]
    async fn vector_store_phase_summary_roundtrip() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let db_path = dir.path().join("vector.sqlite3");

        let store =
            Arc::new(VectorStore::new(&db_path, 64, 200).expect("vector store init should work"));
        store
            .upsert_phase_summary("coding", "short summary")
            .await
            .expect("upsert summary should work");

        let summary = store
            .get_phase_summary("coding")
            .await
            .expect("get summary should work");
        assert_eq!(summary.as_deref(), Some("short summary"));
    }

    #[test]
    fn vector_precision_feedback_calculates_average_similarity() {
        use super::VectorHit;

        let hits = vec![
            VectorHit {
                similarity: 0.9,
                response_snippet: "test1".to_string(),
            },
            VectorHit {
                similarity: 0.8,
                response_snippet: "test2".to_string(),
            },
            VectorHit {
                similarity: 0.7,
                response_snippet: "test3".to_string(),
            },
        ];

        let feedback = VectorPrecisionFeedback::new(&hits);
        assert_eq!(feedback.hit_count, 3);
        assert!((feedback.avg_similarity - 0.8).abs() < 0.01); // (0.9 + 0.8 + 0.7) / 3 = 0.8
    }

    #[test]
    fn vector_precision_feedback_handles_empty_hits() {
        let hits = vec![];
        let feedback = VectorPrecisionFeedback::new(&hits);
        assert_eq!(feedback.hit_count, 0);
        assert!((feedback.avg_similarity - 0.0).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn vector_search_time_decay_demotes_stale_entry() {
        #[cfg(not(feature = "backend-postgres"))]
        use rusqlite::{params, Connection};

        let dir = tempfile::tempdir().expect("temp dir should be created");
        let db_path = dir.path().join("vector_decay.sqlite3");

        let store =
            Arc::new(VectorStore::new(&db_path, 64, 200).expect("vector store init should work"));

        // Insert a fresh entry with an identical query to get identical embeddings.
        Arc::clone(&store)
            .upsert("coding", "rust async performance", "fresh answer")
            .await
            .expect("fresh upsert should work");

        // Back-date an entry to 180 days ago directly in SQLite to simulate stale knowledge.
        // The memory_key is deterministic from (phase, query_text).
        let stale_ts: i64 = super::now_ts() - 180 * 86_400;
        {
            let conn = Connection::open(&db_path).expect("should open db");
            let embedding = super::local_hash_embed("rust async performance stale", 64);
            let embedding_json =
                serde_json::to_string(&embedding).expect("should serialize embedding");
            let embedding_blob = super::embedding_blob(&embedding);
            let (json_value, blob_value): (Option<String>, Option<Vec<u8>>) = match store.mode {
                super::SqliteVectorMode::SqliteVec => (None, Some(embedding_blob)),
                super::SqliteVectorMode::JsonFallback => (Some(embedding_json), None),
            };

            conn.execute(
                "INSERT OR REPLACE INTO vector_memory(
                    memory_key,
                    phase,
                    query_text,
                    response_text,
                    embedding_json,
                    embedding_blob,
                    created_at,
                    updated_at,
                    hit_count
                 )
                 VALUES('__stale_key__', 'coding', 'rust async performance stale', 'stale answer', ?1, ?2, ?3, ?3, 0)",
                params![json_value, blob_value, stale_ts],
            )
            .expect("stale insert should work");
        }

        // The fresh entry should rank higher than the stale one despite similar embeddings.
        let (hits, _) = Arc::clone(&store)
            .search("coding", "rust async performance", 5, 0.0, 200)
            .await
            .expect("search should work");

        // Verify fresh entry ranked first (highest blended score).
        let first_snippet = hits
            .first()
            .map(|h| h.response_snippet.as_str())
            .unwrap_or("");
        assert!(
            first_snippet.contains("fresh"),
            "fresh entry should rank first but got: {first_snippet:?}"
        );
    }

    #[tokio::test]
    async fn hnsw_index_insert_and_search_basic() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let db_path = dir.path().join("hnsw_basic.sqlite3");
        let store = Arc::new(VectorStore::new(&db_path, 64, 200).expect("vector store init"));

        // Insert enough entries to trigger HNSW construction
        for i in 0..50 {
            let query = format!("rust feature number {i}");
            let response = format!("response for feature {i}");
            Arc::clone(&store)
                .upsert("test", &query, &response)
                .await
                .expect("upsert");
        }

        // Trigger HNSW build by calling ensure_hnsw_index
        store.ensure_hnsw_index().expect("ensure_hnsw_index");

        // Search via HNSW path
        let (hits, feedback) = Arc::clone(&store)
            .search("test", "rust feature number 5", 5, 0.0, 200)
            .await
            .expect("hnsw search");
        assert!(!hits.is_empty(), "HNSW search should return hits");
        assert!(
            feedback.avg_similarity > 0.0,
            "should have meaningful similarity"
        );
        assert!(
            (0..50).any(|i| hits[0].response_snippet.contains(&format!("feature {i}"))),
            "top result should be near query: got {:?}",
            hits[0].response_snippet
        );
    }

    #[tokio::test]
    async fn hnsw_index_functional_test() {
        // Functional test: validates HNSW build + search with a moderate dataset.
        // Uses 100 vectors (not 10K) for fast execution in CI.
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let db_path = dir.path().join("hnsw_func.sqlite3");
        let store = Arc::new(VectorStore::new(&db_path, 128, 500).expect("vector store init"));

        // Insert 100 vectors - enough to validate the HNSW index works
        for i in 0..100 {
            let query = format!("functional test query number {i}");
            let response = format!("functional test response {i}");
            Arc::clone(&store)
                .upsert("bench", &query, &response)
                .await
                .expect("upsert");
        }

        // Build the HNSW index
        let built = store.ensure_hnsw_index().expect("ensure_hnsw_index");
        assert!(built, "HNSW index should be built");

        // Run searches and verify results are returned correctly
        // Note: with hash-based embeddings and 100 vectors, the top result
        // may not always be the exact semantic match. We verify that:
        // 1. Results are returned for each query
        // 2. At least one of the top-10 results matches each query index
        for query_idx in [0, 25, 50, 99] {
            let query = format!("functional test query number {query_idx}");
            let (hits, _) = Arc::clone(&store)
                .search("bench", &query, 10, 0.0, 200)
                .await
                .expect("search should succeed");
            assert!(
                !hits.is_empty(),
                "should find results for query {query_idx}"
            );
            let found = hits.iter().any(|h| {
                h.response_snippet
                    .contains(&format!("response {query_idx}"))
            });
            assert!(
                found,
                "query={query_idx} should be in top-10 results, top={}",
                hits[0].response_snippet
            );
        }
    }

    #[tokio::test]
    async fn hnsw_insert_empty_and_build() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let db_path = dir.path().join("hnsw_empty.sqlite3");
        let store = Arc::new(VectorStore::new(&db_path, 64, 100).expect("vector store init"));

        // Build HNSW with empty DB
        let built = store.ensure_hnsw_index().expect("ensure_hnsw_index empty");
        assert!(built, "should build empty index");

        // Search on empty index should return no results
        let (hits, feedback) = Arc::clone(&store)
            .search("test", "something", 5, 0.0, 200)
            .await
            .expect("search on empty");
        assert!(hits.is_empty());
        assert_eq!(feedback.hit_count, 0);
    }

    /// Regression (P2): re-upserting the same (phase, query) must replace the
    /// previous HNSW node instead of appending a duplicate.
    ///
    /// Before the fix, `upsert` inserted into the HNSW index without removing
    /// a pre-existing node with the same memory_key (only evicted keys were
    /// removed). Re-upserting a hot key then returned the same memory_key twice
    /// (stale + fresh content) from the fast path while the SQLite path returned
    /// one row — and the index accumulated dead nodes unboundedly.
    #[tokio::test]
    async fn hnsw_reupsert_replaces_previous_node() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let db_path = dir.path().join("hnsw_reupsert.sqlite3");
        let store = Arc::new(VectorStore::new(&db_path, 64, 200).expect("vector store init"));

        // Seed the store, then force the HNSW index to be built.
        Arc::clone(&store)
            .upsert("test", "same query", "old answer")
            .await
            .expect("first upsert");
        assert!(
            store.ensure_hnsw_index().expect("ensure_hnsw_index"),
            "HNSW index should be built"
        );

        // Re-upsert the same query with new content.
        Arc::clone(&store)
            .upsert("test", "same query", "fresh answer")
            .await
            .expect("re-upsert");

        // The HNSW fast path must return exactly one hit carrying the fresh
        // content — never the stale one, and never the same key twice.
        let (hits, _) = Arc::clone(&store)
            .search("test", "same query", 5, 0.0, 200)
            .await
            .expect("search after re-upsert");
        let fresh: Vec<&str> = hits
            .iter()
            .map(|h| h.response_snippet.as_str())
            .filter(|s| s.contains("fresh"))
            .collect();
        let stale: Vec<&str> = hits
            .iter()
            .map(|h| h.response_snippet.as_str())
            .filter(|s| s.contains("old answer"))
            .collect();
        assert_eq!(fresh.len(), 1, "exactly one fresh hit, got {fresh:?}");
        assert!(
            stale.is_empty(),
            "stale content must not survive, got {stale:?}"
        );
    }

    /// Regression (P2): evicting the HNSW entry-point node must re-point the
    /// entry point to a live node instead of starting searches from a dead
    /// (zeroed) node. Tested directly on `HnswIndex` (private) because the
    /// store-level eviction tie-breaks on same-second `updated_at`, which is
    /// non-deterministic for churn within one second.
    #[test]
    fn hnsw_remove_repairs_entry_point() {
        let mut hnsw = HnswIndex::new(16, 200, 50);
        // Insert three nodes. The entry point is the node with the highest
        // random level — with `random_level()` on fastrand, that is NOT
        // deterministically node 0 (the test previously asserted `Some(0)`
        // and flaked under parallel runs), so the invariant is asserted
        // against whichever node was selected.
        for i in 0..3 {
            let mut vec = vec![0.0f32; 64];
            vec[i] = 1.0; // distinct vectors
            hnsw.insert(
                vec,
                HnswNodeMeta {
                    memory_key: format!("key-{i}"),
                    phase: "test".to_string(),
                    response_text: format!("answer {i}"),
                    updated_at: i as i64,
                },
            );
        }
        let entry = hnsw.entry_point.expect("entry point after inserts");
        assert!(
            !hnsw.metadata[entry].memory_key.is_empty(),
            "entry point must be a live node, got idx {entry}"
        );

        // Remove the entry point; the index must re-point to a live node.
        let entry_key = hnsw.metadata[entry].memory_key.clone();
        hnsw.remove(&entry_key);
        let Some(ep) = hnsw.entry_point else {
            panic!("entry point must be repaired to a live node");
        };
        assert!(
            !hnsw.metadata[ep].memory_key.is_empty(),
            "repaired entry point must be live, got idx {ep}"
        );

        // Removing all nodes clears the entry point entirely.
        for i in 0..3 {
            hnsw.remove(&format!("key-{i}"));
        }
        assert_eq!(hnsw.entry_point, None, "entry point cleared when empty");
    }

    /// Regression (P2): `remove` must delete ALL nodes with a matching
    /// memory_key — leaving a second stale copy would duplicate results on
    /// the search fast path.
    #[test]
    fn hnsw_remove_deletes_all_matching_nodes() {
        let mut hnsw = HnswIndex::new(16, 200, 50);
        for i in 0..3 {
            let mut vec = vec![0.0f32; 64];
            vec[i] = 1.0;
            // Two nodes share the same memory_key (simulates the pre-fix
            // duplicate-node state after a re-upsert).
            hnsw.insert(
                vec.clone(),
                HnswNodeMeta {
                    memory_key: "dup-key".to_string(),
                    phase: "test".to_string(),
                    response_text: format!("answer {i}"),
                    updated_at: i as i64,
                },
            );
        }
        hnsw.remove("dup-key");
        assert!(
            hnsw.metadata.iter().all(|m| m.memory_key.is_empty()),
            "all nodes with the matching key must be removed"
        );
        assert_eq!(
            hnsw.entry_point, None,
            "all nodes removed => no entry point"
        );
    }

    /// Regression (P2): `clear_all` must reset the in-memory HNSW index.
    ///
    /// Previously it only issued the SQLite DELETEs; the HNSW index kept the
    /// old vectors, so the next `search` took the HNSW fast path and returned
    /// stale entries that no longer existed in the database.
    #[tokio::test]
    async fn clear_all_resets_hnsw_index() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let db_path = dir.path().join("hnsw_clear.sqlite3");
        let store = Arc::new(VectorStore::new(&db_path, 64, 200).expect("vector store init"));

        for i in 0..20 {
            let query = format!("rust feature number {i}");
            let response = format!("response for feature {i}");
            Arc::clone(&store)
                .upsert("test", &query, &response)
                .await
                .expect("upsert");
        }
        // Force the HNSW index to be built (search fast path).
        let built = store.ensure_hnsw_index().expect("ensure_hnsw_index");
        assert!(built, "HNSW index should be built");

        // Search before clear hits the HNSW fast path and returns entries.
        let (hits, _) = Arc::clone(&store)
            .search("test", "rust feature number 5", 5, 0.0, 200)
            .await
            .expect("search before clear");
        assert!(!hits.is_empty(), "search before clear should return hits");

        let (memory_deleted, _) = store.clear_all().await.expect("clear_all should succeed");
        assert_eq!(memory_deleted, 20, "all 20 entries should be deleted");

        // After clear_all the HNSW index must be gone: a search must NOT
        // return the entries that existed before the clear.
        let (hits, feedback) = Arc::clone(&store)
            .search("test", "rust feature number 5", 5, 0.0, 200)
            .await
            .expect("search after clear");
        assert!(
            hits.is_empty(),
            "stale HNSW entries must not survive clear_all: {hits:?}"
        );
        assert_eq!(feedback.hit_count, 0);
    }

    /// Regression (P1): a concurrent `upsert` + first `search` must not
    /// deadlock, and the lazily-built HNSW index must not lose entries that
    /// commit while the first build is in flight.
    ///
    /// Before the lock-order fix, `ensure_hnsw_index` held `hnsw` while
    /// waiting for `conn` and `upsert` held `conn` while waiting for `hnsw`,
    /// forming a ring that could hang both threads forever.
    #[tokio::test]
    async fn concurrent_upsert_and_first_search_no_deadlock() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let db_path = dir.path().join("hnsw_concurrent.sqlite3");
        let store = Arc::new(VectorStore::new(&db_path, 64, 10_000).expect("vector store init"));

        // A search task that triggers the lazy HNSW build on its first call,
        // racing against the upserts below.
        let search_store = Arc::clone(&store);
        let search_task = tokio::spawn(async move {
            for i in 0..10 {
                Arc::clone(&search_store)
                    .search("concurrent", &format!("query {i}"), 10, 0.0, 200)
                    .await
                    .expect("search should succeed");
            }
        });

        // Concurrently upsert entries; some of them may commit while the
        // lazy index build is snapshotting or publishing.
        for i in 0..200 {
            Arc::clone(&store)
                .upsert(
                    "concurrent",
                    &format!("query {i}"),
                    &format!("response {i}"),
                )
                .await
                .expect("upsert");
        }

        search_task.await.expect("search task should not panic");

        // Every upserted entry must be present in the published index: the
        // build must not publish a snapshot that misses concurrent writes.
        let hnsw_guard = store.hnsw.lock().expect("hnsw lock");
        let hnsw = hnsw_guard.as_ref().expect("index should be built");
        assert_eq!(
            hnsw.metadata.len(),
            200,
            "no upserted entry may be lost from the HNSW index"
        );
    }
}

// ─── PostgreSQL backend (multi-users-server) ─────────────────────────
//
// Embeddings are computed in Rust (sha2-based projection, same as SQLite backend)
// and stored in a native `pgvector` column through the synchronous `postgres`
// client. Methods expose the same sync signature as the SQLite backend.
#[cfg(feature = "backend-postgres")]
use crate::memory::embedding_provider::{
    local_hash_embed, ConfigurableEmbeddingProvider, EmbeddingProvider,
};
#[cfg(feature = "backend-postgres")]
use crate::memory::pg_migrate::run_migrations;
#[cfg(feature = "backend-postgres")]
use crate::memory::pg_pool::{
    connect_postgres, create_pool, create_pool_pair, pool_get, resolve_pg_dsn, PgPoolPair,
};

#[cfg(feature = "backend-postgres")]
pub struct VectorStore {
    pool: PgPoolPair,
    dimensions: usize,
    max_entries: usize,
    /// Optional embedding provider — overrides the built-in `embed_text()`.
    ///
    /// In production, inject via [`VectorStore::with_embedding_provider`].
    /// When `None`, the built-in minhash fallback (`embed_text()`) is used,
    /// which is only suitable for development/testing.
    embedding_provider: Option<ConfigurableEmbeddingProvider>,
}

#[cfg(feature = "backend-postgres")]
impl std::fmt::Debug for VectorStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VectorStore")
            .field("pool", &"<PgPoolPair>")
            .field("dimensions", &self.dimensions)
            .field("max_entries", &self.max_entries)
            .field(
                "embedding_provider",
                &self
                    .embedding_provider
                    .as_ref()
                    .map(|_| "<ConfigurableEmbeddingProvider>"),
            )
            .finish()
    }
}

#[cfg(feature = "backend-postgres")]
impl VectorStore {
    /// Connect to PostgreSQL and run schema migrations.
    ///
    /// `url` — libpq-style connection string, e.g.
    /// `"postgres://user:pass@localhost/go_on"`
    ///
    /// Supports `sslmode` parameter in the URL:
    /// - `sslmode=require`      — TLS with server verification disabled
    /// - `sslmode=verify-ca`    — TLS with CA verification
    /// - `sslmode=verify-full`  — TLS with CA + hostname verification
    /// - absent, `disable`, `allow`, `prefer` — No TLS (plain connection)
    pub fn new(url: &str, dimensions: usize, max_entries: usize) -> Result<Self> {
        Self::new_with_replica(url, None, dimensions, max_entries)
    }

    /// Create a new vector store with an optional read-replica connection for read/write splitting.
    ///
    /// When `read_replica_url` is `Some`, read queries use the replica pool;
    /// when `None`, the primary pool is used for both reads and writes.
    ///
    /// The DSN is resolved through the canonical `pg_pool::resolve_pg_dsn`
    /// resolver (config `connection_string` → `GO_ON_PG_CONNECTION_STRING` →
    /// `DATABASE_URL` → `PG_DSN` → `GO_ON_DATABASE_URL`), keeping the fallback
    /// order identical to the response cache and memory warm tier.
    pub fn new_with_replica(
        url: &str,
        read_replica_url: Option<String>,
        dimensions: usize,
        max_entries: usize,
    ) -> Result<Self> {
        if dimensions == 0 {
            anyhow::bail!("vector.dimensions must be greater than 0 for pgvector backend");
        }

        let url = resolve_pg_dsn(Some(url)).ok_or_else(|| {
            anyhow::anyhow!(
                "no PostgreSQL connection string configured (set config vector.connection_string, GO_ON_PG_CONNECTION_STRING, DATABASE_URL, PG_DSN or GO_ON_DATABASE_URL)"
            )
        })?;
        let max_pool_size = crate::memory::pg_pool::DEFAULT_PG_POOL_SIZE;
        let write_url = url;
        let write_connect = move || connect_postgres(&write_url);

        let pool = match &read_replica_url {
            Some(replica_url) => {
                let replica_url = replica_url.clone();
                let read_connect = move || connect_postgres(&replica_url);
                create_pool_pair(
                    write_connect,
                    read_replica_url.clone(),
                    read_connect,
                    max_pool_size,
                )
            }
            None => {
                let single = create_pool(write_connect, max_pool_size);
                PgPoolPair {
                    write: single.clone(),
                    read: single,
                }
            }
        };

        // Run schema migrations on the write pool (creates base tables).
        let mut conn = pool_get(&pool.write)?;

        // Ensure pgvector extension is available.
        conn.batch_execute("CREATE EXTENSION IF NOT EXISTS vector")?;

        // Run base migrations (v2 creates vector_memory + phase_summary).
        run_migrations(&mut conn, 2)?;

        // Align the embedding column with the runtime dimensions. The base
        // migration (pg_migrate v2) creates a fixed `vector(768)` column, but
        // the runtime dimension comes from the embedding provider (128 local /
        // 1536 openai / 768 ollama). Without this re-type every upsert fails
        // with a pgvector dimension mismatch. It runs at every startup so both
        // fresh and already-migrated databases converge to the configured dims.
        let dims_sql =
            format!("ALTER TABLE vector_memory ALTER COLUMN embedding TYPE vector({dimensions});");
        conn.batch_execute(&dims_sql)?;

        // Dynamic DDL: HNSW index uses the configured dimensions.
        // The base table was created by the migration; the HNSW index
        // is added here since its dimensions are configurable.
        conn.batch_execute(
            "CREATE INDEX IF NOT EXISTS idx_vector_memory_embedding_cosine
             ON vector_memory USING hnsw (embedding vector_cosine_ops);",
        )?;

        // Startup health check: verify the connection is alive.
        conn.query_one("SELECT 1", &[])
            .map_err(|e| anyhow::anyhow!("postgres health check (SELECT 1) failed: {e}"))?;

        Ok(Self {
            pool,
            dimensions,
            max_entries,
            embedding_provider: None,
        })
    }

    /// Create a new vector store with an embedding provider.
    ///
    /// When a provider is supplied it will be used for all embedding;
    /// otherwise the built-in `embed_text()` fallback is used.
    pub fn with_embedding_provider(mut self, provider: ConfigurableEmbeddingProvider) -> Self {
        self.dimensions = provider.dimensions();
        self.embedding_provider = Some(provider);
        self
    }

    /// Create a new vector store configured from environment variables.
    ///
    /// Reads the connection string from the canonical `pg_pool::resolve_pg_dsn`
    /// resolver (config `connection_string` → `GO_ON_PG_CONNECTION_STRING` →
    /// `DATABASE_URL` → `PG_DSN` → `GO_ON_DATABASE_URL`) and
    /// `GO_ON_EMBEDDING_BACKEND` (embedding provider) from the environment.
    ///
    /// This is the recommended entry point for production deployments.
    pub fn new_with_env(max_entries: usize) -> Result<Self> {
        let url = resolve_pg_dsn(None).ok_or_else(|| {
            anyhow::anyhow!(
                "no PostgreSQL connection string configured (set GO_ON_PG_CONNECTION_STRING, DATABASE_URL, PG_DSN or GO_ON_DATABASE_URL) for postgres backend"
            )
        })?;
        let provider = crate::memory::embedding_provider::embedding_provider_from_env();
        let dimensions = provider.dimensions();
        Self::new(&url, dimensions, max_entries).map(|s| s.with_embedding_provider(provider))
    }

    pub async fn upsert(
        self: Arc<Self>,
        phase: &str,
        query_text: &str,
        response_text: &str,
    ) -> Result<()> {
        let phase = phase.to_string();
        let query_text = query_text.to_string();
        let response_text = response_text.to_string();
        spawn_blocking_vec!(move || {
            let query = query_text.trim();
            let response = response_text.trim();
            if query.is_empty() || response.is_empty() {
                return Ok(());
            }
            let embedding_vec = embed_with_check(query, self.dimensions, &self.embedding_provider)?;
            let embedding = Vector::from(embedding_vec);
            let memory_key = build_memory_key(&phase, query);
            let now = now_ts();
            let max_entries = self.max_entries as i64;
            let mut client = pool_get(&self.pool.write)?;
            client.execute(
                "INSERT INTO vector_memory
                    (memory_key, phase, query_text, response_text, embedding,
                     created_at, updated_at, hit_count)
                 VALUES ($1, $2, $3, $4, $5, $6, $6, 0)
                 ON CONFLICT (memory_key) DO UPDATE SET
                    response_text  = EXCLUDED.response_text,
                    embedding      = EXCLUDED.embedding,
                    updated_at     = EXCLUDED.updated_at",
                &[
                    &memory_key,
                    &phase,
                    &query,
                    &response_text,
                    &embedding,
                    &now,
                ],
            )?;

            // Evict only when the table actually exceeds the cap. The COUNT
            // gate avoids the full-table ORDER BY sort + DELETE on every normal
            // write, matching the SQLite path (vector.rs upsert / cache.rs).
            let over_cap: i64 = client
                .query_one("SELECT COUNT(*) - $1 FROM vector_memory", &[&max_entries])?
                .get(0);
            if over_cap > 0 {
                client.execute(
                    "DELETE FROM vector_memory
                     WHERE memory_key NOT IN (
                         SELECT memory_key FROM vector_memory
                         ORDER BY updated_at DESC LIMIT $1
                     )",
                    &[&max_entries],
                )?;
            }

            Ok(())
        })
    }

    pub async fn search(
        self: Arc<Self>,
        phase: &str,
        query_text: &str,
        top_k: usize,
        min_similarity: f32,
        max_snippet_chars: usize,
    ) -> Result<(Vec<VectorHit>, VectorPrecisionFeedback)> {
        let phase = phase.to_string();
        let query_text = query_text.to_string();
        spawn_blocking_vec!(move || {
            if top_k == 0 {
                return Ok((Vec::new(), VectorPrecisionFeedback::new(&[])));
            }
            let query = query_text.trim();
            if query.is_empty() {
                return Ok((Vec::new(), VectorPrecisionFeedback::new(&[])));
            }

            let query_vec = embed_with_check(query, self.dimensions, &self.embedding_provider)?;
            let query_embedding = Vector::from(query_vec);
            let now = now_ts();
            // Mirror the SQLite path: cap the scan at max_entries but never
            // below the requested top_k (previously a hardcoded LIMIT 300
            // silently truncated results for top_k > 300 and scanned 300 rows
            // even when max_entries < 300).
            let limit = self.max_entries.max(top_k);
            let mut client = pool_get(&self.pool.read)?;
            let rows = client.query(
                &format!(
                    "SELECT memory_key, response_text, updated_at,
                            1 - (embedding <=> {p}2) AS similarity
                     FROM vector_memory
                     WHERE phase = {p}1
                     ORDER BY embedding <=> {p}2
                     LIMIT {p}3",
                    p = PARAM_PREFIX,
                ),
                &[&phase, &query_embedding, &(limit as i64)],
            )?;

            let mut scored: Vec<(String, f32, String)> = Vec::new();
            for row in rows {
                let memory_key: String = row.get(0);
                let response_text: String = row.get(1);
                let updated_at: i64 = row.get(2);
                let similarity = (row.get::<_, f64>(3) as f32).clamp(0.0, 1.0);
                if similarity < min_similarity {
                    continue;
                }
                let blended = blend_similarity_with_recency(similarity, now, updated_at);
                scored.push((memory_key, blended, response_text));
            }

            if !scored.is_empty() {
                // Batched: single UPDATE with IN clause using PostgreSQL array
                let keys: Vec<String> = scored.iter().map(|(k, _, _)| k.clone()).collect();
                let _ = client.execute(
                    &format!(
                        "UPDATE vector_memory
                         SET hit_count = hit_count + 1, last_hit_at = {p}2
                         WHERE memory_key = ANY({p}1::text[])",
                        p = PARAM_PREFIX,
                    ),
                    &[&keys, &now],
                );
            }

            let (hits, feedback) = scored_to_hits(scored, top_k, max_snippet_chars);
            Ok((hits, feedback))
        })
    }

    pub async fn get_phase_summary(&self, phase: &str) -> Result<Option<String>> {
        let pool = self.pool.read.clone();
        let phase = phase.to_string();
        spawn_blocking_vec!(move || {
            let mut client = pool_get(&pool)?;
            Ok(client
                .query_opt(
                    &format!(
                        "SELECT summary_text FROM phase_summary WHERE phase = {p}1",
                        p = PARAM_PREFIX
                    ),
                    &[&phase],
                )?
                .map(|row| row.get(0)))
        })
    }

    pub async fn upsert_phase_summary(&self, phase: &str, summary_text: &str) -> Result<()> {
        let pool = self.pool.write.clone();
        let phase = phase.to_string();
        let text = summary_text.trim().to_string();
        spawn_blocking_vec!(move || {
            if text.is_empty() {
                return Ok(());
            }
            let mut client = pool_get(&pool)?;
            let now = now_ts();
            client.execute(
                &format!(
                    "INSERT INTO phase_summary ({cols})
                     VALUES ({p}1, {p}2, {p}3)
                     ON CONFLICT (phase) DO UPDATE SET
                         summary_text = EXCLUDED.summary_text,
                         updated_at   = EXCLUDED.updated_at",
                    cols = PHASE_SUMMARY_COLUMNS,
                    p = PARAM_PREFIX,
                ),
                &[&phase, &text, &now],
            )?;
            Ok(())
        })
    }

    pub async fn memory_entry_count(&self) -> Result<u64> {
        let pool = self.pool.read.clone();
        spawn_blocking_vec!(move || {
            let mut client = pool_get(&pool)?;
            let row = client.query_one("SELECT COUNT(*) FROM vector_memory", &[])?;
            let count: i64 = row.get(0);
            Ok(count.max(0) as u64)
        })
    }

    pub async fn summary_entry_count(&self) -> Result<u64> {
        let pool = self.pool.read.clone();
        spawn_blocking_vec!(move || {
            let mut client = pool_get(&pool)?;
            let row = client.query_one("SELECT COUNT(*) FROM phase_summary", &[])?;
            let count: i64 = row.get(0);
            Ok(count.max(0) as u64)
        })
    }

    pub async fn clear_all(&self) -> Result<(usize, usize)> {
        let pool = self.pool.write.clone();
        spawn_blocking_vec!(move || {
            let mut client = pool_get(&pool)?;
            let memory_deleted = client.execute("DELETE FROM vector_memory", &[])? as usize;
            let summaries_deleted = client.execute("DELETE FROM phase_summary", &[])? as usize;
            Ok((memory_deleted, summaries_deleted))
        })
    }
}
