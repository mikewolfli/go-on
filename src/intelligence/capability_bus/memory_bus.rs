//! MemoryBus — Unified cache coordination sub-bus (BLUE38 ARCH-13)
//!
//! MemoryBus provides cascading cache coordination across:
//!
//! - **L1**: In-memory response cache (`MemoryResponseCache`)
//! - **L2**: SQLite/Postgres-backed response cache (`ResponseCache`)
//! - **L3**: Vector similarity store (`VectorStore`)
//!
//! Each backend is optional. When a backend is `None`, its level is
//! transparently skipped during lookup and store operations.

use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex, Mutex as StdMutex};

use crate::cache::ResponseCache;
use crate::memory_module::{MemoryPolicy, MemoryStore};
use crate::memory_response_cache::MemoryResponseCache;
use crate::vector::VectorStore;

// ---------------------------------------------------------------------------
// CacheStrategy — controls which tiers participate and entry TTL
// ---------------------------------------------------------------------------

/// Controls which cache tiers are used for a given lookup or store operation.
#[derive(Debug, Clone)]
pub struct CacheStrategy {
    /// If true, consult the in-memory L1 cache (`MemoryResponseCache`).
    pub use_l1_memory: bool,
    /// If true, consult the SQLite/Postgres-backed L2 cache (`ResponseCache`).
    pub use_l2_sqlite: bool,
    /// If true, consult the vector similarity L3 store (`VectorStore`).
    pub use_l3_vector: bool,
    /// Time-to-live in seconds for newly stored entries (0 = no caching).
    pub ttl_seconds: u64,
}

impl Default for CacheStrategy {
    fn default() -> Self {
        Self {
            use_l1_memory: true,
            use_l2_sqlite: true,
            use_l3_vector: false,
            ttl_seconds: 300, // 5 minutes
        }
    }
}

// ---------------------------------------------------------------------------
// MemoryBusProfile — runtime observability snapshot
// ---------------------------------------------------------------------------

/// Runtime profile / metrics snapshot for the MemoryBus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryBusProfile {
    /// Whether the bus is enabled.
    pub enabled: bool,
    /// Overall cache hit rate (hits / (hits + misses)).
    pub cache_hit_rate: f64,
    /// Number of document entries in the vector store.
    pub vector_docs_count: u32,
    /// Number of entries tracked in the in-memory memory store.
    pub memory_entries: u32,
    /// Total cache hits across all tiers since bus creation.
    pub total_cache_hits: u64,
    /// Total cache misses across all tiers since bus creation.
    pub total_cache_misses: u64,
}

impl Default for MemoryBusProfile {
    fn default() -> Self {
        Self {
            enabled: true,
            cache_hit_rate: 0.0,
            vector_docs_count: 0,
            memory_entries: 0,
            total_cache_hits: 0,
            total_cache_misses: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// MemoryBus — unified cache coordinator
// ---------------------------------------------------------------------------

/// Unified cache coordination sub-bus that sits between the CapabilityBus
/// scheduler and the four cache/memory backends.
///
/// Each backend is `Option`-wrapped. When `None`, the corresponding tier is
/// transparently skipped. This keeps the MemoryBus generic and avoids coupling
/// to concrete backend lifetimes.
pub struct MemoryBus {
    /// Response cache reference (L2 — SQLite or Postgres).
    response_cache: Option<Arc<ResponseCache>>,
    /// Vector store reference (L3).
    vector_store: Option<Arc<VectorStore>>,
    /// In-memory memory store (wrapped in StdMutex for sync access).
    memory_store: Option<Arc<StdMutex<MemoryStore>>>,
    /// In-memory response cache (L1).
    memory_response_cache: Option<Arc<StdMutex<MemoryResponseCache>>>,
    /// Runtime profile / metrics.
    profile: Arc<Mutex<MemoryBusProfile>>,
}

impl MemoryBus {
    /// Construct a new `MemoryBus` with optional cache backends.
    ///
    /// Any backend passed as `None` will be skipped during lookups and stores.
    pub fn new(
        response_cache: Option<Arc<ResponseCache>>,
        vector_store: Option<Arc<VectorStore>>,
        memory_store: Option<Arc<StdMutex<MemoryStore>>>,
        memory_response_cache: Option<Arc<StdMutex<MemoryResponseCache>>>,
    ) -> Self {
        Self {
            response_cache,
            vector_store,
            memory_store,
            memory_response_cache,
            profile: Arc::new(Mutex::new(MemoryBusProfile::default())),
        }
    }

    /// Populate backends with sensible defaults from environment variables.
    ///
    /// Creates default in-memory L1 (`MemoryResponseCache`) and memory store
    /// (`MemoryStore`) backends. L2 (`ResponseCache`) and L3 (`VectorStore`)
    /// remain unset and can be wired later via `set_backends`.
    ///
    /// This ensures the MemoryBus never operates with all backends `None`,
    /// which would silently discard all data.
    pub fn with_default_backends(mut self) -> Self {
        if self.memory_response_cache.is_none() {
            self.memory_response_cache =
                Some(Arc::new(StdMutex::new(MemoryResponseCache::default())));
        }
        if self.memory_store.is_none() {
            self.memory_store = Some(Arc::new(StdMutex::new(MemoryStore::new(
                MemoryPolicy::default(),
            ))));
        }
        self
    }

    /// Set (or replace) cache backends after construction.
    /// Any backend passed as `None` will be left unchanged.
    pub fn set_backends(
        &mut self,
        response_cache: Option<Option<Arc<ResponseCache>>>,
        vector_store: Option<Option<Arc<VectorStore>>>,
        memory_store: Option<Option<Arc<StdMutex<MemoryStore>>>>,
        memory_response_cache: Option<Option<Arc<StdMutex<MemoryResponseCache>>>>,
    ) {
        if let Some(rc) = response_cache {
            self.response_cache = rc;
        }
        if let Some(vs) = vector_store {
            self.vector_store = vs;
        }
        if let Some(ms) = memory_store {
            self.memory_store = ms;
        }
        if let Some(mrc) = memory_response_cache {
            self.memory_response_cache = mrc;
        }
    }

    /// Cascading cache lookup: L1 (memory) → L2 (SQLite) → L3 (vector).
    ///
    /// Returns the first hit found according to the provided `strategy`.
    /// If no tier is configured or no entry is found, returns `None`.
    pub fn lookup(&self, key: &str, strategy: &CacheStrategy) -> Option<Vec<u8>> {
        let mut profile = self.profile.lock().unwrap_or_else(|e| e.into_inner());

        // ---- L1: In-memory response cache ----
        if strategy.use_l1_memory {
            if let Some(ref mrc) = self.memory_response_cache {
                let guard = mrc.lock().unwrap_or_else(|poisoned| {
                    tracing::warn!("memory_response_cache lock poisoned in lookup L1 – recovered");
                    poisoned.into_inner()
                });
                let cached = guard.get(key);
                drop(guard);
                if cached.is_some() {
                    profile.total_cache_hits += 1;
                    let total = profile.total_cache_hits + profile.total_cache_misses;
                    profile.cache_hit_rate = if total == 0 {
                        0.0
                    } else {
                        profile.total_cache_hits as f64 / total as f64
                    };
                    // MemoryResponseCache stores String; convert to Vec<u8>.
                    return cached.map(|entry| entry.response_text.into_bytes());
                }
            }
        }

        // ---- L2: SQLite / Postgres response cache ----
        if strategy.use_l2_sqlite {
            if let Some(ref rc) = self.response_cache {
                if let Ok(Some(cached)) = rc.get(key) {
                    profile.total_cache_hits += 1;
                    let total = profile.total_cache_hits + profile.total_cache_misses;
                    profile.cache_hit_rate = if total == 0 {
                        0.0
                    } else {
                        profile.total_cache_hits as f64 / total as f64
                    };
                    return Some(cached.response_text.into_bytes());
                }
            }
        }

        // ---- L3: Vector store ----
        if strategy.use_l3_vector {
            if let Some(ref vs) = self.vector_store {
                // Use a broad phase wildcard so we search all phases.
                if let Ok((hits, _)) = vs.search("*", key, 1, 0.0, 4096) {
                    if let Some(hit) = hits.into_iter().next() {
                        profile.total_cache_hits += 1;
                        let total = profile.total_cache_hits + profile.total_cache_misses;
                        profile.cache_hit_rate = if total == 0 {
                            0.0
                        } else {
                            profile.total_cache_hits as f64 / total as f64
                        };
                        return Some(hit.response_snippet.into_bytes());
                    }
                }
            }
        }

        // ---- Miss ----
        profile.total_cache_misses += 1;
        let total = profile.total_cache_hits + profile.total_cache_misses;
        profile.cache_hit_rate = if total == 0 {
            0.0
        } else {
            profile.total_cache_hits as f64 / total as f64
        };
        None
    }

    /// Store a value using the provided strategy.
    ///
    /// The value is placed into each cache tier that is both available and
    /// enabled by `strategy`.
    pub fn store(&self, key: &str, value: Vec<u8>, strategy: &CacheStrategy) {
        if strategy.ttl_seconds == 0 {
            return;
        }

        let value_str = String::from_utf8_lossy(&value).to_string();

        // ---- L1: In-memory response cache ----
        if strategy.use_l1_memory {
            if let Some(ref mrc) = self.memory_response_cache {
                let guard = mrc.lock().unwrap_or_else(|poisoned| {
                    tracing::warn!("memory_response_cache lock poisoned in store L1 – recovered");
                    poisoned.into_inner()
                });
                guard.put(key.to_string(), value_str.clone(), strategy.ttl_seconds);
            }
        }

        // ---- L2: SQLite / Postgres response cache ----
        if strategy.use_l2_sqlite {
            if let Some(ref rc) = self.response_cache {
                // Use a generic agent name; callers can refine when needed.
                let _ = rc.put(key, &value_str, "memory_bus", Some(strategy.ttl_seconds));
            }
        }

        // ---- L3: Vector store ----
        if strategy.use_l3_vector {
            if let Some(ref vs) = self.vector_store {
                // Store under a "memory_bus" phase with the key as query text.
                let _ = vs.upsert("memory_bus", key, &value_str);
            }
        }

        // ---- In-memory MemoryStore (if available) ----
        if let Some(ref ms) = self.memory_store {
            let mut guard = ms.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("memory_store lock poisoned, recovering");
                poisoned.into_inner()
            });
            let entry = crate::memory_module::MemoryEntry {
                id: key.to_string(),
                class: crate::memory_module::MemoryClass::Episodic,
                content: value_str,
                timestamp: now_iso8601(),
                usefulness: 1.0,
                staleness: 0,
                user_id: None,
            };
            guard.store(entry);
        }
    }

    /// Return a snapshot of the current bus profile / metrics.
    pub fn profile(&self) -> MemoryBusProfile {
        let p = self.profile.lock().unwrap_or_else(|e| e.into_inner());
        let mut snapshot = p.clone();

        // Enrich with live counts from available backends.
        if let Some(ref rc) = self.response_cache {
            if let Ok(stats) = rc.stats() {
                snapshot.total_cache_hits = stats.total_hits.max(snapshot.total_cache_hits);
            }
        }

        if let Some(ref ms) = self.memory_store {
            let guard = ms.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("memory_store lock poisoned, recovering");
                poisoned.into_inner()
            });
            // Approximate total entry count by summing across all classes.
            use crate::memory_module::MemoryClass;
            let mut total: u32 = 0;
            for class in &[
                MemoryClass::Observation,
                MemoryClass::Episodic,
                MemoryClass::Semantic,
                MemoryClass::ProjectState,
                MemoryClass::Transient,
            ] {
                total += guard.retrieve(class.clone(), usize::MAX).len() as u32;
            }
            snapshot.memory_entries = total;
        }

        if let Some(ref mrc) = self.memory_response_cache {
            let guard = match mrc.lock() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    tracing::warn!("[B48] mrc lock poisoned, recovering");
                    poisoned.into_inner()
                }
            };
            snapshot.vector_docs_count = guard.prune_and_count() as u32;
        }

        snapshot
    }

    /// Clear expired entries across all available cache backends.
    pub fn clear_expired(&self) {
        // L1: In-memory response cache — purge_expired does this inline.
        if let Some(ref mrc) = self.memory_response_cache {
            let guard = match mrc.lock() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    tracing::warn!("[B48] mrc lock poisoned, recovering");
                    poisoned.into_inner()
                }
            };
            guard.purge_expired();
        }

        // L2: SQLite / Postgres response cache.
        if let Some(ref rc) = self.response_cache {
            let _ = rc.purge_expired();
        }

        // MemoryStore: run garbage collection.
        if let Some(ref ms) = self.memory_store {
            let mut guard = ms.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("memory_store lock poisoned, recovering");
                poisoned.into_inner()
            });
            guard.gc();
        }

        // VectorStore entries are aged out by LRU via max_entries internally;
        // no explicit expiry purge is exposed, so we skip it here.
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Produce a basic ISO-8601 timestamp string for memory entries.
fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let total_secs = dur.as_secs();
    let secs_of_day = total_secs % 86400;
    let days = total_secs / 86400;

    // Compute year by subtracting days per year (accounting for leap years).
    let mut y = 1970i64;
    let mut remaining = days as i64;
    loop {
        let days_in_year = if (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0) {
            366
        } else {
            365
        };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        y += 1;
    }

    // Lookup month using proper month-day table.
    const MONTH_DAYS: [i64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let is_leap = (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0);
    let mut m = 0usize;
    while m < 12 {
        let days_in_month = if m == 1 && is_leap { 29 } else { MONTH_DAYS[m] };
        if remaining < days_in_month {
            break;
        }
        remaining -= days_in_month;
        m += 1;
    }
    let month = m as u32 + 1;
    let day = remaining as u32 + 1;

    let hours = (secs_of_day / 3600) as u32;
    let minutes = ((secs_of_day % 3600) / 60) as u32;
    let seconds = (secs_of_day % 60) as u32;

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y, month, day, hours, minutes, seconds
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory_module::{MemoryPolicy, MemoryStore};
    use crate::memory_response_cache::MemoryResponseCache;
    use std::sync::Mutex as StdMutex;

    fn make_test_bus() -> MemoryBus {
        // Create minimal backends for testing.
        let mrc = Arc::new(StdMutex::new(MemoryResponseCache::default()));
        let ms = Arc::new(Mutex::new(MemoryStore::new(MemoryPolicy::default())));

        // L2 and L3 are left as None to test L1-only path.
        MemoryBus::new(None, None, Some(ms), Some(mrc))
    }

    #[test]
    fn lookup_miss_returns_none() {
        let bus = make_test_bus();
        let strategy = CacheStrategy {
            use_l1_memory: true,
            use_l2_sqlite: false,
            use_l3_vector: false,
            ttl_seconds: 60,
        };
        let result = bus.lookup("nonexistent", &strategy);
        assert!(result.is_none());

        let p = bus.profile();
        assert_eq!(p.total_cache_hits, 0);
        assert_eq!(p.total_cache_misses, 1);
    }

    #[test]
    fn store_and_lookup_l1_roundtrip() {
        let bus = make_test_bus();
        let strategy = CacheStrategy {
            use_l1_memory: true,
            use_l2_sqlite: false,
            use_l3_vector: false,
            ttl_seconds: 60,
        };

        bus.store("k1", b"hello world".to_vec(), &strategy);
        let result = bus.lookup("k1", &strategy);
        assert!(result.is_some());
        assert_eq!(result.expect("lookup should return value"), b"hello world");

        let p = bus.profile();
        assert!(p.total_cache_hits >= 1);
    }

    #[test]
    fn strategy_disabled_l1_skips_cache() {
        let bus = make_test_bus();
        let store_strategy = CacheStrategy {
            use_l1_memory: true,
            use_l2_sqlite: false,
            use_l3_vector: false,
            ttl_seconds: 60,
        };
        bus.store("k2", b"data".to_vec(), &store_strategy);

        // Lookup with L1 disabled — should miss even though data exists.
        let lookup_strategy = CacheStrategy {
            use_l1_memory: false,
            use_l2_sqlite: false,
            use_l3_vector: false,
            ttl_seconds: 60,
        };
        let result = bus.lookup("k2", &lookup_strategy);
        assert!(result.is_none());
    }

    #[test]
    fn zero_ttl_skips_store() {
        let bus = make_test_bus();
        let strategy = CacheStrategy {
            use_l1_memory: true,
            use_l2_sqlite: false,
            use_l3_vector: false,
            ttl_seconds: 0,
        };

        bus.store("k_zero", b"should not appear".to_vec(), &strategy);
        let result = bus.lookup("k_zero", &strategy);
        assert!(result.is_none());
    }

    #[test]
    fn clear_expired_does_not_panic() {
        let bus = make_test_bus();
        // Just verify that calling clear_expired on a minimal bus doesn't panic.
        bus.clear_expired();
    }
}
