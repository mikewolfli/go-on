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
        "github_search_skills" => McpTool {
            name: name.to_string(),
            description: Some(
                "Search GitHub for skill repositories matching a query. ".to_string()
                    + "Returns repos that may contain installable skills. "
                    + "Use 'import_skill' with the chosen repo to install.",
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Search query (e.g. 'web scraping', 'code review')"},
                    "max_results": {"type": "integer", "description": "Max results to return (1-20)", "default": 10, "minimum": 1, "maximum": 20}
                },
                "required": ["query"]
            })),
        },
        "import_skill" => McpTool {
            name: name.to_string(),
            description: Some(
                "Import a skill from a remote URL or GitHub repository. ".to_string()
                    + "Downloads the skill manifest and registers it locally. "
                    + "Supports GitHub repos (e.g. 'owner/repo') and direct URLs.",
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "source": {
                        "type": "object",
                        "oneOf": [
                            {
                                "title": "GitHub",
                                "type": "object",
                                "properties": {
                                    "repo": {"type": "string", "description": "GitHub repository (owner/repo)"},
                                    "ref": {"type": "string", "description": "Git ref (branch/tag/commit), default: main", "default": "main"},
                                    "path": {"type": "string", "description": "Path within the repo"}
                                },
                                "required": ["repo"]
                            },
                            {
                                "title": "URL",
                                "type": "object",
                                "properties": {
                                    "url": {"type": "string", "description": "Direct URL to the skill manifest JSON"}
                                },
                                "required": ["url"]
                            }
                        ],
                        "description": "Source of the skill to import"
                    }
                },
                "required": ["source"]
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
        "github_search_skills" => {
            tool_input
                .get("query")
                .and_then(|value| value.as_str())
                .ok_or_else(|| anyhow::anyhow!("github_search_skills requires arguments.query"))?;
        }
        "workflow_execute" | "workflow_ask" | "workflow_generate"
            if tool_input.get("task").is_none() =>
        {
            return Err(anyhow::anyhow!("{} tool requires 'task' field", tool_name));
        }
        "workflow_execute" | "workflow_ask" | "workflow_generate" => {}
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// List of all known built-in tool names.
    const KNOWN_TOOLS: &[&str] = &[
        "read_file",
        "write_file",
        "search_files",
        "apply_patch",
        "run_tests",
        "inspect_git_diff",
        "workflow_execute",
        "workflow_ask",
        "workflow_generate",
        "skill_creator",
        "github_search_skills",
        "import_skill",
    ];

    // ── Known tool descriptors ───────────────────────────────────────

    /// Verify that known tools have valid descriptors with name, description,
    /// and input_schema populated.
    #[test]
    fn test_known_tools_have_valid_descriptors() {
        for &name in KNOWN_TOOLS {
            let desc = tool_descriptor(name);
            assert_eq!(desc.name, name, "descriptor name should match for {}", name);
            assert!(
                desc.description.is_some(),
                "descriptor for {} should have a description",
                name
            );
            assert!(
                desc.description.as_deref().unwrap().len() > 5,
                "description for {} should be meaningful (length > 5)",
                name
            );
            assert!(
                desc.input_schema.is_some(),
                "descriptor for {} should have input_schema",
                name
            );
        }
    }

    /// Verify that tools with required arguments are properly reflected.
    #[test]
    fn test_tool_descriptors_have_correct_schema_format() {
        let desc = tool_descriptor("read_file");
        let schema = desc.input_schema.unwrap();
        assert_eq!(schema["type"], "object");
        let required = schema["required"].as_array().unwrap();
        assert!(
            required.contains(&serde_json::Value::String("path".to_string())),
            "read_file should require 'path'"
        );

        let desc = tool_descriptor("write_file");
        let schema = desc.input_schema.unwrap();
        let required = schema["required"].as_array().unwrap();
        assert!(
            required.contains(&serde_json::Value::String("path".to_string())),
            "write_file should require 'path'"
        );
        assert!(
            required.contains(&serde_json::Value::String("content".to_string())),
            "write_file should require 'content'"
        );
    }

    // ── validate_required_arguments ──────────────────────────────────

    /// Verify that `validate_required_arguments` passes for known tools
    /// with correct inputs.
    #[test]
    fn test_validate_required_arguments_known_tools() {
        // read_file requires path
        assert!(validate_required_arguments("read_file", &json!({"path": "foo.txt"})).is_ok());
        // write_file requires path + content
        assert!(validate_required_arguments(
            "write_file",
            &json!({"path": "foo.txt", "content": "hello"})
        )
        .is_ok());
        // search_files requires pattern
        assert!(validate_required_arguments("search_files", &json!({"pattern": "*.rs"})).is_ok());
        // workflow_execute requires task
        assert!(
            validate_required_arguments("workflow_execute", &json!({"task": "do something"}))
                .is_ok()
        );
        // workflow_ask requires task
        assert!(
            validate_required_arguments("workflow_ask", &json!({"task": "analyze this"})).is_ok()
        );
        // workflow_generate requires task
        assert!(
            validate_required_arguments("workflow_generate", &json!({"task": "plan this"})).is_ok()
        );
    }

    /// Verify that `validate_required_arguments` rejects missing arguments.
    #[test]
    fn test_validate_required_arguments_missing() {
        // read_file without path
        let err = validate_required_arguments("read_file", &json!({})).unwrap_err();
        assert!(err
            .to_string()
            .contains("read_file requires arguments.path"));

        // write_file without content
        let err = validate_required_arguments("write_file", &json!({"path": "x.txt"})).unwrap_err();
        assert!(err
            .to_string()
            .contains("write_file requires arguments.content"));

        // search_files without pattern
        let err = validate_required_arguments("search_files", &json!({})).unwrap_err();
        assert!(err
            .to_string()
            .contains("search_files requires arguments.pattern"));
    }

    /// Verify that unknown tools are validated successfully (no required args).
    #[test]
    fn test_validate_required_arguments_unknown_tool() {
        // Unknown tools have no validation rules, so any input should pass
        assert!(validate_required_arguments("unknown_tool", &json!({})).is_ok());
        assert!(validate_required_arguments("unknown_tool", &json!({"anything": 42})).is_ok());
    }

    // ── Unknown tools ────────────────────────────────────────────────

    /// Verify that unknown tools get a generic descriptor with "Registered MCP tool"
    /// description and an empty object schema.
    #[test]
    fn test_unknown_tool_gets_generic_descriptor() {
        let desc = tool_descriptor("some_unknown_tool");
        assert_eq!(desc.name, "some_unknown_tool");
        assert_eq!(
            desc.description.as_deref(),
            Some("Registered MCP tool"),
            "unknown tools should get a generic description"
        );
        let schema = desc.input_schema.unwrap();
        assert_eq!(schema["type"], "object");
    }

    /// Verify that `tool_descriptor_value` also returns generic structure for unknown tools.
    #[test]
    fn test_unknown_tool_descriptor_value() {
        let val = tool_descriptor_value("nonexistent");
        assert_eq!(val["name"], "nonexistent");
        assert_eq!(val["description"], "Registered MCP tool");
    }
}
