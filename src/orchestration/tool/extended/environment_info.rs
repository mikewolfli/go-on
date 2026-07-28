//! Environment info tool
//!
//! Provides information about the user's environment: OS details,
//! project structure summary, and available tool information.

use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};
use anyhow::{Context, Result};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;
use tracing::debug;

/// Input parameters for [`EnvironmentInfoTool`].
#[derive(JsonSchema, Deserialize)]
struct EnvironmentInfoInput {
    /// Project root directory path (defaults to allowed base dir or cwd).
    #[serde(default)]
    project_root: Option<String>,
}

/// Tool that returns contextual information about the runtime environment.
///
/// Useful for agents that need to understand the OS, project layout,
/// or what tooling is available without making separate calls.
pub struct EnvironmentInfoTool;

impl Tool for EnvironmentInfoTool {
    fn name(&self) -> &'static str {
        "environment_info"
    }

    fn description(&self) -> &str {
        "Get OS family, architecture, hostname, project root, and available tooling"
    }

    fn input_schema(&self) -> Value {
        schemars::schema_for!(EnvironmentInfoInput).into()
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        // Determine project root from payload or use an allowed base directory.
        let params: EnvironmentInfoInput = serde_json::from_value(input.payload.clone())
            .context("failed to deserialize environment_info input")?;
        let project_root = params
            .project_root
            .as_deref()
            .or_else(|| input.allowed_base_dir.as_ref().and_then(|p| p.to_str()))
            .unwrap_or(".");

        debug!(project_root = %project_root, "tool: environment_info");

        // ── OS information ────────────────────────────────────────────
        let os = std::env::consts::OS.to_string();
        let arch = std::env::consts::ARCH.to_string();
        let hostname = std::env::var("HOSTNAME")
            .or_else(|_e| std::env::var("HOST"))
            .unwrap_or_default();

        // ── Project structure (basic, top-level only) ─────────────────
        let project_structure = summarize_directory(project_root);

        // ── Available tooling checks ──────────────────────────────────
        let has_cargo = which("cargo");
        let has_git = which("git");
        let has_node = which("node");
        let has_python = which("python3") || which("python");
        let has_make = which("make");
        let has_docker = which("docker");
        let has_rustup = which("rustup");

        let result = serde_json::json!({
            "os": {
                "family": os,
                "arch": arch,
                "hostname": hostname,
            },
            "project": {
                "root": project_root,
                "structure": project_structure,
            },
            "tooling": {
                "cargo": has_cargo,
                "git": has_git,
                "node": has_node,
                "python": has_python,
                "make": has_make,
                "docker": has_docker,
                "rustup": has_rustup,
            },
        });

        debug!(
            os = %os,
            arch = %arch,
            "tool: environment_info complete"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(result),
            error: None,
            verification: Some("environment_info_gathered".to_string()),
            audit_log: Some(format!(
                "environment_info: os={os} arch={arch} cargo={cargo} git={git}",
                os = os,
                arch = arch,
                cargo = has_cargo,
                git = has_git,
            )),
            pua_report: Some(tool_execution_report(
                "environment_info",
                Some("environment_info_gathered"),
            )),
        })
    }
}

/// Check if a command is available on PATH.
fn which(name: &str) -> bool {
    std::process::Command::new("which")
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Summarise the top-level entries of a directory.
fn summarize_directory(path: &str) -> serde_json::Value {
    let dir = Path::new(path);
    if !dir.is_dir() {
        return serde_json::json!({
            "error": "path is not a directory or does not exist"
        });
    }

    let mut entries: Vec<serde_json::Value> = Vec::new();
    let mut file_count = 0usize;
    let mut dir_count = 0usize;

    if let Ok(read_dir) = dir.read_dir() {
        for entry in read_dir.flatten().take(50) {
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
            if is_dir {
                dir_count += 1;
            } else {
                file_count += 1;
            }
            entries.push(serde_json::json!({
                "name": name,
                "type": if is_dir { "directory" } else { "file" },
            }));
        }
    }

    serde_json::json!({
        "total_files": file_count,
        "total_directories": dir_count,
        "entries_shown": entries.len(),
        "entries": entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::tool::ToolInput;
    use std::path::PathBuf;

    fn tool_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "test-env".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload,
            allowed_base_dir: None,
        }
    }

    #[test]
    fn environment_info_returns_os_fields() {
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let input = tool_input(serde_json::json!({
            "project_root": workspace.to_string_lossy(),
        }));
        let tool = EnvironmentInfoTool;
        let output = tool.run(&input).expect("environment_info should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        assert!(result["os"]["family"].as_str().is_some());
        assert!(result["os"]["arch"].as_str().is_some());
        assert!(result["project"]["root"].as_str().is_some());
    }

    #[test]
    fn environment_info_reports_cargo_available() {
        // In a Rust project, cargo should always be available
        let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let input = tool_input(serde_json::json!({
            "project_root": workspace.to_string_lossy(),
        }));
        let tool = EnvironmentInfoTool;
        let output = tool.run(&input).expect("environment_info should succeed");
        let result = output.result.unwrap();
        assert!(result["tooling"]["cargo"].as_bool().unwrap_or(false));
    }

    #[test]
    fn environment_info_rejects_nonexistent_project_root() {
        let input = tool_input(serde_json::json!({
            "project_root": "/nonexistent-path-99999",
        }));
        let tool = EnvironmentInfoTool;
        let output = tool.run(&input).expect("environment_info should succeed");
        let result = output.result.unwrap();
        assert_eq!(
            result["project"]["structure"]["error"]
                .as_str()
                .unwrap_or(""),
            "path is not a directory or does not exist"
        );
    }
}
