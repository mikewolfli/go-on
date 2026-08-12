use serde::{Deserialize, Serialize};
use serde_json::Value;
/// JSON-RPC protocol version string.
pub const JSONRPC_VERSION: &str = "2.0";

/// Human-readable text for an MCP tool result: prefers the structured
/// payload's `message` string (set by the workflow tools), otherwise falls
/// back to the serialized JSON payload.
///
/// Shared by the native MCP arm and the ACP bridge (`acp.mcp.tools.call`)
/// so both entry points return the same text shape for the same tool.
pub fn mcp_tool_result_text(structured: &Value) -> String {
    structured
        .get("message")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| serde_json::to_string(structured).unwrap_or_default())
}

// NOTE: MCP response types (McpInitializeResult, McpListResourcesResult, etc.)
// are defined at the bottom of this file — no separate module needed.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub method: String,
    pub params: Option<Value>,
    pub id: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpTool {
    pub name: String,
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpResource {
    pub uri: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub mime_type: String,
}

// ═══════════════════════════════════════════════════════════════════════════════
// MCP response/result types — used in ACP↔MCP bridge handlers
// ═══════════════════════════════════════════════════════════════════════════════

/// Response type for `mcp.initialize` — the result field of a successful
/// initialize request, following the MCP spec.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpInitializeResult {
    pub protocol_version: String,
    pub capabilities: Value,
    pub server_info: ServerInfo,
}
impl McpInitializeResult {
    pub fn new(
        protocol_version: impl Into<String>,
        capabilities: Value,
        server_info: ServerInfo,
    ) -> Self {
        Self {
            protocol_version: protocol_version.into(),
            capabilities,
            server_info,
        }
    }
}

/// The capabilities advertised by `mcp.initialize`. Single source of truth
/// shared by the native MCP handler and the ACP bridge (`mcp_initialize_payload`)
/// so the two entry points cannot drift.
///
/// Each advertised capability must have a live handler behind it in BOTH
/// entry points:
/// - `tools` / `resources` / `prompts`: list+read endpoints exist in the
///   native handler and the ACP bridge (round-22 parity closure).
/// - `sampling` is deliberately NOT advertised: the native MCP handler
///   implements `mcp.sampling.createMessage`, but the ACP bridge does not
///   expose a sampling backend, so advertising it would over-promise for
///   bridge-routed requests.
/// - No change-notification event source exists for resources, tools, or
///   prompts (lists are static), so `listChanged` is NOT advertised — a server
///   must not declare listChanged when it never sends the corresponding
///   notifications. The base capability keys are still declared so clients
///   know the endpoints exist.
pub(crate) fn mcp_initialize_capabilities() -> Value {
    serde_json::json!({
        "experimental": {
            "agents": {}
        },
        "resources": {},
        "tools": {},
        "prompts": {},
    })
}

/// Response type for `mcp.resources.list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpListResourcesResult {
    pub resources: Vec<Value>,
}
impl McpListResourcesResult {
    pub fn new(resources: Vec<Value>) -> Self {
        Self { resources }
    }
}

/// Response type for `mcp.tools.list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpListToolsResult {
    pub tools: Vec<Value>,
}
impl McpListToolsResult {
    pub fn new(tools: Vec<Value>) -> Self {
        Self { tools }
    }
}

/// Response type for `mcp.tools.call`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCallToolResult {
    pub content: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}
impl McpCallToolResult {
    pub fn new(content: Vec<Value>, structured_content: Option<Value>) -> Self {
        Self {
            content,
            structured_content,
            is_error: None,
        }
    }
}
