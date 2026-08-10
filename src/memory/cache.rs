//! Cache implementation
//!
//! Conditionally compiled:
//! - `backend-sqlite` (local, simple-server): rusqlite-backed, async API via spawn_blocking
//! - `backend-postgres` (multi-users-server): postgres-backed sync API

// Ensure exactly one backend feature is enabled.
#[cfg(all(feature = "backend-sqlite", feature = "backend-postgres"))]
compile_error!("features 'backend-sqlite' and 'backend-postgres' cannot be enabled simultaneously");
#[cfg(not(any(feature = "backend-sqlite", feature = "backend-postgres")))]
compile_error!("one of 'backend-sqlite' or 'backend-postgres' must be enabled");

#[cfg(feature = "backend-sqlite")]
use std::sync::{Arc, Mutex};

use crate::acp::prelude::now_ts;

use anyhow::Result;
use tokio::task::spawn_blocking;

// ─── Shared types (both backends) ────────────────────────────────────────────

/// Cached response structure
pub struct CachedResponse {
    /// The cached response text
    pub response_text: String,
    /// The name of the agent that generated the response
    pub agent_name: Option<String>,
}

/// Aggregated cache statistics snapshot.
#[derive(Debug, Clone, Copy)]
pub struct ResponseCacheStats {
    /// Number of active cache entries.
    pub entry_count: u64,
    /// Configured maximum number of entries.
    pub max_entries: usize,
    /// Sum of all entry hit counters.
    pub total_hits: u64,
    /// Average hits per cached entry.
    pub avg_hits_per_entry: f64,
}

// ─── SQLite backend (local / simple-server) ──────────────────
#[cfg(feature = "backend-sqlite")]
use rusqlite::{params, Connection, OptionalExtension};
#[cfg(feature = "backend-sqlite")]
use std::path::Path;

// ─── Shared backend execution primitives ────────────────────────────────────
// The public async methods (get/put/clear_all/entry_count/stats) are
// duplicated across the two backends only in their connection acquisition and
// placeholder style; the actual statements and result mapping live in the
// per-backend `cache_*` helpers below so the business logic has a single home.

/// Bound values for a cache put (prepared outside `spawn_blocking`).
struct PutParams {
    cache_key: String,
    response_text: String,
    agent_name: String,
    now: i64,
    expires_at: i64,
    max_entries: i64,
}

#[cfg(feature = "backend-sqlite")]
fn cache_get(
    conn: &rusqlite::Connection,
    cache_key: &str,
    now: i64,
) -> Result<Option<CachedResponse>> {
    let found = conn
        .query_row(
            "
            SELECT response_text, agent_name
            FROM response_cache
            WHERE cache_key = ?1 AND expires_at > ?2
                ",
            params![cache_key, now],
            |row| {
                Ok(CachedResponse {
                    response_text: row.get::<_, String>(0)?,
                    agent_name: row.get::<_, Option<String>>(1)?,
                })
            },
        )
        .optional()?;

    if found.is_some() {
        conn.execute(
            "
            UPDATE response_cache
            SET hit_count = hit_count + 1,
                last_hit_at = ?2
            WHERE cache_key = ?1
                ",
            params![cache_key, now],
        )?;
    }
    Ok(found)
}

#[cfg(feature = "backend-postgres")]
fn cache_get(
    client: &mut postgres::Client,
    cache_key: &str,
    now: i64,
) -> Result<Option<CachedResponse>> {
    let row = client.query_opt(
        "SELECT response_text, agent_name FROM response_cache
         WHERE cache_key = $1 AND expires_at > $2",
        &[&cache_key, &now],
    )?;
    if let Some(row) = row {
        client.execute(
            "UPDATE response_cache SET hit_count = hit_count + 1, last_hit_at = $2
             WHERE cache_key = $1",
            &[&cache_key, &now],
        )?;
        Ok(Some(CachedResponse {
            response_text: row.get(0),
            agent_name: row.get(1),
        }))
    } else {
        Ok(None)
    }
}

#[cfg(feature = "backend-sqlite")]
fn cache_put(conn: &rusqlite::Connection, p: &PutParams) -> Result<()> {
    conn.execute(
        "
        INSERT INTO response_cache(
            cache_key,
            response_text,
            agent_name,
            created_at,
            updated_at,
            expires_at,
            hit_count,
            last_hit_at
        )
        VALUES(?1, ?2, ?3, ?4, ?4, ?5, 0, NULL)
        ON CONFLICT(cache_key) DO UPDATE SET
            response_text = excluded.response_text,
            agent_name = excluded.agent_name,
            updated_at = excluded.updated_at,
            expires_at = excluded.expires_at
            ",
        params![
            p.cache_key,
            p.response_text,
            p.agent_name,
            p.now,
            p.expires_at
        ],
    )?;

    // Evict only when the table is over budget, and delete only the
    // excess rows — previously every `put` ran a full-table sort +
    // delete even when the cache was well below `max_entries`.
    let row_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM response_cache", [], |row| row.get(0))?;
    if row_count > p.max_entries {
        let excess = row_count - p.max_entries;
        const SENTINEL_LIMIT: i64 = 2_147_483_647; // max INT32 — portable replacement for SQLite's LIMIT -1
        conn.execute(
            "
        DELETE FROM response_cache
        WHERE cache_key IN (
            SELECT cache_key
            FROM response_cache
            ORDER BY updated_at DESC
            LIMIT ?1 OFFSET ?2
        )
            ",
            params![excess.min(SENTINEL_LIMIT), p.max_entries],
        )?;
    }
    Ok(())
}

#[cfg(feature = "backend-postgres")]
fn cache_put(client: &mut postgres::Client, p: &PutParams) -> Result<()> {
    client.execute(
        "INSERT INTO response_cache
        (cache_key, response_text, agent_name, created_at, updated_at, expires_at, hit_count)
     VALUES ($1, $2, $3, $4, $4, $5, 0)
     ON CONFLICT (cache_key) DO UPDATE SET
        response_text = EXCLUDED.response_text,
        agent_name    = EXCLUDED.agent_name,
        updated_at    = EXCLUDED.updated_at,
        expires_at    = EXCLUDED.expires_at",
        &[
            &p.cache_key,
            &p.response_text,
            &p.agent_name,
            &p.now,
            &p.expires_at,
        ],
    )?;

    // Evict only when over budget (previously every put ran the full
    // NOT IN subquery with a full sort even when under the limit).
    let row_count: i64 = client
        .query_one("SELECT COUNT(*) FROM response_cache", &[])
        .map_err(|e| anyhow::anyhow!("count response_cache rows: {e}"))?
        .get(0);
    if row_count > p.max_entries {
        client.execute(
            "DELETE FROM response_cache
         WHERE cache_key NOT IN (
             SELECT cache_key FROM response_cache
             ORDER BY updated_at DESC LIMIT $1
         )",
            &[&p.max_entries],
        )?;
    }
    Ok(())
}

#[cfg(feature = "backend-sqlite")]
fn cache_clear_all(conn: &rusqlite::Connection) -> Result<usize> {
    Ok(conn.execute("DELETE FROM response_cache", [])?)
}

#[cfg(feature = "backend-postgres")]
fn cache_clear_all(client: &mut postgres::Client) -> Result<usize> {
    Ok(client.execute("DELETE FROM response_cache", &[])? as usize)
}

#[cfg(feature = "backend-sqlite")]
fn cache_entry_count(conn: &rusqlite::Connection) -> Result<u64> {
    let count = conn.query_row("SELECT COUNT(*) FROM response_cache", [], |row| {
        row.get::<_, i64>(0)
    })?;
    Ok(count.max(0) as u64)
}

#[cfg(feature = "backend-postgres")]
fn cache_entry_count(client: &mut postgres::Client) -> Result<u64> {
    let row = client.query_one("SELECT COUNT(*) FROM response_cache", &[])?;
    let count: i64 = row.get(0);
    Ok(count.max(0) as u64)
}

#[cfg(feature = "backend-sqlite")]
fn cache_stats(conn: &rusqlite::Connection, max_entries: usize) -> Result<ResponseCacheStats> {
    let (entry_count_raw, total_hits_raw) = conn.query_row(
        "SELECT COUNT(*), COALESCE(SUM(hit_count), 0) FROM response_cache",
        [],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    )?;
    Ok(build_stats(entry_count_raw, total_hits_raw, max_entries))
}

#[cfg(feature = "backend-postgres")]
fn cache_stats(client: &mut postgres::Client, max_entries: usize) -> Result<ResponseCacheStats> {
    let row = client.query_one(
        "SELECT COUNT(*), COALESCE(SUM(hit_count), 0) FROM response_cache",
        &[],
    )?;
    let entry_count_raw: i64 = row.get(0);
    let total_hits_raw: i64 = row.get(1);
    Ok(build_stats(entry_count_raw, total_hits_raw, max_entries))
}

/// Shared stats aggregation from raw SQL aggregates.
fn build_stats(
    entry_count_raw: i64,
    total_hits_raw: i64,
    max_entries: usize,
) -> ResponseCacheStats {
    let entry_count = entry_count_raw.max(0) as u64;
    let total_hits = total_hits_raw.max(0) as u64;
    let avg_hits_per_entry = if entry_count == 0 {
        0.0
    } else {
        total_hits as f64 / entry_count as f64
    };
    ResponseCacheStats {
        entry_count,
        max_entries,
        total_hits,
        avg_hits_per_entry,
    }
}

/// SQLite-based response cache
#[cfg(feature = "backend-sqlite")]
#[derive(Debug)]
pub struct ResponseCache {
    /// SQLite connection (rwlock-protected)
    conn: Arc<Mutex<Connection>>,
    /// Default time-to-live for cache entries in seconds
    default_ttl_seconds: u64,
    /// Maximum number of entries to keep in the cache
    max_entries: usize,
}

#[cfg(feature = "backend-sqlite")]
impl ResponseCache {
    /// Create a new response cache
    ///
    /// # Arguments
    /// * `path` - Path to the SQLite database file
    /// * `default_ttl_seconds` - Default time-to-live for cache entries
    /// * `max_entries` - Maximum number of entries to keep in the cache
    ///
    /// # Returns
    /// * `Result<Self>` - Returns Ok(Self) if the cache is created successfully, or an error if something goes wrong
    pub fn new(path: &Path, default_ttl_seconds: u64, max_entries: usize) -> Result<Self> {
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

            CREATE TABLE IF NOT EXISTS response_cache (
                cache_key TEXT PRIMARY KEY,
                response_text TEXT NOT NULL,
                agent_name TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL,
                hit_count INTEGER NOT NULL DEFAULT 0,
                last_hit_at INTEGER
            );

            CREATE INDEX IF NOT EXISTS idx_response_cache_expires_at
                ON response_cache(expires_at);
            ",
        )?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            default_ttl_seconds,
            max_entries,
        })
    }

    /// Get a cached response by key
    ///
    /// Read-only: expired rows are filtered by the query (`expires_at > now`)
    /// and never deleted from the read path — cleanup is left to `put`'s
    /// over-budget eviction (previously every `get` ran a DELETE on the
    /// database, turning the hot read path into a writer).
    ///
    /// # Arguments
    /// * `cache_key` - The cache key to look up
    ///
    /// # Returns
    /// * `Result<Option<CachedResponse>>` - Returns Ok(Some(CachedResponse)) if the key is found and not expired, Ok(None) if not found, or an error if something goes wrong
    pub async fn get(&self, cache_key: &str) -> Result<Option<CachedResponse>> {
        let _permit = crate::shared::db_pool::acquire_db_permit().await;
        let conn = self.conn.clone();
        let cache_key = cache_key.to_string();
        spawn_blocking(move || {
            let now = now_ts();
            let conn = conn.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            cache_get(&conn, &cache_key, now)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))?
    }

    /// Put a response into the cache
    ///
    /// # Arguments
    /// * `cache_key` - The cache key to use
    /// * `response_text` - The response text to cache
    /// * `agent_name` - The name of the agent that generated the response
    /// * `ttl_seconds` - Optional time-to-live for this entry (overrides default)
    ///
    /// # Returns
    /// * `Result<()>` - Returns Ok(()) if the entry is cached successfully, or an error if something goes wrong
    pub async fn put(
        &self,
        cache_key: &str,
        response_text: &str,
        agent_name: &str,
        ttl_seconds: Option<u64>,
    ) -> Result<()> {
        let _permit = crate::shared::db_pool::acquire_db_permit().await;
        let conn = self.conn.clone();
        let default_ttl_seconds = self.default_ttl_seconds;
        let max_entries = self.max_entries;
        let response_text = response_text.to_string();
        let agent_name = agent_name.to_string();
        let cache_key = cache_key.to_string();
        spawn_blocking(move || {
            if response_text.trim().is_empty() {
                return Ok(());
            }

            let ttl = ttl_seconds.unwrap_or(default_ttl_seconds);
            if ttl == 0 {
                return Ok(());
            }

            let now = now_ts();
            let params = PutParams {
                cache_key,
                response_text,
                agent_name,
                now,
                expires_at: now + ttl as i64,
                max_entries: max_entries as i64,
            };

            let conn = conn.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            cache_put(&conn, &params)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))?
    }

    /// Clear all entries from the cache
    ///
    /// # Returns
    /// * `Result<usize>` - Returns Ok(usize) with the number of entries cleared, or an error if something goes wrong
    pub async fn clear_all(&self) -> Result<usize> {
        let _permit = crate::shared::db_pool::acquire_db_permit().await;
        let conn = self.conn.clone();
        spawn_blocking(move || {
            let conn = conn.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            cache_clear_all(&conn)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))?
    }

    /// Get the number of entries in the cache
    ///
    /// # Returns
    /// * `Result<u64>` - Returns Ok(u64) with the number of entries, or an error if something goes wrong
    pub async fn entry_count(&self) -> Result<u64> {
        let _permit = crate::shared::db_pool::acquire_db_permit().await;
        let conn = self.conn.clone();
        spawn_blocking(move || {
            let conn = conn.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            cache_entry_count(&conn)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))?
    }

    /// Get aggregate cache statistics used by ACP cache observability APIs.
    pub async fn stats(&self) -> Result<ResponseCacheStats> {
        let _permit = crate::shared::db_pool::acquire_db_permit().await;
        let conn = self.conn.clone();
        let max_entries = self.max_entries;
        spawn_blocking(move || {
            let conn = conn.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            cache_stats(&conn, max_entries)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))?
    }
}

/// L3 durable-backend bridge: lets the multi-level token cache fall through
/// to the SQLite response cache on L1/L2 miss (see `TokenMultiLevelCache`).
#[cfg(feature = "backend-sqlite")]
#[async_trait::async_trait]
impl crate::intelligence::token_cache::PersistentCacheBackend for ResponseCache {
    async fn get_cached(
        &self,
        key: &str,
    ) -> anyhow::Result<Option<crate::intelligence::token_cache::PersistentCachedResponse>> {
        Ok(self.get(key).await?.map(|c| {
            crate::intelligence::token_cache::PersistentCachedResponse {
                response_text: c.response_text,
                agent_name: c.agent_name,
            }
        }))
    }

    async fn put_cached(
        &self,
        key: &str,
        response_text: &str,
        agent_name: Option<&str>,
    ) -> anyhow::Result<()> {
        self.put(key, response_text, agent_name.unwrap_or(""), None)
            .await
    }
}

#[cfg(all(test, feature = "backend-sqlite"))]
mod tests {
    use super::ResponseCache;

    #[tokio::test]
    async fn cache_put_and_get_roundtrip() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let db_path = dir.path().join("cache.sqlite3");

        let cache = ResponseCache::new(&db_path, 60, 10).expect("cache should initialize");
        cache
            .put("k1", "cached response", "deepseek", None)
            .await
            .expect("cache put should succeed");

        let hit = cache.get("k1").await.expect("cache get should succeed");
        assert!(hit.is_some());

        let entry = hit.expect("cache entry should exist");
        assert_eq!(entry.response_text, "cached response");
        assert_eq!(entry.agent_name.as_deref(), Some("deepseek"));
    }

    #[tokio::test]
    async fn cache_stats_reports_entries_and_hits() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let db_path = dir.path().join("cache.sqlite3");

        let cache = ResponseCache::new(&db_path, 60, 10).expect("cache should initialize");
        cache
            .put("k1", "r1", "agent", None)
            .await
            .expect("cache put should succeed");
        cache
            .put("k2", "r2", "agent", None)
            .await
            .expect("cache put should succeed");

        let _ = cache.get("k1").await.expect("cache get should succeed");
        let _ = cache.get("k1").await.expect("cache get should succeed");
        let _ = cache.get("k2").await.expect("cache get should succeed");

        let stats = cache.stats().await.expect("cache stats should succeed");
        assert_eq!(stats.entry_count, 2);
        assert_eq!(stats.max_entries, 10);
        assert_eq!(stats.total_hits, 3);
        assert!((stats.avg_hits_per_entry - 1.5).abs() < f64::EPSILON);
    }
}

// ─── PostgreSQL backend (multi-users-server) ─────────────────────────
//
// Methods share the same sync signature as the SQLite backend so all callers
// (spawn_blocking wrappers in storage.rs / background.rs) work without changes.
// Internally this uses the synchronous `postgres` client, which fits the
// existing sync API and the current `spawn_blocking` call sites.
#[cfg(feature = "backend-postgres")]
use crate::memory::pg_migrate::run_migrations;
// The duplicated PostgreSQL TLS connect stack (parse_sslmode / PermissiveVerifier /
// connect_postgres) was unified into `crate::memory::pg_pool` — both store
// backends share a single `sslmode`-aware connect path.
#[cfg(feature = "backend-postgres")]
use crate::memory::pg_pool::{
    connect_postgres, create_pool, create_pool_pair, pool_get, resolve_pg_dsn, PgPoolPair,
};

#[cfg(feature = "backend-postgres")]
pub struct ResponseCache {
    pool: PgPoolPair,
    default_ttl_seconds: u64,
    max_entries: usize,
}

#[cfg(feature = "backend-postgres")]
impl std::fmt::Debug for ResponseCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResponseCache")
            .field("pool", &"<PgPoolPair>")
            .field("default_ttl_seconds", &self.default_ttl_seconds)
            .field("max_entries", &self.max_entries)
            .finish()
    }
}

#[cfg(feature = "backend-postgres")]
impl ResponseCache {
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
    pub fn new(url: &str, default_ttl_seconds: u64, max_entries: usize) -> Result<Self> {
        Self::new_with_replica(url, None, default_ttl_seconds, max_entries)
    }

    /// Create a new cache with an optional read-replica connection for read/write splitting.
    ///
    /// When `read_replica_url` is `Some`, read queries use the replica pool;
    /// when `None`, the primary pool is used for both reads and writes.
    ///
    /// The DSN is resolved through the canonical `pg_pool::resolve_pg_dsn`
    /// resolver (config `connection_string` → `GO_ON_PG_CONNECTION_STRING` →
    /// `DATABASE_URL` → `PG_DSN` → `GO_ON_DATABASE_URL`), keeping the fallback
    /// order identical to the vector store and memory warm tier.
    pub fn new_with_replica(
        url: &str,
        read_replica_url: Option<String>,
        default_ttl_seconds: u64,
        max_entries: usize,
    ) -> Result<Self> {
        let url = resolve_pg_dsn(Some(url)).ok_or_else(|| {
            anyhow::anyhow!(
                "no PostgreSQL connection string configured (set config cache.connection_string, GO_ON_PG_CONNECTION_STRING, DATABASE_URL, PG_DSN or GO_ON_DATABASE_URL)"
            )
        })?;
        let max_pool_size = 8;
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

        // Run schema migrations on the write pool.
        let mut conn = pool_get(&pool.write)?;
        run_migrations(&mut conn, 1)?; // target v1 (response_cache)

        // Startup health check: verify the connection is alive.
        conn.query_one("SELECT 1", &[])
            .map_err(|e| anyhow::anyhow!("postgres health check (SELECT 1) failed: {e}"))?;

        Ok(Self {
            pool,
            default_ttl_seconds,
            max_entries,
        })
    }

    pub async fn get(&self, cache_key: &str) -> Result<Option<CachedResponse>> {
        let _permit = crate::shared::db_pool::acquire_db_permit().await;
        let pool = self.pool.read.clone();
        let cache_key = cache_key.to_string();
        spawn_blocking(move || {
            let mut client = pool_get(&pool)?;
            let now = now_ts();
            cache_get(&mut client, &cache_key, now)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))?
    }

    pub async fn put(
        &self,
        cache_key: &str,
        response_text: &str,
        agent_name: &str,
        ttl_seconds: Option<u64>,
    ) -> Result<()> {
        let _permit = crate::shared::db_pool::acquire_db_permit().await;
        let pool = self.pool.write.clone();
        let default_ttl_seconds = self.default_ttl_seconds;
        let max_entries = self.max_entries;
        let response_text = response_text.to_string();
        let agent_name = agent_name.to_string();
        let cache_key = cache_key.to_string();
        spawn_blocking(move || {
            if response_text.trim().is_empty() {
                return Ok(());
            }
            let ttl = ttl_seconds.unwrap_or(default_ttl_seconds);
            if ttl == 0 {
                return Ok(());
            }
            let mut client = pool_get(&pool)?;
            let now = now_ts();
            let params = PutParams {
                cache_key,
                response_text,
                agent_name,
                now,
                expires_at: now + ttl as i64,
                max_entries: max_entries as i64,
            };
            cache_put(&mut client, &params)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))?
    }

    pub async fn clear_all(&self) -> Result<usize> {
        let _permit = crate::shared::db_pool::acquire_db_permit().await;
        let pool = self.pool.write.clone();
        spawn_blocking(move || {
            let mut client = pool_get(&pool)?;
            cache_clear_all(&mut client)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))?
    }

    pub async fn entry_count(&self) -> Result<u64> {
        let _permit = crate::shared::db_pool::acquire_db_permit().await;
        let pool = self.pool.read.clone();
        spawn_blocking(move || {
            let mut client = pool_get(&pool)?;
            cache_entry_count(&mut client)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))?
    }

    pub async fn stats(&self) -> Result<ResponseCacheStats> {
        let _permit = crate::shared::db_pool::acquire_db_permit().await;
        let pool = self.pool.read.clone();
        let max_entries = self.max_entries;
        spawn_blocking(move || {
            let mut client = pool_get(&pool)?;
            cache_stats(&mut client, max_entries)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))?
    }
}

/// L3 durable-backend bridge for the PostgreSQL backend — see the SQLite
/// variant above.
#[cfg(feature = "backend-postgres")]
#[async_trait::async_trait]
impl crate::intelligence::token_cache::PersistentCacheBackend for ResponseCache {
    async fn get_cached(
        &self,
        key: &str,
    ) -> anyhow::Result<Option<crate::intelligence::token_cache::PersistentCachedResponse>> {
        Ok(self.get(key).await?.map(|c| {
            crate::intelligence::token_cache::PersistentCachedResponse {
                response_text: c.response_text,
                agent_name: c.agent_name,
            }
        }))
    }

    async fn put_cached(
        &self,
        key: &str,
        response_text: &str,
        agent_name: Option<&str>,
    ) -> anyhow::Result<()> {
        self.put(key, response_text, agent_name.unwrap_or(""), None)
            .await
    }
}
