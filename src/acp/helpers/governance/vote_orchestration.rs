//! BLUE43 Step 4: Extracted vote/orchestration helper for chat orchestration.
//!
//! Provides council/risk decision orchestration and agent selection voting
//! logic as standalone focused functions.

use serde_json::Value;

/// Determine orchestration decisions for node mapping observability.
pub fn derive_response_orchestration(
    execution_plan: &Value,
    tool_execution_results: &[Value],
) -> Value {
    crate::acp::helpers::orchestration_alignment::derive_orchestration_node_decisions(
        execution_plan,
        tool_execution_results,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn derive_response_orchestration_with_valid_inputs() {
        let plan = json!({
            "steps": [
                {"step_id": "s1", "description": "search for files"},
                {"step_id": "s2", "description": "read the config file"},
                {"step_id": "s3", "description": "modify the implementation"},
            ]
        });
        let results = vec![json!({
            "trace": {
                "iterations": [
                    {"stage": "act", "tool": "search_files"},
                    {"stage": "act", "tool": "read_file"},
                ]
            }
        })];

        let result = derive_response_orchestration(&plan, &results);

        assert!(result.get("nodes").and_then(Value::as_array).is_some());
        assert!(result.get("mapped_nodes").and_then(Value::as_u64).is_some());
        assert!(result
            .get("unmapped_nodes")
            .and_then(Value::as_u64)
            .is_some());
        assert!(result
            .get("mapping_ratio")
            .and_then(Value::as_f64)
            .is_some());

        // s1 (search) and s2 (read) should be mapped; s3 (modify) should not
        let mapped = result
            .get("mapped_nodes")
            .and_then(Value::as_u64)
            .expect("mapped_nodes should be a u64");
        assert!(mapped >= 2, "expected at least 2 mapped nodes");
    }

    #[test]
    fn derive_response_orchestration_empty_input() {
        let plan = json!({});
        let results: Vec<Value> = vec![];

        let result = derive_response_orchestration(&plan, &results);

        let nodes = result
            .get("nodes")
            .and_then(Value::as_array)
            .expect("nodes should be an array");
        assert!(nodes.is_empty(), "no steps should produce no nodes");
        assert_eq!(
            result
                .get("mapped_nodes")
                .and_then(Value::as_u64)
                .expect("mapped_nodes should be a u64"),
            0
        );
        assert_eq!(
            result
                .get("unmapped_nodes")
                .and_then(Value::as_u64)
                .expect("unmapped_nodes should be a u64"),
            0
        );
        assert_eq!(
            result
                .get("mapping_ratio")
                .and_then(Value::as_f64)
                .expect("mapping_ratio should be a f64"),
            1.0
        );
    }
}
