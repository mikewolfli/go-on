use anyhow::Result;
use serde_json::{json, Value};

use super::McpTool;

pub(crate) fn tool_descriptor(name: &'static str) -> McpTool {
    match name {
        "read_file" => McpTool {
            name: name.to_string(),
            description: Some("Read contents of a file".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path to read"}
                },
                "required": ["path"]
            })),
        },
        "write_file" => McpTool {
            name: name.to_string(),
            description: Some("Write contents to a file".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "File path to write"},
                    "content": {"type": "string", "description": "Content to write"},
                    "mode": {"type": "string", "enum": ["overwrite", "append"], "description": "Write mode"}
                },
                "required": ["path", "content"]
            })),
        },
        "search_files" => McpTool {
            name: name.to_string(),
            description: Some("Search for files matching a glob pattern".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "pattern": {"type": "string", "description": "Search pattern/glob"},
                    "directory": {"type": "string", "description": "Search directory"}
                },
                "required": ["pattern"]
            })),
        },
        "apply_patch" => McpTool {
            name: name.to_string(),
            description: Some("Apply a patch artifact".to_string()),
            input_schema: Some(json!({"type": "object"})),
        },
        "run_tests" => McpTool {
            name: name.to_string(),
            description: Some("Run test suite".to_string()),
            input_schema: Some(json!({"type": "object"})),
        },
        "inspect_git_diff" => McpTool {
            name: name.to_string(),
            description: Some("Inspect git diff".to_string()),
            input_schema: Some(json!({"type": "object"})),
        },
        other => McpTool {
            name: other.to_string(),
            description: Some("Registered MCP tool".to_string()),
            input_schema: Some(json!({"type": "object"})),
        },
    }
}

pub(crate) fn validate_required_arguments(tool_name: &str, tool_input: &Value) -> Result<()> {
    match tool_name {
        "read_file" => {
            tool_input
                .get("path")
                .and_then(|value| value.as_str())
                .ok_or_else(|| anyhow::anyhow!("read_file requires arguments.path"))?;
        }
        "write_file" => {
            tool_input
                .get("path")
                .and_then(|value| value.as_str())
                .ok_or_else(|| anyhow::anyhow!("write_file requires arguments.path"))?;
            tool_input
                .get("content")
                .and_then(|value| value.as_str())
                .ok_or_else(|| anyhow::anyhow!("write_file requires arguments.content"))?;
        }
        "search_files" => {
            tool_input
                .get("pattern")
                .and_then(|value| value.as_str())
                .ok_or_else(|| anyhow::anyhow!("search_files requires arguments.pattern"))?;
        }
        _ => {}
    }
    Ok(())
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
