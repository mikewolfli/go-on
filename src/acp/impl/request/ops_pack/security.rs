//! Security baseline payload builder.
//!
//! `build_security_baseline_payload` is used internally by both
//! `handle_security_baseline` and `handle_release_readiness`.

use serde_json::{json, Value};

use super::super::*;

pub(super) fn build_security_baseline_payload(server: &AcpServer) -> Value {
    let config_summary =
        super::super::config_pack::governance_config_summary(server.config_path.as_deref());
    let entry_auth_enabled = config_summary
        .get("entry_auth_enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let entry_auth_key_configured = config_summary
        .get("entry_auth_key_configured")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let strict_enabled = config_summary
        .get("production_strict")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let strict_violations = config_summary
        .get("strict_violations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let exposed_http = server.runtime_config.acp_http_bind_addr.is_some();
    let ingress_status = if !exposed_http {
        "local-only"
    } else if entry_auth_enabled && entry_auth_key_configured {
        "hardened"
    } else {
        "risk"
    };

    let mut risk_items = Vec::new();
    if exposed_http && !entry_auth_enabled {
        risk_items.push(json!({
            "severity": "critical",
            "code": "entry_auth.disabled",
            "message": "runtime.acp_http_bind_addr is configured but entry auth is disabled",
            "suggestion": "Set runtime.entry_auth_enabled=true and configure entry auth key",
        }));
    }
    if entry_auth_enabled && !entry_auth_key_configured {
        risk_items.push(json!({
            "severity": "critical",
            "code": "entry_auth.key_missing",
            "message": "Entry auth is enabled but auth key env is missing",
            "suggestion": "Set runtime.entry_auth_api_key_env in process environment",
        }));
    }
    if !strict_enabled {
        risk_items.push(json!({
            "severity": "warn",
            "code": "production_strict.disabled",
            "message": "runtime.production_strict is disabled",
            "suggestion": "Enable runtime.production_strict=true to fail fast on unsafe config",
        }));
    }
    if !strict_violations.is_empty() {
        risk_items.push(json!({
            "severity": if strict_enabled { "critical" } else { "warn" },
            "code": "production_strict.violations",
            "message": format!("{} strict violation(s) detected", strict_violations.len()),
            "violations": strict_violations,
            "suggestion": "Fix strict violations and re-run runtime.health / security.baseline",
        }));
    }

    let level = if risk_items
        .iter()
        .any(|item| item.get("severity").and_then(Value::as_str) == Some("critical"))
    {
        "critical"
    } else if risk_items
        .iter()
        .any(|item| item.get("severity").and_then(Value::as_str) == Some("warn"))
    {
        "warn"
    } else {
        "ok"
    };

    json!({
        "ok": true,
        "baseline": {
            "level": level,
            "ingress_status": ingress_status,
            "exposed_http": exposed_http,
            "entry_auth": {
                "enabled": entry_auth_enabled,
                "key_env": server.runtime_config.entry_auth_api_key_env,
                "key_configured": entry_auth_key_configured,
            },
            "rate_limit": {
                "rpm": server.runtime_config.entry_rate_limit_rpm,
                "burst": server.runtime_config.entry_rate_limit_burst,
            },
            "production_strict": {
                "enabled": strict_enabled,
                "violation_count": strict_violations.len(),
                "violations": strict_violations,
            },
            "risk_count": risk_items.len(),
            "risks": risk_items,
        },
    })
}

pub(super) async fn handle_observability_alerts(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let max_alerts = params
        .get("limit")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(20)
        .clamp(1, 200);

    let status = server.get_status();
    let metrics = server.observability.metrics.snapshot();
    let lock_components = server.observability.lock_monitor.snapshot();
    let lock_summary = summarize_lock_health(&lock_components);
    let degraded_services = collect_degraded_services(server);
    let open_breakers = status
        .circuit_breakers
        .iter()
        .filter(|item| item.state.eq_ignore_ascii_case("open"))
        .count();

    let mut alerts = Vec::new();
    if !status.lifecycle.is_healthy {
        alerts.push(json!({
            "severity": "critical",
            "code": "runtime.unhealthy",
            "message": "Runtime lifecycle is unhealthy",
            "value": {
                "uptime_seconds": status.lifecycle.uptime_seconds,
                "shutdown_requested": status.lifecycle.shutdown_requested,
            },
            "suggestion": "Inspect runtime.health and recent trace events before accepting new traffic",
        }));
    }

    if open_breakers > 0 {
        alerts.push(json!({
            "severity": "critical",
            "code": "breaker.open",
            "message": format!("{} circuit breakers are open", open_breakers),
            "value": {"open_count": open_breakers},
            "suggestion": "Use breaker.status and breaker.recovery to restore degraded services",
        }));
    }

    if !degraded_services.is_empty() {
        alerts.push(json!({
            "severity": "warn",
            "code": "service.degraded",
            "message": format!("{} services are degraded", degraded_services.len()),
            "value": {
                "degraded_count": degraded_services.len(),
                "services": degraded_services,
            },
            "suggestion": "Fallback to secondary agents and run breaker.recovery after stabilizing dependencies",
        }));
    }

    let timeout_total = metrics.agent_timeout_failures_total
        + metrics.review_gate_timeout_total
        + metrics.runtime_probe_timeout_total;
    if timeout_total > 0 {
        alerts.push(json!({
            "severity": "warn",
            "code": "timeout.spike",
            "message": "Timeout counters are above baseline",
            "value": {
                "total": timeout_total,
                "agent_request_total": metrics.agent_timeout_failures_total,
                "review_gate_total": metrics.review_gate_timeout_total,
                "runtime_probe_total": metrics.runtime_probe_timeout_total,
            },
            "suggestion": "Check trace.metrics slow paths and tune request_timeout_seconds for affected phases",
        }));
    }

    if lock_summary.status == "warn" {
        alerts.push(json!({
            "severity": "warn",
            "code": "lock.contention",
            "message": "Lock monitor detected contention or poison recovery",
            "value": {
                "poisoned_total": lock_summary.poisoned_total,
                "recovered_total": lock_summary.recovered_total,
                "slow_wait_total": lock_summary.slow_wait_total,
                "max_wait_ms": lock_summary.max_wait_ms,
                "components_tracked": lock_summary.components_tracked,
            },
            "suggestion": "Review lock-heavy code paths and consider reducing critical section duration",
        }));
    }

    if alerts.is_empty() {
        alerts.push(json!({
            "severity": "info",
            "code": "baseline.ok",
            "message": "No active runtime alerts",
            "value": {
                "total_requests": metrics.total_requests,
                "successful_requests": metrics.successful_requests,
            },
            "suggestion": "Continue periodic quality.baseline and trace.metrics checks",
        }));
    }

    if alerts.len() > max_alerts {
        alerts.truncate(max_alerts);
    }

    let counts = alerts
        .iter()
        .fold((0usize, 0usize, 0usize), |mut acc, alert| {
            match alert
                .get("severity")
                .and_then(Value::as_str)
                .unwrap_or("info")
            {
                "critical" => acc.0 += 1,
                "warn" | "warning" => acc.1 += 1,
                _ => acc.2 += 1,
            }
            acc
        });

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "alerts": {
                "critical": counts.0,
                "warn": counts.1,
                "info": counts.2,
                "total": alerts.len(),
                "items": alerts,
            },
        }),
    )
    .await
}

pub(super) async fn handle_security_baseline(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    send_result(server, request_id, build_security_baseline_payload(server)).await
}
