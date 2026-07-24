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
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::agents::communication::path::AgentPath;

// ---------------------------------------------------------------------------
// AgentGraphEdge — a relationship between parent and child agents
// ---------------------------------------------------------------------------

/// A directed edge from a parent agent to a child agent in the tree.
#[derive(Debug, Clone)]
pub struct AgentGraphEdge {
    /// Parent agent path.
    pub parent: AgentPath,
    /// Child agent path.
    pub child: AgentPath,
    /// Current status of the child agent.
    pub status: String,
    /// Optional child agent name.
    pub child_name: Option<String>,
    /// Optional task description.
    pub task: Option<String>,
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

    /// List all descendant edges reachable from the given path (BFS).
    async fn list_descendants(&self, parent_path: &AgentPath) -> Vec<AgentGraphEdge>;

    /// Remove an edge and all its descendants from the store.
    async fn remove_subtree(&self, parent_path: &AgentPath);
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

    /// Number of edges currently stored.
    pub async fn len(&self) -> usize {
        self.edges.read().await.len()
    }

    /// Whether the store is empty.
    pub async fn is_empty(&self) -> bool {
        self.edges.read().await.is_empty()
    }

    /// Get a specific edge by child path.
    pub async fn get(&self, child_path: &AgentPath) -> Option<AgentGraphEdge> {
        self.edges
            .read()
            .await
            .get(&child_path.to_string())
            .cloned()
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

    async fn list_descendants(&self, parent_path: &AgentPath) -> Vec<AgentGraphEdge> {
        let guard = self.edges.read().await;
        let parent_str = parent_path.to_string();
        // BFS: find all edges where parent starts with parent_str
        guard
            .values()
            .filter(|e| {
                let p = e.parent.to_string();
                p == parent_str || p.starts_with(&format!("{}/", parent_str))
            })
            .cloned()
            .collect()
    }

    async fn remove_subtree(&self, parent_path: &AgentPath) {
        let parent_str = parent_path.to_string();
        let mut guard = self.edges.write().await;
        guard.retain(|_, e| {
            let p = e.parent.to_string();
            p != parent_str && !p.starts_with(&format!("{}/", parent_str))
        });
    }
}

// ---------------------------------------------------------------------------
// SqliteAgentGraphStore — SQLite-backed persistence (feature: backend-sqlite)
// ---------------------------------------------------------------------------

/// SQLite-backed implementation of AgentGraphStore.
///
/// Requires `backend-sqlite` feature. Follows the same pattern as
/// `acp::session_persistence::SessionStore`: `rusqlite::Connection` behind
/// `Arc<Mutex<>>`, all DB operations via `spawn_blocking`.
///
/// Schema: `agent_graph_edges` table with parent/child/status/child_name/task columns.
/// Checkpoint data is stored in the `task` column as JSON.
#[cfg(feature = "backend-sqlite")]
pub struct SqliteAgentGraphStore {
    conn: Arc<std::sync::Mutex<rusqlite::Connection>>,
}

#[cfg(feature = "backend-sqlite")]
impl SqliteAgentGraphStore {
    /// Open (or create) the agent graph database at `db_path`.
    pub async fn open(db_path: &Path) -> Result<Self, String> {
        let path = db_path.to_path_buf();
        let conn = tokio::task::spawn_blocking(move || {
            let conn = rusqlite::Connection::open(&path)
                .map_err(|e| format!("failed to open graph store db: {}", e))?;
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS agent_graph_edges (
                    child_path TEXT PRIMARY KEY,
                    parent_path TEXT NOT NULL,
                    status TEXT NOT NULL DEFAULT 'pending',
                    child_name TEXT,
                    task TEXT
                );",
            )
            .map_err(|e| format!("failed to create graph store table: {}", e))?;
            Ok::<_, String>(conn)
        })
        .await
        .map_err(|e| format!("spawn_blocking failed: {}", e))??;
        Ok(Self {
            conn: Arc::new(std::sync::Mutex::new(conn)),
        })
    }
}

#[cfg(feature = "backend-sqlite")]
#[async_trait::async_trait]
impl AgentGraphStore for SqliteAgentGraphStore {
    async fn upsert_edge(&self, edge: AgentGraphEdge) {
        let child = edge.child.to_string();
        let parent = edge.parent.to_string();
        let status = edge.status;
        let child_name = edge.child_name;
        let task = edge.task;
        let conn = self.conn.clone();
        let _ = tokio::task::spawn_blocking(move || {
            conn.lock().unwrap().execute(
                "INSERT INTO agent_graph_edges (child_path, parent_path, status, child_name, task)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(child_path) DO UPDATE SET
                    parent_path=excluded.parent_path,
                    status=excluded.status,
                    child_name=excluded.child_name,
                    task=excluded.task",
                rusqlite::params![child, parent, status, child_name, task],
            )
        })
        .await;
    }

    async fn set_edge_status(&self, child_path: &AgentPath, status: &str) {
        let key = child_path.to_string();
        let s = status.to_string();
        let conn = self.conn.clone();
        let _ = tokio::task::spawn_blocking(move || {
            conn.lock().unwrap().execute(
                "UPDATE agent_graph_edges SET status = ?1 WHERE child_path = ?2",
                rusqlite::params![s, key],
            )
        })
        .await;
    }

    async fn list_descendants(&self, parent_path: &AgentPath) -> Vec<AgentGraphEdge> {
        let parent_str = parent_path.to_string();
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let guard = conn.lock().unwrap();
            let mut stmt = guard
                .prepare(
                    "SELECT child_path, parent_path, status, child_name, task
                 FROM agent_graph_edges
                 WHERE parent_path = ?1 OR parent_path LIKE ?2",
                )
                .ok()?;
            let pattern = format!("{}/%", parent_str);
            let rows = stmt
                .query_map(rusqlite::params![parent_str, pattern], |row| {
                    let child_str: String = row.get(0)?;
                    let parent_str: String = row.get(1)?;
                    Ok(AgentGraphEdge {
                        child: AgentPath::parse(&child_str)
                            .unwrap_or_else(|_| AgentPath::parse("unknown").unwrap()),
                        parent: AgentPath::parse(&parent_str)
                            .unwrap_or_else(|_| AgentPath::parse("unknown").unwrap()),
                        status: row.get(2)?,
                        child_name: row.get(3)?,
                        task: row.get(4)?,
                    })
                })
                .ok()?;
            Some(rows.filter_map(|r| r.ok()).collect::<Vec<_>>())
        })
        .await
        .unwrap_or_default()
        .unwrap_or_default()
    }

    async fn remove_subtree(&self, parent_path: &AgentPath) {
        let parent_str = parent_path.to_string();
        let pattern = format!("{}/%", parent_str);
        let conn = self.conn.clone();
        let _ = tokio::task::spawn_blocking(move || {
            conn.lock().unwrap().execute(
                "DELETE FROM agent_graph_edges WHERE parent_path = ?1 OR parent_path LIKE ?2",
                rusqlite::params![parent_str, pattern],
            )
        })
        .await;
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

    fn make_edge(parent: &str, child: &str, status: &str) -> AgentGraphEdge {
        AgentGraphEdge {
            parent: make_path(parent),
            child: make_path(child),
            status: status.to_string(),
            child_name: None,
            task: None,
        }
    }

    #[tokio::test]
    async fn test_empty_store() {
        let store = InMemoryAgentGraphStore::new();
        assert!(store.is_empty().await);
        assert_eq!(store.len().await, 0);
    }

    #[tokio::test]
    async fn test_upsert_and_list() {
        let store = InMemoryAgentGraphStore::new();
        let edge = make_edge("root", "root/a", "running");
        store.upsert_edge(edge).await;
        assert_eq!(store.len().await, 1);
        assert!(!store.is_empty().await);

        let descendants = store.list_descendants(&make_path("root")).await;
        assert_eq!(descendants.len(), 1);
        assert_eq!(descendants[0].child.to_string(), "root/a");
    }

    #[tokio::test]
    async fn test_set_edge_status() {
        let store = InMemoryAgentGraphStore::new();
        let edge = make_edge("root", "root/a", "running");
        store.upsert_edge(edge).await;

        store
            .set_edge_status(&make_path("root/a"), "completed")
            .await;

        let updated = store.get(&make_path("root/a")).await.unwrap();
        assert_eq!(updated.status, "completed");
    }

    #[tokio::test]
    async fn test_list_descendants_bfs() {
        let store = InMemoryAgentGraphStore::new();
        store
            .upsert_edge(make_edge("root", "root/a", "running"))
            .await;
        store
            .upsert_edge(make_edge("root/a", "root/a/a1", "running"))
            .await;
        store
            .upsert_edge(make_edge("root/a", "root/a/a2", "running"))
            .await;
        store
            .upsert_edge(make_edge("root", "root/b", "running"))
            .await;

        // Root's descendants should include a, a/a1, a/a2, b
        let root_desc = store.list_descendants(&make_path("root")).await;
        assert_eq!(root_desc.len(), 4);

        // a's descendants should include a1, a2
        let a_desc = store.list_descendants(&make_path("root/a")).await;
        assert_eq!(a_desc.len(), 2);
    }

    #[tokio::test]
    async fn test_remove_subtree() {
        let store = InMemoryAgentGraphStore::new();
        store
            .upsert_edge(make_edge("root", "root/a", "running"))
            .await;
        store
            .upsert_edge(make_edge("root/a", "root/a/a1", "running"))
            .await;
        store
            .upsert_edge(make_edge("root", "root/b", "running"))
            .await;

        store.remove_subtree(&make_path("root/a")).await;

        let descendants = store.list_descendants(&make_path("root")).await;
        assert_eq!(descendants.len(), 1);
        assert_eq!(descendants[0].child.to_string(), "root/b");
    }

    #[tokio::test]
    async fn test_upsert_overwrites_existing() {
        let store = InMemoryAgentGraphStore::new();
        store
            .upsert_edge(make_edge("root", "root/a", "running"))
            .await;
        store
            .upsert_edge(make_edge("root", "root/a", "completed"))
            .await;

        assert_eq!(store.len().await, 1);
        let edge = store.get(&make_path("root/a")).await.unwrap();
        assert_eq!(edge.status, "completed");
    }

    #[tokio::test]
    async fn test_get_nonexistent_returns_none() {
        let store = InMemoryAgentGraphStore::new();
        let result = store.get(&make_path("root/nonexistent")).await;
        assert!(result.is_none());
    }
}
