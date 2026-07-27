//! AgentGraphStore — persistence trait for agent thread graph (BLUE71 §8)
//!
//! Provides a storage abstraction for the agent thread relationship graph.
//! The `InMemoryAgentGraphStore` is the default implementation; a SQLite-backed
//! version can be added later using the same trait.
//!
//! Architecture:
//! - `AgentGraphStore` trait — storage operations for the agent tree
//! - `InMemoryAgentGraphStore` — HashMap-based default implementation
//! - `AgentGraphEdge` — a single edge in the agent relationship graph

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::agents::communication::path::AgentPath;

// ---------------------------------------------------------------------------
// AgentGraphEdge — a relationship between parent and child agents
// ---------------------------------------------------------------------------

/// A directed edge from a parent agent to a child agent in the tree.
#[derive(Debug, Clone)]
pub struct AgentGraphEdge {
    /// Child agent path.
    pub child: AgentPath,
    /// Current status of the child agent.
    pub status: String,
}

// ---------------------------------------------------------------------------
// AgentGraphStore trait — persistence abstraction
// ---------------------------------------------------------------------------

/// Storage abstraction for the agent thread relationship graph (BLUE71 §8).
///
/// Implementations can be in-memory (for testing/development) or
/// SQLite-backed (for production persistence and recovery).
#[async_trait::async_trait]
pub trait AgentGraphStore: Send + Sync {
    /// Insert or update a parent→child edge.
    async fn upsert_edge(&self, edge: AgentGraphEdge);

    /// Update the status of a specific child.
    async fn set_edge_status(&self, child_path: &AgentPath, status: &str);
}

// ---------------------------------------------------------------------------
// InMemoryAgentGraphStore — default implementation
// ---------------------------------------------------------------------------

/// HashMap-based in-memory implementation of AgentGraphStore.
///
/// Uses `Arc<RwLock<HashMap>>` for thread-safe concurrent access.
/// Suitable for development and testing; replace with SQLite for production.
#[derive(Debug, Clone)]
pub struct InMemoryAgentGraphStore {
    edges: Arc<RwLock<HashMap<String, AgentGraphEdge>>>,
}

impl InMemoryAgentGraphStore {
    /// Create an empty in-memory store.
    pub fn new() -> Self {
        Self {
            edges: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for InMemoryAgentGraphStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl AgentGraphStore for InMemoryAgentGraphStore {
    async fn upsert_edge(&self, edge: AgentGraphEdge) {
        let key = edge.child.to_string();
        self.edges.write().await.insert(key, edge);
    }

    async fn set_edge_status(&self, child_path: &AgentPath, status: &str) {
        let key = child_path.to_string();
        let mut guard = self.edges.write().await;
        if let Some(edge) = guard.get_mut(&key) {
            edge.status = status.to_string();
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_path(s: &str) -> AgentPath {
        AgentPath::parse(s).unwrap()
    }

    fn make_edge(child: &str, status: &str) -> AgentGraphEdge {
        AgentGraphEdge {
            child: make_path(child),
            status: status.to_string(),
        }
    }

    #[tokio::test]
    async fn test_upsert_edge() {
        let store = InMemoryAgentGraphStore::new();
        let edge = make_edge("root/a", "running");
        store.upsert_edge(edge).await;

        // Verify by reading internal state
        let guard = store.edges.read().await;
        let stored = guard.get("root/a").unwrap();
        assert_eq!(stored.child.to_string(), "root/a");
        assert_eq!(stored.status, "running");
    }

    #[tokio::test]
    async fn test_set_edge_status() {
        let store = InMemoryAgentGraphStore::new();
        store
            .upsert_edge(make_edge("root/a", "running"))
            .await;

        store
            .set_edge_status(&make_path("root/a"), "completed")
            .await;

        // Verify by reading internal state
        let guard = store.edges.read().await;
        let stored = guard.get("root/a").unwrap();
        assert_eq!(stored.status, "completed");
    }

    #[tokio::test]
    async fn test_upsert_overwrites_existing() {
        let store = InMemoryAgentGraphStore::new();
        store
            .upsert_edge(make_edge("root/a", "running"))
            .await;
        store
            .upsert_edge(make_edge("root/a", "completed"))
            .await;

        let guard = store.edges.read().await;
        assert_eq!(guard.len(), 1);
        let edge = guard.get("root/a").unwrap();
        assert_eq!(edge.status, "completed");
    }
}
