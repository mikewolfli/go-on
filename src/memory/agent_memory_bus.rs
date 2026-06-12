//! Agent shared memory bus (GAP-B54-014).
//!
//! Provides a `MemoryBus` struct that allows agents to store and retrieve
//! memories from the central `MemoryStore`.  When an agent completes a
//! task, key insights are automatically stored.  When an agent is about
//! to start, relevant past memories are retrieved and injected into the
//! prompt context so the agent benefits from past experience.

use std::sync::{Arc, Mutex, OnceLock};

use crate::memory::memory::{MemoryClass, MemoryEntry, MemoryStore};
use crate::memory::vector::VectorStore;
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// MemoryBus
// ---------------------------------------------------------------------------

/// Shared memory bus for agents.
///
/// Agents use this bus to persist insights after completing a task and to
/// retrieve relevant context before starting a new task.  When `user_id` is
/// set, all stored memories are tagged with that user and retrieval filters
/// by that user, providing multi-user isolation.
pub struct AgentMemoryBus {
    /// The underlying memory store (shared so agents and the bus see the
    /// same data without additional synchronisation).
    store: Arc<Mutex<MemoryStore>>,
    /// Maximum number of insights stored per agent task completion.
    max_insights_per_task: usize,
    /// Optional VectorStore for similarity-based memory retrieval.
    /// When set, `retrieve_memories` uses vector search instead of
    /// linear substring/tag scanning.
    vector_store: Option<Arc<VectorStore>>,
    /// Optional user identifier for multi-user isolation.
    /// When set, stored entries are tagged with this user_id and retrieval
    /// filters by this user_id.
    user_id: Option<String>,
}

impl AgentMemoryBus {
    /// Create a new agent memory bus wrapping the given store.
    #[allow(dead_code)] // F-GAP-49 — reserved for multi-user memory bus enhancement
    pub fn new(store: Arc<Mutex<MemoryStore>>) -> Self {
        Self {
            store,
            max_insights_per_task: 5,
            vector_store: None,
            user_id: None,
        }
    }

    /// Create a new agent memory bus with a default `MemoryStore`.
    pub fn new_default() -> Self {
        let store = Arc::new(Mutex::new(MemoryStore::new(Default::default())));
        Self {
            store,
            max_insights_per_task: 5,
            vector_store: None,
            user_id: None,
        }
    }

    /// Set the user_id for multi-user isolation.
    /// When set, stored memories are tagged with this user_id and retrieval
    /// filters by it.
    #[allow(dead_code)] // F-GAP-49 — reserved for multi-user memory bus enhancement
    pub fn with_user_id(mut self, user_id: String) -> Self {
        self.user_id = Some(user_id);
        self
    }

    /// Set the maximum number of insights stored per task completion.
    #[allow(dead_code)] // F-GAP-49 — reserved for multi-user memory bus enhancement
    pub fn with_max_insights_per_task(mut self, n: usize) -> Self {
        self.max_insights_per_task = n;
        self
    }

    /// Return a reference to the underlying store.
    #[allow(dead_code)] // F-GAP-49 — reserved for multi-user memory bus enhancement
    pub fn store(&self) -> &Arc<Mutex<MemoryStore>> {
        &self.store
    }

    /// Attach a VectorStore for similarity-based memory retrieval.
    /// Wired via runtime.rs init_agent_memory_bus_with_vector_store.
    #[allow(dead_code)] // F-GAP-49 — reserved for multi-user memory bus enhancement
    pub fn with_vector_store(mut self, vs: Arc<VectorStore>) -> Self {
        self.vector_store = Some(vs);
        self
    }

    // ── Store ─────────────────────────────────────────────────────────

    /// Store a memory entry in the bus.
    ///
    /// The entry is placed in the `Semantic` class by default so it is
    /// eligible for cross-agent retrieval.
    ///
    /// When `user_id` is provided, it tags the entry with that user_id
    /// to support multi-user isolation. Falls back to `self.user_id` when
    /// the parameter is `None`.
    pub fn store_memory(&self, entry: MemoryEntry, user_id: Option<&str>) {
        let uid = user_id.or(self.user_id.as_deref()).map(|s| s.to_string());
        let mut entry = entry;
        entry.user_id = uid;
        let mut store = match self.store.lock() {
            Ok(s) => s,
            Err(poisoned) => {
                warn!("AgentMemoryBus store poisoned, recovering");
                poisoned.into_inner()
            }
        };
        store.store(entry);
    }

    /// Store an auto‑derived insight after an agent completes a task.
    ///
    /// `tags` are free‑form keywords (e.g. `["rust", "async", "sqlite"]`)
    /// that can be used during retrieval to match relevant tasks.
    ///
    /// The insight is stored as a `MemoryEntry` with class `Semantic` and
    /// the given `importance` (0.0 – 1.0).
    pub fn store_insight(
        &self,
        agent_name: &str,
        task_description: &str,
        insight: &str,
        tags: &[String],
        importance: f32,
        user_id: Option<&str>,
    ) {
        let content = format!(
            "agent={} task={} tags={} insight={}",
            agent_name,
            task_description,
            tags.join(","),
            insight
        );
        let id = format!("agent_mem_{:x}", {
            let mut hasher = sha2::Sha256::new();
            use sha2::Digest;
            hasher.update(content.as_bytes());
            hasher.finalize()
        });

        let entry = MemoryEntry {
            id,
            class: MemoryClass::Semantic,
            content,
            timestamp: format!(
                "{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            ),
            usefulness: importance,
            staleness: 0,
            user_id: None, // will be set by store_memory below
        };
        self.store_memory(entry, user_id);
    }

    /// Automatically store key insights after an agent completes a task.
    ///
    /// This is the high‑level method called by the agent dispatch pipeline.
    /// It extracts up to `max_insights_per_task` insights from the response
    /// text and stores them in the bus.
    pub fn store_agent_completion(
        &self,
        agent_name: &str,
        phase: &str,
        task_description: &str,
        response_text: &str,
        success: bool,
        user_id: Option<&str>,
    ) {
        if response_text.trim().is_empty() {
            return;
        }

        // Generate a set of tags from the phase and agent name.
        let tags: Vec<String> = vec![
            phase.to_string(),
            agent_name.to_string(),
            if success {
                "success".to_string()
            } else {
                "failure".to_string()
            },
        ];

        // Extract short insight snippets from the response (first few sentences).
        let insights = Self::extract_insights(response_text, self.max_insights_per_task);
        let importance = if success { 0.7 } else { 0.3 };

        for (i, snippet) in insights.iter().enumerate() {
            let tag_with_idx = format!("insight_{}", i);
            let mut entry_tags = tags.clone();
            entry_tags.push(tag_with_idx);
            self.store_insight(
                agent_name,
                task_description,
                snippet,
                &entry_tags,
                importance,
                user_id,
            );
        }

        info!(
            "AgentMemoryBus: stored {} insights for agent '{}' on phase '{}' (success={})",
            insights.len(),
            agent_name,
            phase,
            success
        );
    }

    // ── Retrieve ──────────────────────────────────────────────────────

    /// Retrieve up to `limit` memories relevant to the given query.
    ///
    /// When a [`VectorStore`] is attached (via [`with_vector_store`]), this
    /// uses vector similarity search via [`VectorStore::search`] with the
    /// `"agent_memory"` phase.  Otherwise falls back to the original linear
    /// substring/tag scan of the in-memory `Semantic` class.
    pub fn retrieve_memories(
        &self,
        query: &str,
        limit: usize,
        user_id: Option<&str>,
    ) -> Vec<MemoryEntry> {
        let effective_user_id = user_id.or(self.user_id.as_deref());

        // Fast path: vector similarity search via VectorStore
        if let Some(ref vs) = self.vector_store {
            match vs.search("agent_memory", query, limit, 0.0, 512) {
                Ok((hits, _)) => {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    return hits
                        .into_iter()
                        .map(|hit| MemoryEntry {
                            id: format!("vec_{:x}", {
                                use sha2::Digest;
                                let mut h = sha2::Sha256::new();
                                h.update(hit.response_snippet.as_bytes());
                                let d = h.finalize();
                                u64::from_le_bytes(d[0..8].try_into().unwrap_or_default())
                            }),
                            class: MemoryClass::Semantic,
                            content: hit.response_snippet,
                            timestamp: now.to_string(),
                            usefulness: hit.similarity.clamp(0.0, 1.0),
                            staleness: 0,
                            user_id: effective_user_id.map(|s| s.to_string()),
                        })
                        .collect();
                }
                Err(e) => {
                    warn!("AgentMemoryBus: vector search failed, falling back to linear scan: {e}");
                    // Fall through to linear-scan fallback
                }
            }
        }

        // Fallback: linear substring/tag scan with recency/importance weighting.
        // When effective_user_id is set, filter entries to only those belonging to this user.
        let store = match self.store.lock() {
            Ok(s) => s,
            Err(poisoned) => {
                warn!("AgentMemoryBus store poisoned, recovering");
                poisoned.into_inner()
            }
        };

        let all: Vec<MemoryEntry> = store.retrieve(MemoryClass::Semantic, usize::MAX);
        drop(store);

        // Multi-user isolation: filter by effective_user_id when set.
        let all: Vec<MemoryEntry> = match effective_user_id {
            Some(uid) => all
                .into_iter()
                .filter(|e| e.user_id.as_deref() == Some(uid))
                .collect(),
            None => all,
        };

        if all.is_empty() {
            return Vec::new();
        }

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let query_lower = query.to_lowercase();
        let query_tags: Vec<&str> = query_lower
            .split(|c: char| c.is_whitespace() || c == ',' || c == '.')
            .filter(|t| t.len() >= 2)
            .collect();

        // Score each entry using recency, importance, and keyword relevance.
        // The combined score = 0.3 * recency + 0.4 * importance + 0.3 * keyword_match
        // so that more recent, highly important, and semantically matching memories
        // are ranked highest — true short-term memory recall.
        let mut scored: Vec<(f64, &MemoryEntry)> = all
            .iter()
            .map(|entry| {
                let timestamp: u64 = entry.timestamp.parse().unwrap_or(0);
                let age_ms = now_ms.saturating_sub(timestamp);
                // Recency: linear decay over 1 day (86,400,000 ms).
                let recency = 1.0 - (age_ms as f64 / 86400000.0).min(1.0);
                let importance = entry.usefulness as f64;
                let keyword_score = if query_tags.is_empty() {
                    // When there are no meaningful query tokens, rely on recency + importance.
                    0.0
                } else {
                    let matches = query_tags
                        .iter()
                        .filter(|tag| entry.content.to_lowercase().contains(*tag))
                        .count();
                    matches as f64 / query_tags.len() as f64
                };
                let score = 0.3 * recency + 0.4 * importance + 0.3 * keyword_score;
                (score, entry)
            })
            .filter(|(score, _)| *score > 0.0)
            .collect();

        // Sort descending by score.
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        scored
            .into_iter()
            .take(limit)
            .map(|(_, entry)| entry.clone())
            .collect()
    }

    /// Retrieve relevant memories and format them as a prompt context
    /// string that can be injected before an agent starts.
    ///
    /// Returns `None` when no relevant memories are found.
    pub fn retrieve_context_for_agent(
        &self,
        agent_name: &str,
        phase: &str,
        task_description: &str,
        max_memories: usize,
        user_id: Option<&str>,
    ) -> Option<String> {
        let query = format!("{} {} {}", agent_name, phase, task_description);
        let memories = self.retrieve_memories(&query, max_memories, user_id);

        if memories.is_empty() {
            return None;
        }

        let mut lines: Vec<String> = Vec::with_capacity(memories.len() + 2);
        lines.push("── Previous relevant memories ──".to_string());
        for (i, mem) in memories.iter().enumerate() {
            // Strip the internal prefix to keep the context clean.
            let content = mem
                .content
                .trim_start_matches("agent=")
                .trim_start_matches(agent_name)
                .trim_start_matches(" task=");
            lines.push(format!("  {}. {}", i + 1, content));
        }

        Some(lines.join("\n"))
    }

    // ── Internal helpers ──────────────────────────────────────────────

    /// Extract up to `max_insights` short snippets from `text`.
    fn extract_insights(text: &str, max_insights: usize) -> Vec<String> {
        // Split into sentences and take the first `max_insights` non‑empty,
        // meaningful sentences.
        text.split(|c: char| c == '\n' || c.is_ascii_punctuation())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty() && s.len() > 15 && s.chars().any(|c| c.is_alphanumeric()))
            .take(max_insights)
            .map(|s| {
                // Truncate to 200 chars.
                if s.len() > 200 {
                    format!("{}...", &s[..197])
                } else {
                    s.to_string()
                }
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Global singleton for the agent memory bus (used by chat dispatch wiring)
// ---------------------------------------------------------------------------

/// Lazily-initialised global agent memory bus instance.
///
/// Initialised on first access from `process_chat_request` in `acp/impl/chat.rs`.
/// Uses a default `MemoryStore`; call `AGENT_MEMORY_BUS.get_or_init(|| …)` to
/// provide a custom initialiser.
pub static AGENT_MEMORY_BUS: OnceLock<AgentMemoryBus> = OnceLock::new();

/// Clear all stored memories from the global agent memory bus.
/// Used in test teardown to prevent cross-test contamination.
/// Only available in non-Postgres profiles because the caller is gated.
#[cfg(all(test, not(feature = "backend-postgres")))]
pub fn clear_agent_memory_bus() {
    if let Some(bus) = AGENT_MEMORY_BUS.get() {
        if let Ok(mut store) = bus.store.lock() {
            store.clear();
        }
    }
}

/// Pre-initialize `AGENT_MEMORY_BUS` with a `VectorStore` for similarity search.
///
/// This should be called during server startup (e.g. from `new_acp_server()`) so
/// that `retrieve_memories()` uses vector similarity instead of linear scans.
/// Idempotent — does nothing if the bus was already initialised.
///
/// `user_id` is the default user identifier for multi-user isolation; it can be
/// overridden at call time via the `user_id` parameter on individual methods.
pub fn init_agent_memory_bus_with_vector_store(vs: Arc<VectorStore>, user_id: Option<String>) {
    AGENT_MEMORY_BUS.get_or_init(|| {
        let store = Arc::new(Mutex::new(MemoryStore::new(Default::default())));
        AgentMemoryBus {
            store,
            max_insights_per_task: 5,
            vector_store: Some(vs),
            user_id,
        }
    });
    info!("AgentMemoryBus: pre-initialised with VectorStore for similarity search");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_store_and_retrieve() {
        let bus = AgentMemoryBus::new_default();

        bus.store_insight(
            "agent_a",
            "optimize query",
            "use connection pooling",
            &["sql".to_string()],
            0.8,
            None,
        );

        let results = bus.retrieve_memories("sql", 10, None);
        assert_eq!(results.len(), 1, "should find the stored memory by tag");
    }

    #[test]
    fn test_empty_retrieve() {
        let bus = AgentMemoryBus::new_default();
        let results = bus.retrieve_memories("anything", 10, None);
        assert!(results.is_empty(), "no memories should be found");
    }

    #[test]
    fn test_store_agent_completion_extracts_insights() {
        let bus = AgentMemoryBus::new_default();
        bus.store_agent_completion(
            "test_agent",
            "coding",
            "implement feature X",
            "First, I refactored the cache layer. Then I added connection pooling. Finally, I verified the throughput improved by 2x.",
            true,
            None,
        );
        let results = bus.retrieve_memories("cache", 10, None);
        assert!(!results.is_empty(), "should find memories about cache");
    }

    #[test]
    fn test_retrieve_context_for_agent() {
        let bus = AgentMemoryBus::new_default();

        bus.store_insight(
            "agent_a",
            "fix bug in parser",
            "use lookahead to handle nested expressions",
            &["parser".to_string(), "nested".to_string()],
            0.9,
            None,
        );

        let ctx = bus.retrieve_context_for_agent("agent_a", "coding", "fix parser bug", 5, None);
        assert!(ctx.is_some(), "should return context");
        let ctx_str = ctx.unwrap();
        assert!(
            ctx_str.contains("Previous relevant memories"),
            "context should contain header"
        );
        assert!(
            ctx_str.contains("lookahead"),
            "context should contain the memory content"
        );
    }
}
