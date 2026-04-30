use anyhow::Result;
use serde_json::Value;

use crate::shared::tool_descriptors;

pub(crate) fn tool_descriptor(name: &'static str) -> crate::mcp::McpTool {
    tool_descriptors::tool_descriptor(name)
}

pub(crate) fn validate_required_arguments(tool_name: &str, tool_input: &Value) -> Result<()> {
    tool_descriptors::validate_required_arguments(tool_name, tool_input)
}

pub mod error_codes {
    /// JSON-RPC Parse error
    pub const PARSE_ERROR: i32 = -32700;
    /// JSON-RPC Invalid Request
    #[allow(dead_code)] // F-GAP-10 — reserved for future MCP error handling
    pub const INVALID_REQUEST: i32 = -32600;
    /// JSON-RPC Method not found
    pub const METHOD_NOT_FOUND: i32 = -32601;
    /// JSON-RPC Invalid params
    pub const INVALID_PARAMS: i32 = -32602;
    /// JSON-RPC Internal error
    pub const INTERNAL_ERROR: i32 = -32603;
    /// Server error start range
    #[allow(dead_code)] // F-GAP-10 — reserved for future MCP error handling
    pub const SERVER_ERROR_START: i32 = -32099;
    /// Server error end range
    #[allow(dead_code)] // F-GAP-10 — reserved for future MCP error handling
    pub const SERVER_ERROR_END: i32 = -32000;
}
