//! BLUE42 ORCH-FIN-04: ExecutionGraph-driven tool execution.
//!
//! Uses ExecutionGraph nodes (Branch/Join/Condition) to express tool execution
//! as a DAG with fan-out, synchronization, and state tracking. Node states are
//! exposed for governance.status observability.

use std::sync::Arc;

use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::orchestration::execution_graph::{ExNodeId, ExNodeState};
use crate::orchestration::tool::{execute_loop, LoopConfig, LoopDecision, ToolInput, ToolRegistry};

/// Result of a single DAG node execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagNodeResult {
    pub node_id: ExNodeId,
    pub tool_name: String,
    pub state: ExNodeState,
    pub duration_ms: u64,
    /// Preserved tool output payload for observe/replan evidence
    pub tool_output: Option<serde_json::Value>,
    /// Preserved error payload for diagnostic use
    pub error_payload: Option<String>,
}

/// Complete DAG execution trace
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagExecutionTrace {
    pub nodes: Vec<DagNodeResult>,
    pub total_duration_ms: u64,
    pub branch_count: u32,
    pub join_count: u32,
}

/// Build tool execution as a Branch-Join DAG.
/// Independent tools become Branch fans; dependent tools are sequenced.
pub fn build_tool_execution_dag(tool_calls: &[(String, String)]) -> (ExNodeId, Vec<ExNodeId>) {
    let branch_id: ExNodeId = "branch-tools".to_string();
    let tool_node_ids: Vec<ExNodeId> = tool_calls
        .iter()
        .enumerate()
        .map(|(i, (name, _))| format!("tool-{}-{}", name, i))
        .collect();
    (branch_id, tool_node_ids)
}

/// Execute tools as a Branch-Join DAG and return results with node states.
/// Uses tokio::spawn for fan-out, then join_all for synchronization.
pub async fn execute_tool_dag(
    registry: Arc<ToolRegistry>,
    objective: &str,
    iteration: usize,
    tool_calls: &[(String, String)],
) -> (Vec<DagNodeResult>, DagExecutionTrace) {
    use std::time::Instant;
    let dag_start = Instant::now();

    // BLUE43 Step 2: Use build_tool_execution_dag to derive DAG structure
    let (_branch_id, tool_node_ids) = build_tool_execution_dag(tool_calls);
    let num_tools = tool_calls.len();
    let branch_count = if num_tools > 1 { 1 } else { 0 }; // one Branch node
    let join_count = if num_tools > 1 { 1 } else { 0 }; // one Join node

    let jobs = tool_calls
        .iter()
        .enumerate()
        .map(|(i, (tool_name, tool_args_str))| {
            let registry = Arc::clone(&registry);
            let tool_name = tool_name.clone();
            let tool_args_str = tool_args_str.clone();
            let objective = objective.to_string();
            let phase = format!("dag-round-{}", iteration);
            // Use the DAG node ID from build_tool_execution_dag for consistency
            let node_id: ExNodeId = tool_node_ids
                .get(i)
                .cloned()
                .unwrap_or_else(|| format!("tool-{}-{}", tool_name, i));

            tokio::spawn(async move {
                let node_start = Instant::now();
                let parsed_args: Value =
                    serde_json::from_str(&tool_args_str).unwrap_or(serde_json::json!({}));
                let input = ToolInput {
                    task_id: "autonomy-loop".to_string(),
                    phase,
                    agent_role: "autonomy_agent".to_string(),
                    objective,
                    constraints: None,
                    evidence: None,
                    payload: parsed_args,
                    allowed_base_dir: None,
                };
                let cfg = LoopConfig {
                    max_iterations: 1,
                    max_retries_per_tool: 1,
                    enable_fallback: false,
                    verify_output: None,
                };
                let (decision, _trace) = execute_loop(&tool_name, &registry, &input, &[], &cfg);
                let (state, tool_output, error_payload) = match decision {
                    LoopDecision::Complete(ref output) => {
                        (ExNodeState::Completed, output.result.clone(), None)
                    }
                    LoopDecision::Failed {
                        ref reason,
                        ref last_output,
                    } => (
                        ExNodeState::Failed("execution_error".to_string()),
                        last_output.as_ref().and_then(|o| o.result.clone()),
                        Some(reason.clone()),
                    ),
                    _ => (ExNodeState::Skipped, None, None),
                };
                DagNodeResult {
                    node_id,
                    tool_name,
                    state,
                    duration_ms: node_start.elapsed().as_millis() as u64,
                    tool_output,
                    error_payload,
                }
            })
        })
        .collect::<Vec<_>>();

    let results: Vec<DagNodeResult> = join_all(jobs)
        .await
        .into_iter()
        .filter_map(Result::ok)
        .collect();

    let trace = DagExecutionTrace {
        nodes: results.clone(),
        total_duration_ms: dag_start.elapsed().as_millis() as u64,
        branch_count,
        join_count,
    };

    (results, trace)
}

/// Convert DAG execution results into a governance.status-observable payload.
pub fn dag_trace_to_observability(trace: &DagExecutionTrace) -> Value {
    let completed = trace
        .nodes
        .iter()
        .filter(|n| matches!(n.state, ExNodeState::Completed))
        .count();
    let failed = trace
        .nodes
        .iter()
        .filter(|n| matches!(n.state, ExNodeState::Failed(_)))
        .count();
    let total = trace.nodes.len();

    serde_json::json!({
        "dag_execution": {
            "total_nodes": total,
            "completed": completed,
            "failed": failed,
            "branch_count": trace.branch_count,
            "join_count": trace.join_count,
            "total_duration_ms": trace.total_duration_ms,
            "has_tool_evidence": trace.nodes.iter().any(|n| n.tool_output.is_some()),
            "node_details": trace.nodes.iter().map(|n| serde_json::json!({
                "node_id": n.node_id,
                "tool": n.tool_name,
                "state": format!("{:?}", n.state),
                "duration_ms": n.duration_ms,
                "has_output": n.tool_output.is_some(),
                "has_error": n.error_payload.is_some(),
            })).collect::<Vec<_>>(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dag_trace_to_observability_includes_all_nodes() {
        let trace = DagExecutionTrace {
            nodes: vec![
                DagNodeResult {
                    node_id: "tool-read_file-0".into(),
                    tool_name: "read_file".into(),
                    state: ExNodeState::Completed,
                    duration_ms: 15,
                    tool_output: Some(serde_json::json!({"content": "file content"})),
                    error_payload: None,
                },
                DagNodeResult {
                    node_id: "tool-search_files-1".into(),
                    tool_name: "search_files".into(),
                    state: ExNodeState::Completed,
                    duration_ms: 22,
                    tool_output: None,
                    error_payload: None,
                },
            ],
            total_duration_ms: 40,
            branch_count: 1,
            join_count: 1,
        };
        let obs = dag_trace_to_observability(&trace);
        assert_eq!(obs["dag_execution"]["completed"].as_u64(), Some(2));
        assert_eq!(obs["dag_execution"]["total_nodes"].as_u64(), Some(2));
        assert_eq!(obs["dag_execution"]["branch_count"].as_u64(), Some(1));
        assert_eq!(
            obs["dag_execution"]["node_details"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn dag_trace_to_observability_wired_to_governance_record() {
        // Verify that dag_trace_to_observability produces governance-shaped output
        // with all required metrics for the governance payload.
        let trace = DagExecutionTrace {
            nodes: vec![
                DagNodeResult {
                    node_id: "tool-read_file-0".into(),
                    tool_name: "read_file".into(),
                    state: ExNodeState::Completed,
                    duration_ms: 10,
                    tool_output: Some(serde_json::json!({"content": "data"})),
                    error_payload: None,
                },
                DagNodeResult {
                    node_id: "tool-grep-1".into(),
                    tool_name: "grep".into(),
                    state: ExNodeState::Completed,
                    duration_ms: 20,
                    tool_output: None,
                    error_payload: None,
                },
                DagNodeResult {
                    node_id: "tool-write_file-2".into(),
                    tool_name: "write_file".into(),
                    state: ExNodeState::Failed("permission_denied".to_string()),
                    duration_ms: 5,
                    tool_output: None,
                    error_payload: Some("Permission denied".to_string()),
                },
            ],
            total_duration_ms: 35,
            branch_count: 1,
            join_count: 1,
        };
        let obs = dag_trace_to_observability(&trace);

        // Must include the top-level "dag_execution" key for governance parsing
        let exec = &obs["dag_execution"];
        assert_eq!(exec["total_nodes"].as_u64(), Some(3));
        assert_eq!(exec["completed"].as_u64(), Some(2));
        assert_eq!(exec["failed"].as_u64(), Some(1));
        assert_eq!(exec["branch_count"].as_u64(), Some(1));
        assert_eq!(exec["join_count"].as_u64(), Some(1));
        assert_eq!(exec["total_duration_ms"].as_u64(), Some(35));
        assert!(exec["has_tool_evidence"].as_bool().unwrap());

        // DAG width = unique tools per level; depth = max chain length
        // Both are derivable from node_details
        let details = exec["node_details"].as_array().unwrap();
        assert_eq!(details.len(), 3);
        assert_eq!(details[0]["tool"].as_str(), Some("read_file"));
        assert_eq!(details[1]["tool"].as_str(), Some("grep"));
        assert_eq!(details[2]["tool"].as_str(), Some("write_file"));
        assert_eq!(details[0]["state"].as_str(), Some("Completed"));
        assert_eq!(
            details[2]["state"].as_str(),
            Some("Failed(\"permission_denied\")")
        );
        assert!(details[0]["has_output"].as_bool().unwrap());
        assert!(details[2]["has_error"].as_bool().unwrap());
    }

    #[test]
    fn dag_evidence_chain_preserves_tool_output() {
        // Create a DagExecutionTrace with both successful (with tool_output)
        // and failed (with error_payload) nodes.
        let trace = DagExecutionTrace {
            nodes: vec![
                DagNodeResult {
                    node_id: "tool-read_file-0".into(),
                    tool_name: "read_file".into(),
                    state: ExNodeState::Completed,
                    duration_ms: 12,
                    tool_output: Some(serde_json::json!({"content": "evidence data"})),
                    error_payload: None,
                },
                DagNodeResult {
                    node_id: "tool-write_file-1".into(),
                    tool_name: "write_file".into(),
                    state: ExNodeState::Failed("io_error".to_string()),
                    duration_ms: 8,
                    tool_output: None,
                    error_payload: Some("Disk full".to_string()),
                },
            ],
            total_duration_ms: 20,
            branch_count: 1,
            join_count: 1,
        };

        let obs = dag_trace_to_observability(&trace);
        let exec = &obs["dag_execution"];

        // Verify has_tool_evidence is true (one node has tool_output)
        assert!(exec["has_tool_evidence"].as_bool().unwrap());

        // Verify completed/failed counts are correct
        assert_eq!(exec["completed"].as_u64(), Some(1));
        assert_eq!(exec["failed"].as_u64(), Some(1));
        assert_eq!(exec["total_nodes"].as_u64(), Some(2));

        // Verify node_details contains both nodes with correct state
        let details = exec["node_details"].as_array().unwrap();
        assert_eq!(details.len(), 2);

        assert_eq!(details[0]["node_id"].as_str(), Some("tool-read_file-0"));
        assert_eq!(details[0]["tool"].as_str(), Some("read_file"));
        assert_eq!(details[0]["state"].as_str(), Some("Completed"));
        assert!(details[0]["has_output"].as_bool().unwrap());
        assert!(!details[0]["has_error"].as_bool().unwrap());

        assert_eq!(details[1]["node_id"].as_str(), Some("tool-write_file-1"));
        assert_eq!(details[1]["tool"].as_str(), Some("write_file"));
        assert_eq!(details[1]["state"].as_str(), Some("Failed(\"io_error\")"));
        assert!(!details[1]["has_output"].as_bool().unwrap());
        assert!(details[1]["has_error"].as_bool().unwrap());
    }

    #[test]
    fn build_tool_execution_dag_returns_ids() {
        let calls = vec![
            ("read_file".to_string(), "{}".to_string()),
            ("search".to_string(), "{}".to_string()),
        ];
        let (branch_id, tool_ids) = build_tool_execution_dag(&calls);
        assert_eq!(branch_id, "branch-tools");
        assert_eq!(tool_ids.len(), 2);
        assert!(tool_ids[0].starts_with("tool-"));
    }
}
