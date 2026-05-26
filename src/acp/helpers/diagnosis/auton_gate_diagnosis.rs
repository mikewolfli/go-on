//! Diagnostic helpers for AUTON gate readiness chains.
//!
//! These helpers keep blocked-path reasoning out of giant request handlers.

use serde_json::{json, Value};

/// Shared runtime signals used by AUTON gate diagnostics.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // F-GAP-17 — reserved for AUTON gate readiness diagnostics
pub struct AutonGateSignals {
    pub blue30_release_closure_ready: bool,
    pub blue33_release_closure_ready: bool,
    pub observability_gate: bool,
    pub runtime_healthy: bool,
    pub open_breakers: usize,
    pub requests_ok: bool,
}

/// Build blocker list for AUTON start gate: `autonomy_boundary_governance`.
#[allow(dead_code)] // F-GAP-17 — reserved for AUTON gate readiness diagnostics
pub fn autonomy_boundary_blockers(signals: AutonGateSignals) -> Vec<Value> {
    let mut blockers = Vec::new();

    if !signals.blue30_release_closure_ready {
        blockers.push(json!({
            "code": "blue30.closure_pending",
            "message": "BLUE30 release closure is not ready",
            "suggestion": "Clear BLUE30 readiness gates before enabling AUTON boundary governance",
        }));
    }

    if !signals.observability_gate {
        blockers.push(json!({
            "code": "observability.gate_failed",
            "message": "Observability baseline gate is not satisfied",
            "suggestion": "Stabilize trace/metrics and lock-health signal collection",
        }));
    }

    if !signals.runtime_healthy {
        blockers.push(json!({
            "code": "runtime.unhealthy",
            "message": "Runtime lifecycle is unhealthy",
            "suggestion": "Recover runtime health before autonomous boundary escalation",
        }));
    }

    if signals.open_breakers > 0 {
        blockers.push(json!({
            "code": "breaker.open",
            "message": format!("{} circuit breaker(s) are open", signals.open_breakers),
            "suggestion": "Run breaker recovery and verify degraded services are restored",
        }));
    }

    if !signals.requests_ok {
        blockers.push(json!({
            "code": "request.failure_ratio",
            "message": "Request failure baseline is above allowed threshold",
            "suggestion": "Reduce runtime failures before enabling AUTON stage",
        }));
    }

    blockers
}

/// Build blocker list for AUTON scope gate: `autonomy_scope_matrix`.
#[allow(dead_code)] // F-GAP-17 — reserved for AUTON gate readiness diagnostics
pub fn autonomy_scope_blockers(signals: AutonGateSignals) -> Vec<Value> {
    let mut blockers = Vec::new();

    if !signals.blue33_release_closure_ready {
        blockers.push(json!({
            "code": "blue33.closure_pending",
            "message": "BLUE33 release closure is not ready",
            "suggestion": "Complete BLUE33 closure gates before activating scope matrix",
        }));
    }

    if !signals.observability_gate {
        blockers.push(json!({
            "code": "observability.gate_failed",
            "message": "Observability baseline gate is not satisfied",
            "suggestion": "Restore telemetry and lock monitor baseline",
        }));
    }

    if !signals.runtime_healthy {
        blockers.push(json!({
            "code": "runtime.unhealthy",
            "message": "Runtime lifecycle is unhealthy",
            "suggestion": "Recover runtime before enabling auto-vs-human scope matrix",
        }));
    }

    if signals.open_breakers > 0 {
        blockers.push(json!({
            "code": "breaker.open",
            "message": format!("{} circuit breaker(s) are open", signals.open_breakers),
            "suggestion": "Close open breakers before redline runtime promotion",
        }));
    }

    if !signals.requests_ok {
        blockers.push(json!({
            "code": "request.failure_ratio",
            "message": "Request failure baseline is above allowed threshold",
            "suggestion": "Restore success baseline for AUTON scope escalation",
        }));
    }

    blockers
}
