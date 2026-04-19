use super::*;

pub(super) async fn handle_metrics(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    let status = server.get_status();
    send_result(
        server,
        request_id,
        json!({
            "metrics": status.metrics,
            "timestamp": status.timestamp,
        }),
    )
    .await
}

pub(super) async fn handle_metrics_get(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    let snapshot = serde_json::to_value(server.metrics.snapshot())?;
    let mut result = snapshot.clone();
    if let Value::Object(ref mut map) = result {
        map.insert("ok".to_string(), json!(true));
        map.insert("metrics".to_string(), snapshot);
    }
    send_result(server, request_id, result).await
}

pub(super) async fn handle_metrics_prometheus(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    let metrics = server.metrics.snapshot();
    let gauges = build_runtime_gauge_snapshot(server);
    let breaker_snapshot = server
        .circuit_breakers
        .lock()
        .map(|guard| {
            guard
                .snapshots()
                .into_iter()
                .map(|item| {
                    (
                        item.name,
                        PrometheusCircuitBreakerSnapshot {
                            state: item.state,
                            consecutive_failures: item.failure_count as u64,
                        },
                    )
                })
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let phase_limiter_snapshot = server
        .phase_rate_limiter
        .lock()
        .map(|guard| guard.snapshot())
        .unwrap_or_default();
    let inflight_snapshot = server
        .inflight_limiter
        .lock()
        .map(|guard| guard.snapshot())
        .unwrap_or_default();
    let lifecycle_snapshot = server
        .lifecycle_state
        .lock()
        .map(|guard| PrometheusLifecycleSnapshot {
            shutting_down: guard.shutdown_requested(),
        })
        .unwrap_or(PrometheusLifecycleSnapshot {
            shutting_down: false,
        });
    let maintenance_snapshot = server
        .maintenance_tracker
        .lock()
        .map(|guard| {
            let snapshot = guard.snapshot();
            PrometheusMaintenanceSnapshot {
                cycles_total: snapshot.cycles_total,
                running: snapshot.running,
            }
        })
        .unwrap_or(PrometheusMaintenanceSnapshot {
            cycles_total: 0,
            running: false,
        });
    let text = build_prometheus_metrics(
        &PrometheusMetricsSnapshot {
            chat_requests_total: metrics.chat_requests_total,
            cache_lookup_total: 0,
            cache_hit_total: 0,
            cache_store_total: 0,
            vector_search_total: metrics.vector_search_total,
            vector_hit_total: metrics.vector_hit_total,
            vector_store_total: metrics.vector_store_total,
            summary_read_total: metrics.summary_read_total,
            summary_hit_total: metrics.summary_hit_total,
            summary_store_total: metrics.summary_store_total,
            agent_failures_total: metrics.failed_requests,
            agent_timeout_failures_total: metrics.agent_timeout_failures_total,
            runtime_probe_timeout_total: metrics.runtime_probe_timeout_total,
            agent_panic_failures_total: 0,
            agent_other_failures_total: 0,
            review_gate_total: metrics.review_gate_total,
            review_gate_approved_total: metrics.review_gate_approved_total,
            review_gate_rejected_total: metrics.review_gate_rejected_total,
            review_gate_timeout_total: metrics.review_gate_timeout_total,
            review_gate_degraded_total: metrics.review_gate_degraded_total,
            review_gate_invalid_response_total: metrics.review_gate_invalid_response_total,
            lazy_blue5_doc_lookup_total: 0,
            lazy_blue5_doc_hit_total: 0,
            lazy_blue5_doc_reload_total: 0,
            lazy_app_config_lookup_total: 0,
            lazy_app_config_hit_total: 0,
            lazy_app_config_reload_total: 0,
            lazy_clarification_lookup_total: 0,
            lazy_clarification_hit_total: 0,
            lazy_clarification_reload_total: 0,
            chat_latency_count: metrics.chat_requests_total,
            chat_latency_sum_seconds: metrics.chat_latency_sum_ms / 1000.0,
            chat_latency_bucket_counts: metrics.chat_latency_bucket_counts,
            agent_latency_count: metrics.total_requests,
            agent_latency_sum_seconds: metrics.request_latency_sum_ms / 1000.0,
            agent_latency_bucket_counts: metrics.request_latency_bucket_counts,
            review_latency_count: metrics.review_gate_total,
            review_latency_sum_seconds: metrics.review_latency_sum_ms / 1000.0,
            review_latency_bucket_counts: metrics.review_latency_bucket_counts,
        },
        &gauges,
        &breaker_snapshot,
        &phase_limiter_snapshot,
        &inflight_snapshot,
        &lifecycle_snapshot,
        &maintenance_snapshot,
    );

    send_result(server, request_id, json!({ "text": text })).await
}

pub(super) async fn handle_metrics_reset(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    server.metrics.reset_all();
    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "reset": true,
            "timestamp": crate::acp::prelude::now_ts(),
        }),
    )
    .await
}

pub(super) async fn handle_debug_panel_get(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    send_result(server, request_id, build_debug_panel_payload(server).await).await
}

async fn build_debug_panel_payload(server: &AcpServer) -> Value {
    let state = server.conversation_state.lock().await;
    let conversation_count = state
        .checkpoints
        .iter()
        .map(|cp| cp.conversation_id.clone())
        .collect::<std::collections::HashSet<_>>()
        .len();
    let checkpoint_count = state.checkpoints.len();

    json!({
        "ok": true,
        "panel": {
            "trace": {"stage_transitions": []},
            "selected_agents": [],
            "review_outcomes": [],
            "runtime_health": {"ok": true},
            "review_gate": {
                "total": server.metrics.snapshot().review_gate_total,
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

fn build_trace_payload(params: &Value) -> Value {
    let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(100) as usize;

    let trace_events = trace_events()
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();
    let trace_events_len = trace_events.len();

    let limited_trace_events = if trace_events.len() > limit {
        trace_events[trace_events.len() - limit..].to_vec()
    } else {
        trace_events
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

pub(super) async fn handle_shutdown(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    info!("{}", t("info.shutdown_requested"));
    server.begin_shutdown();
    server.shutdown_notify.notify_waiters();

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "shutdown": "initiated"
        }),
    )
    .await
}

pub(super) async fn handle_health(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    let status = server.get_status();
    let metrics = server.metrics.snapshot();
    send_result(
        server,
        request_id,
        json!({
            "lifecycle": {
                "shutting_down": status.lifecycle.shutdown_requested,
                "is_healthy": status.lifecycle.is_healthy,
                "uptime_seconds": status.lifecycle.uptime_seconds,
            },
            "maintenance": status.maintenance,
            "review_gate": {
                "total": metrics.review_gate_total,
                "approved": metrics.review_gate_approved_total,
                "rejected": metrics.review_gate_rejected_total,
                "timeout": metrics.review_gate_timeout_total,
                "degraded": metrics.review_gate_degraded_total,
                "invalid_response": metrics.review_gate_invalid_response_total,
            },
            "timeouts": {
                "agent_request_total": metrics.agent_timeout_failures_total,
                "review_gate_total": metrics.review_gate_timeout_total,
                "runtime_probe_total": metrics.runtime_probe_timeout_total,
            },
            "timestamp": status.timestamp,
        }),
    )
    .await
}

fn check_status_label(value: CheckStatus) -> &'static str {
    match value {
        CheckStatus::Healthy => "healthy",
        CheckStatus::Warn => "warn",
        CheckStatus::Error => "error",
        CheckStatus::Skipped => "skipped",
    }
}

fn build_health_probes_payload(server: &AcpServer) -> Result<Value> {
    let status = server.get_status();
    let metrics = server.metrics.snapshot();

    let config_path = server.config_path.as_deref().map(Path::new);
    let report = build_runtime_healthcheck_report(
        config_path,
        server.response_cache.as_deref(),
        server.vector_store.as_deref(),
    )?;

    let healthy_count = report
        .components
        .iter()
        .filter(|item| item.status == CheckStatus::Healthy)
        .count();
    let warn_count = report
        .components
        .iter()
        .filter(|item| item.status == CheckStatus::Warn)
        .count();
    let error_count = report
        .components
        .iter()
        .filter(|item| item.status == CheckStatus::Error)
        .count();
    let skipped_count = report
        .components
        .iter()
        .filter(|item| item.status == CheckStatus::Skipped)
        .count();

    let readiness_status = if error_count > 0 {
        "not_ready"
    } else if warn_count > 0 {
        "degraded"
    } else {
        "ready"
    };

    let liveness_ok = status.lifecycle.is_healthy || status.lifecycle.shutdown_requested;
    let liveness_status = if liveness_ok { "alive" } else { "degraded" };

    let circuit_breakers = status
        .circuit_breakers
        .iter()
        .map(|item| {
            json!({
                "name": item.name,
                "state": item.state,
                "failure_count": item.failure_count,
                "success_count": item.success_count,
                "last_state_change": item.last_state_change,
                "total_failures": item.total_failures,
                "total_successes": item.total_successes,
            })
        })
        .collect::<Vec<_>>();

    let rate_limiter_buckets = with_acp_lock(
        server.lock_monitor.as_ref(),
        ACP_LOCK_PHASE_RATE_LIMITER,
        server.phase_rate_limiter.as_ref(),
        |guard| {
            guard
                .snapshot()
                .into_iter()
                .map(|(phase, (tokens, capacity))| {
                    json!({
                        "phase": phase,
                        "tokens": tokens,
                        "capacity": capacity,
                        "used_percent": if capacity > 0.0 { ((capacity - tokens) / capacity * 100.0).clamp(0.0, 100.0) } else { 0.0 },
                    })
                })
                .collect::<Vec<_>>()
        },
    );

    let lock_components = server.lock_monitor.snapshot();
    let lock_summary = summarize_lock_health(&lock_components);
    let timeout_status = if metrics.agent_timeout_failures_total > 0
        || metrics.review_gate_timeout_total > 0
        || metrics.runtime_probe_timeout_total > 0
    {
        "warn"
    } else {
        "healthy"
    };

    let mut dependencies = report
        .components
        .iter()
        .map(|item| {
            json!({
                "name": item.name,
                "status": check_status_label(item.status),
                "message": item.message,
                "details": item.details,
            })
        })
        .collect::<Vec<_>>();
    dependencies.push(json!({
        "name": "locks",
        "status": lock_summary.status,
        "message": format!(
            "poisoned={}, recovered={}, slow_waits={}",
            lock_summary.poisoned_total, lock_summary.recovered_total, lock_summary.slow_wait_total
        ),
        "details": {
            "poisoned_total": lock_summary.poisoned_total,
            "recovered_total": lock_summary.recovered_total,
            "slow_wait_total": lock_summary.slow_wait_total,
            "max_wait_ms": lock_summary.max_wait_ms,
            "components_tracked": lock_summary.components_tracked,
        }
    }));
    dependencies.push(json!({
        "name": "timeouts",
        "status": timeout_status,
        "message": format!(
            "agent={}, review_gate={}, runtime_probe={}",
            metrics.agent_timeout_failures_total,
            metrics.review_gate_timeout_total,
            metrics.runtime_probe_timeout_total,
        ),
        "details": {
            "agent_request_total": metrics.agent_timeout_failures_total,
            "review_gate_total": metrics.review_gate_timeout_total,
            "runtime_probe_total": metrics.runtime_probe_timeout_total,
        }
    }));

    Ok(json!({
        "ok": true,
        "probes": {
            "liveness": {
                "status": liveness_status,
                "ok": liveness_ok,
                "shutting_down": status.lifecycle.shutdown_requested,
                "uptime_seconds": status.lifecycle.uptime_seconds,
            },
            "readiness": {
                "status": readiness_status,
                "ok": error_count == 0,
                "overall_status": check_status_label(report.overall_status),
                "generated_at": report.generated_at,
            },
            "summary": {
                "healthy": healthy_count,
                "warn": warn_count,
                "error": error_count,
                "skipped": skipped_count,
            },
            "dependencies": dependencies,
            "circuit_breakers": circuit_breakers,
            "rate_limiter": {
                "tracked": rate_limiter_buckets.len(),
                "buckets": rate_limiter_buckets,
            },
            "locks": {
                "status": lock_summary.status,
                "poisoned_total": lock_summary.poisoned_total,
                "recovered_total": lock_summary.recovered_total,
                "slow_wait_total": lock_summary.slow_wait_total,
                "max_wait_ms": lock_summary.max_wait_ms,
                "components_tracked": lock_summary.components_tracked,
                "components": lock_components,
            },
            "timeouts": {
                "status": timeout_status,
                "agent_request_total": metrics.agent_timeout_failures_total,
                "review_gate_total": metrics.review_gate_timeout_total,
                "runtime_probe_total": metrics.runtime_probe_timeout_total,
            },
            "timestamp": status.timestamp,
        }
    }))
}

pub(super) async fn handle_health_probes(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    send_result(server, request_id, build_health_probes_payload(server)?).await
}

fn build_runtime_stability_payload(server: &AcpServer) -> Result<Value> {
    let status = server.get_status();
    let _metrics = server.metrics.snapshot();
    let config_path = server.config_path.as_deref().map(Path::new);
    let report = build_runtime_healthcheck_report(
        config_path,
        server.response_cache.as_deref(),
        server.vector_store.as_deref(),
    )?;

    let mut config_warnings = Vec::new();
    let mut strict_violations = Vec::new();

    if let Some(cfg_path) = config_path {
        if let Ok(cfg) = AppConfig::load(cfg_path) {
            config_warnings = collect_config_warnings(cfg_path, &cfg);
            strict_violations = collect_production_strict_violations(&cfg);
        }
    }

    let error_count = report
        .components
        .iter()
        .filter(|item| item.status == CheckStatus::Error)
        .count();
    let warn_count = report
        .components
        .iter()
        .filter(|item| item.status == CheckStatus::Warn)
        .count();

    let graceful_shutdown_ready = !status.lifecycle.shutdown_requested;
    let uptime_seconds = status.lifecycle.uptime_seconds;

    let config_valid = if let Some(cfg_path) = config_path {
        AppConfig::load(cfg_path).is_ok()
    } else {
        true
    };

    let mut stability_score = 100;
    if error_count > 0 {
        stability_score -= (error_count as i32).min(30);
    }
    if warn_count > 0 {
        stability_score -= ((warn_count as i32) / 2).min(20);
    }
    if !graceful_shutdown_ready {
        stability_score -= 15;
    }
    if !config_valid {
        stability_score -= 25;
    }
    if !strict_violations.is_empty() {
        stability_score -= (strict_violations.len() as i32 * 5).min(30);
    }
    stability_score = stability_score.clamp(0, 100);

    let stability_level = match stability_score {
        90..=100 => "excellent",
        75..=89 => "good",
        60..=74 => "fair",
        40..=59 => "poor",
        _ => "critical",
    };

    let safe_restart_ready =
        graceful_shutdown_ready && config_valid && strict_violations.is_empty();

    let mut checks = vec![
        json!({
            "name": "health_check",
            "status": if error_count == 0 { "pass" } else { "fail" },
            "errors": error_count,
            "warnings": warn_count,
            "description": format!("Health check: {} errors, {} warnings", error_count, warn_count),
        }),
        json!({
            "name": "graceful_shutdown",
            "status": if graceful_shutdown_ready { "pass" } else { "fail" },
            "uptime_seconds": uptime_seconds,
            "shutdown_requested": status.lifecycle.shutdown_requested,
            "description": if graceful_shutdown_ready {
                "Graceful shutdown capability ready".to_string()
            } else {
                "Graceful shutdown in progress or unavailable".to_string()
            },
        }),
        json!({
            "name": "config_validation",
            "status": if config_valid { "pass" } else { "fail" },
            "warning_count": config_warnings.len(),
            "description": format!("Config validation: {} warnings", config_warnings.len()),
        }),
    ];

    if !strict_violations.is_empty() {
        checks.push(json!({
            "name": "production_strict_mode",
            "status": "fail",
            "violation_count": strict_violations.len(),
            "violations": strict_violations.iter().take(5).map(|v| {
                json!({
                    "code": "strict_violation",
                    "message": v,
                })
            }).collect::<Vec<_>>(),
            "description": format!("Production strict mode: {} violations", strict_violations.len()),
        }));
    } else {
        checks.push(json!({
            "name": "production_strict_mode",
            "status": "pass",
            "violation_count": 0,
            "description": "No production strict mode violations".to_string(),
        }));
    }

    Ok(json!({
        "ok": true,
        "stability": {
            "score": stability_score,
            "level": stability_level,
            "safe_restart_ready": safe_restart_ready,
            "summary": {
                "health_errors": error_count,
                "health_warnings": warn_count,
                "uptime_seconds": uptime_seconds,
                "config_warnings": config_warnings.len(),
                "strict_violations": strict_violations.len(),
            },
            "checks": checks,
            "recommendation": if stability_score >= 75 {
                "System is stable. Safe to operate.".to_string()
            } else if stability_score >= 60 {
                "System has degraded capability. Review warnings before critical operations.".to_string()
            } else {
                "System is unstable. Address errors before restart or upgrades.".to_string()
            },
            "timestamp": status.timestamp,
        }
    }))
}

pub(super) async fn handle_runtime_stability(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    send_result(server, request_id, build_runtime_stability_payload(server)?).await
}

pub(super) async fn handle_runtime_self_model(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let payload = build_runtime_self_model_payload(server, &params)?;
    send_result(server, request_id, payload).await
}

fn build_runtime_self_model_payload(server: &AcpServer, params: &Value) -> Result<Value> {
    let probes_payload = build_health_probes_payload(server)?;
    let stability_payload = build_runtime_stability_payload(server)?;
    let offline_eval_payload = build_rl_alignment_offline_eval_payload(params);

    let probes = probes_payload
        .get("probes")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let stability = stability_payload
        .get("stability")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let offline_eval = offline_eval_payload
        .get("offline_eval")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let readiness_status = probes
        .get("readiness")
        .and_then(|value| value.get("status"))
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let safe_restart_ready = stability
        .get("safe_restart_ready")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let stability_level = stability
        .get("level")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let recommended_mode = offline_eval
        .get("decision")
        .and_then(|value| value.get("recommended_mode"))
        .and_then(Value::as_str)
        .unwrap_or("conservative");
    let fallback_triggered = offline_eval
        .get("decision")
        .and_then(|value| value.get("fallback_triggered"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let drift_alert = offline_eval
        .get("drift")
        .and_then(|value| value.get("alert"))
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let summary = stability
        .get("summary")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let warnings = offline_eval
        .get("warnings")
        .cloned()
        .unwrap_or_else(|| json!([]));

    let mut recommendations = Vec::new();
    if readiness_status != "ready" {
        recommendations.push(
            "Review runtime dependencies, probes, and breaker state before serving critical traffic."
                .to_string(),
        );
    }
    if !safe_restart_ready {
        recommendations.push(
            "Avoid restart or rollout until config validation and strict-mode constraints are green."
                .to_string(),
        );
    }
    if drift_alert || fallback_triggered {
        recommendations.push(
            "Keep runtime in conservative mode until reward drift and safety regressions recover."
                .to_string(),
        );
    }
    if recommendations.is_empty() {
        recommendations.push(
            "System is operating within the expected envelope; continue normal runtime supervision."
                .to_string(),
        );
    }

    let timestamp = probes
        .get("timestamp")
        .cloned()
        .or_else(|| stability.get("timestamp").cloned())
        .unwrap_or_else(|| json!(0));

    Ok(json!({
        "ok": true,
        "self_model": {
            "health": probes,
            "stability": stability,
            "drift": offline_eval.get("drift").cloned().unwrap_or_else(|| json!({})),
            "decision": {
                "recommended_mode": recommended_mode,
                "fallback_triggered": fallback_triggered,
                "readiness_status": readiness_status,
                "stability_level": stability_level,
                "safe_restart_ready": safe_restart_ready,
            },
            "constraints": {
                "shutdown_requested": probes
                    .get("liveness")
                    .and_then(|value| value.get("shutting_down"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                "health_errors": summary.get("health_errors").cloned().unwrap_or_else(|| json!(0)),
                "health_warnings": summary.get("health_warnings").cloned().unwrap_or_else(|| json!(0)),
                "config_warnings": summary.get("config_warnings").cloned().unwrap_or_else(|| json!(0)),
                "strict_violations": summary.get("strict_violations").cloned().unwrap_or_else(|| json!(0)),
            },
            "warnings": warnings,
            "recommendations": recommendations,
            "source_methods": ["health.probes", "runtime.stability", "rl.alignment.offline_eval"],
            "timestamp": timestamp,
        }
    }))
}

fn build_provider_status_payload(server: &AcpServer) -> Result<Value> {
    let status = server.get_status();
    let config_path = server.config_path.as_deref().map(Path::new);
    let report = build_runtime_healthcheck_report(
        config_path,
        server.response_cache.as_deref(),
        server.vector_store.as_deref(),
    )?;

    let provider_component = report
        .components
        .iter()
        .find(|item| item.name == "provider_dependencies");

    let provider_status = provider_component
        .map(|item| check_status_label(item.status))
        .unwrap_or("skipped");
    let provider_message = provider_component
        .map(|item| item.message.clone())
        .unwrap_or_else(|| "provider dependency snapshot unavailable".to_string());
    let provider_details = provider_component
        .map(|item| item.details.clone())
        .unwrap_or_else(|| json!({}));

    let ready = provider_details
        .get("ready")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let degraded = provider_details
        .get("degraded")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total = provider_details
        .get("total")
        .and_then(Value::as_u64)
        .unwrap_or(0);

    let configured_agents = provider_details
        .get("agents")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let registry_catalog = server
        .agent_registry()
        .map(|registry| {
            registry
                .models()
                .into_iter()
                .map(|(name, default_model, available_models)| {
                    json!({
                        "agent": name,
                        "default_model": default_model.as_ref().map(|item| item.id.clone()),
                        "available_models": available_models.len(),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let configured_total = configured_agents.len() as u64;
    let catalog_total = registry_catalog.len() as u64;

    Ok(json!({
        "ok": true,
        "provider_status": {
            "status": provider_status,
            "message": provider_message,
            "summary": {
                "ready": ready,
                "degraded": degraded,
                "configured": total.max(configured_total),
                "registry": catalog_total,
                "coverage_percent": if total > 0 {
                    ((ready as f64 / total as f64) * 100.0).round()
                } else {
                    0.0
                },
            },
            "configured_agents": configured_agents,
            "registry_catalog": registry_catalog,
            "timestamp": status.timestamp,
        }
    }))
}

pub(super) async fn handle_provider_status(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    send_result(server, request_id, build_provider_status_payload(server)?).await
}

pub(super) async fn handle_governance_status(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let status = server.get_status();
    let runtime_snapshot = server.metrics.snapshot();

    let pua_plan = server
        .pua_enforcement_plan
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();

    let pua_learning = pua_feedback_collector()
        .extract_learning_data(200)
        .unwrap_or_default();
    let recent_failed = pua_learning.iter().filter(|record| !record.passed).count();
    let governance_audit = load_governance_audit_events(20).unwrap_or_default();

    let rules = governance_rule_fingerprint(server.config_path.as_deref());
    let config_summary = config_pack::governance_config_summary(server.config_path.as_deref());

    let entry_rate_snapshot = with_acp_lock(
        server.lock_monitor.as_ref(),
        ACP_LOCK_PHASE_RATE_LIMITER,
        server.phase_rate_limiter.as_ref(),
        |guard| guard.snapshot(),
    );
    let entry_sources_tracked = entry_rate_snapshot
        .keys()
        .filter(|name| name.starts_with("entry:"))
        .count();

    let breaker_open_count = status
        .circuit_breakers
        .iter()
        .filter(|item| item.state.eq_ignore_ascii_case("open"))
        .count();

    let tool_registry = ToolRegistry::new();
    let tool_matrix = tool_registry.capability_matrix();
    let (tool_total, high_risk_total, fallback_enabled_total) = tool_matrix
        .get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            let high_risk = tools
                .iter()
                .filter(|item| {
                    item.get("risk_level")
                        .and_then(Value::as_str)
                        .map(|value| value.eq_ignore_ascii_case("high"))
                        .unwrap_or(false)
                })
                .count();
            let fallback_enabled = tools
                .iter()
                .filter(|item| {
                    item.get("fallback_chain")
                        .and_then(Value::as_array)
                        .map(|chain| !chain.is_empty())
                        .unwrap_or(false)
                })
                .count();
            (tools.len(), high_risk, fallback_enabled)
        })
        .unwrap_or((0, 0, 0));

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "governance": {
                "status": if status.lifecycle.is_healthy && recent_failed == 0 && breaker_open_count == 0 {
                    "healthy"
                } else {
                    "degraded"
                },
                "runtime": {
                    "is_healthy": status.lifecycle.is_healthy,
                    "shutting_down": status.lifecycle.shutdown_requested,
                    "uptime_seconds": status.lifecycle.uptime_seconds,
                },
                "rules": rules,
                "pua": {
                    "escalation_level": pua_plan.escalation_level,
                    "red_line_count": pua_plan.red_lines.len(),
                    "stage_requirement_count": pua_plan.stage_requirements.len(),
                    "mandatory_safeguards_count": pua_plan.mandatory_safeguards.len(),
                    "mandatory_evidence_count": pua_plan.mandatory_evidence.len(),
                },
                "violations": {
                    "pua_recent_total": pua_learning.len(),
                    "pua_recent_failed": recent_failed,
                    "review_gate_rejected_total": runtime_snapshot.review_gate_rejected_total,
                    "breaker_open_count": breaker_open_count,
                },
                "dynamic_rules": {
                    "runtime_mutable": true,
                    "red_line_count": pua_plan.red_lines.len(),
                    "stage_requirement_count": pua_plan.stage_requirements.len(),
                    "quality_compass_count": pua_plan.quality_compass.len(),
                },
                "tool_matrix": {
                    "summary": {
                        "tool_total": tool_total,
                        "high_risk_total": high_risk_total,
                        "fallback_enabled_total": fallback_enabled_total,
                    },
                    "capabilities": tool_matrix,
                },
                "audit": {
                    "recent_total": governance_audit.len(),
                    "recent": governance_audit,
                },
                "config": config_summary,
                "entry_guard": {
                    "auth_enabled": server.runtime_config.entry_auth_enabled,
                    "auth_key_env": server.runtime_config.entry_auth_api_key_env,
                    "auth_key_configured": std::env::var(&server.runtime_config.entry_auth_api_key_env)
                        .ok()
                        .map(|value| !value.trim().is_empty())
                        .unwrap_or(false),
                    "rate_limit_rpm": server.runtime_config.entry_rate_limit_rpm,
                    "rate_limit_burst": server.runtime_config.entry_rate_limit_burst,
                    "sources_tracked": entry_sources_tracked,
                },
                "timestamp": status.timestamp,
            }
        }),
    )
    .await
}

pub(super) async fn handle_optimization_peak(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let status = server.get_status();
    let runtime_snapshot = server.metrics.snapshot();
    let config_summary = config_pack::governance_config_summary(server.config_path.as_deref());
    let repro_summary = repro_pack::reproducible_build_summary(server.config_path.as_deref());
    let pua_learning = pua_feedback_collector()
        .extract_learning_data(200)
        .unwrap_or_default();

    let total_requests = runtime_snapshot.total_requests.max(1) as f64;
    let failed_ratio = runtime_snapshot.failed_requests as f64 / total_requests;
    let review_reject_ratio = runtime_snapshot.review_gate_rejected_total as f64 / total_requests;
    let timeout_total = runtime_snapshot.agent_timeout_failures_total
        + runtime_snapshot.review_gate_timeout_total
        + runtime_snapshot.runtime_probe_timeout_total;
    let breaker_open_count = status
        .circuit_breakers
        .iter()
        .filter(|item| item.state.eq_ignore_ascii_case("open"))
        .count() as u64;

    let strict_enabled = config_summary
        .get("production_strict")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let strict_violation_count = config_summary
        .get("strict_violation_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let entry_auth_enabled = config_summary
        .get("entry_auth_enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let entry_auth_key_configured = config_summary
        .get("entry_auth_key_configured")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let required_total = repro_summary
        .get("reproducibility")
        .and_then(|value| value.get("required_total"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let required_present = repro_summary
        .get("reproducibility")
        .and_then(|value| value.get("required_present"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let missing_required = repro_summary
        .get("reproducibility")
        .and_then(|value| value.get("missing_required"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let recent_failed = pua_learning.iter().filter(|record| !record.passed).count() as u64;

    let task = params
        .get("task")
        .and_then(Value::as_str)
        .or_else(|| params.get("objective").and_then(Value::as_str))
        .unwrap_or("One-shot optimization peak validation");
    let hardness = summarize_hardness(task, &params);
    let cost = summarize_token_cost_governance(task, &params, hardness.clone(), &runtime_snapshot);
    let estimated_total_cost = cost.telemetry.estimated_total_cost;
    let budget_class = cost.budget.budget_class.clone();

    let max_failure_ratio = params
        .get("max_failure_ratio")
        .and_then(Value::as_f64)
        .unwrap_or(0.10);
    let max_review_reject_ratio = params
        .get("max_review_reject_ratio")
        .and_then(Value::as_f64)
        .unwrap_or(0.10);
    let max_timeout_total = params
        .get("max_timeout_total")
        .and_then(Value::as_u64)
        .unwrap_or(10);
    let max_estimated_cost = params
        .get("max_estimated_cost")
        .and_then(Value::as_f64)
        .unwrap_or(1.50);

    let quality_pass =
        failed_ratio <= max_failure_ratio && review_reject_ratio <= max_review_reject_ratio;
    let cost_pass = estimated_total_cost <= max_estimated_cost;
    let stability_pass = status.lifecycle.is_healthy
        && breaker_open_count == 0
        && timeout_total <= max_timeout_total;
    let security_pass = strict_enabled
        && strict_violation_count == 0
        && entry_auth_enabled
        && entry_auth_key_configured;
    let repro_pass = required_total == required_present && missing_required.is_empty();
    let governance_pass = recent_failed == 0;

    let gates = vec![
        json!({
            "name": "quality",
            "passed": quality_pass,
            "failure_ratio": failed_ratio,
            "max_failure_ratio": max_failure_ratio,
            "review_reject_ratio": review_reject_ratio,
            "max_review_reject_ratio": max_review_reject_ratio,
        }),
        json!({
            "name": "cost",
            "passed": cost_pass,
            "estimated_total_cost": estimated_total_cost,
            "max_estimated_cost": max_estimated_cost,
            "budget_class": budget_class,
        }),
        json!({
            "name": "stability",
            "passed": stability_pass,
            "runtime_healthy": status.lifecycle.is_healthy,
            "breaker_open_count": breaker_open_count,
            "timeout_total": timeout_total,
            "max_timeout_total": max_timeout_total,
        }),
        json!({
            "name": "security",
            "passed": security_pass,
            "production_strict": strict_enabled,
            "strict_violation_count": strict_violation_count,
            "entry_auth_enabled": entry_auth_enabled,
            "entry_auth_key_configured": entry_auth_key_configured,
        }),
        json!({
            "name": "reproducibility",
            "passed": repro_pass,
            "required_total": required_total,
            "required_present": required_present,
            "missing_required": missing_required,
        }),
        json!({
            "name": "governance",
            "passed": governance_pass,
            "pua_recent_total": pua_learning.len(),
            "pua_recent_failed": recent_failed,
        }),
    ];

    let overall_pass = gates
        .iter()
        .all(|gate| gate.get("passed").and_then(Value::as_bool) == Some(true));

    let success_requests = runtime_snapshot
        .total_requests
        .saturating_sub(runtime_snapshot.failed_requests);
    let task_success_rate = if runtime_snapshot.total_requests > 0 {
        success_requests as f64 / runtime_snapshot.total_requests as f64
    } else {
        1.0
    };
    let first_pass_rate = if runtime_snapshot.review_gate_total > 0 {
        runtime_snapshot.review_gate_approved_total as f64 / runtime_snapshot.review_gate_total as f64
    } else {
        1.0
    };
    let mean_repair_iterations = if runtime_snapshot.review_gate_total > 0 {
        (runtime_snapshot.review_gate_rejected_total as f64 / runtime_snapshot.review_gate_total as f64)
            .clamp(0.0, 1.0)
            * 2.0
    } else {
        0.0
    };
    let human_intervention_rate = if runtime_snapshot.total_requests > 0 {
        runtime_snapshot.review_gate_rejected_total as f64 / runtime_snapshot.total_requests as f64
    } else {
        0.0
    };
    let regression_rate = if runtime_snapshot.total_requests > 0 {
        runtime_snapshot.failed_requests as f64 / runtime_snapshot.total_requests as f64
    } else {
        0.0
    };

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "peak": {
                "version": "x11-one-shot-optimization-peak-v1",
                "overall_pass": overall_pass,
                "status": if overall_pass { "peak_ready" } else { "needs_action" },
                "frozen_scope": ["X1", "X2", "X3", "X4", "X5", "X6", "X7", "X8", "X9", "X10"],
                "window": {
                    "sprint": params
                        .get("sprint")
                        .and_then(Value::as_str)
                        .unwrap_or("blue15-x11"),
                    "freeze_mode": params
                        .get("freeze_mode")
                        .and_then(Value::as_str)
                        .unwrap_or("strict"),
                },
                "task": task,
                "hardness": hardness,
                "cost": cost,
                "gates": gates,
                "summary": {
                    "total_requests": runtime_snapshot.total_requests,
                    "failed_requests": runtime_snapshot.failed_requests,
                    "review_gate_rejected_total": runtime_snapshot.review_gate_rejected_total,
                    "agent_timeout_failures_total": runtime_snapshot.agent_timeout_failures_total,
                    "review_gate_timeout_total": runtime_snapshot.review_gate_timeout_total,
                    "runtime_probe_timeout_total": runtime_snapshot.runtime_probe_timeout_total,
                    "uptime_seconds": status.lifecycle.uptime_seconds,
                },
                "indicators": {
                    "task_success_rate": task_success_rate,
                    "first_pass_rate": first_pass_rate,
                    "mean_repair_iterations": mean_repair_iterations,
                    "human_intervention_rate": human_intervention_rate,
                    "regression_rate": regression_rate,
                },
                "timestamp": status.timestamp,
            }
        }),
    )
    .await
}

const GOVERNANCE_AUDIT_DIR: &str = ".goon/governance";
const GOVERNANCE_AUDIT_FILE: &str = "audit.ndjson";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GovernanceAuditEvent {
    timestamp: u64,
    action: String,
    actor: String,
    result: String,
    detail: Value,
}

fn append_governance_audit_event(event: &GovernanceAuditEvent) -> Result<()> {
    let dir = Path::new(GOVERNANCE_AUDIT_DIR);
    fs::create_dir_all(dir)?;
    let path = dir.join(GOVERNANCE_AUDIT_FILE);
    let mut file = fs::OpenOptions::new().create(true).append(true).open(path)?;
    let line = serde_json::to_string(event)?;
    writeln!(file, "{}", line)?;
    Ok(())
}

fn load_governance_audit_events(limit: usize) -> Result<Vec<GovernanceAuditEvent>> {
    if limit == 0 {
        return Ok(Vec::new());
    }

    let path = Path::new(GOVERNANCE_AUDIT_DIR).join(GOVERNANCE_AUDIT_FILE);
    if !path.exists() {
        return Ok(Vec::new());
    }

    let content = fs::read_to_string(path)?;
    let mut events = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let event: GovernanceAuditEvent = serde_json::from_str(trimmed)?;
        events.push(event);
    }

    if events.len() > limit {
        Ok(events.split_off(events.len() - limit))
    } else {
        Ok(events)
    }
}

pub(super) async fn handle_governance_plan_get(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    let plan = server
        .pua_enforcement_plan
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();

    send_result(server, request_id, json!({ "ok": true, "plan": plan })).await
}

pub(super) async fn handle_governance_plan_update(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let mut plan = server
        .pua_enforcement_plan
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();

    if let Some(level) = params.get("escalation_level").and_then(Value::as_str) {
        plan.escalation_level = level.to_string();
    }
    if let Some(items) = params.get("red_lines").and_then(Value::as_array) {
        plan.red_lines = items
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect();
    }
    if let Some(items) = params.get("quality_compass").and_then(Value::as_array) {
        plan.quality_compass = items
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect();
    }
    if let Some(items) = params.get("mandatory_safeguards").and_then(Value::as_array) {
        plan.mandatory_safeguards = items
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect();
    }
    if let Some(items) = params.get("mandatory_evidence").and_then(Value::as_array) {
        plan.mandatory_evidence = items
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect();
    }
    if let Some(stage_requirements) = params.get("stage_requirements") {
        plan.stage_requirements =
            serde_json::from_value::<Vec<PuaStageRequirement>>(stage_requirements.clone())?;
    }

    if let Ok(mut guard) = server.pua_enforcement_plan.lock() {
        *guard = plan.clone();
    }

    let event = GovernanceAuditEvent {
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
    let _ = append_governance_audit_event(&event);

    send_result(server, request_id, json!({ "ok": true, "plan": plan })).await
}

pub(super) async fn handle_governance_audit_recent(
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
    let events = load_governance_audit_events(limit).unwrap_or_default();

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

#[derive(Debug, Clone, Copy, PartialEq)]
struct LockHealthSummary {
    status: &'static str,
    poisoned_total: u64,
    recovered_total: u64,
    slow_wait_total: u64,
    max_wait_ms: f64,
    components_tracked: usize,
}

fn summarize_lock_health(components: &[AcpLockSnapshot]) -> LockHealthSummary {
    let poisoned_total = components.iter().map(|item| item.poisoned_total).sum::<u64>();
    let recovered_total = components.iter().map(|item| item.recovered_total).sum::<u64>();
    let slow_wait_total = components.iter().map(|item| item.slow_wait_total).sum::<u64>();
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

pub(super) async fn handle_action_check(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let kind = params
        .get("kind")
        .and_then(Value::as_str)
        .and_then(ActionCheckKind::parse)
        .unwrap_or(ActionCheckKind::All);
    let report = run_action_check(&clone_artifact_ledger(server), kind)?;
    send_result(server, request_id, json!({"ok": report.ok, "report": report})).await
}

pub(super) async fn handle_conversation_checkpoint_create(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let Some(conversation_id) = params.get("conversation_id").and_then(Value::as_str) else {
        return send_error(
            server,
            request_id,
            -32602,
            "conversation_id is required".to_string(),
            None,
        )
        .await;
    };

    if conversation_id.trim().is_empty() {
        return send_error(
            server,
            request_id,
            -32602,
            "conversation_id is required".to_string(),
            None,
        )
        .await;
    }
    let branch_id = params
        .get("branch_id")
        .or_else(|| params.get("branch"))
        .and_then(Value::as_str)
        .unwrap_or("main");
    if branch_id.trim().is_empty() || branch_id.chars().any(char::is_whitespace) {
        return send_error(
            server,
            request_id,
            -32602,
            "branch_id is invalid".to_string(),
            None,
        )
        .await;
    }
    let messages = match parse_messages(&params) {
        Some(messages) if !messages.is_empty() => messages,
        _ => {
            return send_error(
                server,
                request_id,
                -32602,
                "messages are required".to_string(),
                None,
            )
            .await;
        }
    };

    let note = params.get("note").and_then(Value::as_str).map(str::to_string);
    let checkpoint =
        create_checkpoint_record(server, conversation_id, branch_id, messages, note, None).await;

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "checkpoint": checkpoint,
        }),
    )
    .await
}

pub(super) async fn handle_conversation_checkpoint_list(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let Some(conversation_id) = params.get("conversation_id").and_then(Value::as_str) else {
        return send_error(
            server,
            request_id,
            -32602,
            "conversation_id is required".to_string(),
            None,
        )
        .await;
    };
    let branch_id = params
        .get("branch_id")
        .or_else(|| params.get("branch"))
        .and_then(Value::as_str);
    let limit = params.get("limit").and_then(Value::as_u64).map(|v| v as usize);
    let checkpoints = list_checkpoint_records(server, conversation_id, branch_id, limit).await;

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "conversation_id": conversation_id,
            "count": checkpoints.len(),
            "checkpoints": checkpoints,
        }),
    )
    .await
}

pub(super) async fn handle_conversation_rollback(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let Some(conversation_id) = params.get("conversation_id").and_then(Value::as_str) else {
        return send_error(
            server,
            request_id,
            -32602,
            "conversation_id is required".to_string(),
            None,
        )
        .await;
    };
    let Some(checkpoint_id) = params.get("checkpoint_id").and_then(Value::as_str) else {
        return send_error(
            server,
            request_id,
            -32602,
            "checkpoint_id is required".to_string(),
            None,
        )
        .await;
    };

    let branch_id = params
        .get("branch_id")
        .or_else(|| params.get("branch"))
        .and_then(Value::as_str)
        .unwrap_or("main");
    let checkpoint = match find_checkpoint(server, conversation_id, checkpoint_id).await {
        Some(checkpoint) => checkpoint,
        None => {
            return send_error(
                server,
                request_id,
                -32004,
                format!("checkpoint not found: {}", checkpoint_id),
                None,
            )
            .await;
        }
    };
    let previous_head = get_branch_head_id(server, conversation_id, branch_id).await;
    let rollback = create_checkpoint_record(
        server,
        conversation_id,
        branch_id,
        checkpoint.messages.clone(),
        Some(format!("rollback:{}", checkpoint_id)),
        Some(checkpoint_id.to_string()),
    )
    .await;

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "conversation_id": conversation_id,
            "branch_id": branch_id,
            "checkpoint": rollback,
            "previous_head": previous_head,
            "current_head": rollback.checkpoint_id,
        }),
    )
    .await
}

pub(super) async fn handle_conversation_checkpoint_prune(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let Some(conversation_id) = params.get("conversation_id").and_then(Value::as_str) else {
        return send_error(
            server,
            request_id,
            -32602,
            "conversation_id is required".to_string(),
            None,
        )
        .await;
    };
    let keep = params.get("keep").and_then(Value::as_u64).unwrap_or(1) as usize;
    if keep == 0 {
        return send_error(
            server,
            request_id,
            -32602,
            "keep must be >= 1".to_string(),
            None,
        )
        .await;
    }
    let branch_id = params
        .get("branch_id")
        .or_else(|| params.get("branch"))
        .and_then(Value::as_str)
        .unwrap_or("main");
    let (removed, repaired_heads, dropped_heads) =
        prune_checkpoints(server, conversation_id, branch_id, keep).await;

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "removed": removed,
            "repaired_heads": repaired_heads,
            "dropped_heads": dropped_heads,
        }),
    )
    .await
}

pub(super) async fn handle_autotune_status(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    let autotune_state = if let Some(autotune) = server.autotune.as_ref() {
        let lock = autotune.lock().await;
        Some(lock.clone())
    } else {
        None
    };

    let autotune_config = server.autotune_config.as_ref().cloned();
    let enabled = autotune_config.as_ref().map(|cfg| cfg.enabled).unwrap_or(false);

    send_result(
        server,
        request_id,
        json!({
            "enabled": enabled,
            "state": autotune_state,
            "autotune": {
                "enabled": enabled,
                "state": autotune_state,
            },
        }),
    )
    .await
}

pub(super) async fn handle_autotune_get(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    let Some(autotune) = server.autotune.as_ref() else {
        return send_result(
            server,
            request_id,
            json!({
                "enabled": false,
                "autotune": null,
                "params": null,
            }),
        )
        .await;
    };

    let state = autotune.lock().await;
    let snap = state.snapshot();
    let mut result = snap.clone();
    if let Value::Object(ref mut map) = result {
        map.insert("enabled".to_string(), json!(true));
        map.insert("autotune".to_string(), snap.clone());
        map.insert("params".to_string(), snap);
    }
    send_result(server, request_id, result).await
}

pub(super) async fn handle_selector_status(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    let snapshot = server
        .adaptive_model_selector
        .lock()
        .map(|selector| selector.snapshot())
        .unwrap_or_default();

    send_result(server, request_id, json!({ "selector": snapshot })).await
}

pub(super) async fn handle_hardness_status(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let task = params
        .get("task")
        .and_then(Value::as_str)
        .or_else(|| params.get("objective").and_then(Value::as_str))
        .unwrap_or("");
    let hardness = summarize_hardness(task, &params);

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "hardness": hardness,
            "routing": {
                "mode": hardness.budget.recommended_mode,
                "parallelism_cap": hardness.budget.parallelism_cap,
                "timeout_seconds": hardness.budget.timeout_seconds,
                "required_reviews": hardness.budget.required_reviews,
            },
        }),
    )
    .await
}

pub(super) async fn handle_error_contract(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    send_result(
        server,
        request_id,
        json!({
            "contract": {
                "version": "x8-error-contract-v1",
                "kinds": [
                    {
                        "kind": "InvalidParams",
                        "codes": [-32602],
                        "retry": {"retryable": false, "strategy": "none", "max_retries": 0}
                    },
                    {
                        "kind": "MethodNotFound",
                        "codes": [-32601],
                        "retry": {"retryable": false, "strategy": "none", "max_retries": 0}
                    },
                    {
                        "kind": "AuthRequired",
                        "codes": [-32003],
                        "retry": {"retryable": false, "strategy": "none", "max_retries": 0}
                    },
                    {
                        "kind": "RateLimited",
                        "codes": [-32029],
                        "retry": {"retryable": true, "strategy": "exponential_backoff", "base_delay_ms": 500, "max_delay_ms": 10000, "max_retries": 3}
                    },
                    {
                        "kind": "UpstreamTimeout",
                        "codes": [-32603],
                        "retry": {"retryable": true, "strategy": "exponential_backoff", "base_delay_ms": 500, "max_delay_ms": 10000, "max_retries": 3}
                    },
                    {
                        "kind": "PuaViolation",
                        "codes": [-32603],
                        "retry": {"retryable": false, "strategy": "none", "max_retries": 0}
                    },
                    {
                        "kind": "BudgetExceeded",
                        "codes": [-32603],
                        "retry": {"retryable": false, "strategy": "none", "max_retries": 0}
                    },
                    {
                        "kind": "SandboxBlocked",
                        "codes": [-32603],
                        "retry": {"retryable": false, "strategy": "none", "max_retries": 0}
                    },
                    {
                        "kind": "InternalError",
                        "codes": [-32603],
                        "retry": {"retryable": false, "strategy": "none", "max_retries": 0}
                    }
                ],
                "compatibility": {
                    "request_error_context_prefix": "acp.handle_request.dispatch"
                }
            }
        }),
    )
    .await
}

pub(super) async fn handle_cost_status(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let task = params
        .get("task")
        .and_then(Value::as_str)
        .or_else(|| params.get("objective").and_then(Value::as_str))
        .unwrap_or("");
    let hardness = summarize_hardness(task, &params);
    let cost = summarize_token_cost_governance(task, &params, hardness, &server.metrics.snapshot());

    send_result(server, request_id, json!({ "ok": true, "cost": cost })).await
}

pub(super) async fn handle_autotune_reset(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let (Some(autotune), Some(config)) = (server.autotune.as_ref(), server.autotune_config.as_ref())
    else {
        return send_result(
            server,
            request_id,
            json!({
                "ok": true,
                "autotune": "disabled",
                "reset": false,
                "enabled": false,
            }),
        )
        .await;
    };

    let mut lock = autotune.lock().await;
    let before = lock.snapshot();
    *lock = AutoTuneState::new(config);
    let after = lock.snapshot();

    let mut persisted = false;
    let mut warning = None::<String>;
    if let Some(path) = &server.autotune_state_path {
        match lock.save(path) {
            Ok(()) => persisted = true,
            Err(err) => {
                warning = Some(tf(
                    "warning.failed_save_autotune",
                    &[("error", &format!("{}", err))],
                ));
            }
        }
    }

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "autotune": "reset",
            "reset": true,
            "enabled": true,
            "persisted": persisted,
            "state_before": before,
            "state_after": after,
            "warning": warning,
        }),
    )
    .await
}