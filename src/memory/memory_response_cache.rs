use std::sync::Mutex;

use indexmap::IndexMap;

use crate::acp::prelude::now_ts;

#[derive(Debug, Clone)]
pub(crate) struct MemoryCachedResponse {
    pub(crate) response_text: String,
    pub(crate) expires_at: i64,
}

#[derive(Debug, Default)]
pub struct MemoryResponseCache {
    pub(crate) inner: Mutex<IndexMap<String, MemoryCachedResponse>>,
}

impl MemoryResponseCache {
    /// Retrieve a cached response by key. Returns `None` if expired or absent.
    ///
    /// On hit, the entry is promoted to the back of the cache (most recently used)
    /// using `move_index` to avoid the remove-then-reinsert double hash pattern.
    pub(crate) fn get(&self, key: &str) -> Option<MemoryCachedResponse> {
        let now = now_ts();
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = guard.get(key) {
            if entry.expires_at <= now {
                guard.shift_remove(key);
                return None;
            }
            // Use IndexMap's get_index_of and move_index to promote in O(1)
            // without remove + reinsert (saves one hash + one clone).
            let entry = entry.clone();
            if let Some(idx) = guard.get_index_of(key) {
                let last = guard.len() - 1;
                if idx != last {
                    guard.move_index(idx, last);
                }
            }
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

    /// Purge expired entries and return the count of remaining non-expired entries.
    pub(crate) fn prune_and_count(&self) -> usize {
        let now = now_ts();
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.retain(|_, entry| entry.expires_at > now);
        guard.len()
    }

    /// Insert a response into the cache with TTL. No-op if `ttl_seconds` is 0.
    pub(crate) fn put(&self, key: String, response_text: String, ttl_seconds: u64) {
        if ttl_seconds == 0 {
            return;
        }

        let expires_at = now_ts() + ttl_seconds as i64;
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());

        // Use entry API to avoid separate remove + insert hash lookups
        let entry = MemoryCachedResponse {
            response_text,
            expires_at,
        };
        guard.insert(key, entry);

        const MAX_ENTRIES: usize = 2048;
        if guard.len() > MAX_ENTRIES {
            guard.retain(|_, v| v.expires_at > now_ts());
            while guard.len() > MAX_ENTRIES {
                guard.swap_remove_index(0);
            }
        }
    }
}
