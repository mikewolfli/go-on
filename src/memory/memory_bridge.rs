//! MemoryStore ↔ MemoryPersistence bridge
//!
//! Coordinates operations between the two parallel memory subsystems:
//!
//! - [`MemoryStore`] — in-memory per-class memory store
//!   with promotion between `MemoryClass` levels (Observation → Episodic → Semantic → ProjectState).
//! - [`MemoryPersistence`] — three-tier
//!   persistence (Hot / Warm / Cold) with automatic tier migration.
//!
//! # Bridge functions
//!
//! | Operation | MemoryStore | MemoryPersistence |
//! |---|---|---|
//! | `bridge_store` | `store()` | `store()` |
//! | `bridge_promote` | `promote()` | — (tier migration runs in the 5-minute
//!   background task; see below) |
//!
//! # Background auto-migration
//!
//! The auto-migrate background task is now spawned inside
//! `start_background_tasks()` (src/acp/background.rs) using the server's
//! existing `MemoryPersistence`, rather than creating a redundant instance
//! during server startup. It is the **only** full-table tier-migration scan:
//! `bridge_promote` only promotes in-memory classes and never scans the warm
//! table, keeping "new memories stay hot until TTL expiry migrates them"
//! semantics.

use std::sync::Mutex;

use crate::memory::memory::{MemoryEntry as CanonicalEntry, MemoryPromotionReport, MemoryStore};
use crate::memory::memory_persistence::{
    MemoryEntry as PersistenceEntry, MemoryPersistence, MemoryTier,
};

// ── From impl: CanonicalEntry → PersistenceEntry ──────────────────────────

impl From<CanonicalEntry> for PersistenceEntry {
    fn from(entry: CanonicalEntry) -> Self {
        // M8: Parse timestamp string -> i64 with multiple fallback strategies.
        //     1. Try i64 (whole seconds / milliseconds).
        //     2. Try f64 (float seconds, e.g. 1700000000.123).
        //     3. Fall back to current wall-clock time.
        let created_at = entry
            .timestamp
            .parse::<i64>()
            .or_else(|_| entry.timestamp.parse::<f64>().map(|t| t as i64))
            .unwrap_or_else(|_| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0)
            });

        Self {
            id: entry.id,
            tier: MemoryTier::Hot,
            class: format!("{:?}", entry.class),
            content: entry.content,
            created_at,
            accessed_at: 0,
            usefulness: entry.usefulness,
            embedding: None,
            access_count: 1,
            session_id: entry.session_id,
            user_id: entry.user_id,
        }
    }
}

// ── Bridge operations ────────────────────────────────────────

// ── Coordinated bridge operations ────────────────────────────────────────

/// Convert a canonical [`MemoryEntry`](CanonicalEntry) into a persistence
/// [`MemoryEntry`](PersistenceEntry) and call `MemoryPersistence::store()`.
///
/// Bridge API for persistence-only store (wired into production flow).
pub async fn persist_store(
    persistence: &MemoryPersistence,
    entry: CanonicalEntry,
) -> anyhow::Result<()> {
    let p_entry: PersistenceEntry = entry.into();
    persistence.store(p_entry).await
}

/// Bridge for `store()` — persists the entry in both subsystems.
///
/// 1. Stores the entry in the in-memory [`MemoryStore`].
/// 2. Converts and stores the entry in [`MemoryPersistence`].
///
/// # Errors
///
/// Bridge API for coordinated dual-store (memory + persistence, wired into production flow).
pub async fn bridge_store(
    memory_store: &Mutex<MemoryStore>,
    persistence: &MemoryPersistence,
    entry: CanonicalEntry,
) -> anyhow::Result<()> {
    // Step 1: in-memory store
    {
        let mut store = memory_store.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("memory bridge mutex poisoned during store");
            poisoned.into_inner()
        });
        store.store(entry.clone());
    }

    // Step 2: persistence (conversion via `From` impl)
    persist_store(persistence, entry).await?;

    Ok(())
}

/// Bridge for `promote()` — promotes in-memory memory-class levels only.
///
/// Runs `MemoryStore::promote()` to move entries between memory classes
/// (Observation → Episodic → Semantic → ProjectState).
///
/// Persistence tier migration (hot → warm / warm → cold) is deliberately NOT
/// triggered here: it is a full-table scan, and the single 5-minute background
/// task (`memory_auto_migrate` in src/acp/background.rs) is the sole owner of
/// that scan. New memories stay in the hot tier until their TTL expires.
///
/// Returns the [`MemoryPromotionReport`] from the in-memory promotion.
pub async fn bridge_promote(
    memory_store: &Mutex<MemoryStore>,
) -> anyhow::Result<MemoryPromotionReport> {
    // Promote in-memory classes
    let report = {
        let mut store = memory_store.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("memory bridge mutex poisoned during promote");
            poisoned.into_inner()
        });
        store.promote()
    };

    Ok(report)
}

// ── Memory base path ─────────────────────────────────────────────────────

/// Return the memory base directory: the `GO_ON_MEMORY_PATH` environment
/// override if set, otherwise the canonical go-on data dir joined with
/// `memory` (see `crate::shared::goon_paths`).
pub fn memory_base_path() -> std::path::PathBuf {
    match std::env::var("GO_ON_MEMORY_PATH") {
        Ok(override_path) => std::path::PathBuf::from(override_path),
        Err(_) => crate::shared::goon_paths::goon_subdir("memory"),
    }
}

// ── Convenience initialiser ──────────────────────────────────────────────

// PERF-FIX: init_memory_persistence_with_auto_migrate() removed.
// This function created a redundant third MemoryPersistence instance (third
// SQLite connection + filesystem ops) synchronously during the critical startup
// path in new_acp_server().  The auto-migrate background task now runs in
// start_background_tasks() (src/acp/background.rs) using the server's existing
// MemoryPersistence, which already owns the SQLite warm store and cold storage.
// This eliminates ~33% of SQLite init overhead from the startup critical path.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::memory::{MemoryClass, MemoryEntry, MemoryPolicy};
    use std::sync::Arc;
    use std::time::Duration;
    use tempfile::TempDir;
    use tokio_util::sync::CancellationToken;

    fn make_canonical(id: &str, class: MemoryClass, usefulness: f32) -> CanonicalEntry {
        MemoryEntry {
            id: id.to_string(),
            class,
            content: format!("content-{id}"),
            timestamp: String::new(),
            usefulness,
            staleness: 0,
            user_id: None,
            session_id: None,
        }
    }

    #[tokio::test]
    async fn test_bridge_store_and_promote() {
        let store = Mutex::new(MemoryStore::new(MemoryPolicy::default()));
        let tmp = tempfile::tempdir().expect("create temp dir");
        let db_path = tmp.path().join("warm.db");
        let cold_path = tmp.path().join("cold");
        let persistence =
            MemoryPersistence::new(&db_path, &cold_path, None).expect("create MemoryPersistence");

        // Store an entry via the bridge
        let entry = make_canonical("bridge-test-1", MemoryClass::Observation, 0.80);
        bridge_store(&store, &persistence, entry)
            .await
            .expect("bridge_store should succeed");

        // Promote via the bridge (in-memory class promotion only; tier
        // migration is owned by the background auto-migrate task)
        let report = bridge_promote(&store)
            .await
            .expect("bridge_promote should succeed");
        // The entry with usefulness 0.80 from Observation should promote to Episodic
        assert_eq!(
            report.promoted_count, 1,
            "expected 1 promotion (Observation→Episodic)"
        );
    }

    #[tokio::test]
    async fn test_background_task_cancellation() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let cold_path = tmp.path().join("cold");
        let persistence = Arc::new(
            MemoryPersistence::new(&db_path, &cold_path, None).expect("create MemoryPersistence"),
        );

        let cancel = CancellationToken::new();
        let mp = Arc::clone(&persistence);
        let cancel_clone = cancel.clone();
        let handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(300));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // First tick completes immediately per tokio docs
            interval.tick().await;
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        if cancel_clone.is_cancelled() {
                            tracing::info!("auto_migrate task cancelled");
                            break;
                        }
                        let _ = mp.auto_migrate().await;
                    }
                    _ = cancel_clone.cancelled() => {
                        tracing::info!("auto_migrate task cancelled via token");
                        break;
                    }
                }
            }
        });
        cancel.cancel();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            handle.is_finished(),
            "task should finish promptly after cancellation"
        );
    }
}
