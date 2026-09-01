//! SQLite vector-store backend (`backend-sqlite` profile).

use crate::acp::prelude::now_ts;
use crate::memory::embedding_provider::ConfigurableEmbeddingProvider;
use crate::shared::math::cosine_similarity_f32;
use anyhow::Result;
use rusqlite::{ffi::sqlite3_auto_extension, params, Connection, OptionalExtension};
use sqlite_vec::sqlite3_vec_init;
use std::path::Path;
use std::sync::Arc;
use std::sync::{Mutex, Mutex as StdMutex, Once};
use tokio::task::spawn_blocking;

use super::hnsw::{spawn_blocking_vec, HnswIndex, HnswNodeMeta};
use super::shared::{
    blend_similarity_with_recency, build_memory_key, embed_with_check, scored_to_hits, VectorHit,
    VectorPrecisionFeedback,
};
use super::{PARAM_PREFIX, PHASE_SUMMARY_COLUMNS};

/// Vector store for similarity search
#[cfg(not(feature = "backend-postgres"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SqliteVectorMode {
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
    pub(crate) mode: SqliteVectorMode,
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
    pub(crate) hnsw: Arc<StdMutex<Option<HnswIndex>>>,
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
    /// Fast path: when the index already exists, this returns `Ok(false)`
    /// immediately without touching the database. The fast path takes a brief
    /// standalone `hnsw` lock that is released before any `conn` access, so it
    /// cannot deadlock against the `conn` -> `hnsw` build order below.
    /// Otherwise it reads all vectors from the database and constructs the
    /// HNSW graph. Called lazily on first search when no HNSW index exists
    /// yet. Returns true if the index was built, false if it already existed.
    ///
    /// Lock order convention: `conn` -> `hnsw` (never the reverse), matching
    /// [`VectorStore::upsert`] and [`VectorStore::clear_all`]. The `conn`
    /// guard is held through the build and publication so that a concurrent
    /// `upsert` cannot commit a row between the snapshot and the index
    /// publication (which would silently drop that entry from the search fast
    /// path), and a concurrent `clear_all` cannot wipe the rows the snapshot
    /// was taken from and leave a stale index published afterwards.
    pub(crate) fn ensure_hnsw_index(&self) -> Result<bool> {
        // Fast path: skip the full table scan when the index already exists.
        // The `hnsw` guard is scoped here and dropped before `conn` is
        // acquired below, so this standalone lock never participates in the
        // `conn` -> `hnsw` ordering (it cannot form an `hnsw` -> `conn` wait).
        {
            let hnsw_guard = self.hnsw.lock().unwrap_or_else(|poisoned| {
                tracing::warn!(
                    "vector hnsw mutex poisoned in 'ensure_hnsw_index' fast path, recovering"
                );
                poisoned.into_inner()
            });
            if hnsw_guard.is_some() {
                return Ok(false);
            }
        }

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
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|c| f32::from_le_bytes(*c))
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
pub(crate) fn embedding_blob(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(values));
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
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
