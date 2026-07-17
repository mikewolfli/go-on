//! Container (Docker) management tools.
//!
//! Execute Docker commands to manage containers, images, and logs.

use std::process::Command;

use anyhow::{Context, Result};
use serde_json::{json, Value};
use tracing::debug;

use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};

// ── DockerPsTool ──────────────────────────────────────────────────────────

pub struct DockerPsTool;

impl Tool for DockerPsTool {
    fn name(&self) -> &'static str {
        "docker_ps"
    }

    fn description(&self) -> &str {
        "List Docker containers (running and optionally stopped). \
         Wraps `docker ps`. Returns container IDs, names, images, status, and ports."
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let all = input
            .payload
            .get("all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let format = input
            .payload
            .get("format")
            .and_then(|v| v.as_str())
            .unwrap_or("json");

        debug!(all = %all, format = %format, "tool: docker_ps");

        let mut cmd = Command::new("docker");
        cmd.arg("ps");

        if all {
            cmd.arg("--all");
        }

        if format == "json" {
            cmd.args(["--format", "{{json .}}"]);
        }

        let output = match cmd.output() {
            Ok(o) => o,
            Err(e) => {
                debug!(error = %e, "tool: docker_ps failed — Docker not available");
                return Ok(ToolOutput {
                    success: false,
                    result: Some(json!({
                        "containers": [],
                        "error": e.to_string(),
                    })),
                    error: Some(format!("docker ps failed: {}", e)),
                    verification: Some("docker_ps_completed".to_string()),
                    audit_log: Some(format!("docker_ps failed: {}", e)),
                    pua_report: Some(tool_execution_report(
                        "docker_ps",
                        Some("docker_ps_completed"),
                    )),
                });
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        if !output.status.success() {
            debug!(stderr = %stderr, "tool: docker_ps failed");
            return Ok(ToolOutput {
                success: false,
                result: Some(json!({
                    "containers": [],
                    "error": stderr,
                })),
                error: Some(format!("docker ps failed: {}", stderr.trim())),
                verification: Some("docker_ps_completed".to_string()),
                audit_log: Some(format!("docker_ps failed: {}", stderr.trim())),
                pua_report: Some(tool_execution_report(
                    "docker_ps",
                    Some("docker_ps_completed"),
                )),
            });
        }

        let containers: Vec<Value> = if format == "json" {
            stdout
                .lines()
                .filter_map(|line| {
                    serde_json::from_str::<Value>(line).ok().map(|v| {
                        // Ensure common fields exist.
                        let id = v["ID"].as_str().unwrap_or("").to_string();
                        let names = v["Names"].as_str().unwrap_or("").to_string();
                        let image = v["Image"].as_str().unwrap_or("").to_string();
                        let status = v["Status"].as_str().unwrap_or("").to_string();
                        let ports = v["Ports"].as_str().unwrap_or("").to_string();
                        json!({
                            "id": id,
                            "names": names,
                            "image": image,
                            "status": status,
                            "ports": ports,
                            "created": v["CreatedAt"].as_str().unwrap_or(""),
                            "running_for": v["RunningFor"].as_str().unwrap_or(""),
                            "state": v["State"].as_str().unwrap_or(""),
                        })
                    })
                })
                .collect()
        } else {
            // Plain text format — return raw lines.
            stdout.lines().map(|line| json!({"raw": line})).collect()
        };

        debug!(count = %containers.len(), "tool: docker_ps complete");

        Ok(ToolOutput {
            success: true,
            result: Some(json!({
                "containers": containers,
                "count": containers.len(),
            })),
            error: None,
            verification: Some("docker_ps_completed".to_string()),
            audit_log: Some(format!("docker_ps: {} containers listed", containers.len())),
            pua_report: Some(tool_execution_report(
                "docker_ps",
                Some("docker_ps_completed"),
            )),
        })
    }
}

// ── DockerExecTool ────────────────────────────────────────────────────────

pub struct DockerExecTool;

impl Tool for DockerExecTool {
    fn name(&self) -> &'static str {
        "docker_exec"
    }

    fn description(&self) -> &str {
        "Execute a command inside a running Docker container. \
         Wraps `docker exec`. Returns stdout and stderr from the command."
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let container = input
            .payload
            .get("container")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("docker_exec requires arguments.container"))?;

        let command = input
            .payload
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("docker_exec requires arguments.command"))?;

        let interactive = input
            .payload
            .get("interactive")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let workdir = input.payload.get("workdir").and_then(|v| v.as_str());

        debug!(
            container = %container,
            command = %command,
            "tool: docker_exec"
        );

        let mut cmd = Command::new("docker");
        cmd.args(["exec"]);

        if interactive {
            cmd.arg("-i");
        }

        if let Some(dir) = workdir {
            cmd.args(["-w", dir]);
        }

        cmd.args([container, "sh", "-c", command]);

        let output = cmd
            .output()
            .context("failed to execute `docker exec` — is Docker running?")?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let success = output.status.success();
        let exit_code = output.status.code().unwrap_or(-1);

        debug!(
            container = %container,
            exit_code = %exit_code,
            success = %success,
            "tool: docker_exec complete"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(json!({
                "stdout": stdout,
                "stderr": stderr,
                "exit_code": exit_code,
                "success": success,
            })),
            error: if success { None } else { Some(stderr.clone()) },
            verification: Some("docker_exec_completed".to_string()),
            audit_log: Some(format!(
                "docker_exec: {} in container {}, exit={}",
                command, container, exit_code
            )),
            pua_report: Some(tool_execution_report(
                "docker_exec",
                Some("docker_exec_completed"),
            )),
        })
    }
}

// ── DockerLogsTool ────────────────────────────────────────────────────────

pub struct DockerLogsTool;

impl Tool for DockerLogsTool {
    fn name(&self) -> &'static str {
        "docker_logs"
    }

    fn description(&self) -> &str {
        "View logs from a Docker container. Wraps `docker logs`. \
         Supports tail, since, and follow options."
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let container = input
            .payload
            .get("container")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("docker_logs requires arguments.container"))?;

        let tail = input
            .payload
            .get("tail")
            .and_then(|v| v.as_str())
            .unwrap_or("50");
        let since = input.payload.get("since").and_then(|v| v.as_str());
        let timestamps = input
            .payload
            .get("timestamps")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        debug!(
            container = %container,
            tail = %tail,
            "tool: docker_logs"
        );

        let mut cmd = Command::new("docker");
        cmd.args(["logs"]);

        if timestamps {
            cmd.arg("-t");
        }

        cmd.args(["--tail", tail]);

        if let Some(s) = since {
            cmd.args(["--since", s]);
        }

        cmd.arg(container);

        let output = cmd
            .output()
            .context("failed to execute `docker logs` — is the container running?")?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        // `docker logs` writes both stdout and stderr; combine them.
        let mut log_lines: Vec<&str> = stdout.lines().collect();
        log_lines.extend(stderr.lines());

        debug!(
            container = %container,
            lines = %log_lines.len(),
            "tool: docker_logs complete"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(json!({
                "container": container,
                "lines": log_lines.len(),
                "logs": log_lines,
            })),
            error: None,
            verification: Some("docker_logs_completed".to_string()),
            audit_log: Some(format!(
                "docker_logs: {} lines from container {}",
                log_lines.len(),
                container
            )),
            pua_report: Some(tool_execution_report(
                "docker_logs",
                Some("docker_logs_completed"),
            )),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::tool::ToolInput;

    fn tool_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "test-docker".to_string(),
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
    fn docker_ps_fails_gracefully_without_docker() {
        // This test should pass even without Docker installed.
        let input = tool_input(json!({}));
        let tool = DockerPsTool;
        let result = tool.run(&input);
        // It may succeed or fail depending on the environment, but it shouldn't panic.
        assert!(
            result.is_ok(),
            "docker_ps should return Ok even without Docker"
        );
        let output = result.unwrap();
        // If Docker is not available, success should be false or containers empty.
        if !output.success {
            let has_error = output.error.is_some();
            assert!(has_error, "failure should include error message");
        }
    }

    #[test]
    fn docker_exec_requires_container_arg() {
        let input = tool_input(json!({"command": "ls"}));
        let tool = DockerExecTool;
        let result = tool.run(&input);
        assert!(
            result.is_err(),
            "docker_exec without container should error"
        );
        assert!(
            result.unwrap_err().to_string().contains("container"),
            "error should mention missing container"
        );
    }

    #[test]
    fn docker_exec_requires_command_arg() {
        let input = tool_input(json!({"container": "test"}));
        let tool = DockerExecTool;
        let result = tool.run(&input);
        assert!(result.is_err(), "docker_exec without command should error");
        assert!(
            result.unwrap_err().to_string().contains("command"),
            "error should mention missing command"
        );
    }

    #[test]
    fn docker_logs_requires_container_arg() {
        let input = tool_input(json!({}));
        let tool = DockerLogsTool;
        let result = tool.run(&input);
        assert!(
            result.is_err(),
            "docker_logs without container should error"
        );
        assert!(
            result.unwrap_err().to_string().contains("container"),
            "error should mention missing container"
        );
    }
}
