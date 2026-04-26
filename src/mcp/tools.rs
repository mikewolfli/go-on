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
    pub const PARSE_ERROR: i32 = -32700;
    #[allow(dead_code)]
    pub const INVALID_REQUEST: i32 = -32600;
    #[allow(dead_code)]
    pub const METHOD_NOT_FOUND: i32 = -32601;
    #[allow(dead_code)]
    pub const INVALID_PARAMS: i32 = -32602;
    pub const INTERNAL_ERROR: i32 = -32603;
    #[allow(dead_code)]
    pub const SERVER_ERROR_START: i32 = -32099;
    #[allow(dead_code)]
    pub const SERVER_ERROR_END: i32 = -32000;
}
