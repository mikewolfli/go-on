use super::*;

pub(super) async fn handle_breaker_status(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    let breakers = server
        .circuit_breakers
        .lock()
        .map(|guard| guard.snapshots())
        .unwrap_or_default();
    let open_count = breakers
        .iter()
        .filter(|item| item.state.eq_ignore_ascii_case("open"))
        .count();
    let degraded_services = collect_degraded_services(server);
    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "open_count": open_count,
            "degraded_count": degraded_services.len(),
            "degraded_services": degraded_services,
            "breakers": breakers,
        }),
    )
    .await
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
    let metrics = server.metrics.snapshot();
    let lock_components = server.lock_monitor.snapshot();
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
    let config_summary =
        super::config_pack::governance_config_summary(server.config_path.as_deref());
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

    send_result(
        server,
        request_id,
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
        }),
    )
    .await
}

fn classify_harness_suite(name: &str) -> &'static str {
    let lowered = name.to_ascii_lowercase();
    if lowered.contains("adversarial") || lowered.contains("fault") || lowered.contains("chaos") {
        "adversarial"
    } else if lowered.contains("long-chain") || lowered.contains("long_chain") {
        "long_chain"
    } else if lowered.contains("smoke")
        || lowered.contains("runtime-health")
        || lowered.contains("quality-benchmark")
    {
        "smoke"
    } else {
        "regression"
    }
}

pub(super) async fn handle_harness_status(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let fixed_seed = params
        .get("seed")
        .and_then(Value::as_u64)
        .unwrap_or(20260415);

    let mut smoke = Vec::new();
    let mut regression = Vec::new();
    let mut adversarial = Vec::new();
    let mut long_chain = Vec::new();
    let mut warnings = Vec::new();

    let requests_root = Path::new("requests");
    match fs::read_dir(requests_root) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let path = entry.path();
                let is_ndjson = path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| ext.eq_ignore_ascii_case("ndjson"))
                    .unwrap_or(false);
                if !is_ndjson {
                    continue;
                }
                let Some(name) = path
                    .file_name()
                    .and_then(|item| item.to_str())
                    .map(|item| item.to_string())
                else {
                    continue;
                };

                match classify_harness_suite(&name) {
                    "smoke" => smoke.push(name),
                    "adversarial" => adversarial.push(name),
                    "long_chain" => long_chain.push(name),
                    _ => regression.push(name),
                }
            }
            smoke.sort();
            regression.sort();
            adversarial.sort();
            long_chain.sort();
        }
        Err(err) => {
            warnings.push(format!("failed to read requests directory: {err}"));
        }
    }

    let scenario_total = smoke.len() + regression.len() + adversarial.len() + long_chain.len();
    let metrics = server.metrics.snapshot();
    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "harness": {
                "fixed_seed": fixed_seed,
                "scenario_total": scenario_total,
                "suites": {
                    "smoke": {
                        "count": smoke.len(),
                        "files": smoke,
                    },
                    "regression": {
                        "count": regression.len(),
                        "files": regression,
                    },
                    "adversarial": {
                        "count": adversarial.len(),
                        "files": adversarial,
                    },
                    "long_chain": {
                        "count": long_chain.len(),
                        "files": long_chain,
                    },
                },
                "scorecard": [
                    {
                        "dimension": "correctness",
                        "target": "all scenarios pass without rpc error",
                        "status": "tracked",
                    },
                    {
                        "dimension": "stability",
                        "target": "runtime.health remains healthy across suites",
                        "status": "tracked",
                    },
                    {
                        "dimension": "latency",
                        "target": "p95 bounded by phase timeout budget",
                        "status": "tracked",
                    },
                    {
                        "dimension": "cost",
                        "target": "timeout spikes remain within baseline",
                        "status": "tracked",
                    },
                    {
                        "dimension": "safety",
                        "target": "security.baseline level stays warn/ok before deploy",
                        "status": "tracked",
                    }
                ],
                "runtime_snapshot": {
                    "total_requests": metrics.total_requests,
                    "failed_requests": metrics.failed_requests,
                    "agent_timeout_failures_total": metrics.agent_timeout_failures_total,
                    "review_gate_timeout_total": metrics.review_gate_timeout_total,
                    "runtime_probe_timeout_total": metrics.runtime_probe_timeout_total,
                },
                "warnings": warnings,
            },
        }),
    )
    .await
}

pub(super) async fn handle_breaker_reset(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let target = params
        .get("agent")
        .or_else(|| params.get("name"))
        .and_then(Value::as_str);
    let reset_count = server
        .circuit_breakers
        .lock()
        .map(|guard| guard.reset(target))
        .unwrap_or(0);
    let breakers = server
        .circuit_breakers
        .lock()
        .map(|guard| guard.snapshots())
        .unwrap_or_default();

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "removed": reset_count,
            "target": target,
            "breakers": breakers,
        }),
    )
    .await
}

fn health_status_label(status: crate::failure_prevention::HealthStatus) -> &'static str {
    match status {
        crate::failure_prevention::HealthStatus::Healthy => "healthy",
        crate::failure_prevention::HealthStatus::Degraded => "degraded",
        crate::failure_prevention::HealthStatus::Unhealthy => "unhealthy",
    }
}

fn circuit_state_label(state: crate::failure_prevention::CircuitBreakerState) -> &'static str {
    match state {
        crate::failure_prevention::CircuitBreakerState::Closed => "closed",
        crate::failure_prevention::CircuitBreakerState::Open => "open",
        crate::failure_prevention::CircuitBreakerState::HalfOpen => "half-open",
    }
}

fn degradation_level_label(level: crate::failure_prevention::DegradationLevel) -> &'static str {
    match level {
        crate::failure_prevention::DegradationLevel::None => "none",
        crate::failure_prevention::DegradationLevel::Minimal => "minimal",
        crate::failure_prevention::DegradationLevel::Moderate => "moderate",
        crate::failure_prevention::DegradationLevel::Significant => "significant",
        crate::failure_prevention::DegradationLevel::Critical => "critical",
    }
}

fn recovery_action(
    status: crate::failure_prevention::HealthStatus,
    level: crate::failure_prevention::DegradationLevel,
) -> &'static str {
    if matches!(status, crate::failure_prevention::HealthStatus::Unhealthy)
        || matches!(level, crate::failure_prevention::DegradationLevel::Critical)
    {
        "reset_breaker_and_fallback"
    } else if matches!(status, crate::failure_prevention::HealthStatus::Degraded)
        || matches!(
            level,
            crate::failure_prevention::DegradationLevel::Significant
        )
    {
        "degrade_to_secondary_agent"
    } else {
        "observe"
    }
}

fn collect_degraded_services(server: &AcpServer) -> Vec<Value> {
    server
        .failure_prevention
        .lock()
        .map(|fp| {
            let mut services = fp.get_health_report();
            services.sort_by(|a, b| a.service_name.cmp(&b.service_name));
            services
                .into_iter()
                .filter_map(|health| {
                    let circuit = fp.get_circuit_state(&health.service_name);
                    let level = fp.get_degradation_strategy(&health.service_name);
                    let should_recover = !matches!(
                        health.status,
                        crate::failure_prevention::HealthStatus::Healthy
                    ) || !matches!(
                        circuit,
                        crate::failure_prevention::CircuitBreakerState::Closed
                    ) || fp.should_degrade(&health.service_name);
                    if !should_recover {
                        return None;
                    }

                    Some(json!({
                        "service": health.service_name,
                        "health_status": health_status_label(health.status),
                        "circuit_state": circuit_state_label(circuit),
                        "degradation_level": degradation_level_label(level),
                        "success_rate": health.success_rate,
                        "error_rate": health.error_rate,
                        "avg_latency_ms": health.avg_latency_ms,
                        "recommended_action": recovery_action(health.status, level),
                    }))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

pub(super) async fn handle_breaker_recovery(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let target = params
        .get("agent")
        .or_else(|| params.get("name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let dry_run = params
        .get("dry_run")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let degraded_before = collect_degraded_services(server);
    let candidates = degraded_before
        .iter()
        .filter_map(|item| {
            item.get("service")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .filter(|service| target.map(|t| t == service).unwrap_or(true))
        .collect::<Vec<_>>();

    let (recovered_services, breaker_reset_count) = if dry_run {
        (Vec::new(), 0)
    } else {
        let recovered_services = server
            .failure_prevention
            .lock()
            .map(|mut fp| fp.recover(target))
            .unwrap_or_default();
        let breaker_reset_count = server
            .circuit_breakers
            .lock()
            .map(|guard| guard.reset(target))
            .unwrap_or(0);
        (recovered_services, breaker_reset_count)
    };
    let degraded_after = collect_degraded_services(server);

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "dry_run": dry_run,
            "target": target,
            "candidates": candidates,
            "candidate_count": candidates.len(),
            "recovered_services": recovered_services,
            "recovered_count": recovered_services.len(),
            "breaker_reset_count": breaker_reset_count,
            "remaining_degraded_count": degraded_after.len(),
            "remaining_degraded_services": degraded_after,
        }),
    )
    .await
}

pub(super) async fn handle_cache_clear(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    let memory_removed = server
        .memory_response_cache
        .lock()
        .map(|cache| cache.clear_all())
        .unwrap_or(0);
    let persistent_removed = if let Some(cache) = server.response_cache.clone() {
        cache_clear(server, cache).await?
    } else {
        0
    };

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "memory_removed": memory_removed,
            "sqlite_removed": persistent_removed,
            "total_removed": memory_removed + persistent_removed,
        }),
    )
    .await
}

pub(super) async fn handle_vector_clear(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    let (memory_removed, summary_removed) = if let Some(store) = server.vector_store.clone() {
        store.clear_all()?
    } else {
        (0, 0)
    };

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "vector_removed": memory_removed,
            "summary_removed": summary_removed,
        }),
    )
    .await
}

pub(super) async fn handle_maintenance_gc(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    let cycle = run_maintenance_cycle(server).await?;
    let maintenance = server
        .maintenance_tracker
        .lock()
        .map(|guard| guard.snapshot())
        .unwrap_or_default();

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "memory_expired_removed": cycle.memory_expired_removed,
            "sqlite_expired_removed": cycle.sqlite_expired_removed,
            "cache_vacuumed": cycle.cache_vacuumed,
            "vector_vacuumed": cycle.vector_vacuumed,
            "maintenance": maintenance,
        }),
    )
    .await
}
