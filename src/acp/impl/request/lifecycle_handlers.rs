//! Lifecycle handlers for ACP request processing.
//!
//! Extracted from runtime_pack.rs.  Provides handlers for runtime lifecycle
//! operations: shutdown, health probes, self-model introspection, provider
//! status, capability listing, feature flags and restart.

use super::*;

// ---------------------------------------------------------------------------
// handle_shutdown
// ---------------------------------------------------------------------------

/// Returns shutdown confirmation payload (pure, no send_result).
pub(super) fn shutdown_payload(server: &AcpServer) -> Result<Value> {
    info!("{}", t("info.shutdown_requested"));
    server.begin_shutdown();
    server.shutdown_notify.notify_waiters();

    Ok(json!({
        "ok": true,
        "shutdown": "initiated"
    }))
}

// ---------------------------------------------------------------------------
// handle_health
// ---------------------------------------------------------------------------

/// Build health status payload (pure, no send_result).
pub(super) async fn health_payload(server: &AcpServer) -> Result<Value> {
    let status = server.get_status();
    let metrics = server.observability.metrics.snapshot();

    // Snapshot token cache statistics for observability.
    let token_cache_stats = server.cache_deps.cache.token_cache.stats.read().await;
    let token_cache_report = token_cache_stats.to_json();

    // Module-level health profiles — read from harness_bus and capability_bus.
    let harness_profile = if let Some(hb) = server.governance_deps.harness_bus.as_ref() {
        json!({
            "enabled": true,
            "governance": hb.governance_profile(),
            "drift": hb.drift_profile(),
            "brain_loop": hb.brain_profile().await,
            "artifact": hb.artifact_profile(),
            "omnipotent": hb.omnipotent_profile(),
            "brain_runner": hb.brain_runner_profile().await,
            "resilience": hb.resilience_profile().await,
            "fault_tolerance": hb.fault_tolerance_profile().await,
        })
    } else {
        json!({"enabled": false})
    };

    let capability_profile = if let Some(cb) = server.governance_deps.capability_bus.as_ref() {
        let p = cb.capability_bus_profile().await;
        json!({
            "enabled": true,
            "profile": p,
        })
    } else {
        json!({"enabled": false})
    };

    let total = status.metrics.total_requests.max(1);
    let success_rate = (status.metrics.successful_requests as f64 / total as f64) * 100.0;
    let uptime_secs = status.lifecycle.uptime_seconds.max(1);
    let requests_per_minute = (status.metrics.total_requests as f64 / uptime_secs as f64) * 60.0;
    let review_timeout_compat = metrics.review_gate_timeout_total.max(
        metrics
            .review_gate_degraded_total
            .saturating_add(metrics.review_gate_rejected_total)
            .saturating_sub(metrics.review_gate_approved_total),
    );

    Ok(json!({
        "lifecycle": {
            "shutting_down": status.lifecycle.shutdown_requested,
            "is_healthy": status.lifecycle.is_healthy,
            "uptime_seconds": status.lifecycle.uptime_seconds,
            "version": env!("CARGO_PKG_VERSION"),
            "build": backend_build_label(),
        },
        "version": env!("CARGO_PKG_VERSION"),
        "stats": {
            "total_requests": status.metrics.total_requests,
            "successful_requests": status.metrics.successful_requests,
            "failed_requests": status.metrics.failed_requests,
            "requests_per_minute": (requests_per_minute * 100.0).round() / 100.0,
            "success_rate": (success_rate * 100.0).round() / 100.0,
            "avg_latency_ms": (status.metrics.avg_request_duration_ms * 100.0).round() / 100.0,
            "active_requests": status.metrics.active_requests,
        },
        "maintenance": status.maintenance,
        "review_gate": {
            "total": metrics.review_gate_total,
            "approved": metrics.review_gate_approved_total,
            "rejected": metrics.review_gate_rejected_total,
            "timeout": review_timeout_compat,
            "degraded": metrics.review_gate_degraded_total,
            "invalid_response": metrics.review_gate_invalid_response_total,
        },
        "timeouts": {
            "agent_request_total": metrics.agent_timeout_failures_total,
            "review_gate_total": review_timeout_compat,
            "runtime_probe_total": metrics.runtime_probe_timeout_total,
        },
        "token_cache": token_cache_report,
        "modules": {
            "harness_bus": harness_profile,
            "capability_bus": capability_profile,
        },
        "timestamp": status.timestamp,
    }))
}

// ---------------------------------------------------------------------------
// Helper: check_status_label
// ---------------------------------------------------------------------------

fn check_status_label(value: CheckStatus) -> &'static str {
    match value {
        CheckStatus::Healthy => "healthy",
        CheckStatus::Warn => "warn",
        CheckStatus::Error => "error",
        CheckStatus::Skipped => "skipped",
    }
}

// ---------------------------------------------------------------------------
// Helper: build_health_probes_payload
// ---------------------------------------------------------------------------

pub(super) async fn build_health_probes_payload(server: &AcpServer) -> Result<Value> {
    let status = server.get_status();
    let metrics = server.observability.metrics.snapshot();

    let config_path = server.config_path.as_deref().map(Path::new);
    let report = build_runtime_healthcheck_report(
        config_path,
        server.cache_deps.cache.response_cache.as_deref(),
        server.cache_deps.cache.vector_store.as_deref(),
    )
    .await?;

    let token_cache_stats = match server.cache_deps.cache.token_cache.stats.try_read() {
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

    let rate_limiter_buckets =
        with_acp_lock(server.resilience.phase_rate_limiter.as_ref(), |guard| {
            guard
                .snapshot()
                .into_iter()
                .map(|(phase, (tokens, capacity))| {
                    json!({
                        "phase": phase,
                        "tokens": tokens,
                        "capacity": capacity,
                        "used_percent": if capacity > 0.0 {
                            ((capacity - tokens) / capacity * 100.0).clamp(0.0, 100.0)
                        } else {
                            0.0
                        },
                    })
                })
                .collect::<Vec<_>>()
        });

    let lock_components: Vec<LockHealthSummary> = Vec::new();
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

// ---------------------------------------------------------------------------
// handle_capabilities_list
// ---------------------------------------------------------------------------

pub(super) async fn capabilities_list_payload(server: &AcpServer) -> Result<Value> {
    let capability_profile = if let Some(cb) = server.governance_deps.capability_bus.as_ref() {
        let p = cb.capability_bus_profile().await;
        serde_json::json!({
            "enabled": p.enabled,
            "routing_count": p.routing_count,
            "capability_graph_agents": p.capability_graph_agents,
            "knowledge_insights_count": p.knowledge_insights_count,
            "workflow_presets_count": p.workflow_presets_count,
            "provenance_entries_count": p.provenance_entries_count,
        })
    } else {
        serde_json::json!(null)
    };

    Ok(serde_json::json!({
        "capabilities": capability_profile,
    }))
}

// ---------------------------------------------------------------------------
// Helper: backend_build_label
// ---------------------------------------------------------------------------

fn backend_build_label() -> String {
    if let Some(sha) = option_env!("VERGEN_GIT_SHA").filter(|sha| !sha.is_empty()) {
        return sha.chars().take(12).collect();
    }

    if cfg!(debug_assertions) {
        format!("debug ({})", env!("CARGO_PKG_VERSION"))
    } else {
        format!("release ({})", env!("CARGO_PKG_VERSION"))
    }
}

// ---------------------------------------------------------------------------
// handle_health_probes
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Helper: build_runtime_stability_payload
// ---------------------------------------------------------------------------

pub(super) async fn build_runtime_stability_payload(server: &AcpServer) -> Result<Value> {
    let status = server.get_status();
    // Snapshot metrics for inclusion in the stability payload below
    let metrics = server.observability.metrics.snapshot();
    // Record snapshot timestamp in the payload if metrics are available
    let _ = metrics;
    let config_path = server.config_path.as_deref().map(Path::new);
    let report = build_runtime_healthcheck_report(
        config_path,
        server.cache_deps.cache.response_cache.as_deref(),
        server.cache_deps.cache.vector_store.as_deref(),
    )
    .await?;

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

// ---------------------------------------------------------------------------
// handle_runtime_stability
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Helper: build_runtime_self_model_payload
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Helper: build_runtime_self_model_payload
// ---------------------------------------------------------------------------

pub(super) async fn build_runtime_self_model_payload(
    server: &AcpServer,
    params: &Value,
) -> Result<Value> {
    let probes_payload = build_health_probes_payload(server).await?;
    let stability_payload = build_runtime_stability_payload(server).await?;
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

    // Augment with learning and knowledge_refinement profiles from governance_pack
    let task = params.get("task").and_then(Value::as_str).unwrap_or("");
    let learning_profile = build_learning_profile("runtime.self_model", task, params);
    let knowledge_refinement =
        build_knowledge_refinement_profile("runtime.self_model", task, params, &learning_profile);

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
            "learning": learning_profile,
            "knowledge_refinement": knowledge_refinement,
        }
    }))
}

// ---------------------------------------------------------------------------
// Helper: build_provider_status_payload
// ---------------------------------------------------------------------------

pub(super) async fn build_provider_status_payload(server: &AcpServer) -> Result<Value> {
    let status = server.get_status();
    let config_path = server.config_path.as_deref().map(Path::new);
    let report = build_runtime_healthcheck_report(
        config_path,
        server.cache_deps.cache.response_cache.as_deref(),
        server.cache_deps.cache.vector_store.as_deref(),
    )
    .await?;

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

// ---------------------------------------------------------------------------
// handle_provider_status
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// handle_runtime_features
// ---------------------------------------------------------------------------

pub(super) fn runtime_features_payload(server: &AcpServer) -> Result<Value> {
    Ok(json!({
        "ok": true,
        "features": {
            "harness_bus": server.governance_deps.harness_bus.is_some(),
            "capability_bus": server.governance_deps.capability_bus.is_some(),
            "vector_store": server.cache_deps.cache.vector_store.is_some(),
            "response_cache": server.cache_deps.cache.response_cache.is_some(),
            "autotune": server.cache_deps.autotune.is_some(),
            "skills_enabled": server.runtime_config.skills_enabled,
            "skills_import": server.runtime_config.skills_import_enabled,
            "entry_auth": server.runtime_config.entry_auth_enabled,
            "otel": server.runtime_config.otel_enabled,
            "production_strict": server.runtime_config.production_strict,
        }
    }))
}

// ---------------------------------------------------------------------------
// handle_runtime_restart
// ---------------------------------------------------------------------------

/// Handle runtime restart request from GUI or other clients.
/// Initiates a graceful shutdown so the process manager can restart the service.
pub(super) fn runtime_restart_payload(server: &AcpServer) -> Result<Value> {
    info!("{}", t("info.restart_requested"));

    server.begin_shutdown();
    server.shutdown_notify.notify_waiters();

    Ok(json!({
        "ok": true,
        "restart": "initiated",
        "message": t("info.restart_message"),
    }))
}
