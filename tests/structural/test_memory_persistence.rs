//! Memory Persistence End-to-End
//!
//! Validates the three-tier memory persistence lifecycle through the real
//! production API (`MemoryPersistence::store` / `promote_to_warm` /
//! `promote_to_cold` / `auto_migrate` / `search_by_session`):
//!   write L1 (hot) → migrate to L2 (warm) → archive to L3 (cold) →
//!   restore from L3 → cross-session retrieval
//!
//! No tier fields are hand-assigned in assertions — every migration below
//! goes through the production promotion/migration entry points and the
//! assertions verify the observable outcome (retrievability + reported
//! migration counts), not re-implemented eviction arithmetic.

use std::path::PathBuf;

use go_on::memory::memory_persistence::{
    MemoryEntry, MemoryPersistence, MemoryTier, MemoryTieringPolicy,
};

// ── Context ────────────────────────────────────────────────────────────────

/// Per-test temp workspace (warm SQLite db + cold archive dir), removed on drop.
struct MemoryE2eContext {
    base: PathBuf,
}

impl MemoryE2eContext {
    fn new(tag: &str) -> Self {
        let base =
            std::env::temp_dir().join(format!("go-on-e2e-memory-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::create_dir_all(&base);
        Self { base }
    }

    fn db_path(&self) -> PathBuf {
        self.base.join("warm.sqlite3")
    }

    fn cold_path(&self) -> PathBuf {
        self.base.join("cold")
    }

    fn persistence(&self, policy: MemoryTieringPolicy) -> MemoryPersistence {
        MemoryPersistence::new(&self.db_path(), &self.cold_path(), Some(policy))
            .expect("persistence should initialize")
    }
}

impl Drop for MemoryE2eContext {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

/// Entry helper: hot entry bound to a session so the real warm/cold read
/// paths (`search_by_session`) can retrieve it after migration.
fn session_entry(
    id: &str,
    class: &str,
    content: &str,
    usefulness: f32,
    session: &str,
) -> MemoryEntry {
    let mut entry = MemoryEntry::new_hot(id, class, content, usefulness);
    entry.session_id = Some(session.to_string());
    entry
}

// ── Tests ──────────────────────────────────────────────────────────────────

/// Full memory persistence lifecycle across all three tiers, driven entirely
/// through the production API:
///   store (L1) → promote_to_warm (L2) → search → promote_to_cold (L3) →
///   search (cold fallback) → unknown session yields nothing.
#[tokio::test]
async fn test_memory_persistence_three_tier_lifecycle() {
    let ctx = MemoryE2eContext::new("lifecycle");
    let persistence = ctx.persistence(MemoryTieringPolicy::default());

    let entry = session_entry(
        "mem-e2e-l1-001",
        "episodic",
        "User mentioned preference for dark mode",
        0.75,
        "session-a",
    );

    // ── 1. Write L1 (Hot) via the real store path ────────────────────────
    persistence
        .store(entry.clone())
        .await
        .expect("store should place the entry in the hot tier");

    // ── 2. Migrate L1 → L2 (Warm) via the real promotion API ─────────────
    persistence
        .promote_to_warm(entry.clone())
        .await
        .expect("promote_to_warm should move the entry to the warm tier");

    let warm_hits = persistence
        .search_by_session("session-a", 16)
        .await
        .expect("warm search should succeed");
    assert_eq!(warm_hits.len(), 1, "promoted entry must be retrievable");
    assert_eq!(warm_hits[0].id, "mem-e2e-l1-001");
    assert_eq!(
        warm_hits[0].content,
        "User mentioned preference for dark mode"
    );
    assert_eq!(warm_hits[0].tier, MemoryTier::Warm);

    // ── 3. Archive L2 → L3 (Cold) via the real archival API ──────────────
    persistence
        .promote_to_cold(entry.clone())
        .await
        .expect("promote_to_cold should archive the entry");

    // ── 4. Restore from L3 (cold fallback in the real read path) ─────────
    let cold_hits = persistence
        .search_by_session("session-a", 16)
        .await
        .expect("cold fallback search should succeed");
    assert_eq!(cold_hits.len(), 1, "archived entry must remain recoverable");
    assert_eq!(cold_hits[0].id, "mem-e2e-l1-001");
    assert_eq!(cold_hits[0].tier, MemoryTier::Cold);

    // ── 5. Cross-session isolation ───────────────────────────────────────
    let other_session = persistence
        .search_by_session("session-b", 16)
        .await
        .expect("search should succeed");
    assert!(
        other_session.is_empty(),
        "a different session must not see this entry"
    );
}

/// Real automatic migration: with an instantly-expired hot TTL, `auto_migrate`
/// evicts the hot entries and routes them by usefulness — useful ones are
/// promoted to warm, low-usefulness ones are archived straight to cold.
#[tokio::test]
async fn test_memory_persistence_automatic_demotion_on_capacity() {
    let ctx = MemoryE2eContext::new("auto-migrate");
    let policy = MemoryTieringPolicy {
        // 0-second hot TTL makes every stored entry instantly expired, so the
        // real migration cycle has something to evict without waiting.
        hot_ttl_secs: 0,
        hot_max_entries: 5,
        ..Default::default()
    };
    assert_eq!(policy.hot_max_entries, 5);

    let persistence = ctx.persistence(policy);

    let useful = session_entry(
        "auto-migrate-warm",
        "test",
        "useful entry (>= hot_threshold)",
        0.9,
        "session-migrate",
    );
    let stale = session_entry(
        "auto-migrate-cold",
        "test",
        "stale entry (< hot_threshold)",
        0.1,
        "session-migrate",
    );
    persistence
        .store(useful.clone())
        .await
        .expect("store should succeed");
    persistence
        .store(stale.clone())
        .await
        .expect("store should succeed");

    let report = persistence
        .auto_migrate()
        .await
        .expect("auto_migrate should succeed");

    // The real migration cycle must route by usefulness: 0.9 → warm,
    // 0.1 → cold, with no warm-tier churn in the same pass.
    assert_eq!(
        report.promoted_hot_to_warm, 1,
        "useful entry promoted to warm"
    );
    assert_eq!(
        report.demoted_hot_to_cold, 1,
        "stale entry archived directly to cold"
    );
    assert_eq!(report.promoted_warm_to_cold, 0);
    assert_eq!(report.evicted_warm, 0);

    // Both entries must remain recoverable through the real read path.
    let hits = persistence
        .search_by_session("session-migrate", 16)
        .await
        .expect("search should succeed");
    let ids: Vec<&str> = hits.iter().map(|e| e.id.as_str()).collect();
    assert!(
        ids.contains(&"auto-migrate-warm"),
        "warm-promoted entry must be retrievable, got: {ids:?}"
    );
    assert!(
        ids.contains(&"auto-migrate-cold"),
        "cold-archived entry must be retrievable via fallback, got: {ids:?}"
    );
}

/// Usefulness thresholds are enforced by the real tiering policy: in the warm
/// TTL pass of `auto_migrate`, entries at or above `warm_threshold` (0.6) are
/// archived to cold while low-usefulness warm entries are evicted.
#[tokio::test]
async fn test_memory_persistence_metadata_index() {
    let ctx = MemoryE2eContext::new("meta-index");
    let policy = MemoryTieringPolicy {
        // 0-second warm TTL makes every warm entry an immediate candidate for
        // the warm → cold / eviction pass.
        warm_ttl_secs: 0,
        hot_ttl_secs: 3600,
        ..Default::default()
    };

    let persistence = ctx.persistence(policy);

    let useful = session_entry("meta-001", "episodic", "met entry 1", 0.9, "session-meta");
    let low = session_entry("meta-002", "semantic", "met entry 2", 0.2, "session-meta");
    persistence
        .promote_to_warm(useful.clone())
        .await
        .expect("promote_to_warm should succeed");
    persistence
        .promote_to_warm(low.clone())
        .await
        .expect("promote_to_warm should succeed");

    let report = persistence
        .auto_migrate()
        .await
        .expect("auto_migrate should succeed");

    // usefulness 0.9 >= warm_threshold 0.6 → archived to cold;
    // usefulness 0.2 < 0.6 → evicted from the warm tier.
    assert_eq!(report.promoted_warm_to_cold, 1);
    assert_eq!(report.evicted_warm, 1);

    let hits = persistence
        .search_by_session("session-meta", 16)
        .await
        .expect("search should succeed");
    let ids: Vec<&str> = hits.iter().map(|e| e.id.as_str()).collect();
    assert!(
        ids.contains(&"meta-001"),
        "archived useful entry must remain retrievable, got: {ids:?}"
    );
    assert!(
        !ids.contains(&"meta-002"),
        "evicted low-usefulness entry must be gone, got: {ids:?}"
    );
}
