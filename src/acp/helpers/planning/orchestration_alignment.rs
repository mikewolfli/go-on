use serde_json::{json, Value};

fn step_expected_tools(description: &str) -> Vec<&'static str> {
    let text = description.to_ascii_lowercase();
    let mut expected = Vec::new();

    if ["search", "find", "discover", "scan"]
        .iter()
        .any(|kw| text.contains(kw))
    {
        expected.push("search_files");
    }
    if ["read", "inspect", "analyze", "review", "trace"]
        .iter()
        .any(|kw| text.contains(kw))
    {
        expected.push("read_file");
        expected.push("inspect_git_diff");
    }
    if ["write", "patch", "modify", "edit", "refactor", "fix"]
        .iter()
        .any(|kw| text.contains(kw))
    {
        expected.push("write_file");
        expected.push("apply_patch");
    }
    if ["test", "verify", "build", "compile", "run"]
        .iter()
        .any(|kw| text.contains(kw))
    {
        expected.push("run_tests");
    }

    expected
}

fn collect_executed_tools(tool_execution_results: &[Value]) -> Vec<String> {
    let mut tools = Vec::new();
    for result in tool_execution_results {
        if let Some(iterations) = result
            .get("trace")
            .and_then(|trace| trace.get("iterations"))
            .and_then(Value::as_array)
        {
            for item in iterations {
                let stage = item
                    .get("stage")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !stage.eq_ignore_ascii_case("act") {
                    continue;
                }
                let tool = item
                    .get("tool")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim();
                if tool.is_empty() {
                    continue;
                }
                if !tools.iter().any(|existing| existing == tool) {
                    tools.push(tool.to_string());
                }
            }
        }
    }
    tools
}

pub(crate) fn derive_plan_trace_alignment(
    execution_plan: &Value,
    tool_execution_results: &[Value],
) -> Value {
    let steps = execution_plan
        .get("steps")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let executed_tools = collect_executed_tools(tool_execution_results);

    let mut tool_required_step_count = 0_u64;
    let mut matched_step_count = 0_u64;
    let mut missing_steps = Vec::new();

    for step in steps {
        let step_id = step
            .get("step_id")
            .and_then(Value::as_str)
            .unwrap_or("unknown-step");
        let description = step
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default();

        let expected_tools = step_expected_tools(description);
        if expected_tools.is_empty() {
            continue;
        }
        tool_required_step_count += 1;

        let matched = expected_tools
            .iter()
            .any(|tool| executed_tools.iter().any(|executed| executed == tool));
        if matched {
            matched_step_count += 1;
        } else {
            missing_steps.push(json!({
                "step_id": step_id,
                "description": description,
                "expected_tools": expected_tools,
            }));
        }
    }

    let coverage = if tool_required_step_count == 0 {
        1.0
    } else {
        matched_step_count as f64 / tool_required_step_count as f64
    };

    json!({
        "tool_required_step_count": tool_required_step_count,
        "matched_step_count": matched_step_count,
        "coverage_ratio": coverage,
        "executed_tools": executed_tools,
        "missing_steps": missing_steps,
    })
}

pub(crate) fn derive_orchestration_node_decisions(
    execution_plan: &Value,
    tool_execution_results: &[Value],
) -> Value {
    let steps = execution_plan
        .get("steps")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let executed_tools = collect_executed_tools(tool_execution_results);

    let mut mapped_nodes = 0_u64;
    let mut unmapped_nodes = 0_u64;
    let mut nodes = Vec::new();

    for step in steps {
        let step_id = step
            .get("step_id")
            .and_then(Value::as_str)
            .unwrap_or("unknown-step");
        let description = step
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let expected_tools = step_expected_tools(description);

        if expected_tools.is_empty() {
            nodes.push(json!({
                "step_id": step_id,
                "decision": "observe_only",
                "mapped": true,
                "description": description,
                "expected_tools": [],
                "matched_tool": Value::Null,
            }));
            mapped_nodes += 1;
            continue;
        }

        let matched_tool = expected_tools
            .iter()
            .find(|tool| executed_tools.iter().any(|executed| executed == **tool))
            .map(|tool| tool.to_string());

        let mapped = matched_tool.is_some();
        if mapped {
            mapped_nodes += 1;
        } else {
            unmapped_nodes += 1;
        }

        nodes.push(json!({
            "step_id": step_id,
            "decision": if mapped { "tool_executed" } else { "replan_required" },
            "mapped": mapped,
            "description": description,
            "expected_tools": expected_tools,
            "matched_tool": matched_tool,
        }));
    }

    let total_nodes = mapped_nodes + unmapped_nodes;
    let mapping_ratio = if total_nodes == 0 {
        1.0
    } else {
        mapped_nodes as f64 / total_nodes as f64
    };

    json!({
        "nodes": nodes,
        "mapped_nodes": mapped_nodes,
        "unmapped_nodes": unmapped_nodes,
        "mapping_ratio": mapping_ratio,
    })
}

#[allow(dead_code)] // F-GAP: reserved for subtask node decision tracking
pub(crate) fn derive_runtime_subtask_node_decisions(records: &[Value]) -> Value {
    let mut mapped_nodes = 0_u64;
    let mut unmapped_nodes = 0_u64;
    let mut nodes = Vec::new();

    for record in records {
        let step_id = record
            .get("id")
            .or_else(|| record.get("subtask_id"))
            .and_then(Value::as_str)
            .unwrap_or("unknown-subtask");
        let description = record
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let outcome = record
            .get("outcome")
            .or_else(|| record.get("status"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let outcome_lower = outcome.to_ascii_lowercase();

        let mapped = matches!(
            outcome_lower.as_str(),
            "completed" | "complete" | "success" | "succeeded" | "done"
        );

        let decision = if mapped {
            "tool_executed"
        } else if matches!(
            outcome_lower.as_str(),
            "failed" | "error" | "timeout" | "cancelled"
        ) {
            "replan_required"
        } else {
            "observe_only"
        };

        if mapped {
            mapped_nodes += 1;
        } else {
            unmapped_nodes += 1;
        }

        nodes.push(json!({
            "step_id": step_id,
            "decision": decision,
            "mapped": mapped,
            "description": description,
            "outcome": outcome,
        }));
    }

    let total_nodes = mapped_nodes + unmapped_nodes;
    let mapping_ratio = if total_nodes == 0 {
        1.0
    } else {
        mapped_nodes as f64 / total_nodes as f64
    };

    json!({
        "nodes": nodes,
        "mapped_nodes": mapped_nodes,
        "unmapped_nodes": unmapped_nodes,
        "mapping_ratio": mapping_ratio,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_sample_execution_plan() -> Value {
        json!({
            "plan_id": "test-plan-1",
            "steps": [
                {
                    "step_id": "step-1",
                    "description": "Search for relevant files and discover the codebase",
                    "depends_on": []
                },
                {
                    "step_id": "step-2",
                    "description": "Read and analyze the source code",
                    "depends_on": ["step-1"]
                },
                {
                    "step_id": "step-3",
                    "description": "Write a patch to fix the issue",
                    "depends_on": ["step-2"]
                },
                {
                    "step_id": "step-4",
                    "description": "Run tests to verify correctness",
                    "depends_on": ["step-3"]
                }
            ]
        })
    }

    fn make_sample_tool_results_with_search_and_read() -> Vec<Value> {
        vec![json!({
            "trace": {
                "iterations": [
                    {
                        "stage": "act",
                        "tool": "search_files"
                    },
                    {
                        "stage": "act",
                        "tool": "read_file"
                    }
                ]
            }
        })]
    }

    #[test]
    fn test_derive_plan_trace_alignment_with_matching_tools() {
        let plan = make_sample_execution_plan();
        let results = make_sample_tool_results_with_search_and_read();
        let alignment = derive_plan_trace_alignment(&plan, &results);

        assert!(alignment.get("coverage_ratio").is_some());
        assert!(alignment.get("tool_required_step_count").is_some());
        assert!(alignment.get("matched_step_count").is_some());
        assert!(alignment.get("executed_tools").is_some());
        assert!(alignment.get("missing_steps").is_some());

        let coverage = alignment["coverage_ratio"]
            .as_f64()
            .expect("coverage_ratio should be a f64");
        assert!(
            (0.0..=1.0).contains(&coverage),
            "coverage should be in [0, 1]"
        );
    }

    #[test]
    fn test_derive_plan_trace_alignment_with_empty_results() {
        let plan = make_sample_execution_plan();
        let results: Vec<Value> = vec![];
        let alignment = derive_plan_trace_alignment(&plan, &results);

        assert_eq!(
            alignment["executed_tools"]
                .as_array()
                .expect("executed_tools should be an array")
                .len(),
            0
        );
        let coverage = alignment["coverage_ratio"]
            .as_f64()
            .expect("coverage_ratio should be a f64");
        assert!((0.0..=1.0).contains(&coverage));
    }

    #[test]
    fn test_derive_orchestration_node_decisions_returns_mapped_nodes() {
        let plan = make_sample_execution_plan();
        let results = make_sample_tool_results_with_search_and_read();
        let decisions = derive_orchestration_node_decisions(&plan, &results);

        assert!(decisions.get("nodes").is_some());
        assert!(decisions.get("mapped_nodes").is_some());
        assert!(decisions.get("unmapped_nodes").is_some());
        assert!(decisions.get("mapping_ratio").is_some());

        let mapping_ratio = decisions["mapping_ratio"]
            .as_f64()
            .expect("mapping_ratio should be a f64");
        assert!(
            (0.0..=1.0).contains(&mapping_ratio),
            "mapping_ratio should be in [0, 1]"
        );

        let nodes = decisions["nodes"]
            .as_array()
            .expect("nodes should be an array");
        assert!(!nodes.is_empty(), "should have at least one node");
    }

    #[test]
    fn test_derive_runtime_subtask_node_decisions_handles_completed_status() {
        let records = vec![
            json!({"id": "sub-1", "description": "first task", "outcome": "completed"}),
            json!({"id": "sub-2", "description": "second task", "outcome": "failed"}),
            json!({"id": "sub-3", "description": "third task", "outcome": "in_progress"}),
        ];
        let decisions = derive_runtime_subtask_node_decisions(&records);

        assert_eq!(
            decisions["mapped_nodes"]
                .as_u64()
                .expect("mapped_nodes should be a u64"),
            1
        );
        assert_eq!(
            decisions["unmapped_nodes"]
                .as_u64()
                .expect("unmapped_nodes should be a u64"),
            2
        );
        let nodes = decisions["nodes"]
            .as_array()
            .expect("nodes should be an array");
        assert_eq!(nodes.len(), 3);

        // First should be tool_executed
        assert_eq!(
            nodes[0]["decision"]
                .as_str()
                .expect("decision should be a string"),
            "tool_executed"
        );
        // Second should be replan_required
        assert_eq!(
            nodes[1]["decision"]
                .as_str()
                .expect("decision should be a string"),
            "replan_required"
        );
        // Third should be observe_only (not a terminal outcome)
        assert_eq!(
            nodes[2]["decision"]
                .as_str()
                .expect("decision should be a string"),
            "observe_only"
        );
    }

    #[test]
    fn test_derive_runtime_subtask_node_decisions_empty_records() {
        let records: Vec<Value> = vec![];
        let decisions = derive_runtime_subtask_node_decisions(&records);
        assert_eq!(
            decisions["mapped_nodes"]
                .as_u64()
                .expect("mapped_nodes should be a u64"),
            0
        );
        assert_eq!(
            decisions["unmapped_nodes"]
                .as_u64()
                .expect("unmapped_nodes should be a u64"),
            0
        );
        assert_eq!(
            decisions["mapping_ratio"]
                .as_f64()
                .expect("mapping_ratio should be a f64"),
            1.0
        );
    }
}
