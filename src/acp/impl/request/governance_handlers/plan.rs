//! Governance plan handlers: plan.get, plan.update, and norms helper.

use super::*;

// ---------------------------------------------------------------------------
// governance.plan.get — retrieve current PUA enforcement plan
// ---------------------------------------------------------------------------

pub(crate) async fn handle_governance_plan_get(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    let plan = server
        .governance_deps
        .pua_enforcement_plan
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();

    send_result(server, request_id, json!({ "ok": true, "plan": plan })).await
}

// ---------------------------------------------------------------------------
// governance.plan.update — modify PUA enforcement plan
// ---------------------------------------------------------------------------

pub(crate) async fn handle_governance_plan_update(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let plan = match server.governance_deps.pua_enforcement_plan.lock() {
        Ok(mut guard) => {
            if let Some(level) = params.get("escalation_level").and_then(Value::as_str) {
                guard.escalation_level = level.to_string();
            }
            if let Some(items) = params.get("red_lines").and_then(Value::as_array) {
                guard.red_lines = items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect();
            }
            if let Some(items) = params.get("quality_compass").and_then(Value::as_array) {
                guard.quality_compass = items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect();
            }
            if let Some(items) = params.get("mandatory_safeguards").and_then(Value::as_array) {
                guard.mandatory_safeguards = items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect();
            }
            if let Some(items) = params.get("mandatory_evidence").and_then(Value::as_array) {
                guard.mandatory_evidence = items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(ToString::to_string)
                    .collect();
            }
            if let Some(stage_requirements) = params.get("stage_requirements") {
                guard.stage_requirements =
                    serde_json::from_value::<Vec<PuaStageRequirement>>(stage_requirements.clone())?;
            }
            guard.clone()
        }
        Err(_) => PuaEnforcementPlan::default(),
    };

    let event = super::audit::GovernanceAuditEvent {
        timestamp: crate::acp::prelude::now_ts().max(0) as u64,
        action: "governance.plan.update".to_string(),
        actor: "rpc".to_string(),
        result: "success".to_string(),
        detail: json!({
            "escalation_level": plan.escalation_level,
            "red_line_count": plan.red_lines.len(),
            "stage_requirement_count": plan.stage_requirements.len(),
            "mandatory_safeguards_count": plan.mandatory_safeguards.len(),
            "mandatory_evidence_count": plan.mandatory_evidence.len(),
        }),
    };
    let _ = super::audit::append_governance_audit_event(&event);

    send_result(server, request_id, json!({ "ok": true, "plan": plan })).await
}

/// Helper: extract tracked norms from a PUA enforcement plan.
pub(crate) fn norms_tracked_for(plan: &PuaEnforcementPlan) -> Vec<&str> {
    let mut sources = Vec::new();
    if !plan.quality_compass.is_empty() {
        sources.push("quality_compass");
    }
    if !plan.red_lines.is_empty() {
        sources.push("red_lines");
    }
    if !plan.mandatory_safeguards.is_empty() {
        sources.push("mandatory_safeguards");
    }
    if !plan.mandatory_evidence.is_empty() {
        sources.push("mandatory_evidence");
    }
    sources
}
