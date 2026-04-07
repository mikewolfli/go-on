//! Cache implementation
//!
//! This module provides a SQLite-based response cache for storing and retrieving agent responses.

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

/// Cached response structure
pub struct CachedResponse {
    /// The cached response text
    pub response_text: String,
    /// The name of the agent that generated the response
    pub agent_name: Option<String>,
}

/// SQLite-based response cache
pub struct ResponseCache {
    /// SQLite connection (mutex-protected)
    conn: Mutex<Connection>,
    /// Default time-to-live for cache entries in seconds
    default_ttl_seconds: u64,
    /// Maximum number of entries to keep in the cache
    max_entries: usize,
}

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
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
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
}
