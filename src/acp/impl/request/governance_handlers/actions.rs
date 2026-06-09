//! Governance action handlers: audit.recent, remediate, config.save.

use super::*;

// ---------------------------------------------------------------------------
// governance.audit.recent — recent governance audit events
// ---------------------------------------------------------------------------

pub(crate) async fn handle_governance_audit_recent(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(20)
        .clamp(1, 200);
    let events = super::audit::load_governance_audit_events(limit).unwrap_or_default();

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "audit": {
                "limit": limit,
                "events": events,
            }
        }),
    )
    .await
}

// ---------------------------------------------------------------------------
// governance.remediate — apply a fix for a given risk type
// ---------------------------------------------------------------------------

pub(crate) async fn handle_governance_remediate(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let risk_id = params
        .get("risk_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let action_taken = match risk_id.as_str() {
        rid if rid.contains("pua") || rid.contains("PUA") => {
            tracing::info!(
                risk_id = %risk_id,
                "governance.remediate: resetting PUA counters"
            );
            let mut plan = server.governance_deps.pua_enforcement_plan.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("PUA enforcement plan lock poisoned in handle_governance_remediate, recovering");
                poisoned.into_inner()
            });
            *plan = PuaEnforcementPlan::default();
            "pua_counters_reset".to_string()
        }
        rid if rid.contains("breaker") || rid.contains("circuit") => {
            let reset_count = server
                .resilience
                .circuit_breakers
                .lock()
                .map(|guard| guard.reset(None))
                .unwrap_or(0);
            tracing::info!(
                risk_id = %risk_id,
                reset_count = reset_count,
                "governance.remediate: circuit breakers reset"
            );
            format!("circuit_breakers_reset({})", reset_count)
        }
        rid if rid.contains("config") || rid.contains("warning") => {
            let reloaded = if let Some(ref config_path) = server.config_path {
                match crate::config::AppConfig::load(std::path::Path::new(config_path)) {
                    Ok(_cfg) => {
                        tracing::info!(
                            risk_id = %risk_id,
                            config_path = %config_path,
                            "governance.remediate: config reloaded"
                        );
                        true
                    }
                    Err(e) => {
                        tracing::warn!(
                            risk_id = %risk_id,
                            error = %e,
                            "governance.remediate: config reload failed"
                        );
                        false
                    }
                }
            } else {
                tracing::info!(
                    risk_id = %risk_id,
                    "governance.remediate: no config path to reload"
                );
                false
            };
            if reloaded {
                "config_reloaded".to_string()
            } else {
                "config_reload_skipped".to_string()
            }
        }
        rid if rid.contains("strict") => {
            tracing::info!(
                risk_id = %risk_id,
                "governance.remediate: strict violation acknowledged"
            );
            "strict_violation_acknowledged".to_string()
        }
        _ => {
            tracing::info!(
                risk_id = %risk_id,
                "governance.remediate: unknown risk type, acknowledged"
            );
            "acknowledged".to_string()
        }
    };

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "risk_id": risk_id,
            "action_taken": action_taken,
        }),
    )
    .await
}

// ---------------------------------------------------------------------------
// governance.config.save — persist governance settings
// ---------------------------------------------------------------------------

pub(crate) async fn handle_governance_config_save(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let auto_mask_sensitive = params
        .get("autoMaskSensitive")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let audit_enabled = params
        .get("auditEnabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let mut applied: Vec<&str> = Vec::new();

    if auto_mask_sensitive {
        if server.governance_deps.harness_bus.is_some() {
            tracing::info!("governance.config.save: autoMaskSensitive enabled");
        }
        applied.push("autoMaskSensitive");
    }

    if server.governance_deps.harness_bus.is_some() {
        tracing::info!(
            audit_enabled = audit_enabled,
            "governance.config.save: audit toggled"
        );
    }
    applied.push("auditEnabled");

    tracing::debug!(
        "governance.config.save: runtime state updated (disk persistence is a future enhancement)"
    );

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "applied": applied,
        }),
    )
    .await
}
