//! MemoryStore ↔ MemoryPersistence bridge
//!
//! Coordinates operations between the two parallel memory subsystems:
//!
//! - [`MemoryStore`](crate::memory::memory::MemoryStore) — in-memory per-class memory store
//!   with promotion between `MemoryClass` levels (Observation → Episodic → Semantic → ProjectState).
//! - [`MemoryPersistence`](crate::memory::memory_persistence::MemoryPersistence) — three-tier
//!   persistence (Hot / Warm / Cold) with automatic tier migration.
//!
//! # Bridge functions
//!
//! | Operation | MemoryStore | MemoryPersistence |
//! |---|---|---|
//! | `bridge_store` | `store()` | `store()` |
//! | `bridge_promote` | `promote()` | `auto_migrate()` |
//!
//! # Background auto-migration
//!
//! [`start_auto_migrate_task`] spawns a tokio task that calls
//! `MemoryPersistence::auto_migrate()` every 5 minutes.

use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::memory::memory::{MemoryEntry as CanonicalEntry, MemoryPromotionReport, MemoryStore};
use crate::memory::memory_persistence::{
    MemoryEntry as PersistenceEntry, MemoryPersistence, MemoryTier,
};

// ── From impl: CanonicalEntry → PersistenceEntry ──────────────────────────

impl From<CanonicalEntry> for PersistenceEntry {
    fn from(entry: CanonicalEntry) -> Self {
        Self {
            id: entry.id,
            tier: MemoryTier::Hot,
            class: format!("{:?}", entry.class),
            content: entry.content,
            created_at: entry.timestamp.parse::<i64>().unwrap_or_else(|_| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0)
            }),
            accessed_at: 0,
            usefulness: entry.usefulness,
            embedding: None,
            access_count: 1,
            session_id: None,
        }
    }
}

// ── Background auto-migrate ──────────────────────────────────────────────

/// Start a background task that periodically calls `auto_migrate()` on the
/// persistence layer every 5 minutes.
///
/// The task can be cancelled via the optional `CancellationToken`.  Logs a
/// debug message with the migration report on each cycle.
pub fn start_auto_migrate_task(
    memory_persistence: Arc<MemoryPersistence>,
    cancel: Option<CancellationToken>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(300)); // 5 minutes
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        // Optional initial delay so the system stabilises before first migration
        interval.tick().await;

        loop {
            tokio::select! {
                _ = async {
                    if let Some(ref cancel) = cancel {
                        if cancel.is_cancelled() {
                            return;
                        }
                    }
                    // Check cancellation via a small yield instead of busy-loop
                    tokio::time::sleep(Duration::from_millis(0)).await;
                } => {}
                _ = interval.tick() => {}
            }

            // Check cancellation token (if provided)
            if let Some(ref cancel) = cancel {
                if cancel.is_cancelled() {
                    tracing::info!("auto_migrate task cancelled");
                    break;
                }
            }

            match memory_persistence.auto_migrate() {
                Ok(report) => {
                    let total = report.promoted_hot_to_warm
                        + report.promoted_warm_to_cold
                        + report.demoted_hot_to_cold
                        + report.evicted_warm;
                    if total > 0 {
                        tracing::debug!(
                            target = "memory_bridge",
                            promoted_hot_to_warm = report.promoted_hot_to_warm,
                            promoted_warm_to_cold = report.promoted_warm_to_cold,
                            demoted_hot_to_cold = report.demoted_hot_to_cold,
                            evicted_warm = report.evicted_warm,
                            "memory_persistence auto_migrate cycle complete"
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        target = "memory_bridge",
                        "memory_persistence auto_migrate failed: {e}"
                    );
                }
            }
        }
    })
}

// ── Coordinated bridge operations ────────────────────────────────────────

/// Convert a canonical [`MemoryEntry`](CanonicalEntry) into a persistence
/// [`MemoryEntry`](PersistenceEntry) and call `MemoryPersistence::store()`.
///
/// Returns the persistence operation result.
///
/// Reserved bridge API — prefixed with `_` to suppress dead-code warnings
/// until it is wired into production flow.
pub fn _persist_store(
    persistence: &MemoryPersistence,
    entry: CanonicalEntry,
) -> anyhow::Result<()> {
    let p_entry: PersistenceEntry = entry.into();
    persistence.store(p_entry)
}

/// Bridge for `store()` — persists the entry in both subsystems.
///
/// 1. Stores the entry in the in-memory [`MemoryStore`].
/// 2. Converts and stores the entry in [`MemoryPersistence`].
///
/// # Errors
///
/// Returns an error if the persistence `store()` call fails.  The entry
/// will still have been added to the in-memory store.
///
/// Reserved bridge API — prefixed with `_` to suppress dead-code warnings
/// until it is wired into production flow.
pub fn _bridge_store(
    memory_store: &StdMutex<MemoryStore>,
    persistence: &MemoryPersistence,
    entry: CanonicalEntry,
) -> anyhow::Result<()> {
    // Step 1: in-memory store
    let mut store = memory_store.lock().unwrap_or_else(|poisoned| {
        tracing::warn!(
            target = "memory_bridge",
            "MemoryStore mutex poisoned, recovering"
        );
        poisoned.into_inner()
    });
    store.store(entry.clone());

    // Step 2: persistence (conversion via `From` impl)
    _persist_store(persistence, entry)?;

    Ok(())
}

/// Bridge for `promote()` — promotes in-memory entries and triggers tier migration.
///
/// 1. Runs `MemoryStore::promote()` to move entries between memory classes.
/// 2. Triggers `MemoryPersistence::auto_migrate()` to move entries between tiers.
///
/// Returns the [`MemoryPromotionReport`] from the in-memory promotion.
///
/// # Errors
///
/// Returns an error if `auto_migrate()` fails.  The in-memory promotion will
/// still have been applied.
pub fn bridge_promote(
    memory_store: &StdMutex<MemoryStore>,
    persistence: &MemoryPersistence,
) -> anyhow::Result<MemoryPromotionReport> {
    // Step 1: promote in-memory classes
    let report = {
        let mut store = memory_store.lock().unwrap_or_else(|poisoned| {
            tracing::warn!(
                target = "memory_bridge",
                "MemoryStore mutex poisoned, recovering"
            );
            poisoned.into_inner()
        });
        store.promote()
    };

    // Step 2: trigger persistence tier migration
    let _migration_report = persistence.auto_migrate()?;

    Ok(report)
}

// ── Memory base path ─────────────────────────────────────────────────────

/// Return the memory base directory, sourced from the `GO_ON_MEMORY_PATH`
/// environment variable, falling back to `.goon/memory/`.
pub fn memory_base_path() -> std::path::PathBuf {
    let base = std::env::var("GO_ON_MEMORY_PATH").unwrap_or_else(|_| ".goon/memory/".to_string());
    std::path::PathBuf::from(base)
}

// ── Convenience initialiser ──────────────────────────────────────────────

/// Create a [`MemoryPersistence`] with the default paths and wire up the
/// background auto-migrate task.
///
/// Call this from `start_server()` to fulfil the GAP-B54-011 wiring
/// requirement.  Returns the persistence handle so it can be reused
/// elsewhere.
pub fn init_memory_persistence_with_auto_migrate(
    cancel: Option<CancellationToken>,
) -> Option<Arc<MemoryPersistence>> {
    let base = memory_base_path();
    let db_path = base.join("warm.db");
    let cold_path = base.join("cold");

    match MemoryPersistence::new(&db_path, &cold_path, None) {
        Ok(mp) => {
            let mp = Arc::new(mp);
            let task = start_auto_migrate_task(Arc::clone(&mp), cancel);
            // Detach the handle — the task runs for the process lifetime.
            #[allow(clippy::let_underscore_future)]
            let _ = task;
            Some(mp)
        }
        Err(e) => {
            tracing::warn!(
                target = "memory_bridge",
                "failed to create MemoryPersistence for auto-migrate task: {e}"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::memory::{MemoryClass, MemoryEntry, MemoryPolicy};

    fn make_canonical(id: &str, class: MemoryClass, usefulness: f32) -> CanonicalEntry {
        MemoryEntry {
            id: id.to_string(),
            class,
            content: format!("content-{id}"),
            timestamp: String::new(),
            usefulness,
            staleness: 0,
        }
    }

    #[test]
    fn test_bridge_store_and_promote() {
        let store = StdMutex::new(MemoryStore::new(MemoryPolicy::default()));
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("warm.db");
        let cold_path = tmp.path().join("cold");
        let persistence = MemoryPersistence::new(&db_path, &cold_path, None).unwrap();

        // Store an entry via the bridge
        let entry = make_canonical("bridge-test-1", MemoryClass::Observation, 0.80);
        _bridge_store(&store, &persistence, entry).unwrap();

        // Promote via the bridge
        let report = bridge_promote(&store, &persistence).unwrap();
        // The entry with usefulness 0.80 from Observation should promote to Episodic
        assert_eq!(
            report.promoted_count, 1,
            "expected 1 promotion (Observation→Episodic)"
        );
    }

    #[test]
    fn test_background_task_cancellation() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("warm.db");
        let cold_path = tmp.path().join("cold");
        let persistence = Arc::new(MemoryPersistence::new(&db_path, &cold_path, None).unwrap());

        let cancel = CancellationToken::new();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let mut handle: Option<tokio::task::JoinHandle<()>> = None;
        rt.block_on(async {
            let h = start_auto_migrate_task(Arc::clone(&persistence), Some(cancel.clone()));
            cancel.cancel();
            tokio::time::sleep(Duration::from_millis(50)).await;
            handle = Some(h);
        });
        let handle = handle.expect("handle should be set");
        assert!(
            handle.is_finished(),
            "task should finish promptly after cancellation"
        );
    }
}
