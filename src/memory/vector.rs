//! Vector storage and search
//!
//! This module provides vector storage and similarity search functionality for memory retrieval.

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

/// Vector search hit
#[allow(dead_code)]
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
    #[allow(dead_code)]
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
pub struct VectorStore {
    /// SQLite connection (mutex-protected)
    conn: Mutex<Connection>,
    /// Embedding dimensions
    dimensions: usize,
    /// Maximum number of entries to keep
    max_entries: usize,
}

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
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }

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
                embedding_json TEXT NOT NULL,
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

        Ok(Self {
            conn: Mutex::new(conn),
            dimensions,
            max_entries,
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
        let memory_key = build_memory_key(phase, query);
        let now = now_ts();

        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("vector mutex poisoned in 'upsert'"))?;

        conn.execute(
            "
            INSERT INTO vector_memory(
                memory_key,
                phase,
                query_text,
                response_text,
                embedding_json,
                created_at,
                updated_at,
                hit_count,
                last_hit_at
            )
            VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?6, 0, NULL)
            ON CONFLICT(memory_key) DO UPDATE SET
                response_text = excluded.response_text,
                embedding_json = excluded.embedding_json,
                updated_at = excluded.updated_at
            ",
            params![memory_key, phase, query, response, embedding_json, now,],
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
        // (memory_key, blended_score, response_text)
        let mut scored: Vec<(String, f32, String)> = Vec::new();

        while let Some(row) = rows.next()? {
            let memory_key: String = row.get(0)?;
            let response_text: String = row.get(1)?;
            let embedding_json: String = row.get(2)?;
            let updated_at: i64 = row.get(3)?;

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

            // Time-decay weight: entries age at roughly 1/(1 + age_days * 0.05).
            // At 0 days: weight = 1.0.  At 20 days: weight ≈ 0.5.  At 200 days: weight ≈ 0.09.
            // Blended score = 70% similarity + 30% recency, keeping the raw similarity
            // above min_similarity gate (already checked above) while demoting stale entries.
            const DECAY_FACTOR: f64 = 0.05;
            let age_secs = (now - updated_at).max(0) as f64;
            let age_days = age_secs / 86_400.0;
            let recency_weight = (1.0 / (1.0 + age_days * DECAY_FACTOR)) as f32;
            let blended = similarity * 0.70 + recency_weight * 0.30;

            scored.push((memory_key, blended, response_text));
        }

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

#[cfg(test)]
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
            conn.execute(
                "INSERT OR REPLACE INTO vector_memory(memory_key, phase, query_text, response_text, embedding_json, created_at, updated_at, hit_count)
                 VALUES('__stale_key__', 'coding', 'rust async performance stale', 'stale answer', '[]', ?1, ?1, 0)",
                params![stale_ts],
            )
            .expect("stale insert should work");

            // Give the stale entry a valid embedding so it computes a real similarity.
            let embedding = super::embed_text("rust async performance stale", 64);
            let embedding_json =
                serde_json::to_string(&embedding).expect("should serialize embedding");
            conn.execute(
                "UPDATE vector_memory SET embedding_json = ?1 WHERE memory_key = '__stale_key__'",
                params![embedding_json],
            )
            .expect("stale embedding update should work");
        }

        // The fresh entry should rank higher than the stale one despite similar embeddings.
        let (hits, _) = store
            .search("coding", "rust async performance", 5, 0.0, 200)
            .expect("search should work");

        // Verify fresh entry ranked first (highest blended score).
        let first_snippet = hits.first().map(|h| h.response_snippet.as_str()).unwrap_or("");
        assert!(
            first_snippet.contains("fresh"),
            "fresh entry should rank first but got: {first_snippet:?}"
        );
    }
}
