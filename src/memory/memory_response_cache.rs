use std::collections::HashMap;
use std::sync::Mutex;

use crate::acp::prelude::now_ts;

#[derive(Debug, Clone)]
pub(crate) struct MemoryCachedResponse {
    pub(crate) response_text: String,
    expires_at: i64,
}

#[derive(Debug, Default)]
pub struct MemoryResponseCache {
    inner: Mutex<HashMap<String, MemoryCachedResponse>>,
}

impl MemoryResponseCache {
    pub(crate) fn get(&self, key: &str) -> Option<MemoryCachedResponse> {
        let now = now_ts();
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        // Only evict the requested key if expired; bulk cleanup happens in purge_expired().
        if let Some(entry) = guard.get(key) {
            if entry.expires_at <= now {
                guard.remove(key);
                return None;
            }
            return Some(entry.clone());
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
        self.purge_expired();
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).len()
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
        guard.insert(
            key,
            MemoryCachedResponse {
                response_text,
                expires_at,
            },
        );

        // Keep L1 cache bounded to avoid unbounded memory growth.
        // Eviction strategy: first purge expired entries (O(n)), then
        // drain excess entries arbitrarily without sorting (O(excess)).
        // This avoids the O(n log n) sort on every insert over the limit.
        if guard.len() > 2048 {
            guard.retain(|_, v| v.expires_at > now_ts());
            if guard.len() > 2048 {
                let excess = guard.len() - 2048;
                let keys: Vec<String> = guard.keys().take(excess).cloned().collect();
                for k in keys {
                    guard.remove(&k);
                }
            }
        }
    }
}
