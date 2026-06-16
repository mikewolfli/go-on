use anyhow::Result;
use serde_json::Value;

use crate::shared::tool_descriptors;

pub(crate) fn validate_required_arguments(tool_name: &str, tool_input: &Value) -> Result<()> {
    tool_descriptors::validate_required_arguments(tool_name, tool_input)
}

pub mod error_codes {
    /// JSON-RPC Parse error
    pub const PARSE_ERROR: i32 = -32700;
    /// JSON-RPC Invalid Request
    pub const INVALID_REQUEST: i32 = -32600;
    /// JSON-RPC Method not found
    pub const METHOD_NOT_FOUND: i32 = -32601;
    /// JSON-RPC Invalid params
    pub const INVALID_PARAMS: i32 = -32602;
    /// JSON-RPC Internal error
    pub const INTERNAL_ERROR: i32 = -32603;
    /// Request cancelled by client notification.
    pub const REQUEST_CANCELLED: i32 = -32800;
    /// Request timed out before producing a result.
    pub const REQUEST_TIMEOUT: i32 = -32801;
}
