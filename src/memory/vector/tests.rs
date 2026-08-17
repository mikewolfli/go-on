//! SQLite vector-store tests, moved verbatim from the former single-file
//! `vector.rs`. Declared via `mod tests;` in `mod.rs`; the `super::` paths
//! resolve against the `vector` module root, which wires the test-only
//! re-exports the suite relies on.

use super::hnsw::{HnswIndex, HnswNodeMeta};
use super::*;
use std::sync::Arc;

#[tokio::test]
async fn vector_store_upsert_and_search() {
    let dir = tempfile::tempdir().expect("temp dir should be created");
    let db_path = dir.path().join("vector.sqlite3");

    let store =
        Arc::new(VectorStore::new(&db_path, 64, 200).expect("vector store init should work"));
    Arc::clone(&store)
        .upsert(
            "coding",
            "optimize rust async cache",
            "Use sqlite cache and tune ttl for repeated requests.",
        )
        .await
        .expect("upsert should work");

    let (hits, feedback) = Arc::clone(&store)
        .search("coding", "how to optimize async cache", 2, 0.1, 200)
        .await
        .expect("search should work");
    assert!(!hits.is_empty());
    assert!(feedback.hit_count > 0);
    assert!(feedback.avg_similarity > 0.0);
}

#[tokio::test]
async fn vector_store_phase_summary_roundtrip() {
    let dir = tempfile::tempdir().expect("temp dir should be created");
    let db_path = dir.path().join("vector.sqlite3");

    let store =
        Arc::new(VectorStore::new(&db_path, 64, 200).expect("vector store init should work"));
    store
        .upsert_phase_summary("coding", "short summary")
        .await
        .expect("upsert summary should work");

    let summary = store
        .get_phase_summary("coding")
        .await
        .expect("get summary should work");
    assert_eq!(summary.as_deref(), Some("short summary"));
}

#[test]
fn vector_precision_feedback_calculates_average_similarity() {
    use super::VectorHit;

    let hits = vec![
        VectorHit {
            similarity: 0.9,
            response_snippet: "test1".to_string(),
        },
        VectorHit {
            similarity: 0.8,
            response_snippet: "test2".to_string(),
        },
        VectorHit {
            similarity: 0.7,
            response_snippet: "test3".to_string(),
        },
    ];

    let feedback = VectorPrecisionFeedback::new(&hits);
    assert_eq!(feedback.hit_count, 3);
    assert!((feedback.avg_similarity - 0.8).abs() < 0.01); // (0.9 + 0.8 + 0.7) / 3 = 0.8
}

#[test]
fn vector_precision_feedback_handles_empty_hits() {
    let hits = vec![];
    let feedback = VectorPrecisionFeedback::new(&hits);
    assert_eq!(feedback.hit_count, 0);
    assert!((feedback.avg_similarity - 0.0).abs() < f32::EPSILON);
}

#[tokio::test]
async fn vector_search_time_decay_demotes_stale_entry() {
    #[cfg(not(feature = "backend-postgres"))]
    use rusqlite::{params, Connection};

    let dir = tempfile::tempdir().expect("temp dir should be created");
    let db_path = dir.path().join("vector_decay.sqlite3");

    let store =
        Arc::new(VectorStore::new(&db_path, 64, 200).expect("vector store init should work"));

    // Insert a fresh entry with an identical query to get identical embeddings.
    Arc::clone(&store)
        .upsert("coding", "rust async performance", "fresh answer")
        .await
        .expect("fresh upsert should work");

    // Back-date an entry to 180 days ago directly in SQLite to simulate stale knowledge.
    // The memory_key is deterministic from (phase, query_text).
    let stale_ts: i64 = super::now_ts() - 180 * 86_400;
    {
        let conn = Connection::open(&db_path).expect("should open db");
        let embedding = super::local_hash_embed("rust async performance stale", 64);
        let embedding_json = serde_json::to_string(&embedding).expect("should serialize embedding");
        let embedding_blob = super::embedding_blob(&embedding);
        let (json_value, blob_value): (Option<String>, Option<Vec<u8>>) = match store.mode {
            super::SqliteVectorMode::SqliteVec => (None, Some(embedding_blob)),
            super::SqliteVectorMode::JsonFallback => (Some(embedding_json), None),
        };

        conn.execute(
            "INSERT OR REPLACE INTO vector_memory(
                memory_key,
                phase,
                query_text,
                response_text,
                embedding_json,
                embedding_blob,
                created_at,
                updated_at,
                hit_count
             )
             VALUES('__stale_key__', 'coding', 'rust async performance stale', 'stale answer', ?1, ?2, ?3, ?3, 0)",
            params![json_value, blob_value, stale_ts],
        )
        .expect("stale insert should work");
    }

    // The fresh entry should rank higher than the stale one despite similar embeddings.
    let (hits, _) = Arc::clone(&store)
        .search("coding", "rust async performance", 5, 0.0, 200)
        .await
        .expect("search should work");

    // Verify fresh entry ranked first (highest blended score).
    let first_snippet = hits
        .first()
        .map(|h| h.response_snippet.as_str())
        .unwrap_or("");
    assert!(
        first_snippet.contains("fresh"),
        "fresh entry should rank first but got: {first_snippet:?}"
    );
}

#[tokio::test]
async fn hnsw_index_insert_and_search_basic() {
    let dir = tempfile::tempdir().expect("temp dir should be created");
    let db_path = dir.path().join("hnsw_basic.sqlite3");
    let store = Arc::new(VectorStore::new(&db_path, 64, 200).expect("vector store init"));

    // Insert enough entries to trigger HNSW construction
    for i in 0..50 {
        let query = format!("rust feature number {i}");
        let response = format!("response for feature {i}");
        Arc::clone(&store)
            .upsert("test", &query, &response)
            .await
            .expect("upsert");
    }

    // Trigger HNSW build by calling ensure_hnsw_index
    store.ensure_hnsw_index().expect("ensure_hnsw_index");

    // Search via HNSW path
    let (hits, feedback) = Arc::clone(&store)
        .search("test", "rust feature number 5", 5, 0.0, 200)
        .await
        .expect("hnsw search");
    assert!(!hits.is_empty(), "HNSW search should return hits");
    assert!(
        feedback.avg_similarity > 0.0,
        "should have meaningful similarity"
    );
    assert!(
        (0..50).any(|i| hits[0].response_snippet.contains(&format!("feature {i}"))),
        "top result should be near query: got {:?}",
        hits[0].response_snippet
    );
}

#[tokio::test]
async fn hnsw_index_functional_test() {
    // Functional test: validates HNSW build + search with a moderate dataset.
    // Uses 100 vectors (not 10K) for fast execution in CI.
    let dir = tempfile::tempdir().expect("temp dir should be created");
    let db_path = dir.path().join("hnsw_func.sqlite3");
    let store = Arc::new(VectorStore::new(&db_path, 128, 500).expect("vector store init"));

    // Insert 100 vectors - enough to validate the HNSW index works
    for i in 0..100 {
        let query = format!("functional test query number {i}");
        let response = format!("functional test response {i}");
        Arc::clone(&store)
            .upsert("bench", &query, &response)
            .await
            .expect("upsert");
    }

    // Build the HNSW index
    let built = store.ensure_hnsw_index().expect("ensure_hnsw_index");
    assert!(built, "HNSW index should be built");

    // Run searches and verify results are returned correctly
    // Note: with hash-based embeddings and 100 vectors, the top result
    // may not always be the exact semantic match. We verify that:
    // 1. Results are returned for each query
    // 2. At least one of the top-10 results matches each query index
    for query_idx in [0, 25, 50, 99] {
        let query = format!("functional test query number {query_idx}");
        let (hits, _) = Arc::clone(&store)
            .search("bench", &query, 10, 0.0, 200)
            .await
            .expect("search should succeed");
        assert!(
            !hits.is_empty(),
            "should find results for query {query_idx}"
        );
        let found = hits.iter().any(|h| {
            h.response_snippet
                .contains(&format!("response {query_idx}"))
        });
        assert!(
            found,
            "query={query_idx} should be in top-10 results, top={}",
            hits[0].response_snippet
        );
    }
}

#[tokio::test]
async fn hnsw_insert_empty_and_build() {
    let dir = tempfile::tempdir().expect("temp dir should be created");
    let db_path = dir.path().join("hnsw_empty.sqlite3");
    let store = Arc::new(VectorStore::new(&db_path, 64, 100).expect("vector store init"));

    // Build HNSW with empty DB
    let built = store.ensure_hnsw_index().expect("ensure_hnsw_index empty");
    assert!(built, "should build empty index");

    // Search on empty index should return no results
    let (hits, feedback) = Arc::clone(&store)
        .search("test", "something", 5, 0.0, 200)
        .await
        .expect("search on empty");
    assert!(hits.is_empty());
    assert_eq!(feedback.hit_count, 0);
}

#[tokio::test]
async fn ensure_hnsw_index_is_idempotent() {
    let dir = tempfile::tempdir().expect("temp dir should be created");
    let db_path = dir.path().join("hnsw_idempotent.sqlite3");
    let store = Arc::new(VectorStore::new(&db_path, 64, 200).expect("vector store init"));

    Arc::clone(&store)
        .upsert("test", "idempotent query", "idempotent response")
        .await
        .expect("upsert");

    // First ensure builds the index (empty store -> no index yet).
    let first = store.ensure_hnsw_index().expect("ensure_hnsw_index first");
    assert!(first, "first ensure should build the index");

    // Second ensure must hit the fast path and report the index as
    // already present without re-reading the table.
    let second = store.ensure_hnsw_index().expect("ensure_hnsw_index second");
    assert!(!second, "second ensure should not rebuild the index");
}

/// Regression (P2): re-upserting the same (phase, query) must replace the
/// previous HNSW node instead of appending a duplicate.
///
/// Before the fix, `upsert` inserted into the HNSW index without removing
/// a pre-existing node with the same memory_key (only evicted keys were
/// removed). Re-upserting a hot key then returned the same memory_key twice
/// (stale + fresh content) from the fast path while the SQLite path returned
/// one row — and the index accumulated dead nodes unboundedly.
#[tokio::test]
async fn hnsw_reupsert_replaces_previous_node() {
    let dir = tempfile::tempdir().expect("temp dir should be created");
    let db_path = dir.path().join("hnsw_reupsert.sqlite3");
    let store = Arc::new(VectorStore::new(&db_path, 64, 200).expect("vector store init"));

    // Seed the store, then force the HNSW index to be built.
    Arc::clone(&store)
        .upsert("test", "same query", "old answer")
        .await
        .expect("first upsert");
    assert!(
        store.ensure_hnsw_index().expect("ensure_hnsw_index"),
        "HNSW index should be built"
    );

    // Re-upsert the same query with new content.
    Arc::clone(&store)
        .upsert("test", "same query", "fresh answer")
        .await
        .expect("re-upsert");

    // The HNSW fast path must return exactly one hit carrying the fresh
    // content — never the stale one, and never the same key twice.
    let (hits, _) = Arc::clone(&store)
        .search("test", "same query", 5, 0.0, 200)
        .await
        .expect("search after re-upsert");
    let fresh: Vec<&str> = hits
        .iter()
        .map(|h| h.response_snippet.as_str())
        .filter(|s| s.contains("fresh"))
        .collect();
    let stale: Vec<&str> = hits
        .iter()
        .map(|h| h.response_snippet.as_str())
        .filter(|s| s.contains("old answer"))
        .collect();
    assert_eq!(fresh.len(), 1, "exactly one fresh hit, got {fresh:?}");
    assert!(
        stale.is_empty(),
        "stale content must not survive, got {stale:?}"
    );
}

/// Regression (P2): evicting the HNSW entry-point node must re-point the
/// entry point to a live node instead of starting searches from a dead
/// (zeroed) node. Tested directly on `HnswIndex` (private) because the
/// store-level eviction tie-breaks on same-second `updated_at`, which is
/// non-deterministic for churn within one second.
#[test]
fn hnsw_remove_repairs_entry_point() {
    let mut hnsw = HnswIndex::new(16, 200, 50);
    // Insert three nodes. The entry point is the node with the highest
    // random level — with `random_level()` on fastrand, that is NOT
    // deterministically node 0 (the test previously asserted `Some(0)`
    // and flaked under parallel runs), so the invariant is asserted
    // against whichever node was selected.
    for i in 0..3 {
        let mut vec = vec![0.0f32; 64];
        vec[i] = 1.0; // distinct vectors
        hnsw.insert(
            vec,
            HnswNodeMeta {
                memory_key: format!("key-{i}"),
                phase: "test".to_string(),
                response_text: format!("answer {i}"),
                updated_at: i as i64,
            },
        );
    }
    let entry = hnsw.entry_point.expect("entry point after inserts");
    assert!(
        !hnsw.metadata[entry].memory_key.is_empty(),
        "entry point must be a live node, got idx {entry}"
    );

    // Remove the entry point; the index must re-point to a live node.
    let entry_key = hnsw.metadata[entry].memory_key.clone();
    hnsw.remove(&entry_key);
    let Some(ep) = hnsw.entry_point else {
        panic!("entry point must be repaired to a live node");
    };
    assert!(
        !hnsw.metadata[ep].memory_key.is_empty(),
        "repaired entry point must be live, got idx {ep}"
    );

    // Removing all nodes clears the entry point entirely.
    for i in 0..3 {
        hnsw.remove(&format!("key-{i}"));
    }
    assert_eq!(hnsw.entry_point, None, "entry point cleared when empty");
}

/// Regression (P2): `remove` must delete ALL nodes with a matching
/// memory_key — leaving a second stale copy would duplicate results on
/// the search fast path.
#[test]
fn hnsw_remove_deletes_all_matching_nodes() {
    let mut hnsw = HnswIndex::new(16, 200, 50);
    for i in 0..3 {
        let mut vec = vec![0.0f32; 64];
        vec[i] = 1.0;
        // Two nodes share the same memory_key (simulates the pre-fix
        // duplicate-node state after a re-upsert).
        hnsw.insert(
            vec.clone(),
            HnswNodeMeta {
                memory_key: "dup-key".to_string(),
                phase: "test".to_string(),
                response_text: format!("answer {i}"),
                updated_at: i as i64,
            },
        );
    }
    hnsw.remove("dup-key");
    assert!(
        hnsw.metadata.iter().all(|m| m.memory_key.is_empty()),
        "all nodes with the matching key must be removed"
    );
    assert_eq!(
        hnsw.entry_point, None,
        "all nodes removed => no entry point"
    );
}

/// Regression (P2): `clear_all` must reset the in-memory HNSW index.
///
/// Previously it only issued the SQLite DELETEs; the HNSW index kept the
/// old vectors, so the next `search` took the HNSW fast path and returned
/// stale entries that no longer existed in the database.
#[tokio::test]
async fn clear_all_resets_hnsw_index() {
    let dir = tempfile::tempdir().expect("temp dir should be created");
    let db_path = dir.path().join("hnsw_clear.sqlite3");
    let store = Arc::new(VectorStore::new(&db_path, 64, 200).expect("vector store init"));

    for i in 0..20 {
        let query = format!("rust feature number {i}");
        let response = format!("response for feature {i}");
        Arc::clone(&store)
            .upsert("test", &query, &response)
            .await
            .expect("upsert");
    }
    // Force the HNSW index to be built (search fast path).
    let built = store.ensure_hnsw_index().expect("ensure_hnsw_index");
    assert!(built, "HNSW index should be built");

    // Search before clear hits the HNSW fast path and returns entries.
    let (hits, _) = Arc::clone(&store)
        .search("test", "rust feature number 5", 5, 0.0, 200)
        .await
        .expect("search before clear");
    assert!(!hits.is_empty(), "search before clear should return hits");

    let (memory_deleted, _) = store.clear_all().await.expect("clear_all should succeed");
    assert_eq!(memory_deleted, 20, "all 20 entries should be deleted");

    // After clear_all the HNSW index must be gone: a search must NOT
    // return the entries that existed before the clear.
    let (hits, feedback) = Arc::clone(&store)
        .search("test", "rust feature number 5", 5, 0.0, 200)
        .await
        .expect("search after clear");
    assert!(
        hits.is_empty(),
        "stale HNSW entries must not survive clear_all: {hits:?}"
    );
    assert_eq!(feedback.hit_count, 0);
}

/// Regression (P1): a concurrent `upsert` + first `search` must not
/// deadlock, and the lazily-built HNSW index must not lose entries that
/// commit while the first build is in flight.
///
/// Before the lock-order fix, `ensure_hnsw_index` held `hnsw` while
/// waiting for `conn` and `upsert` held `conn` while waiting for `hnsw`,
/// forming a ring that could hang both threads forever.
#[tokio::test]
async fn concurrent_upsert_and_first_search_no_deadlock() {
    let dir = tempfile::tempdir().expect("temp dir should be created");
    let db_path = dir.path().join("hnsw_concurrent.sqlite3");
    let store = Arc::new(VectorStore::new(&db_path, 64, 10_000).expect("vector store init"));

    // A search task that triggers the lazy HNSW build on its first call,
    // racing against the upserts below.
    let search_store = Arc::clone(&store);
    let search_task = tokio::spawn(async move {
        for i in 0..10 {
            Arc::clone(&search_store)
                .search("concurrent", &format!("query {i}"), 10, 0.0, 200)
                .await
                .expect("search should succeed");
        }
    });

    // Concurrently upsert entries; some of them may commit while the
    // lazy index build is snapshotting or publishing.
    for i in 0..200 {
        Arc::clone(&store)
            .upsert(
                "concurrent",
                &format!("query {i}"),
                &format!("response {i}"),
            )
            .await
            .expect("upsert");
    }

    search_task.await.expect("search task should not panic");

    // Every upserted entry must be present in the published index: the
    // build must not publish a snapshot that misses concurrent writes.
    let hnsw_guard = store.hnsw.lock().expect("hnsw lock");
    let hnsw = hnsw_guard.as_ref().expect("index should be built");
    assert_eq!(
        hnsw.metadata.len(),
        200,
        "no upserted entry may be lost from the HNSW index"
    );
}
