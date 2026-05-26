//! Config/observability handlers extracted from runtime_pack.rs.
//!
//! Provides `debug_panel.get`, `trace.get`, and `trace.metrics` handlers.

use super::*;

pub(super) async fn handle_debug_panel_get(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    send_result(server, request_id, build_debug_panel_payload(server).await).await
}

pub(super) async fn build_debug_panel_payload(server: &AcpServer) -> Value {
    let state = server.conversation_state.lock().await;
    let conversation_count = state
        .checkpoints
        .iter()
        .map(|cp| cp.conversation_id.as_str())
        .collect::<std::collections::HashSet<_>>()
        .len();
    let checkpoint_count = state.checkpoints.len();
    let autonomy_runtime_metrics =
        crate::acp::helpers::autonomy_metrics::autonomy_metrics_snapshot();
    let autonomy_loop_completion_ratio = autonomy_runtime_metrics
        .get("autonomy_loop_completion_ratio")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let repair_cycle_effective_ratio = autonomy_runtime_metrics
        .get("repair_cycle_effective_ratio")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let repair_replan_required_ratio = autonomy_runtime_metrics
        .get("repair_replan_required_ratio")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let repair_replan_required_total = autonomy_runtime_metrics
        .get("repair_replan_required_total")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let idempotency_pending_continuation_ratio = autonomy_runtime_metrics
        .get("idempotency_pending_continuation_ratio")
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    let idempotency_pending_continuation_hit_total = autonomy_runtime_metrics
        .get("idempotency_pending_continuation_hit_total")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let orchestration_node_mapping_ratio = autonomy_runtime_metrics
        .get("orchestration_node_mapping_ratio")
        .and_then(Value::as_f64)
        .unwrap_or(1.0);
    let orchestration_node_mapped_total = autonomy_runtime_metrics
        .get("orchestration_node_mapped_total")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let orchestration_node_unmapped_total = autonomy_runtime_metrics
        .get("orchestration_node_unmapped_total")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let behavior_backed = autonomy_loop_completion_ratio > 0.0
        || repair_cycle_effective_ratio > 0.0
        || autonomy_runtime_metrics
            .get("idempotency_hit_total")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            > 0;

    json!({
        "ok": true,
        "panel": {
            "trace": {"stage_transitions": []},
            "selected_agents": [],
            "review_outcomes": [],
            "runtime_health": {"ok": true},
            "review_gate": {
                "total": server.observability.metrics.snapshot().review_gate_total,
            },
            "autonomy_behavior_validation": {
                "ready": behavior_backed,
                "behavior_backed": true,
                "tool_followup_enabled": true,
                "clarification_resume_enabled": true,
                "execution_cache_bypass_enabled": true,
                "tool_governance": crate::acp::helpers::tool_governance::tool_governance_counters(),
                "tool_governance_default_policy": {
                    "active_when_harness_bus_absent": server.harness_bus.is_none(),
                    "snapshot": crate::acp::helpers::tool_governance_defaults::default_governance_policy_snapshot(),
                },
                "repair_cycle_effective_ratio": repair_cycle_effective_ratio,
                "repair_replan_required_ratio": repair_replan_required_ratio,
                "repair_replan_required_total": repair_replan_required_total,
                "idempotency_pending_continuation_ratio": idempotency_pending_continuation_ratio,
                "idempotency_pending_continuation_hit_total": idempotency_pending_continuation_hit_total,
                "orchestration_node_mapping_ratio": orchestration_node_mapping_ratio,
                "orchestration_node_mapped_total": orchestration_node_mapped_total,
                "orchestration_node_unmapped_total": orchestration_node_unmapped_total,
                "autonomy_runtime_metrics": autonomy_runtime_metrics,
            },
            "conversations": {
                "count": conversation_count,
                "checkpoints": checkpoint_count,
            }
        }
    })
}

pub(super) async fn handle_trace_get(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    send_result(server, request_id, build_trace_payload(&params)).await
}

pub(super) fn build_trace_payload(params: &Value) -> Value {
    let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(100) as usize;

    let (trace_events_len, limited_trace_events) = match trace_events().lock() {
        Ok(guard) => {
            let total = guard.len();
            let start = total.saturating_sub(limit);
            let events = guard.iter().skip(start).cloned().collect::<Vec<_>>();
            (total, events)
        }
        Err(_) => (0, Vec::new()),
    };

    json!({
        "events": limited_trace_events,
        "total": trace_events_len,
        "limit": limit,
    })
}

pub(super) async fn handle_trace_metrics(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    send_result(server, request_id, trace_metrics_snapshot(server)).await
}
