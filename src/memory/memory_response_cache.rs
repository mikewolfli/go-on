use std::sync::Mutex;

use indexmap::IndexMap;

use crate::acp::prelude::now_ts;

#[derive(Debug, Clone)]
pub(crate) struct MemoryCachedResponse {
    #[allow(
        dead_code,
        reason = "stored for retrieval, read via .response_text accessor"
    )]
    pub(crate) response_text: String,
    expires_at: i64,
}

#[derive(Debug, Default)]
pub struct MemoryResponseCache {
    inner: Mutex<IndexMap<String, MemoryCachedResponse>>,
}

impl MemoryResponseCache {
    /// Retrieve a cached response by key. Returns `None` if expired or absent.
    #[expect(dead_code, reason = "public API surface for cache integration")]
    pub(crate) fn get(&self, key: &str) -> Option<MemoryCachedResponse> {
        let now = now_ts();
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = guard.get(key) {
            if entry.expires_at <= now {
                guard.shift_remove(key);
                return None;
            }
            let entry = entry.clone();
            guard.shift_remove(key);
            guard.insert(key.to_string(), entry.clone());
            return Some(entry);
        }
        None
    }

    /// Purge all expired entries and return the count removed.
    pub(crate) fn purge_expired(&self) -> usize {
        let now = now_ts();
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let before = guard.len();
        guard.retain(|_, entry| entry.expires_at > now);
        before.saturating_sub(guard.len())
    }

    /// Clear all entries from the cache and return the count removed.
    pub(crate) fn clear_all(&self) -> usize {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let removed = guard.len();
        guard.clear();
        removed
    }

    /// Return the number of non-expired entries.
    #[expect(dead_code, reason = "public API surface for cache integration")]
    pub(crate) fn active_entries(&self) -> usize {
        let now = now_ts();
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.retain(|_, entry| entry.expires_at > now);
        guard.len()
    }

    /// Insert a response into the cache with TTL. No-op if `ttl_seconds` is 0.
    #[expect(dead_code, reason = "public API surface for cache integration")]
    pub(crate) fn put(&self, key: String, response_text: String, ttl_seconds: u64) {
        if ttl_seconds == 0 {
            return;
        }

        let expires_at = now_ts() + ttl_seconds as i64;
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());

        guard.shift_remove(&key);
        guard.insert(
            key,
            MemoryCachedResponse {
                response_text,
                expires_at,
            },
        );

        const MAX_ENTRIES: usize = 2048;
        if guard.len() > MAX_ENTRIES {
            guard.retain(|_, v| v.expires_at > now_ts());
            while guard.len() > MAX_ENTRIES {
                guard.swap_remove_index(0);
            }
        }
    }
}
