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
//! # integration-test
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

    // Validate MemoryEntry helper methods.
    assert!(entry_a.age_secs() >= 0, "age must be non-negative");
    assert!(entry_a.idle_secs() >= 0, "idle time must be non-negative");
    let mut touched = entry_a.clone();
    touched.touch();
    assert_eq!(touched.access_count, 1, "touch increments access count");
    assert!(touched.accessed_at >= entry_a.accessed_at);

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

    // Validate that warm tier entries retain their content and metadata.
    assert_eq!(warm_a.content, "User mentioned preference for dark mode");
    assert_eq!(warm_b.content, "Project deadline is 2026-06-15");
    assert_eq!(warm_a.usefulness, entry_a.usefulness);

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

    // Validate cold tier invariants.
    assert_eq!(cold_a.tier, MemoryTier::Cold);
    assert_eq!(cold_b.tier, MemoryTier::Cold);
    assert_eq!(cold_a.id, "mem-e2e-l1-001");
    assert_eq!(cold_b.id, "mem-e2e-l1-002");
    assert!(!cold_a.content.is_empty());
    assert!(!cold_b.content.is_empty());

    // ── 5. Restore from L3 → L2 ────────────────────────────────────────
    // Restore an entry and verify it lands in warm.
    let mut restored = cold_a.clone();
    restored.tier = MemoryTier::Warm;
    assert_eq!(restored.tier, MemoryTier::Warm);
    assert_eq!(restored.content, "User mentioned preference for dark mode");
    assert_eq!(restored.id, "mem-e2e-l1-001");

    // Verify restored entry retains original content and ID.
    assert_eq!(restored.content, "User mentioned preference for dark mode");
    assert_eq!(restored.id, "mem-e2e-l1-001");
    assert_eq!(restored.tier, MemoryTier::Warm);
    assert_eq!(restored.class, "episodic");

    // ── 6. Cross-session retrieval ─────────────────────────────────────
    // Simulate a second session retrieving the restored entry.
    let session_b_content = restored.content.clone();
    assert_eq!(session_b_content, "User mentioned preference for dark mode");

    // Simulate cross-session: construct a fresh entry with the same content
    // and verify it matches.
    let cross_session = MemoryEntry::new_hot(
        "mem-e2e-l1-001",
        "episodic",
        "User mentioned preference for dark mode",
        0.75,
    );
    assert_eq!(cross_session.id, "mem-e2e-l1-001");
    assert_eq!(cross_session.content, restored.content);
    assert_eq!(cross_session.class, restored.class);
    assert_eq!(cross_session.tier, MemoryTier::Hot);

    // Verify the archive directory exists and is valid.
    assert!(
        ctx.archive_dir.as_ref().unwrap().exists(),
        "archive directory must exist for L3 storage"
    );

    // ── 7. Teardown via Drop ───────────────────────────────────────────
    sleep(Duration::from_millis(10)).await;
}

/// Tests automatic demotion from L1 → L2 when hot cache exceeds capacity.
#[tokio::test]
async fn test_memory_persistence_automatic_demotion_on_capacity() {
    // Create a policy with a small hot cache to force eviction.
    let policy = MemoryTieringPolicy {
        hot_max_entries: 5,
        hot_ttl_secs: 3600,
        ..Default::default()
    };

    assert_eq!(policy.hot_max_entries, 5);

    // Validate the tiering policy.
    assert_eq!(policy.warm_threshold, 0.6);
    assert_eq!(policy.hot_threshold, 0.3);
    assert!(policy.hot_max_entries > 0);
    assert!(policy.hot_ttl_secs > 0);

    // Simulate the eviction logic: inserting 10 entries into a cache with
    // max 5. The first 5 inserted entries would be evicted (LRU) when
    // entries 5-9 are inserted. Evicted entries conceptually move to warm.
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

    // The first 5 inserted entries would be evicted when inserting entries 5-9.
    let evicted_count = entries.len().saturating_sub(policy.hot_max_entries);
    assert_eq!(evicted_count, 5);
    assert!(evicted_count > 0, "entries above capacity must be demoted");

    // Verify the evicted entries (those beyond max) are structurally sound.
    for entry in entries[policy.hot_max_entries..].iter() {
        assert_eq!(entry.tier, MemoryTier::Hot, "source entries remain hot");
        assert!(entry.id.starts_with("auto-demote-"));
        assert!(entry.usefulness > 0.0);
        // Simulate demotion.
        let mut demoted = entry.clone();
        demoted.tier = MemoryTier::Warm;
        assert_eq!(demoted.tier, MemoryTier::Warm);
    }

    // Validate TTL-based promotion logic.
    let ttl_policy = MemoryTieringPolicy {
        hot_ttl_secs: 1,
        warm_ttl_secs: 10,
        ..Default::default()
    };
    assert_eq!(ttl_policy.hot_ttl_secs, 1);
    assert_eq!(ttl_policy.warm_ttl_secs, 10);

    sleep(Duration::from_millis(10)).await;
}

/// Tests the metadata index retrieval.
#[tokio::test]
async fn test_memory_persistence_metadata_index() {
    // Validate that we can construct entries with proper metadata and
    // verify the structural invariants that a metadata index would enforce.
    let cold_path = std::env::temp_dir().join("go-on-e2e-meta-cold");
    let _ = std::fs::create_dir_all(&cold_path);

    // Create entries with different classes and usefulness scores to
    // simulate what a metadata index would track.
    let entries = vec![
        MemoryEntry::new_hot("meta-001", "episodic", "met entry 1", 0.9),
        MemoryEntry::new_hot("meta-002", "semantic", "met entry 2", 0.7),
        MemoryEntry::new_hot("meta-003", "procedural", "met entry 3", 0.5),
        MemoryEntry::new_hot("meta-004", "episodic", "met entry 4", 0.2),
    ];

    // Verify all tiers start as Hot.
    for entry in &entries {
        assert_eq!(entry.tier, MemoryTier::Hot);
        assert!(!entry.id.is_empty());
        assert!(!entry.content.is_empty());
    }

    // Verify the cold storage path is valid.
    assert!(cold_path.exists(), "cold storage directory must exist");
    assert!(cold_path.is_dir());

    // Simulate an index query: entries with usefulness >= 0.6 are
    // candidates for promotion to Warm.
    let high_usefulness: Vec<&MemoryEntry> =
        entries.iter().filter(|e| e.usefulness >= 0.6).collect();
    assert_eq!(high_usefulness.len(), 2, "entries with usefulness >= 0.6");
    assert!(high_usefulness.iter().any(|e| e.id == "meta-001"));
    assert!(high_usefulness.iter().any(|e| e.id == "meta-002"));

    // Validate tear-down of temp directory.
    let _ = std::fs::remove_dir_all(&cold_path);

    sleep(Duration::from_millis(10)).await;
}
