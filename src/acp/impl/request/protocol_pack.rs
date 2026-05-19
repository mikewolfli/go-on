use tracing::warn;

use super::*;
use crate::mcp::MCP_VERSION;

/// Build Go-On chat params from ACP prompt content blocks.
/// ACP sends `prompt: [{type: "text", text: "..."}]`,
/// returns Go-On chat params: `{mode, messages: [{role, content}], conversation_id}`.
fn build_chat_params_from_acp(params: Value) -> Value {
    let prompt_blocks = params.get("prompt").and_then(|p| p.as_array());
    let text = match prompt_blocks {
        Some(blocks) => blocks
            .iter()
            .filter_map(|block| {
                if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                    block.get("text").and_then(|t| t.as_str())
                } else {
                    None
                }
            })
            .collect::<Vec<&str>>()
            .join("\n"),
        None => String::new(),
    };
    let messages: Vec<Value> = if text.is_empty() {
        vec![]
    } else {
        vec![json!({"role": "user", "content": text})]
    };
    let mut chat_params = json!({
        "mode": "ask",
        "messages": messages,
    });
    if let Some(cid) = params.get("sessionId").and_then(|s| s.as_str()) {
        chat_params["conversation_id"] = json!(cid);
    }
    chat_params
}

pub(super) async fn handle_initialize(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    send_result(
        server,
        request_id,
        json!({
            // Standard ACP InitializeResponse fields (camelCase)
            "protocolVersion": 1,
            "agentInfo": {
                "name": "go-on",
                "version": env!("CARGO_PKG_VERSION")
            },
            "agentCapabilities": {
                "loadSession": true,
                "promptCapabilities": {
                    "image": false,
                    "audio": false,
                    "embeddedContext": false
                },
                "mcpCapabilities": {
                    "http": true,
                    "sse": false
                },
                "sessionCapabilities": {
                    "list": {},
                    "additionalDirectories": {}
                }
            },
            // Go-On custom fields (backward compat)
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
            "protocolVersion": MCP_VERSION,
            "capabilities": {},
            "serverInfo": {
                "name": "go-on",
                "version": env!("CARGO_PKG_VERSION")
            }
        }),
    )
    .await
}

use std::sync::atomic::{AtomicU64, Ordering};

/// Atomic counter for generating unique ACP session IDs.
static ACP_SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Generate a unique session ID for the standard ACP protocol.
fn generate_acp_session_id() -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let seq = ACP_SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("acp-session-{:x}-{:x}", ts, seq)
}

/// Handle `session/new` — creates a new ACP session.
///
/// Standard ACP: client sends `cwd` + optional `mcpServers`,
/// agent responds with `sessionId`.
pub(super) async fn handle_session_new(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let session_id = generate_acp_session_id();
    let mut modes = build_default_modes();

    // If client specified an initial mode, use it
    if let Some(mode_id) = params.get("mode").and_then(|m| m.as_str()) {
        if let Some(obj) = modes.as_object_mut() {
            obj.insert("currentModeId".to_string(), json!(mode_id));
        }
    }

    send_result(
        server,
        request_id,
        json!({
            "sessionId": session_id,
            "modes": modes,
            "configOptions": [
                {
                    "id": "mode",
                    "name": "Chat Mode",
                    "description": "Select interaction mode",
                    "category": "mode",
                    "type": "select",
                    "currentValue": "ask",
                    "options": [
                        {"value": "ask", "name": "Ask / 对话"},
                        {"value": "plan", "name": "Plan / 计划"},
                        {"value": "edit", "name": "Edit / 编辑"},
                        {"value": "safeguard", "name": "Safeguard / 安全"},
                        {"value": "full_auto", "name": "Full Auto / 全自动"}
                    ]
                }
            ],
        }),
    )
    .await
}

/// Handle `session/load` — loads an existing session.
///
/// Standard ACP: client sends `sessionId` + optional `cwd`,
/// agent restores the session context and returns available modes/config.
pub(super) async fn handle_session_load(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let modes = build_default_modes();
    send_result(
        server,
        request_id,
        json!({
            "modes": modes,
            "configOptions": [],
        }),
    )
    .await
}

/// Handle `session/prompt` — processes a user prompt within a session.
///
/// Standard ACP: client sends `sessionId` + `prompt` (content blocks),
/// agent streams notifications and returns a `PromptResponse` with `stopReason`.
/// Maps to Go-On's internal chat handler for the actual AI processing.
/// Converts ACP `prompt` content blocks to Go-On `messages` format.
pub(super) async fn handle_session_prompt(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    // Use process_chat_request for proper agent selection via the Capability Bus.
    // This ensures correct agent routing and orchestration.
    use crate::acp::r#impl::chat::{process_chat_request, ChatParams};
    use crate::rpc_protocol::chat_trace_context;

    // Build Go-On chat params from ACP prompt
    let chat_params_value = build_chat_params_from_acp(params);
    let chat_params: ChatParams = match serde_json::from_value(chat_params_value) {
        Ok(p) => p,
        Err(e) => {
            return send_error(
                server,
                request_id,
                -32602,
                format!("invalid chat params: {}", e),
                None,
            )
            .await;
        }
    };

    let pipeline_trace = chat_trace_context(&request_id, "session.prompt");

    tracing::info!("ACP session/prompt: delegating to process_chat_request");

    // Wrap process_chat_request in a 60-second total timeout to prevent hangs
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        process_chat_request(server, &chat_params, None, &pipeline_trace, None, None),
    )
    .await;

    match result {
        Ok(_result) => {
            tracing::info!("ACP session/prompt: completed successfully");
            send_result(server, request_id, json!({"stopReason": "end_turn"})).await
        }
        Err(err) => {
            let msg = err.to_string();
            tracing::warn!("ACP session/prompt: error: {}", msg);
            if msg.to_ascii_lowercase().contains("rate limited") {
                send_error(server, request_id, -32029, msg, None).await
            } else {
                send_error(server, request_id, -32603, msg, None).await
            }
        }
    }
}

/// Handle `session/cancel` — cancels an ongoing prompt turn.
///
/// Standard ACP notification: client sends `sessionId`,
/// agent stops processing and returns `StopReason::Cancelled`.
pub(super) async fn handle_session_cancel(
    _server: &AcpServer,
    _params: Value,
    _request_id: Option<Value>,
) -> Result<()> {
    // session/cancel is a notification — no response expected per JSON-RPC spec.
    // In the future, we can hook into active request cancellation here.
    // For now, the chat handler detects cancellation via its own mechanisms.
    Ok(())
}

/// Handle `session/list` — lists existing sessions.
///
/// Standard ACP: client may send optional `cwd` filter,
/// agent returns list of known sessions.
pub(super) async fn handle_session_list(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    send_result(
        server,
        request_id,
        json!({
            "sessions": [],
        }),
    )
    .await
}

/// Handle `session/set_mode` — sets the current mode for a session.
///
/// Standard ACP: client sends `sessionId` + `modeId`,
/// agent switches mode. Returns updated config options per spec.
pub(super) async fn handle_session_set_mode(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    // Per ACP spec, SetSessionModeResponse is empty (mode change confirmed).
    // Mode state is communicated via session/update notification.
    send_result(server, request_id, json!({})).await
}

/// Handle `session/set_config_option` — sets a configuration option for a session.
///
/// Standard ACP: client sends `sessionId` + `configId` + `value`,
/// agent applies the option and returns updated config options list.
pub(super) async fn handle_session_set_config_option(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    send_result(
        server,
        request_id,
        json!({
            "configOptions": [],
        }),
    )
    .await
}

/// Build the default set of session modes.
fn build_default_modes() -> serde_json::Value {
    json!({
        "currentModeId": "ask",
        "availableModes": [
            {
                "id": "ask",
                "name": "Ask / 对话",
                "description": "Q&A assistant — general questions"
            },
            {
                "id": "plan",
                "name": "Plan / 计划",
                "description": "Planning mode — structured task breakdown"
            },
            {
                "id": "edit",
                "name": "Edit / 编辑",
                "description": "Edit/review mode — code changes"
            },
            {
                "id": "safeguard",
                "name": "Safeguard / 安全",
                "description": "Safety-first — escalation on high-risk operations"
            },
            {
                "id": "full_auto",
                "name": "Full Auto / 全自动",
                "description": "Fully autonomous — agent runs without user confirmation"
            }
        ]
    })
}

// ── Standard ACP authentication handlers ──────────────────────────────────

/// Handle `authenticate` — authenticates the client.
/// Standard ACP: client sends `methodId`, agent performs auth and returns success.
pub(super) async fn handle_authenticate(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    // Default implementation: all auth succeeds (auth is optional).
    // If entry_auth is enabled, it's handled at the HTTP/transport layer.
    send_result(server, request_id, json!({})).await
}

/// Handle `logout` — terminates the current authenticated session.
pub(super) async fn handle_logout(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    send_result(server, request_id, json!({})).await
}

/// Handle `$/cancel_request` — protocol-level request cancellation notification.
/// JSON-RPC notification: client can cancel an in-flight request by ID.
/// No response expected per JSON-RPC spec.
pub(super) async fn handle_cancel_request(
    _server: &AcpServer,
    _params: Value,
    _request_id: Option<Value>,
) -> Result<()> {
    // $/cancel_request is a notification — no response expected.
    // Future enhancement: route cancellation to the active request handler.
    Ok(())
}

// ── MCP bridge handlers (mcp.* methods routed through ACP dispatch) ──────

/// Handle `mcp.ping` — health check ping.
pub(super) async fn handle_mcp_ping(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    send_result(server, request_id, json!({})).await
}

/// Handle `mcp.resources.list` — list available resources.
pub(super) async fn handle_mcp_resources_list(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    send_result(server, request_id, json!({"resources": []})).await
}

/// Handle `mcp.resources.read` — read a specific resource by URI.
pub(super) async fn handle_mcp_resources_read(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    send_error(
        server,
        request_id,
        -32602,
        "Resource reading via MCP bridge is not supported; use the dedicated MCP server instead"
            .to_string(),
        None,
    )
    .await
}

/// Handle `mcp.resources.subscribe` — subscribe to resource changes.
pub(super) async fn handle_mcp_resources_subscribe(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    send_result(server, request_id, json!({})).await
}

/// Handle `mcp.logging.setLevel` — set the MCP logging level.
pub(super) async fn handle_mcp_logging_set_level(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    send_result(server, request_id, json!({})).await
}

/// Handle `mcp.completion.complete` — complete a text input.
pub(super) async fn handle_mcp_completion_complete(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    send_result(server, request_id, json!({"completion": []})).await
}

/// Handle `mcp.sampling.createMessage` — create a sampling request.
pub(super) async fn handle_mcp_sampling_create_message(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    send_error(
        server,
        request_id,
        -32602,
        "Sampling via MCP bridge is not supported; use the chat/session API instead".to_string(),
        None,
    )
    .await
}

// ── MCP tool handlers ────────────────────────────────────────────────────

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

static SKILL_VERSION_HISTORY: OnceLock<StdMutex<HashMap<String, Vec<Value>>>> = OnceLock::new();

fn skill_version_history() -> &'static StdMutex<HashMap<String, Vec<Value>>> {
    SKILL_VERSION_HISTORY.get_or_init(|| StdMutex::new(HashMap::new()))
}

fn load_skill_manifest(path: &str) -> Result<Value> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read skill manifest {}", path))?;
    serde_json::from_str::<Value>(&raw)
        .with_context(|| format!("failed to parse skill manifest {}", path))
}

fn save_skill_manifest(path: &str, manifest: &Value) -> Result<()> {
    let payload = serde_json::to_string_pretty(manifest)
        .context("failed to serialize skill manifest payload")?;
    fs::write(path, payload).with_context(|| format!("failed to write skill manifest {}", path))
}

fn parse_semver_patch(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.trim().split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    let patch = parts.next()?.parse::<u64>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

fn bump_patch_version(version: &str) -> String {
    parse_semver_patch(version)
        .map(|(major, minor, patch)| format!("{}.{}.{}", major, minor, patch + 1))
        .unwrap_or_else(|| "1.0.0".to_string())
}

fn build_skill_version_snapshot(
    record: &ImportedSkillRecord,
    manifest: &Value,
    updated_by: &str,
    change_summary: &str,
) -> Value {
    let updated_at = crate::acp::prelude::now_ts();
    json!({
        "name": record.name,
        "version": manifest
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or(record.version.as_str()),
        "description": manifest
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or(record.description.as_str()),
        "input_schema": manifest.get("input_schema").cloned().unwrap_or_else(|| json!({"type":"object"})),
        "prompt_template": manifest.get("prompt_template").cloned().unwrap_or(Value::Null),
        "manifest_path": record.manifest_path,
        "saved_at": updated_at,
        "updated_at": updated_at,
        "updated_by": updated_by,
        "change_summary": change_summary,
    })
}

fn push_skill_version_snapshot(name: &str, snapshot: Value) {
    if let Ok(mut history) = skill_version_history().lock() {
        let entries = history.entry(name.to_string()).or_default();
        entries.push(snapshot);
        if entries.len() > 100 {
            let overflow = entries.len() - 100;
            entries.drain(0..overflow);
        }
    }
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

    // If the imported manifest carries a prompt_template (e.g. from SKILL.md),
    // also register it as a prompt-based skill in the SkillRegistry so that
    // the skill is actually executable (not just manifest-backed).
    if let Ok(manifest_value) = load_skill_manifest(&imported.manifest_path) {
        if let Some(prompt_template) = manifest_value
            .get("prompt_template")
            .and_then(Value::as_str)
            .map(|s| s.to_string())
        {
            let skill_name = manifest_value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(&imported.name)
                .to_string();
            let skill_description = manifest_value
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let input_schema = manifest_value
                .get("input_schema")
                .and_then(|v| v.as_object())
                .map(|obj| {
                    obj.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect::<std::collections::HashMap<String, String>>()
                })
                .unwrap_or_default();
            match server.skill_registry.lock() {
                Ok(mut registry) => {
                    if let Err(e) = registry.create_skill_from_prompt(
                        &skill_name,
                        &skill_description,
                        &prompt_template,
                        input_schema,
                    ) {
                        warn!("SKILL.md prompt-skill registration skipped: {}", e);
                    }
                }
                Err(e) => {
                    warn!(
                        "SKILL.md prompt-skill registration skipped (lock error): {}",
                        e
                    );
                }
            }
        }
    }

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
        let reason = tf("error.imported_skill_not_found", &[("name", &name)]);
        record_skill_admin_audit("remove", &name, false, &reason);
        return send_error(server, request_id, -32602, reason, None).await;
    }
    let unregistered = server
        .skill_registry
        .lock()
        .map(|mut registry| {
            let unregistered = registry.unregister(&name);
            // Persist prompt skill removal to disk
            if let Err(e) = registry.save_prompt_skills_to_disk() {
                tracing::warn!("Failed to persist prompt skills after removal: {}", e);
            }
            unregistered
        })
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

pub(super) async fn handle_skill_create(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let name = match parse_skill_name_param(&params) {
        Ok(name) => name,
        Err(err) => {
            record_skill_admin_audit("create", "skill.create", false, &err.to_string());
            return send_error(server, request_id, -32602, err.to_string(), None).await;
        }
    };
    let description = params
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| anyhow::anyhow!("missing required param: description"))?;
    let prompt_template = params
        .get("prompt_template")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| anyhow::anyhow!("missing required param: prompt_template"))?;
    let input_schema: std::collections::HashMap<String, String> = params
        .get("input_schema")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    let result = {
        let mut registry = server
            .skill_registry
            .lock()
            .map_err(|err| anyhow::anyhow!("skill registry lock error: {}", err))?;
        registry.create_skill_from_prompt(&name, &description, &prompt_template, input_schema)
    };
    // Lock is dropped before await
    if let Err(err) = result {
        record_skill_admin_audit("create", &name, false, &err.to_string());
        return send_error(server, request_id, -32602, err.to_string(), None).await;
    }

    record_skill_admin_audit("create", &name, true, "created skill from prompt template");
    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "name": name,
        }),
    )
    .await
}

pub(super) async fn handle_skill_update(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    match skill_update_payload(server, &params) {
        Ok(payload) => send_result(server, request_id, payload).await,
        Err(err) => send_error(server, request_id, -32602, err.to_string(), None).await,
    }
}

pub(super) fn skill_update_payload(server: &AcpServer, params: &Value) -> Result<Value> {
    let name = match parse_skill_name_param(params) {
        Ok(name) => name,
        Err(err) => {
            record_skill_admin_audit("update", "skill.update", false, &err.to_string());
            return Err(err);
        }
    };

    let mut store = open_skill_import_store(server)?;
    let Some(mut record) = store.get(&name) else {
        let reason = tf("error.imported_skill_not_found", &[("name", &name)]);
        record_skill_admin_audit("update", &name, false, &reason);
        anyhow::bail!(reason);
    };

    let mut manifest = load_skill_manifest(&record.manifest_path)?;
    push_skill_version_snapshot(
        &name,
        build_skill_version_snapshot(&record, &manifest, "system", "initial skill import"),
    );

    if let Some(description) = params.get("description").and_then(Value::as_str) {
        manifest["description"] = json!(description);
        record.description = description.to_string();
    }
    if let Some(schema) = params.get("input_schema") {
        manifest["input_schema"] = schema.clone();
    }
    if let Some(prompt_template) = params.get("prompt_template").and_then(Value::as_str) {
        manifest["prompt_template"] = json!(prompt_template);
    }

    let current_version = manifest
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or(record.version.as_str());
    let target_version = params
        .get("version")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| bump_patch_version(current_version));
    manifest["version"] = json!(target_version.clone());
    record.version = target_version;

    save_skill_manifest(&record.manifest_path, &manifest)?;
    push_skill_version_snapshot(
        &name,
        build_skill_version_snapshot(
            &record,
            &manifest,
            "system",
            "updated imported skill manifest",
        ),
    );

    store.upsert_record(record.clone());
    store.save()?;

    record_skill_admin_audit("update", &name, true, "updated imported skill manifest");
    Ok(json!({
        "ok": true,
        "action": "update",
        "name": name,
        "skill": normalize_imported_record(record),
    }))
}

pub(super) async fn handle_skill_version_list(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    match skill_version_list_payload(server, &params) {
        Ok(payload) => send_result(server, request_id, payload).await,
        Err(err) => send_error(server, request_id, -32602, err.to_string(), None).await,
    }
}

pub(super) fn skill_version_list_payload(server: &AcpServer, params: &Value) -> Result<Value> {
    let name = match parse_skill_name_param(params) {
        Ok(name) => name,
        Err(err) => {
            record_skill_admin_audit(
                "version.list",
                "skill.version.list",
                false,
                &err.to_string(),
            );
            return Err(err);
        }
    };

    let store = open_skill_import_store(server)?;
    let Some(record) = store.get(&name) else {
        let reason = tf("error.imported_skill_not_found", &[("name", &name)]);
        record_skill_admin_audit("version.list", &name, false, &reason);
        anyhow::bail!(reason);
    };

    let manifest = load_skill_manifest(&record.manifest_path)?;
    let mut versions = skill_version_history()
        .lock()
        .ok()
        .and_then(|history| history.get(&name).cloned())
        .unwrap_or_default();
    versions.push(build_skill_version_snapshot(
        &record,
        &manifest,
        "system",
        "current imported skill snapshot",
    ));

    record_skill_admin_audit("version.list", &name, true, "listed skill versions");
    Ok(json!({
        "ok": true,
        "name": name,
        "versions": versions,
    }))
}

pub(super) async fn handle_skill_version_rollback(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    match skill_version_rollback_payload(server, &params) {
        Ok(payload) => send_result(server, request_id, payload).await,
        Err(err) => send_error(server, request_id, -32602, err.to_string(), None).await,
    }
}

pub(super) fn skill_version_rollback_payload(server: &AcpServer, params: &Value) -> Result<Value> {
    let name = match parse_skill_name_param(params) {
        Ok(name) => name,
        Err(err) => {
            record_skill_admin_audit(
                "version.rollback",
                "skill.version.rollback",
                false,
                &err.to_string(),
            );
            return Err(err);
        }
    };
    let Some(target_version) = params.get("version").and_then(Value::as_str) else {
        anyhow::bail!("version is required");
    };

    let mut store = open_skill_import_store(server)?;
    let Some(mut record) = store.get(&name) else {
        let reason = tf("error.imported_skill_not_found", &[("name", &name)]);
        record_skill_admin_audit("version.rollback", &name, false, &reason);
        anyhow::bail!(reason);
    };

    let history = skill_version_history()
        .lock()
        .ok()
        .and_then(|entries| entries.get(&name).cloned())
        .unwrap_or_default();
    let Some(snapshot) = history.into_iter().rev().find(|entry| {
        entry
            .get("version")
            .and_then(Value::as_str)
            .map(|version| version == target_version)
            .unwrap_or(false)
    }) else {
        anyhow::bail!(
            "version '{}' not found for skill '{}'",
            target_version,
            name
        );
    };

    let mut manifest = load_skill_manifest(&record.manifest_path)?;
    if let Some(description) = snapshot.get("description") {
        manifest["description"] = description.clone();
        if let Some(text) = description.as_str() {
            record.description = text.to_string();
        }
    }
    if let Some(schema) = snapshot.get("input_schema") {
        manifest["input_schema"] = schema.clone();
    }
    if let Some(prompt_template) = snapshot.get("prompt_template") {
        manifest["prompt_template"] = prompt_template.clone();
    }
    manifest["version"] = json!(target_version);
    record.version = target_version.to_string();

    save_skill_manifest(&record.manifest_path, &manifest)?;
    store.upsert_record(record.clone());
    store.save()?;
    push_skill_version_snapshot(
        &name,
        build_skill_version_snapshot(
            &record,
            &manifest,
            "system",
            "rolled back imported skill version",
        ),
    );

    record_skill_admin_audit(
        "version.rollback",
        &name,
        true,
        "rolled back imported skill version",
    );
    Ok(json!({
        "ok": true,
        "action": "rollback",
        "name": name,
        "version": target_version,
        "skill": normalize_imported_record(record),
    }))
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

pub(super) async fn handle_models_list(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let models = server
        .agent_registry
        .as_ref()
        .map(|registry| {
            registry
                .models()
                .into_iter()
                .flat_map(|(provider_name, _default_model, models)| {
                    models.into_iter().map(move |m| {
                        json!({
                            "id": m.id,
                            "name": m.name,
                            "description": m.description,
                            "provider": provider_name.clone(),
                            "is_default": m.is_default,
                            "capabilities": m.capabilities,
                            "context_window": m.context_window,
                        })
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    send_result(
        server,
        request_id,
        json!({
            "models": models
        }),
    )
    .await
}
