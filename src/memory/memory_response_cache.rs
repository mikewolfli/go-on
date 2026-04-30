use std::collections::HashMap;
use std::sync::Mutex as StdMutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub(crate) struct MemoryCachedResponse {
    #[allow(dead_code)] // Bucket F — accessed via clone from get()
    pub(crate) response_text: String,
    expires_at: i64,
}

#[derive(Default)]
pub struct MemoryResponseCache {
    inner: StdMutex<HashMap<String, MemoryCachedResponse>>,
}

impl MemoryResponseCache {
    #[allow(dead_code)] // Bucket F — used by agent response cache layer
    pub(crate) fn get(&self, key: &str) -> Option<MemoryCachedResponse> {
        let now = now_ts();
        let mut guard = self.inner.lock().ok()?;
        guard.retain(|_, entry| entry.expires_at > now);
        guard.get(key).cloned()
    }

    pub(crate) fn purge_expired(&self) -> usize {
        let now = now_ts();
        if let Ok(mut guard) = self.inner.lock() {
            let before = guard.len();
            guard.retain(|_, entry| entry.expires_at > now);
            return before.saturating_sub(guard.len());
        }
        0
    }

    pub(crate) fn clear_all(&self) -> usize {
        if let Ok(mut guard) = self.inner.lock() {
            let removed = guard.len();
            guard.clear();
            return removed;
        }
        0
    }

    pub(crate) fn active_entries(&self) -> usize {
        self.purge_expired();
        self.inner.lock().map(|guard| guard.len()).unwrap_or(0)
    }

    #[allow(dead_code)] // Bucket F — used to store agent responses
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
        if let Ok(mut guard) = self.inner.lock() {
            guard.insert(
                key,
                MemoryCachedResponse {
                    response_text,
                    expires_at,
                },
            );

            // Keep L1 cache bounded to avoid unbounded memory growth.
            if guard.len() > 2048 {
                let mut entries: Vec<(String, i64)> = guard
                    .iter()
                    .map(|(k, v)| (k.clone(), v.expires_at))
                    .collect();
                entries.sort_by_key(|(_, expires_at)| *expires_at);
                let remove_count = guard.len() - 2048;
                for (k, _) in entries.into_iter().take(remove_count) {
                    guard.remove(&k);
                }
            }
        }
    }
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
