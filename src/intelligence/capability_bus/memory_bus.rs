//! MemoryBus — Unified cache coordination sub-bus (BLUE38 ARCH-13)
//!
//! MemoryBus aggregates cache/memory backends (L1 in-memory response cache,
//! L2 SQLite/Postgres response cache, L3 vector store, in-memory MemoryStore)
//! for observability. The former cascading `lookup`/`store` hot path was
//! removed: production never called it (the chat hot path talks to
//! `TokenMultiLevelCache` + `SemanticResponseCache` directly), so the bus now
//! only wires backends and reports a live profile snapshot.

use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex, Mutex as StdMutex};

use crate::cache::ResponseCache;
use crate::memory::semantic_cache::SemanticResponseCache;
use crate::memory_module::{MemoryClass, MemoryPolicy, MemoryStore};
use crate::vector::VectorStore;

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
/// scheduler and the cache/memory backends.
///
/// Each backend is `Option`-wrapped. When `None`, the corresponding tier is
/// skipped in the reported profile. This keeps the MemoryBus generic and avoids
/// coupling to concrete backend lifetimes.
pub struct MemoryBus {
    /// Response cache reference (L2 — SQLite or Postgres).
    response_cache: Option<Arc<ResponseCache>>,
    /// Vector store reference (L3).
    vector_store: Option<Arc<VectorStore>>,
    /// In-memory memory store (wrapped in StdMutex for sync access).
    memory_store: Option<Arc<StdMutex<MemoryStore>>>,
    /// In-memory response cache (L1 — semantic cache configured for exact
    /// matching).  Internally thread-safe via its own `RwLock`.
    memory_response_cache: Option<Arc<std::sync::RwLock<SemanticResponseCache>>>,
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
        memory_response_cache: Option<Arc<std::sync::RwLock<SemanticResponseCache>>>,
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
            self.memory_response_cache = Some(Arc::new(std::sync::RwLock::new(
                SemanticResponseCache::new(Default::default()),
            )));
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
        memory_response_cache: Option<Option<Arc<std::sync::RwLock<SemanticResponseCache>>>>,
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

    /// Return a snapshot of the current bus profile / metrics.
    pub async fn profile(&self) -> MemoryBusProfile {
        let mut snapshot = {
            let p = self.profile.lock().unwrap_or_else(|e| e.into_inner());
            p.clone()
        };

        // Enrich with live counts from available backends.
        if let Some(ref rc) = self.response_cache {
            if let Ok(stats) = rc.stats().await {
                snapshot.total_cache_hits = stats.total_hits.max(snapshot.total_cache_hits);
            }
        }

        if let Some(ref vs) = self.vector_store {
            if let Ok(count) = vs.memory_entry_count().await {
                snapshot.vector_docs_count = count as u32;
            }
        }

        if let Some(ref ms) = self.memory_store {
            let guard = ms.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("memory_store lock poisoned, recovering");
                poisoned.into_inner()
            });
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
            if let Ok(guard) = mrc.read() {
                let stats = guard.stats();
                snapshot.total_cache_hits += stats.total_hits;
                snapshot.total_cache_misses = stats.total_misses;
                snapshot.cache_hit_rate = stats.hit_ratio;
            }
        }

        snapshot
    }
}
