use std::sync::Arc;

use futures_util::future::join_all;
use serde_json::Value;

use crate::orchestration::tool::{execute_loop, LoopConfig, LoopDecision, ToolInput, ToolRegistry};

pub struct DagToolResult {
    pub tool_name: String,
    pub decision: LoopDecision,
}

pub async fn execute_parallel_tool_calls(
    registry: Arc<ToolRegistry>,
    objective: &str,
    iteration: usize,
    tool_calls: &[(String, String)],
) -> Vec<DagToolResult> {
    let jobs = tool_calls
        .iter()
        .map(|(tool_name, tool_args_str)| {
            let registry = Arc::clone(&registry);
            let tool_name = tool_name.clone();
            let tool_args_str = tool_args_str.clone();
            let objective = objective.to_string();
            let phase = format!("dag-round-{}", iteration);
            tokio::spawn(async move {
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
                DagToolResult {
                    tool_name,
                    decision,
                }
            })
        })
        .collect::<Vec<_>>();

    join_all(jobs)
        .await
        .into_iter()
        .filter_map(Result::ok)
        .collect()
}
