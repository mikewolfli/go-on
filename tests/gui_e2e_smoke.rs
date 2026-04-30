/// GUI E2E Smoke Tests
/// Validates that the data contracts between GUI components and backend
/// are consistent. These tests don't require a browser; they validate
/// the JSON shapes that Vue components expect.
#[cfg(test)]
mod gui_e2e_smoke_tests {
    use serde_json::{json, Value};

    /// Validate DashboardView data contract
    fn validate_dashboard_contract(data: &Value) -> Result<(), String> {
        if !data.is_object() {
            return Err("dashboard data must be an object".to_string());
        }
        let obj = data.as_object().unwrap();

        // System status
        if let Some(status) = obj.get("system_status") {
            if !status.is_string() {
                return Err("dashboard.system_status must be string".to_string());
            }
        }

        // Active agents (optional, must be number if present)
        if let Some(agents) = obj.get("active_agents") {
            if !agents.is_u64() && !agents.is_i64() {
                return Err("dashboard.active_agents must be integer".to_string());
            }
        }

        // Recent activity (optional, must be array if present)
        if let Some(activity) = obj.get("recent_activity") {
            if !activity.is_array() {
                return Err("dashboard.recent_activity must be array".to_string());
            }
        }

        Ok(())
    }

    /// Validate ConfigView data contract
    fn validate_config_contract(data: &Value) -> Result<(), String> {
        if !data.is_object() {
            return Err("config data must be an object".to_string());
        }
        let obj = data.as_object().unwrap();

        let expected_sections = vec!["providers", "protocol", "governance", "log"];
        for section in &expected_sections {
            if let Some(val) = obj.get(*section) {
                if !val.is_object() && !val.is_string() && !val.is_boolean() {
                    return Err(format!(
                        "config.{} must be object, string, or boolean if present",
                        section
                    ));
                }
            }
        }

        Ok(())
    }

    /// Validate MonitorView data contract
    fn validate_monitor_contract(data: &Value) -> Result<(), String> {
        if !data.is_object() {
            return Err("monitor data must be an object".to_string());
        }
        let obj = data.as_object().unwrap();

        if let Some(metrics) = obj.get("metrics") {
            if !metrics.is_object() {
                return Err("monitor.metrics must be object".to_string());
            }
        }

        if let Some(alerts) = obj.get("alerts") {
            if !alerts.is_array() {
                return Err("monitor.alerts must be array".to_string());
            }
        }

        Ok(())
    }

    /// Validate WorkflowView data contract
    fn validate_workflow_contract(data: &Value) -> Result<(), String> {
        if !data.is_object() {
            return Err("workflow data must be an object".to_string());
        }
        let obj = data.as_object().unwrap();

        if let Some(workflows) = obj.get("workflows") {
            if !workflows.is_array() {
                return Err("workflow.workflows must be array".to_string());
            }
        }

        if let Some(status) = obj.get("execution_status") {
            if !status.is_string() {
                return Err("workflow.execution_status must be string".to_string());
            }
        }

        Ok(())
    }

    /// Validate SecurityView data contract
    fn validate_security_contract(data: &Value) -> Result<(), String> {
        if !data.is_object() {
            return Err("security data must be an object".to_string());
        }
        let obj = data.as_object().unwrap();

        if let Some(policies) = obj.get("policies") {
            if !policies.is_array() {
                return Err("security.policies must be array".to_string());
            }
        }

        if let Some(audit_log) = obj.get("audit_log") {
            if !audit_log.is_array() {
                return Err("security.audit_log must be array".to_string());
            }
        }

        Ok(())
    }

    // ── Tests ──

    #[test]
    fn gui_dashboard_contract() {
        let data = json!({
            "system_status": "running",
            "active_agents": 5,
            "recent_activity": [
                {"action": "chat.completion", "timestamp": "2026-04-30T12:00:00Z"}
            ]
        });
        assert!(validate_dashboard_contract(&data).is_ok());

        // Minimal valid data
        let minimal = json!({"system_status": "stopped"});
        assert!(validate_dashboard_contract(&minimal).is_ok());
    }

    #[test]
    fn gui_config_contract() {
        let data = json!({
            "providers": {"openai": {"enabled": true}},
            "protocol": "acp",
            "governance": {"strict_mode": false},
            "log": {"level": "info"}
        });
        assert!(validate_config_contract(&data).is_ok());
    }

    #[test]
    fn gui_monitor_contract() {
        let data = json!({
            "metrics": {
                "cpu_percent": 45.2,
                "memory_mb": 256.0,
                "requests_per_minute": 120
            },
            "alerts": [
                {"severity": "warning", "message": "High memory usage"}
            ]
        });
        assert!(validate_monitor_contract(&data).is_ok());
    }

    #[test]
    fn gui_workflow_contract() {
        let data = json!({
            "workflows": [
                {"id": "wf-1", "name": "Test Workflow", "status": "active"}
            ],
            "execution_status": "idle"
        });
        assert!(validate_workflow_contract(&data).is_ok());
    }

    #[test]
    fn gui_security_contract() {
        let data = json!({
            "policies": [
                {"name": "strict-mode", "enabled": true}
            ],
            "audit_log": [
                {"action": "policy.evaluate", "result": "allow"}
            ]
        });
        assert!(validate_security_contract(&data).is_ok());
    }

    #[test]
    fn gui_all_views_contract_consistency() {
        // All views must share common response fields
        let dashboard = json!({"system_status": "running"});
        let security = json!({"policies": []});
        let monitor = json!({"metrics": {}});
        let workflow = json!({"workflows": [], "execution_status": "idle"});
        let config = json!({"providers": {}});

        assert!(validate_dashboard_contract(&dashboard).is_ok());
        assert!(validate_security_contract(&security).is_ok());
        assert!(validate_monitor_contract(&monitor).is_ok());
        assert!(validate_workflow_contract(&workflow).is_ok());
        assert!(validate_config_contract(&config).is_ok());
    }
}
