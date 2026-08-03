use super::*;

// ── MCP bridge handlers (mcp.* methods routed through ACP dispatch) ──────

/// Handle `mcp.ping` — health check ping.
pub async fn mcp_ping_payload(_server: &AcpServer) -> Result<Value> {
    Ok(serde_json::Value::Object(serde_json::Map::new()))
}

/// Handle `mcp.resources.list` — list available resources.
pub async fn mcp_resources_list_payload(_server: &AcpServer) -> Result<Value> {
    use crate::mcp::McpListResourcesResult;
    let result = McpListResourcesResult::new(vec![]);
    Ok(serde_json::to_value(&result)?)
}

/// Handle `mcp.resources.read` — read a specific resource by URI.
pub async fn mcp_resources_read_payload(_server: &AcpServer, _params: Value) -> Result<Value> {
    Err(anyhow::anyhow!(
        "Resource reading via MCP bridge is not supported; use the dedicated MCP server instead"
    ))
}

/// Handle `mcp.resources.subscribe` — subscribe to resource changes.
pub async fn mcp_resources_subscribe_payload(_server: &AcpServer, _params: Value) -> Result<Value> {
    Ok(serde_json::Value::Object(serde_json::Map::new()))
}

/// Handle `mcp.logging.setLevel` — set the MCP logging level.
pub async fn mcp_logging_set_level_payload(_server: &AcpServer, _params: Value) -> Result<Value> {
    Ok(serde_json::Value::Object(serde_json::Map::new()))
}

/// Handle `mcp.completion.complete` — complete a text input.
pub async fn mcp_completion_complete_payload(_server: &AcpServer, _params: Value) -> Result<Value> {
    Ok(serde_json::Value::Object(serde_json::Map::new()))
}

/// Handle `mcp.sampling.createMessage` — create a sampling request.
pub async fn mcp_sampling_create_message_payload(
    _server: &AcpServer,
    _params: Value,
) -> Result<Value> {
    Err(anyhow::anyhow!(
        "Sampling via MCP bridge is not supported; use the chat/session API instead"
    ))
}

// ── MCP tool handlers ────────────────────────────────────────────────────

pub async fn mcp_tools_list_payload(server: &AcpServer) -> Result<Value> {
    use crate::mcp::McpListToolsResult;
    let tools = build_mcp_tool_descriptors(Some(server));
    let result = McpListToolsResult::new(tools);
    let value = serde_json::to_value(&result)?;
    let method = super::super::DISPATCH_REQUEST_METHOD
        .try_with(|m| m.clone())
        .unwrap_or_else(|_| "mcp.tools.list".to_string());
    let value = super::super::inject_platform_profiles_if_absent(value, &method);
    Ok(value)
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
    use crate::mcp::McpListToolsResult;
    let tools = build_mcp_tool_descriptors(Some(server));
    let result = McpListToolsResult::new(tools);
    let value = serde_json::to_value(&result)?;
    let method = super::super::DISPATCH_REQUEST_METHOD
        .try_with(|m| m.clone())
        .unwrap_or_else(|_| "tools.list".to_string());
    let value = super::super::inject_platform_profiles_if_absent(value, &method);
    Ok(value)
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
