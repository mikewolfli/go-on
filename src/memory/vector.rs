//! Vector storage and search
//!
//! Conditionally compiled:
//! - `backend-sqlite` (profile-local, profile-simple-server): rusqlite-backed, sync API
//! - `backend-postgres` (profile-multi-users-server): postgres + pgvector-backed sync API

// Ensure features are mutually exclusive
#[cfg(all(feature = "backend-sqlite", feature = "backend-postgres"))]
compile_error!("features 'backend-sqlite' and 'backend-postgres' cannot be enabled simultaneously");

#[cfg(not(feature = "backend-postgres"))]
use std::path::Path;
use std::sync::Mutex;
#[cfg(not(feature = "backend-postgres"))]
use std::sync::Once;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
#[cfg(feature = "backend-postgres")]
use pgvector::Vector;
#[cfg(not(feature = "backend-postgres"))]
use rusqlite::{ffi::sqlite3_auto_extension, params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
#[cfg(not(feature = "backend-postgres"))]
use sqlite_vec::sqlite3_vec_init;
#[cfg(all(not(feature = "backend-postgres"), feature = "profile-local"))]
#[cfg(all(
    not(feature = "backend-postgres"),
    feature = "profile-local",
    not(feature = "profile-simple-server"),
    not(feature = "profile-multi-users-server")
))]
use tracing::warn;

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

/// Vector store for similarity search
#[cfg(not(feature = "backend-postgres"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SqliteVectorMode {
    SqliteVec,
    JsonFallback,
}

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
}

#[cfg(not(feature = "backend-postgres"))]
impl VectorStore {
    /// Create a new vector store
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
                last_hit_at INTEGER
            );

            CREATE INDEX IF NOT EXISTS idx_vector_memory_phase_updated_at
                ON vector_memory(phase, updated_at DESC);

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
        })
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

        let embedding = embed_text(query, self.dimensions);
        let embedding_json = serde_json::to_string(&embedding)?;
        let embedding_blob = embedding_blob(&embedding);
        let memory_key = build_memory_key(phase, query);
        let now = now_ts();

        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("vector mutex poisoned in 'upsert'"))?;

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

        conn.execute(
            "
            DELETE FROM vector_memory
            WHERE memory_key IN (
                SELECT memory_key
                FROM vector_memory
                ORDER BY updated_at DESC
                LIMIT -1 OFFSET ?1
            )
            ",
            params![self.max_entries as i64],
        )?;

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

        let query_embedding = embed_text(query, self.dimensions);
        let now = now_ts();
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("vector mutex poisoned in 'search'"))?;

        let mut scored = match self.mode {
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
                    LIMIT 300
                    ",
                )?;
                let mut rows = stmt.query(params![phase, query_blob])?;
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
                    LIMIT 300
                    ",
                )?;

                let mut rows = stmt.query(params![phase])?;
                let mut scored: Vec<(String, f32, String)> = Vec::new();

                while let Some(row) = rows.next()? {
                    let memory_key: String = row.get(0)?;
                    let response_text: String = row.get(1)?;
                    let embedding_json: Option<String> = row.get(2)?;
                    let updated_at: i64 = row.get(3)?;

                    let Some(embedding_json) = embedding_json else {
                        continue;
                    };
                    let memory_embedding: Vec<f32> = match serde_json::from_str(&embedding_json) {
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

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);

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
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("vector mutex poisoned in 'get_phase_summary'"))?;

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
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("vector mutex poisoned in 'upsert_phase_summary'"))?;

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
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("vector mutex poisoned in 'memory_entry_count'"))?;
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
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("vector mutex poisoned in 'summary_entry_count'"))?;
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
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("vector mutex poisoned in 'clear'"))?;
        let memory_deleted = conn.execute("DELETE FROM vector_memory", [])?;
        let summaries_deleted = conn.execute("DELETE FROM phase_summary", [])?;
        Ok((memory_deleted, summaries_deleted))
    }

    /// Reclaim SQLite free pages after retention cleanup.
    pub fn vacuum(&self) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("vector mutex poisoned in 'vacuum'"))?;
        conn.execute_batch("VACUUM;")?;
        Ok(())
    }
}

/// Wrapper around the `extern "C"` sqlite3_vec_init symbol that matches
/// the signature expected by sqlite3_auto_extension.
///
/// This avoids undefined behaviour from transmuting a function pointer
/// with one ABI signature to another (Rust ABI vs C ABI).
/// SAFETY: `db`, `pz_err_msg`, and `p_err_msg` are valid pointers
/// provided by the SQLite runtime.
#[cfg(not(feature = "backend-postgres"))]
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
    REGISTER.call_once(|| unsafe {
        sqlite3_auto_extension(Some(sqlite3_vec_init_auto_extension));
    });
}

#[cfg(not(feature = "backend-postgres"))]
fn resolve_sqlite_vector_mode(conn: &Connection) -> Result<SqliteVectorMode> {
    let probe = conn.query_row("SELECT vec_version()", [], |row| row.get::<_, String>(0));
    match probe {
        Ok(_) => Ok(SqliteVectorMode::SqliteVec),
        Err(err) => {
            #[cfg(all(
                feature = "profile-local",
                not(feature = "profile-simple-server"),
                not(feature = "profile-multi-users-server")
            ))]
            {
                warn!(
                    "sqlite-vec unavailable, falling back to JSON embedding table for profile-local: {}",
                    err
                );
                Ok(SqliteVectorMode::JsonFallback)
            }

            #[cfg(not(all(
                feature = "profile-local",
                not(feature = "profile-simple-server"),
                not(feature = "profile-multi-users-server")
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

fn tokenize(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter_map(|token| {
            let t = token.trim().to_ascii_lowercase();
            if t.len() >= 2 {
                Some(t)
            } else {
                None
            }
        })
        .collect()
}

fn embed_text(text: &str, dimensions: usize) -> Vec<f32> {
    let mut vector = vec![0_f32; dimensions];
    if dimensions == 0 {
        return vector;
    }

    for token in tokenize(text) {
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        let digest = hasher.finalize();

        let mut idx_bytes = [0_u8; 8];
        idx_bytes.copy_from_slice(&digest[0..8]);
        let idx = (u64::from_le_bytes(idx_bytes) as usize) % dimensions;
        let sign = if digest[8] % 2 == 0 { 1.0 } else { -1.0 };
        vector[idx] += sign;
    }

    let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in &mut vector {
            *value /= norm;
        }
    }

    vector
}

#[cfg(feature = "backend-sqlite")]
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

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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
            let embedding = super::embed_text("rust async performance stale", 64);
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
}

// ─── PostgreSQL backend (profile-multi-users-server) ─────────────────────────
//
// Embeddings are computed in Rust (sha2-based projection, same as SQLite backend)
// and stored in a native `pgvector` column through the synchronous `postgres`
// client. Methods expose the same sync signature as the SQLite backend.
#[cfg(feature = "backend-postgres")]
use postgres::{Client, NoTls};

#[cfg(feature = "backend-postgres")]
pub struct VectorStore {
    client: Mutex<Client>,
    dimensions: usize,
    max_entries: usize,
}

#[cfg(feature = "backend-postgres")]
impl std::fmt::Debug for VectorStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VectorStore")
            .field("client", &"<postgres Client>")
            .field("dimensions", &self.dimensions)
            .field("max_entries", &self.max_entries)
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
                last_hit_at     BIGINT
            );
            CREATE INDEX IF NOT EXISTS idx_vector_memory_phase_updated_at
                ON vector_memory(phase, updated_at DESC);
            CREATE INDEX IF NOT EXISTS idx_vector_memory_embedding_cosine
                ON vector_memory USING hnsw (embedding vector_cosine_ops);
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
        })
    }

    pub fn upsert(&self, phase: &str, query_text: &str, response_text: &str) -> Result<()> {
        let query = query_text.trim();
        let response = response_text.trim();
        if query.is_empty() || response.is_empty() {
            return Ok(());
        }
        let embedding = Vector::from(embed_text(query, self.dimensions));
        let memory_key = build_memory_key(phase, query);
        let now = now_ts();
        let max_entries = self.max_entries as i64;
        let mut client = self
            .client
            .lock()
            .map_err(|_| anyhow::anyhow!("vector mutex poisoned in 'upsert'"))?;
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

        let query_embedding = Vector::from(embed_text(query, self.dimensions));
        let now = now_ts();
        let mut client = self
            .client
            .lock()
            .map_err(|_| anyhow::anyhow!("vector mutex poisoned in 'search'"))?;
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
        let mut client = self
            .client
            .lock()
            .map_err(|_| anyhow::anyhow!("vector mutex poisoned in 'get_phase_summary'"))?;
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
        let mut client = self
            .client
            .lock()
            .map_err(|_| anyhow::anyhow!("vector mutex poisoned in 'upsert_phase_summary'"))?;
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
        let mut client = self
            .client
            .lock()
            .map_err(|_| anyhow::anyhow!("vector mutex poisoned in 'memory_entry_count'"))?;
        let row = client.query_one("SELECT COUNT(*) FROM vector_memory", &[])?;
        let count: i64 = row.get(0);
        Ok(count.max(0) as u64)
    }

    pub fn summary_entry_count(&self) -> Result<u64> {
        let mut client = self
            .client
            .lock()
            .map_err(|_| anyhow::anyhow!("vector mutex poisoned in 'summary_entry_count'"))?;
        let row = client.query_one("SELECT COUNT(*) FROM phase_summary", &[])?;
        let count: i64 = row.get(0);
        Ok(count.max(0) as u64)
    }

    pub fn clear_all(&self) -> Result<(usize, usize)> {
        let mut client = self
            .client
            .lock()
            .map_err(|_| anyhow::anyhow!("vector mutex poisoned in 'clear_all'"))?;
        let memory_deleted = client.execute("DELETE FROM vector_memory", &[])? as usize;
        let summaries_deleted = client.execute("DELETE FROM phase_summary", &[])? as usize;
        Ok((memory_deleted, summaries_deleted))
    }

    /// No-op on PostgreSQL — VACUUM is managed by autovacuum.
    pub fn vacuum(&self) -> Result<()> {
        Ok(())
    }
}
