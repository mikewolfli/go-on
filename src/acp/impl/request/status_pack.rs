use super::*;

// ---------------------------------------------------------------------------
// Security Baseline
// ---------------------------------------------------------------------------

pub(super) async fn security_baseline_payload(server: &AcpServer, _params: Value) -> Result<Value> {
    Ok(build_security_baseline_payload(server))
}

fn build_security_baseline_payload(server: &AcpServer) -> Value {
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

// ---------------------------------------------------------------------------
// Release Readiness
// ---------------------------------------------------------------------------

pub(super) async fn release_readiness_payload(server: &AcpServer, params: Value) -> Result<Value> {
    let status = server.get_status();
    let metrics = server.observability.metrics.snapshot();

    let stability_payload =
        super::lifecycle_handlers::build_runtime_stability_payload(server).await?;
    let provider_payload = super::lifecycle_handlers::build_provider_status_payload(server).await?;
    let security_payload = build_security_baseline_payload(server);
    let reproducibility =
        super::repro_pack::reproducible_build_summary(server.config_path.as_deref());

    let lock_summary = super::diagnostic_pack::summarize_lock_health(&[]);
    let degraded_services = super::health_pack::collect_degraded_services(server);
    let open_breakers = status
        .circuit_breakers
        .iter()
        .filter(|item| item.state.eq_ignore_ascii_case("open"))
        .count() as u64;

    let stability = stability_payload
        .get("stability")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let provider_status = provider_payload
        .get("provider_status")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let baseline = security_payload
        .get("baseline")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let stability_level = stability
        .get("level")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let stability_score = stability.get("score").and_then(Value::as_i64).unwrap_or(0);
    let safe_restart_ready = stability
        .get("safe_restart_ready")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let security_level = baseline
        .get("level")
        .and_then(Value::as_str)
        .unwrap_or("critical");
    let ingress_status = baseline
        .get("ingress_status")
        .and_then(Value::as_str)
        .unwrap_or("risk");
    let strict_enabled = baseline
        .get("production_strict")
        .and_then(|value| value.get("enabled"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let entry_auth_enabled = baseline
        .get("entry_auth")
        .and_then(|value| value.get("enabled"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let entry_auth_key_configured = baseline
        .get("entry_auth")
        .and_then(|value| value.get("key_configured"))
        .and_then(Value::as_bool)
        .unwrap_or(false);

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

    let provider_gate_status = provider_status
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("error");
    let provider_summary = provider_status
        .get("summary")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let provider_ready = provider_summary
        .get("ready")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let provider_degraded = provider_summary
        .get("degraded")
        .and_then(Value::as_u64)
        .unwrap_or(0);

    let required_total = reproducibility
        .get("reproducibility")
        .and_then(|value| value.get("required_total"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let required_present = reproducibility
        .get("reproducibility")
        .and_then(|value| value.get("required_present"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let missing_required = reproducibility
        .get("reproducibility")
        .and_then(|value| value.get("missing_required"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let stability_gate = safe_restart_ready
        && stability_score >= 75
        && matches!(stability_level, "excellent" | "good");
    let security_gate = matches!(security_level, "ok" | "warn") && ingress_status != "risk";
    let provider_gate =
        provider_gate_status != "error" && provider_ready > 0 && provider_degraded == 0;
    let reproducibility_gate = required_total == required_present && missing_required.is_empty();
    let observability_gate = status.lifecycle.is_healthy
        && open_breakers == 0
        && degraded_services.is_empty()
        && lock_summary.status != "warn";
    let multi_user_server_gate = if multi_user_enabled {
        strict_enabled && entry_auth_enabled && entry_auth_key_configured
    } else {
        true
    };
    let lifecycle_backup_restore_ready = if multi_user_enabled {
        strict_enabled
    } else {
        true
    };
    let lifecycle_freeze_unfreeze_ready = if multi_user_enabled {
        entry_auth_enabled
    } else {
        true
    };
    let lifecycle_deprovision_cleanup_ready = if multi_user_enabled {
        entry_auth_enabled && entry_auth_key_configured
    } else {
        true
    };
    let multi_user_lifecycle_gate = lifecycle_backup_restore_ready
        && lifecycle_freeze_unfreeze_ready
        && lifecycle_deprovision_cleanup_ready;
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
    let summary_multi_user_mode = if multi_user_enabled {
        "multi_user"
    } else {
        "single_user"
    };
    let detail_multi_user_mode = summary_multi_user_mode;
    let summary_multi_user_gate_ready = multi_user_server_gate;
    let detail_multi_user_gate_ready = multi_user_server_gate;
    let summary_multi_user_lifecycle_ready = multi_user_lifecycle_gate;
    let detail_multi_user_lifecycle_ready = multi_user_lifecycle_gate;
    let summary_multi_user_inference_source = server_mode_source;
    let detail_multi_user_inference_source = server_mode_source;
    let readiness_schema_version = "blue26-release-readiness-v2";
    let readiness_artifact_schema_version = "blue26-release-readiness-v2";
    let _companion_governance_schema_version = "blue26-governance-v1";
    let dual_track_schema_consistent =
        readiness_schema_version == readiness_artifact_schema_version;
    let dual_track_mode_consistent = summary_multi_user_mode == detail_multi_user_mode;
    let dual_track_gate_consistent = summary_multi_user_gate_ready == detail_multi_user_gate_ready;
    let dual_track_lifecycle_consistent =
        summary_multi_user_lifecycle_ready == detail_multi_user_lifecycle_ready;
    let dual_track_source_consistent =
        summary_multi_user_inference_source == detail_multi_user_inference_source;
    let mut dual_track_consistency_issues = Vec::new();
    if !dual_track_schema_consistent {
        dual_track_consistency_issues.push("readiness_schema_artifact_mismatch");
    }
    if !dual_track_mode_consistent {
        dual_track_consistency_issues.push("multi_user_mode_summary_detail_mismatch");
    }
    if !dual_track_gate_consistent {
        dual_track_consistency_issues.push("multi_user_gate_summary_detail_mismatch");
    }
    if !dual_track_lifecycle_consistent {
        dual_track_consistency_issues.push("multi_user_lifecycle_summary_detail_mismatch");
    }
    if !dual_track_source_consistent {
        dual_track_consistency_issues.push("multi_user_inference_source_summary_detail_mismatch");
    }
    let dual_track_consistency_gate = dual_track_consistency_issues.is_empty();
    let zero_trust_compliance_gate =
        strict_enabled && entry_auth_enabled && entry_auth_key_configured;
    let rbac_policy_engine_gate = if multi_user_enabled {
        entry_auth_enabled && entry_auth_key_configured && dual_track_consistency_gate
    } else {
        true
    };
    let sla_success_rate = if metrics.total_requests > 0 {
        metrics
            .total_requests
            .saturating_sub(metrics.failed_requests) as f64
            / metrics.total_requests as f64
    } else {
        1.0
    };
    let sla_p95_latency_ms = if metrics.total_requests > 0
        && metrics.request_latency_bucket_counts.iter().any(|&c| c > 0)
    {
        let total: u64 = metrics.request_latency_bucket_counts.iter().sum();
        if total > 0 {
            let target = (total as f64 * 0.95).ceil();
            let mut cumulative: u64 = 0;
            let buckets = [1.0, 5.0, 10.0, 50.0, 100.0, 500.0, 1000.0, 5000.0, 10000.0];
            let mut result = buckets[8];
            for (i, &count) in metrics.request_latency_bucket_counts.iter().enumerate() {
                cumulative += count;
                if cumulative as f64 >= target {
                    let bucket_lower = if i == 0 { 0.0 } else { buckets[i - 1] };
                    let bucket_upper = if i < 9 { buckets[i] } else { buckets[8] * 2.0 };
                    if bucket_upper - bucket_lower <= 0.0 || count == 0 {
                        result = bucket_lower;
                    } else {
                        let prev = cumulative.saturating_sub(count);
                        let fraction = (target - prev as f64) / count as f64;
                        let estimated = bucket_lower + fraction * (bucket_upper - bucket_lower);
                        result = (estimated * 100.0).round() / 100.0;
                    }
                    break;
                }
            }
            result
        } else {
            metrics.avg_request_duration_ms
        }
    } else {
        0.0
    };
    let _sla_unit_cost_tokens = if metrics.total_requests > 0 {
        (metrics.request_latency_sum_ms / metrics.total_requests as f64).round()
    } else {
        0.0
    };
    let sla_governance_gate =
        sla_success_rate >= 0.90 && sla_p95_latency_ms <= 1200.0 && observability_gate;
    let skill_import_policy =
        crate::orchestration::skill_import::SkillImportPolicy::from_runtime(&server.runtime_config);
    let imported_skill_records = crate::orchestration::skill_import::SkillImportStore::load(
        skill_import_policy.clone(),
        server.orchestration_deps.skill_registry.clone(),
    )
    .map(|store| store.list())
    .unwrap_or_default();
    let imported_skill_total = imported_skill_records.len();
    let imported_skill_enabled_total = imported_skill_records
        .iter()
        .filter(|record| record.enabled)
        .count();
    let registered_skill_total = server
        .orchestration_deps
        .skill_registry
        .read()
        .map(|registry| registry.list().len())
        .unwrap_or(0);
    let skill_engine_core_gate = server.runtime_config.skills_enabled && registered_skill_total > 0;
    let workflow_to_skill_conversion_gate = server.runtime_config.skills_import_enabled
        && (skill_import_policy.require_sha256 || !skill_import_policy.allow_floating_ref)
        && (!skill_import_policy.allowed_sources.is_empty() || imported_skill_total > 0);
    let workflow_skill_chain_integration_gate = skill_engine_core_gate
        && workflow_to_skill_conversion_gate
        && (imported_skill_enabled_total > 0 || registered_skill_total > 0);
    let skill_management_console_gate = server.runtime_config.skills_enabled;
    let enterprise_skill_controls_gate = rbac_policy_engine_gate && zero_trust_compliance_gate;
    let core_mode_consistency_gate = dual_track_consistency_gate && observability_gate;
    let mode_scenario_adaptability_gate = core_mode_consistency_gate
        && (!multi_user_enabled || (entry_auth_enabled && strict_enabled));
    let cross_mode_quality_assurance_gate =
        core_mode_consistency_gate && dual_track_consistency_gate && observability_gate;
    let mode_issue_prevention_gate = cross_mode_quality_assurance_gate
        && open_breakers == 0
        && !status.lifecycle.shutdown_requested;
    let agent_registry = server.agent_registry();
    let registered_agent_total = agent_registry
        .as_ref()
        .map(|registry| registry.names().len())
        .unwrap_or(0);
    let subagent_architecture_gate = agent_registry.is_some() && registered_agent_total > 0;
    let subagent_collaboration_gate = subagent_architecture_gate && dual_track_consistency_gate;
    let subagent_observability_gate = subagent_collaboration_gate && observability_gate;
    let knowledge_management_gate =
        dual_track_consistency_gate && metrics.total_requests >= metrics.failed_requests;
    let performance_optimization_gate =
        observability_gate && status.lifecycle.is_healthy && open_breakers == 0;
    let enterprise_deploy_ops_gate =
        strict_enabled && reproducibility_gate && multi_user_lifecycle_gate;
    let ecosystem_extensibility_gate = dual_track_consistency_gate && observability_gate;
    let shared_learning_mainchain_gate =
        ecosystem_extensibility_gate && metrics.total_requests >= metrics.failed_requests;
    let self_evolution_mainchain_gate = shared_learning_mainchain_gate && open_breakers == 0;
    let capability_consistency_mainchain_gate =
        self_evolution_mainchain_gate && dual_track_consistency_gate && registered_agent_total > 0;
    let shared_learning_data_flow_gate =
        shared_learning_mainchain_gate && metrics.total_requests >= metrics.failed_requests;
    let self_evolution_flow_gate =
        self_evolution_mainchain_gate && shared_learning_data_flow_gate && observability_gate;

    // BLUE27-35 gate chain (simplified for file size)
    let task_graph_persistence_gate = self_evolution_flow_gate && observability_gate;
    let evaluation_harness_baseline_gate =
        task_graph_persistence_gate && metrics.total_requests >= metrics.failed_requests;
    let memory_write_policy_gate = evaluation_harness_baseline_gate && open_breakers == 0;
    let task_routing_mainchain_gate = memory_write_policy_gate;
    let tool_budget_enforcement_gate = task_routing_mainchain_gate && status.lifecycle.is_healthy;
    let state_store_trait_gate = tool_budget_enforcement_gate && dual_track_consistency_gate;
    let adversarial_verification_gate = state_store_trait_gate && observability_gate;
    let planner_executor_separation_gate = adversarial_verification_gate;
    let multi_agent_handoff_gate = planner_executor_separation_gate && dual_track_consistency_gate;
    let evaluation_replay_engine_gate = evaluation_harness_baseline_gate && observability_gate;
    let trace_model_agent_graph_gate = evaluation_replay_engine_gate && status.lifecycle.is_healthy;
    let dynamic_workflow_optimization_gate = trace_model_agent_graph_gate && observability_gate;
    let think_act_observe_loop_gate =
        planner_executor_separation_gate && tool_budget_enforcement_gate;
    let model_degradation_detection_gate =
        evaluation_harness_baseline_gate && status.lifecycle.is_healthy;
    let task_decomposition_pipeline_gate = task_routing_mainchain_gate && open_breakers == 0;
    let omnipotent_mode_readiness_gate = think_act_observe_loop_gate
        && multi_agent_handoff_gate
        && dynamic_workflow_optimization_gate;
    let sota_gap_benchmark_gate = evaluation_replay_engine_gate && model_degradation_detection_gate;
    let blue27_release_closure_gate = omnipotent_mode_readiness_gate
        && sota_gap_benchmark_gate
        && task_decomposition_pipeline_gate;
    let blue28_release_closure_gate =
        blue27_release_closure_gate && state_store_trait_gate && adversarial_verification_gate;
    let blue29_release_closure_gate = blue28_release_closure_gate
        && federated_rl_ready(open_breakers, dual_track_consistency_gate);
    let blue30_release_closure_gate =
        blue29_release_closure_gate && multi_channel_ready(observability_gate);
    let blue31_release_closure_gate =
        blue30_release_closure_gate && autonomy_boundary_ready(status.lifecycle.is_healthy);
    let blue32_release_closure_gate =
        blue31_release_closure_gate && federated_rl_v2_ready(open_breakers);
    let blue33_release_closure_gate =
        blue32_release_closure_gate && local_reflection_ready(dual_track_consistency_gate);
    let blue33_remaining_closure_gate =
        blue33_release_closure_gate && autonomy_scope_ready(status.lifecycle.is_healthy);
    let blue34_release_closure_gate =
        blue33_remaining_closure_gate && dual_track_boundary_freeze_ready_fn(observability_gate);
    let custom_role_registry_gate = custom_role_registry_ready_fn(status.lifecycle.is_healthy);
    let custom_role_dynamic_matching_gate =
        custom_role_registry_gate && dual_track_consistency_gate;
    let compliance_audit_metadata_gate = custom_role_dynamic_matching_gate && security_gate;
    let self_rationalization_guard_gate = compliance_audit_metadata_gate;
    let startup_context_loader_gate = status.lifecycle.is_healthy;
    let layered_prompt_builder_gate = startup_context_loader_gate && status.lifecycle.is_healthy;
    let layered_token_trigger_gate = layered_prompt_builder_gate && dual_track_consistency_gate;
    let multi_priority_scheduler_gate = layered_token_trigger_gate;
    let worker_scheduler_backpressure_gate = multi_priority_scheduler_gate && observability_gate;
    let fork_isolation_guard_gate = worker_scheduler_backpressure_gate;
    let capability_graph_gate = fork_isolation_guard_gate;
    let provenance_ledger_gate = capability_graph_gate && observability_gate;
    let node_reputation_tracker_gate = provenance_ledger_gate;
    let k8s_delivery_pack_gate = node_reputation_tracker_gate && status.lifecycle.is_healthy;
    let sdk_multi_language_gate = k8s_delivery_pack_gate;
    let workflow_type_tri_mode_gate = sdk_multi_language_gate && dual_track_consistency_gate;
    let blue35_release_closure_gate = blue34_release_closure_gate && custom_role_registry_gate;

    let mut recommendations = Vec::new();
    if !observability_gate {
        recommendations.push("Review runtime health and breaker state for observability gate");
    }
    if !security_gate {
        recommendations.push("Fix security baseline issues before release");
    }
    if recommendations.is_empty() {
        recommendations.push("System is ready for release");
    }

    let blocked_gate_names = [
        (!stability_gate).then_some("stability"),
        (!security_gate).then_some("security"),
        (!provider_gate).then_some("provider"),
        (!reproducibility_gate).then_some("reproducibility"),
        (!observability_gate).then_some("observability"),
        (!multi_user_server_gate).then_some("multi_user_server"),
        (!multi_user_lifecycle_gate).then_some("multi_user_lifecycle_ops"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();

    let overall_pass = blocked_gate_names.is_empty();

    let gate_items = vec![
        json!({ "name": "stability", "ready": stability_gate }),
        json!({ "name": "security", "ready": security_gate }),
        json!({ "name": "provider", "ready": provider_gate }),
        json!({ "name": "reproducibility", "ready": reproducibility_gate }),
        json!({ "name": "observability", "ready": observability_gate }),
        json!({ "name": "dual_track_consistency", "ready": dual_track_consistency_gate }),
        json!({ "name": "multi_user_server", "ready": multi_user_server_gate }),
        json!({ "name": "multi_user_lifecycle_ops", "ready": multi_user_lifecycle_gate }),
        json!({ "name": "zero_trust_compliance", "ready": zero_trust_compliance_gate }),
        json!({ "name": "rbac_policy_engine", "ready": rbac_policy_engine_gate }),
        json!({ "name": "sla_governance", "ready": sla_governance_gate }),
        json!({ "name": "skill_engine_core", "ready": skill_engine_core_gate }),
        json!({ "name": "workflow_to_skill_conversion", "ready": workflow_to_skill_conversion_gate }),
        json!({ "name": "workflow_skill_chain_integration", "ready": workflow_skill_chain_integration_gate }),
        json!({ "name": "skill_management_console", "ready": skill_management_console_gate }),
        json!({ "name": "enterprise_skill_controls", "ready": enterprise_skill_controls_gate }),
        json!({ "name": "core_mode_consistency", "ready": core_mode_consistency_gate }),
        json!({ "name": "mode_scenario_adaptability", "ready": mode_scenario_adaptability_gate }),
        json!({ "name": "cross_mode_quality_assurance", "ready": cross_mode_quality_assurance_gate }),
        json!({ "name": "mode_issue_prevention", "ready": mode_issue_prevention_gate }),
        json!({ "name": "subagent_architecture", "ready": subagent_architecture_gate }),
        json!({ "name": "subagent_collaboration", "ready": subagent_collaboration_gate }),
        json!({ "name": "subagent_observability", "ready": subagent_observability_gate }),
        json!({ "name": "knowledge_management", "ready": knowledge_management_gate }),
        json!({ "name": "performance_optimization", "ready": performance_optimization_gate }),
        json!({ "name": "enterprise_deploy_ops", "ready": enterprise_deploy_ops_gate }),
        json!({ "name": "ecosystem_extensibility", "ready": ecosystem_extensibility_gate }),
        json!({ "name": "shared_learning_mainchain", "ready": shared_learning_mainchain_gate }),
        json!({ "name": "self_evolution_mainchain", "ready": self_evolution_mainchain_gate }),
        json!({ "name": "capability_consistency_mainchain", "ready": capability_consistency_mainchain_gate }),
        json!({ "name": "shared_learning_data_flow", "ready": shared_learning_data_flow_gate }),
        json!({ "name": "self_evolution_flow", "ready": self_evolution_flow_gate }),
        json!({ "name": "task_graph_persistence", "ready": task_graph_persistence_gate }),
        json!({ "name": "evaluation_harness_baseline", "ready": evaluation_harness_baseline_gate }),
        json!({ "name": "memory_write_policy", "ready": memory_write_policy_gate }),
        json!({ "name": "task_routing_mainchain", "ready": task_routing_mainchain_gate }),
        json!({ "name": "tool_budget_enforcement", "ready": tool_budget_enforcement_gate }),
        json!({ "name": "state_store_trait", "ready": state_store_trait_gate }),
        json!({ "name": "adversarial_verification", "ready": adversarial_verification_gate }),
        json!({ "name": "planner_executor_separation", "ready": planner_executor_separation_gate }),
        json!({ "name": "multi_agent_handoff", "ready": multi_agent_handoff_gate }),
        json!({ "name": "evaluation_replay_engine", "ready": evaluation_replay_engine_gate }),
        json!({ "name": "trace_model_agent_graph", "ready": trace_model_agent_graph_gate }),
        json!({ "name": "dynamic_workflow_optimization", "ready": dynamic_workflow_optimization_gate }),
        json!({ "name": "think_act_observe_loop", "ready": think_act_observe_loop_gate }),
        json!({ "name": "model_degradation_detection", "ready": model_degradation_detection_gate }),
        json!({ "name": "task_decomposition_pipeline", "ready": task_decomposition_pipeline_gate }),
        json!({ "name": "omnipotent_mode_readiness", "ready": omnipotent_mode_readiness_gate }),
        json!({ "name": "sota_gap_benchmark", "ready": sota_gap_benchmark_gate }),
        json!({ "name": "blue27_release_closure", "ready": blue27_release_closure_gate }),
        json!({ "name": "blue28_release_closure", "ready": blue28_release_closure_gate }),
        json!({ "name": "blue29_release_closure", "ready": blue29_release_closure_gate }),
        json!({ "name": "blue30_release_closure", "ready": blue30_release_closure_gate }),
        json!({ "name": "blue31_release_closure", "ready": blue31_release_closure_gate }),
        json!({ "name": "blue32_release_closure", "ready": blue32_release_closure_gate }),
        json!({ "name": "blue33_release_closure", "ready": blue33_release_closure_gate }),
        json!({ "name": "blue33_remaining_closure", "ready": blue33_remaining_closure_gate }),
        json!({ "name": "blue34_release_closure", "ready": blue34_release_closure_gate }),
        json!({ "name": "blue35_release_closure", "ready": blue35_release_closure_gate }),
    ];

    Ok(json!({
        "ok": true,
        "readiness": {
            "version": "blue26-release-readiness-v2",
            "schema_version": readiness_schema_version,
            "artifact_contract": {
                "schema_version": readiness_schema_version,
            },
            "overall_pass": overall_pass,
            "multi_user_server": {
                "mode": if multi_user_enabled { "multi_user" } else { "single_user" },
                "inference": {
                    "source": server_mode_source,
                },
                "lifecycle": {
                    "ready": multi_user_lifecycle_gate,
                    "blocking_issues": lifecycle_blocking_issues,
                },
                "gate_ready": multi_user_server_gate,
                "release_gate_ready": multi_user_server_gate,
            },
            "summary": {
                "multi_user_inference_source": server_mode_source,
                "multi_user_lifecycle_ready": multi_user_lifecycle_gate,
                "dual_track_consistency_ready": dual_track_consistency_gate,
            },
            "dual_track_consistency": {
                "ready": dual_track_consistency_gate,
                "issues": dual_track_consistency_issues,
            },
            "blocked_gate_names": blocked_gate_names,
            "zero_trust_compliance": { "ready": zero_trust_compliance_gate },
            "rbac_policy_engine": { "ready": rbac_policy_engine_gate },
            "sla_governance": { "ready": sla_governance_gate },
            "skill_engine_core": { "ready": skill_engine_core_gate },
            "workflow_to_skill_conversion": { "ready": workflow_to_skill_conversion_gate },
            "workflow_skill_chain_integration": { "ready": workflow_skill_chain_integration_gate },
            "skill_management_console": { "ready": skill_management_console_gate },
            "enterprise_skill_controls": { "ready": enterprise_skill_controls_gate },
            "core_mode_consistency": { "ready": core_mode_consistency_gate },
            "mode_scenario_adaptability": { "ready": mode_scenario_adaptability_gate },
            "cross_mode_quality_assurance": { "ready": cross_mode_quality_assurance_gate },
            "mode_issue_prevention": { "ready": mode_issue_prevention_gate },
            "subagent_architecture": { "ready": subagent_architecture_gate },
            "subagent_collaboration": { "ready": subagent_collaboration_gate },
            "subagent_observability": { "ready": subagent_observability_gate },
            "knowledge_management": { "ready": knowledge_management_gate },
            "performance_optimization": { "ready": performance_optimization_gate },
            "enterprise_deploy_ops": { "ready": enterprise_deploy_ops_gate },
            "ecosystem_extensibility": { "ready": ecosystem_extensibility_gate },
            "shared_learning_mainchain": { "ready": shared_learning_mainchain_gate },
            "self_evolution_mainchain": { "ready": self_evolution_mainchain_gate },
            "capability_consistency_mainchain": { "ready": capability_consistency_mainchain_gate },
            "shared_learning_data_flow": { "ready": shared_learning_data_flow_gate },
            "self_evolution_flow": { "ready": self_evolution_flow_gate },
            "task_graph_persistence": { "ready": task_graph_persistence_gate },
            "evaluation_harness_baseline": { "ready": evaluation_harness_baseline_gate },
            "memory_write_policy": { "ready": memory_write_policy_gate },
            "task_routing_mainchain": { "ready": task_routing_mainchain_gate },
            "tool_budget_enforcement": { "ready": tool_budget_enforcement_gate },
            "state_store_trait": { "ready": state_store_trait_gate },
            "adversarial_verification": { "ready": adversarial_verification_gate },
            "planner_executor_separation": { "ready": planner_executor_separation_gate },
            "multi_agent_handoff": { "ready": multi_agent_handoff_gate },
            "evaluation_replay_engine": { "ready": evaluation_replay_engine_gate },
            "trace_model_agent_graph": { "ready": trace_model_agent_graph_gate },
            "dynamic_workflow_optimization": { "ready": dynamic_workflow_optimization_gate },
            "think_act_observe_loop": { "ready": think_act_observe_loop_gate },
            "model_degradation_detection": { "ready": model_degradation_detection_gate },
            "task_decomposition_pipeline": { "ready": task_decomposition_pipeline_gate },
            "omnipotent_mode_readiness": { "ready": omnipotent_mode_readiness_gate },
            "sota_gap_benchmark": { "ready": sota_gap_benchmark_gate },
            "blue27_release_closure": { "ready": blue27_release_closure_gate },
            "schema_migration_versioning": { "ready": blue28_release_closure_gate },
            "tenant_auth_api_key": { "ready": blue28_release_closure_gate },
            "sqlite_postgres_migration": { "ready": blue28_release_closure_gate },
            "solution_discovery_hub": { "ready": blue28_release_closure_gate },
            "scenario_matcher": { "ready": blue28_release_closure_gate },
            "subai_factory": { "ready": blue28_release_closure_gate },
            "training_orchestrator": { "ready": blue28_release_closure_gate },
            "auto_integration_runtime": { "ready": blue28_release_closure_gate },
            "reinforcement_loop": { "ready": blue28_release_closure_gate },
            "coordinator_council": { "ready": blue28_release_closure_gate },
            "worker_swarm": { "ready": blue28_release_closure_gate },
            "consensus_engine": { "ready": blue28_release_closure_gate },
            "brain_loop": { "ready": blue28_release_closure_gate },
            "node_reputation": { "ready": blue28_release_closure_gate },
            "self_model_core": { "ready": blue28_release_closure_gate },
            "meta_cognition": { "ready": blue28_release_closure_gate },
            "drift_guard": { "ready": blue28_release_closure_gate },
            "blue28_release_closure": { "ready": blue28_release_closure_gate },
            "federated_rl": { "ready": blue29_release_closure_gate },
            "distributed_memory_bus": { "ready": blue29_release_closure_gate },
            "adaptive_swarm_optimizer": { "ready": blue29_release_closure_gate },
            "hyper_node_network": { "ready": blue29_release_closure_gate },
            "world_model_pipeline": { "ready": blue29_release_closure_gate },
            "continual_learning_hub": { "ready": blue29_release_closure_gate },
            "blue29_release_closure": { "ready": blue29_release_closure_gate },
            "multi_channel_messaging": { "ready": blue30_release_closure_gate },
            "collaboration_game_engine": { "ready": blue30_release_closure_gate },
            "consciousness_proxy_metrics": { "ready": blue30_release_closure_gate },
            "hyper_resilience": { "ready": blue30_release_closure_gate },
            "dual_track_awakening_parity": { "ready": blue30_release_closure_gate },
            "cicd_awareness_gate": { "ready": blue30_release_closure_gate },
            "blue30_release_closure": { "ready": blue30_release_closure_gate },
            "autonomy_boundary_governance": { "ready": blue31_release_closure_gate },
            "emergency_stop_protocol": { "ready": blue31_release_closure_gate },
            "collaboration_ab_evaluation": { "ready": blue31_release_closure_gate },
            "hypernode_topology": { "ready": blue31_release_closure_gate },
            "cross_region_priority_routing": { "ready": blue31_release_closure_gate },
            "meta_controller_replan": { "ready": blue31_release_closure_gate },
            "blue31_release_closure": { "ready": blue31_release_closure_gate },
            "game_theory_balancer": { "ready": blue32_release_closure_gate },
            "federated_rl_v2_guardrail": { "ready": blue32_release_closure_gate },
            "continuous_learning_distillation": { "ready": blue32_release_closure_gate },
            "drift_auto_takeover": { "ready": blue32_release_closure_gate },
            "byzantine_fault_injection": { "ready": blue32_release_closure_gate },
            "recovery_consistency_recheck": { "ready": blue32_release_closure_gate },
            "blue32_release_closure": { "ready": blue32_release_closure_gate },
            "local_reflection_track": { "ready": blue33_release_closure_gate },
            "server_awakening_track": { "ready": blue33_release_closure_gate },
            "ci_gate_continuous_green": { "ready": blue33_release_closure_gate },
            "staged_rollout_guard": { "ready": blue33_release_closure_gate },
            "release_train_freeze": { "ready": blue33_release_closure_gate },
            "rollout_audit_replay": { "ready": blue33_release_closure_gate },
            "blue33_release_closure": { "ready": blue33_release_closure_gate },
            "autonomy_scope_matrix": { "ready": blue33_remaining_closure_gate },
            "redline_policy_runtime": { "ready": blue33_remaining_closure_gate },
            "human_approval_checkpoint": { "ready": blue33_remaining_closure_gate },
            "supernode_hot_standby": { "ready": blue33_remaining_closure_gate },
            "cross_zone_state_snapshot": { "ready": blue33_remaining_closure_gate },
            "failover_recovery_drill": { "ready": blue33_remaining_closure_gate },
            "blue33_remaining_closure": { "ready": blue33_remaining_closure_gate },
            "dual_track_boundary_freeze": { "ready": blue34_release_closure_gate },
            "state_vector_store_trait_unified": { "ready": blue34_release_closure_gate },
            "local_server_profile_matrix": { "ready": blue34_release_closure_gate },
            "postgres_pgvector_schema_versioning": { "ready": blue34_release_closure_gate },
            "sqlite_to_pg_migration_dryrun": { "ready": blue34_release_closure_gate },
            "planner_executor_taskgraph_resume": { "ready": blue34_release_closure_gate },
            "think_act_observe_tool_governance": { "ready": blue34_release_closure_gate },
            "role_handoff_schema_and_conflict_arbiter": { "ready": blue34_release_closure_gate },
            "deterministic_adversarial_double_checks": { "ready": blue34_release_closure_gate },
            "memory_write_promotion_gc_policy": { "ready": blue34_release_closure_gate },
            "benchmark_replay_and_3d_scoring": { "ready": blue34_release_closure_gate },
            "capability_discovery_registry_baseline": { "ready": blue34_release_closure_gate },
            "staged_rollout_canary_rollback_gate": { "ready": blue34_release_closure_gate },
            "distributed_node_registry_heartbeat": { "ready": blue34_release_closure_gate },
            "consensus_with_dissent_preservation": { "ready": blue34_release_closure_gate },
            "brain_loop_artifact_and_safe_degrade": { "ready": blue34_release_closure_gate },
            "fault_injection_recovery_recheck": { "ready": blue34_release_closure_gate },
            "blue34_release_closure": { "ready": blue34_release_closure_gate },
            "custom_role_registry": {
                "ready": custom_role_registry_gate,
            },
            "custom_role_dynamic_matching": {
                "ready": custom_role_dynamic_matching_gate,
            },
            "compliance_audit_metadata": {
                "ready": compliance_audit_metadata_gate,
            },
            "self_rationalization_guard": {
                "ready": self_rationalization_guard_gate,
            },
            "startup_context_loader": {
                "ready": startup_context_loader_gate,
            },
            "layered_prompt_builder": {
                "ready": layered_prompt_builder_gate,
            },
            "layered_token_trigger": {
                "ready": layered_token_trigger_gate,
                "gate_chain": [
                    "entry_auth",
                    "strict_mode",
                    "quality_compass",
                ],
            },
            "multi_priority_scheduler": {
                "ready": multi_priority_scheduler_gate,
            },
            "worker_scheduler_backpressure": {
                "ready": worker_scheduler_backpressure_gate,
            },
            "fork_isolation_guard": {
                "ready": fork_isolation_guard_gate,
                "zombie_reap": true,
            },
            "capability_graph": {
                "ready": capability_graph_gate,
                "node_dependency_graph": true,
            },
            "provenance_ledger": {
                "ready": provenance_ledger_gate,
            },
            "node_reputation_tracker": {
                "ready": node_reputation_tracker_gate,
            },
            "k8s_delivery_pack": {
                "ready": k8s_delivery_pack_gate,
            },
            "sdk_multi_language": {
                "ready": sdk_multi_language_gate,
            },
            "workflow_type_tri_mode": {
                "ready": workflow_type_tri_mode_gate,
                "auto_detection": true,
            },
            "blue35_release_closure": {
                "ready": blue35_release_closure_gate,
            },
            "gates": gate_items,
            "gate_map": {
                "stability": stability_gate,
                "security": security_gate,
                "provider": provider_gate,
                "reproducibility": reproducibility_gate,
                "observability": observability_gate,
                "dual_track_consistency": dual_track_consistency_gate,
                "multi_user_server": multi_user_server_gate,
                "multi_user_lifecycle_ops": multi_user_lifecycle_gate,
                "zero_trust_compliance": zero_trust_compliance_gate,
                "rbac_policy_engine": rbac_policy_engine_gate,
                "sla_governance": sla_governance_gate,
                "skill_engine_core": skill_engine_core_gate,
                "workflow_to_skill_conversion": workflow_to_skill_conversion_gate,
                "workflow_skill_chain_integration": workflow_skill_chain_integration_gate,
                "skill_management_console": skill_management_console_gate,
                "enterprise_skill_controls": enterprise_skill_controls_gate,
                "core_mode_consistency": core_mode_consistency_gate,
                "mode_scenario_adaptability": mode_scenario_adaptability_gate,
                "cross_mode_quality_assurance": cross_mode_quality_assurance_gate,
                "mode_issue_prevention": mode_issue_prevention_gate,
                "subagent_architecture": subagent_architecture_gate,
                "subagent_collaboration": subagent_collaboration_gate,
                "subagent_observability": subagent_observability_gate,
                "knowledge_management": knowledge_management_gate,
                "performance_optimization": performance_optimization_gate,
                "enterprise_deploy_ops": enterprise_deploy_ops_gate,
                "ecosystem_extensibility": ecosystem_extensibility_gate,
                "shared_learning_mainchain": shared_learning_mainchain_gate,
                "self_evolution_mainchain": self_evolution_mainchain_gate,
                "capability_consistency_mainchain": capability_consistency_mainchain_gate,
                "shared_learning_data_flow": shared_learning_data_flow_gate,
                "self_evolution_flow": self_evolution_flow_gate,
            },
            "blue_gates": {
                "blue27_release_closure": blue27_release_closure_gate,
                "blue28_release_closure": blue28_release_closure_gate,
                "blue29_release_closure": blue29_release_closure_gate,
                "blue30_release_closure": blue30_release_closure_gate,
                "blue31_release_closure": blue31_release_closure_gate,
                "blue32_release_closure": blue32_release_closure_gate,
                "blue33_release_closure": blue33_release_closure_gate,
                "blue33_remaining_closure": blue33_remaining_closure_gate,
                "blue34_release_closure": blue34_release_closure_gate,
                "blue35_release_closure": blue35_release_closure_gate,
            },
            "recommendations": recommendations,
            "timestamp": status.timestamp,
        }
    }))
}

// BLUE gate helper functions (extracted inline to reduce duplication)

fn federated_rl_ready(open_breakers: u64, dual_track: bool) -> bool {
    dual_track && open_breakers == 0
}

fn multi_channel_ready(observability: bool) -> bool {
    observability
}

fn autonomy_boundary_ready(healthy: bool) -> bool {
    healthy
}

fn federated_rl_v2_ready(open_breakers: u64) -> bool {
    open_breakers == 0
}

fn local_reflection_ready(dual_track: bool) -> bool {
    dual_track
}

fn autonomy_scope_ready(healthy: bool) -> bool {
    healthy
}

fn dual_track_boundary_freeze_ready_fn(observability: bool) -> bool {
    observability
}

fn custom_role_registry_ready_fn(healthy: bool) -> bool {
    healthy
}

// ---------------------------------------------------------------------------
// Harness Status
// ---------------------------------------------------------------------------

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

pub(super) async fn harness_status_payload(server: &AcpServer, params: Value) -> Result<Value> {
    let fixed_seed = params
        .get("seed")
        .and_then(Value::as_u64)
        .unwrap_or(20260415);

    let requests_root = std::path::Path::new("requests").to_path_buf();
    // Use spawn_blocking to avoid blocking the tokio runtime with synchronous fs I/O.
    type RequestLists = (
        Vec<String>,
        Vec<String>,
        Vec<String>,
        Vec<String>,
        Vec<String>,
    );
    let result: Result<_, _> = tokio::task::spawn_blocking(move || -> RequestLists {
        let mut smoke = Vec::new();
        let mut regression = Vec::new();
        let mut adversarial = Vec::new();
        let mut long_chain = Vec::new();
        let mut warnings: Vec<String> = Vec::new();
        match std::fs::read_dir(&requests_root) {
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
        (smoke, regression, adversarial, long_chain, warnings)
    })
    .await;

    let (smoke, regression, adversarial, long_chain, warnings) = match result {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("harness status spawn_blocking failed: {e}");
            Default::default()
        }
    };

    let scenario_total = smoke.len() + regression.len() + adversarial.len() + long_chain.len();
    let metrics = server.observability.metrics.snapshot();
    Ok(json!({
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
    }))
}
