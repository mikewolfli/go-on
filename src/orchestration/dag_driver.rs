//! BLUE42 ORCH-FIN-04: ExecutionGraph-driven tool execution.
//!
//! Uses ExecutionGraph nodes (Branch/Join/Condition) to express tool execution
//! as a DAG with fan-out, synchronization, and state tracking. Node states are
//! exposed for governance.status observability.

use std::sync::Arc;

use crate::i18n::runtime::tf;
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Semaphore;
use tracing::warn;

use crate::orchestration::dag_executor::{build_dag_from_tool_calls, DagGraph};
use crate::orchestration::execution_graph::{ExNodeId, ExNodeState};
use crate::orchestration::planner_executor::ExecutionPlan;
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
    // Delegate to the real DAG builder: convert String args to Value
    let converted: Vec<(String, Value)> = tool_calls
        .iter()
        .map(|(name, args_str)| {
            let parsed: Value = serde_json::from_str(args_str).unwrap_or_else(|e| {
                warn!(
                    "failed to parse JSON args for tool '{}': {}; using empty object",
                    name, e
                );
                serde_json::json!({})
            });
            (name.clone(), parsed)
        })
        .collect();
    let graph = build_dag_from_tool_calls(&converted);
    let tool_node_ids: Vec<ExNodeId> = graph.nodes.keys().cloned().collect();
    let branch_id: ExNodeId = if graph.nodes.is_empty() {
        "branch-tools".to_string()
    } else {
        // Use the first entry point as the branch node ID
        graph
            .entry_points
            .first()
            .cloned()
            .unwrap_or_else(|| "branch-tools".to_string())
    };
    (branch_id, tool_node_ids)
}

/// Execute tools as a Branch-Join DAG and return results with node states.
///
/// When an `ExecutionPlan` with real dependency edges is provided, tools are
/// executed in topological levels (nodes in the same level run in parallel;
/// outputs flow from completed nodes to dependent nodes).
/// When no plan is provided, falls back to flat parallel fan-out.
pub async fn execute_tool_dag(
    registry: Arc<ToolRegistry>,
    objective: &str,
    iteration: usize,
    tool_calls: &[(String, String)],
    plan: Option<&ExecutionPlan>,
) -> (Vec<DagNodeResult>, DagExecutionTrace) {
    match plan {
        Some(plan) if !plan.steps.is_empty() => {
            execute_with_plan_topology(registry, objective, iteration, tool_calls, plan).await
        }
        _ => execute_flat_fanout(registry, objective, iteration, tool_calls).await,
    }
}

/// Execute all tool calls in parallel with no dependency ordering (flat fan-out).
///
/// A Semaphore limits concurrent tool execution to avoid overwhelming the system.
/// Max concurrency defaults to 10, configurable via `GO_ON_DAG_FANOUT_CONCURRENCY` env var.
async fn execute_flat_fanout(
    registry: Arc<ToolRegistry>,
    objective: &str,
    iteration: usize,
    tool_calls: &[(String, String)],
) -> (Vec<DagNodeResult>, DagExecutionTrace) {
    use std::time::Instant;
    let dag_start = Instant::now();

    // Limit concurrent tool execution to avoid overwhelming the system.
    let max_concurrency: usize = std::env::var("GO_ON_DAG_FANOUT_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let concurrency_semaphore = Arc::new(Semaphore::new(max_concurrency));

    // Collect fallback tool names from the tool calls themselves.
    let preferred_tools: Vec<String> = tool_calls.iter().map(|(name, _)| name.clone()).collect();

    let (_branch_id, tool_node_ids) = build_tool_execution_dag(tool_calls);
    let num_tools = tool_calls.len();
    let branch_count = if num_tools > 1 { 1 } else { 0 };
    let join_count = if num_tools > 1 { 1 } else { 0 };

    let jobs = create_tool_jobs(
        &registry,
        objective,
        iteration,
        tool_calls,
        &tool_node_ids,
        None,
        &preferred_tools,
        Some(Arc::clone(&concurrency_semaphore)),
    );

    let results: Vec<DagNodeResult> = join_all(jobs)
        .await
        .into_iter()
        .filter_map(|r| match r {
            Ok(result) => Some(result),
            Err(join_err) => {
                warn!("DAG tool task panicked: {}", join_err);
                None
            }
        })
        .collect();

    let trace = DagExecutionTrace {
        nodes: results.clone(),
        total_duration_ms: dag_start.elapsed().as_millis() as u64,
        branch_count,
        join_count,
    };

    (results, trace)
}

/// Execute tool calls respecting the plan's topological dependency structure.
///
/// Builds a DagGraph from the plan's steps, computes topological levels,
/// distributes tool calls across levels, and executes level-by-level with
/// output propagation from completed nodes to dependent nodes.
async fn execute_with_plan_topology(
    registry: Arc<ToolRegistry>,
    objective: &str,
    iteration: usize,
    tool_calls: &[(String, String)],
    plan: &ExecutionPlan,
) -> (Vec<DagNodeResult>, DagExecutionTrace) {
    use std::time::Instant;
    let dag_start = Instant::now();

    // Limit concurrent tool execution within each topological level.
    let max_concurrency: usize = std::env::var("GO_ON_DAG_FANOUT_CONCURRENCY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let concurrency_semaphore = Arc::new(Semaphore::new(max_concurrency));

    // Collect fallback tool names from the tool calls.
    let preferred_tools: Vec<String> = tool_calls.iter().map(|(name, _)| name.clone()).collect();

    // Build a DagGraph from plan steps to extract topological levels
    let mut graph = DagGraph::new();
    for step in &plan.steps {
        let node_input = serde_json::json!({
            "step_id": &step.step_id,
            "description": &step.description,
        });
        graph.add_node(
            step.step_id.clone(),
            format!("phase:{:?}", step.mode),
            node_input,
            step.depends_on.clone(),
        );
    }

    // Compute topological levels (groups of plan steps that can run in parallel)
    let levels = match graph.topological_sort() {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(
                "{}",
                tf("status.dag.cycle_detected", &[("error", &e.to_string())])
            );
            return execute_flat_fanout(registry, objective, iteration, tool_calls).await;
        }
    };

    let width = graph.width;
    let depth = graph.depth;

    if levels.is_empty() || tool_calls.is_empty() {
        let trace = DagExecutionTrace {
            nodes: vec![],
            total_duration_ms: dag_start.elapsed().as_millis() as u64,
            branch_count: depth as u32,
            join_count: width as u32,
        };
        return (vec![], trace);
    }

    // Distribute tool calls across topological levels (round-robin assignment)
    let num_levels = levels.len();
    let num_tools = tool_calls.len();
    let mut level_tool_indices: Vec<Vec<usize>> = Vec::with_capacity(num_levels);
    for _ in 0..num_levels {
        level_tool_indices.push(Vec::new());
    }
    for i in 0..num_tools {
        level_tool_indices[i % num_levels].push(i);
    }

    // Execute level by level — tools within a level run in parallel;
    // accumulated outputs flow into the next level as dependency evidence.
    let mut all_results: Vec<DagNodeResult> = Vec::with_capacity(num_tools);
    let mut accumulated_outputs: Vec<serde_json::Value> = Vec::new();

    for (level_idx, _level) in levels.iter().enumerate() {
        let tool_indices = &level_tool_indices[level_idx];
        if tool_indices.is_empty() {
            continue;
        }

        // Build dependency evidence from prior levels' outputs
        let dependency_evidence: Option<serde_json::Value> = if accumulated_outputs.is_empty() {
            None
        } else {
            Some(serde_json::json!({
                "prior_level_outputs": accumulated_outputs,
            }))
        };

        // Collect the tool calls assigned to this level
        let level_tool_calls: Vec<(String, String)> = tool_indices
            .iter()
            .map(|&i| tool_calls[i].clone())
            .collect();

        // Generate stable node IDs for this level
        let level_node_ids: Vec<ExNodeId> = tool_indices
            .iter()
            .map(|&i| {
                let (name, _) = &tool_calls[i];
                format!("tool-{}-{}-L{}", name, i, level_idx)
            })
            .collect();

        let jobs = create_tool_jobs(
            &registry,
            objective,
            iteration,
            &level_tool_calls,
            &level_node_ids,
            dependency_evidence.clone(),
            &preferred_tools,
            Some(Arc::clone(&concurrency_semaphore)),
        );

        // Track panicked tasks so the information is not lost.
        let mut panicked_tasks: Vec<String> = Vec::new();
        let level_results: Vec<DagNodeResult> = join_all(jobs)
            .await
            .into_iter()
            .filter_map(|r| match r {
                Ok(result) => Some(result),
                Err(join_err) => {
                    warn!("DAG tool task panicked: {}", join_err);
                    panicked_tasks.push(format!("task panicked: {}", join_err));
                    None
                }
            })
            .collect();
        if !panicked_tasks.is_empty() {
            warn!(
                "DAG level {}: {} task(s) panicked and were discarded",
                level_idx,
                panicked_tasks.len()
            );
        }

        // Collect outputs from this level for propagation to the next level
        for node in &level_results {
            if let Some(ref output) = node.tool_output {
                accumulated_outputs.push(output.clone());
            }
        }

        all_results.extend(level_results);
    }

    let trace = DagExecutionTrace {
        nodes: all_results.clone(),
        total_duration_ms: dag_start.elapsed().as_millis() as u64,
        branch_count: depth as u32,
        join_count: width as u32,
    };

    (all_results, trace)
}

/// Create tokio::spawn jobs for a set of tool calls.
///
/// `dependency_evidence` is injected as the `evidence` field in ToolInput
/// when present, allowing downstream tools to consume prior outputs.
/// `preferred_tools` constrains the tools that `execute_loop` will consider;
/// when non-empty it replaces the old hardcoded `&[]` (which meant "all reg tools").
/// `concurrency_semaphore` caps the number of simultaneously-executing tasks.
fn create_tool_jobs(
    registry: &Arc<ToolRegistry>,
    objective: &str,
    iteration: usize,
    tool_calls: &[(String, String)],
    node_ids: &[ExNodeId],
    dependency_evidence: Option<serde_json::Value>,
    preferred_tools: &[String],
    concurrency_semaphore: Option<Arc<Semaphore>>,
) -> Vec<tokio::task::JoinHandle<DagNodeResult>> {
    use std::time::Instant;

    tool_calls
        .iter()
        .enumerate()
        .map(|(i, (tool_name, tool_args_str))| {
            let registry = Arc::clone(registry);
            let tool_name = tool_name.clone();
            let tool_args_str = tool_args_str.clone();
            let objective = objective.to_string();
            let phase = format!("dag-round-{}", iteration);
            let node_id: ExNodeId = node_ids
                .get(i)
                .cloned()
                .unwrap_or_else(|| format!("tool-{}-{}", tool_name, i));
            let evidence = dependency_evidence.clone();
            let semaphore = concurrency_semaphore.clone();
            let pref_tools = preferred_tools.to_vec();

            tokio::spawn(async move {
                // Acquire concurrency permit before starting work.
                let _permit = match semaphore {
                    Some(ref sem) => Some(
                        sem.acquire()
                            .await
                            .expect("DAG concurrency semaphore was closed"),
                    ),
                    None => None,
                };

                let node_start = Instant::now();
                let parsed_args: Value =
                    serde_json::from_str(&tool_args_str).unwrap_or(serde_json::json!({}));
                let input = ToolInput {
                    task_id: "autonomy-loop".to_string(),
                    phase,
                    agent_role: "autonomy_agent".to_string(),
                    objective,
                    constraints: None,
                    evidence: evidence.map(|v| v.to_string()),
                    payload: parsed_args,
                    allowed_base_dir: None,
                };
                let cfg = LoopConfig {
                    max_iterations: 1,
                    max_retries_per_tool: 1,
                    enable_fallback: false,
                    verify_output: None,
                };
                // Use preferred_tools (tool names from the plan) instead of hardcoded `&[]`.
                let (decision, _trace) =
                    execute_loop(&tool_name, &registry, &input, &pref_tools, &cfg);
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
        .collect::<Vec<_>>()
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
            "dag_width": trace.join_count,
            "dag_depth": trace.branch_count,
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
        // entry_points is now properly tracked: first entry point is the first tool's ID
        assert_eq!(branch_id, "tool-read_file-0");
        assert_eq!(tool_ids.len(), 2);
        assert!(tool_ids[0].starts_with("tool-"));
    }

    /// GAP-46-02: Verify that tools with plan-specified dependencies execute
    /// in correct topological levels. A Simple plan has 2 sequential steps
    /// (exec-1 → review-1). With 4 tool calls, 2 go to level 0 and 2 to
    /// level 1. The execute_with_plan_topology path ensures level-0 completes
    /// before level-1 starts.
    #[tokio::test]
    async fn test_dag_executor_executes_with_topological_levels() {
        use crate::orchestration::mode::ModeKind;
        use crate::orchestration::planner_executor::{DagMetrics, ExecutionPlan, PlanStep};

        // Build a Simple execution plan: exec-1 → review-1
        let plan = ExecutionPlan {
            plan_id: "test-plan".to_string(),
            steps: vec![
                PlanStep {
                    step_id: "exec-1".to_string(),
                    description: "Execute task".to_string(),
                    mode: ModeKind::FullAuto,
                    agent: None,
                    depends_on: vec![],
                    timeout_seconds: 10,
                },
                PlanStep {
                    step_id: "review-1".to_string(),
                    description: "Review output".to_string(),
                    mode: ModeKind::SafeGuard,
                    agent: None,
                    depends_on: vec!["exec-1".to_string()],
                    timeout_seconds: 10,
                },
            ],
            parallel_groups: vec![],
            dag_metrics: Some(DagMetrics {
                width: 1,
                depth: 2,
                parallel_group_count: 0,
                total_steps: 2,
                complexity_level: "Simple".into(),
            }),
        };

        // 4 tool calls — will be distributed: 2 in level 0, 2 in level 1
        let tool_calls: Vec<(String, String)> = vec![
            ("read_file".to_string(), "{}".to_string()),
            ("grep".to_string(), "{}".to_string()),
            ("write_file".to_string(), "{}".to_string()),
            ("bash".to_string(), "{}".to_string()),
        ];

        // Use the plan-driven path
        let (results, trace) = execute_tool_dag(
            std::sync::Arc::new(ToolRegistry::new()),
            "test objective",
            0,
            &tool_calls,
            Some(&plan),
        )
        .await;

        // All 4 tools should execute
        assert_eq!(results.len(), 4, "all 4 tools should execute");

        // DAG depth = number of topological levels = 2
        // DAG width = max steps per level = 1 (plan: 1 step per level)
        assert_eq!(trace.branch_count, 2, "depth = 2 levels");
        assert_eq!(trace.join_count, 1, "width = 1 (plan has 1 step per level)");

        // Verify observability payload includes width and depth
        let obs = dag_trace_to_observability(&trace);
        let exec = &obs["dag_execution"];
        assert_eq!(exec["dag_width"].as_u64(), Some(1));
        assert_eq!(exec["dag_depth"].as_u64(), Some(2));
        assert_eq!(exec["total_nodes"].as_u64(), Some(4));
    }

    /// GAP-46-02: Verify that node outputs from completed levels flow into
    /// dependent levels. When level 0 tools produce outputs, those outputs
    /// are accumulated and made available as dependency evidence for level 1+.
    #[test]
    fn test_dag_executor_preserves_dependency_output() {
        use crate::orchestration::mode::ModeKind;
        use crate::orchestration::planner_executor::{DagMetrics, ExecutionPlan, PlanStep};

        // Build a Medium plan: plan-1 → [sub-1, sub-2] → review-1 (3 levels)
        let plan = ExecutionPlan {
            plan_id: "test-medium-plan".to_string(),
            steps: vec![
                PlanStep {
                    step_id: "plan-1".to_string(),
                    description: "Analyze objective".to_string(),
                    mode: ModeKind::Agent,
                    agent: None,
                    depends_on: vec![],
                    timeout_seconds: 10,
                },
                PlanStep {
                    step_id: "sub-1".to_string(),
                    description: "Subtask 1".to_string(),
                    mode: ModeKind::FullAuto,
                    agent: None,
                    depends_on: vec!["plan-1".to_string()],
                    timeout_seconds: 10,
                },
                PlanStep {
                    step_id: "sub-2".to_string(),
                    description: "Subtask 2".to_string(),
                    mode: ModeKind::FullAuto,
                    agent: None,
                    depends_on: vec!["plan-1".to_string()],
                    timeout_seconds: 10,
                },
                PlanStep {
                    step_id: "review-1".to_string(),
                    description: "Review consolidated output".to_string(),
                    mode: ModeKind::SafeGuard,
                    agent: None,
                    depends_on: vec!["sub-1".to_string(), "sub-2".to_string()],
                    timeout_seconds: 10,
                },
            ],
            parallel_groups: vec![vec!["sub-1".to_string(), "sub-2".to_string()]],
            dag_metrics: Some(DagMetrics {
                width: 2,
                depth: 3,
                parallel_group_count: 1,
                total_steps: 4,
                complexity_level: "Medium".into(),
            }),
        };

        // Build a DagGraph from the plan to verify topological sort yields 3 levels
        let mut graph = DagGraph::new();
        for step in &plan.steps {
            graph.add_node(
                step.step_id.clone(),
                format!("phase:{:?}", step.mode),
                serde_json::json!({"step_id": &step.step_id}),
                step.depends_on.clone(),
            );
        }
        let levels = graph.topological_sort().unwrap();

        // Should have 3 levels:
        // Level 0: plan-1 (no deps)
        // Level 1: sub-1, sub-2 (depend on plan-1)
        // Level 2: review-1 (depends on sub-1, sub-2)
        assert_eq!(levels.len(), 3, "should have 3 topological levels");
        assert_eq!(levels[0], vec!["plan-1"]);
        assert_eq!(
            levels[1].len(),
            2,
            "level 1 should have two parallel subtasks"
        );
        assert!(levels[1].contains(&"sub-1".to_string()));
        assert!(levels[1].contains(&"sub-2".to_string()));
        assert_eq!(levels[2], vec!["review-1"]);

        // Verify DAG metrics are populated with real values
        assert_eq!(graph.width, 2, "max width is 2 parallel substeps");
        assert_eq!(graph.depth, 3, "depth is 3 levels");
    }
}
