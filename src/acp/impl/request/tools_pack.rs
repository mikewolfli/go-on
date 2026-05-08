use super::*;

pub(super) fn skill_import_policy(server: &AcpServer) -> SkillImportPolicy {
    SkillImportPolicy::from_runtime(&server.runtime_config)
}

pub(super) fn open_skill_import_store(server: &AcpServer) -> Result<SkillImportStore> {
    SkillImportStore::load(skill_import_policy(server))
}

pub(super) fn build_mcp_tool_descriptors(server: &AcpServer) -> Vec<Value> {
    let mut tools = vec![
        json!({
            "name": "acp_trace_get",
            "description": "Get ACP trace events",
            "input_schema": {"type": "object"}
        }),
        json!({
            "name": "acp_debug_panel_get",
            "description": "Get ACP debug panel snapshot",
            "input_schema": {"type": "object"}
        }),
        json!({
            "name": "goon_workflow_run_list",
            "description": "List workflow runs with pagination and status filter",
            "input_schema": {"type": "object"}
        }),
        json!({
            "name": "goon_workflow_run_get",
            "description": "Get workflow run details by run_id",
            "input_schema": {"type": "object", "required": ["run_id"]}
        }),
        json!({
            "name": "goon_workflow_run_cancel",
            "description": "Cancel workflow run by run_id",
            "input_schema": {"type": "object", "required": ["run_id"]}
        }),
        json!({
            "name": "goon_workflow_run_pause",
            "description": "Pause workflow run by run_id",
            "input_schema": {"type": "object", "required": ["run_id"]}
        }),
        json!({
            "name": "goon_workflow_run_resume",
            "description": "Resume workflow run by run_id",
            "input_schema": {"type": "object", "required": ["run_id"]}
        }),
        json!({
            "name": "goon_provider_test_connection",
            "description": "Validate provider connectivity and key readiness",
            "input_schema": {"type": "object", "required": ["provider"]}
        }),
        json!({
            "name": "goon_provider_test_completion",
            "description": "Validate provider/model completion route",
            "input_schema": {"type": "object", "required": ["provider"]}
        }),
        json!({
            "name": "goon_provider_capabilities",
            "description": "Query provider model capabilities metadata",
            "input_schema": {"type": "object", "required": ["provider"]}
        }),
        json!({
            "name": "goon_metrics_window_query",
            "description": "Query metrics time-window series (1m/5m/1h)",
            "input_schema": {"type": "object"}
        }),
        json!({
            "name": "goon_metrics_errors_summary",
            "description": "Query grouped errors and sample failures",
            "input_schema": {"type": "object"}
        }),
        json!({
            "name": "goon_skill_update",
            "description": "Update imported skill manifest fields",
            "input_schema": {"type": "object", "required": ["name"]}
        }),
        json!({
            "name": "goon_skill_version_list",
            "description": "List imported skill version snapshots",
            "input_schema": {"type": "object", "required": ["name"]}
        }),
        json!({
            "name": "goon_skill_version_rollback",
            "description": "Rollback imported skill to a specified version",
            "input_schema": {"type": "object", "required": ["name", "version"]}
        }),
    ];

    let registry = ToolRegistry::new();
    let mut builtins = registry.names();
    builtins.sort_unstable();
    tools.extend(builtins.into_iter().map(|name| {
        serde_json::to_value(local_tool_descriptor(name)).unwrap_or_else(|_| {
            json!({
                "name": name,
                "description": "Registered MCP tool",
                "input_schema": {"type": "object"}
            })
        })
    }));

    if let Ok(registry) = server.skill_registry.lock() {
        tools.extend(registry.list().into_iter().map(|skill| {
            json!({
                "name": skill.name,
                "description": skill.description,
                "input_schema": skill.input_schema,
                "x_runtime": {
                    "score": skill.score,
                    "total_calls": skill.total_calls,
                    "success_calls": skill.success_calls,
                    "failure_calls": skill.failure_calls,
                    "average_latency_ms": skill.average_latency_ms,
                }
            })
        }));
    }

    if let Ok(store) = open_skill_import_store(server) {
        for record in store.list().into_iter().filter(|record| record.enabled) {
            let (description, input_schema) = load_imported_skill_manifest(&record)
                .map(|manifest| {
                    let description = if manifest.description.trim().is_empty() {
                        format!(
                            "Imported skill manifest {}@{}",
                            manifest.name, manifest.version
                        )
                    } else {
                        manifest.description
                    };
                    (description, manifest.input_schema)
                })
                .unwrap_or_else(|| {
                    (
                        format!("Imported skill manifest {}@{}", record.name, record.version),
                        json!({"type": "object"}),
                    )
                });

            tools.push(json!({
                "name": record.name,
                "description": description,
                "input_schema": input_schema,
                "x_import": {
                    "source": record.source,
                    "source_ref": record.source_ref,
                    "sha256": record.sha256,
                    "version": record.version,
                    "manifest_path": record.manifest_path,
                }
            }));
        }
    }

    tools
}

pub(super) async fn execute_mcp_tool_call(
    server: &AcpServer,
    name: &str,
    arguments: &Value,
) -> Result<Value> {
    let policy = policy_bundle_for_target(server.runtime_config.deployment_target.as_deref());
    let budget_scope = budget_scope_key(name, arguments);
    let estimated_tokens = estimate_argument_tokens(arguments);
    let pua_engine = PuaRuleEngine::new(server.pua_enforcement_plan.clone());
    let remaining_tokens = {
        let mut trackers = tool_budget_trackers()
            .lock()
            .map_err(|e| anyhow::anyhow!("failed to lock tool budget tracker: {e}"))?;
        let tracker = trackers.entry(budget_scope.clone()).or_insert_with(|| {
            BudgetTracker::new(task_budget_for_target(
                server.runtime_config.deployment_target.as_deref(),
            ))
        });
        tracker.check_wall_clock().map_err(|err| {
            anyhow::anyhow!("budget denied tool '{name}' in scope '{budget_scope}': {err}")
        })?;
        tracker.record_tool_call().map_err(|err| {
            anyhow::anyhow!("budget denied tool '{name}' in scope '{budget_scope}': {err}")
        })?;
        tracker
            .consume_with_pua(estimated_tokens, &pua_engine)
            .map_err(|err| {
                anyhow::anyhow!("budget denied tool '{name}' in scope '{budget_scope}': {err}")
            })?;
        tracker.remaining_tokens()
    };

    let action = governance_action_for_tool(name);
    let decision = enforce_action(&policy, action);
    if !decision.allowed {
        anyhow::bail!(
            "hardening policy denied tool '{}' (policy={}, sandbox={}): {}",
            name,
            decision.policy_name,
            decision.sandbox_level,
            decision.reason
        );
    }
    info!(
        "hardening allow tool={} policy={} sandbox={} budget_scope={} estimated_tokens={} remaining_tokens={}",
        name,
        decision.policy_name,
        decision.sandbox_level,
        budget_scope,
        estimated_tokens,
        remaining_tokens
    );

    match name {
        "acp_trace_get" => {
            let trace = build_trace_payload(arguments);
            Ok(json!({
                "ok": true,
                "events": trace.get("events").cloned().unwrap_or_else(|| json!([])),
                "total": trace.get("total").cloned().unwrap_or_else(|| json!(0)),
                "limit": trace.get("limit").cloned().unwrap_or_else(|| json!(100)),
            }))
        }
        "acp_debug_panel_get" => Ok(build_debug_panel_payload(server).await),
        "goon_workflow_run_list" => Ok(workflow_run_list_payload(arguments)),
        "goon_workflow_run_get" => workflow_run_get_payload(arguments),
        "goon_workflow_run_cancel" => workflow_run_transition_payload(arguments, "cancelled"),
        "goon_workflow_run_pause" => workflow_run_transition_payload(arguments, "paused"),
        "goon_workflow_run_resume" => workflow_run_transition_payload(arguments, "running"),
        "goon_provider_test_connection" => provider_test_connection_payload(server, arguments),
        "goon_provider_test_completion" => provider_test_completion_payload(server, arguments),
        "goon_provider_capabilities" => provider_capabilities_payload(server, arguments),
        "goon_metrics_window_query" => Ok(metrics_window_query_payload(server, arguments)),
        "goon_metrics_errors_summary" => Ok(metrics_errors_summary_payload(server, arguments)),
        "goon_skill_update" => skill_update_payload(server, arguments),
        "goon_skill_version_list" => skill_version_list_payload(server, arguments),
        "goon_skill_version_rollback" => skill_version_rollback_payload(server, arguments),
        _ => {
            let registry = ToolRegistry::new();
            if let Some(tool) = registry.get(name) {
                validate_tool_arguments(name, arguments)?;
                let result = tool.run(&ToolInput {
                    task_id: format!("mcp-tool-{name}"),
                    phase: "mcp".to_string(),
                    agent_role: "tool".to_string(),
                    objective: format!("Execute MCP tool '{name}'"),
                    constraints: None,
                    evidence: None,
                    payload: arguments.clone(),
                    allowed_base_dir: None,
                })?;
                return Ok(serde_json::to_value(result)?);
            }

            let resolved_skill_name = server.skill_registry.lock().ok().and_then(|registry| {
                if registry.get(name).is_some() {
                    Some(name.to_string())
                } else {
                    registry.best_match_with_input(name, arguments)
                }
            });
            let skill = resolved_skill_name.as_ref().and_then(|resolved| {
                server
                    .skill_registry
                    .lock()
                    .ok()
                    .and_then(|registry| registry.get(resolved))
            });
            match skill {
                Some(skill) => {
                    let started = Instant::now();
                    let outcome = skill.execute(arguments).await;
                    let skill_name = resolved_skill_name.as_deref().unwrap_or(name);
                    if let Ok(mut registry) = server.skill_registry.lock() {
                        registry.record_outcome(skill_name, outcome.is_ok(), started.elapsed());
                    }
                    outcome
                }
                None => {
                    if let Some(imported) = find_enabled_imported_skill(server, name)? {
                        if let Some(manifest) = load_imported_skill_manifest(&imported) {
                            return Ok(json!({
                                "ok": true,
                                "executed": false,
                                "mode": "imported_manifest",
                                "code": "NOT_IMPLEMENTED_EXECUTOR",
                                "name": manifest.name,
                                "version": manifest.version,
                                "source": imported.source,
                                "source_ref": imported.source_ref,
                                "sha256": imported.sha256,
                                "input": arguments,
                                "note": "Imported skill is manifest-backed in this release; execution returns structured passthrough until runtime plugin executor is enabled."
                            }));
                        }
                        return Ok(json!({
                            "ok": true,
                            "executed": false,
                            "mode": "imported_manifest",
                            "code": "NOT_IMPLEMENTED_EXECUTOR",
                            "name": imported.name,
                            "version": imported.version,
                            "source": imported.source,
                            "source_ref": imported.source_ref,
                            "sha256": imported.sha256,
                            "input": arguments,
                            "note": "Imported skill manifest is unavailable; returned metadata passthrough response."
                        }));
                    }
                    anyhow::bail!("unknown mcp tool: {name}")
                }
            }
        }
    }
}

pub(super) fn find_enabled_imported_skill(
    server: &AcpServer,
    name: &str,
) -> Result<Option<ImportedSkillRecord>> {
    let store = open_skill_import_store(server)?;
    Ok(store
        .list()
        .into_iter()
        .find(|record| record.enabled && record.name == name))
}

pub(super) fn load_imported_skill_manifest(
    record: &ImportedSkillRecord,
) -> Option<SkillImportManifest> {
    let raw = fs::read_to_string(&record.manifest_path).ok()?;
    serde_json::from_str::<SkillImportManifest>(&raw).ok()
}

pub(super) fn budget_scope_key(name: &str, arguments: &Value) -> String {
    if let Some(task_id) = arguments.get("task_id").and_then(Value::as_str) {
        return format!("task:{task_id}");
    }
    if let Some(conversation_id) = arguments.get("conversation_id").and_then(Value::as_str) {
        return format!("conversation:{conversation_id}");
    }
    format!("tool:{name}")
}

pub(super) fn estimate_argument_tokens(arguments: &Value) -> usize {
    // Lightweight approximation keeps budget enforcement deterministic without model calls.
    serde_json::to_string(arguments)
        .map(|payload| (payload.len() / 4).max(1))
        .unwrap_or(1)
}

pub(super) fn governance_action_for_tool(name: &str) -> GovernanceAction {
    let normalized = name.to_ascii_lowercase();
    if normalized.contains("shell") || normalized.contains("command") {
        return GovernanceAction::Shell;
    }
    if normalized.contains("write") || normalized.contains("edit") || normalized.contains("create")
    {
        return GovernanceAction::Write;
    }
    if normalized.contains("search") || normalized.contains("find") {
        return GovernanceAction::Search;
    }
    GovernanceAction::Read
}

pub(super) fn local_tool_descriptor(name: &'static str) -> Value {
    crate::shared::tool_descriptors::tool_descriptor_value(name)
}

pub(super) fn validate_tool_arguments(tool_name: &str, tool_input: &Value) -> Result<()> {
    crate::shared::tool_descriptors::validate_required_arguments(tool_name, tool_input)
}
