//! Shared tool descriptor and validation functions.
//!
//! These functions are shared between the ACP request handler and the MCP tools
//! module to eliminate code duplication for built-in tool descriptors and
//! argument validation.

use serde_json::{json, Value};

use crate::mcp::McpTool;

mod build;
mod cad;
mod core;
mod data;
mod docs;
mod extended;
mod game;
mod media;
mod validate;

pub use validate::validate_required_arguments;

/// Get the MCP tool descriptor for a given built-in tool name.
///
/// Returns a fully populated `McpTool` with name, description, and input schema.
/// For unknown tool names, returns a generic descriptor.
pub fn tool_descriptor(name: &str) -> McpTool {
    core::descriptor(name)
        .or_else(|| extended::descriptor(name))
        .or_else(|| build::descriptor(name))
        .or_else(|| cad::descriptor(name))
        .or_else(|| docs::descriptor(name))
        .or_else(|| data::descriptor(name))
        .or_else(|| media::descriptor(name))
        .or_else(|| game::descriptor(name))
        .unwrap_or_else(|| McpTool {
            name: name.to_string(),
            description: Some("Registered MCP tool".to_string()),
            input_schema: Some(json!({"type": "object"})),
        })
}

/// Get the JSON `Value` representation of a tool descriptor (used by ACP handler).
///
/// This is a convenience wrapper around `tool_descriptor` that returns the
/// serialized JSON value. It is used by the ACP request handler in `request.rs`
/// for building MCP tool descriptor lists.
pub fn tool_descriptor_value(name: &str) -> Value {
    let tool = tool_descriptor(name);
    serde_json::to_value(tool).unwrap_or_else(|_| {
        json!({
            "name": name,
            "description": "Registered MCP tool",
            "input_schema": {"type": "object"}
        })
    })
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
        "skill-creator",
        "github_search_skills",
        "import_skill",
        // Extended tools with full descriptors
        "shell_exec",
        "http_request",
        "grep",
        "find_files",
        "git",
        "list_directory",
        "move_path",
        "delete_path",
        "edit_file",
        "cargo_check",
        "cargo_test",
        "compress",
        "decompress",
        "date_time",
        "dns_lookup",
        "ping",
        "port_scan",
        // Skill tools
        "skill_execute",
        "skill_list",
        // Round 2 additions
        "diagnostics",
        "environment_info",
        // Web search
        "web_search",
        // LSP-like code intelligence tools
        "go_to_definition",
        "find_references",
        "apply_code_action",
        // Format, packages, and utility tools
        "format_code",
        "search_packages",
        "uuid_gen",
        "random_token",
        "encode_decode",
        "hash_file",
        // P1 extended tools
        "build_run",
        "lint_run",
        "dependency_add",
        "json_query",
        "yaml_query",
        "template_render",
        // P2 extended tools
        "code_metrics",
        "security_scan",
        "docker_ps",
        "docker_exec",
        "docker_logs",
        "docker_build",
        "docker_push",
        "docker_compose",
        "file_watch",
        "memory_search",
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
