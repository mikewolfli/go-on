//! Vector storage and search
//!
//! Conditionally compiled:
//! - `backend-sqlite` (local, simple-server): rusqlite-backed, sync API
//! - `backend-postgres` (multi-users-server): postgres + pgvector-backed sync API

// Ensure features are mutually exclusive
#[cfg(all(feature = "backend-sqlite", feature = "backend-postgres"))]
compile_error!("features 'backend-sqlite' and 'backend-postgres' cannot be enabled simultaneously");

use crate::acp::prelude::now_ts;
#[cfg(not(feature = "backend-postgres"))]
use std::path::Path;
use std::sync::Mutex;
#[cfg(not(feature = "backend-postgres"))]
use std::sync::Mutex as StdMutex;
#[cfg(not(feature = "backend-postgres"))]
use std::sync::Once;

#[cfg(not(feature = "backend-postgres"))]
use crate::memory::embedding_provider::{
    local_hash_embed, ConfigurableEmbeddingProvider, EmbeddingProvider,
};
use anyhow::Result;
#[cfg(not(feature = "backend-postgres"))]
use fastrand;
#[cfg(feature = "backend-postgres")]
use pgvector::Vector;
#[cfg(not(feature = "backend-postgres"))]
use rusqlite::{ffi::sqlite3_auto_extension, params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
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
        1.0 - cosine_similarity(query, v)
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
                dist: 1.0 - cosine_similarity(node_vec, &self.vectors[n]),
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

    /// Remove a node from the index by its memory_key.
    ///
    /// Replaces the node's vector with a zero-vector and clears metadata
    /// so it is effectively filtered out during distance computations.
    fn remove(&mut self, memory_key: &str) {
        if let Some(pos) = self
            .metadata
            .iter()
            .position(|m| m.memory_key == memory_key)
        {
            // Zero out the vector (distance will be ~1.0, effectively invisible)
            self.vectors[pos].fill(0.0);
            // Clear metadata so the node won't be matched again
            self.metadata[pos] = HnswNodeMeta {
                memory_key: String::new(),
                response_text: String::new(),
                updated_at: 0,
            };
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

/// Vector store for similarity search
#[cfg(not(feature = "backend-postgres"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SqliteVectorMode {
    SqliteVec,
    JsonFallback,
}

/// Vector store for similarity search
#[cfg(not(feature = "backend-postgres"))]
#[derive(Debug)]
pub struct VectorStore {
    /// SQLite connection (mutex-protected)
    conn: Mutex<Connection>,
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
    hnsw: StdMutex<Option<HnswIndex>>,
}

#[cfg(not(feature = "backend-postgres"))]
impl VectorStore {
    /// Create a new vector store with the built-in minhash fallback for embeddings.
    ///
    /// ⚠️  The minhash fallback is only suitable for development/testing.
    ///     Production deployments should call [`new_with_env()`] or
    ///     [`with_embedding_provider()`] to use real embeddings.
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
            conn: Mutex::new(conn),
            dimensions,
            max_entries,
            mode,
            embedding_provider: None,
            hnsw: StdMutex::new(None),
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
    /// via [`embedding_provider_from_env()`] and passes the result to
    /// [`with_embedding_provider()`].
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
    pub fn upsert(&self, phase: &str, query_text: &str, response_text: &str) -> Result<()> {
        let query = query_text.trim();
        let response = response_text.trim();
        if query.is_empty() || response.is_empty() {
            return Ok(());
        }
        let embedding = if let Some(ref provider) = self.embedding_provider {
            let vec = provider.embed(query);
            if vec.len() != self.dimensions {
                anyhow::bail!(
                    "Embedding dimension mismatch in upsert: got {} but store expects {} dimensions",
                    vec.len(),
                    self.dimensions,
                );
            }
            vec
        } else {
            embed_text(query, self.dimensions)
        };
        let embedding_json = serde_json::to_string(&embedding)?;
        let embedding_blob = embedding_blob(&embedding);
        let memory_key = build_memory_key(phase, query);
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

        const SENTINEL_LIMIT: i64 = i64::MAX; // portable replacement for SQLite's LIMIT -1

        // Collect evicted memory keys before deleting from SQLite.
        // NOTE: `conn` is already locked above — reuse it to avoid deadlock
        // on std::sync::Mutex (non-reentrant).
        let evicted_keys: Vec<String> = {
            let mut stmt = conn.prepare(
                "SELECT memory_key FROM vector_memory ORDER BY updated_at DESC LIMIT ?2 OFFSET ?1",
            )?;
            let rows = stmt.query_map(params![self.max_entries as i64, SENTINEL_LIMIT], |row| {
                row.get::<_, String>(0)
            })?;
            rows.filter_map(|r| r.ok()).collect()
        };

        conn.execute(
            "
            DELETE FROM vector_memory
            WHERE memory_key IN (
                SELECT memory_key
                FROM vector_memory
                ORDER BY updated_at DESC
                LIMIT ?2 OFFSET ?1
            )
            ",
            params![self.max_entries as i64, SENTINEL_LIMIT],
        )?;

        // Also remove evicted entries from HNSW index if it exists
        if let Ok(mut hnsw_guard) = self.hnsw.lock() {
            if let Some(ref mut hnsw) = *hnsw_guard {
                for key in &evicted_keys {
                    hnsw.remove(key);
                }
            }
        }

        // Update HNSW index if it exists
        if let Ok(mut hnsw_guard) = self.hnsw.lock() {
            if let Some(ref mut hnsw) = *hnsw_guard {
                hnsw.insert(
                    embedding,
                    HnswNodeMeta {
                        memory_key,
                        response_text: response.to_string(),
                        updated_at: now,
                    },
                );
            }
        }

        Ok(())
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
    pub fn search(
        &self,
        phase: &str,
        query_text: &str,
        top_k: usize,
        min_similarity: f32,
        max_snippet_chars: usize,
    ) -> Result<(Vec<VectorHit>, VectorPrecisionFeedback)> {
        if top_k == 0 {
            return Ok((Vec::new(), VectorPrecisionFeedback::new(&[])));
        }

        let query = query_text.trim();
        if query.is_empty() {
            return Ok((Vec::new(), VectorPrecisionFeedback::new(&[])));
        }

        let query_embedding = if let Some(ref provider) = self.embedding_provider {
            let vec = provider.embed(query);
            if vec.len() != self.dimensions {
                anyhow::bail!(
                    "Embedding dimension mismatch in search: got {} but store expects {} dimensions",
                    vec.len(),
                    self.dimensions,
                );
            }
            vec
        } else {
            embed_text(query, self.dimensions)
        };
        let now = now_ts();
        let limit = self.max_entries.max(top_k);

        // Try HNSW fast path first — if index exists, use it for O(log N) search
        {
            let hnsw_exists = self.hnsw.lock().map(|g| g.is_some()).unwrap_or(false);
            if hnsw_exists {
                return self.hnsw_search(
                    &query_embedding,
                    phase,
                    top_k,
                    min_similarity,
                    max_snippet_chars,
                    now,
                );
            }
        }

        // Collect results within a locked scope, then release the lock
        // before doing sorting/processing (minimizes lock duration).
        let mut scored = {
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
                        let blended = blend_similarity_with_recency(similarity, now, updated_at);
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
                        let memory_embedding: Vec<f32> = match serde_json::from_str(&embedding_json)
                        {
                            Ok(v) => v,
                            Err(_) => continue,
                        };
                        if memory_embedding.len() != query_embedding.len() {
                            continue;
                        }

                        let similarity = cosine_similarity(&query_embedding, &memory_embedding);
                        if similarity < min_similarity {
                            continue;
                        }

                        let blended = blend_similarity_with_recency(similarity, now, updated_at);
                        scored.push((memory_key, blended, response_text));
                    }

                    scored
                }
            };

            // Update hit counts while still holding the lock
            if !scored.is_empty() {
                for (memory_key, _, _) in &scored {
                    conn.execute(
                        "
                        UPDATE vector_memory
                        SET hit_count = hit_count + 1,
                            last_hit_at = ?2
                        WHERE memory_key = ?1
                        ",
                        params![memory_key, now],
                    )?;
                }
            }

            scored
        }; // conn is dropped here, releasing the lock

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);

        let hits: Vec<VectorHit> = scored
            .into_iter()
            .map(|(_, blended_score, response_text)| VectorHit {
                similarity: blended_score,
                response_snippet: trim_chars(&response_text, max_snippet_chars),
            })
            .collect();

        let feedback = VectorPrecisionFeedback::new(&hits);
        Ok((hits, feedback))
    }

    /// Get phase summary
    ///
    /// # Arguments
    /// * `phase` - Phase name
    ///
    /// # Returns
    /// * `Result<Option<String>>` - Returns Ok(Some(String)) if a summary exists, Ok(None) if not, or an error if something goes wrong
    pub fn get_phase_summary(&self, phase: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("vector mutex poisoned in 'get_phase_summary', recovering");
            poisoned.into_inner()
        });

        let summary = conn
            .query_row(
                "SELECT summary_text FROM phase_summary WHERE phase = ?1",
                params![phase],
                |row| row.get::<_, String>(0),
            )
            .optional()?;

        Ok(summary)
    }

    /// Upsert phase summary
    ///
    /// # Arguments
    /// * `phase` - Phase name
    /// * `summary_text` - Summary text
    ///
    /// # Returns
    /// * `Result<()>` - Returns Ok(()) if the summary is upserted successfully, or an error if something goes wrong
    pub fn upsert_phase_summary(&self, phase: &str, summary_text: &str) -> Result<()> {
        let text = summary_text.trim();
        if text.is_empty() {
            return Ok(());
        }

        let now = now_ts();
        let conn = self.conn.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("vector mutex poisoned in 'upsert_phase_summary', recovering");
            poisoned.into_inner()
        });

        conn.execute(
            "
            INSERT INTO phase_summary(phase, summary_text, updated_at)
            VALUES(?1, ?2, ?3)
            ON CONFLICT(phase) DO UPDATE SET
                summary_text = excluded.summary_text,
                updated_at = excluded.updated_at
            ",
            params![phase, text, now],
        )?;

        Ok(())
    }

    /// Get memory entry count
    ///
    /// # Returns
    /// * `Result<u64>` - Returns Ok(u64) with the number of memory entries, or an error if something goes wrong
    pub fn memory_entry_count(&self) -> Result<u64> {
        let conn = self.conn.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("vector mutex poisoned in 'memory_entry_count', recovering");
            poisoned.into_inner()
        });
        let count = conn.query_row("SELECT COUNT(*) FROM vector_memory", [], |row| {
            row.get::<_, i64>(0)
        })?;
        Ok(count.max(0) as u64)
    }

    /// Get summary entry count
    ///
    /// # Returns
    /// * `Result<u64>` - Returns Ok(u64) with the number of summary entries, or an error if something goes wrong
    pub fn summary_entry_count(&self) -> Result<u64> {
        let conn = self.conn.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("vector mutex poisoned in 'summary_entry_count', recovering");
            poisoned.into_inner()
        });
        let count = conn.query_row("SELECT COUNT(*) FROM phase_summary", [], |row| {
            row.get::<_, i64>(0)
        })?;
        Ok(count.max(0) as u64)
    }

    /// Clear all entries
    ///
    /// # Returns
    /// * `Result<(usize, usize)>` - Returns Ok((usize, usize)) with the number of memory entries and summary entries deleted, or an error if something goes wrong
    pub fn clear_all(&self) -> Result<(usize, usize)> {
        let conn = self.conn.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("vector mutex poisoned in 'clear', recovering");
            poisoned.into_inner()
        });
        let memory_deleted = conn.execute("DELETE FROM vector_memory", [])?;
        let summaries_deleted = conn.execute("DELETE FROM phase_summary", [])?;
        Ok((memory_deleted, summaries_deleted))
    }

    /// Reclaim SQLite free pages after retention cleanup.
    pub fn vacuum(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("vector mutex poisoned in 'vacuum', recovering");
            poisoned.into_inner()
        });
        conn.execute_batch("VACUUM;")?;
        Ok(())
    }

    /// Ensure the in-memory HNSW index is built from SQLite data.
    ///
    /// Reads all vectors from the database and constructs the HNSW graph.
    /// Called lazily on first search when no HNSW index exists yet.
    /// Returns true if the index was built, false if it already existed.
    #[allow(dead_code)] // F-GAP-49 — reserved for HNSW index ensure
    fn ensure_hnsw_index(&self) -> Result<bool> {
        let mut hnsw_guard = self.hnsw.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("vector hnsw mutex poisoned in 'ensure_hnsw_index', recovering");
            poisoned.into_inner()
        });
        if hnsw_guard.is_some() {
            return Ok(false);
        }

        // Read all entries from SQLite (collect into Vec within a nested scope so
        // the Statement / Rows borrow ends before we close the connection).
        let entries: Vec<(Vec<f32>, HnswNodeMeta)> = {
            let conn = self.conn.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("vector mutex poisoned in 'ensure_hnsw_index', recovering");
                poisoned.into_inner()
            });

            let mut stmt = conn.prepare(
                "SELECT memory_key, response_text, updated_at, embedding_blob, embedding_json
                 FROM vector_memory
                 ORDER BY updated_at ASC",
            )?;

            let mut entries: Vec<(Vec<f32>, HnswNodeMeta)> = Vec::new();
            let mut rows = stmt.query([])?;

            while let Some(row) = rows.next()? {
                let memory_key: String = row.get(0)?;
                let response_text: String = row.get(1)?;
                let updated_at: i64 = row.get(2)?;
                let embedding_blob: Option<Vec<u8>> = row.get(3)?;
                let embedding_json: Option<String> = row.get(4)?;

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
                        response_text,
                        updated_at,
                    },
                ));
            }
            entries
        };

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
            // Fallback: no HNSW index
            return self.search(phase, "", top_k, min_similarity, max_snippet_chars);
        };

        // Prefetch more candidates than top_k because recency blending may re-rank
        let ef = (top_k * 4).max(hnsw.ef_search);
        let results = hnsw.search(query_embedding, ef);

        let metadata = hnsw.metadata.clone();
        drop(hnsw_guard);

        let mut scored: Vec<(String, f32, String)> = Vec::with_capacity(results.len());
        for nd in &results {
            if nd.dist > 1.0 - min_similarity {
                continue;
            }
            let similarity = (1.0_f32 - nd.dist).clamp(0.0, 1.0);
            let meta = &metadata[nd.idx];
            let blended = blend_similarity_with_recency(similarity, now, meta.updated_at);
            scored.push((meta.memory_key.clone(), blended, meta.response_text.clone()));
        }

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);

        // Update hit counts in SQLite
        let conn = self.conn.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("vector mutex poisoned in 'hnsw_search::hit_count', recovering");
            poisoned.into_inner()
        });
        if !scored.is_empty() {
            for (memory_key, _, _) in &scored {
                let _ = conn.execute(
                    "UPDATE vector_memory SET hit_count = hit_count + 1, last_hit_at = ?2 WHERE memory_key = ?1",
                    params![memory_key, now],
                );
            }
        }
        drop(conn);

        let hits: Vec<VectorHit> = scored
            .into_iter()
            .map(|(_, blended_score, response_text)| VectorHit {
                similarity: blended_score,
                response_snippet: trim_chars(&response_text, max_snippet_chars),
            })
            .collect();

        let feedback = VectorPrecisionFeedback::new(&hits);
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
                // Keep variant reachable across profile combinations so dead_code
                // does not fire when fallback is compile-time disabled.
                let _fallback_marker = SqliteVectorMode::JsonFallback;
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

#[cfg(not(feature = "backend-postgres"))]
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum::<f32>()
}

fn trim_chars(input: &str, max_chars: usize) -> String {
    input.chars().take(max_chars).collect()
}

fn build_memory_key(phase: &str, query_text: &str) -> String {
    let payload = format!("{}|{}", phase, query_text.trim());
    let mut hasher = Sha256::new();
    hasher.update(payload.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(all(test, not(feature = "backend-postgres")))]
mod tests {
    use super::{VectorPrecisionFeedback, VectorStore};

    #[test]
    fn vector_store_upsert_and_search() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let db_path = dir.path().join("vector.sqlite3");

        let store = VectorStore::new(&db_path, 64, 200).expect("vector store init should work");
        store
            .upsert(
                "coding",
                "optimize rust async cache",
                "Use sqlite cache and tune ttl for repeated requests.",
            )
            .expect("upsert should work");

        let (hits, feedback) = store
            .search("coding", "how to optimize async cache", 2, 0.1, 200)
            .expect("search should work");
        assert!(!hits.is_empty());
        assert!(feedback.hit_count > 0);
        assert!(feedback.avg_similarity > 0.0);
    }

    #[test]
    fn vector_store_phase_summary_roundtrip() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let db_path = dir.path().join("vector.sqlite3");

        let store = VectorStore::new(&db_path, 64, 200).expect("vector store init should work");
        store
            .upsert_phase_summary("coding", "short summary")
            .expect("upsert summary should work");

        let summary = store
            .get_phase_summary("coding")
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

    #[test]
    fn vector_search_time_decay_demotes_stale_entry() {
        #[cfg(not(feature = "backend-postgres"))]
        use rusqlite::{params, Connection};

        let dir = tempfile::tempdir().expect("temp dir should be created");
        let db_path = dir.path().join("vector_decay.sqlite3");

        let store = VectorStore::new(&db_path, 64, 200).expect("vector store init should work");

        // Insert a fresh entry with an identical query to get identical embeddings.
        store
            .upsert("coding", "rust async performance", "fresh answer")
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
        let (hits, _) = store
            .search("coding", "rust async performance", 5, 0.0, 200)
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

    #[test]
    fn hnsw_index_insert_and_search_basic() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let db_path = dir.path().join("hnsw_basic.sqlite3");
        let store = VectorStore::new(&db_path, 64, 200).expect("vector store init");

        // Insert enough entries to trigger HNSW construction
        for i in 0..50 {
            let query = format!("rust feature number {i}");
            let response = format!("response for feature {i}");
            store.upsert("test", &query, &response).expect("upsert");
        }

        // Trigger HNSW build by calling ensure_hnsw_index
        store.ensure_hnsw_index().expect("ensure_hnsw_index");

        // Search via HNSW path
        let (hits, feedback) = store
            .search("test", "rust feature number 5", 5, 0.0, 200)
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

    #[test]
    #[ignore = "10K-vector benchmark — run explicitly with -- --ignored"]
    fn hnsw_benchmark_10k_vectors() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let db_path = dir.path().join("hnsw_10k.sqlite3");
        let store = VectorStore::new(&db_path, 128, 10000).expect("vector store init");

        // Insert 10,000 vectors using the public upsert API.
        // HNSW index is None during this phase, so each upsert only goes to SQLite.
        for i in 0..10_000 {
            let query = format!("benchmark query number {i}");
            let response = format!("benchmark response {i}");
            store.upsert("bench", &query, &response).expect("upsert");
        }

        // Build the HNSW index from the SQLite data
        let built = store.ensure_hnsw_index().expect("ensure_hnsw_index");
        assert!(built, "HNSW index should be built");

        // Warm-up: run one search to ensure any lazy initialization is done
        let _ = store
            .search("bench", "benchmark query number 42", 10, 0.0, 200)
            .expect("warmup search");

        // Measure HNSW search latency for 10 queries
        let start = std::time::Instant::now();
        for _ in 0..10 {
            let (hits, _) = store
                .search("bench", "benchmark query number 42", 10, 0.0, 200)
                .expect("search");
            assert!(!hits.is_empty(), "should find similar entries");
        }
        let elapsed = start.elapsed();
        let avg_us = elapsed.as_micros() as f64 / 10.0;

        // HNSW search should be fast: each search under 10 ms on 10K vectors
        assert!(
            avg_us < 10_000.0,
            "HNSW search too slow: {avg_us:.0}us avg (expected < 10ms)"
        );
    }

    #[test]
    fn hnsw_insert_empty_and_build() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let db_path = dir.path().join("hnsw_empty.sqlite3");
        let store = VectorStore::new(&db_path, 64, 100).expect("vector store init");

        // Build HNSW with empty DB
        let built = store.ensure_hnsw_index().expect("ensure_hnsw_index empty");
        assert!(built, "should build empty index");

        // Search on empty index should return no results
        let (hits, feedback) = store
            .search("test", "something", 5, 0.0, 200)
            .expect("search on empty");
        assert!(hits.is_empty());
        assert_eq!(feedback.hit_count, 0);
    }
}

// ─── PostgreSQL backend (multi-users-server) ─────────────────────────
//
// Embeddings are computed in Rust (sha2-based projection, same as SQLite backend)
// and stored in a native `pgvector` column through the synchronous `postgres`
// client. Methods expose the same sync signature as the SQLite backend.
#[cfg(feature = "backend-postgres")]
use postgres::{Client, NoTls};

#[cfg(feature = "backend-postgres")]
use crate::memory::embedding_provider::{
    local_hash_embed, ConfigurableEmbeddingProvider, EmbeddingProvider,
};

#[cfg(feature = "backend-postgres")]
pub struct VectorStore {
    client: Mutex<Client>,
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
            .field("client", &"<postgres Client>")
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
    pub fn new(url: &str, dimensions: usize, max_entries: usize) -> Result<Self> {
        if dimensions == 0 {
            anyhow::bail!("vector.dimensions must be greater than 0 for pgvector backend");
        }

        let mut client = Client::connect(url, NoTls)?;

        let schema_sql = format!(
            "CREATE TABLE IF NOT EXISTS vector_memory (
                memory_key      TEXT PRIMARY KEY,
                phase           TEXT NOT NULL,
                query_text      TEXT NOT NULL,
                response_text   TEXT NOT NULL,
                embedding       vector({dimensions}) NOT NULL,
                created_at      BIGINT NOT NULL,
                updated_at      BIGINT NOT NULL,
                hit_count       BIGINT NOT NULL DEFAULT 0,
                last_hit_at     BIGINT,
                user_id         TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_vector_memory_phase_updated_at
                ON vector_memory(phase, updated_at DESC);
            CREATE INDEX IF NOT EXISTS idx_vector_memory_embedding_cosine
                ON vector_memory USING hnsw (embedding vector_cosine_ops);
            CREATE INDEX IF NOT EXISTS idx_vector_memory_user_id
                ON vector_memory(user_id);
            CREATE TABLE IF NOT EXISTS phase_summary (
                phase           TEXT PRIMARY KEY,
                summary_text    TEXT NOT NULL,
                updated_at      BIGINT NOT NULL
            );"
        );

        client.batch_execute("CREATE EXTENSION IF NOT EXISTS vector")?;
        client.batch_execute(&schema_sql)?;

        Ok(Self {
            client: Mutex::new(client),
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

    pub fn upsert(&self, phase: &str, query_text: &str, response_text: &str) -> Result<()> {
        let query = query_text.trim();
        let response = response_text.trim();
        if query.is_empty() || response.is_empty() {
            return Ok(());
        }
        let embedding_vec = if let Some(ref provider) = self.embedding_provider {
            let vec = provider.embed(query);
            if vec.len() != self.dimensions {
                anyhow::bail!(
                    "Embedding dimension mismatch in upsert: got {} but store expects {} dimensions",
                    vec.len(),
                    self.dimensions,
                );
            }
            vec
        } else {
            embed_text(query, self.dimensions)
        };
        let embedding = Vector::from(embedding_vec);
        let memory_key = build_memory_key(phase, query);
        let now = now_ts();
        let max_entries = self.max_entries as i64;
        let mut client = self.client.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("vector mutex poisoned in 'upsert', recovering");
            poisoned.into_inner()
        });
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

        client.execute(
            "DELETE FROM vector_memory
             WHERE memory_key NOT IN (
                 SELECT memory_key FROM vector_memory
                 ORDER BY updated_at DESC LIMIT $1
             )",
            &[&max_entries],
        )?;

        Ok(())
    }

    pub fn search(
        &self,
        phase: &str,
        query_text: &str,
        top_k: usize,
        min_similarity: f32,
        max_snippet_chars: usize,
    ) -> Result<(Vec<VectorHit>, VectorPrecisionFeedback)> {
        if top_k == 0 {
            return Ok((Vec::new(), VectorPrecisionFeedback::new(&[])));
        }
        let query = query_text.trim();
        if query.is_empty() {
            return Ok((Vec::new(), VectorPrecisionFeedback::new(&[])));
        }

        let query_vec = if let Some(ref provider) = self.embedding_provider {
            let vec = provider.embed(query);
            if vec.len() != self.dimensions {
                anyhow::bail!(
                    "Embedding dimension mismatch in search: got {} but store expects {} dimensions",
                    vec.len(),
                    self.dimensions,
                );
            }
            vec
        } else {
            embed_text(query, self.dimensions)
        };
        let query_embedding = Vector::from(query_vec);
        let now = now_ts();
        let mut client = self.client.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("vector mutex poisoned in 'search', recovering");
            poisoned.into_inner()
        });
        let rows = client.query(
            "SELECT memory_key, response_text, updated_at,
                    1 - (embedding <=> $2) AS similarity
             FROM vector_memory
             WHERE phase = $1
             ORDER BY embedding <=> $2
             LIMIT 300",
            &[&phase, &query_embedding],
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

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);

        if !scored.is_empty() {
            for (key, _, _) in &scored {
                let _ = client.execute(
                    "UPDATE vector_memory
                     SET hit_count = hit_count + 1, last_hit_at = $2
                     WHERE memory_key = $1",
                    &[key, &now],
                );
            }
        }

        let hits: Vec<VectorHit> = scored
            .into_iter()
            .map(|(_, score, text)| VectorHit {
                similarity: score,
                response_snippet: trim_chars(&text, max_snippet_chars),
            })
            .collect();

        let feedback = VectorPrecisionFeedback::new(&hits);
        Ok((hits, feedback))
    }

    pub fn get_phase_summary(&self, phase: &str) -> Result<Option<String>> {
        let mut client = self.client.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("vector mutex poisoned in 'get_phase_summary', recovering");
            poisoned.into_inner()
        });
        Ok(client
            .query_opt(
                "SELECT summary_text FROM phase_summary WHERE phase = $1",
                &[&phase],
            )?
            .map(|row| row.get(0)))
    }

    pub fn upsert_phase_summary(&self, phase: &str, summary_text: &str) -> Result<()> {
        let text = summary_text.trim();
        if text.is_empty() {
            return Ok(());
        }
        let mut client = self.client.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("vector mutex poisoned in 'upsert_phase_summary', recovering");
            poisoned.into_inner()
        });
        let now = now_ts();
        client.execute(
            "INSERT INTO phase_summary (phase, summary_text, updated_at)
             VALUES ($1, $2, $3)
             ON CONFLICT (phase) DO UPDATE SET
                 summary_text = EXCLUDED.summary_text,
                 updated_at   = EXCLUDED.updated_at",
            &[&phase, &text, &now],
        )?;
        Ok(())
    }

    pub fn memory_entry_count(&self) -> Result<u64> {
        let mut client = self.client.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("vector mutex poisoned in 'memory_entry_count', recovering");
            poisoned.into_inner()
        });
        let row = client.query_one("SELECT COUNT(*) FROM vector_memory", &[])?;
        let count: i64 = row.get(0);
        Ok(count.max(0) as u64)
    }

    pub fn summary_entry_count(&self) -> Result<u64> {
        let mut client = self.client.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("vector mutex poisoned in 'summary_entry_count', recovering");
            poisoned.into_inner()
        });
        let row = client.query_one("SELECT COUNT(*) FROM phase_summary", &[])?;
        let count: i64 = row.get(0);
        Ok(count.max(0) as u64)
    }

    pub fn clear_all(&self) -> Result<(usize, usize)> {
        let mut client = self.client.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("vector mutex poisoned in 'clear_all', recovering");
            poisoned.into_inner()
        });
        let memory_deleted = client.execute("DELETE FROM vector_memory", &[])? as usize;
        let summaries_deleted = client.execute("DELETE FROM phase_summary", &[])? as usize;
        Ok((memory_deleted, summaries_deleted))
    }

    /// Reclaim PostgreSQL storage after retention cleanup.
    ///
    /// Issues `VACUUM ANALYZE` to reclaim free pages and update query planner
    /// statistics, matching the SQLite backend's `vacuum()` behaviour. While
    /// PostgreSQL autovacuum handles routine maintenance automatically, issuing
    /// an explicit `VACUUM ANALYZE` after large deletions ensures immediate
    /// space reclamation and consistent test/benchmark behaviour.
    pub fn vacuum(&self) -> Result<()> {
        let mut client = self.client.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("vector mutex poisoned in 'vacuum', recovering");
            poisoned.into_inner()
        });
        client.batch_execute("VACUUM ANALYZE")?;
        Ok(())
    }
}
