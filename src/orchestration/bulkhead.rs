//! Bulkhead pattern — per-service concurrency isolation.
//!
//! Prevents a single LLM provider or tool from exhausting all worker threads.
//! Each provider gets its own semaphore with configurable capacity.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Manages per-provider bulkhead semaphores.
///
/// Each provider (e.g. an LLM API or tool executor) gets its own
/// `tokio::sync::Semaphore` with a configurable capacity.  Acquiring a
/// permit before dispatching work prevents one provider from consuming
/// all available worker threads.
pub struct Bulkhead {
    /// Per-provider semaphores for bulkhead isolation.
    semaphores: RwLock<HashMap<String, Arc<Semaphore>>>,
    /// Default per-provider concurrency limit when no explicit limit has been set.
    default_limit: usize,
}

impl Bulkhead {
    /// Create a new `Bulkhead` with the given default per-provider limit.
    ///
    /// The `default_limit` is used when `acquire()` is called for a provider
    /// that has no explicit limit set via [`set_limit`](Self::set_limit).
    pub fn new(default_limit: usize) -> Self {
        Self {
            semaphores: RwLock::new(HashMap::new()),
            default_limit,
        }
    }

    /// Set the maximum number of concurrent operations for a provider.
    ///
    /// If a semaphore already exists for this provider it is replaced,
    /// so in-flight permits obtained from the previous semaphore are
    /// **not** affected.
    pub fn set_limit(&self, provider: &str, limit: usize) {
        if let Ok(mut map) = self.semaphores.write() {
            map.insert(provider.to_string(), Arc::new(Semaphore::new(limit)));
        }
    }

    /// Acquire a permit for the given provider, blocking until one is available.
    ///
    /// If no semaphore exists for this provider yet, one is created with the
    /// default limit that was passed to [`new`](Self::new).
    ///
    /// Returns `None` if the semaphore has been closed (should not happen
    /// under normal operation).
    pub async fn acquire(&self, provider: &str) -> Option<OwnedSemaphorePermit> {
        let semaphore = {
            let map = self.semaphores.read().ok()?;
            map.get(provider).cloned()
        };

        let semaphore = match semaphore {
            Some(s) => s,
            None => {
                let mut map = self.semaphores.write().ok()?;
                // Double-check after acquiring the write lock to avoid races.
                map.get(provider).cloned().unwrap_or_else(|| {
                    let s = Arc::new(Semaphore::new(self.default_limit));
                    map.insert(provider.to_string(), s.clone());
                    s
                })
            }
        };

        semaphore.acquire_owned().await.ok()
    }

    /// Try to acquire a permit for the given provider without blocking.
    ///
    /// If no semaphore exists for this provider yet, one is created with the
    /// default limit.
    ///
    /// Returns `Ok(Some(permit))` on success, `Ok(None)` if no permits are
    /// currently available, or `Err` if the lock is poisoned.
    pub fn try_acquire(
        &self,
        provider: &str,
    ) -> Result<Option<OwnedSemaphorePermit>, &'static str> {
        // Fast path: try read lock first.
        let map = match self.semaphores.read() {
            Ok(map) => map,
            Err(_) => return Err("bulkhead semaphore lock poisoned"),
        };
        if let Some(semaphore) = map.get(provider) {
            let semaphore = semaphore.clone();
            return match semaphore.try_acquire_owned() {
                Ok(permit) => Ok(Some(permit)),
                Err(_) => Ok(None),
            };
        }
        drop(map);

        // Slow path: acquire write lock to create the semaphore.
        let mut map = match self.semaphores.write() {
            Ok(map) => map,
            Err(_) => return Err("bulkhead write lock poisoned"),
        };
        let semaphore = map.get(provider).cloned().unwrap_or_else(|| {
            let s = Arc::new(Semaphore::new(self.default_limit));
            map.insert(provider.to_string(), s.clone());
            s
        });
        match semaphore.try_acquire_owned() {
            Ok(permit) => Ok(Some(permit)),
            Err(_) => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_acquire_and_release() {
        let bulkhead = Bulkhead::new(2);
        let p1 = bulkhead.acquire("openai").await;
        assert!(p1.is_some());
        let p2 = bulkhead.acquire("openai").await;
        assert!(p2.is_some());
        // Third should block — we can't test blocking in a simple way,
        // but verify our permits exist.
        drop(p1);
        drop(p2);
    }

    #[tokio::test]
    async fn test_set_limit_replaces_semaphore() {
        let bulkhead = Bulkhead::new(1);
        bulkhead.set_limit("openai", 5);
        let p1 = bulkhead.acquire("openai").await;
        assert!(p1.is_some());
        let p2 = bulkhead.acquire("openai").await;
        assert!(p2.is_some());
        let p3 = bulkhead.acquire("openai").await;
        assert!(p3.is_some());
        let p4 = bulkhead.acquire("openai").await;
        assert!(p4.is_some());
        let p5 = bulkhead.acquire("openai").await;
        assert!(p5.is_some());
        // Sixth would block — drop permits.
        drop((p1, p2, p3, p4, p5));
    }

    #[tokio::test]
    async fn test_separate_providers_independent() {
        let bulkhead = Bulkhead::new(1);
        let p1 = bulkhead.acquire("openai").await;
        assert!(p1.is_some());
        let p2 = bulkhead.acquire("anthropic").await;
        assert!(p2.is_some());
        // Both providers have their own permits.
        drop((p1, p2));
    }

    #[test]
    fn test_try_acquire_non_blocking() {
        let bulkhead = Bulkhead::new(1);
        let p1 = bulkhead.try_acquire("openai").unwrap();
        assert!(p1.is_some());
        // Second acquire should fail because the first permit is still held.
        assert!(bulkhead.try_acquire("openai").unwrap().is_none());
        drop(p1);
    }

    #[test]
    fn test_try_acquire_lazy_creation() {
        let bulkhead = Bulkhead::new(3);
        let p1 = bulkhead.try_acquire("new-provider").unwrap();
        assert!(p1.is_some());
        let p2 = bulkhead.try_acquire("new-provider").unwrap();
        assert!(p2.is_some());
        let p3 = bulkhead.try_acquire("new-provider").unwrap();
        assert!(p3.is_some());
        assert!(bulkhead.try_acquire("new-provider").unwrap().is_none());
        drop((p1, p2, p3));
    }
}
