use super::protocol_pack::record_tool_call_audit_with_protocol;
use super::*;

pub(super) fn record_mcp_tool_audit(name: &str, arguments: &Value, success: bool, reason: &str) {
    record_tool_call_audit_with_protocol(name, arguments, success, reason, "acp_stdio");
}

pub(super) fn record_skill_admin_audit(action: &str, target: &str, success: bool, reason: &str) {
    record_skill_admin_audit_with_protocol(action, target, success, reason, "acp_stdio");
}

pub(super) fn record_skill_admin_audit_with_protocol(
    action: &str,
    target: &str,
    success: bool,
    reason: &str,
    protocol: &str,
) {
    let entry = AutonomousEditAuditEntry {
        timestamp: crate::acp::prelude::now_ts().to_string(),
        agent: format!("skill.{}", action),
        file_path: target.to_string(),
        change_summary: format!(
            "action={} status={} protocol={}",
            action,
            if success { "ok" } else { "error" },
            protocol,
        ),
        approval_reason: reason.to_string(),
        confidence_score: if success { 1.0 } else { 0.0 },
        reversible: action != "import",
    };
    if let Err(err) = mcp_audit_logger().record(&entry) {
        debug!("failed to record skill admin audit: {}", err);
    }
}

pub(super) fn parse_skill_name_param(params: &Value) -> Result<String> {
    params
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| anyhow::anyhow!("missing required param: name"))
}

pub(super) fn skill_import_policy(server: &AcpServer) -> SkillImportPolicy {
    SkillImportPolicy::from_runtime(&server.runtime_config)
}

pub(super) fn open_skill_import_store(server: &AcpServer) -> Result<SkillImportStore> {
    SkillImportStore::load(skill_import_policy(server))
}

pub(super) fn normalize_imported_record(record: ImportedSkillRecord) -> Value {
    json!({
        "name": record.name,
        "version": record.version,
        "description": record.description,
        "source": record.source,
        "source_ref": record.source_ref,
        "sha256": record.sha256,
        "manifest_path": record.manifest_path,
        "enabled": record.enabled,
        "imported_at": record.imported_at,
    })
}

pub(super) async fn handle_skill_import(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let request: SkillImportRequest =
        serde_json::from_value(params).context("invalid params for skill.import")?;
    let mut store = open_skill_import_store(server)?;
    let imported = match store.import_skill(request).await {
        Ok(record) => record,
        Err(err) => {
            record_skill_admin_audit("import", "skill.import", false, &err.to_string());
            return send_error(server, request_id, -32602, err.to_string(), None).await;
        }
    };
    store.save()?;
    let imported_name = imported.name.clone();
    record_skill_admin_audit(
        "import",
        &imported.name,
        true,
        "imported skill manifest with supply-chain checks",
    );
    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "action": "import",
            "name": imported_name,
            "skill": normalize_imported_record(imported)
        }),
    )
    .await
}

pub(super) async fn handle_skill_list_imported(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    let store = open_skill_import_store(server)?;
    let skills = store
        .list()
        .into_iter()
        .map(normalize_imported_record)
        .collect::<Vec<_>>();
    let total = skills.len();
    let enabled = skills
        .iter()
        .filter(|skill| skill.get("enabled").and_then(Value::as_bool) == Some(true))
        .count();
    let disabled = total.saturating_sub(enabled);
    record_skill_admin_audit(
        "list_imported",
        "skill.list_imported",
        true,
        "listed imported skills",
    );
    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "action": "list_imported",
            "total": total,
            "enabled": enabled,
            "disabled": disabled,
            "skills": skills,
        }),
    )
    .await
}

pub(super) async fn handle_skill_enabled_toggle(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
    enabled: bool,
) -> Result<()> {
    let action = if enabled { "enable" } else { "disable" };
    let name = match parse_skill_name_param(&params) {
        Ok(name) => name,
        Err(err) => {
            record_skill_admin_audit(action, "skill.toggle", false, &err.to_string());
            return send_error(server, request_id, -32602, err.to_string(), None).await;
        }
    };
    let mut store = open_skill_import_store(server)?;
    let updated = match store.set_enabled(&name, enabled) {
        Ok(record) => record,
        Err(err) => {
            record_skill_admin_audit(action, &name, false, &err.to_string());
            return send_error(server, request_id, -32602, err.to_string(), None).await;
        }
    };
    store.save()?;
    record_skill_admin_audit(action, &name, true, "updated imported skill state");
    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "action": action,
            "name": name,
            "skill": normalize_imported_record(updated),
        }),
    )
    .await
}

pub(super) async fn handle_skill_remove(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let name = match parse_skill_name_param(&params) {
        Ok(name) => name,
        Err(err) => {
            record_skill_admin_audit("remove", "skill.remove", false, &err.to_string());
            return send_error(server, request_id, -32602, err.to_string(), None).await;
        }
    };
    let mut store = open_skill_import_store(server)?;
    let removed = store.remove(&name);
    if !removed {
        let reason = format!("imported skill '{}' not found", name);
        record_skill_admin_audit("remove", &name, false, &reason);
        return send_error(server, request_id, -32602, reason, None).await;
    }
    let unregistered = server
        .skill_registry
        .lock()
        .map(|mut registry| registry.unregister(&name))
        .unwrap_or(false);
    store.save()?;
    record_skill_admin_audit("remove", &name, true, "removed imported skill record");

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "action": "remove",
            "removed": removed,
            "unregistered": unregistered,
            "name": name,
        }),
    )
    .await
}

pub(super) fn governance_action_label(action: GovernanceAction) -> &'static str {
    match action {
        GovernanceAction::Read => "read",
        GovernanceAction::Search => "search",
        GovernanceAction::Write => "write",
        GovernanceAction::Shell => "shell",
    }
}

pub(super) fn audit_file_path_from_arguments(name: &str, arguments: &Value) -> String {
    for key in ["path", "filePath", "sourcePdfPath"] {
        if let Some(path) = arguments.get(key).and_then(Value::as_str) {
            return path.to_string();
        }
    }
    format!("tool:{name}")
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
