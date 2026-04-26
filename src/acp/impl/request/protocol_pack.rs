use super::*;

pub(super) async fn handle_initialize(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    send_result(
        server,
        request_id,
        json!({
            "name": "go-on",
            "version": env!("CARGO_PKG_VERSION"),
            "protocol": "acp",
            "capabilities": {
                "chat": true,
                "phase": true,
                "metrics": true,
                "shutdown": true,
                "health": true,
                "debug_panel": true,
                "mcp_adapter": true,
            }
        }),
    )
    .await
}

pub(super) async fn handle_mcp_initialize(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    send_result(
        server,
        request_id,
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "serverInfo": {
                "name": "go-on",
                "version": env!("CARGO_PKG_VERSION")
            }
        }),
    )
    .await
}

pub(super) async fn handle_mcp_tools_list(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    let tools = build_mcp_tool_descriptors(server);

    send_result(
        server,
        request_id,
        json!({
            "tools": tools
        }),
    )
    .await
}

pub(super) async fn handle_mcp_tools_call(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let name = params
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or_default();

    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let structured = match execute_mcp_tool_call(server, name, &arguments).await {
        Ok(structured) => structured,
        Err(err) => {
            record_mcp_tool_audit(name, &arguments, false, &err.to_string());
            return send_error(server, request_id, -32602, err.to_string(), None).await;
        }
    };
    record_mcp_tool_audit(name, &arguments, true, "tool executed successfully");

    send_result(
        server,
        request_id,
        json!({
            "content": [{"type": "text", "text": structured.to_string()}],
            "structuredContent": structured
        }),
    )
    .await
}

fn record_mcp_tool_audit(name: &str, arguments: &Value, success: bool, reason: &str) {
    record_tool_call_audit_with_protocol(name, arguments, success, reason, "acp_stdio");
}

pub fn record_tool_call_audit_with_protocol(
    name: &str,
    arguments: &Value,
    success: bool,
    reason: &str,
    protocol: &str,
) {
    let action = governance_action_for_tool(name);
    let reversible = matches!(action, GovernanceAction::Read | GovernanceAction::Search);
    let file_path = audit_file_path_from_arguments(name, arguments);
    let entry = AutonomousEditAuditEntry {
        timestamp: crate::acp::prelude::now_ts().to_string(),
        agent: "mcp.tools.call".to_string(),
        file_path,
        change_summary: format!(
            "tool={} action={} status={} protocol={}",
            name,
            governance_action_label(action),
            if success { "ok" } else { "error" },
            protocol,
        ),
        approval_reason: reason.to_string(),
        confidence_score: if success { 1.0 } else { 0.0 },
        reversible,
    };

    if let Err(err) = mcp_audit_logger().record(&entry) {
        debug!("failed to record mcp audit log: {}", err);
    }
}

fn record_skill_admin_audit(action: &str, target: &str, success: bool, reason: &str) {
    record_skill_admin_audit_with_protocol(action, target, success, reason, "acp_stdio");
}

fn record_skill_admin_audit_with_protocol(
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

fn parse_skill_name_param(params: &Value) -> Result<String> {
    params
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| anyhow::anyhow!("missing required param: name"))
}

fn skill_import_policy(server: &AcpServer) -> SkillImportPolicy {
    SkillImportPolicy::from_runtime(&server.runtime_config)
}

fn open_skill_import_store(server: &AcpServer) -> Result<SkillImportStore> {
    SkillImportStore::load(skill_import_policy(server))
}

fn normalize_imported_record(record: ImportedSkillRecord) -> Value {
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

fn governance_action_label(action: GovernanceAction) -> &'static str {
    match action {
        GovernanceAction::Read => "read",
        GovernanceAction::Search => "search",
        GovernanceAction::Write => "write",
        GovernanceAction::Shell => "shell",
    }
}

fn audit_file_path_from_arguments(name: &str, arguments: &Value) -> String {
    for key in ["path", "filePath", "sourcePdfPath"] {
        if let Some(path) = arguments.get(key).and_then(Value::as_str) {
            return path.to_string();
        }
    }
    format!("tool:{name}")
}

pub(super) async fn handle_chat(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
    trace: &RequestTraceContext,
) -> Result<()> {
    use crate::acp::r#impl::chat::handle_chat as chat_handler;

    match chat_handler(
        server,
        request_id.clone(),
        Some(params),
        None,
        Some(trace.clone()),
    )
    .await
    {
        Ok(()) => Ok(()),
        Err(err) => {
            let message = err.to_string();
            if message.to_ascii_lowercase().contains("rate limited") {
                send_error(server, request_id, -32029, message, None).await
            } else {
                send_error(server, request_id, -32603, message, None).await
            }
        }
    }
}

pub(super) async fn handle_phase(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
    _trace: &RequestTraceContext,
) -> Result<()> {
    let rate_limiter = server
        .phase_rate_limiter
        .lock()
        .map(|guard| {
            json!({
                "tracked": guard.tracked_phases(),
                "buckets": guard.snapshot(),
            })
        })
        .unwrap_or_else(|_| json!({"tracked": 0, "buckets": {}}));

    let inflight = server
        .inflight_limiter
        .lock()
        .map(|guard| {
            let (global, phase) = guard.snapshot();
            json!({"global": global, "phase": phase})
        })
        .unwrap_or_else(|_| json!({"global": 0, "phase": {}}));

    send_result(
        server,
        request_id,
        json!({
            "rate_limiter": rate_limiter,
            "inflight": inflight,
        }),
    )
    .await
}
