//! GAP-B52-13: Memory Retrieval and Linking
//!
//! Provides semantic retrieval of relevant memories and session linkage.
//! Integrates with the memory tier persistence layer (`memory_persistence`)
//! and the vector store for similarity-based searches.
//!
//! # Features
//! - `retrieve_relevant_memories(query, limit)` – semantic search across tiers
//! - `retrieve_related_sessions(session_id)` – find memories by session
//! - `MemoryLink` graph for cross-referencing related memories

//! - `link_memories(m1, m2, link_type)` – create bidirectional links
//! - ANN vector index search via `VectorIndex` for cosine-similarity retrieval

use crate::memory::embedding_provider::{ConfigurableEmbeddingProvider, EmbeddingProvider};
use crate::memory::memory_persistence::{MemoryEntry, MemoryPersistence};
use crate::memory::vector_index::VectorIndex;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

// ===========================================================================
// MemoryLink
// ===========================================================================

/// Describes the semantic relationship between two memories.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum LinkType {
    /// m1 and m2 are semantically similar.
    Similar,
    /// m1 is a continuation or follow-up of m2.
    Continuation,
    /// m1 provides supporting evidence for m2.
    Supports,
    /// m1 contradicts m2.
    Contradicts,
    /// m1 is derived from or summarizes m2.
    DerivedFrom,
    /// Custom link type.
    Custom(String),
}

impl LinkType {
    /// Return a human-readable label for this link type.
    #[allow(dead_code)] // Public API for test consumers
    pub fn label(&self) -> &str {
        match self {
            LinkType::Similar => "similar",
            LinkType::Continuation => "continuation",
            LinkType::Supports => "supports",
            LinkType::Contradicts => "contradicts",
            LinkType::DerivedFrom => "derived_from",
            LinkType::Custom(_) => "custom",
        }
    }
}

/// A directed link between two memory entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryLink {
    /// ID of the source memory.
    pub m1: String,
    /// ID of the target memory.
    pub m2: String,
    /// The semantic relationship.
    pub link_type: LinkType,
    /// Unix timestamp (seconds) when this link was created.
    pub created_at: i64,
    /// Optional weight/strength of the link (0.0 – 1.0).
    pub strength: f32,
}

impl MemoryLink {
    /// Create a new memory link.
    pub fn new(
        m1: impl Into<String>,
        m2: impl Into<String>,
        link_type: LinkType,
        strength: f32,
    ) -> Self {
        Self {
            m1: m1.into(),
            m2: m2.into(),
            link_type,
            created_at: now_secs(),
            strength: strength.clamp(0.0, 1.0),
        }
    }
}

// ===========================================================================
// MemoryRetrievalEngine
// ===========================================================================

/// The retrieval engine that coordinates searches across memory tiers
/// and maintains the memory link graph.
///
/// The engine holds:
/// - A reference to the `MemoryPersistence` instance for tier access.
/// - An in-memory link graph for fast traversal.
/// - Session-to-memory mapping.
#[derive(Debug)]
pub struct MemoryRetrievalEngine {
    /// Reference to the persistence manager.
    persistence: MemoryPersistence,
    /// Link graph: (m1, m2) → MemoryLink. Also indexed by each direction.
    links: Mutex<LinkGraph>,
    /// Session index: session_id → set of memory IDs.
    session_index: Mutex<HashMap<String, Vec<String>>>,
    /// Optional ANN vector index for cosine-similarity search.
    vector_index: Option<VectorIndex>,
    /// Embedding provider for vectorizing queries at retrieval time.
    embedding_provider: ConfigurableEmbeddingProvider,
}

/// Internal link graph with bidirectional indexing.
#[derive(Debug, Clone, Default)]
struct LinkGraph {
    /// All links keyed by (m1, m2).
    links: HashMap<(String, String), MemoryLink>,
    /// Forward index: m1 → Vec<(m2, link_type)>.
    forward: HashMap<String, Vec<(String, LinkType)>>,
    /// Reverse index: m2 → Vec<(m1, link_type)>.
    reverse: HashMap<String, Vec<(String, LinkType)>>,
}

impl LinkGraph {
    fn insert(&mut self, link: MemoryLink) {
        let m1 = link.m1.clone();
        let m2 = link.m2.clone();
        let lt = link.link_type.clone();
        self.links.insert((m1.clone(), m2.clone()), link);
        self.forward
            .entry(m1.clone())
            .or_default()
            .push((m2.clone(), lt.clone()));
        self.reverse.entry(m2).or_default().push((m1, lt));
    }

    fn get_links_for(&self, id: &str) -> Vec<MemoryLink> {
        let mut result = Vec::new();
        // Outgoing
        if let Some(out) = self.forward.get(id) {
            for (target, lt) in out {
                if let Some(link) = self.links.get(&(id.to_string(), target.clone())) {
                    result.push(link.clone());
                } else {
                    // Reconstruct a minimal link
                    result.push(MemoryLink::new(id, target, lt.clone(), 0.5));
                }
            }
        }
        // Incoming
        if let Some(inc) = self.reverse.get(id) {
            for (source, lt) in inc {
                if let Some(link) = self.links.get(&(source.clone(), id.to_string())) {
                    result.push(link.clone());
                } else {
                    result.push(MemoryLink::new(source, id, lt.clone(), 0.5));
                }
            }
        }
        result
    }
}

impl MemoryRetrievalEngine {
    /// Create a new retrieval engine backed by the given persistence manager.
    pub fn new(persistence: MemoryPersistence) -> Self {
        Self {
            persistence,
            links: Mutex::new(LinkGraph::default()),
            session_index: Mutex::new(HashMap::new()),
            vector_index: None,
            embedding_provider: ConfigurableEmbeddingProvider::new_local(128),
        }
    }

    /// Create a new retrieval engine with a pre-built vector index.
    ///
    /// The index is used as an additional signal in `retrieve_relevant_memories`:
    /// entries with high cosine similarity to the query are boosted in the
    /// final ranking even when the token-overlap heuristic is weak.
    pub fn with_vector_index(persistence: MemoryPersistence, vector_index: VectorIndex) -> Self {
        Self {
            persistence,
            links: Mutex::new(LinkGraph::default()),
            session_index: Mutex::new(HashMap::new()),
            vector_index: Some(vector_index),
            embedding_provider: ConfigurableEmbeddingProvider::new_local(128),
        }
    }

    /// Returns a reference to the underlying persistence manager.
    pub fn persistence(&self) -> &MemoryPersistence {
        &self.persistence
    }

    /// Override the embedding provider used for query vectorization.
    ///
    /// By default, a local minhash provider (128 dims) is used. Call this
    /// with a `ConfigurableEmbeddingProvider` configured for `OpenAi`,
    /// `Ollama`, or `Qwen3` to use a real embedding model instead.
    pub fn with_embedding_provider(mut self, provider: ConfigurableEmbeddingProvider) -> Self {
        self.embedding_provider = provider;
        self
    }

    // ── Retrieval ──────────────────────────────────────────────────────────

    /// Retrieve the most relevant memories for a given text query.
    ///
    /// Searches across all tiers and the optional ANN vector index:
    /// 1. Hot cache: exact-match on content keywords (simple token overlap).
    /// 2. Warm store: usefulness-sorted search.
    /// 3. ANN vector index search (when configured): cosine-similarity search
    ///    that catches semantically similar entries the token-overlap step may
    ///    miss.
    ///
    /// Results are deduplicated by ID and sorted by relevance (usefulness + recency).
    ///
    /// # Arguments
    /// * `query` - The search text.
    /// * `limit` - Maximum number of results to return.
    ///
    /// # Returns
    /// A vector of deduplicated `MemoryEntry` values.
    pub fn retrieve_relevant_memories(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryEntry>> {
        if query.trim().is_empty() || limit == 0 {
            return Ok(Vec::new());
        }

        let query_lower = query.to_lowercase();
        let query_tokens: Vec<&str> = query_lower.split_whitespace().collect();

        let mut results: Vec<MemoryEntry> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

        // ── L1: Scan hot cache ──
        {
            let hot_entries = self.collect_hot_entries();
            for mut entry in hot_entries {
                if seen.contains(&entry.id) {
                    continue;
                }
                if self.text_matches_query(&entry.content, &query_tokens) {
                    entry.touch();
                    seen.insert(entry.id.clone());
                    results.push(entry);
                }
            }
        }

        // ── L2: Search warm store by usefulness ──
        {
            let warm_entries = self
                .persistence
                .warm_store()
                .search_by_usefulness(0.0, limit * 2)
                .unwrap_or_default();
            for mut entry in warm_entries {
                if seen.contains(&entry.id) {
                    continue;
                }
                if self.text_matches_query(&entry.content, &query_tokens) {
                    entry.touch();
                    seen.insert(entry.id.clone());
                    results.push(entry);
                }
            }
        }

        // ── L3: ANN vector index search (semantic, bypasses token overlap) ──
        {
            // Compute a query embedding using the configured embedding provider.
            // When set to `Local` (default), this uses a lightweight deterministic
            // hash with no network call. When set to `OpenAi`/`Ollama`, this calls
            // the respective API.
            let embedding = self.embedding_provider.embed(query);
            let query_vec: Vec<f64> = embedding.iter().map(|v| *v as f64).collect();

            let ann_results = self.search_vector_index(&query_vec, limit);
            for mut entry in ann_results {
                if seen.contains(&entry.id) {
                    continue;
                }
                entry.touch();
                seen.insert(entry.id.clone());
                results.push(entry);
            }
        }

        // ── L4: Cold storage search (long-term archival) ──
        {
            // Scan cold storage via the persistence layer.
            // This is a linear scan of all cold shards and should be optimized
            // with ColdStorageIndex in production.
            let cold_entries = self.persistence().cold_entries().unwrap_or_default();
            for mut entry in cold_entries {
                if seen.contains(&entry.id) {
                    continue;
                }
                if self.text_matches_query(&entry.content, &query_tokens) {
                    entry.touch();
                    seen.insert(entry.id.clone());
                    results.push(entry);
                }
            }
        }

        // ── Sort by usefulness descending, then by accessed_at descending ──
        results.sort_by(|a, b| {
            b.usefulness
                .partial_cmp(&a.usefulness)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.accessed_at.cmp(&a.accessed_at))
        });

        results.truncate(limit);
        Ok(results)
    }

    /// Retrieve all memory entries associated with a given session.
    ///
    /// Checks the session index first, then falls back to scanning
    /// the warm store for matching session_id.
    pub fn retrieve_related_sessions(&self, session_id: &str) -> Result<Vec<MemoryEntry>> {
        let mut results: Vec<MemoryEntry> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

        // Check in-memory session index.
        {
            let idx = self
                .session_index
                .lock()
                .map_err(|e| anyhow::anyhow!("session index mutex poisoned: {}", e))?;
            if let Some(ids) = idx.get(session_id) {
                for mem_id in ids {
                    if seen.contains(mem_id) {
                        continue;
                    }
                    seen.insert(mem_id.clone());
                    // Try to retrieve the full entry.
                    if let Ok(Some(entry)) = self.persistence.retrieve(None, mem_id) {
                        results.push(entry);
                    }
                }
            }
        }

        // Query warm store directly for session_id matches.
        #[cfg(feature = "backend-sqlite")]
        {
            let warm_entries = self
                .persistence
                .warm_store()
                .search_by_session(session_id, 100)
                .unwrap_or_default();
            for entry in warm_entries {
                if seen.contains(&entry.id) {
                    continue;
                }
                seen.insert(entry.id.clone());
                results.push(entry);
            }
        }

        results.sort_by_key(|b| std::cmp::Reverse(b.accessed_at));
        Ok(results)
    }

    // ── Linking ────────────────────────────────────────────────────────────

    /// Create a bidirectional link between two memory entries.
    ///
    /// Returns `Ok(())` if the link was created. If a link already exists
    /// between the two entries, it is updated with the new type/strength.
    ///
    /// # Arguments
    /// * `m1` - ID of the first memory.
    /// * `m2` - ID of the second memory.
    /// * `link_type` - The semantic relationship.
    pub fn link_memories(&self, m1: &str, m2: &str, link_type: LinkType) -> Result<()> {
        if m1 == m2 {
            anyhow::bail!("cannot link a memory to itself");
        }

        let link = MemoryLink::new(m1, m2, link_type, 1.0);
        let mut graph = self
            .links
            .lock()
            .map_err(|e| anyhow::anyhow!("link graph mutex poisoned: {}", e))?;
        graph.insert(link);
        Ok(())
    }

    /// Get all links involving a specific memory entry.
    pub fn get_links(&self, memory_id: &str) -> Result<Vec<MemoryLink>> {
        let graph = self
            .links
            .lock()
            .map_err(|e| anyhow::anyhow!("link graph mutex poisoned: {}", e))?;
        Ok(graph.get_links_for(memory_id))
    }

    /// Check if two memories are already linked.
    pub fn are_linked(&self, m1: &str, m2: &str) -> Result<bool> {
        let graph = self
            .links
            .lock()
            .map_err(|e| anyhow::anyhow!("link graph mutex poisoned: {}", e))?;
        Ok(graph.links.contains_key(&(m1.to_string(), m2.to_string()))
            || graph.links.contains_key(&(m2.to_string(), m1.to_string())))
    }

    /// Return the total number of links in the graph.
    pub fn link_count(&self) -> Result<usize> {
        let graph = self
            .links
            .lock()
            .map_err(|e| anyhow::anyhow!("link graph mutex poisoned: {}", e))?;
        Ok(graph.links.len())
    }

    /// Register a memory ID as belonging to a session.
    pub fn index_session_memory(&self, session_id: &str, memory_id: &str) -> Result<()> {
        let mut idx = self
            .session_index
            .lock()
            .map_err(|e| anyhow::anyhow!("session index mutex poisoned: {}", e))?;
        idx.entry(session_id.to_string())
            .or_default()
            .push(memory_id.to_string());
        Ok(())
    }

    // ── Private helpers ────────────────────────────────────────────────────

    /// Collect all entries currently in the hot cache.
    fn collect_hot_entries(&self) -> Vec<MemoryEntry> {
        // Snapshot all hot-tier entries via the persistence layer, which
        // exposes hot entries through `hot_entries()`. This enables L1 cache
        // scanning for the retrieval engine.
        self.persistence.hot_entries()
    }

    // ── Vector index ───────────────────────────────────────────────────────

    /// Build (or rebuild) the ANN vector index from the warm store.
    ///
    /// Iterates all entries in the warm store and indexes those that carry
    /// a pre-computed embedding.  The index dimension is inferred from the
    /// first embedding found; entries with a mismatched dimension are skipped.
    ///
    /// Returns `true` when at least one entry was indexed.
    pub fn build_vector_index_from_warm_store(&mut self) -> Result<bool> {
        let dimension = self.embedding_provider.dimensions();

        let warm_entries = self
            .persistence
            .warm_store()
            .search_by_usefulness(0.0, 10_000)
            .unwrap_or_default();

        let idx = VectorIndex::from_entries(&warm_entries, dimension);
        let has_entries = !idx.is_empty();
        self.vector_index = Some(idx);
        Ok(has_entries)
    }

    /// Search the vector index with a query embedding, returning scored memory
    /// entries.  Returns an empty vec when no index is configured or the index
    /// is empty.
    pub fn search_vector_index(&self, query_embedding: &[f64], k: usize) -> Vec<MemoryEntry> {
        let Some(ref idx) = self.vector_index else {
            return Vec::new();
        };
        if idx.is_empty() {
            return Vec::new();
        }
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        idx.search(query_embedding, k)
            .into_iter()
            .filter_map(|(id, sim, _content)| {
                // Try to hydrate the full MemoryEntry from persistence.
                self.persistence
                    .retrieve(None, &id)
                    .ok()
                    .flatten()
                    .map(|mut entry| {
                        // Boost usefulness by the similarity score so that
                        // vector-matched entries rank higher in the final sort.
                        entry.usefulness = entry.usefulness.max(sim as f32 * 0.9);
                        entry.accessed_at = entry.accessed_at.max(now_secs);
                        entry
                    })
            })
            .collect()
    }

    /// Simple token-overlap matching. Returns true if any query token
    /// appears in the content text.
    fn text_matches_query(&self, content: &str, query_tokens: &[&str]) -> bool {
        if query_tokens.is_empty() {
            return false;
        }
        let content_lower = content.to_lowercase();
        query_tokens
            .iter()
            .any(|token| content_lower.contains(token))
    }
}

// ===========================================================================
// Helpers
// ===========================================================================

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::memory_persistence::MemoryEntry;
    use tempfile::TempDir;

    fn setup_engine() -> (TempDir, MemoryRetrievalEngine) {
        let dir = TempDir::new().expect("temp dir creation should succeed");
        let db_path = dir.path().join("warm.db");
        let cold_path = dir.path().join("cold");
        let persistence = MemoryPersistence::new(&db_path, &cold_path, None)
            .expect("persistence should initialize");
        let engine = MemoryRetrievalEngine::new(persistence);
        (dir, engine)
    }

    fn seed_entry(engine: &MemoryRetrievalEngine, id: &str, content: &str, usefulness: f32) {
        let entry = MemoryEntry::new_hot(id, "test", content, usefulness);
        engine
            .persistence()
            .store(entry)
            .expect("store should succeed");
    }

    #[test]
    fn test_retrieve_relevant_memories_empty_query() {
        let (_dir, engine) = setup_engine();
        let results = engine
            .retrieve_relevant_memories("", 10)
            .expect("retrieve relevant memories should succeed for empty query");
        assert!(results.is_empty());
    }

    #[test]
    fn test_retrieve_relevant_memories_zero_limit() {
        let (_dir, engine) = setup_engine();
        let results = engine
            .retrieve_relevant_memories("hello", 0)
            .expect("retrieve relevant memories should succeed for zero limit");
        assert!(results.is_empty());
    }

    #[test]
    fn test_link_memories_roundtrip() {
        let (_dir, engine) = setup_engine();

        seed_entry(&engine, "m1", "first memory", 0.8);
        seed_entry(&engine, "m2", "second memory", 0.6);

        engine
            .link_memories("m1", "m2", LinkType::Similar)
            .expect("link memories should succeed");

        let links = engine.get_links("m1").expect("get links should succeed");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].m2, "m2");
        assert_eq!(links[0].link_type, LinkType::Similar);
    }

    #[test]
    fn test_link_memories_self_link_fails() {
        let (_dir, engine) = setup_engine();
        let result = engine.link_memories("m1", "m1", LinkType::Similar);
        assert!(result.is_err());
    }

    #[test]
    fn test_are_linked() {
        let (_dir, engine) = setup_engine();
        seed_entry(&engine, "a", "content a", 0.5);
        seed_entry(&engine, "b", "content b", 0.5);

        assert!(!engine
            .are_linked("a", "b")
            .expect("are linked should succeed"));
        engine
            .link_memories("a", "b", LinkType::Supports)
            .expect("link memories should succeed");
        assert!(engine
            .are_linked("a", "b")
            .expect("are linked should succeed"));
    }

    #[test]
    fn test_link_count() {
        let (_dir, engine) = setup_engine();
        seed_entry(&engine, "x", "x", 0.5);
        seed_entry(&engine, "y", "y", 0.5);
        seed_entry(&engine, "z", "z", 0.5);

        engine
            .link_memories("x", "y", LinkType::Similar)
            .expect("link memories should succeed");
        engine
            .link_memories("y", "z", LinkType::Continuation)
            .expect("link memories should succeed");

        assert_eq!(engine.link_count().expect("link count should succeed"), 2);
    }

    #[test]
    fn test_index_session_memory() {
        let (_dir, engine) = setup_engine();
        seed_entry(&engine, "mem1", "session content", 0.7);

        engine
            .index_session_memory("session-123", "mem1")
            .expect("index session memory should succeed");

        let entries = engine
            .retrieve_related_sessions("session-123")
            .expect("retrieve related sessions should succeed");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "mem1");
    }

    #[test]
    fn test_retrieve_related_sessions_empty() {
        let (_dir, engine) = setup_engine();
        let entries = engine
            .retrieve_related_sessions("nonexistent-session")
            .expect("retrieve related sessions should succeed");
        assert!(entries.is_empty());
    }

    #[test]
    fn test_link_type_label() {
        assert_eq!(LinkType::Similar.label(), "similar");
        assert_eq!(LinkType::Continuation.label(), "continuation");
        assert_eq!(LinkType::Custom("my_type".into()).label(), "my_type");
    }

    #[test]
    fn test_memory_link_creation() {
        let link = MemoryLink::new("m1", "m2", LinkType::DerivedFrom, 0.85);
        assert_eq!(link.m1, "m1");
        assert_eq!(link.m2, "m2");
        assert_eq!(link.link_type, LinkType::DerivedFrom);
        assert_eq!(link.strength, 0.85);
        assert!(link.created_at > 0);
    }

    #[test]
    fn test_get_links_reverse() {
        let (_dir, engine) = setup_engine();
        seed_entry(&engine, "a", "alpha", 0.5);
        seed_entry(&engine, "b", "beta", 0.5);

        engine
            .link_memories("a", "b", LinkType::Similar)
            .expect("link memories should succeed");

        // Getting links for "b" should return the link involving "b" (reverse lookup).
        let links_for_b = engine.get_links("b").expect("get links should succeed");

        assert_eq!(links_for_b.len(), 1);
        // The link has m1="a", m2="b". Both endpoints should involve "b".
        assert!(
            links_for_b[0].m1 == "b" || links_for_b[0].m2 == "b",
            "Expected link involving 'b', got (m1={}, m2={})",
            links_for_b[0].m1,
            links_for_b[0].m2
        );
    }

    #[test]
    fn test_vector_index_search_returns_results() {
        let (_dir, engine) = setup_engine();

        // Seed entries with distinct content that won't match the query via
        // token overlap but will be caught by the vector index.
        // We store entries in the warm store with embeddings.
        let entry = MemoryEntry {
            id: "vec-1".to_string(),
            tier: crate::memory::memory_persistence::MemoryTier::Warm,
            class: "test".to_string(),
            content: "the cat sat on the mat".to_string(),
            created_at: 1000,
            accessed_at: 1000,
            usefulness: 0.5,
            embedding: Some(crate::memory::embedding_provider::local_hash_embed(
                "feline animal",
                128,
            )),
            access_count: 1,
            session_id: None,
            user_id: None,
        };
        engine
            .persistence()
            .store(entry)
            .expect("store should succeed");
        // Promote from hot → warm so the vector index can find it
        // (auto_migrate only evicts expired entries, but this entry is fresh)
        let stored = engine
            .persistence()
            .retrieve(None, "vec-1")
            .expect("retrieve should succeed")
            .expect("retrieved entry should be present");
        engine
            .persistence()
            .promote(&stored)
            .expect("promote should succeed");

        // Build vector index from warm store
        let mut mut_engine = engine;
        assert!(
            mut_engine
                .build_vector_index_from_warm_store()
                .expect("build vector index should succeed"),
            "expected at least one indexed entry"
        );

        // Search via the vector index directly
        let query_embedding: Vec<f64> =
            crate::memory::embedding_provider::local_hash_embed("cat", 128)
                .iter()
                .map(|v| *v as f64)
                .collect();
        let results = mut_engine.search_vector_index(&query_embedding, 5);
        assert!(
            !results.is_empty(),
            "vector index should return the seeded entry"
        );
        assert_eq!(results[0].id, "vec-1");

        // Full retrieve_relevant_memories should also pick it up via L3
        let memories = mut_engine
            .retrieve_relevant_memories("cat", 10)
            .expect("retrieve relevant memories should succeed");
        assert!(
            memories.iter().any(|m| m.id == "vec-1"),
            "retrieve_relevant_memories should include the vector-matched entry"
        );
    }

    #[test]
    fn test_vector_index_empty_no_embeddings() {
        let (_dir, mut engine) = setup_engine();
        // Seed an entry without an embedding
        seed_entry(&engine, "no-emb", "plain text", 0.5);

        let indexed = engine
            .build_vector_index_from_warm_store()
            .expect("build vector index should succeed");
        assert!(!indexed, "no entries should be indexed without embeddings");

        let query_embedding = vec![0.0_f64; 128];
        let results = engine.search_vector_index(&query_embedding, 5);
        assert!(results.is_empty(), "empty index should return no results");
    }
}
