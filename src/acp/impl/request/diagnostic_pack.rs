//! Diagnostic pack for ACP metrics/health collection.
//!
//! Extracted from runtime_pack.rs — provides lock health summary
//! utilities used by status_pack and request dispatch.
//!
//! Note: Per lock-optimization pass (log-20260622-5), fine-grained lock
//! monitoring data is no longer collected at runtime. The lock-status
//! handlers return empty/sensible defaults.

use super::*;

/// Summary of lock health across all tracked components.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub(super) struct LockHealthSummary {
    pub(super) status: &'static str,
    pub(super) poisoned_total: u64,
    pub(super) recovered_total: u64,
    pub(super) slow_wait_total: u64,
    pub(super) max_wait_ms: f64,
    pub(super) components_tracked: usize,
}

/// Aggregate per-component lock snapshots into a single health summary.
///
/// The summary status is `"warn"` when any component has been poisoned,
/// any slow wait has been recorded, or any lock wait exceeded 5 ms.
/// Otherwise the status is `"healthy"`.
pub(super) fn summarize_lock_health(components: &[LockHealthSummary]) -> LockHealthSummary {
    let poisoned_total = components
        .iter()
        .map(|item| item.poisoned_total)
        .sum::<u64>();
    let recovered_total = components
        .iter()
        .map(|item| item.recovered_total)
        .sum::<u64>();
    let slow_wait_total = components
        .iter()
        .map(|item| item.slow_wait_total)
        .sum::<u64>();
    let max_wait_ms = components
        .iter()
        .map(|item| item.max_wait_ms)
        .fold(0.0_f64, f64::max);
    let status = if poisoned_total > 0 || slow_wait_total > 0 || max_wait_ms >= 5.0 {
        "warn"
    } else {
        "healthy"
    };

    LockHealthSummary {
        status,
        poisoned_total,
        recovered_total,
        slow_wait_total,
        max_wait_ms,
        components_tracked: components.len(),
    }
}

/// Handle "lock.status" request — return empty lock status (monitoring removed).
pub(super) async fn handle_lock_status(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let summary = LockHealthSummary {
        status: "healthy",
        poisoned_total: 0,
        recovered_total: 0,
        slow_wait_total: 0,
        max_wait_ms: 0.0,
        components_tracked: 0,
    };

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "locks": {
                "status": summary.status,
                "poisoned_total": summary.poisoned_total,
                "recovered_total": summary.recovered_total,
                "slow_wait_total": summary.slow_wait_total,
                "max_wait_ms": summary.max_wait_ms,
                "components_tracked": summary.components_tracked,
                "contention_top": [],
                "components": [],
            },
        }),
    )
    .await
}

/// Handle "observability.alerts" request — return current observability alerts.
pub(super) async fn handle_observability_alerts(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let metrics = server.observability.metrics.snapshot();
    let mut alerts: Vec<Value> = Vec::new();

    if metrics.agent_timeout_failures_total > 0 {
        alerts.push(json!({
            "severity": "warn",
            "component": "agent",
            "type": "timeout",
            "total": metrics.agent_timeout_failures_total,
        }));
    }
    if metrics.review_gate_timeout_total > 0 {
        alerts.push(json!({
            "severity": "warn",
            "component": "review_gate",
            "type": "timeout",
            "total": metrics.review_gate_timeout_total,
        }));
    }
    if metrics.runtime_probe_timeout_total > 0 {
        alerts.push(json!({
            "severity": "warn",
            "component": "runtime_probe",
            "type": "timeout",
            "total": metrics.runtime_probe_timeout_total,
        }));
    }

    let alert_total = alerts.len();

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "alerts": {
                "items": alerts,
                "total": alert_total,
            },
            "total": alert_total,
        }),
    )
    .await
}
