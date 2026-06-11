use std::sync::Mutex;

use indexmap::IndexMap;

use crate::acp::prelude::now_ts;

#[derive(Debug, Clone)]
pub(crate) struct MemoryCachedResponse {
    pub(crate) response_text: String,
    expires_at: i64,
}

#[derive(Debug, Default)]
pub struct MemoryResponseCache {
    inner: Mutex<IndexMap<String, MemoryCachedResponse>>,
}

impl MemoryResponseCache {
    pub(crate) fn get(&self, key: &str) -> Option<MemoryCachedResponse> {
        let now = now_ts();
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        // Check expiry and promote to MRU position on access.
        if let Some(entry) = guard.get(key) {
            if entry.expires_at <= now {
                guard.shift_remove(key);
                return None;
            }
            // Promote to MRU (back of IndexMap) by removing and re-inserting.
            let entry = entry.clone();
            guard.shift_remove(key);
            guard.insert(key.to_string(), entry.clone());
            return Some(entry);
        }
        None
    }

    pub(crate) fn purge_expired(&self) -> usize {
        let now = now_ts();
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let before = guard.len();
        guard.retain(|_, entry| entry.expires_at > now);
        before.saturating_sub(guard.len())
    }

    pub(crate) fn clear_all(&self) -> usize {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let removed = guard.len();
        guard.clear();
        removed
    }

    pub(crate) fn active_entries(&self) -> usize {
        // Single lock scope: purge expired entries and count in one atomic
        // operation, avoiding a TOCTOU race where a concurrent put() adds a
        // new entry between purge and re-lock.
        let now = now_ts();
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        guard.retain(|_, entry| entry.expires_at > now);
        guard.len()
    }

    pub(crate) fn put(
        &self,
        key: String,
        response_text: String,
        _agent_name: Option<String>,
        ttl_seconds: u64,
    ) {
        if ttl_seconds == 0 {
            return;
        }

        let expires_at = now_ts() + ttl_seconds as i64;
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());

        // Remove old entry first if it exists (so re-insert moves to back).
        guard.shift_remove(&key);
        guard.insert(
            key,
            MemoryCachedResponse {
                response_text,
                expires_at,
            },
        );

        // Keep L1 cache bounded to avoid unbounded memory growth.
        // LRU eviction: first purge expired entries, then evict oldest (front).
        const MAX_ENTRIES: usize = 2048;
        if guard.len() > MAX_ENTRIES {
            guard.retain(|_, v| v.expires_at > now_ts());
            while guard.len() > MAX_ENTRIES {
                // IndexMap preserves insertion order: front = LRU, back = MRU.
                guard.swap_remove_index(0);
            }
        }
    }
}
