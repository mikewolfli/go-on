//! Required-argument validation for built-in tools.

use anyhow::Result;
use serde_json::Value;

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
        // ── Extended tool validation ────────────────────────
        "shell_exec" => {
            tool_input
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("shell_exec requires arguments.command"))?;
        }
        "grep" => {
            tool_input
                .get("pattern")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("grep requires arguments.pattern"))?;
        }
        "find_files" => {
            tool_input
                .get("pattern")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("find_files requires arguments.pattern"))?;
        }
        "list_directory" => {
            tool_input
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("list_directory requires arguments.path"))?;
        }
        "move_path" | "file_move" => {
            tool_input
                .get("source")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("move_path requires arguments.source"))?;
            tool_input
                .get("destination")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("move_path requires arguments.destination"))?;
        }
        "edit_file" => {
            tool_input
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("edit_file requires arguments.path"))?;
            tool_input
                .get("old_text")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("edit_file requires arguments.old_text"))?;
            tool_input
                .get("new_text")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("edit_file requires arguments.new_text"))?;
        }
        "delete_path" | "file_delete" => {
            tool_input
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("delete_path requires arguments.path"))?;
            tool_input
                .get("confirm")
                .and_then(|v| v.as_bool())
                .ok_or_else(|| {
                    anyhow::anyhow!("delete_path requires arguments.confirm (boolean)")
                })?;
        }
        "git" => {
            tool_input
                .get("subcommand")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("git requires arguments.subcommand"))?;
        }
        "compress" | "decompress" => {
            tool_input
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("{} requires arguments.path", tool_name))?;
        }
        "dns_lookup" => {
            tool_input
                .get("hostname")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("dns_lookup requires arguments.hostname"))?;
        }
        "ping" => {
            tool_input
                .get("host")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("ping requires arguments.host"))?;
        }
        "port_scan" => {
            tool_input
                .get("host")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("port_scan requires arguments.host"))?;
        }
        "skill_execute" => {
            tool_input
                .get("skill_name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("skill_execute requires arguments.skill_name"))?;
        }
        "skill_create" => {
            tool_input
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("skill_create requires arguments.name"))?;
            tool_input
                .get("description")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("skill_create requires arguments.description"))?;
            tool_input
                .get("prompt_template")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    anyhow::anyhow!("skill_create requires arguments.prompt_template")
                })?;
        }
        "go_to_definition" | "find_references" => {
            tool_input
                .get("symbol")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("{} requires arguments.symbol", tool_name))?;
        }
        "apply_code_action" => {
            tool_input
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("apply_code_action requires arguments.path"))?;
            tool_input
                .get("action")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("apply_code_action requires arguments.action"))?;
        }
        "format_code" => {
            tool_input
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("format_code requires arguments.path"))?;
        }
        "search_packages" => {
            tool_input
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("search_packages requires arguments.query"))?;
        }
        "memory_search" => {
            tool_input
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("memory_search requires arguments.query"))?;
        }
        "encode_decode" => {
            tool_input
                .get("operation")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("encode_decode requires arguments.operation"))?;
            tool_input
                .get("input")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("encode_decode requires arguments.input"))?;
        }
        "hash_file" => {
            tool_input
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("hash_file requires arguments.path"))?;
        }
        "build_run" | "lint_run" => {}
        "dependency_add" => {
            tool_input
                .get("package")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("dependency_add requires arguments.package"))?;
        }
        "json_query" | "yaml_query" => {
            tool_input
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("{} requires arguments.path", tool_name))?;
        }
        "template_render" => {
            tool_input
                .get("template")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("template_render requires arguments.template"))?;
        }
        "uuid_gen" | "random_token" | "skill_reload" => {}
        "workflow_execute" | "workflow_ask" | "workflow_generate" => {}
        // ── P2 tool validation ─────────────────────────────────
        "code_metrics" | "security_scan" | "docker_ps" | "file_watch" => {}
        "docker_exec" => {
            tool_input
                .get("container")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("docker_exec requires arguments.container"))?;
            tool_input
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("docker_exec requires arguments.command"))?;
        }
        "docker_logs" => {
            tool_input
                .get("container")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("docker_logs requires arguments.container"))?;
        }
        "docker_push" => {
            tool_input
                .get("image")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("docker_push requires arguments.image"))?;
        }
        "docker_compose" => {
            tool_input
                .get("subcommand")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("docker_compose requires arguments.subcommand"))?;
        }
        _ => {}
    }
    Ok(())
}
