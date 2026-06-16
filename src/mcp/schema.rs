use serde::{Deserialize, Serialize};
use serde_json::Value;
/// JSON-RPC protocol version string.
pub const JSONRPC_VERSION: &str = "2.0";

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

    pub fn with_is_error(mut self, is_error: bool) -> Self {
        self.is_error = Some(is_error);
        self
    }
}
