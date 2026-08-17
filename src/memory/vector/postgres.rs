//! PostgreSQL vector-store backend (`multi-users-server` profile).
//!
//! Embeddings are computed in Rust (sha2-based projection, same as SQLite backend)
//! and stored in a native `pgvector` column through the synchronous `postgres`
//! client. Methods expose the same sync signature as the SQLite backend.

use crate::acp::prelude::now_ts;
use crate::memory::embedding_provider::ConfigurableEmbeddingProvider;
use crate::memory::pg_migrate::run_migrations;
use crate::memory::pg_pool::{
    connect_postgres, create_pool, create_pool_pair, pool_get, resolve_pg_dsn, PgPoolPair,
};
use anyhow::Result;
use pgvector::Vector;
use std::sync::Arc;
use tokio::task::spawn_blocking;

use super::hnsw::spawn_blocking_vec;
use super::shared::{
    blend_similarity_with_recency, build_memory_key, embed_with_check, scored_to_hits, VectorHit,
    VectorPrecisionFeedback,
};
use super::{PARAM_PREFIX, PHASE_SUMMARY_COLUMNS};

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
