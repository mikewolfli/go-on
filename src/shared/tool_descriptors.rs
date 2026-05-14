//! Shared tool descriptor and validation functions.
//!
//! These functions are shared between the ACP request handler and the MCP tools
//! module to eliminate code duplication for built-in tool descriptors and
//! argument validation.

use anyhow::Result;
use serde_json::{json, Value};

use crate::mcp::McpTool;

/// Get the MCP tool descriptor for a given built-in tool name.
///
/// Returns a fully populated `McpTool` with name, description, and input schema.
/// For unknown tool names, returns a generic descriptor.
pub fn tool_descriptor(name: &'static str) -> McpTool {
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
        "workflow_execute" => McpTool {
            name: name.to_string(),
            description: Some("Execute a workflow with the given task description".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "task": {"type": "string", "description": "Task description for the workflow"},
                    "phase": {"type": "string", "description": "Optional phase name (default: coding)"}
                },
                "required": ["task"]
            })),
        },
        "workflow_ask" => McpTool {
            name: name.to_string(),
            description: Some(
                "Ask the AI to analyze a task, create necessary skills, and execute a workflow"
                    .to_string(),
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "task": {"type": "string", "description": "Natural language task description"},
                    "auto_create_skills": {"type": "boolean", "description": "Auto-create skills for workflow nodes"},
                },
                "required": ["task"]
            })),
        },
        "workflow_generate" => McpTool {
            name: name.to_string(),
            description: Some(
                "Generate a workflow plan from a task description without executing it".to_string(),
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "task": {"type": "string", "description": "Task description to plan"},
                },
                "required": ["task"]
            })),
        },
        "skill_creator" => McpTool {
            name: name.to_string(),
            description: Some(
                "Create a new reusable skill from a prompt template (SKILL-CREATOR)".to_string(),
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Skill name"},
                    "description": {"type": "string", "description": "Skill description"},
                    "prompt_template": {"type": "string", "description": "Prompt template for the skill"},
                    "input_schema": {"type": "object", "description": "JSON schema for skill input"}
                },
                "required": ["name", "description", "prompt_template"]
            })),
        },
        other => McpTool {
            name: other.to_string(),
            description: Some("Registered MCP tool".to_string()),
            input_schema: Some(json!({"type": "object"})),
        },
    }
}

/// Get the JSON `Value` representation of a tool descriptor (used by ACP handler).
///
/// This is a convenience wrapper around `tool_descriptor` that returns the
/// serialized JSON value. It is used by the ACP request handler in `request.rs`
/// for building MCP tool descriptor lists.
pub fn tool_descriptor_value(name: &'static str) -> Value {
    let tool = tool_descriptor(name);
    serde_json::to_value(tool).unwrap_or_else(|_| {
        json!({
            "name": name,
            "description": "Registered MCP tool",
            "input_schema": {"type": "object"}
        })
    })
}

/// Validate required arguments for a built-in tool.
///
/// Checks that the tool's required arguments are present in the provided input.
/// Returns an error with a descriptive message if any required argument is missing.
pub fn validate_required_arguments(tool_name: &str, tool_input: &Value) -> Result<()> {
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
        "workflow_execute" | "workflow_ask" | "workflow_generate" | "skill_creator" => {
            if tool_name.starts_with("workflow") && tool_input.get("task").is_none() {
                return Err(anyhow::anyhow!("{} tool requires 'task' field", tool_name));
            }
        }
        _ => {
            return Err(anyhow::anyhow!("unknown tool '{}'", tool_name));
        }
    }
    Ok(())
}
