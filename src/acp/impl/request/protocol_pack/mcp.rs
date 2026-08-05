use super::*;

// ── MCP bridge handlers (mcp.* methods routed through ACP dispatch) ──────

/// Handle `mcp.ping` — health check ping.
pub async fn mcp_ping_payload(_server: &AcpServer) -> Result<Value> {
    Ok(serde_json::Value::Object(serde_json::Map::new()))
}

/// Handle `mcp.resources.list` — list available resources.
///
/// Mirrors the native MCP handler (`go-on://agents` + `go-on://tools`) so the
/// ACP bridge and the dedicated MCP server cannot drift.
pub async fn mcp_resources_list_payload(_server: &AcpServer) -> Result<Value> {
    use crate::mcp::{McpListResourcesResult, McpResource};
    let resources: Vec<Value> = vec![
        serde_json::to_value(McpResource {
            uri: "go-on://agents".to_string(),
            name: "Available Agents".to_string(),
            description: Some("List of deployed agents".to_string()),
            mime_type: "application/json".to_string(),
        })?,
        serde_json::to_value(McpResource {
            uri: "go-on://tools".to_string(),
            name: "Available Tools".to_string(),
            description: Some("List of available tools".to_string()),
            mime_type: "application/json".to_string(),
        })?,
    ];
    Ok(serde_json::to_value(McpListResourcesResult::new(
        resources,
    ))?)
}

/// Handle `mcp.resources.read` — read a specific resource by URI.
///
/// Mirrors the native `go-on://agents` / `go-on://tools` resources.
pub async fn mcp_resources_read_payload(server: &AcpServer, params: Value) -> Result<Value> {
    let uri = params
        .get("uri")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("Missing URI for mcp.resources.read"))?;
    match uri {
        "go-on://agents" => {
            let agents = server
                .agent_registry()
                .map(|r| r.names())
                .unwrap_or_default();
            Ok(json!({
                "contents": [{
                    "uri": "go-on://agents",
                    "mimeType": "application/json",
                    "text": serde_json::to_string(&json!({"agents": agents}))?
                }]
            }))
        }
        "go-on://tools" => {
            let tools: Vec<&str> = server.tool_registry().names();
            Ok(json!({
                "contents": [{
                    "uri": "go-on://tools",
                    "mimeType": "application/json",
                    "text": serde_json::to_string(&json!({"tools": tools}))?
                }]
            }))
        }
        _ => Err(anyhow::anyhow!("Unknown MCP resource: {}", uri)),
    }
}

/// Handle `mcp.resources.subscribe` — subscription is not supported.
///
/// The native handler rejects subscription with METHOD_NOT_FOUND and nothing
/// ever emits change notifications, so returning a fake `{}` success here
/// would be declaration/implementation drift. Honest error instead.
pub async fn mcp_resources_subscribe_payload(_server: &AcpServer, _params: Value) -> Result<Value> {
    Err(anyhow::anyhow!(
        "Resource subscription is not supported; resources are static"
    ))
}

/// Handle `mcp.logging.setLevel` — not supported on the bridge transport.
///
/// The dedicated MCP server implements real log-level switching; the ACP
/// bridge does not own an `McpServer` instance, so an honest error is returned
/// instead of a fake success.
pub async fn mcp_logging_set_level_payload(_server: &AcpServer, _params: Value) -> Result<Value> {
    Err(anyhow::anyhow!(
        "Logging level control is not supported on the ACP bridge; use the dedicated MCP server"
    ))
}

/// Handle `mcp.completion.complete` — not supported on the bridge transport.
///
/// The dedicated MCP server implements real argument completion; the ACP
/// bridge returns an honest error instead of a fake success.
pub async fn mcp_completion_complete_payload(_server: &AcpServer, _params: Value) -> Result<Value> {
    Err(anyhow::anyhow!(
        "Completion is not supported on the ACP bridge; use the dedicated MCP server"
    ))
}

/// Handle `mcp.sampling.createMessage` — create a sampling request.
///
/// `sampling` is deliberately NOT advertised in `mcp_initialize_capabilities()`;
/// the error keeps declaration and implementation aligned.
pub async fn mcp_sampling_create_message_payload(
    _server: &AcpServer,
    _params: Value,
) -> Result<Value> {
    Err(anyhow::anyhow!(
        "Sampling via MCP bridge is not supported; use the chat/session API instead"
    ))
}

/// Handle `mcp.prompts.list` — list discoverable prompt templates.
///
/// Mirrors the native MCP `prompts/list` shape: `template://<cat>.<id>` entries
/// from the ACP prompt manager plus `agent://<name>` entries from the agent
/// registry.
pub async fn mcp_prompts_list_payload(server: &AcpServer, params: Value) -> Result<Value> {
    let lang = params.get("lang").and_then(Value::as_str).unwrap_or("en");
    let mut prompts: Vec<Value> = Vec::new();

    if let Ok(collection) = server.prompt_manager.get_all_templates(lang) {
        for category in collection.categories {
            for template in category.templates {
                prompts.push(json!({
                    "name": format!("template://{}.{}", category.id, template.id),
                    "description": template.description,
                    "arguments": [
                        {
                            "name": "input",
                            "description": "Optional input for replacing {{input}} placeholder",
                            "required": false
                        }
                    ]
                }));
            }
        }
    }

    if let Some(registry) = server.agent_registry() {
        for name in registry.names() {
            prompts.push(json!({
                "name": format!("agent://{}", name),
                "description": format!("System prompt for '{}' agent", name),
                "arguments": []
            }));
        }
    }

    Ok(json!({ "prompts": prompts }))
}

/// Handle `mcp.prompts.get` — resolve a prompt template by name.
///
/// Supports `template://<cat>.<id>` (via the prompt manager) and
/// `agent://<name>` (agent system prompt), matching the native handler.
pub async fn mcp_prompts_get_payload(server: &AcpServer, params: Value) -> Result<Value> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if name.is_empty() {
        anyhow::bail!("Missing 'name' for mcp.prompts.get");
    }
    let lang = params.get("lang").and_then(Value::as_str).unwrap_or("en");

    if let Some(template_name) = name.strip_prefix("template://") {
        let normalized = template_name.trim().trim_start_matches('/');
        if let Some((cat_id, cat_name, tpl)) = server.prompt_manager.get_template(lang, normalized)
        {
            let input = params
                .get("arguments")
                .and_then(|a| a.get("input"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            let content = if tpl.content.contains("{{input}}") {
                tpl.content.replace("{{input}}", input)
            } else if input.is_empty() {
                tpl.content
            } else {
                format!("{}\n\n{}", tpl.content, input)
            };
            return Ok(json!({
                "description": format!("Template prompt '{}.{}'", cat_id, tpl.id),
                "messages": [
                    {
                        "role": "system",
                        "content": content
                    }
                ],
                "template": {
                    "category_id": cat_id,
                    "category_name": cat_name,
                    "id": tpl.id,
                    "title": tpl.title,
                }
            }));
        }
        anyhow::bail!("Prompt template '{}' not found", template_name);
    }

    if let Some(agent_name) = name.strip_prefix("agent://") {
        if let Some(registry) = server.agent_registry() {
            if registry.get(agent_name).is_some() {
                return Ok(json!({
                    "description": format!("Agent system prompt for '{}'", agent_name),
                    "messages": [
                        {
                            "role": "system",
                            "content": format!("You are a '{}' agent providing AI assistance.", agent_name)
                        },
                        {
                            "role": "user",
                            "content": "Hello!"
                        }
                    ]
                }));
            }
        }
        anyhow::bail!("Agent '{}' not found", agent_name);
    }

    anyhow::bail!("Prompt '{}' not found", name)
}

// ── MCP tool handlers ────────────────────────────────────────────────────

/// Shared implementation for `mcp.tools.list` / `tools.list` — identical
/// except for the fallback method label used for platform-profile injection.
async fn tools_list_payload_impl(server: &AcpServer, default_method: &str) -> Result<Value> {
    use crate::mcp::McpListToolsResult;
    let tools = build_mcp_tool_descriptors(Some(server));
    let result = McpListToolsResult::new(tools);
    let value = serde_json::to_value(&result)?;
    let method = super::super::DISPATCH_REQUEST_METHOD
        .try_with(|m| m.clone())
        .unwrap_or_else(|_| default_method.to_string());
    Ok(super::super::inject_platform_profiles_if_absent(
        value, &method,
    ))
}

pub async fn mcp_tools_list_payload(server: &AcpServer) -> Result<Value> {
    tools_list_payload_impl(server, "mcp.tools.list").await
}

pub async fn mcp_tools_call_payload(server: &AcpServer, params: Value) -> Result<Value> {
    use crate::mcp::McpCallToolResult;

    let name = params
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or_default();

    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));

    let structured = match execute_mcp_tool_call(server, name, &arguments).await {
        Ok(structured) => structured,
        Err(err) => {
            record_mcp_tool_audit(name, &arguments, false, &err.to_string());
            return Err(anyhow::anyhow!(err.to_string()));
        }
    };
    record_mcp_tool_audit(name, &arguments, true, "tool executed successfully");

    let mut content = serde_json::Map::new();
    content.insert("type".to_string(), Value::String("text".to_string()));
    content.insert("text".to_string(), Value::String(structured.to_string()));
    let result = McpCallToolResult::new(vec![Value::Object(content)], Some(structured));
    Ok(serde_json::to_value(&result)?)
}

// ── ACP tool handlers ────────────────────────────────────────────────────

pub async fn acp_tools_list_payload(server: &AcpServer) -> Result<Value> {
    tools_list_payload_impl(server, "tools.list").await
}

/// Handle `tools/call` — execute a tool by name via the ACP protocol with
/// streaming progress updates.
pub async fn acp_tools_call_payload(server: &AcpServer, params: Value) -> Result<Value> {
    let name = params
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or_default();

    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));

    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .map(|s| s.to_string());

    // ── Send "started" progress update ──────────────────────────────────
    if let Some(ref sid) = session_id {
        let msg = format!("🔧 **{}** — executing...", name);
        super::session::send_chunk(server, sid, "agent_message_chunk", &msg).await;
    }

    // ── Execute the tool ────────────────────────────────────────────────
    let structured = match execute_mcp_tool_call(server, name, &arguments).await {
        Ok(structured) => structured,
        Err(err) => {
            record_mcp_tool_audit(name, &arguments, false, &err.to_string());
            if let Some(ref sid) = session_id {
                let msg = format!("❌ **{}** failed: {}", name, err);
                super::session::send_chunk(server, sid, "agent_message_chunk", &msg).await;
            }
            return Err(anyhow::anyhow!(err.to_string()));
        }
    };
    record_mcp_tool_audit(name, &arguments, true, "tool executed successfully");

    // ── Send "completed" progress update ────────────────────────────────
    if let Some(ref sid) = session_id {
        let msg = format!("✅ **{}** — completed", name);
        super::session::send_chunk(server, sid, "agent_message_chunk", &msg).await;
    }

    let mut content = serde_json::Map::new();
    content.insert("type".to_string(), Value::String("text".to_string()));
    content.insert("text".to_string(), Value::String(structured.to_string()));

    Ok(serde_json::json!({
        "content": [Value::Object(content)],
        "structured": structured
    }))
}

// ── Audit helpers for tool calls ────────────────────────────────────────

pub fn record_mcp_tool_audit(name: &str, arguments: &Value, success: bool, reason: &str) {
    record_tool_call_audit_with_protocol(name, arguments, success, reason, "acp_stdio");
}

pub fn record_tool_call_audit_with_protocol(
    name: &str,
    arguments: &Value,
    success: bool,
    reason: &str,
    protocol: &str,
) {
    use crate::governance::hardening::AutonomousEditAuditEntry;
    let action = governance_action_for_tool(name);
    let reversible = matches!(
        action,
        crate::governance::hardening::GovernanceAction::Read
            | crate::governance::hardening::GovernanceAction::Search
    );
    let file_path = super::audit::audit_file_path_from_arguments(name, arguments);
    let entry = AutonomousEditAuditEntry {
        timestamp: crate::acp::prelude::now_ts().to_string(),
        agent: "mcp.tools.call".to_string(),
        file_path,
        change_summary: format!(
            "tool={} action={} status={} protocol={}",
            name,
            super::audit::governance_action_label(action),
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
