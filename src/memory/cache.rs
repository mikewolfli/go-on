//! Cache implementation
//!
//! Conditionally compiled:
//! - `backend-sqlite` (profile-local, profile-simple-server): rusqlite-backed, sync API
//! - `backend-postgres` (profile-multi-users-server): postgres-backed sync API

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;

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

// ─── SQLite backend (profile-local / profile-simple-server) ──────────────────
#[cfg(not(feature = "backend-postgres"))]
use std::path::Path;
#[cfg(not(feature = "backend-postgres"))]
use rusqlite::{params, Connection, OptionalExtension};

/// SQLite-based response cache
#[cfg(not(feature = "backend-postgres"))]
pub struct ResponseCache {
    /// SQLite connection (mutex-protected)
    conn: Mutex<Connection>,
    /// Default time-to-live for cache entries in seconds
    default_ttl_seconds: u64,
    /// Maximum number of entries to keep in the cache
    max_entries: usize,
}

#[cfg(not(feature = "backend-postgres"))]
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
            conn: Mutex::new(conn),
            default_ttl_seconds,
            max_entries,
        })
    }

    /// Get a cached response by key
    ///
    /// # Arguments
    /// * `cache_key` - The cache key to look up
    ///
    /// # Returns
    /// * `Result<Option<CachedResponse>>` - Returns Ok(Some(CachedResponse)) if the key is found and not expired, Ok(None) if not found, or an error if something goes wrong
    pub fn get(&self, cache_key: &str) -> Result<Option<CachedResponse>> {
        let now = now_ts();
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("cache mutex poisoned in 'get'"))?;

        conn.execute(
            "DELETE FROM response_cache WHERE expires_at <= ?1",
            params![now],
        )?;

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
    pub fn put(
        &self,
        cache_key: &str,
        response_text: &str,
        agent_name: &str,
        ttl_seconds: Option<u64>,
    ) -> Result<()> {
        if response_text.trim().is_empty() {
            return Ok(());
        }

        let ttl = ttl_seconds.unwrap_or(self.default_ttl_seconds);
        if ttl == 0 {
            return Ok(());
        }

        let now = now_ts();
        let expires_at = now + ttl as i64;

        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("cache mutex poisoned in 'put'"))?;

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
            params![cache_key, response_text, agent_name, now, expires_at],
        )?;

        conn.execute(
            "
            DELETE FROM response_cache
            WHERE cache_key IN (
                SELECT cache_key
                FROM response_cache
                ORDER BY updated_at DESC
                LIMIT -1 OFFSET ?1
            )
            ",
            params![self.max_entries as i64],
        )?;

        Ok(())
    }

    /// Purge expired entries from the cache
    ///
    /// # Returns
    /// * `Result<usize>` - Returns Ok(usize) with the number of entries purged, or an error if something goes wrong
    pub fn purge_expired(&self) -> Result<usize> {
        let now = now_ts();
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("cache mutex poisoned in 'purge_expired'"))?;
        let affected = conn.execute(
            "DELETE FROM response_cache WHERE expires_at <= ?1",
            params![now],
        )?;
        Ok(affected)
    }

    /// Clear all entries from the cache
    ///
    /// # Returns
    /// * `Result<usize>` - Returns Ok(usize) with the number of entries cleared, or an error if something goes wrong
    pub fn clear_all(&self) -> Result<usize> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("cache mutex poisoned in 'clear'"))?;
        let affected = conn.execute("DELETE FROM response_cache", [])?;
        Ok(affected)
    }

    /// Reclaim SQLite free pages after cleanup-heavy maintenance cycles.
    pub fn vacuum(&self) -> Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("cache mutex poisoned in 'vacuum'"))?;
        conn.execute_batch("VACUUM;")?;
        Ok(())
    }

    /// Get the number of entries in the cache
    ///
    /// # Returns
    /// * `Result<u64>` - Returns Ok(u64) with the number of entries, or an error if something goes wrong
    pub fn entry_count(&self) -> Result<u64> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("cache mutex poisoned in 'entry_count'"))?;
        let count = conn.query_row("SELECT COUNT(*) FROM response_cache", [], |row| {
            row.get::<_, i64>(0)
        })?;
        Ok(count.max(0) as u64)
    }

    /// Get aggregate cache statistics used by ACP cache observability APIs.
    pub fn stats(&self) -> Result<ResponseCacheStats> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("cache mutex poisoned in 'stats'"))?;

        let (entry_count_raw, total_hits_raw) = conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(hit_count), 0) FROM response_cache",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )?;

        let entry_count = entry_count_raw.max(0) as u64;
        let total_hits = total_hits_raw.max(0) as u64;
        let avg_hits_per_entry = if entry_count == 0 {
            0.0
        } else {
            total_hits as f64 / entry_count as f64
        };

        Ok(ResponseCacheStats {
            entry_count,
            max_entries: self.max_entries,
            total_hits,
            avg_hits_per_entry,
        })
    }
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(all(test, not(feature = "backend-postgres")))]
mod tests {
    use super::ResponseCache;

    #[test]
    fn cache_put_and_get_roundtrip() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let db_path = dir.path().join("cache.sqlite3");

        let cache = ResponseCache::new(&db_path, 60, 10).expect("cache should initialize");
        cache
            .put("k1", "cached response", "deepseek", None)
            .expect("cache put should succeed");

        let hit = cache.get("k1").expect("cache get should succeed");
        assert!(hit.is_some());

        let entry = hit.expect("cache entry should exist");
        assert_eq!(entry.response_text, "cached response");
        assert_eq!(entry.agent_name.as_deref(), Some("deepseek"));
    }

    #[test]
    fn cache_stats_reports_entries_and_hits() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let db_path = dir.path().join("cache.sqlite3");

        let cache = ResponseCache::new(&db_path, 60, 10).expect("cache should initialize");
        cache
            .put("k1", "r1", "agent", None)
            .expect("cache put should succeed");
        cache
            .put("k2", "r2", "agent", None)
            .expect("cache put should succeed");

        let _ = cache.get("k1").expect("cache get should succeed");
        let _ = cache.get("k1").expect("cache get should succeed");
        let _ = cache.get("k2").expect("cache get should succeed");

        let stats = cache.stats().expect("cache stats should succeed");
        assert_eq!(stats.entry_count, 2);
        assert_eq!(stats.max_entries, 10);
        assert_eq!(stats.total_hits, 3);
        assert!((stats.avg_hits_per_entry - 1.5).abs() < f64::EPSILON);
    }
}

// ─── PostgreSQL backend (profile-multi-users-server) ─────────────────────────
//
// Methods share the same sync signature as the SQLite backend so all callers
// (spawn_blocking wrappers in storage.rs / background.rs) work without changes.
// Internally this uses the synchronous `postgres` client, which fits the
// existing sync API and the current `spawn_blocking` call sites.
#[cfg(feature = "backend-postgres")]
use postgres::{Client, NoTls};

#[cfg(feature = "backend-postgres")]
pub struct ResponseCache {
    client: Mutex<Client>,
    default_ttl_seconds: u64,
    max_entries: usize,
}

#[cfg(feature = "backend-postgres")]
impl ResponseCache {
    /// Connect to PostgreSQL and run schema migrations.
    ///
    /// `url` — libpq-style connection string, e.g.
    /// `"postgres://user:pass@localhost/go_on"`
    pub fn new(url: &str, default_ttl_seconds: u64, max_entries: usize) -> Result<Self> {
        let mut client = Client::connect(url, NoTls)?;
        client.batch_execute(
            "CREATE TABLE IF NOT EXISTS response_cache (
                cache_key        TEXT    PRIMARY KEY,
                response_text    TEXT    NOT NULL,
                agent_name       TEXT,
                created_at       BIGINT  NOT NULL,
                updated_at       BIGINT  NOT NULL,
                expires_at       BIGINT  NOT NULL,
                hit_count        BIGINT  NOT NULL DEFAULT 0,
                last_hit_at      BIGINT
            );
            CREATE INDEX IF NOT EXISTS idx_response_cache_expires_at
                ON response_cache(expires_at);",
        )?;

        Ok(Self {
            client: Mutex::new(client),
            default_ttl_seconds,
            max_entries,
        })
    }

    pub fn get(&self, cache_key: &str) -> Result<Option<CachedResponse>> {
        let mut client = self
            .client
            .lock()
            .map_err(|_| anyhow::anyhow!("cache mutex poisoned in 'get'"))?;
        let now = now_ts();
        client.execute("DELETE FROM response_cache WHERE expires_at <= $1", &[&now])?;

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

    pub fn put(
        &self,
        cache_key: &str,
        response_text: &str,
        agent_name: &str,
        ttl_seconds: Option<u64>,
    ) -> Result<()> {
        if response_text.trim().is_empty() {
            return Ok(());
        }
        let ttl = ttl_seconds.unwrap_or(self.default_ttl_seconds);
        if ttl == 0 {
            return Ok(());
        }
        let mut client = self
            .client
            .lock()
            .map_err(|_| anyhow::anyhow!("cache mutex poisoned in 'put'"))?;
        let max_entries = self.max_entries as i64;
        let now = now_ts();
        let expires_at = now + ttl as i64;
        client.execute(
            "INSERT INTO response_cache
                (cache_key, response_text, agent_name, created_at, updated_at, expires_at, hit_count)
             VALUES ($1, $2, $3, $4, $4, $5, 0)
             ON CONFLICT (cache_key) DO UPDATE SET
                response_text = EXCLUDED.response_text,
                agent_name    = EXCLUDED.agent_name,
                updated_at    = EXCLUDED.updated_at,
                expires_at    = EXCLUDED.expires_at",
            &[&cache_key, &response_text, &agent_name, &now, &expires_at],
        )?;

        client.execute(
            "DELETE FROM response_cache
             WHERE cache_key NOT IN (
                 SELECT cache_key FROM response_cache
                 ORDER BY updated_at DESC LIMIT $1
             )",
            &[&max_entries],
        )?;

        Ok(())
    }

    pub fn purge_expired(&self) -> Result<usize> {
        let mut client = self
            .client
            .lock()
            .map_err(|_| anyhow::anyhow!("cache mutex poisoned in 'purge_expired'"))?;
        let now = now_ts();
        Ok(client.execute("DELETE FROM response_cache WHERE expires_at <= $1", &[&now])? as usize)
    }

    pub fn clear_all(&self) -> Result<usize> {
        let mut client = self
            .client
            .lock()
            .map_err(|_| anyhow::anyhow!("cache mutex poisoned in 'clear_all'"))?;
        Ok(client.execute("DELETE FROM response_cache", &[])? as usize)
    }

    /// No-op on PostgreSQL — VACUUM is managed by autovacuum.
    pub fn vacuum(&self) -> Result<()> {
        Ok(())
    }

    pub fn entry_count(&self) -> Result<u64> {
        let mut client = self
            .client
            .lock()
            .map_err(|_| anyhow::anyhow!("cache mutex poisoned in 'entry_count'"))?;
        let row = client.query_one("SELECT COUNT(*) FROM response_cache", &[])?;
        let count: i64 = row.get(0);
        Ok(count.max(0) as u64)
    }

    pub fn stats(&self) -> Result<ResponseCacheStats> {
        let mut client = self
            .client
            .lock()
            .map_err(|_| anyhow::anyhow!("cache mutex poisoned in 'stats'"))?;
        let row = client.query_one(
            "SELECT COUNT(*), COALESCE(SUM(hit_count), 0) FROM response_cache",
            &[],
        )?;
        let entry_count_raw: i64 = row.get(0);
        let total_hits_raw: i64 = row.get(1);
        let entry_count = entry_count_raw.max(0) as u64;
        let total_hits = total_hits_raw.max(0) as u64;
        let avg_hits_per_entry = if entry_count == 0 {
            0.0
        } else {
            total_hits as f64 / entry_count as f64
        };
        Ok(ResponseCacheStats {
            entry_count,
            max_entries: self.max_entries,
            total_hits,
            avg_hits_per_entry,
        })
    }
}

