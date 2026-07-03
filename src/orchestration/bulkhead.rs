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
    ///
    /// This method is a public API intended for runtime configuration.
    /// The binary build does not invoke it yet; it is kept as a tested,
    /// meaningful public method for future wiring.
    #[allow(dead_code, reason = "public API reserved for runtime configuration")]
    pub fn set_limit(&self, provider: &str, limit: usize) {
        let mut map = match self.semaphores.write() {
            Ok(map) => map,
            Err(poisoned) => {
                tracing::warn!("bulkhead write lock poisoned in set_limit");
                poisoned.into_inner()
            }
        };
        map.insert(provider.to_string(), Arc::new(Semaphore::new(limit)));
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

    #[test]
    fn test_set_limit_replaces_semaphore() {
        let bulkhead = Bulkhead::new(1);
        // Acquire the only permit.
        let p1 = bulkhead.try_acquire("svc").unwrap().expect("first permit");
        assert!(bulkhead.try_acquire("svc").unwrap().is_none());
        drop(p1);

        // Increase the limit and verify we can now acquire more.
        bulkhead.set_limit("svc", 5);
        let p2 = bulkhead
            .try_acquire("svc")
            .unwrap()
            .expect("first after resize");
        let p3 = bulkhead
            .try_acquire("svc")
            .unwrap()
            .expect("second after resize");
        drop((p2, p3));
    }
}
