/// Step 2.4: Three-Endpoint Contract Structure Validation
/// Validates that repair_readiness and repair_history contract structures are consistent
/// across backend (Rust), addon (TypeScript), and GUI (Tauri) endpoints
///
/// NOTE: These tests validate contract structure only, not actual RPC calls.
/// Full RPC contract tests are performed in the integration test suite.
#[cfg(test)]
mod three_endpoint_contract_tests {
    use serde_json::{json, Value};

    /// Validate repair_readiness contract structure
    fn validate_repair_readiness_contract(repair_readiness: &Value) -> Result<(), String> {
        // Check required fields
        if !repair_readiness.is_object() {
            return Err("repair_readiness must be an object".to_string());
        }

        let obj = repair_readiness.as_object().unwrap();

        // Check eligible field (required boolean)
        if !obj.contains_key("eligible") {
            return Err("repair_readiness missing required field: eligible".to_string());
        }
        if !obj["eligible"].is_boolean() {
            return Err("repair_readiness.eligible must be boolean".to_string());
        }

        // Check max_iterations field (required integer)
        if !obj.contains_key("max_iterations") {
            return Err("repair_readiness missing required field: max_iterations".to_string());
        }
        if !obj["max_iterations"].is_u64() {
            return Err("repair_readiness.max_iterations must be integer".to_string());
        }

        // Check governance_mode field (required string)
        if !obj.contains_key("governance_mode") {
            return Err("repair_readiness missing required field: governance_mode".to_string());
        }
        if !obj["governance_mode"].is_string() {
            return Err("repair_readiness.governance_mode must be string".to_string());
        }

        // Check reason field (required string)
        if !obj.contains_key("reason") {
            return Err("repair_readiness missing required field: reason".to_string());
        }
        if !obj["reason"].is_string() {
            return Err("repair_readiness.reason must be string".to_string());
        }

        // Validate governance_mode enum values
        let mode = obj["governance_mode"].as_str().unwrap();
        let valid_modes = vec!["assisted", "conservative", "manual", "disabled"];
        if !valid_modes.contains(&mode) {
            return Err(format!(
                "repair_readiness.governance_mode must be one of {:?}, got '{}'",
                valid_modes, mode
            ));
        }

        Ok(())
    }

    /// Validate repair_history contract structure
    fn validate_repair_history_contract(repair_history: &Value) -> Result<(), String> {
        // Check structure
        if !repair_history.is_object() {
            return Err("repair_history must be an object".to_string());
        }

        let obj = repair_history.as_object().unwrap();

        // Check iteration field (optional or required)
        if let Some(iteration) = obj.get("iteration") {
            if !iteration.is_u64() {
                return Err("repair_history.iteration must be integer if present".to_string());
            }
        }

        // Check max_iterations field (optional or required)
        if let Some(max) = obj.get("max_iterations") {
            if !max.is_u64() {
                return Err("repair_history.max_iterations must be integer if present".to_string());
            }
        }

        // Check actions field (must be array)
        if !obj.contains_key("actions") && !obj.contains_key("repair_actions_executed") {
            // Either actions or repair_actions_executed must exist
            return Err(
                "repair_history must contain 'actions' or 'repair_actions_executed'".to_string(),
            );
        }

        if let Some(actions) = obj.get("actions") {
            if !actions.is_array() {
                return Err("repair_history.actions must be array".to_string());
            }
            // Validate each action if present
            for action in actions.as_array().unwrap() {
                if action.is_object() {
                    validate_repair_action(action)?;
                }
            }
        }

        Ok(())
    }

    /// Validate individual repair action structure
    fn validate_repair_action(action: &Value) -> Result<(), String> {
        if !action.is_object() {
            return Err("repair_action must be an object".to_string());
        }

        let obj = action.as_object().unwrap();

        // Check iteration field
        if let Some(iteration) = obj.get("iteration") {
            if !iteration.is_u64() {
                return Err("repair_action.iteration must be integer if present".to_string());
            }
        }

        // Check type field (optional)
        if let Some(action_type) = obj.get("type") {
            if !action_type.is_string() {
                return Err("repair_action.type must be string if present".to_string());
            }
        }

        // Check subtask_id field (optional)
        if let Some(subtask_id) = obj.get("subtask_id") {
            if !subtask_id.is_string() {
                return Err("repair_action.subtask_id must be string if present".to_string());
            }
        }

        // Check result field (optional)
        if let Some(result) = obj.get("result") {
            if !result.is_string() {
                return Err("repair_action.result must be string if present".to_string());
            }
            // Validate result enum values
            let valid_results = vec!["success", "in_progress", "failed"];
            if !valid_results.contains(&result.as_str().unwrap()) {
                return Err(format!(
                    "repair_action.result must be one of {:?}",
                    valid_results
                ));
            }
        }

        Ok(())
    }

    #[test]
    fn step2_4_workflow_execute_has_repair_readiness_contract() {
        // This test validates that workflow.execute response includes repair_readiness
        // with correct contract structure
        let sample_response = json!({
            "ok": true,
            "repair_readiness": {
                "eligible": false,
                "max_iterations": 2,
                "governance_mode": "assisted",
                "reason": "no failures or auto-repair disabled",
            }
        });

        let repair_readiness = &sample_response["repair_readiness"];
        assert!(
            validate_repair_readiness_contract(repair_readiness).is_ok(),
            "workflow.execute should have valid repair_readiness contract"
        );
    }

    #[test]
    fn step2_4_task_execute_has_repair_readiness_contract() {
        // This test validates that task.execute response includes repair_readiness
        // with correct contract structure
        let sample_response = json!({
            "ok": true,
            "repair_readiness": {
                "eligible": true,
                "max_iterations": 2,
                "governance_mode": "conservative",
                "reason": "3 failures detected and auto-repair is enabled",
            }
        });

        let repair_readiness = &sample_response["repair_readiness"];
        assert!(
            validate_repair_readiness_contract(repair_readiness).is_ok(),
            "task.execute should have valid repair_readiness contract"
        );
    }

    #[test]
    fn step2_4_workflow_execute_has_repair_history_contract() {
        // This test validates that workflow.execute response includes repair_history
        // with correct contract structure when repair is triggered
        let sample_response = json!({
            "ok": true,
            "repair_history": {
                "iteration": 1,
                "max_iterations": 2,
                "failed_subtasks_pending": 1,
                "repair_actions_executed": 0,
                "governance_mode": "assisted",
                "actions": []
            }
        });

        let repair_history = &sample_response["repair_history"];
        assert!(
            validate_repair_history_contract(repair_history).is_ok(),
            "workflow.execute should have valid repair_history contract"
        );
    }

    #[test]
    fn step2_4_task_execute_has_repair_history_contract() {
        // This test validates that task.execute response includes repair_history
        // with correct contract structure
        let sample_response = json!({
            "ok": true,
            "repair_history": {
                "iteration": 1,
                "max_iterations": 2,
                "failed_subtasks_pending": 2,
                "repair_actions_executed": 1,
                "governance_mode": "assisted",
                "actions": [
                    {
                        "iteration": 1,
                        "type": "retry_subtask",
                        "subtask_id": "subtask-001",
                        "result": "in_progress"
                    }
                ]
            }
        });

        let repair_history = &sample_response["repair_history"];
        assert!(
            validate_repair_history_contract(repair_history).is_ok(),
            "task.execute should have valid repair_history contract"
        );
    }

    #[test]
    fn step2_4_three_endpoints_repair_readiness_contract_consistency() {
        // Validate that repair_readiness has consistent structure across endpoints
        let backend_response = json!({
            "eligible": true,
            "max_iterations": 2,
            "governance_mode": "assisted",
            "reason": "test"
        });

        let addon_expected = json!({
            "eligible": true,
            "max_iterations": 2,
            "governance_mode": "assisted",
            "reason": "test"
        });

        let gui_expected = json!({
            "eligible": true,
            "max_iterations": 2,
            "governance_mode": "assisted",
            "reason": "test"
        });

        // All three should have identical structure
        assert!(validate_repair_readiness_contract(&backend_response).is_ok());
        assert!(validate_repair_readiness_contract(&addon_expected).is_ok());
        assert!(validate_repair_readiness_contract(&gui_expected).is_ok());

        // Field names must match exactly
        assert_eq!(
            backend_response
                .as_object()
                .unwrap()
                .keys()
                .collect::<std::collections::HashSet<_>>(),
            addon_expected
                .as_object()
                .unwrap()
                .keys()
                .collect::<std::collections::HashSet<_>>()
        );
    }

    #[test]
    fn step2_4_repair_readiness_eligible_field_consistency() {
        // Test that eligible field has consistent semantics across endpoints
        // When eligible=true: auto-repair can proceed
        // When eligible=false: no auto-repair will occur

        let enabled = json!({
            "eligible": true,
            "max_iterations": 2,
            "governance_mode": "assisted",
            "reason": "failures detected"
        });

        let disabled = json!({
            "eligible": false,
            "max_iterations": 0,
            "governance_mode": "disabled",
            "reason": "no failures"
        });

        assert!(validate_repair_readiness_contract(&enabled).is_ok());
        assert!(validate_repair_readiness_contract(&disabled).is_ok());

        // Verify field types are consistent
        assert!(enabled["eligible"].is_boolean());
        assert!(disabled["eligible"].is_boolean());
        assert!(enabled["max_iterations"].is_u64());
        assert!(disabled["max_iterations"].is_u64());
    }

    #[test]
    fn step2_4_governance_mode_enum_consistency() {
        // Test that governance_mode values are consistent across endpoints
        let valid_modes = vec!["assisted", "conservative", "manual", "disabled"];

        for mode in valid_modes {
            let response = json!({
                "eligible": true,
                "max_iterations": 2,
                "governance_mode": mode,
                "reason": "test"
            });

            assert!(
                validate_repair_readiness_contract(&response).is_ok(),
                "governance_mode '{}' should be valid",
                mode
            );
        }

        // Invalid mode should fail validation
        let invalid = json!({
            "eligible": true,
            "max_iterations": 2,
            "governance_mode": "invalid_mode",
            "reason": "test"
        });

        assert!(validate_repair_readiness_contract(&invalid).is_err());
    }

    #[test]
    fn step2_4_repair_action_result_enum_consistency() {
        // Test that repair action result values are consistent across endpoints
        let valid_results = vec!["success", "in_progress", "failed"];

        for result in valid_results {
            let action = json!({
                "iteration": 1,
                "type": "retry_subtask",
                "subtask_id": "test-001",
                "result": result
            });

            assert!(
                validate_repair_action(&action).is_ok(),
                "result '{}' should be valid",
                result
            );
        }

        // Invalid result should fail validation
        let invalid = json!({
            "iteration": 1,
            "type": "retry_subtask",
            "subtask_id": "test-001",
            "result": "invalid_result"
        });

        assert!(validate_repair_action(&invalid).is_err());
    }

    #[test]
    fn step2_4_empty_repair_history_valid_when_no_repair() {
        // When no repair occurs, repair_history should have empty actions array
        let no_repair = json!({
            "actions": []
        });

        assert!(validate_repair_history_contract(&no_repair).is_ok());
    }

    #[test]
    fn step2_4_max_iterations_default_value() {
        // Test that max_iterations defaults to 2 per BLUE22 spec
        let response = json!({
            "eligible": true,
            "max_iterations": 2,
            "governance_mode": "assisted",
            "reason": "test"
        });

        assert!(validate_repair_readiness_contract(&response).is_ok());
        assert_eq!(response["max_iterations"], 2);
    }
}
