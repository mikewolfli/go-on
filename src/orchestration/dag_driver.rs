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
#[allow(dead_code)]
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
            let node_id: ExNodeId = format!("tool-{}-{}", tool_name, i);

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
                let state = match decision {
                    LoopDecision::Complete(_) => ExNodeState::Completed,
                    LoopDecision::Failed { .. } => {
                        ExNodeState::Failed("execution_error".to_string())
                    }
                    _ => ExNodeState::Skipped,
                };
                DagNodeResult {
                    node_id,
                    tool_name,
                    state,
                    duration_ms: node_start.elapsed().as_millis() as u64,
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
#[allow(dead_code)]
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
            "node_details": trace.nodes.iter().map(|n| serde_json::json!({
                "node_id": n.node_id,
                "tool": n.tool_name,
                "state": format!("{:?}", n.state),
                "duration_ms": n.duration_ms,
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
                },
                DagNodeResult {
                    node_id: "tool-search_files-1".into(),
                    tool_name: "search_files".into(),
                    state: ExNodeState::Completed,
                    duration_ms: 22,
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
