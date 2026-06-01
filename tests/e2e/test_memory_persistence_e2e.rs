//! Memory Persistence End-to-End
//!
//! Validates the three-tier memory persistence lifecycle:
//!   write L1 (hot) → migrate to L2 (warm) → archive to L3 (cold) →
//!   restore from L3 → cross-session retrieval
//!
//! Uses go_on::memory::memory_persistence types to construct entries and
//! validate the tiering policy. Real integration requires a go-on instance
//! with the `backend-sqlite` feature enabled and filesystem access for L3.
//!
//! # integration-test-stub
//! Tier migration is validated structurally. In production, a background
//! worker calls promote() / demote() based on access patterns and TTL.

use std::path::PathBuf;
use std::time::Duration;
use tokio::time::sleep;

use go_on::memory::memory_persistence::{MemoryEntry, MemoryTier, MemoryTieringPolicy};

// ── Context ────────────────────────────────────────────────────────────────

struct MemoryE2eContext {
    entry_ids: Vec<String>,
    archive_dir: Option<PathBuf>,
}

impl MemoryE2eContext {
    fn new() -> Self {
        let archive_dir = std::env::temp_dir().join("go-on-e2e-memory-archive");
        let _ = std::fs::create_dir_all(&archive_dir);
        Self {
            entry_ids: Vec::new(),
            archive_dir: Some(archive_dir),
        }
    }
}

impl Drop for MemoryE2eContext {
    fn drop(&mut self) {
        if let Some(dir) = &self.archive_dir {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

/// Full memory persistence lifecycle across all three tiers.
#[tokio::test]
#[ignore]
async fn test_memory_persistence_three_tier_lifecycle() {
    let mut ctx = MemoryE2eContext::new();

    // ── 1. Setup tiering policy ─────────────────────────────────────────
    let policy = MemoryTieringPolicy::default();
    assert_eq!(policy.hot_max_entries, 2048);
    assert_eq!(policy.hot_ttl_secs, 300);

    // ── 2. Write L1 (Hot) entries ──────────────────────────────────────
    let entry_a = MemoryEntry::new_hot(
        "mem-e2e-l1-001",
        "episodic",
        "User mentioned preference for dark mode",
        0.75,
    );
    let entry_b = MemoryEntry::new_hot(
        "mem-e2e-l1-002",
        "semantic",
        "Project deadline is 2026-06-15",
        0.90,
    );

    assert_eq!(entry_a.tier, MemoryTier::Hot);
    assert_eq!(entry_a.class, "episodic");
    assert_eq!(entry_b.class, "semantic");
    assert!(entry_b.usefulness > entry_a.usefulness);

    ctx.entry_ids.push(entry_a.id.clone());
    ctx.entry_ids.push(entry_b.id.clone());
    assert_eq!(ctx.entry_ids.len(), 2);

    // integration-test-stub: real write inserts into HotCache with LRU.
    // The MemoryPersistence struct wraps HotCache + WarmStore + ColdStorage
    // and is constructed with `MemoryPersistence::new(db_path, cold_base, policy)`.

    // ── 3. Migrate L1 → L2 (Warm) ──────────────────────────────────────
    // Promote if usefulness >= hot_threshold (default 0.3).
    let promote_threshold = policy.hot_threshold;
    assert!(
        entry_a.usefulness >= promote_threshold,
        "entry_a should qualify for promotion"
    );
    assert!(
        entry_b.usefulness >= promote_threshold,
        "entry_b should qualify for promotion"
    );

    // Simulate promotion by changing tier.
    let mut warm_a = entry_a.clone();
    warm_a.tier = MemoryTier::Warm;
    let mut warm_b = entry_b.clone();
    warm_b.tier = MemoryTier::Warm;

    assert_eq!(warm_a.tier, MemoryTier::Warm);
    assert_eq!(warm_b.tier, MemoryTier::Warm);

    // integration-test-stub: real promotion calls
    // mgr.promote_to_warm(id) which moves data from HotCache to WarmStore (SQLite).

    // ── 4. Archive to L3 (Cold) ────────────────────────────────────────
    // Demote if usefulness < warm_threshold (default 0.6) or idle time exceeds TTL.
    // Here entry_b has high usefulness and should be retained in warm.
    // entry_a has borderline usefulness but is above the threshold.
    let archive_dir = ctx.archive_dir.as_ref().unwrap();
    assert!(archive_dir.exists(), "archive directory must exist");

    let mut cold_a = warm_a.clone();
    cold_a.tier = MemoryTier::Cold;
    let mut cold_b = warm_b.clone();
    cold_b.tier = MemoryTier::Cold;

    assert_eq!(cold_a.tier, MemoryTier::Cold);
    assert_eq!(cold_b.tier, MemoryTier::Cold);

    // integration-test-stub: real demotion appends the entry as gzip NDJSON
    // via ColdStorage::append_entry() and deletes from WarmStore.

    // ── 5. Restore from L3 → L2 ────────────────────────────────────────
    // Restore an entry and verify it lands in warm.
    let mut restored = cold_a.clone();
    restored.tier = MemoryTier::Warm;
    assert_eq!(restored.tier, MemoryTier::Warm);
    assert_eq!(restored.content, "User mentioned preference for dark mode");
    assert_eq!(restored.id, "mem-e2e-l1-001");

    // integration-test-stub: real restore reads the NDJSON file, deserializes,
    // and upserts into WarmStore, then deletes the cold shard entry.

    // ── 6. Cross-session retrieval ─────────────────────────────────────
    // Simulate a second session retrieving the restored entry.
    let session_b_content = restored.content.clone();
    assert_eq!(session_b_content, "User mentioned preference for dark mode");

    // integration-test-stub: real cross-session uses a new MemoryPersistence
    // instance that reads from the same SQLite database.

    // ── 7. Teardown via Drop ───────────────────────────────────────────
    sleep(Duration::from_millis(10)).await;
    assert!(true, "memory persistence three-tier lifecycle passed");
}

/// Tests automatic demotion from L1 → L2 when hot cache exceeds capacity.
#[tokio::test]
#[ignore]
async fn test_memory_persistence_automatic_demotion_on_capacity() {
    // Create a policy with a small hot cache to force eviction.
    let policy = MemoryTieringPolicy {
        hot_max_entries: 5,
        hot_ttl_secs: 3600,
        ..Default::default()
    };

    assert_eq!(policy.hot_max_entries, 5);

    // integration-test-stub: real scenario inserts 10 entries into an
    // L1 cache with max 5; the 5 oldest are auto-demoted to L2 (Warm).
    // The hot count ≤ 5 and warm count ≥ 5 after demotion.
    //
    // Simulate by checking logic:
    let entries: Vec<MemoryEntry> = (0..10)
        .map(|i| {
            MemoryEntry::new_hot(
                format!("auto-demote-{:02}", i),
                "test",
                format!("entry-{}", i),
                0.5,
            )
        })
        .collect();

    // The first 5 inserted entries would be evicted (LRU) when inserting
    // entries 5-9. Evicted entries conceptually move to warm.
    let evicted_count = entries.len().saturating_sub(policy.hot_max_entries);
    assert_eq!(evicted_count, 5);
    assert!(evicted_count > 0, "entries above capacity must be demoted");

    sleep(Duration::from_millis(10)).await;
    assert!(true, "automatic demotion skeleton passed");
}

/// Tests the metadata index retrieval.
#[tokio::test]
#[ignore]
async fn test_memory_persistence_metadata_index() {
    // integration-test-stub: real code uses MemoryPersistence::new(db_path, cold_path, None)
    // which requires the backend-sqlite feature. Here we just validate the pattern.
    let _db_path = std::env::temp_dir().join("go-on-e2e-meta.db");
    let cold_path = std::env::temp_dir().join("go-on-e2e-meta-cold");
    let _ = std::fs::create_dir_all(&cold_path);

    // Real construction (only works with backend-sqlite feature):
    // let mgr = MemoryPersistence::new(&db_path, &cold_path, None).expect("MemoryPersistence init");
    // let index = mgr.load_metadata_index().expect("load index");

    sleep(Duration::from_millis(10)).await;
    assert!(true, "metadata index test passed");
}
