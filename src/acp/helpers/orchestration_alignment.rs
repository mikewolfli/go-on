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
