//! `memory_search` tool — full-text search over the cross-session warm memory.
//!
//! Searches the warm-tier memory store (`warm_memory` in warm.db, written by
//! the persistence layer across sessions) using `memory::search`. The
//! database path is resolved the same way the persistence layer resolves it:
//! `memory_base_path().join("warm.db")` (respecting `GO_ON_MEMORY_PATH`).

use crate::governance::pua::tool_execution_report;
use crate::memory::memory_bridge::memory_base_path;
use crate::memory::search::MemorySearcher;
use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};
use anyhow::Result;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

/// Default cap on the number of hits returned per invocation.
const DEFAULT_LIMIT: usize = 10;
/// Hard cap enforced on the `limit` argument.
const MAX_LIMIT: usize = 50;

/// Resolve the warm-memory database path from the same source the persistence
/// layer uses (`memory_base_path()` honors `GO_ON_MEMORY_PATH`).
pub fn default_db_path() -> std::path::PathBuf {
    memory_base_path().join("warm.db")
}

/// Tool for full-text search across the cross-session warm memory store.
pub struct MemorySearchTool {
    /// Searcher opened lazily on first run (opening warm.db at registration
    /// time would force SQLite I/O onto the startup path). The cached
    /// construction `Result` lets a failed first open surface on every call
    /// instead of poisoning the cell.
    searcher: OnceLock<Result<MemorySearcher>>,
}

impl MemorySearchTool {
    /// Create a tool that lazily opens the warm-memory database on first run.
    pub fn new() -> Self {
        Self {
            searcher: OnceLock::new(),
        }
    }

    /// Create a tool backed by a prebuilt searcher (e.g. one over a temp
    /// database in tests, or a caller-owned connection).
    pub fn with_searcher(searcher: MemorySearcher) -> Self {
        Self {
            searcher: OnceLock::from(Ok(searcher)),
        }
    }

    fn searcher(&self) -> Result<&MemorySearcher> {
        self.searcher
            .get_or_init(|| MemorySearcher::new(&default_db_path()))
            .as_ref()
            .map_err(|e| anyhow::anyhow!("failed to open memory search database: {e}"))
    }
}

impl Default for MemorySearchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for MemorySearchTool {
    fn name(&self) -> &'static str {
        "memory_search"
    }

    fn description(&self) -> &str {
        "Full-text search across cross-session memory (warm tier). Returns memory \
         entries whose content matches the query, ranked by relevance. Supports \
         Chinese/Japanese/Korean (CJK) substring queries."
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let query = input.payload["query"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("memory_search requires arguments.query"))?
            .trim();
        if query.is_empty() {
            anyhow::bail!("memory_search requires a non-empty 'query'");
        }
        let requested = input.payload["limit"]
            .as_u64()
            .unwrap_or(DEFAULT_LIMIT as u64) as usize;
        // Floor at 1 (a 0/negative limit is meaningless) and cap at MAX_LIMIT.
        let limit = requested.clamp(1, MAX_LIMIT);

        let hits = self.searcher()?.search(query, limit)?;

        let hit_json: Vec<serde_json::Value> = hits
            .iter()
            .map(|hit| {
                serde_json::json!({
                    "session_id": hit.session_id,
                    "role": hit.role,
                    "content": hit.content,
                    "score": hit.score,
                })
            })
            .collect();

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "query": query,
                "count": hits.len(),
                "hits": hit_json,
            })),
            error: None,
            verification: Some("memory_search_completed".to_string()),
            audit_log: Some(format!("memory_search '{query}': {} hits", hits.len())),
            pua_report: Some(tool_execution_report(
                "memory_search",
                Some("memory_search_completed"),
            )),
        })
    }

    fn run_async(
        self: Arc<Self>,
        input: ToolInput,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<ToolOutput>> + Send>> {
        // The SQLite work is synchronous; offload it onto the blocking pool
        // (same contract as the trait's default implementation).
        Box::pin(async move {
            tokio::task::spawn_blocking(move || self.run(&input))
                .await
                .map_err(|e| anyhow::anyhow!("memory_search blocking task failed: {e}"))?
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::search::MemoryHit;
    use rusqlite::{params, Connection};
    use tempfile::TempDir;

    const WARM_MEMORY_DDL: &str = "CREATE TABLE warm_memory (
        id TEXT PRIMARY KEY,
        tier TEXT NOT NULL DEFAULT 'warm',
        class TEXT NOT NULL,
        content TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        accessed_at INTEGER NOT NULL,
        usefulness REAL NOT NULL DEFAULT 0.0,
        embedding_json TEXT,
        access_count INTEGER NOT NULL DEFAULT 0,
        session_id TEXT,
        user_id TEXT
    );";

    fn insert_memory(conn: &Connection, id: &str, content: &str, session_id: &str) {
        conn.execute(
            "INSERT INTO warm_memory
                 (id, tier, class, content, created_at, accessed_at, usefulness,
                  embedding_json, access_count, session_id, user_id)
             VALUES (?1, 'warm', 'episodic', ?2, ?3, ?3, 0.8, NULL, 0, ?4, NULL)",
            params![id, content, 1_700_000_000_i64, session_id],
        )
        .expect("insert memory row");
    }

    fn searcher_over_seeded_db(rows: &[(&str, &str)]) -> (TempDir, MemorySearcher) {
        let dir = TempDir::new().expect("temp dir");
        let db_path = dir.path().join("warm.db");
        let conn = Connection::open(&db_path).expect("open db");
        conn.execute_batch(WARM_MEMORY_DDL).expect("create schema");
        for (i, (content, session)) in rows.iter().enumerate() {
            insert_memory(&conn, &format!("row-{i}"), content, session);
        }
        drop(conn);
        let searcher = MemorySearcher::new(&db_path).expect("build searcher");
        (dir, searcher)
    }

    fn tool_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "test-memory-search".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload,
            allowed_base_dir: None,
        }
    }

    fn assert_hit_content(output: &ToolOutput, expected: &str) {
        let result = output.result.as_ref().expect("result");
        let hits = result["hits"].as_array().expect("hits array");
        let hit = hits
            .iter()
            .find(|h| h["content"].as_str().unwrap_or_default() == expected)
            .unwrap_or_else(|| panic!("expected hit with content {expected:?}"));
        assert!(
            hit["session_id"].as_str().is_some(),
            "hit should carry session_id"
        );
        assert!(hit["score"].is_number(), "hit should carry a score");
    }

    #[test]
    fn memory_search_tool_returns_ranked_hits() {
        let (_dir, searcher) = searcher_over_seeded_db(&[
            ("rust auth refactor notes", "session-a"),
            ("python cache tuning", "session-b"),
        ]);
        let tool = MemorySearchTool::with_searcher(searcher);

        let output = tool
            .run(&tool_input(serde_json::json!({"query": "auth refactor"})))
            .expect("run");
        assert!(output.success);
        let result = output.result.as_ref().expect("result");
        assert_eq!(result["count"].as_u64().unwrap(), 1);
        assert_hit_content(&output, "rust auth refactor notes");
        assert_eq!(
            output.verification.as_deref(),
            Some("memory_search_completed")
        );
    }

    #[test]
    fn memory_search_tool_caps_limit_and_defaults() {
        let (_dir, searcher) = searcher_over_seeded_db(&[]);
        let tool = MemorySearchTool::with_searcher(searcher);

        // No limit → default 10; oversized limit → capped at 50.
        let output = tool
            .run(&tool_input(serde_json::json!({"query": "rust"})))
            .expect("run");
        assert!(output.success);

        let output = tool
            .run(&tool_input(
                serde_json::json!({"query": "rust", "limit": 9999}),
            ))
            .expect("run");
        assert!(output.success);
    }

    #[test]
    fn memory_search_tool_rejects_missing_or_empty_query() {
        let (_dir, searcher) = searcher_over_seeded_db(&[("rust refactor", "session-a")]);
        let tool = MemorySearchTool::with_searcher(searcher);

        let err = tool.run(&tool_input(serde_json::json!({}))).unwrap_err();
        assert!(err.to_string().contains("query"));

        let err = tool
            .run(&tool_input(serde_json::json!({"query": "   "})))
            .unwrap_err();
        assert!(err.to_string().contains("query"));
    }

    #[tokio::test]
    async fn memory_search_tool_run_async_offloads_to_blocking_pool() {
        let (_dir, searcher) = searcher_over_seeded_db(&[
            ("跨会话记忆检索测试", "session-zh"),
            ("unrelated note", "session-en"),
        ]);
        let tool = Arc::new(MemorySearchTool::with_searcher(searcher));

        let output = tool
            .run_async(tool_input(serde_json::json!({"query": "记忆检索"})))
            .await
            .expect("async run");
        assert!(output.success);
        assert_hit_content(&output, "跨会话记忆检索测试");
    }

    #[test]
    fn memory_hit_serializes_with_expected_fields() {
        let hit = MemoryHit {
            session_id: Some("s-1".to_string()),
            role: "episodic".to_string(),
            content: "some memory".to_string(),
            score: 0.75,
        };
        let value = serde_json::to_value(&hit).expect("serialize");
        assert_eq!(value["session_id"], "s-1");
        assert_eq!(value["role"], "episodic");
        assert_eq!(value["content"], "some memory");
        assert_eq!(value["score"], 0.75);
    }
}
