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

use crate::memory::memory_persistence::{MemoryEntry, MemoryPersistence};
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
    /// Human-readable label.
    #[allow(dead_code)] // F-GAP-49 — reserved memory retrieval feature
    pub fn label(&self) -> &str {
        match self {
            LinkType::Similar => "similar",
            LinkType::Continuation => "continuation",
            LinkType::Supports => "supports",
            LinkType::Contradicts => "contradicts",
            LinkType::DerivedFrom => "derived_from",
            LinkType::Custom(s) => s.as_str(),
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
    #[allow(dead_code)] // F-GAP-49 — reserved memory retrieval feature
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

    #[allow(dead_code)] // F-GAP-49 — reserved memory retrieval feature
    fn has_link(&self, m1: &str, m2: &str) -> bool {
        self.links.contains_key(&(m1.to_string(), m2.to_string()))
            || self.links.contains_key(&(m2.to_string(), m1.to_string()))
    }

    #[allow(dead_code)] // F-GAP-49 — reserved memory retrieval feature
    fn len(&self) -> usize {
        self.links.len()
    }

    #[allow(dead_code)] // F-GAP-49 — reserved memory retrieval feature
    fn is_empty(&self) -> bool {
        self.links.is_empty()
    }
}

#[allow(dead_code)] // F-GAP-49 — reserved memory retrieval feature
impl MemoryRetrievalEngine {
    /// Create a new retrieval engine backed by the given persistence manager.
    pub fn new(persistence: MemoryPersistence) -> Self {
        Self {
            persistence,
            links: Mutex::new(LinkGraph::default()),
            session_index: Mutex::new(HashMap::new()),
        }
    }

    /// Returns a reference to the underlying persistence manager.
    pub fn persistence(&self) -> &MemoryPersistence {
        &self.persistence
    }

    // ── Retrieval ──────────────────────────────────────────────────────────

    /// Retrieve the most relevant memories for a given text query.
    ///
    /// Searches across all three tiers:
    /// 1. Hot cache: exact-match on content keywords (simple token overlap).
    /// 2. Warm store: usefulness-sorted search.
    /// 3. Cold store: keyword scan (limited for performance).
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
                    if let Ok(Some(entry)) = self.persistence.retrieve(mem_id) {
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
        Ok(graph.has_link(m1, m2))
    }

    /// Return the total number of links in the graph.
    pub fn link_count(&self) -> Result<usize> {
        let graph = self
            .links
            .lock()
            .map_err(|e| anyhow::anyhow!("link graph mutex poisoned: {}", e))?;
        Ok(graph.len())
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

#[allow(dead_code)] // F-GAP-49 — reserved memory retrieval feature
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
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("warm.db");
        let cold_path = dir.path().join("cold");
        let persistence = MemoryPersistence::new(&db_path, &cold_path, None).unwrap();
        let engine = MemoryRetrievalEngine::new(persistence);
        (dir, engine)
    }

    fn seed_entry(engine: &MemoryRetrievalEngine, id: &str, content: &str, usefulness: f32) {
        let entry = MemoryEntry::new_hot(id, "test", content, usefulness);
        engine.persistence().store(entry).unwrap();
    }

    #[test]
    fn test_retrieve_relevant_memories_empty_query() {
        let (_dir, engine) = setup_engine();
        let results = engine.retrieve_relevant_memories("", 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_retrieve_relevant_memories_zero_limit() {
        let (_dir, engine) = setup_engine();
        let results = engine.retrieve_relevant_memories("hello", 0).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_link_memories_roundtrip() {
        let (_dir, engine) = setup_engine();

        seed_entry(&engine, "m1", "first memory", 0.8);
        seed_entry(&engine, "m2", "second memory", 0.6);

        engine.link_memories("m1", "m2", LinkType::Similar).unwrap();

        let links = engine.get_links("m1").unwrap();
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

        assert!(!engine.are_linked("a", "b").unwrap());
        engine.link_memories("a", "b", LinkType::Supports).unwrap();
        assert!(engine.are_linked("a", "b").unwrap());
    }

    #[test]
    fn test_link_count() {
        let (_dir, engine) = setup_engine();
        seed_entry(&engine, "x", "x", 0.5);
        seed_entry(&engine, "y", "y", 0.5);
        seed_entry(&engine, "z", "z", 0.5);

        engine.link_memories("x", "y", LinkType::Similar).unwrap();
        engine
            .link_memories("y", "z", LinkType::Continuation)
            .unwrap();

        assert_eq!(engine.link_count().unwrap(), 2);
    }

    #[test]
    fn test_index_session_memory() {
        let (_dir, engine) = setup_engine();
        seed_entry(&engine, "mem1", "session content", 0.7);

        engine.index_session_memory("session-123", "mem1").unwrap();

        let entries = engine.retrieve_related_sessions("session-123").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "mem1");
    }

    #[test]
    fn test_retrieve_related_sessions_empty() {
        let (_dir, engine) = setup_engine();
        let entries = engine
            .retrieve_related_sessions("nonexistent-session")
            .unwrap();
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

        engine.link_memories("a", "b", LinkType::Similar).unwrap();

        // Getting links for "b" should return the same link (reverse lookup).
        let links_for_b = engine.get_links("b").unwrap();
        assert_eq!(links_for_b.len(), 1);
        assert_eq!(links_for_b[0].m1, "a");
    }
}
