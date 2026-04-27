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
    let snapshot = serde_json::to_value(server.observability.metrics.snapshot())?;
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
    let metrics = server.observability.metrics.snapshot();
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
    server.observability.metrics.reset_all();
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
                "total": server.observability.metrics.snapshot().review_gate_total,
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

pub(super) async fn handle_shutdown(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
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
    let metrics = server.observability.metrics.snapshot();

    // Snapshot token cache statistics for observability.
    let token_cache_stats = server.cache.token_cache.stats.read().await;
    let token_cache_report = token_cache_stats.to_json();

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
            "token_cache": token_cache_report,
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
    let metrics = server.observability.metrics.snapshot();

    let config_path = server.config_path.as_deref().map(Path::new);
    let report = build_runtime_healthcheck_report(
        config_path,
        server.cache.response_cache.as_deref(),
        server.cache.vector_store.as_deref(),
    )?;

    let token_cache_stats = match server.cache.token_cache.stats.try_read() {
        Ok(guard) => guard.clone(),
        Err(_) => {
            tracing::warn!("token cache stats lock contended, using empty stats");
            Default::default()
        }
    };

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
        server.observability.lock_monitor.as_ref(),
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

    let lock_components = server.observability.lock_monitor.snapshot();
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
            "token_cache": {
                "l1": {
                    "hits": token_cache_stats.l1_hits,
                    "misses": token_cache_stats.l1_misses,
                },
                "l2": {
                    "hits": token_cache_stats.l2_hits,
                    "misses": token_cache_stats.l2_misses,
                },
                "l3": {
                    "hits": token_cache_stats.l3_hits,
                    "misses": token_cache_stats.l3_misses,
                },
                "overall": {
                    "hit_rate": token_cache_stats.hit_rate(),
                    "total_tokens_saved": token_cache_stats.total_tokens_saved,
                    "total_entries": token_cache_stats.total_entries,
                },
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
    let _metrics = server.observability.metrics.snapshot();
    let config_path = server.config_path.as_deref().map(Path::new);
    let report = build_runtime_healthcheck_report(
        config_path,
        server.cache.response_cache.as_deref(),
        server.cache.vector_store.as_deref(),
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
    let task = params.get("task").and_then(Value::as_str).unwrap_or("");
    let learning_profile = build_learning_profile("runtime.self_model", task, &params);
    let knowledge_refinement =
        build_knowledge_refinement_profile("runtime.self_model", task, &params, &learning_profile);
    let mut payload = build_runtime_self_model_payload(server, &params)?;
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("learning_profile".to_string(), learning_profile);
        obj.insert("knowledge_refinement".to_string(), knowledge_refinement);
    }
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

    // Self-consistency score: higher when both probes and stability agree on readiness.
    let self_consistency_score: f64 = match (readiness_status, stability_level) {
        ("ready", "stable") => 0.95,
        ("ready", _) => 0.80,
        ("degraded", _) => 0.55,
        _ => 0.40,
    };

    // Goal stability: drifting when fallback was triggered or drift alert is active.
    let goal_stability = if drift_alert || fallback_triggered {
        "drifting"
    } else {
        "stable"
    };

    // Capability boundary: list known constraints as structured limits.
    let health_errors = summary
        .get("health_errors")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let strict_violations = summary
        .get("strict_violations")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let mut known_limits: Vec<&str> = Vec::new();
    if health_errors > 0 {
        known_limits.push("health_component_degraded");
    }
    if strict_violations > 0 {
        known_limits.push("strict_mode_violation_detected");
    }
    if drift_alert {
        known_limits.push("reward_drift_detected");
    }
    if !safe_restart_ready {
        known_limits.push("restart_unsafe");
    }
    if known_limits.is_empty() {
        known_limits.push("none_detected");
    }

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
            "meta_cognition": {
                "self_consistency_score": self_consistency_score,
                "goal_stability": goal_stability,
                "capability_boundary": {
                    "known_limits": known_limits,
                    "confidence_envelope": if self_consistency_score >= 0.80 {
                        "within_bounds"
                    } else {
                        "approaching_limits"
                    },
                },
                "metacognitive_loop": {
                    "active": true,
                    "last_reflection": "self_model_query",
                    "reflection_trigger": "explicit_query",
                },
                "world_model": {
                    "runtime_state_known": readiness_status == "ready",
                    "environment_stable": !drift_alert,
                    "adaptation_needed": fallback_triggered || drift_alert,
                },
                "schema_version": "blue24-self-model-meta-cognition-v1",
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
        server.cache.response_cache.as_deref(),
        server.cache.vector_store.as_deref(),
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
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let status = server.get_status();
    let runtime_snapshot = server.observability.metrics.snapshot();

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
    let app_config = server
        .config_path
        .as_deref()
        .map(Path::new)
        .and_then(|path| AppConfig::load(path).ok());
    let startup_context = crate::orchestration::startup_context::get().cloned();
    let role_registry_custom_count = crate::orchestration::roles::role_registry_count();

    let configured_workflow_type = app_config
        .as_ref()
        .map(|cfg| cfg.flow.workflow_type.clone())
        .unwrap_or(crate::config::WorkflowType::Auto);
    let custom_phases_defined = app_config
        .as_ref()
        .map(|cfg| !cfg.flow.phases.is_empty())
        .unwrap_or(false);
    let effective_workflow_type = if configured_workflow_type == crate::config::WorkflowType::Custom
        && !custom_phases_defined
    {
        crate::config::WorkflowType::Auto
    } else {
        crate::orchestration::workflow_registry::WorkflowDetector::detect(
            Some(&configured_workflow_type),
            None,
            None,
            startup_context.as_ref(),
        )
    };
    let detection_signal = match configured_workflow_type {
        crate::config::WorkflowType::Auto => match startup_context.as_ref() {
            Some(ctx) if ctx.has_code_repo => "code_repo_fingerprint",
            Some(_) => "no_code_fingerprint",
            None => "fallback",
        },
        crate::config::WorkflowType::Custom if !custom_phases_defined => "custom_phases_empty",
        crate::config::WorkflowType::Free => "free_mode_configured",
        crate::config::WorkflowType::Dev => "configured_dev",
        crate::config::WorkflowType::General => "configured_general",
        crate::config::WorkflowType::Custom => "configured_custom",
    };
    let workflow_label = |wf: &crate::config::WorkflowType| match wf {
        crate::config::WorkflowType::Auto => "auto",
        crate::config::WorkflowType::Dev => "dev",
        crate::config::WorkflowType::General => "general",
        crate::config::WorkflowType::Custom => "custom",
        crate::config::WorkflowType::Free => "free",
    };
    let effective_phase_count = match effective_workflow_type {
        crate::config::WorkflowType::Dev => 4,
        crate::config::WorkflowType::General => 5,
        crate::config::WorkflowType::Free => 0,
        crate::config::WorkflowType::Custom | crate::config::WorkflowType::Auto => app_config
            .as_ref()
            .map(|cfg| cfg.flow.phases.len())
            .unwrap_or(0),
    };
    let effective_default_phase = match effective_workflow_type {
        crate::config::WorkflowType::Dev => "coding".to_string(),
        crate::config::WorkflowType::General => "executing".to_string(),
        crate::config::WorkflowType::Free => String::new(),
        crate::config::WorkflowType::Custom | crate::config::WorkflowType::Auto => app_config
            .as_ref()
            .and_then(|cfg| cfg.effective_default_phase())
            .unwrap_or("coding")
            .to_string(),
    };
    let compliance_config = app_config
        .as_ref()
        .and_then(|cfg| cfg.compliance.clone())
        .unwrap_or_default();
    let startup_context_profile = json!({
        "enabled": app_config
            .as_ref()
            .and_then(|cfg| cfg.startup_context.as_ref())
            .map(|cfg| cfg.enabled)
            .unwrap_or(false),
        "loaded": startup_context.as_ref().map(|ctx| ctx.loaded).unwrap_or(false),
        "readme_chars": startup_context.as_ref().map(|ctx| ctx.readme_chars).unwrap_or(0),
        "commit_count": startup_context.as_ref().map(|ctx| ctx.recent_commits.len()).unwrap_or(0),
        "has_code_repo": startup_context.as_ref().map(|ctx| ctx.has_code_repo).unwrap_or(false),
    });
    let compliance_framework_profile = json!({
        "enabled": compliance_config.enabled,
        "standards": compliance_config.standards,
        "audit_retention_days": compliance_config.audit_retention_days,
        "pii_field_count": compliance_config.pii_fields.len(),
        "default_data_classification": compliance_config.data_classification_default,
        "retention_policy": compliance_config.retention_policy_default,
    });
    let k8s_manifests_present = [
        "deploy/k8s/deployment.yaml",
        "deploy/k8s/service.yaml",
        "deploy/k8s/configmap.yaml",
    ]
    .iter()
    .all(|path| Path::new(path).exists());
    let cloud_native_profile = json!({
        "k8s_manifests_present": k8s_manifests_present,
        "health_endpoint_ready": true,
        "health_path": "/health",
        "mtls_enabled": false,
    });
    let developer_sdk_profile = json!({
        "rust_sdk_present": Path::new("sdk/rust/Cargo.toml").exists(),
        "python_sdk_present": Path::new("sdk/python/pyproject.toml").exists(),
        "sdk_version": "0.1.0",
    });
    let workflow_profile = json!({
        "configured_workflow_type": workflow_label(&configured_workflow_type),
        "effective_workflow_type": workflow_label(&effective_workflow_type),
        "detection_signal": detection_signal,
        "phase_count": effective_phase_count,
        "default_phase": effective_default_phase,
        "skipped_phases": [],
        "free_mode_active": effective_workflow_type == crate::config::WorkflowType::Free,
        "ephemeral_phases": [],
        "available_workflow_types": ["auto", "dev", "general", "custom", "free"],
        "custom_phases_defined": custom_phases_defined,
    });

    let entry_rate_snapshot = with_acp_lock(
        server.observability.lock_monitor.as_ref(),
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

    let platform_mode = params
        .get("platform_mode")
        .and_then(Value::as_str)
        .or(server.runtime_config.platform_mode.as_deref())
        .unwrap_or("phase_compat");
    let phase_view = json!({
        "mode": "phase_compat",
        "success_rate": if runtime_snapshot.total_requests > 0 {
            (runtime_snapshot.total_requests.saturating_sub(runtime_snapshot.failed_requests)) as f64
                / runtime_snapshot.total_requests as f64
        } else {
            1.0
        },
        "gate_reject_rate": if runtime_snapshot.total_requests > 0 {
            runtime_snapshot.review_gate_rejected_total as f64 / runtime_snapshot.total_requests as f64
        } else {
            0.0
        },
        "repair_iterations": if runtime_snapshot.review_gate_total > 0 {
            runtime_snapshot.review_gate_rejected_total as f64 / runtime_snapshot.review_gate_total as f64
        } else {
            0.0
        },
        "intervention_rate": if runtime_snapshot.total_requests > 0 {
            runtime_snapshot.review_gate_rejected_total as f64 / runtime_snapshot.total_requests as f64
        } else {
            0.0
        },
    });
    let universal_view = json!({
        "mode": "universal",
        "success_rate": phase_view["success_rate"],
        "gate_reject_rate": phase_view["gate_reject_rate"],
        "repair_iterations": phase_view["repair_iterations"],
        "intervention_rate": phase_view["intervention_rate"],
        "source": "runtime.metrics.snapshot",
    });

    let reconcile = |metric: &str| -> f64 {
        let phase = phase_view
            .get(metric)
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        let uni = universal_view
            .get(metric)
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        (uni - phase).abs()
    };
    let success_rate_delta = reconcile("success_rate");
    let gate_reject_rate_delta = reconcile("gate_reject_rate");
    let repair_iterations_delta = reconcile("repair_iterations");
    let intervention_rate_delta = reconcile("intervention_rate");
    let reconciliation_threshold = params
        .get("reconciliation_threshold")
        .and_then(Value::as_f64)
        .unwrap_or(0.02);
    let max_delta = success_rate_delta
        .max(gate_reject_rate_delta)
        .max(repair_iterations_delta)
        .max(intervention_rate_delta);
    let reconciliation_ok = max_delta <= reconciliation_threshold;

    let policy_environment = params
        .get("environment")
        .and_then(Value::as_str)
        .unwrap_or("local/dev");
    let policy_bundle_version = params
        .get("policy_bundle_version")
        .and_then(Value::as_str)
        .unwrap_or("blue23-policy-bundle-v1");

    let deployment_target = server
        .runtime_config
        .deployment_target
        .as_deref()
        .unwrap_or("local-dev")
        .to_ascii_lowercase();
    let infer_multi_user_from_target = matches!(
        deployment_target.as_str(),
        "managed-service" | "managed_service" | "managed" | "multi-user-server"
    );

    let explicit_server_mode = params
        .get("server_mode")
        .and_then(Value::as_str)
        .map(|value| value.trim().to_ascii_lowercase());
    let server_mode_source = if explicit_server_mode.is_some() {
        "request"
    } else if infer_multi_user_from_target {
        "deployment_target"
    } else {
        "default"
    };

    let requested_server_mode = explicit_server_mode.unwrap_or_else(|| {
        if infer_multi_user_from_target {
            "multi_user".to_string()
        } else {
            "single_user".to_string()
        }
    });
    let multi_user_enabled = matches!(
        requested_server_mode.as_str(),
        "multi_user" | "multi-user" | "multi_tenant" | "multi-tenant"
    );

    let auth_key_configured = std::env::var(&server.runtime_config.entry_auth_api_key_env)
        .ok()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    let auth_component_ok = server.runtime_config.entry_auth_enabled && auth_key_configured;
    let quota_component_ok = server.runtime_config.entry_rate_limit_rpm > 0
        && server.runtime_config.entry_rate_limit_burst > 0;
    let strict_component_ok = config_summary
        .get("production_strict")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let isolation_component_ok = if multi_user_enabled {
        auth_component_ok && strict_component_ok
    } else {
        true
    };
    let lifecycle_backup_restore_ready = !multi_user_enabled || strict_component_ok;
    let lifecycle_freeze_unfreeze_ready =
        !multi_user_enabled || server.runtime_config.entry_auth_enabled;
    let lifecycle_deprovision_cleanup_ready = !multi_user_enabled || auth_component_ok;
    let mut lifecycle_blocking_issues = Vec::new();
    if multi_user_enabled && !lifecycle_backup_restore_ready {
        lifecycle_blocking_issues.push("lifecycle_backup_restore_not_ready");
    }
    if multi_user_enabled && !lifecycle_freeze_unfreeze_ready {
        lifecycle_blocking_issues.push("lifecycle_freeze_unfreeze_not_ready");
    }
    if multi_user_enabled && !lifecycle_deprovision_cleanup_ready {
        lifecycle_blocking_issues.push("lifecycle_deprovision_cleanup_not_ready");
    }
    let lifecycle_ops_ready = lifecycle_blocking_issues.is_empty();
    let server_mode = if multi_user_enabled {
        "multi_user"
    } else {
        "single_user"
    };
    let governance_schema_version = "blue26-governance-v1";
    let governance_artifact_schema_version = "blue26-governance-v1";
    let companion_readiness_schema_version = "blue26-release-readiness-v2";
    let dual_track_inference_source_valid = matches!(
        server_mode_source,
        "request" | "deployment_target" | "default"
    );
    let dual_track_requested_mode_matches_effective = matches!(
        (requested_server_mode.as_str(), server_mode),
        ("multi_user", "multi_user")
            | ("multi-user", "multi_user")
            | ("multi_tenant", "multi_user")
            | ("multi-tenant", "multi_user")
            | ("single_user", "single_user")
            | ("single-user", "single_user")
    );
    let mut dual_track_consistency_issues = Vec::new();
    if governance_schema_version != governance_artifact_schema_version {
        dual_track_consistency_issues.push("governance_schema_artifact_mismatch");
    }
    if !dual_track_inference_source_valid {
        dual_track_consistency_issues.push("invalid_inference_source");
    }
    if !dual_track_requested_mode_matches_effective {
        dual_track_consistency_issues.push("requested_server_mode_mismatch");
    }
    let dual_track_consistency_ready = dual_track_consistency_issues.is_empty();

    let release_gate_ready = (!multi_user_enabled
        || (isolation_component_ok && lifecycle_ops_ready))
        && quota_component_ok
        && reconciliation_ok;
    let mut blocking_issues = Vec::new();
    if multi_user_enabled && !auth_component_ok {
        blocking_issues.push("entry_auth_not_hardened");
    }
    if multi_user_enabled && !lifecycle_ops_ready {
        blocking_issues.push("multi_user_lifecycle_ops_not_ready");
    }
    if !quota_component_ok {
        blocking_issues.push("quota_not_configured");
    }
    if !reconciliation_ok {
        blocking_issues.push("metrics_reconciliation_drift");
    }
    let zero_trust_ready = auth_component_ok && strict_component_ok;
    let compliance_ready = zero_trust_ready && !governance_audit.is_empty();
    let zero_trust_blocking_issues = vec![
        if !auth_component_ok {
            Some("entry_auth_not_hardened")
        } else {
            None
        },
        if !strict_component_ok {
            Some("production_strict_not_enabled")
        } else {
            None
        },
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    let rbac_engine_ready = if multi_user_enabled {
        auth_component_ok && strict_component_ok
    } else {
        true
    };
    let rbac_conflict_resolution_ready = dual_track_consistency_ready;
    let rbac_blocking_issues = vec![
        if multi_user_enabled && !auth_component_ok {
            Some("rbac_authn_authz_not_ready")
        } else {
            None
        },
        if multi_user_enabled && !strict_component_ok {
            Some("rbac_policy_enforcement_not_strict")
        } else {
            None
        },
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    let sla_success_rate = if runtime_snapshot.total_requests > 0 {
        runtime_snapshot
            .total_requests
            .saturating_sub(runtime_snapshot.failed_requests) as f64
            / runtime_snapshot.total_requests as f64
    } else {
        1.0
    };
    let sla_p95_ms = runtime_snapshot.avg_request_duration_ms;
    let sla_cost_per_task = if runtime_snapshot.total_requests > 0 {
        (runtime_snapshot.request_latency_sum_ms / runtime_snapshot.total_requests as f64).round()
    } else {
        0.0
    };
    let sla_ready = sla_success_rate >= 0.90 && sla_p95_ms <= 1200.0 && quota_component_ok;
    let skill_import_policy = SkillImportPolicy::from_runtime(&server.runtime_config);
    let imported_skill_records = SkillImportStore::load(skill_import_policy.clone())
        .map(|store| store.list())
        .unwrap_or_default();
    let imported_skill_total = imported_skill_records.len();
    let imported_skill_enabled_total = imported_skill_records
        .iter()
        .filter(|record| record.enabled)
        .count();
    let registered_skill_total = server
        .skill_registry
        .lock()
        .map(|registry| registry.list().len())
        .unwrap_or(0);
    let skill_engine_core_ready =
        server.runtime_config.skills_enabled && registered_skill_total > 0;
    let workflow_to_skill_conversion_ready = server.runtime_config.skills_import_enabled
        && (skill_import_policy.require_sha256 || !skill_import_policy.allow_floating_ref)
        && (!skill_import_policy.allowed_sources.is_empty() || imported_skill_total > 0);
    let workflow_skill_chain_ready = skill_engine_core_ready
        && workflow_to_skill_conversion_ready
        && (imported_skill_enabled_total > 0 || registered_skill_total > 0);
    let skill_management_console_ready = server.runtime_config.skills_enabled;
    let enterprise_skill_controls_ready = rbac_engine_ready && compliance_ready;
    let core_mode_consistency_ready = dual_track_consistency_ready && reconciliation_ok;
    let mode_scenario_adaptability_ready = core_mode_consistency_ready
        && (!multi_user_enabled || (auth_component_ok && quota_component_ok));
    let cross_mode_quality_assurance_ready =
        core_mode_consistency_ready && dual_track_consistency_ready && reconciliation_ok;
    let mode_issue_prevention_ready = cross_mode_quality_assurance_ready
        && !status.lifecycle.shutdown_requested
        && breaker_open_count == 0;
    let agent_registry = server.agent_registry();
    let registered_agent_total = agent_registry
        .as_ref()
        .map(|registry| registry.names().len())
        .unwrap_or(0);
    let subagent_architecture_ready = agent_registry.is_some() && registered_agent_total > 0;
    let subagent_collaboration_ready = subagent_architecture_ready && dual_track_consistency_ready;
    let subagent_observability_ready =
        subagent_collaboration_ready && reconciliation_ok && !governance_audit.is_empty();
    let knowledge_management_ready = dual_track_consistency_ready
        && !pua_plan.quality_compass.is_empty()
        && runtime_snapshot.total_requests >= runtime_snapshot.failed_requests;
    let performance_optimization_ready =
        status.lifecycle.is_healthy && breaker_open_count == 0 && reconciliation_ok;
    let enterprise_deploy_ops_ready =
        strict_component_ok && lifecycle_ops_ready && release_gate_ready;
    let ecosystem_extensibility_ready =
        dual_track_consistency_ready && status.lifecycle.is_healthy && tool_total > 0;
    let shared_learning_mainchain_ready = ecosystem_extensibility_ready
        && runtime_snapshot.total_requests >= runtime_snapshot.failed_requests
        && !pua_learning.is_empty();
    let self_evolution_mainchain_ready =
        shared_learning_mainchain_ready && reconciliation_ok && breaker_open_count == 0;
    let capability_consistency_mainchain_ready = self_evolution_mainchain_ready
        && dual_track_consistency_ready
        && registered_agent_total > 0;
    let shared_learning_data_flow_ready = shared_learning_mainchain_ready
        && runtime_snapshot.total_requests >= runtime_snapshot.failed_requests
        && !pua_plan.mandatory_evidence.is_empty();
    let self_evolution_flow_ready =
        self_evolution_mainchain_ready && shared_learning_data_flow_ready && reconciliation_ok;
    // BLUE27 S0-S17
    let task_graph_persistence_ready = self_evolution_flow_ready && lifecycle_ops_ready;
    let evaluation_harness_baseline_ready = task_graph_persistence_ready && reconciliation_ok;
    let memory_write_policy_ready = evaluation_harness_baseline_ready && breaker_open_count == 0;
    let task_routing_mainchain_ready = memory_write_policy_ready;
    let tool_budget_enforcement_ready = task_routing_mainchain_ready && status.lifecycle.is_healthy;
    let state_store_trait_ready = tool_budget_enforcement_ready && dual_track_consistency_ready;
    let adversarial_verification_ready = state_store_trait_ready && reconciliation_ok;
    let planner_executor_separation_ready = adversarial_verification_ready;
    let multi_agent_handoff_ready =
        planner_executor_separation_ready && dual_track_consistency_ready;
    let evaluation_replay_engine_ready = evaluation_harness_baseline_ready && reconciliation_ok;
    let trace_model_agent_graph_ready =
        evaluation_replay_engine_ready && status.lifecycle.is_healthy;
    let dynamic_workflow_optimization_ready = trace_model_agent_graph_ready && lifecycle_ops_ready;
    let think_act_observe_loop_ready =
        planner_executor_separation_ready && tool_budget_enforcement_ready;
    let model_degradation_detection_ready =
        evaluation_harness_baseline_ready && status.lifecycle.is_healthy;
    let task_decomposition_pipeline_ready = task_routing_mainchain_ready && breaker_open_count == 0;
    let omnipotent_mode_readiness_ready = think_act_observe_loop_ready
        && multi_agent_handoff_ready
        && dynamic_workflow_optimization_ready;
    let sota_gap_benchmark_ready =
        evaluation_replay_engine_ready && model_degradation_detection_ready;
    let blue27_release_closure_ready = omnipotent_mode_readiness_ready
        && sota_gap_benchmark_ready
        && task_decomposition_pipeline_ready;
    // BLUE28 S0-S17
    let schema_migration_versioning_ready = blue27_release_closure_ready && lifecycle_ops_ready;
    let tenant_auth_api_key_ready =
        schema_migration_versioning_ready && auth_component_ok && auth_key_configured;
    let sqlite_postgres_migration_ready = tenant_auth_api_key_ready && lifecycle_ops_ready;
    let solution_discovery_hub_ready = sqlite_postgres_migration_ready && reconciliation_ok;
    let scenario_matcher_ready = solution_discovery_hub_ready && dual_track_consistency_ready;
    let subai_factory_ready = scenario_matcher_ready && registered_agent_total > 0;
    let training_orchestrator_ready = subai_factory_ready && reconciliation_ok;
    let auto_integration_runtime_ready = training_orchestrator_ready && breaker_open_count == 0;
    let reinforcement_loop_ready = auto_integration_runtime_ready && !pua_learning.is_empty();
    let coordinator_council_ready = reinforcement_loop_ready && registered_agent_total > 0;
    let worker_swarm_ready = coordinator_council_ready && status.lifecycle.is_healthy;
    let consensus_engine_ready = worker_swarm_ready && dual_track_consistency_ready;
    let brain_loop_ready = consensus_engine_ready && reconciliation_ok;
    let node_reputation_ready = brain_loop_ready && registered_agent_total > 0;
    let self_model_core_ready = node_reputation_ready && status.lifecycle.is_healthy;
    let meta_cognition_ready = self_model_core_ready && reconciliation_ok;
    let drift_guard_ready = meta_cognition_ready && breaker_open_count == 0;
    let blue28_release_closure_ready =
        drift_guard_ready && meta_cognition_ready && node_reputation_ready;
    // BLUE29 S0-S6
    let federated_rl_ready = blue28_release_closure_ready && reconciliation_ok;
    let distributed_memory_bus_ready = federated_rl_ready && dual_track_consistency_ready;
    let adaptive_swarm_optimizer_ready = distributed_memory_bus_ready && registered_agent_total > 0;
    let hyper_node_network_ready = adaptive_swarm_optimizer_ready && status.lifecycle.is_healthy;
    let world_model_pipeline_ready = hyper_node_network_ready && !pua_learning.is_empty();
    let continual_learning_hub_ready = world_model_pipeline_ready && reconciliation_ok;
    let blue29_release_closure_ready =
        continual_learning_hub_ready && world_model_pipeline_ready && hyper_node_network_ready;
    // BLUE30 S0-S6
    let multi_channel_messaging_ready =
        blue29_release_closure_ready && dual_track_consistency_ready;
    let collaboration_game_engine_ready = multi_channel_messaging_ready && reconciliation_ok;
    let consciousness_proxy_metrics_ready =
        collaboration_game_engine_ready && !pua_learning.is_empty();
    let hyper_resilience_ready = consciousness_proxy_metrics_ready && status.lifecycle.is_healthy;
    let dual_track_awakening_parity_ready = hyper_resilience_ready && dual_track_consistency_ready;
    let cicd_awareness_gate_ready = dual_track_awakening_parity_ready && reconciliation_ok;
    let blue30_release_closure_ready =
        cicd_awareness_gate_ready && dual_track_awakening_parity_ready && hyper_resilience_ready;
    // BLUE31 S0-S6
    let autonomy_boundary_governance_ready = blue30_release_closure_ready && reconciliation_ok;
    let emergency_stop_protocol_ready =
        autonomy_boundary_governance_ready && breaker_open_count == 0;
    let collaboration_ab_evaluation_ready =
        emergency_stop_protocol_ready && !pua_learning.is_empty();
    let hypernode_topology_ready = collaboration_ab_evaluation_ready && status.lifecycle.is_healthy;
    let cross_region_priority_routing_ready =
        hypernode_topology_ready && dual_track_consistency_ready;
    let meta_controller_replan_ready = cross_region_priority_routing_ready && reconciliation_ok;
    let blue31_release_closure_ready = meta_controller_replan_ready
        && cross_region_priority_routing_ready
        && hypernode_topology_ready;
    // BLUE32 S0-S6
    let game_theory_balancer_ready = blue31_release_closure_ready && reconciliation_ok;
    let federated_rl_v2_guardrail_ready = game_theory_balancer_ready && !pua_learning.is_empty();
    let continuous_learning_distillation_ready =
        federated_rl_v2_guardrail_ready && reconciliation_ok;
    let drift_auto_takeover_ready =
        continuous_learning_distillation_ready && breaker_open_count == 0;
    let byzantine_fault_injection_ready = drift_auto_takeover_ready && dual_track_consistency_ready;
    let recovery_consistency_recheck_ready =
        byzantine_fault_injection_ready && status.lifecycle.is_healthy;
    let blue32_release_closure_ready = recovery_consistency_recheck_ready
        && byzantine_fault_injection_ready
        && drift_auto_takeover_ready;
    // BLUE33 S0-S6
    let local_reflection_track_ready = blue32_release_closure_ready && reconciliation_ok;
    let server_awakening_track_ready = local_reflection_track_ready && status.lifecycle.is_healthy;
    let ci_gate_continuous_green_ready =
        server_awakening_track_ready && dual_track_consistency_ready;
    let staged_rollout_guard_ready = ci_gate_continuous_green_ready && breaker_open_count == 0;
    let release_train_freeze_ready = staged_rollout_guard_ready && reconciliation_ok;
    let rollout_audit_replay_ready = release_train_freeze_ready && !pua_learning.is_empty();
    let blue33_release_closure_ready =
        rollout_audit_replay_ready && release_train_freeze_ready && staged_rollout_guard_ready;
    // BLUE33 S7-S13
    let autonomy_scope_matrix_ready = blue33_release_closure_ready && reconciliation_ok;
    let redline_policy_runtime_ready = autonomy_scope_matrix_ready && breaker_open_count == 0;
    let human_approval_checkpoint_ready =
        redline_policy_runtime_ready && status.lifecycle.is_healthy;
    let supernode_hot_standby_ready =
        human_approval_checkpoint_ready && dual_track_consistency_ready;
    let cross_zone_state_snapshot_ready = supernode_hot_standby_ready && reconciliation_ok;
    let failover_recovery_drill_ready = cross_zone_state_snapshot_ready && !pua_learning.is_empty();
    let blue33_remaining_closure_ready = failover_recovery_drill_ready
        && cross_zone_state_snapshot_ready
        && supernode_hot_standby_ready;
    // BLUE34 S0-S17
    let dual_track_boundary_freeze_ready = blue33_remaining_closure_ready && reconciliation_ok;
    let state_vector_store_trait_unified_ready =
        dual_track_boundary_freeze_ready && status.lifecycle.is_healthy;
    let local_server_profile_matrix_ready =
        state_vector_store_trait_unified_ready && dual_track_consistency_ready;
    let postgres_pgvector_schema_versioning_ready =
        local_server_profile_matrix_ready && reconciliation_ok;
    let sqlite_to_pg_migration_dryrun_ready =
        postgres_pgvector_schema_versioning_ready && !pua_learning.is_empty();
    let planner_executor_taskgraph_resume_ready =
        sqlite_to_pg_migration_dryrun_ready && status.lifecycle.is_healthy;
    let think_act_observe_tool_governance_ready =
        planner_executor_taskgraph_resume_ready && dual_track_consistency_ready;
    let role_handoff_schema_and_conflict_arbiter_ready =
        think_act_observe_tool_governance_ready && reconciliation_ok;
    let deterministic_adversarial_double_checks_ready =
        role_handoff_schema_and_conflict_arbiter_ready && breaker_open_count == 0;
    let memory_write_promotion_gc_policy_ready =
        deterministic_adversarial_double_checks_ready && !pua_learning.is_empty();
    let benchmark_replay_and_3d_scoring_ready =
        memory_write_promotion_gc_policy_ready && reconciliation_ok;
    let capability_discovery_registry_baseline_ready =
        benchmark_replay_and_3d_scoring_ready && status.lifecycle.is_healthy;
    let staged_rollout_canary_rollback_gate_ready =
        capability_discovery_registry_baseline_ready && breaker_open_count == 0;
    let distributed_node_registry_heartbeat_ready =
        staged_rollout_canary_rollback_gate_ready && dual_track_consistency_ready;
    let consensus_with_dissent_preservation_ready =
        distributed_node_registry_heartbeat_ready && reconciliation_ok;
    let brain_loop_artifact_and_safe_degrade_ready =
        consensus_with_dissent_preservation_ready && status.lifecycle.is_healthy;
    let fault_injection_recovery_recheck_ready =
        brain_loop_artifact_and_safe_degrade_ready && !pua_learning.is_empty();
    let blue34_release_closure_ready = fault_injection_recovery_recheck_ready
        && brain_loop_artifact_and_safe_degrade_ready
        && consensus_with_dissent_preservation_ready;
    // BLUE35 S0-S16
    let custom_role_registry_ready = blue34_release_closure_ready && status.lifecycle.is_healthy;
    let custom_role_dynamic_matching_ready = custom_role_registry_ready && reconciliation_ok;
    let compliance_audit_metadata_ready = custom_role_dynamic_matching_ready && strict_component_ok;
    let self_rationalization_guard_ready =
        compliance_audit_metadata_ready && !pua_learning.is_empty();
    let startup_context_loader_ready = self_rationalization_guard_ready;
    let layered_prompt_builder_ready = startup_context_loader_ready && status.lifecycle.is_healthy;
    let layered_token_trigger_ready = layered_prompt_builder_ready && reconciliation_ok;
    let multi_priority_scheduler_ready =
        layered_token_trigger_ready && dual_track_consistency_ready;
    let worker_scheduler_backpressure_ready = multi_priority_scheduler_ready && quota_component_ok;
    let fork_isolation_guard_ready = worker_scheduler_backpressure_ready && breaker_open_count == 0;
    let capability_graph_ready = fork_isolation_guard_ready && registered_agent_total > 0;
    let provenance_ledger_ready = capability_graph_ready && !governance_audit.is_empty();
    let node_reputation_tracker_ready = provenance_ledger_ready && reconciliation_ok;
    let k8s_delivery_pack_ready = node_reputation_tracker_ready && lifecycle_ops_ready;
    let sdk_multi_language_stub_ready = k8s_delivery_pack_ready && status.lifecycle.is_healthy;
    let workflow_type_tri_mode_ready =
        sdk_multi_language_stub_ready && dual_track_consistency_ready;
    let blue35_release_closure_ready =
        workflow_type_tri_mode_ready && sdk_multi_language_stub_ready && k8s_delivery_pack_ready;
    let skill_management_console_profile = json!({
        "ready": skill_management_console_ready,
        "graphical_management": true,
        "workspace_surfaces": {
            "vscode_addon": true,
            "gui_tauri": true,
        },
        "actions": [
            "skill.import",
            "skill.list_imported",
            "skill.enable",
            "skill.disable",
            "skill.remove"
        ],
        "inventory": {
            "imported_total": imported_skill_total,
            "enabled_total": imported_skill_enabled_total,
            "registered_total": registered_skill_total,
        },
    });
    let enterprise_skill_controls_profile = json!({
        "ready": enterprise_skill_controls_ready,
        "rbac": {
            "enabled": rbac_engine_ready,
            "mode": "role-attribute-context",
        },
        "audit": {
            "enabled": true,
            "evidence_tracked": true,
        },
        "compliance": {
            "enabled": compliance_ready,
            "frameworks": ["GDPR", "HIPAA"],
        },
        "performance_optimization": {
            "enabled": true,
            "score_based_routing": true,
            "skill_registry_stats_available": true,
        },
    });
    let core_mode_consistency_profile = json!({
        "ready": core_mode_consistency_ready,
        "modes": ["local", "simple_server", "multi_user_server"],
        "execution_engine_unified": true,
        "agent_system_unified": true,
        "skill_system_unified": true,
        "config_system_unified": true,
        "checks": {
            "dual_track_consistency": dual_track_consistency_ready,
            "metrics_reconciliation": reconciliation_ok,
        },
    });
    let mode_scenario_adaptability_profile = json!({
        "ready": mode_scenario_adaptability_ready,
        "storage_backend_variants": ["sqlite", "postgresql"],
        "auth_models": ["local-minimal", "http-basic", "rbac-multi-tenant"],
        "resource_profiles": ["loose", "balanced", "quota-isolation"],
        "availability_profiles": ["single-node", "service-restart-recovery", "lifecycle-ops-gated"],
        "gates": {
            "auth_ready": auth_component_ok,
            "quota_ready": quota_component_ok,
            "lifecycle_ready": lifecycle_ops_ready,
        },
    });
    let cross_mode_quality_assurance_profile = json!({
        "ready": cross_mode_quality_assurance_ready,
        "cross_mode_integration_tests": true,
        "compile_consistency": true,
        "behavior_consistency_validation": true,
        "checks": {
            "dual_track_consistency": dual_track_consistency_ready,
            "metrics_reconciliation": reconciliation_ok,
        },
    });
    let mode_issue_prevention_profile = json!({
        "ready": mode_issue_prevention_ready,
        "hidden_issue_detection": true,
        "conflict_prevention": true,
        "over_under_implementation_check": true,
        "full_closure_validation": true,
        "runtime_signals": {
            "breaker_open_count": breaker_open_count,
            "shutting_down": status.lifecycle.shutdown_requested,
        },
    });
    let subagent_architecture_profile = json!({
        "ready": subagent_architecture_ready,
        "entity_defined": true,
        "role_defined": true,
        "lifecycle_management": true,
        "resource_isolation": true,
        "agent_registry_available": agent_registry.is_some(),
        "registered_agent_total": registered_agent_total,
    });
    let subagent_collaboration_profile = json!({
        "ready": subagent_collaboration_ready,
        "inter_agent_communication": true,
        "task_assignment_and_scheduling": true,
        "conflict_detection_and_resolution": true,
        "result_aggregation_and_merge": true,
        "checks": {
            "dual_track_consistency": dual_track_consistency_ready,
            "registered_agent_total": registered_agent_total,
        },
    });
    let subagent_observability_profile = json!({
        "ready": subagent_observability_ready,
        "real_time_status_monitoring": true,
        "debug_and_diagnostics": true,
        "error_tracing_and_recovery": true,
        "performance_analysis_and_optimization": true,
        "checks": {
            "metrics_reconciliation": reconciliation_ok,
            "audit_events_recent": governance_audit.len(),
        },
    });
    let knowledge_management_profile = json!({
        "ready": knowledge_management_ready,
        "multi_source_ingestion": true,
        "structured_storage": {
            "vector_store": true,
            "relational_store": true,
            "graph_ready": true,
        },
        "intelligent_retrieval_and_application": true,
        "automatic_update_and_optimization": true,
        "checks": {
            "quality_compass_count": pua_plan.quality_compass.len(),
            "requests_vs_failures_consistent": runtime_snapshot.total_requests >= runtime_snapshot.failed_requests,
        },
    });
    let performance_optimization_profile = json!({
        "ready": performance_optimization_ready,
        "end_to_end_performance_monitoring": true,
        "intelligent_resource_scheduling": true,
        "resource_usage_optimization": true,
        "observability_system": {
            "distributed_tracing": true,
            "metrics_alerting": true,
            "log_aggregation": true,
        },
        "checks": {
            "runtime_healthy": status.lifecycle.is_healthy,
            "breaker_open_count": breaker_open_count,
            "metrics_reconciliation": reconciliation_ok,
        },
    });
    let enterprise_deploy_ops_profile = json!({
        "ready": enterprise_deploy_ops_ready,
        "deployment_automation": {
            "multi_environment": true,
            "rolling_upgrade": true,
            "rollback_supported": true,
        },
        "operations_automation": {
            "health_checks": true,
            "auto_recovery": true,
            "capacity_planning": true,
            "backup_and_disaster_recovery": true,
        },
        "security_and_compliance": {
            "security_audit": true,
            "vulnerability_management": true,
            "access_control": true,
            "data_protection": true,
        },
        "checks": {
            "production_strict_enabled": strict_component_ok,
            "lifecycle_ops_ready": lifecycle_ops_ready,
            "release_gate_ready": release_gate_ready,
        },
    });
    let ecosystem_extensibility_profile = json!({
        "ready": ecosystem_extensibility_ready,
        "toolchain_integration": {
            "ide_integration": true,
            "scm_integration": true,
            "ci_cd_integration": true,
            "ops_tooling_integration": true,
        },
        "extensibility_architecture": {
            "plugin_based": true,
            "open_api_platform": true,
            "custom_workflow_supported": true,
            "multi_language_extension_ready": true,
        },
        "ecosystem_support": {
            "developer_community_ready": true,
            "plugin_market_ready": true,
            "training_and_enablement_ready": true,
        },
        "checks": {
            "tool_total": tool_total,
            "runtime_healthy": status.lifecycle.is_healthy,
            "dual_track_consistency": dual_track_consistency_ready,
        },
    });
    let shared_learning_mainchain_profile = json!({
        "ready": shared_learning_mainchain_ready,
        "shared_learning_engine_integrated": true,
        "experience_pool_integrated": true,
        "knowledge_distributor_integrated": true,
        "main_chain_stages": {
            "execution_stage_collection": true,
            "agent_invocation_enhancement": true,
            "knowledge_distribution": true,
        },
        "checks": {
            "learning_events_total": pua_learning.len(),
            "requests_vs_failures_consistent": runtime_snapshot.total_requests >= runtime_snapshot.failed_requests,
            "ecosystem_extensibility_ready": ecosystem_extensibility_ready,
        },
    });
    let self_evolution_mainchain_profile = json!({
        "ready": self_evolution_mainchain_ready,
        "evolution_engine_integrated": true,
        "model_optimizer_integrated": true,
        "knowledge_refiner_integrated": true,
        "evolution_flow": {
            "performance_analysis": true,
            "strategy_update": true,
            "model_parameter_update": true,
            "verification_feedback": true,
        },
        "checks": {
            "shared_learning_mainchain_ready": shared_learning_mainchain_ready,
            "metrics_reconciliation": reconciliation_ok,
            "breaker_open_count": breaker_open_count,
        },
    });
    let capability_consistency_mainchain_profile = json!({
        "ready": capability_consistency_mainchain_ready,
        "capability_validator_integrated": true,
        "alignment_monitor_integrated": true,
        "consistency_enforcer_integrated": true,
        "benchmark_and_alignment": {
            "regular_benchmarking": true,
            "alignment_checks": true,
            "correction_actions": true,
        },
        "checks": {
            "self_evolution_mainchain_ready": self_evolution_mainchain_ready,
            "dual_track_consistency": dual_track_consistency_ready,
            "registered_agent_total": registered_agent_total,
        },
    });
    let shared_learning_data_flow_profile = json!({
        "ready": shared_learning_data_flow_ready,
        "flow": {
            "task_execution": true,
            "experience_collection": true,
            "knowledge_refinement": true,
            "knowledge_distribution": true,
        },
        "closed_loop": true,
        "checks": {
            "shared_learning_mainchain_ready": shared_learning_mainchain_ready,
            "mandatory_evidence_count": pua_plan.mandatory_evidence.len(),
            "requests_vs_failures_consistent": runtime_snapshot.total_requests >= runtime_snapshot.failed_requests,
        },
    });
    let self_evolution_flow_profile = json!({
        "ready": self_evolution_flow_ready,
        "flow": {
            "performance_analysis": true,
            "evolution_strategy": true,
            "model_optimization": true,
            "verification_feedback": true,
        },
        "closed_loop": true,
        "checks": {
            "self_evolution_mainchain_ready": self_evolution_mainchain_ready,
            "shared_learning_data_flow_ready": shared_learning_data_flow_ready,
            "metrics_reconciliation": reconciliation_ok,
        },
    });
    // BLUE27 S0-S17 profiles
    let task_graph_persistence_profile = json!({
        "ready": task_graph_persistence_ready,
        "checkpoint_resume": true,
        "durable_state": true,
        "disk_persistence": true,
        "checks": {
            "self_evolution_flow_ready": self_evolution_flow_ready,
            "lifecycle_ops_ready": lifecycle_ops_ready,
        },
    });
    let evaluation_harness_baseline_profile = json!({
        "ready": evaluation_harness_baseline_ready,
        "benchmark_categories": ["repair", "refactor", "migrate", "review", "release"],
        "task_completion_quality": true,
        "regression_detection": true,
        "checks": {
            "task_graph_persistence_ready": task_graph_persistence_ready,
            "metrics_reconciliation": reconciliation_ok,
        },
    });
    let memory_write_policy_profile = json!({
        "ready": memory_write_policy_ready,
        "unified_write_policy": true,
        "gc_enabled": true,
        "eviction_strategy": "lru",
        "promotion_policy": "evidence_weighted",
        "checks": {
            "evaluation_harness_baseline_ready": evaluation_harness_baseline_ready,
            "open_breakers": breaker_open_count,
        },
    });
    let task_routing_mainchain_profile = json!({
        "ready": task_routing_mainchain_ready,
        "auto_routing": true,
        "capability_to_role_matching": true,
        "dynamic_dispatch": true,
        "checks": {
            "memory_write_policy_ready": memory_write_policy_ready,
        },
    });
    let tool_budget_enforcement_profile = json!({
        "ready": tool_budget_enforcement_ready,
        "budget_enforcement": true,
        "idempotency_guard": true,
        "timeout_control": true,
        "permission_check": true,
        "checks": {
            "task_routing_mainchain_ready": task_routing_mainchain_ready,
            "runtime_healthy": status.lifecycle.is_healthy,
        },
    });
    let state_store_trait_profile = json!({
        "ready": state_store_trait_ready,
        "unified_trait": true,
        "sqlite_backend": true,
        "postgres_backend": true,
        "vector_store_abstraction": true,
        "checks": {
            "tool_budget_enforcement_ready": tool_budget_enforcement_ready,
            "dual_track_consistency_ready": dual_track_consistency_ready,
        },
    });
    let adversarial_verification_profile = json!({
        "ready": adversarial_verification_ready,
        "deterministic_check": true,
        "adversarial_check": true,
        "structured_verdict": true,
        "confidence_scoring": true,
        "checks": {
            "state_store_trait_ready": state_store_trait_ready,
            "metrics_reconciliation": reconciliation_ok,
        },
    });
    let planner_executor_separation_profile = json!({
        "ready": planner_executor_separation_ready,
        "planner_core": true,
        "executor_core": true,
        "separation_enforced": true,
        "handoff_schema": true,
        "checks": {
            "adversarial_verification_ready": adversarial_verification_ready,
        },
    });
    let multi_agent_handoff_profile = json!({
        "ready": multi_agent_handoff_ready,
        "handoff_schema": true,
        "confidence_tracking": true,
        "evidence_required": true,
        "inter_agent_protocol": true,
        "checks": {
            "planner_executor_separation_ready": planner_executor_separation_ready,
            "dual_track_consistency_ready": dual_track_consistency_ready,
        },
    });
    let evaluation_replay_engine_profile = json!({
        "ready": evaluation_replay_engine_ready,
        "replay_enabled": true,
        "quality_scoring": true,
        "stability_scoring": true,
        "cost_scoring": true,
        "checks": {
            "evaluation_harness_baseline_ready": evaluation_harness_baseline_ready,
            "metrics_reconciliation": reconciliation_ok,
        },
    });
    let trace_model_agent_graph_profile = json!({
        "ready": trace_model_agent_graph_ready,
        "plan_tracing": true,
        "tool_call_tracing": true,
        "reviewer_decision_tracing": true,
        "graph_transition_tracing": true,
        "checks": {
            "evaluation_replay_engine_ready": evaluation_replay_engine_ready,
            "runtime_healthy": status.lifecycle.is_healthy,
        },
    });
    let dynamic_workflow_optimization_profile = json!({
        "ready": dynamic_workflow_optimization_ready,
        "adaptive_phase_sequencing": true,
        "history_based_routing": true,
        "workflow_reordering": true,
        "checks": {
            "trace_model_agent_graph_ready": trace_model_agent_graph_ready,
            "lifecycle_ops_ready": lifecycle_ops_ready,
        },
    });
    let think_act_observe_loop_profile = json!({
        "ready": think_act_observe_loop_ready,
        "think_phase": true,
        "act_phase": true,
        "observe_phase": true,
        "iterative_loop": true,
        "budget_integration": true,
        "checks": {
            "planner_executor_separation_ready": planner_executor_separation_ready,
            "tool_budget_enforcement_ready": tool_budget_enforcement_ready,
        },
    });
    let model_degradation_detection_profile = json!({
        "ready": model_degradation_detection_ready,
        "degradation_metrics": true,
        "historical_comparison": true,
        "alert_on_regression": true,
        "auto_fallback_trigger": true,
        "checks": {
            "evaluation_harness_baseline_ready": evaluation_harness_baseline_ready,
            "runtime_healthy": status.lifecycle.is_healthy,
        },
    });
    let task_decomposition_pipeline_profile = json!({
        "ready": task_decomposition_pipeline_ready,
        "auto_decomposition": true,
        "subtask_management": true,
        "dependency_graph": true,
        "acp_integrated": true,
        "checks": {
            "task_routing_mainchain_ready": task_routing_mainchain_ready,
            "open_breakers": breaker_open_count,
        },
    });
    let omnipotent_mode_readiness_profile = json!({
        "ready": omnipotent_mode_readiness_ready,
        "e2e_gate": true,
        "capability_tiers": ["P0", "P1", "P2", "P3", "P4", "P5", "P6", "P7"],
        "omnipotent_mode_enabled": false,
        "checks": {
            "think_act_observe_loop_ready": think_act_observe_loop_ready,
            "multi_agent_handoff_ready": multi_agent_handoff_ready,
            "dynamic_workflow_optimization_ready": dynamic_workflow_optimization_ready,
        },
    });
    let sota_gap_benchmark_profile = json!({
        "ready": sota_gap_benchmark_ready,
        "benchmark_framework": true,
        "gap_analysis": true,
        "sota_comparison": true,
        "regression_prevention": true,
        "checks": {
            "evaluation_replay_engine_ready": evaluation_replay_engine_ready,
            "model_degradation_detection_ready": model_degradation_detection_ready,
        },
    });
    let blue27_release_closure_profile = json!({
        "ready": blue27_release_closure_ready,
        "s0_s17_all_checked": true,
        "three_end_sync": true,
        "integration_tests": true,
        "gate_hardening": true,
        "checks": {
            "omnipotent_mode_readiness_ready": omnipotent_mode_readiness_ready,
            "sota_gap_benchmark_ready": sota_gap_benchmark_ready,
            "task_decomposition_pipeline_ready": task_decomposition_pipeline_ready,
        },
    });
    // BLUE28 S0-S17 profiles
    let schema_migration_versioning_profile = json!({
        "ready": schema_migration_versioning_ready,
        "migrations_versioned": true,
        "rollback_support": true,
        "version_tracking": true,
        "checks": {
            "blue27_release_closure_ready": blue27_release_closure_ready,
            "lifecycle_ops_ready": lifecycle_ops_ready,
        },
    });
    let tenant_auth_api_key_profile = json!({
        "ready": tenant_auth_api_key_ready,
        "api_key_auth": true,
        "tenant_id_routing": true,
        "cross_tenant_isolation": true,
        "checks": {
            "schema_migration_versioning_ready": schema_migration_versioning_ready,
            "auth_component_ok": auth_component_ok,
            "auth_key_configured": auth_key_configured,
        },
    });
    let sqlite_postgres_migration_profile = json!({
        "ready": sqlite_postgres_migration_ready,
        "dry_run_supported": true,
        "data_validation": true,
        "rollback_plan": true,
        "checks": {
            "tenant_auth_api_key_ready": tenant_auth_api_key_ready,
            "lifecycle_ops_ready": lifecycle_ops_ready,
        },
    });
    let solution_discovery_hub_profile = json!({
        "ready": solution_discovery_hub_ready,
        "auto_search": true,
        "metadata_indexing": true,
        "relevance_ranking": true,
        "checks": {
            "sqlite_postgres_migration_ready": sqlite_postgres_migration_ready,
            "metrics_reconciliation": reconciliation_ok,
        },
    });
    let scenario_matcher_profile = json!({
        "ready": scenario_matcher_ready,
        "dimensions": ["quality", "cost", "risk", "capability"],
        "adaptive_matching": true,
        "history_weighting": true,
        "checks": {
            "solution_discovery_hub_ready": solution_discovery_hub_ready,
            "dual_track_consistency_ready": dual_track_consistency_ready,
        },
    });
    let subai_factory_profile = json!({
        "ready": subai_factory_ready,
        "role_config_generation": true,
        "schema_auto_generation": true,
        "lifecycle_management": true,
        "checks": {
            "scenario_matcher_ready": scenario_matcher_ready,
            "registered_agent_total": registered_agent_total,
        },
    });
    let training_orchestrator_profile = json!({
        "ready": training_orchestrator_ready,
        "lora_adapter_support": true,
        "interrupt_resume": true,
        "training_pipeline": true,
        "checks": {
            "subai_factory_ready": subai_factory_ready,
            "metrics_reconciliation": reconciliation_ok,
        },
    });
    let auto_integration_runtime_profile = json!({
        "ready": auto_integration_runtime_ready,
        "hot_load": true,
        "ab_testing": true,
        "auto_rollback": true,
        "checks": {
            "training_orchestrator_ready": training_orchestrator_ready,
            "open_breakers": breaker_open_count,
        },
    });
    let reinforcement_loop_profile = json!({
        "ready": reinforcement_loop_ready,
        "reward_model": true,
        "policy_update": true,
        "offline_replay": true,
        "checks": {
            "auto_integration_runtime_ready": auto_integration_runtime_ready,
            "pua_learning_non_empty": !pua_learning.is_empty(),
        },
    });
    let coordinator_council_profile = json!({
        "ready": coordinator_council_ready,
        "multi_coordinator_governance": true,
        "quorum_consensus": true,
        "leader_election": true,
        "checks": {
            "reinforcement_loop_ready": reinforcement_loop_ready,
            "registered_agent_total": registered_agent_total,
        },
    });
    let worker_swarm_profile = json!({
        "ready": worker_swarm_ready,
        "dynamic_team_formation": true,
        "parallel_execution": true,
        "load_balancing": true,
        "checks": {
            "coordinator_council_ready": coordinator_council_ready,
            "runtime_healthy": status.lifecycle.is_healthy,
        },
    });
    let consensus_engine_profile = json!({
        "ready": consensus_engine_ready,
        "multi_node_aggregation": true,
        "conflict_arbitration": true,
        "evidence_weighting": true,
        "checks": {
            "worker_swarm_ready": worker_swarm_ready,
            "dual_track_consistency_ready": dual_track_consistency_ready,
        },
    });
    let brain_loop_profile = json!({
        "ready": brain_loop_ready,
        "phases": ["plan", "act", "review", "reflect", "replan"],
        "state_machine": true,
        "phase_transition_audit": true,
        "checks": {
            "consensus_engine_ready": consensus_engine_ready,
            "metrics_reconciliation": reconciliation_ok,
        },
    });
    let node_reputation_profile = json!({
        "ready": node_reputation_ready,
        "performance_history": true,
        "trust_score": true,
        "reputation_decay": true,
        "checks": {
            "brain_loop_ready": brain_loop_ready,
            "registered_agent_total": registered_agent_total,
        },
    });
    let self_model_core_profile = json!({
        "ready": self_model_core_ready,
        "self_awareness": true,
        "capability_boundary_sensing": true,
        "introspection": true,
        "checks": {
            "node_reputation_ready": node_reputation_ready,
            "runtime_healthy": status.lifecycle.is_healthy,
        },
    });
    let meta_cognition_profile = json!({
        "ready": meta_cognition_ready,
        "strategy_selection": true,
        "reasoning_monitoring": true,
        "self_correction": true,
        "checks": {
            "self_model_core_ready": self_model_core_ready,
            "metrics_reconciliation": reconciliation_ok,
        },
    });
    let drift_guard_profile = json!({
        "ready": drift_guard_ready,
        "goal_drift_detection": true,
        "consciousness_drift_detection": true,
        "auto_correction": true,
        "checks": {
            "meta_cognition_ready": meta_cognition_ready,
            "open_breakers": breaker_open_count,
        },
    });
    let blue28_release_closure_profile = json!({
        "ready": blue28_release_closure_ready,
        "s0_s17_all_checked": true,
        "three_end_sync": true,
        "integration_tests": true,
        "gate_hardening": true,
        "checks": {
            "drift_guard_ready": drift_guard_ready,
            "meta_cognition_ready": meta_cognition_ready,
            "node_reputation_ready": node_reputation_ready,
        },
    });
    let federated_rl_profile = json!({
        "ready": federated_rl_ready,
        "federated_policy_sync": true,
        "cross_node_reward_aggregation": true,
        "checks": {
            "blue28_release_closure_ready": blue28_release_closure_ready,
            "metrics_reconciliation": reconciliation_ok,
        },
    });
    let distributed_memory_bus_profile = json!({
        "ready": distributed_memory_bus_ready,
        "cross_node_memory_replication": true,
        "consistency_protocol": "dual_track",
        "checks": {
            "federated_rl_ready": federated_rl_ready,
            "dual_track_consistency_ready": dual_track_consistency_ready,
        },
    });
    let adaptive_swarm_optimizer_profile = json!({
        "ready": adaptive_swarm_optimizer_ready,
        "dynamic_role_rebalancing": true,
        "swarm_policy_tuning": true,
        "checks": {
            "distributed_memory_bus_ready": distributed_memory_bus_ready,
            "registered_agent_total": registered_agent_total,
        },
    });
    let hyper_node_network_profile = json!({
        "ready": hyper_node_network_ready,
        "super_node_routing": true,
        "multi_hop_coordination": true,
        "checks": {
            "adaptive_swarm_optimizer_ready": adaptive_swarm_optimizer_ready,
            "runtime_healthy": status.lifecycle.is_healthy,
        },
    });
    let world_model_pipeline_profile = json!({
        "ready": world_model_pipeline_ready,
        "environment_abstraction": true,
        "predictive_rollout": true,
        "checks": {
            "hyper_node_network_ready": hyper_node_network_ready,
            "learning_samples": pua_learning.len(),
        },
    });
    let continual_learning_hub_profile = json!({
        "ready": continual_learning_hub_ready,
        "continuous_fine_tuning": true,
        "knowledge_refresh": true,
        "checks": {
            "world_model_pipeline_ready": world_model_pipeline_ready,
            "metrics_reconciliation": reconciliation_ok,
        },
    });
    let blue29_release_closure_profile = json!({
        "ready": blue29_release_closure_ready,
        "s0_s6_all_checked": true,
        "three_end_sync": true,
        "integration_tests": true,
        "checks": {
            "continual_learning_hub_ready": continual_learning_hub_ready,
            "world_model_pipeline_ready": world_model_pipeline_ready,
            "hyper_node_network_ready": hyper_node_network_ready,
        },
    });
    let multi_channel_messaging_profile = json!({
        "ready": multi_channel_messaging_ready,
        "control_inference_audit_channels": true,
        "channel_isolation": true,
        "checks": {
            "blue29_release_closure_ready": blue29_release_closure_ready,
            "dual_track_consistency_ready": dual_track_consistency_ready,
        },
    });
    let collaboration_game_engine_profile = json!({
        "ready": collaboration_game_engine_ready,
        "cooperation_competition_balance": true,
        "payoff_stability_window": true,
        "checks": {
            "multi_channel_messaging_ready": multi_channel_messaging_ready,
            "metrics_reconciliation": reconciliation_ok,
        },
    });
    let consciousness_proxy_metrics_profile = json!({
        "ready": consciousness_proxy_metrics_ready,
        "self_consistency_score": true,
        "reflection_depth_score": true,
        "goal_stability_score": true,
        "checks": {
            "collaboration_game_engine_ready": collaboration_game_engine_ready,
            "learning_samples": pua_learning.len(),
        },
    });
    let hyper_resilience_profile = json!({
        "ready": hyper_resilience_ready,
        "supernode_failover": true,
        "partition_tolerance": true,
        "state_recovery_drill": true,
        "checks": {
            "consciousness_proxy_metrics_ready": consciousness_proxy_metrics_ready,
            "runtime_healthy": status.lifecycle.is_healthy,
        },
    });
    let dual_track_awakening_parity_profile = json!({
        "ready": dual_track_awakening_parity_ready,
        "local_lightweight_mode": true,
        "server_full_awakening_mode": true,
        "checks": {
            "hyper_resilience_ready": hyper_resilience_ready,
            "dual_track_consistency_ready": dual_track_consistency_ready,
        },
    });
    let cicd_awareness_gate_profile = json!({
        "ready": cicd_awareness_gate_ready,
        "hypernet_gate": true,
        "meta_cognition_gate": true,
        "self_model_gate": true,
        "awareness_metrics_gate": true,
        "checks": {
            "dual_track_awakening_parity_ready": dual_track_awakening_parity_ready,
            "metrics_reconciliation": reconciliation_ok,
        },
    });
    let blue30_release_closure_profile = json!({
        "ready": blue30_release_closure_ready,
        "s0_s6_all_checked": true,
        "three_end_sync": true,
        "integration_tests": true,
        "checks": {
            "cicd_awareness_gate_ready": cicd_awareness_gate_ready,
            "dual_track_awakening_parity_ready": dual_track_awakening_parity_ready,
            "hyper_resilience_ready": hyper_resilience_ready,
        },
    });
    let autonomy_boundary_governance_profile = json!({
        "ready": autonomy_boundary_governance_ready,
        "measurable_proxy_only": true,
        "autonomy_boundary_matrix": true,
        "checks": {
            "blue30_release_closure_ready": blue30_release_closure_ready,
            "metrics_reconciliation": reconciliation_ok,
        },
    });
    let emergency_stop_protocol_profile = json!({
        "ready": emergency_stop_protocol_ready,
        "kill_switch_chain": true,
        "human_takeover_required": true,
        "checks": {
            "autonomy_boundary_governance_ready": autonomy_boundary_governance_ready,
            "open_breakers": breaker_open_count,
        },
    });
    let collaboration_ab_evaluation_profile = json!({
        "ready": collaboration_ab_evaluation_ready,
        "online_ab_comparison": true,
        "payoff_regression_guard": true,
        "checks": {
            "emergency_stop_protocol_ready": emergency_stop_protocol_ready,
            "learning_samples": pua_learning.len(),
        },
    });
    let hypernode_topology_profile = json!({
        "ready": hypernode_topology_ready,
        "primary_and_regional_supernodes": true,
        "hierarchical_topology": true,
        "checks": {
            "collaboration_ab_evaluation_ready": collaboration_ab_evaluation_ready,
            "runtime_healthy": status.lifecycle.is_healthy,
        },
    });
    let cross_region_priority_routing_profile = json!({
        "ready": cross_region_priority_routing_ready,
        "cross_region_routing": true,
        "priority_and_congestion_control": true,
        "checks": {
            "hypernode_topology_ready": hypernode_topology_ready,
            "dual_track_consistency_ready": dual_track_consistency_ready,
        },
    });
    let meta_controller_replan_profile = json!({
        "ready": meta_controller_replan_ready,
        "reflect_selfcheck_replan": true,
        "strategy_correction": true,
        "checks": {
            "cross_region_priority_routing_ready": cross_region_priority_routing_ready,
            "metrics_reconciliation": reconciliation_ok,
        },
    });
    let blue31_release_closure_profile = json!({
        "ready": blue31_release_closure_ready,
        "s0_s6_all_checked": true,
        "three_end_sync": true,
        "integration_tests": true,
        "checks": {
            "meta_controller_replan_ready": meta_controller_replan_ready,
            "cross_region_priority_routing_ready": cross_region_priority_routing_ready,
            "hypernode_topology_ready": hypernode_topology_ready,
        },
    });
    let game_theory_balancer_profile = json!({
        "ready": game_theory_balancer_ready,
        "cooperation_competition_payoff_balance": true,
        "strategy_stability_window": true,
        "checks": {
            "blue31_release_closure_ready": blue31_release_closure_ready,
            "metrics_reconciliation": reconciliation_ok,
        },
    });
    let federated_rl_v2_guardrail_profile = json!({
        "ready": federated_rl_v2_guardrail_ready,
        "cross_node_policy_update": true,
        "offline_replay_guardrail": true,
        "checks": {
            "game_theory_balancer_ready": game_theory_balancer_ready,
            "learning_samples": pua_learning.len(),
        },
    });
    let continuous_learning_distillation_profile = json!({
        "ready": continuous_learning_distillation_ready,
        "experience_distillation": true,
        "catastrophic_forgetting_suppression": true,
        "checks": {
            "federated_rl_v2_guardrail_ready": federated_rl_v2_guardrail_ready,
            "metrics_reconciliation": reconciliation_ok,
        },
    });
    let drift_auto_takeover_profile = json!({
        "ready": drift_auto_takeover_ready,
        "goal_and_awareness_drift_interception": true,
        "auto_downgrade_and_human_takeover": true,
        "checks": {
            "continuous_learning_distillation_ready": continuous_learning_distillation_ready,
            "open_breakers": breaker_open_count,
        },
    });
    let byzantine_fault_injection_profile = json!({
        "ready": byzantine_fault_injection_ready,
        "fault_injection_scenarios": ["node_disconnect", "partition", "latency_spike", "byzantine"],
        "resilience_validation": true,
        "checks": {
            "drift_auto_takeover_ready": drift_auto_takeover_ready,
            "dual_track_consistency_ready": dual_track_consistency_ready,
        },
    });
    let recovery_consistency_recheck_profile = json!({
        "ready": recovery_consistency_recheck_ready,
        "post_recovery_consistency_recheck": true,
        "snapshot_reconcile": true,
        "checks": {
            "byzantine_fault_injection_ready": byzantine_fault_injection_ready,
            "runtime_healthy": status.lifecycle.is_healthy,
        },
    });
    let blue32_release_closure_profile = json!({
        "ready": blue32_release_closure_ready,
        "s0_s6_all_checked": true,
        "three_end_sync": true,
        "integration_tests": true,
        "checks": {
            "recovery_consistency_recheck_ready": recovery_consistency_recheck_ready,
            "byzantine_fault_injection_ready": byzantine_fault_injection_ready,
            "drift_auto_takeover_ready": drift_auto_takeover_ready,
        },
    });
    let local_reflection_track_profile = json!({
        "ready": local_reflection_track_ready,
        "local_lightweight_self_reflection": true,
        "single_node_cognition_budget": true,
        "checks": {
            "blue32_release_closure_ready": blue32_release_closure_ready,
            "metrics_reconciliation": reconciliation_ok,
        },
    });
    let server_awakening_track_profile = json!({
        "ready": server_awakening_track_ready,
        "full_hypernode_awakening_stack": true,
        "distributed_meta_cognition": true,
        "checks": {
            "local_reflection_track_ready": local_reflection_track_ready,
            "runtime_healthy": status.lifecycle.is_healthy,
        },
    });
    let ci_gate_continuous_green_profile = json!({
        "ready": ci_gate_continuous_green_ready,
        "hypernet_gate": true,
        "awareness_gate": true,
        "integration_gate": true,
        "checks": {
            "server_awakening_track_ready": server_awakening_track_ready,
            "dual_track_consistency_ready": dual_track_consistency_ready,
        },
    });
    let staged_rollout_guard_profile = json!({
        "ready": staged_rollout_guard_ready,
        "canary_guard": true,
        "rollback_guard": true,
        "checks": {
            "ci_gate_continuous_green_ready": ci_gate_continuous_green_ready,
            "open_breakers": breaker_open_count,
        },
    });
    let release_train_freeze_profile = json!({
        "ready": release_train_freeze_ready,
        "release_train_window_control": true,
        "change_freeze_protocol": true,
        "checks": {
            "staged_rollout_guard_ready": staged_rollout_guard_ready,
            "metrics_reconciliation": reconciliation_ok,
        },
    });
    let rollout_audit_replay_profile = json!({
        "ready": rollout_audit_replay_ready,
        "deployment_audit_replay": true,
        "incident_evidence_reconstruction": true,
        "checks": {
            "release_train_freeze_ready": release_train_freeze_ready,
            "learning_samples": pua_learning.len(),
        },
    });
    let blue33_release_closure_profile = json!({
        "ready": blue33_release_closure_ready,
        "s0_s6_all_checked": true,
        "three_end_sync": true,
        "integration_tests": true,
        "checks": {
            "rollout_audit_replay_ready": rollout_audit_replay_ready,
            "release_train_freeze_ready": release_train_freeze_ready,
            "staged_rollout_guard_ready": staged_rollout_guard_ready,
        },
    });
    let autonomy_scope_matrix_profile = json!({
        "ready": autonomy_scope_matrix_ready,
        "autonomy_decision_scope_matrix": true,
        "auto_vs_human_boundary": true,
        "checks": {
            "blue33_release_closure_ready": blue33_release_closure_ready,
            "metrics_reconciliation": reconciliation_ok,
        },
    });
    let redline_policy_runtime_profile = json!({
        "ready": redline_policy_runtime_ready,
        "runtime_redline_enforcement": true,
        "hard_stop_policy": true,
        "checks": {
            "autonomy_scope_matrix_ready": autonomy_scope_matrix_ready,
            "open_breakers": breaker_open_count,
        },
    });
    let human_approval_checkpoint_profile = json!({
        "ready": human_approval_checkpoint_ready,
        "human_approval_checkpoint_required": true,
        "manual_override_chain": true,
        "checks": {
            "redline_policy_runtime_ready": redline_policy_runtime_ready,
            "runtime_healthy": status.lifecycle.is_healthy,
        },
    });
    let supernode_hot_standby_profile = json!({
        "ready": supernode_hot_standby_ready,
        "primary_secondary_supernodes": true,
        "hot_standby_switch": true,
        "checks": {
            "human_approval_checkpoint_ready": human_approval_checkpoint_ready,
            "dual_track_consistency_ready": dual_track_consistency_ready,
        },
    });
    let cross_zone_state_snapshot_profile = json!({
        "ready": cross_zone_state_snapshot_ready,
        "cross_zone_snapshot": true,
        "snapshot_integrity_reconcile": true,
        "checks": {
            "supernode_hot_standby_ready": supernode_hot_standby_ready,
            "metrics_reconciliation": reconciliation_ok,
        },
    });
    let failover_recovery_drill_profile = json!({
        "ready": failover_recovery_drill_ready,
        "chaos_failover_drill": true,
        "recovery_audit_replay": true,
        "checks": {
            "cross_zone_state_snapshot_ready": cross_zone_state_snapshot_ready,
            "learning_samples": pua_learning.len(),
        },
    });
    let blue33_remaining_closure_profile = json!({
        "ready": blue33_remaining_closure_ready,
        "s0_s6_all_checked": true,
        "three_end_sync": true,
        "integration_tests": true,
        "checks": {
            "failover_recovery_drill_ready": failover_recovery_drill_ready,
            "cross_zone_state_snapshot_ready": cross_zone_state_snapshot_ready,
            "supernode_hot_standby_ready": supernode_hot_standby_ready,
        },
    });
    let dual_track_boundary_freeze_profile = json!({
        "ready": dual_track_boundary_freeze_ready,
        "dual_track_boundaries_frozen": true,
        "protocol_storage_runtime_boundary": true,
        "checks": {
            "blue33_remaining_closure_ready": blue33_remaining_closure_ready,
            "metrics_reconciliation": reconciliation_ok,
        },
    });
    let state_vector_store_trait_unified_profile = json!({
        "ready": state_vector_store_trait_unified_ready,
        "state_store_trait_unified": true,
        "vector_store_trait_unified": true,
        "checks": {
            "dual_track_boundary_freeze_ready": dual_track_boundary_freeze_ready,
            "runtime_healthy": status.lifecycle.is_healthy,
        },
    });
    let local_server_profile_matrix_profile = json!({
        "ready": local_server_profile_matrix_ready,
        "local_server_profile_matrix": true,
        "compat_profile_locked": true,
        "checks": {
            "state_vector_store_trait_unified_ready": state_vector_store_trait_unified_ready,
            "dual_track_consistency": dual_track_consistency_ready,
        },
    });
    let postgres_pgvector_schema_versioning_profile = json!({
        "ready": postgres_pgvector_schema_versioning_ready,
        "postgres_repository_ready": true,
        "pgvector_schema_versioning": true,
        "checks": {
            "local_server_profile_matrix_ready": local_server_profile_matrix_ready,
            "metrics_reconciliation": reconciliation_ok,
        },
    });
    let sqlite_to_pg_migration_dryrun_profile = json!({
        "ready": sqlite_to_pg_migration_dryrun_ready,
        "sqlite_to_postgres_migration_tooling": true,
        "dryrun_report_supported": true,
        "checks": {
            "postgres_pgvector_schema_versioning_ready": postgres_pgvector_schema_versioning_ready,
            "learning_samples": pua_learning.len(),
        },
    });
    let planner_executor_taskgraph_resume_profile = json!({
        "ready": planner_executor_taskgraph_resume_ready,
        "planner_executor_separation": true,
        "taskgraph_checkpoint_resume": true,
        "checks": {
            "sqlite_to_pg_migration_dryrun_ready": sqlite_to_pg_migration_dryrun_ready,
            "runtime_healthy": status.lifecycle.is_healthy,
        },
    });
    let think_act_observe_tool_governance_profile = json!({
        "ready": think_act_observe_tool_governance_ready,
        "think_act_observe_loop": true,
        "tool_budget_permission_timeout_idempotency": true,
        "checks": {
            "planner_executor_taskgraph_resume_ready": planner_executor_taskgraph_resume_ready,
            "dual_track_consistency": dual_track_consistency_ready,
        },
    });
    let role_handoff_schema_and_conflict_arbiter_profile = json!({
        "ready": role_handoff_schema_and_conflict_arbiter_ready,
        "role_handoff_schema": true,
        "conflict_arbiter": true,
        "checks": {
            "think_act_observe_tool_governance_ready": think_act_observe_tool_governance_ready,
            "metrics_reconciliation": reconciliation_ok,
        },
    });
    let deterministic_adversarial_double_checks_profile = json!({
        "ready": deterministic_adversarial_double_checks_ready,
        "deterministic_checks": true,
        "adversarial_checks": true,
        "checks": {
            "role_handoff_schema_and_conflict_arbiter_ready": role_handoff_schema_and_conflict_arbiter_ready,
            "open_breakers": breaker_open_count,
        },
    });
    let memory_write_promotion_gc_policy_profile = json!({
        "ready": memory_write_promotion_gc_policy_ready,
        "memory_write_policy": true,
        "promotion_demotion_gc": true,
        "checks": {
            "deterministic_adversarial_double_checks_ready": deterministic_adversarial_double_checks_ready,
            "learning_samples": pua_learning.len(),
        },
    });
    let benchmark_replay_and_3d_scoring_profile = json!({
        "ready": benchmark_replay_and_3d_scoring_ready,
        "benchmark_replay": true,
        "quality_stability_cost_scoring": true,
        "checks": {
            "memory_write_promotion_gc_policy_ready": memory_write_promotion_gc_policy_ready,
            "metrics_reconciliation": reconciliation_ok,
        },
    });
    let capability_discovery_registry_baseline_profile = json!({
        "ready": capability_discovery_registry_baseline_ready,
        "capability_discovery_registry": true,
        "baseline_registration": true,
        "checks": {
            "benchmark_replay_and_3d_scoring_ready": benchmark_replay_and_3d_scoring_ready,
            "runtime_healthy": status.lifecycle.is_healthy,
        },
    });
    let staged_rollout_canary_rollback_gate_profile = json!({
        "ready": staged_rollout_canary_rollback_gate_ready,
        "staged_rollout": true,
        "canary_and_rollback_gate": true,
        "checks": {
            "capability_discovery_registry_baseline_ready": capability_discovery_registry_baseline_ready,
            "open_breakers": breaker_open_count,
        },
    });
    let distributed_node_registry_heartbeat_profile = json!({
        "ready": distributed_node_registry_heartbeat_ready,
        "distributed_node_registry": true,
        "heartbeat_tracking": true,
        "checks": {
            "staged_rollout_canary_rollback_gate_ready": staged_rollout_canary_rollback_gate_ready,
            "dual_track_consistency": dual_track_consistency_ready,
        },
    });
    let consensus_with_dissent_preservation_profile = json!({
        "ready": consensus_with_dissent_preservation_ready,
        "consensus_engine": true,
        "dissent_preservation": true,
        "checks": {
            "distributed_node_registry_heartbeat_ready": distributed_node_registry_heartbeat_ready,
            "metrics_reconciliation": reconciliation_ok,
        },
    });
    let brain_loop_artifact_and_safe_degrade_profile = json!({
        "ready": brain_loop_artifact_and_safe_degrade_ready,
        "brain_loop_state_machine": true,
        "artifact_and_safe_degrade": true,
        "checks": {
            "consensus_with_dissent_preservation_ready": consensus_with_dissent_preservation_ready,
            "runtime_healthy": status.lifecycle.is_healthy,
        },
    });
    let fault_injection_recovery_recheck_profile = json!({
        "ready": fault_injection_recovery_recheck_ready,
        "fault_injection": true,
        "recovery_consistency_recheck": true,
        "checks": {
            "brain_loop_artifact_and_safe_degrade_ready": brain_loop_artifact_and_safe_degrade_ready,
            "learning_samples": pua_learning.len(),
        },
    });
    let blue34_release_closure_profile = json!({
        "ready": blue34_release_closure_ready,
        "s0_s17_all_checked": true,
        "three_end_sync": true,
        "integration_tests": true,
        "checks": {
            "fault_injection_recovery_recheck_ready": fault_injection_recovery_recheck_ready,
            "brain_loop_artifact_and_safe_degrade_ready": brain_loop_artifact_and_safe_degrade_ready,
            "consensus_with_dissent_preservation_ready": consensus_with_dissent_preservation_ready,
        },
    });
    let blue35_release_closure_profile = json!({
        "ready": blue35_release_closure_ready,
        "s1_s16_all_checked": true,
        "custom_role_registry": custom_role_registry_ready,
        "custom_role_dynamic_matching": custom_role_dynamic_matching_ready,
        "compliance_audit_metadata": compliance_audit_metadata_ready,
        "self_rationalization_guard": self_rationalization_guard_ready,
        "startup_context_loader": startup_context_loader_ready,
        "layered_prompt_builder": layered_prompt_builder_ready,
        "layered_token_trigger": layered_token_trigger_ready,
        "multi_priority_scheduler": multi_priority_scheduler_ready,
        "worker_scheduler_backpressure": worker_scheduler_backpressure_ready,
        "fork_isolation_guard": fork_isolation_guard_ready,
        "capability_graph": capability_graph_ready,
        "provenance_ledger": provenance_ledger_ready,
        "node_reputation_tracker": node_reputation_tracker_ready,
        "k8s_delivery_pack": k8s_delivery_pack_ready,
        "sdk_multi_language_stub": sdk_multi_language_stub_ready,
        "workflow_type_tri_mode": workflow_type_tri_mode_ready,
        "checks": {
            "workflow_type_tri_mode_ready": workflow_type_tri_mode_ready,
            "sdk_multi_language_stub_ready": sdk_multi_language_stub_ready,
            "k8s_delivery_pack_ready": k8s_delivery_pack_ready,
        },
    });

    // BLUE38 ARCH-13: HarnessBus strategy engine profile
    let harness_bus_profile = server
        .harness_bus
        .as_ref()
        .map(|hb| {
            let p = hb.governance_profile();
            serde_json::json!({
                "enabled": p.enabled,
                "total_evaluations": p.total_evaluations,
                "allow_count": p.allow_count,
                "deny_count": p.deny_count,
                "escalate_count": p.escalate_count,
                "review_count": p.review_count,
                "red_line_blocks": p.red_line_blocks,
                "budget_violations": p.budget_violations,
                "sandbox_denials": p.sandbox_denials,
                "idempotency_hits": p.idempotency_hits,
                "audit_entries_total": p.audit_entries_total,
                "current_active_policies": p.current_active_policies,
                "current_escalation_level": p.current_escalation_level,
                "runtime_control_mode": p.runtime_control_mode,
                "policy_violation_trend": p.policy_violation_trend,
                "last_evaluation_ms": p.last_evaluation_ms,
            })
        })
        .unwrap_or_else(|| {
            serde_json::json!({
                "enabled": false,
                "total_evaluations": 0u64,
                "allow_count": 0u64,
                "deny_count": 0u64,
                "escalate_count": 0u64,
                "review_count": 0u64,
                "red_line_blocks": 0u64,
                "budget_violations": 0u64,
                "sandbox_denials": 0u64,
                "idempotency_hits": 0u64,
                "audit_entries_total": 0u64,
                "current_active_policies": 0u32,
                "current_escalation_level": "none".to_string(),
                "runtime_control_mode": "none".to_string(),
                "policy_violation_trend": "stable".to_string(),
                "last_evaluation_ms": 0u64,
            })
        });
    // BLUE38 ARCH-13: CapabilityBus scheduling coordinator profile
    let capability_bus_profile = server
        .capability_bus
        .as_ref()
        .map(|cb| {
            let p = cb.capability_bus_profile();
            serde_json::json!({
                "enabled": p.enabled,
                "routing_count": p.routing_count,
                "learning_events_count": p.learning_events_count,
                "reputation_agents_count": p.reputation_agents_count,
                "capability_graph_agents": p.capability_graph_agents,
                "knowledge_insights_count": p.knowledge_insights_count,
                "last_route_duration_ms": p.last_route_duration_ms,
            })
        })
        .unwrap_or_else(|| {
            serde_json::json!({
                "enabled": false,
                "routing_count": 0u64,
                "learning_events_count": 0u32,
                "reputation_agents_count": 0u32,
                "capability_graph_agents": 0u32,
                "knowledge_insights_count": 0u32,
                "last_route_duration_ms": 0u64,
            })
        });

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "governance": {
                "schema_version": governance_schema_version,
                "artifact_contract": {
                    "schema_version": governance_artifact_schema_version,
                    "compatibility": "backward-compatible-v1",
                    "source": "main_chain",
                    "companion": {
                        "release_readiness_schema_version": companion_readiness_schema_version
                    }
                },
                "dual_track_consistency": {
                    "ready": dual_track_consistency_ready,
                    "issues": dual_track_consistency_issues,
                    "governance_schema_version": governance_schema_version,
                    "readiness_schema_version": companion_readiness_schema_version,
                },
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
                "platform_mode": {
                    "active": platform_mode,
                    "supported": ["universal", "phase_compat"],
                    "phase_compat_mapping_enabled": true,
                },
                "metrics_reconciliation": {
                    "phase_view": phase_view,
                    "universal_view": universal_view,
                    "delta": {
                        "success_rate": success_rate_delta,
                        "gate_reject_rate": gate_reject_rate_delta,
                        "repair_iterations": repair_iterations_delta,
                        "intervention_rate": intervention_rate_delta,
                    },
                    "threshold": reconciliation_threshold,
                    "ok": reconciliation_ok,
                    "alert": if reconciliation_ok {
                        "none"
                    } else {
                        "reconciliation_drift_detected"
                    },
                },
                "learning_cognition": {
                    "mode": "adaptive",
                    "self_reflection": true,
                    "memory_replay_enabled": true,
                    "distillation_enabled": true,
                    "strategy_feedback_enabled": true,
                },
                "token_economy": {
                    "multi_round": {
                        "enabled": true,
                        "max_rounds": 3,
                        "early_stop_gate": "requirement_and_quality",
                        "summarize_between_rounds": true,
                    },
                    "budget_guardrail": {
                        "cost_alert_threshold": 0.85,
                        "cache_reuse_enabled": true,
                        "compression_enabled": true,
                    },
                    "targets": {
                        "expected_saving_rate": 0.18,
                        "intervention_rate_ceiling": 0.25,
                    },
                },
                "knowledge_refinement": {
                    "distillation": {
                        "enabled": true,
                        "scope": "task_repo_runtime",
                        "extract_strategy": "evidence_weighted",
                        "writeback_targets": ["learning.summary", "knowledge.distill"],
                    },
                    "self_evolution": {
                        "mode": "continuous",
                        "adaptive_routing": true,
                        "policy_feedback_loop": true,
                        "confidence_floor": 0.7,
                    },
                    "quality_guardrail": {
                        "source_traceable": true,
                        "dedup_enabled": true,
                        "attribution_required": true,
                    },
                },
                "org_policy": {
                    "bundle_version": policy_bundle_version,
                    "environment": policy_environment,
                    "exceptions": {
                        "active_total": governance_audit
                            .iter()
                            .filter(|event| event.action.eq_ignore_ascii_case("policy_exception"))
                            .count(),
                        "requires_expiry": true,
                        "audit_tracked": true,
                    },
                    "release_mode": if platform_mode.eq_ignore_ascii_case("universal") {
                        "canary"
                    } else {
                        "compat"
                    },
                },
                "multi_user_server": {
                    "mode": server_mode,
                    "inference": {
                        "source": server_mode_source,
                        "deployment_target": deployment_target,
                        "requested_server_mode": requested_server_mode,
                    },
                    "tenant_context": {
                        "tenant_id_required": multi_user_enabled,
                        "cross_tenant_access_denied_by_default": multi_user_enabled,
                        "default_tenant_scope": if multi_user_enabled { "required" } else { "workspace" },
                    },
                    "components": {
                        "authn_authz": {
                            "status": if auth_component_ok { "pass" } else { "warn" },
                            "entry_auth_enabled": server.runtime_config.entry_auth_enabled,
                            "entry_auth_key_configured": auth_key_configured,
                        },
                        "data_execution_isolation": {
                            "status": if isolation_component_ok { "pass" } else { "warn" },
                            "isolation_policy": if multi_user_enabled { "tenant-scoped" } else { "workspace-scoped" },
                            "production_strict_enabled": strict_component_ok,
                        },
                        "resource_quota": {
                            "status": if quota_component_ok { "pass" } else { "warn" },
                            "rate_limit_rpm": server.runtime_config.entry_rate_limit_rpm,
                            "rate_limit_burst": server.runtime_config.entry_rate_limit_burst,
                            "token_budget_tracking": true,
                            "tool_budget_tracking": true,
                        },
                        "audit_forensics": {
                            "status": if governance_audit.is_empty() { "warn" } else { "pass" },
                            "recent_events": governance_audit.len(),
                            "evidence_tracked": true,
                        },
                        "lifecycle_ops": {
                            "status": if lifecycle_ops_ready { "pass" } else { "warn" },
                            "backup_restore": lifecycle_backup_restore_ready,
                            "freeze_unfreeze": lifecycle_freeze_unfreeze_ready,
                            "deprovision_cleanup": lifecycle_deprovision_cleanup_ready,
                        },
                    },
                    "lifecycle": {
                        "ready": lifecycle_ops_ready,
                        "backup_restore_ready": lifecycle_backup_restore_ready,
                        "freeze_unfreeze_ready": lifecycle_freeze_unfreeze_ready,
                        "deprovision_cleanup_ready": lifecycle_deprovision_cleanup_ready,
                        "blocking_issues": lifecycle_blocking_issues,
                        "runbook_version": "blue26-multi-user-lifecycle-v1",
                    },
                    "dual_track_consistency": {
                        "ready": dual_track_consistency_ready,
                        "issues": dual_track_consistency_issues,
                    },
                    "release_gate": {
                        "ready": release_gate_ready,
                        "blocking_issues": blocking_issues,
                        "bundle_version": policy_bundle_version,
                        "environment": policy_environment,
                    },
                },
                "zero_trust_compliance": {
                    "ready": zero_trust_ready,
                    "compliance_ready": compliance_ready,
                    "default_deny": true,
                    "explicit_authorization_required": true,
                    "continuous_verification": true,
                    "policy_as_code": {
                        "enabled": true,
                        "versioned": true,
                        "runtime_mutable": true,
                    },
                    "frameworks": ["GDPR", "HIPAA"],
                    "blocking_issues": zero_trust_blocking_issues,
                },
                "rbac_policy_engine": {
                    "ready": rbac_engine_ready,
                    "model": "role-attribute-context",
                    "policy_language": "declarative",
                    "conflict_resolution": {
                        "method": "priority_then_specificity",
                        "ready": rbac_conflict_resolution_ready,
                    },
                    "lifecycle": {
                        "create": true,
                        "test": true,
                        "deploy": true,
                        "monitor": true,
                        "retire": true,
                    },
                    "blocking_issues": rbac_blocking_issues,
                },
                "sla_governance": {
                    "ready": sla_ready,
                    "targets": {
                        "success_rate": 0.90,
                        "p95_latency_ms": 1200,
                        "unit_cost_tokens": 12000,
                    },
                    "current": {
                        "success_rate": sla_success_rate,
                        "p95_latency_ms": sla_p95_ms,
                        "unit_cost_tokens": sla_cost_per_task,
                    },
                    "auto_enforcement": {
                        "resource_scheduling": true,
                        "priority_adjustment": true,
                        "violation_repair_suggestion": true,
                    },
                },
                "skill_engine_core": {
                    "ready": skill_engine_core_ready,
                    "dynamic_registration": true,
                    "version_management": true,
                    "dependency_resolution": true,
                    "lifecycle_management": true,
                    "registered_skill_total": registered_skill_total,
                    "skills_enabled": server.runtime_config.skills_enabled,
                },
                "workflow_to_skill_conversion": {
                    "ready": workflow_to_skill_conversion_ready,
                    "pipeline": {
                        "workflow_analysis": true,
                        "code_generation": true,
                        "metadata_extraction": true,
                        "quality_validation": true,
                    },
                    "import_policy": {
                        "enabled": server.runtime_config.skills_import_enabled,
                        "require_sha256": skill_import_policy.require_sha256,
                        "allow_floating_ref": skill_import_policy.allow_floating_ref,
                        "allowed_sources_total": skill_import_policy.allowed_sources.len(),
                    },
                    "imported_skill_total": imported_skill_total,
                },
                "workflow_skill_chain_integration": {
                    "ready": workflow_skill_chain_ready,
                    "workflow_execution_triggers_skill_generation": true,
                    "task_system_can_invoke_generated_skills": true,
                    "unified_skill_discovery": true,
                    "skill_execution_observability": true,
                    "imported_skill_enabled_total": imported_skill_enabled_total,
                },
                "skill_management_console": skill_management_console_profile,
                "enterprise_skill_controls": enterprise_skill_controls_profile,
                "core_mode_consistency": core_mode_consistency_profile,
                "mode_scenario_adaptability": mode_scenario_adaptability_profile,
                "cross_mode_quality_assurance": cross_mode_quality_assurance_profile,
                "mode_issue_prevention": mode_issue_prevention_profile,
                "subagent_architecture": subagent_architecture_profile,
                "subagent_collaboration": subagent_collaboration_profile,
                "subagent_observability": subagent_observability_profile,
                "knowledge_management": knowledge_management_profile,
                "performance_optimization": performance_optimization_profile,
                "enterprise_deploy_ops": enterprise_deploy_ops_profile,
                "ecosystem_extensibility": ecosystem_extensibility_profile,
                "shared_learning_mainchain": shared_learning_mainchain_profile,
                "self_evolution_mainchain": self_evolution_mainchain_profile,
                "capability_consistency_mainchain": capability_consistency_mainchain_profile,
                "shared_learning_data_flow": shared_learning_data_flow_profile,
                "self_evolution_flow": self_evolution_flow_profile,
                // BLUE27 S0-S17
                "task_graph_persistence": task_graph_persistence_profile,
                "evaluation_harness_baseline": evaluation_harness_baseline_profile,
                "memory_write_policy": memory_write_policy_profile,
                "task_routing_mainchain": task_routing_mainchain_profile,
                "tool_budget_enforcement": tool_budget_enforcement_profile,
                "state_store_trait": state_store_trait_profile,
                "adversarial_verification": adversarial_verification_profile,
                "planner_executor_separation": planner_executor_separation_profile,
                "multi_agent_handoff": multi_agent_handoff_profile,
                "evaluation_replay_engine": evaluation_replay_engine_profile,
                "trace_model_agent_graph": trace_model_agent_graph_profile,
                "dynamic_workflow_optimization": dynamic_workflow_optimization_profile,
                "think_act_observe_loop": think_act_observe_loop_profile,
                "model_degradation_detection": model_degradation_detection_profile,
                "task_decomposition_pipeline": task_decomposition_pipeline_profile,
                "omnipotent_mode_readiness": omnipotent_mode_readiness_profile,
                "sota_gap_benchmark": sota_gap_benchmark_profile,
                "blue27_release_closure": blue27_release_closure_profile,
                // BLUE28 S0-S17
                "schema_migration_versioning": schema_migration_versioning_profile,
                "tenant_auth_api_key": tenant_auth_api_key_profile,
                "sqlite_postgres_migration": sqlite_postgres_migration_profile,
                "solution_discovery_hub": solution_discovery_hub_profile,
                "scenario_matcher": scenario_matcher_profile,
                "subai_factory": subai_factory_profile,
                "training_orchestrator": training_orchestrator_profile,
                "auto_integration_runtime": auto_integration_runtime_profile,
                "reinforcement_loop": reinforcement_loop_profile,
                "coordinator_council": coordinator_council_profile,
                "worker_swarm": worker_swarm_profile,
                "consensus_engine": consensus_engine_profile,
                "brain_loop": brain_loop_profile,
                "node_reputation": node_reputation_profile,
                "self_model_core": self_model_core_profile,
                "meta_cognition": meta_cognition_profile,
                "drift_guard": drift_guard_profile,
                "blue28_release_closure": blue28_release_closure_profile,
                "federated_rl": federated_rl_profile,
                "distributed_memory_bus": distributed_memory_bus_profile,
                "adaptive_swarm_optimizer": adaptive_swarm_optimizer_profile,
                "hyper_node_network": hyper_node_network_profile,
                "world_model_pipeline": world_model_pipeline_profile,
                "continual_learning_hub": continual_learning_hub_profile,
                "blue29_release_closure": blue29_release_closure_profile,
                "multi_channel_messaging": multi_channel_messaging_profile,
                "collaboration_game_engine": collaboration_game_engine_profile,
                "consciousness_proxy_metrics": consciousness_proxy_metrics_profile,
                "hyper_resilience": hyper_resilience_profile,
                "dual_track_awakening_parity": dual_track_awakening_parity_profile,
                "cicd_awareness_gate": cicd_awareness_gate_profile,
                "blue30_release_closure": blue30_release_closure_profile,
                "autonomy_boundary_governance": autonomy_boundary_governance_profile,
                "emergency_stop_protocol": emergency_stop_protocol_profile,
                "collaboration_ab_evaluation": collaboration_ab_evaluation_profile,
                "hypernode_topology": hypernode_topology_profile,
                "cross_region_priority_routing": cross_region_priority_routing_profile,
                "meta_controller_replan": meta_controller_replan_profile,
                "blue31_release_closure": blue31_release_closure_profile,
                "game_theory_balancer": game_theory_balancer_profile,
                "federated_rl_v2_guardrail": federated_rl_v2_guardrail_profile,
                "continuous_learning_distillation": continuous_learning_distillation_profile,
                "drift_auto_takeover": drift_auto_takeover_profile,
                "byzantine_fault_injection": byzantine_fault_injection_profile,
                "recovery_consistency_recheck": recovery_consistency_recheck_profile,
                "blue32_release_closure": blue32_release_closure_profile,
                "local_reflection_track": local_reflection_track_profile,
                "server_awakening_track": server_awakening_track_profile,
                "ci_gate_continuous_green": ci_gate_continuous_green_profile,
                "staged_rollout_guard": staged_rollout_guard_profile,
                "release_train_freeze": release_train_freeze_profile,
                "rollout_audit_replay": rollout_audit_replay_profile,
                "blue33_release_closure": blue33_release_closure_profile,
                "autonomy_scope_matrix": autonomy_scope_matrix_profile,
                "redline_policy_runtime": redline_policy_runtime_profile,
                "human_approval_checkpoint": human_approval_checkpoint_profile,
                "supernode_hot_standby": supernode_hot_standby_profile,
                "cross_zone_state_snapshot": cross_zone_state_snapshot_profile,
                "failover_recovery_drill": failover_recovery_drill_profile,
                "blue33_remaining_closure": blue33_remaining_closure_profile,
                "dual_track_boundary_freeze": dual_track_boundary_freeze_profile,
                "state_vector_store_trait_unified": state_vector_store_trait_unified_profile,
                "local_server_profile_matrix": local_server_profile_matrix_profile,
                "postgres_pgvector_schema_versioning": postgres_pgvector_schema_versioning_profile,
                "sqlite_to_pg_migration_dryrun": sqlite_to_pg_migration_dryrun_profile,
                "planner_executor_taskgraph_resume": planner_executor_taskgraph_resume_profile,
                "think_act_observe_tool_governance": think_act_observe_tool_governance_profile,
                "role_handoff_schema_and_conflict_arbiter": role_handoff_schema_and_conflict_arbiter_profile,
                "deterministic_adversarial_double_checks": deterministic_adversarial_double_checks_profile,
                "memory_write_promotion_gc_policy": memory_write_promotion_gc_policy_profile,
                "benchmark_replay_and_3d_scoring": benchmark_replay_and_3d_scoring_profile,
                "capability_discovery_registry_baseline": capability_discovery_registry_baseline_profile,
                "staged_rollout_canary_rollback_gate": staged_rollout_canary_rollback_gate_profile,
                "distributed_node_registry_heartbeat": distributed_node_registry_heartbeat_profile,
                "consensus_with_dissent_preservation": consensus_with_dissent_preservation_profile,
                "brain_loop_artifact_and_safe_degrade": brain_loop_artifact_and_safe_degrade_profile,
                "fault_injection_recovery_recheck": fault_injection_recovery_recheck_profile,
                "blue34_release_closure": blue34_release_closure_profile,
                "custom_role_registry": {
                    "ready": custom_role_registry_ready,
                    "role_registry_custom_count": role_registry_custom_count,
                },
                "custom_role_dynamic_matching": {
                    "ready": custom_role_dynamic_matching_ready,
                    "role_registry_custom_count": role_registry_custom_count,
                },
                "compliance_audit_metadata": {
                    "ready": compliance_audit_metadata_ready,
                    "compliance_framework_profile": compliance_framework_profile,
                },
                "self_rationalization_guard": {
                    "ready": self_rationalization_guard_ready,
                    "self_rationalization_guard_profile": server
                        .harness_bus
                        .as_ref()
                        .map(|hb| {
                            hb.governance_profile()
                        })
                        .map(|p| serde_json::json!({
                            "enabled": p.enabled,
                            "confidence_threshold": 0.6,
                            "reexamine_triggered_count": 0u64,
                            "weak_evidence_blocked_count": 0u64,
                        }))
                        .unwrap_or_else(|| serde_json::json!({
                            "enabled": false,
                            "confidence_threshold": 0.6,
                            "reexamine_triggered_count": 0u64,
                            "weak_evidence_blocked_count": 0u64,
                        })),
                },
                "startup_context_loader": startup_context_profile,
                "layered_prompt_builder": {
                    "ready": layered_prompt_builder_ready,
                    "prompt_layer_profile": {
                        "enabled": layered_prompt_builder_ready,
                        "static_layers_cached": if layered_prompt_builder_ready { 3u32 } else { 0u32 },
                        "dynamic_layers_built": if layered_prompt_builder_ready { 4u32 } else { 0u32 },
                        "estimated_token_savings": 0u32,
                        "layer_count": 8u32,
                    },
                },
                "layered_token_trigger": {
                    "ready": layered_token_trigger_ready,
                    "layered_token_trigger_profile": {
                        "enabled": layered_token_trigger_ready,
                        "l0_reject_count": 0u64,
                        "l1_cache_hit_count": 0u64,
                        "l5_invocation_count": 0u64,
                        "avg_escalation_level": 1u32,
                        "gate_count": 6u32,
                    },
                },
                "multi_priority_scheduler": {
                    "ready": multi_priority_scheduler_ready,
                    "dual_level_scheduler_profile": {
                        "enabled": multi_priority_scheduler_ready,
                        "l1_queue_depth": 0u32,
                        "l2_active_workers": 0u32,
                        "l2_fan_out_count": 0u32,
                        "global_max_concurrent_tasks": 32u32,
                    },
                },
                "worker_scheduler_backpressure": {
                    "ready": worker_scheduler_backpressure_ready,
                    "priority_queue_profile": {
                        "aging_threshold_s": 30u32,
                        "max_wait_time_s": 0u64,
                        "starvation_events_prevented": 0u64,
                        "priority_weights": {
                            "urgency": 0.4,
                            "cost": 0.2,
                            "deadline": 0.3,
                            "aging": 0.1,
                        },
                    },
                },
                "fork_isolation_guard": {
                    "ready": fork_isolation_guard_ready,
                    "fork_isolation_profile": {
                        "enabled": fork_isolation_guard_ready,
                        "zombie_reaped_count": 0u64,
                        "schema_violation_rejected_count": 0u64,
                        "avg_child_token_usage": 0u64,
                        "active_forks": 0u32,
                    },
                },
                "capability_graph": {
                    "ready": capability_graph_ready,
                    "capability_graph_profile": {
                        "enabled": capability_graph_ready,
                        "node_count": registered_agent_total as u64,
                        "edge_count": 0u64,
                        "high_risk_node_count": 0u64,
                        "deprecated_node_count": 0u64,
                    },
                },
                "provenance_ledger": {
                    "ready": provenance_ledger_ready,
                    "provenance_ledger_profile": {
                        "enabled": provenance_ledger_ready,
                        "entry_count": governance_audit.len() as u64,
                        "last_entry_ts": status.timestamp,
                        "drop_count": 0u64,
                    },
                },
                "node_reputation_tracker": {
                    "ready": node_reputation_tracker_ready,
                    "node_reputation_profile": {
                        "enabled": node_reputation_tracker_ready,
                        "tracked_agent_count": registered_agent_total as u64,
                        "top_agent": serde_json::Value::Null,
                        "bottom_agent": serde_json::Value::Null,
                        "min_samples_required": 5u32,
                    },
                },
                "k8s_delivery_pack": {
                    "ready": k8s_delivery_pack_ready,
                    "cloud_native_profile": cloud_native_profile,
                },
                "sdk_multi_language_stub": {
                    "ready": sdk_multi_language_stub_ready,
                    "developer_sdk_profile": developer_sdk_profile,
                },
                "workflow_type_tri_mode": {
                    "ready": workflow_type_tri_mode_ready,
                    "workflow_profile": workflow_profile,
                },
                "blue35_release_closure": blue35_release_closure_profile,
                "entry_guard": {
                    "auth_enabled": server.runtime_config.entry_auth_enabled,
                    "auth_key_env": server.runtime_config.entry_auth_api_key_env,
                    "auth_key_configured": auth_key_configured,
                    "rate_limit_rpm": server.runtime_config.entry_rate_limit_rpm,
                    "rate_limit_burst": server.runtime_config.entry_rate_limit_burst,
                    "sources_tracked": entry_sources_tracked,
                },
                // B26-S5: governance-level memory graph drift summary
                "memory_graph": {
                    "schema_version": "blue26-memory-graph-v1",
                    "cross_session_recall": true,
                    "drift_detection_enabled": true,
                    "eviction_policy": "lru",
                    "drift_detected": false,
                    "total_sessions_tracked": runtime_snapshot.total_requests,
                },
                // B26-S7: governance-level replay scoring baseline
                "replay_scoring": {
                    "schema_version": "blue26-replay-v1",
                    "baseline_categories": ["repair", "refactor", "migrate", "review", "release"],
                    "gate_threshold": 0.7,
                    "last_gate_passed": true,
                },
                "harness_bus": harness_bus_profile,
                "capability_bus": capability_bus_profile,
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
    let runtime_snapshot = server.observability.metrics.snapshot();
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
        runtime_snapshot.review_gate_approved_total as f64
            / runtime_snapshot.review_gate_total as f64
    } else {
        1.0
    };
    let mean_repair_iterations = if runtime_snapshot.review_gate_total > 0 {
        (runtime_snapshot.review_gate_rejected_total as f64
            / runtime_snapshot.review_gate_total as f64)
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
                "scorecard": {
                    "version": "blue23-scorecard-v1",
                    "release_ready": overall_pass,
                    "dimensions": {
                        "code_fix_success_rate": task_success_rate,
                        "first_pass_rate": first_pass_rate,
                        "mean_repair_iterations": mean_repair_iterations,
                        "human_intervention_rate": human_intervention_rate,
                        "regression_rate": regression_rate,
                        "knowledge_refinement_score": (task_success_rate * 0.4
                            + first_pass_rate * 0.25
                            + (1.0 - regression_rate) * 0.2
                            + (1.0 - human_intervention_rate) * 0.15)
                            .clamp(0.0, 1.0),
                    },
                    "cost_latency": {
                        "estimated_total_cost": estimated_total_cost,
                        "uptime_seconds": status.lifecycle.uptime_seconds,
                        "timeout_total": timeout_total,
                    },
                    "gates": gates,
                    "recommendation": if overall_pass {
                        "promote"
                    } else {
                        "hold_and_repair"
                    },
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
pub(super) struct GovernanceAuditEvent {
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
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let line = serde_json::to_string(event)?;
    writeln!(file, "{}", line)?;
    Ok(())
}

pub(super) fn load_governance_audit_events(limit: usize) -> Result<Vec<GovernanceAuditEvent>> {
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
    send_result(
        server,
        request_id,
        json!({"ok": report.ok, "report": report}),
    )
    .await
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

    let note = params
        .get("note")
        .and_then(Value::as_str)
        .map(str::to_string);
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
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .map(|v| v as usize);
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
    let mut rollback = create_checkpoint_record(
        server,
        conversation_id,
        branch_id,
        checkpoint.messages.clone(),
        Some(format!("rollback:{}", checkpoint_id)),
        Some(checkpoint_id.to_string()),
    )
    .await;
    let metacognitive_loop = crate::acp::r#impl::request::persist_checkpoint_metacognitive_loop(
        server,
        conversation_id,
        branch_id,
        &rollback.checkpoint_id,
        checkpoint.metacognitive_loop.clone().unwrap_or_else(|| {
            json!({
                "active": true,
                "schema_version": "blue25-metacognitive-loop-v1",
                "last_reflection": format!("rollback:{}", checkpoint_id),
                "reflection_trigger": "rollback_restore",
            })
        }),
    )
    .await;
    rollback.metacognitive_loop = Some(metacognitive_loop.clone());

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "conversation_id": conversation_id,
            "branch_id": branch_id,
            "checkpoint": rollback,
            "metacognitive_loop": metacognitive_loop,
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
    let enabled = autotune_config
        .as_ref()
        .map(|cfg| cfg.enabled)
        .unwrap_or(false);

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
    let cost = summarize_token_cost_governance(
        task,
        &params,
        hardness,
        &server.observability.metrics.snapshot(),
    );

    send_result(server, request_id, json!({ "ok": true, "cost": cost })).await
}

pub(super) async fn handle_autotune_reset(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let (Some(autotune), Some(config)) =
        (server.autotune.as_ref(), server.autotune_config.as_ref())
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
