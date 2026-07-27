//! Persistent storage for TaskGraph checkpoints and node state.
//!
//! Conditionally compiled:
//! - `backend-sqlite` (local, simple-server): rusqlite-backed
//! - `backend-postgres` (multi-users-server): postgres-backed

use std::collections::HashSet;
#[cfg(not(feature = "backend-postgres"))]
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::task::spawn_blocking;

use anyhow::Result;
use serde_json;

use crate::orchestration::core_dag::{TaskGraph, TaskGraphCheckpointArtifact, TaskNode};

// ─── SQLite backend (local / simple-server) ──────────────────
#[cfg(not(feature = "backend-postgres"))]
use rusqlite::{params, Connection, OptionalExtension};

/// Lock the Mutex, recovering from poison with a warning.
/// Uses shared `crate::lock_or_recover!` macro.
#[cfg(not(feature = "backend-postgres"))]
fn lock_guard(conn: &Mutex<Connection>) -> std::sync::MutexGuard<'_, Connection> {
    match conn.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::warn!("task_graph_store mutex poisoned, recovering");
            poisoned.into_inner()
        }
    }
}

/// SQLite-backed persistent store for task graphs and checkpoints.
#[cfg(not(feature = "backend-postgres"))]
pub struct TaskGraphStore {
    /// SQLite connection (mutex-protected)
    conn: Arc<Mutex<Connection>>,
    /// Path to the database file
    _db_path: PathBuf,
}

#[cfg(not(feature = "backend-postgres"))]
impl TaskGraphStore {
    /// Create or open the task graph store at `db_path`.
    ///
    /// Creates the database file and schema tables if they don't exist.
    pub fn new(db_path: &Path) -> Result<Self> {
        if let Some(parent) = db_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }

        let conn = Connection::open(db_path)?;
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;

            CREATE TABLE IF NOT EXISTS task_graphs (
                graph_id TEXT PRIMARY KEY,
                root_node_id TEXT NOT NULL,
                serialized_graph TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                status TEXT NOT NULL DEFAULT 'active'
            );

            CREATE TABLE IF NOT EXISTS task_checkpoints (
                checkpoint_id TEXT PRIMARY KEY,
                graph_id TEXT NOT NULL,
                serialized_checkpoint TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                FOREIGN KEY (graph_id) REFERENCES task_graphs(graph_id)
            );

            CREATE INDEX IF NOT EXISTS idx_checkpoints_graph_id
                ON task_checkpoints(graph_id);
            CREATE INDEX IF NOT EXISTS idx_task_graphs_status
                ON task_graphs(status);
            ",
        )?;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            _db_path: db_path.to_path_buf(),
        })
    }

    /// Save a task graph, inserting or replacing an existing entry.
    pub async fn save_graph(&self, graph_id: &str, graph: &TaskGraph) -> Result<()> {
        let _permit = crate::shared::db_pool::acquire_db_permit().await;
        let conn = self.conn.clone();
        let graph_id = graph_id.to_string();
        let root_node_id = graph.root.clone();
        let serialized = serde_json::to_string(graph)?;
        spawn_blocking(move || {
            let now = now_ts();
            let conn = lock_guard(&conn);
            conn.execute(
                "INSERT INTO task_graphs (graph_id, root_node_id, serialized_graph, created_at, updated_at, status)
                 VALUES (?1, ?2, ?3, ?4, ?5, 'active')
                 ON CONFLICT(graph_id) DO UPDATE SET
                    root_node_id = excluded.root_node_id,
                    serialized_graph = excluded.serialized_graph,
                    updated_at = excluded.updated_at,
                    status = excluded.status",
                params![graph_id, root_node_id, serialized, now, now],
            )?;
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))?
    }

    /// Load a task graph by its graph_id.
    pub async fn load_graph(&self, graph_id: &str) -> Result<Option<TaskGraph>> {
        let _permit = crate::shared::db_pool::acquire_db_permit().await;
        let conn = self.conn.clone();
        let graph_id = graph_id.to_string();
        spawn_blocking(move || {
            let conn = lock_guard(&conn);
            let mut stmt =
                conn.prepare("SELECT serialized_graph FROM task_graphs WHERE graph_id = ?1")?;
            let result: Option<String> = stmt
                .query_row(params![graph_id], |row| row.get(0))
                .optional()?;
            match result {
                Some(json) => {
                    let graph: TaskGraph = serde_json::from_str(&json)?;
                    Ok(Some(graph))
                }
                None => Ok(None),
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))?
    }

    /// Save a checkpoint artifact, associating it with a graph.
    pub async fn save_checkpoint(
        &self,
        checkpoint: &TaskGraphCheckpointArtifact,
        graph_id: &str,
    ) -> Result<()> {
        let _permit = crate::shared::db_pool::acquire_db_permit().await;
        let conn = self.conn.clone();
        let graph_id = graph_id.to_string();
        let checkpoint_id = checkpoint.checkpoint_id.clone();
        let serialized = serde_json::to_string(checkpoint)?;
        spawn_blocking(move || {
            let now = now_ts();
            let conn = lock_guard(&conn);
            conn.execute(
                "INSERT INTO task_checkpoints (checkpoint_id, graph_id, serialized_checkpoint, created_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(checkpoint_id) DO UPDATE SET
                    graph_id = excluded.graph_id,
                    serialized_checkpoint = excluded.serialized_checkpoint,
                    created_at = excluded.created_at",
                params![checkpoint_id, graph_id, serialized, now],
            )?;
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))?
    }

    /// Load a checkpoint artifact by its checkpoint_id.
    pub async fn load_checkpoint(
        &self,
        checkpoint_id: &str,
    ) -> Result<Option<TaskGraphCheckpointArtifact>> {
        let _permit = crate::shared::db_pool::acquire_db_permit().await;
        let conn = self.conn.clone();
        let checkpoint_id = checkpoint_id.to_string();
        spawn_blocking(move || {
            let conn = lock_guard(&conn);
            let mut stmt = conn.prepare(
                "SELECT serialized_checkpoint FROM task_checkpoints WHERE checkpoint_id = ?1",
            )?;
            let result: Option<String> = stmt
                .query_row(params![checkpoint_id], |row| row.get(0))
                .optional()?;
            match result {
                Some(json) => {
                    let ckpt: TaskGraphCheckpointArtifact = serde_json::from_str(&json)?;
                    Ok(Some(ckpt))
                }
                None => Ok(None),
            }
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))?
    }

    /// List all graph IDs with status 'active'.
    pub async fn list_active_graphs(&self) -> Result<Vec<String>> {
        let _permit = crate::shared::db_pool::acquire_db_permit().await;
        let conn = self.conn.clone();
        spawn_blocking(move || {
            let conn = lock_guard(&conn);
            let mut stmt = conn.prepare(
                "SELECT graph_id FROM task_graphs WHERE status = 'active' ORDER BY updated_at DESC",
            )?;
            let ids: Vec<String> = stmt
                .query_map([], |row| row.get(0))?
                .collect::<std::result::Result<Vec<String>, _>>()?;
            Ok(ids)
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))?
    }

    /// Mark a graph as completed (status = 'completed').
    pub async fn mark_graph_completed(&self, graph_id: &str) -> Result<()> {
        let _permit = crate::shared::db_pool::acquire_db_permit().await;
        let conn = self.conn.clone();
        let graph_id = graph_id.to_string();
        spawn_blocking(move || {
            let now = now_ts();
            let conn = lock_guard(&conn);
            conn.execute(
                "UPDATE task_graphs SET status = 'completed', updated_at = ?1 WHERE graph_id = ?2",
                params![now, graph_id],
            )?;
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))?
    }

    /// Delete a graph and all its associated checkpoints.
    pub async fn delete_graph(&self, graph_id: &str) -> Result<()> {
        let _permit = crate::shared::db_pool::acquire_db_permit().await;
        let conn = self.conn.clone();
        let graph_id = graph_id.to_string();
        spawn_blocking(move || {
            let conn = lock_guard(&conn);
            conn.execute(
                "DELETE FROM task_checkpoints WHERE graph_id = ?1",
                params![graph_id],
            )?;
            conn.execute(
                "DELETE FROM task_graphs WHERE graph_id = ?1",
                params![graph_id],
            )?;
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!("spawn_blocking join error: {e}"))?
    }

    /// Load a checkpoint and reconstruct a TaskGraph from its subtask_records.
    ///
    /// Each subtask record is converted back into a TaskNode.  The graph is
    /// rebuilt with a synthetic root node.  Dependencies are restored from
    /// each record; nodes with no stored dependencies fall back to depending
    /// on the root.
    pub async fn restore_graph_from_checkpoint(
        &self,
        checkpoint_id: &str,
    ) -> Result<Option<TaskGraph>> {
        let ckpt = match self.load_checkpoint(checkpoint_id).await? {
            Some(c) => c,
            None => return Ok(None),
        };

        let root_id = format!("restored-root-{}", ckpt.checkpoint_id);
        let root_node = TaskNode {
            id: root_id.clone(),
            kind: "restored_root".to_string(),
            state: "done".to_string(),
            input: serde_json::json!({
                "task": ckpt.task,
                "phases_completed": ckpt.phases_completed,
                "restored_from_checkpoint": ckpt.checkpoint_id,
            }),
            output: None,
            dependencies: HashSet::new(),
            retries: 0,
        };

        let mut graph = TaskGraph::new(root_node);

        for record in &ckpt.subtask_records {
            let node_id = record.subtask_id.clone();
            let node_state = match record.outcome.as_deref() {
                Some("completed") | Some("success") => "done".to_string(),
                Some("failed") => "failed".to_string(),
                _ => "pending".to_string(),
            };
            // Build dependency set from stored dependencies (fall back to root
            // for backward compatibility with checkpoints that lack this field).
            let dependencies: HashSet<String> = if record.dependencies.is_empty() {
                HashSet::from([root_id.clone()])
            } else {
                record.dependencies.iter().cloned().collect()
            };
            let node = TaskNode {
                id: node_id.clone(),
                kind: record.phase.clone(),
                state: node_state,
                input: serde_json::json!({
                    "description": record.description,
                }),
                output: record
                    .result_summary
                    .clone()
                    .map(|s| serde_json::json!({ "summary": s })),
                dependencies: dependencies.clone(),
                retries: 0,
            };
            graph.add_node(node);
            for dep_id in &dependencies {
                let _ = graph.add_edge(dep_id.clone(), node_id.clone());
            }
        }

        Ok(Some(graph))
    }
}

// ─── Postgres backend (multi-users-server) ──────────────────────────
#[cfg(feature = "backend-postgres")]
use postgres::{Client, NoTls};

/// Lock the Mutex, recovering from poison with a warning.
#[cfg(feature = "backend-postgres")]
fn pg_lock_guard(conn: &Mutex<Client>) -> std::sync::MutexGuard<'_, Client> {
    match conn.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::error!("task_graph_store (pg) mutex poisoned, recovering");
            poisoned.into_inner()
        }
    }
}

/// Postgres-backed persistent store for task graphs and checkpoints.
#[cfg(feature = "backend-postgres")]
pub struct TaskGraphStore {
    /// Postgres client connection (mutex-protected)
    client: Mutex<Client>,
    /// Path to the database file (used as a label / connection identifier)
    _db_path: PathBuf,
}

#[cfg(feature = "backend-postgres")]
impl TaskGraphStore {
    /// Connect to PostgreSQL and run schema migrations.
    ///
    /// `conn_str` — libpq-style connection string, e.g.
    /// `"postgres://user:pass@localhost/go_on"`
    pub fn new(conn_str: &str) -> Result<Self> {
        let mut client = Client::connect(conn_str, NoTls)?;
        client.batch_execute(
            "CREATE TABLE IF NOT EXISTS task_graphs (
                graph_id        TEXT    PRIMARY KEY,
                root_node_id    TEXT    NOT NULL,
                serialized_graph TEXT   NOT NULL,
                created_at      BIGINT  NOT NULL,
                updated_at      BIGINT  NOT NULL,
                status          TEXT    NOT NULL DEFAULT 'active'
            );

            CREATE TABLE IF NOT EXISTS task_checkpoints (
                checkpoint_id          TEXT    PRIMARY KEY,
                graph_id               TEXT    NOT NULL,
                serialized_checkpoint  TEXT    NOT NULL,
                created_at             BIGINT  NOT NULL,
                FOREIGN KEY (graph_id) REFERENCES task_graphs(graph_id)
            );

            CREATE INDEX IF NOT EXISTS idx_checkpoints_graph_id
                ON task_checkpoints(graph_id);
            CREATE INDEX IF NOT EXISTS idx_task_graphs_status
                ON task_graphs(status);",
        )?;

        Ok(Self {
            client: Mutex::new(client),
            _db_path: PathBuf::from(conn_str),
        })
    }

    /// Save a task graph, inserting or replacing an existing entry.
    pub fn save_graph(&self, graph_id: &str, graph: &TaskGraph) -> Result<()> {
        let now = now_ts();
        let serialized = serde_json::to_string(graph)?;
        let mut client = pg_lock_guard(&self.client);
        client.execute(
            "INSERT INTO task_graphs (graph_id, root_node_id, serialized_graph, created_at, updated_at, status)
             VALUES ($1, $2, $3, $4, $5, 'active')
             ON CONFLICT (graph_id) DO UPDATE SET
                root_node_id = EXCLUDED.root_node_id,
                serialized_graph = EXCLUDED.serialized_graph,
                updated_at = EXCLUDED.updated_at,
                status = EXCLUDED.status",
            &[&graph_id, &graph.root, &serialized, &now, &now],
        )?;
        Ok(())
    }

    /// Load a task graph by its graph_id.
    pub fn load_graph(&self, graph_id: &str) -> Result<Option<TaskGraph>> {
        let mut client = pg_lock_guard(&self.client);
        let rows = client.query(
            "SELECT serialized_graph FROM task_graphs WHERE graph_id = $1",
            &[&graph_id],
        )?;
        match rows.first() {
            Some(row) => {
                let json: String = row.get(0);
                let graph: TaskGraph = serde_json::from_str(&json)?;
                Ok(Some(graph))
            }
            None => Ok(None),
        }
    }

    /// Save a checkpoint artifact, associating it with a graph.
    pub fn save_checkpoint(
        &self,
        checkpoint: &TaskGraphCheckpointArtifact,
        graph_id: &str,
    ) -> Result<()> {
        let now = now_ts();
        let serialized = serde_json::to_string(checkpoint)?;
        let mut client = pg_lock_guard(&self.client);
        client.execute(
            "INSERT INTO task_checkpoints (checkpoint_id, graph_id, serialized_checkpoint, created_at)
             VALUES ($1, $2, $3, $4)
             ON CONFLICT (checkpoint_id) DO UPDATE SET
                graph_id = EXCLUDED.graph_id,
                serialized_checkpoint = EXCLUDED.serialized_checkpoint,
                created_at = EXCLUDED.created_at",
            &[&checkpoint.checkpoint_id, &graph_id, &serialized, &now],
        )?;
        Ok(())
    }

    /// Load a checkpoint artifact by its checkpoint_id.
    pub fn load_checkpoint(
        &self,
        checkpoint_id: &str,
    ) -> Result<Option<TaskGraphCheckpointArtifact>> {
        let mut client = pg_lock_guard(&self.client);
        let rows = client.query(
            "SELECT serialized_checkpoint FROM task_checkpoints WHERE checkpoint_id = $1",
            &[&checkpoint_id],
        )?;
        match rows.first() {
            Some(row) => {
                let json: String = row.get(0);
                let ckpt: TaskGraphCheckpointArtifact = serde_json::from_str(&json)?;
                Ok(Some(ckpt))
            }
            None => Ok(None),
        }
    }

    /// List all graph IDs with status 'active'.
    pub fn list_active_graphs(&self) -> Result<Vec<String>> {
        let mut client = pg_lock_guard(&self.client);
        let rows = client.query(
            "SELECT graph_id FROM task_graphs WHERE status = 'active' ORDER BY updated_at DESC",
            &[],
        )?;
        let ids = rows.iter().map(|row| row.get::<_, String>(0)).collect();
        Ok(ids)
    }

    /// Mark a graph as completed (status = 'completed').
    pub fn mark_graph_completed(&self, graph_id: &str) -> Result<()> {
        let now = now_ts();
        let mut client = pg_lock_guard(&self.client);
        client.execute(
            "UPDATE task_graphs SET status = 'completed', updated_at = $1 WHERE graph_id = $2",
            &[&now, &graph_id],
        )?;
        Ok(())
    }

    /// Delete a graph and all its associated checkpoints.
    pub fn delete_graph(&self, graph_id: &str) -> Result<()> {
        let mut client = pg_lock_guard(&self.client);
        client.execute(
            "DELETE FROM task_checkpoints WHERE graph_id = $1",
            &[&graph_id],
        )?;
        client.execute("DELETE FROM task_graphs WHERE graph_id = $1", &[&graph_id])?;
        Ok(())
    }

    /// Load a checkpoint and reconstruct a TaskGraph from its subtask_records.
    ///
    /// Each subtask record is converted back into a TaskNode.  The graph is
    /// rebuilt with a synthetic root node.  Dependencies are restored from
    /// each record; nodes with no stored dependencies fall back to depending
    /// on the root.
    pub fn restore_graph_from_checkpoint(&self, checkpoint_id: &str) -> Result<Option<TaskGraph>> {
        let ckpt = match self.load_checkpoint(checkpoint_id)? {
            Some(c) => c,
            None => return Ok(None),
        };

        let root_id = format!("restored-root-{}", ckpt.checkpoint_id);
        let root_node = TaskNode {
            id: root_id.clone(),
            kind: "restored_root".to_string(),
            state: "done".to_string(),
            input: serde_json::json!({
                "task": ckpt.task,
                "phases_completed": ckpt.phases_completed,
                "restored_from_checkpoint": ckpt.checkpoint_id,
            }),
            output: None,
            dependencies: HashSet::new(),
            retries: 0,
        };

        let mut graph = TaskGraph::new(root_node);

        for record in &ckpt.subtask_records {
            let node_id = record.subtask_id.clone();
            let node_state = match record.outcome.as_deref() {
                Some("completed") | Some("success") => "done".to_string(),
                Some("failed") => "failed".to_string(),
                _ => "pending".to_string(),
            };
            // Build dependency set from stored dependencies (fall back to root
            // for backward compatibility with checkpoints that lack this field).
            let dependencies: HashSet<String> = if record.dependencies.is_empty() {
                HashSet::from([root_id.clone()])
            } else {
                record.dependencies.iter().cloned().collect()
            };
            let node = TaskNode {
                id: node_id.clone(),
                kind: record.phase.clone(),
                state: node_state,
                input: serde_json::json!({
                    "description": record.description,
                }),
                output: record
                    .result_summary
                    .clone()
                    .map(|s| serde_json::json!({ "summary": s })),
                dependencies: dependencies.clone(),
                retries: 0,
            };
            graph.add_node(node);
            for dep_id in &dependencies {
                let _ = graph.add_edge(dep_id.clone(), node_id.clone());
            }
        }

        Ok(Some(graph))
    }
}

// ─── Shared helpers (both backends) ─────────────────────────────────────────

fn now_ts() -> i64 {
    crate::shared::timestamps::now_ts()
}

// ─── Tests (SQLite only) ────────────────────────────────────────────────────

#[cfg(all(test, not(feature = "backend-postgres")))]
mod tests {
    use super::*;
    use crate::orchestration::core_dag::PlannedSubtaskRecord;

    fn make_sample_graph() -> (String, TaskGraph) {
        let graph_id = "test-graph-001".to_string();
        let root = TaskNode {
            id: "root".to_string(),
            kind: "plan".to_string(),
            state: "done".to_string(),
            input: serde_json::json!({ "task": "test task" }),
            output: Some(serde_json::json!({ "result": "ok" })),
            dependencies: HashSet::new(),
            retries: 0,
        };
        let mut graph = TaskGraph::new(root);

        let child = TaskNode {
            id: "child-1".to_string(),
            kind: "edit".to_string(),
            state: "done".to_string(),
            input: serde_json::json!({ "step": 1 }),
            output: Some(serde_json::json!({ "result": "done" })),
            dependencies: HashSet::from(["root".to_string()]),
            retries: 0,
        };
        graph.add_node(child);
        let _ = graph.add_edge("root".to_string(), "child-1".to_string());

        (graph_id, graph)
    }

    fn make_sample_checkpoint(graph: &TaskGraph) -> TaskGraphCheckpointArtifact {
        let records: Vec<PlannedSubtaskRecord> = graph
            .nodes
            .values()
            .filter(|n| n.id != graph.root)
            .map(|n| PlannedSubtaskRecord {
                subtask_id: n.id.clone(),
                description: format!("subtask {}", n.id),
                phase: n.kind.clone(),
                outcome: Some(
                    if n.state == "done" {
                        "completed"
                    } else {
                        "pending"
                    }
                    .to_string(),
                ),
                result_summary: n.output.as_ref().map(|o| o.to_string()),
                dependencies: n.dependencies.iter().cloned().collect(),
            })
            .collect();

        graph.snapshot("test task", 1, records)
    }

    #[tokio::test]
    async fn test_save_and_load_graph() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let db_path = dir.path().join("task_graphs.sqlite3");
        let store = TaskGraphStore::new(&db_path).expect("store should initialize");

        let (graph_id, graph) = make_sample_graph();
        store
            .save_graph(&graph_id, &graph)
            .await
            .expect("save_graph should succeed");

        let loaded = store
            .load_graph(&graph_id)
            .await
            .expect("load_graph should succeed")
            .expect("graph should exist");

        assert_eq!(loaded.nodes.len(), graph.nodes.len());
        assert_eq!(loaded.root, graph.root);
        for (id, node) in &graph.nodes {
            let loaded_node = loaded.nodes.get(id).expect("node should exist");
            assert_eq!(loaded_node.state, node.state);
            assert_eq!(loaded_node.kind, node.kind);
        }
    }

    #[tokio::test]
    async fn test_save_and_load_checkpoint() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let db_path = dir.path().join("task_graphs.sqlite3");
        let store = TaskGraphStore::new(&db_path).expect("store should initialize");

        let (graph_id, graph) = make_sample_graph();
        store
            .save_graph(&graph_id, &graph)
            .await
            .expect("save_graph should succeed");

        let checkpoint = make_sample_checkpoint(&graph);
        store
            .save_checkpoint(&checkpoint, &graph_id)
            .await
            .expect("save_checkpoint should succeed");

        let loaded = store
            .load_checkpoint(&checkpoint.checkpoint_id)
            .await
            .expect("load_checkpoint should succeed")
            .expect("checkpoint should exist");

        assert_eq!(loaded.checkpoint_id, checkpoint.checkpoint_id);
        assert_eq!(loaded.task, checkpoint.task);
        assert_eq!(loaded.phases_completed, checkpoint.phases_completed);
        assert_eq!(
            loaded.subtask_records.len(),
            checkpoint.subtask_records.len()
        );
    }

    #[tokio::test]
    async fn test_restore_graph_from_checkpoint() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let db_path = dir.path().join("task_graphs.sqlite3");
        let store = TaskGraphStore::new(&db_path).expect("store should initialize");

        let (graph_id, graph) = make_sample_graph();
        store
            .save_graph(&graph_id, &graph)
            .await
            .expect("save_graph should succeed");

        let checkpoint = make_sample_checkpoint(&graph);
        store
            .save_checkpoint(&checkpoint, &graph_id)
            .await
            .expect("save_checkpoint should succeed");

        let restored = store
            .restore_graph_from_checkpoint(&checkpoint.checkpoint_id)
            .await
            .expect("restore_graph_from_checkpoint should succeed")
            .expect("restored graph should exist");

        // The restored graph has a synthetic root + one node per subtask record
        assert_eq!(restored.nodes.len(), 1 + checkpoint.subtask_records.len());
        assert!(restored.nodes.contains_key(&restored.root));

        // Verify that each subtask record produced a node in the graph
        for record in &checkpoint.subtask_records {
            let node = restored.nodes.get(&record.subtask_id).unwrap_or_else(|| {
                panic!("node {} should exist in restored graph", record.subtask_id)
            });
            assert_eq!(node.kind, record.phase);
        }
    }

    #[tokio::test]
    async fn test_list_active_graphs() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let db_path = dir.path().join("task_graphs.sqlite3");
        let store = TaskGraphStore::new(&db_path).expect("store should initialize");

        let (gid1, g1) = make_sample_graph();
        let (_, mut g2) = make_sample_graph();
        g2.root = "root-2".to_string();
        let gid2 = "test-graph-002".to_string();

        store.save_graph(&gid1, &g1).await.expect("save_graph 1");
        store.save_graph(&gid2, &g2).await.expect("save_graph 2");

        let active = store
            .list_active_graphs()
            .await
            .expect("list_active_graphs");
        assert!(active.contains(&gid1));
        assert!(active.contains(&gid2));
    }

    #[tokio::test]
    async fn test_mark_completed_and_list() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let db_path = dir.path().join("task_graphs.sqlite3");
        let store = TaskGraphStore::new(&db_path).expect("store should initialize");

        let (graph_id, graph) = make_sample_graph();
        store
            .save_graph(&graph_id, &graph)
            .await
            .expect("save_graph should succeed");

        store
            .mark_graph_completed(&graph_id)
            .await
            .expect("mark_graph_completed should succeed");

        let active = store
            .list_active_graphs()
            .await
            .expect("list_active_graphs");
        assert!(!active.contains(&graph_id));
    }

    #[tokio::test]
    async fn test_delete_graph_cascades_checkpoints() {
        let dir = tempfile::tempdir().expect("temp dir should be created");
        let db_path = dir.path().join("task_graphs.sqlite3");
        let store = TaskGraphStore::new(&db_path).expect("store should initialize");

        let (graph_id, graph) = make_sample_graph();
        store
            .save_graph(&graph_id, &graph)
            .await
            .expect("save_graph should succeed");

        let checkpoint = make_sample_checkpoint(&graph);
        store
            .save_checkpoint(&checkpoint, &graph_id)
            .await
            .expect("save_checkpoint should succeed");

        // Delete the graph (should cascade to checkpoints)
        store
            .delete_graph(&graph_id)
            .await
            .expect("delete_graph should succeed");

        // Graph should no longer exist
        let loaded_graph = store
            .load_graph(&graph_id)
            .await
            .expect("load_graph should succeed");
        assert!(loaded_graph.is_none());

        // Checkpoint should also be gone
        let loaded_ckpt = store
            .load_checkpoint(&checkpoint.checkpoint_id)
            .await
            .expect("load_checkpoint should succeed");
        assert!(loaded_ckpt.is_none());
    }
}
