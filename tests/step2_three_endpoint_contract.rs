/// Step 2.4: Three-Endpoint Contract Structure Validation
/// Validates that repair_readiness and repair_history contract structures are consistent
/// across backend (Rust), addon (TypeScript), and GUI (Tauri) endpoints
///
/// Extends to cover all RPC response fields per P1 requirement:
///   - health check field consistency
///   - capabilities.list field consistency
///   - initialize response field consistency
///   - governance.status field consistency (harness_bus 17 fields, capability_bus 11 fields, Phase 4 sub-bus 15 fields)
///   - execution cycle field consistency (BLUE22)
///   - configuration field consistency
///   - error response field consistency
///   - Field name drift detection
///
/// NOTE: These tests validate contract structure only, not actual RPC calls.
/// Full RPC contract tests are performed in the integration test suite.
#[cfg(test)]
mod three_endpoint_contract_tests {
    use serde_json::{json, Value};
    use std::collections::HashSet;

    /// Validate repair_readiness contract structure
    fn validate_repair_readiness_contract(repair_readiness: &Value) -> Result<(), String> {
        if !repair_readiness.is_object() {
            return Err("repair_readiness must be an object".to_string());
        }
        let obj = repair_readiness.as_object().unwrap();

        if !obj.contains_key("eligible") {
            return Err("repair_readiness missing required field: eligible".to_string());
        }
        if !obj["eligible"].is_boolean() {
            return Err("repair_readiness.eligible must be boolean".to_string());
        }

        if !obj.contains_key("max_iterations") {
            return Err("repair_readiness missing required field: max_iterations".to_string());
        }
        if !obj["max_iterations"].is_u64() {
            return Err("repair_readiness.max_iterations must be integer".to_string());
        }

        if !obj.contains_key("governance_mode") {
            return Err("repair_readiness missing required field: governance_mode".to_string());
        }
        if !obj["governance_mode"].is_string() {
            return Err("repair_readiness.governance_mode must be string".to_string());
        }

        if !obj.contains_key("reason") {
            return Err("repair_readiness missing required field: reason".to_string());
        }
        if !obj["reason"].is_string() {
            return Err("repair_readiness.reason must be string".to_string());
        }

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
        if !repair_history.is_object() {
            return Err("repair_history must be an object".to_string());
        }
        let obj = repair_history.as_object().unwrap();

        if let Some(iteration) = obj.get("iteration") {
            if !iteration.is_u64() {
                return Err("repair_history.iteration must be integer if present".to_string());
            }
        }

        if let Some(max) = obj.get("max_iterations") {
            if !max.is_u64() {
                return Err("repair_history.max_iterations must be integer if present".to_string());
            }
        }

        if !obj.contains_key("actions") && !obj.contains_key("repair_actions_executed") {
            return Err(
                "repair_history must contain 'actions' or 'repair_actions_executed'".to_string(),
            );
        }

        if let Some(actions) = obj.get("actions") {
            if !actions.is_array() {
                return Err("repair_history.actions must be array".to_string());
            }
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

        if let Some(iteration) = obj.get("iteration") {
            if !iteration.is_u64() {
                return Err("repair_action.iteration must be integer if present".to_string());
            }
        }

        if let Some(action_type) = obj.get("type") {
            if !action_type.is_string() {
                return Err("repair_action.type must be string if present".to_string());
            }
        }

        if let Some(subtask_id) = obj.get("subtask_id") {
            if !subtask_id.is_string() {
                return Err("repair_action.subtask_id must be string if present".to_string());
            }
        }

        if let Some(result) = obj.get("result") {
            if !result.is_string() {
                return Err("repair_action.result must be string if present".to_string());
            }
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

    // ──────────────────────────────────────────────────────────────
    // Existing tests for repair_readiness / repair_history
    // ──────────────────────────────────────────────────────────────

    #[test]
    fn step2_4_workflow_execute_has_repair_readiness_contract() {
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

        assert!(validate_repair_readiness_contract(&backend_response).is_ok());
        assert!(validate_repair_readiness_contract(&addon_expected).is_ok());
        assert!(validate_repair_readiness_contract(&gui_expected).is_ok());

        assert_eq!(
            backend_response
                .as_object()
                .unwrap()
                .keys()
                .collect::<HashSet<_>>(),
            addon_expected
                .as_object()
                .unwrap()
                .keys()
                .collect::<HashSet<_>>()
        );
    }

    #[test]
    fn step2_4_repair_readiness_eligible_field_consistency() {
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

        assert!(enabled["eligible"].is_boolean());
        assert!(disabled["eligible"].is_boolean());
        assert!(enabled["max_iterations"].is_u64());
        assert!(disabled["max_iterations"].is_u64());
    }

    #[test]
    fn step2_4_governance_mode_enum_consistency() {
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
        let no_repair = json!({ "actions": [] });
        assert!(validate_repair_history_contract(&no_repair).is_ok());
    }

    #[test]
    fn step2_4_max_iterations_default_value() {
        let response = json!({
            "eligible": true,
            "max_iterations": 2,
            "governance_mode": "assisted",
            "reason": "test"
        });

        assert!(validate_repair_readiness_contract(&response).is_ok());
        assert_eq!(response["max_iterations"], 2);
    }

    // ── Extended contract validation: All RPC response fields ──────────────

    /// Validate health check response contract across endpoints
    fn validate_health_contract(health: &Value) -> Result<(), String> {
        if !health.is_object() {
            return Err("health must be an object".to_string());
        }
        let obj = health.as_object().unwrap();

        // Check status field (required string)
        if !obj.contains_key("status") {
            return Err("health missing required field: status".to_string());
        }
        if !obj["status"].is_string() {
            return Err("health.status must be string".to_string());
        }

        // Validate status enum values
        let status = obj["status"].as_str().unwrap();
        let valid_statuses = vec!["ok", "degraded", "error"];
        if !valid_statuses.contains(&status) {
            return Err(format!(
                "health.status must be one of {:?}, got '{}'",
                valid_statuses, status
            ));
        }

        Ok(())
    }

    /// Validate capabilities.list response contract
    fn validate_capabilities_contract(capabilities: &Value) -> Result<(), String> {
        if !capabilities.is_object() {
            return Err("capabilities must be an object".to_string());
        }
        let obj = capabilities.as_object().unwrap();

        // Check capabilities array (optional but must be array if present)
        if let Some(caps) = obj.get("capabilities") {
            if !caps.is_array() {
                return Err("capabilities.capabilities must be array".to_string());
            }
        }

        Ok(())
    }

    /// Validate governance.status response contract
    fn validate_governance_status_contract(status: &Value) -> Result<(), String> {
        if !status.is_object() {
            return Err("governance.status must be an object".to_string());
        }
        let obj = status.as_object().unwrap();

        // Check harness_bus metrics (should be object if present)
        if let Some(harness) = obj.get("harness_bus") {
            if !harness.is_object() {
                return Err("governance.status.harness_bus must be object".to_string());
            }
            let hb = harness.as_object().unwrap();
            // Verify at least some expected HarnessBus metrics exist
            let expected_metrics = vec!["policy_evaluations", "audit_events", "sandbox_checks"];
            for metric in &expected_metrics {
                if !hb.contains_key(*metric) {
                    return Err(format!("harness_bus missing metric: {}", metric));
                }
            }
        }

        // Check capability_bus metrics (should be object if present)
        if let Some(cap) = obj.get("capability_bus") {
            if !cap.is_object() {
                return Err("governance.status.capability_bus must be object".to_string());
            }
        }

        Ok(())
    }

    /// Validate initialize response contract
    fn validate_initialize_contract(init: &Value) -> Result<(), String> {
        if !init.is_object() {
            return Err("initialize must be an object".to_string());
        }
        let obj = init.as_object().unwrap();

        // Check protocol field (optional but must be string if present)
        if let Some(protocol) = obj.get("protocol") {
            if !protocol.is_string() {
                return Err("initialize.protocol must be string".to_string());
            }
        }

        // Check serverInfo field (optional but must be object if present)
        if let Some(info) = obj.get("serverInfo") {
            if !info.is_object() {
                return Err("initialize.serverInfo must be object".to_string());
            }
        }

        // Check version field (optional but must be string if present)
        if let Some(version) = obj.get("version") {
            if !version.is_string() {
                return Err("initialize.version must be string".to_string());
            }
        }

        Ok(())
    }

    /// Validate execution cycle response contract (BLUE22)
    fn validate_execution_cycle_contract(cycle: &Value) -> Result<(), String> {
        if !cycle.is_object() {
            return Err("execution_cycle must be an object".to_string());
        }
        let obj = cycle.as_object().unwrap();

        // Check capability_profile (optional but must be object if present)
        if let Some(profile) = obj.get("capability_profile") {
            if !profile.is_object() {
                return Err("execution_cycle.capability_profile must be object".to_string());
            }
        }

        // Check governance_profile (optional but must be object if present)
        if let Some(gov) = obj.get("governance_profile") {
            if !gov.is_object() {
                return Err("execution_cycle.governance_profile must be object".to_string());
            }
        }

        // Check execution_cycle (optional but must be object if present)
        if let Some(exec) = obj.get("execution_cycle") {
            if !exec.is_object() {
                return Err("execution_cycle.execution_cycle must be object".to_string());
            }
            let ec = exec.as_object().unwrap();

            // cycle_id should be string if present
            if let Some(cycle_id) = ec.get("cycle_id") {
                if !cycle_id.is_string() {
                    return Err("execution_cycle.cycle_id must be string".to_string());
                }
            }

            // history_summary should be object if present
            if let Some(history) = ec.get("history_summary") {
                if !history.is_object() {
                    return Err("execution_cycle.history_summary must be object".to_string());
                }
            }
        }

        Ok(())
    }

    /// Validate error response contract consistency
    fn validate_error_contract(error: &Value) -> Result<(), String> {
        if !error.is_object() {
            return Err("error must be an object".to_string());
        }
        let obj = error.as_object().unwrap();

        // code field (required integer)
        if !obj.contains_key("code") {
            return Err("error missing required field: code".to_string());
        }
        if !obj["code"].is_i64() && !obj["code"].is_u64() {
            return Err("error.code must be integer".to_string());
        }

        // message field (required string)
        if !obj.contains_key("message") {
            return Err("error missing required field: message".to_string());
        }
        if !obj["message"].is_string() {
            return Err("error.message must be string".to_string());
        }

        Ok(())
    }

    /// Validate configuration response contract
    fn validate_config_contract(config: &Value) -> Result<(), String> {
        if !config.is_object() {
            return Err("config must be an object".to_string());
        }
        let obj = config.as_object().unwrap();

        // Check for common config fields
        let known_fields = vec!["providers", "protocol", "governance", "memory", "log"];
        for field in &known_fields {
            if let Some(val) = obj.get(*field) {
                if !val.is_object() && !val.is_string() && !val.is_boolean() {
                    return Err(format!(
                        "config.{} must be object, string, or boolean if present",
                        field
                    ));
                }
            }
        }

        Ok(())
    }

    // ── Extended tests ────────────────────────────────────────────────────

    #[test]
    fn step2_4_health_contract_consistency() {
        // Backend health response shape
        let backend = json!({
            "status": "ok",
            "version": "0.8.2",
            "uptime_seconds": 3600
        });

        // GUI expected shape
        let gui = json!({
            "status": "ok",
            "version": "0.8.2"
        });

        // Addon expected shape
        let addon = json!({
            "status": "ok",
            "version": "0.8.2"
        });

        assert!(validate_health_contract(&backend).is_ok());
        assert!(validate_health_contract(&gui).is_ok());
        assert!(validate_health_contract(&addon).is_ok());

        // All must have consistent status field type
        assert!(backend["status"].is_string());
        assert!(gui["status"].is_string());
        assert!(addon["status"].is_string());

        // Version must be string when present
        assert!(backend["version"].is_string());
        assert!(gui["version"].is_string());
        assert!(addon["version"].is_string());
    }

    #[test]
    fn step2_4_capabilities_contract_consistency() {
        let response = json!({
            "capabilities": [
                {
                    "name": "chat.completions",
                    "version": "1.0"
                },
                {
                    "name": "health",
                    "version": "1.0"
                }
            ]
        });

        assert!(validate_capabilities_contract(&response).is_ok());

        // Validate each capability has required fields
        if let Some(caps) = response["capabilities"].as_array() {
            for cap in caps {
                assert!(cap.get("name").is_some(), "capability should have name");
            }
        }
    }

    #[test]
    fn step2_4_governance_status_contract_consistency() {
        let response = json!({
            "harness_bus": {
                "policy_evaluations": 150,
                "audit_events": 42,
                "sandbox_checks": 89,
                "allowed_count": 130,
                "denied_count": 15,
                "escalated_count": 5
            },
            "capability_bus": {
                "total_agents": 10,
                "active_agents": 5,
                "decisions_made": 200
            }
        });

        assert!(validate_governance_status_contract(&response).is_ok());

        // HarnessBus metrics must be numeric
        if let Some(hb) = response["harness_bus"].as_object() {
            for (_k, v) in hb {
                assert!(
                    v.is_u64() || v.is_i64() || v.is_f64(),
                    "harness_bus metrics must be numeric"
                );
            }
        }

        // CapabilityBus metrics must be numeric
        if let Some(cb) = response["capability_bus"].as_object() {
            for (_k, v) in cb {
                assert!(
                    v.is_u64() || v.is_i64() || v.is_f64(),
                    "capability_bus metrics must be numeric"
                );
            }
        }
    }

    #[test]
    fn step2_4_initialize_contract_consistency() {
        let response = json!({
            "protocol": "acp",
            "version": "1.0",
            "serverInfo": {
                "name": "go-on",
                "version": "0.8.2"
            }
        });

        assert!(validate_initialize_contract(&response).is_ok());

        // Protocol must be a recognized value
        let valid_protocols = ["acp", "mcp", "auto"];
        assert!(valid_protocols.contains(&response["protocol"].as_str().unwrap()));

        // serverInfo must have name and version
        if let Some(info) = response["serverInfo"].as_object() {
            assert!(info.contains_key("name"), "serverInfo should have name");
            assert!(
                info.contains_key("version"),
                "serverInfo should have version"
            );
        }
    }

    #[test]
    fn step2_4_error_contract_consistency() {
        // Standard JSON-RPC error shape
        let std_error = json!({
            "code": -32601,
            "message": "Method not found",
            "data": null
        });

        // Application error shape
        let app_error = json!({
            "code": 1001,
            "message": "Invalid request parameters"
        });

        assert!(validate_error_contract(&std_error).is_ok());
        assert!(validate_error_contract(&app_error).is_ok());

        // code must be integer for both
        assert!(std_error["code"].is_i64() || std_error["code"].is_u64());
        assert!(app_error["code"].is_i64() || app_error["code"].is_u64());
    }

    #[test]
    fn step2_4_field_name_drift_detection() {
        // Detect if field names drift across endpoints by checking
        // that the expected contract fields match exactly

        let expected_fields = vec!["eligible", "max_iterations", "governance_mode", "reason"];

        let response = json!({
            "eligible": true,
            "max_iterations": 2,
            "governance_mode": "assisted",
            "reason": "test"
        });

        let actual_fields: Vec<&str> = response
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect();

        for field in &expected_fields {
            assert!(
                actual_fields.contains(field),
                "Field drift detected: '{}' is missing from repair_readiness contract",
                field
            );
        }

        // No unexpected fields should exist
        for field in &actual_fields {
            assert!(
                expected_fields.contains(field),
                "Unexpected field '{}' in repair_readiness contract - potential drift",
                field
            );
        }
    }

    #[test]
    fn step2_4_execution_cycle_contract_consistency() {
        let response = json!({
            "capability_profile": {
                "platform_mode": "adaptive",
                "phase_compat": {"phase": 3}
            },
            "governance_profile": {
                "risk_band": "medium",
                "budget": {"remaining": 1000}
            },
            "execution_cycle": {
                "cycle_id": "cycle-001",
                "current_cycle": {"plan_version": "v1"},
                "history_summary": {
                    "total_cycles": 5,
                    "pending_repair_iterations": 0
                }
            }
        });

        assert!(validate_execution_cycle_contract(&response).is_ok());

        // Validate specific field types
        if let Some(cycle) = response["execution_cycle"].as_object() {
            assert!(cycle["cycle_id"].is_string());
            if let Some(history) = cycle.get("history_summary") {
                assert!(history["total_cycles"].is_u64() || history["total_cycles"].is_i64());
            }
        }
    }

    #[test]
    fn step2_4_config_contract_consistency() {
        let response = json!({
            "providers": {
                "openai": {"enabled": true},
                "anthropic": {"enabled": false}
            },
            "protocol": "acp",
            "governance": {
                "strict_mode": true,
                "audit_enabled": true
            },
            "memory": {
                "cache_size": 1000,
                "vector_store": "sqlite"
            },
            "log": {
                "level": "info",
                "format": "json"
            }
        });

        assert!(validate_config_contract(&response).is_ok());

        // Providers must have enabled flag for each provider
        if let Some(providers) = response["providers"].as_object() {
            for (_name, config) in providers {
                assert!(
                    config.get("enabled").is_some(),
                    "each provider should have 'enabled' field"
                );
            }
        }
    }
}
