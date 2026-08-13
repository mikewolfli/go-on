//! Container (Docker) management tools.
//!
//! Execute Docker commands to manage containers, images, and logs.
//!
//! SECURITY: these tools deliberately do NOT go through the OS command
//! sandbox. The docker CLI is a client for a privileged daemon socket
//! (`/var/run/docker.sock`): granting the sandbox access to that socket IS
//! host-level access (docker build/exec run arbitrary code with the daemon's
//! privileges), so wrapping the CLI in bwrap adds no containment while
//! breaking socket visibility. The control surface for docker is the
//! governance layer (policy gates, approval, HighRiskExecute classification),
//! not filesystem isolation.

use std::process::{Command, Output};

use anyhow::{Context, Result};
use serde_json::{json, Value};
use tracing::debug;

use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};

/// Run a `docker` command with the given args and capture its output.
/// Shared by all six Docker tools so command construction and the
/// stdout/stderr extraction never drift between them.
fn run_docker_command<I, S>(args: I) -> std::io::Result<Output>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let mut cmd = Command::new("docker");
    cmd.args(args);
    cmd.output()
}

/// `(stdout, stderr)` from a captured process output.
fn output_text(output: &Output) -> (String, String) {
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

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

        let mut args = vec!["ps"];
        if all {
            args.push("--all");
        }
        if format == "json" {
            args.extend(["--format", "{{json .}}"]);
        }

        let output = match run_docker_command(&args) {
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

        let (stdout, stderr) = output_text(&output);

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

        let mut args = vec!["exec"];
        if interactive {
            args.push("-i");
        }
        if let Some(dir) = workdir {
            args.push("-w");
            args.push(dir);
        }
        args.push(container);
        args.push("sh");
        args.push("-c");
        args.push(command);

        let output = run_docker_command(&args)
            .context("failed to execute `docker exec` — is Docker running?")?;

        let (stdout, stderr) = output_text(&output);
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

        let mut args = vec!["logs"];
        if timestamps {
            args.push("-t");
        }
        args.push("--tail");
        args.push(tail);
        if let Some(s) = since {
            args.push("--since");
            args.push(s);
        }
        args.push(container);

        let output = run_docker_command(&args)
            .context("failed to execute `docker logs` — is the container running?")?;

        let (stdout, stderr) = output_text(&output);

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

// ── DockerBuildTool ────────────────────────────────────────────────────────

pub struct DockerBuildTool;

impl Tool for DockerBuildTool {
    fn name(&self) -> &'static str {
        "docker_build"
    }

    fn description(&self) -> &str {
        "Build a Docker image from a Dockerfile. Supports build args, tags, and docker compose build."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Build context directory (default: .)"},
                "tag": {"type": "string", "description": "Image tag (e.g. 'myapp:latest')", "default": "latest"},
                "dockerfile": {"type": "string", "description": "Path to Dockerfile (default: Dockerfile)"},
                "build_args": {"type": "object", "description": "Build-time variables as key-value pairs"},
                "no_cache": {"type": "boolean", "description": "Build without cache (default: false)"},
            },
            "required": []
        })
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input
            .payload
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(".");
        let tag = input
            .payload
            .get("tag")
            .and_then(|v| v.as_str())
            .unwrap_or("latest");
        let dockerfile = input.payload.get("dockerfile").and_then(|v| v.as_str());
        let no_cache = input
            .payload
            .get("no_cache")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let build_args = input.payload.get("build_args").and_then(|v| v.as_object());

        debug!(
            path = %path,
            tag = %tag,
            no_cache = %no_cache,
            "tool: docker_build"
        );

        let mut args: Vec<String> = vec!["build".to_string()];
        if let Some(df) = dockerfile {
            args.push("-f".to_string());
            args.push(df.to_string());
        }
        if no_cache {
            args.push("--no-cache".to_string());
        }
        args.push("-t".to_string());
        args.push(tag.to_string());
        if let Some(extra_args) = build_args {
            for (key, val) in extra_args {
                let build_arg = format!("{}={}", key, val.as_str().unwrap_or(""));
                args.push("--build-arg".to_string());
                args.push(build_arg);
            }
        }
        args.push(path.to_string());

        let output = run_docker_command(&args)
            .context("failed to execute `docker build` — is Docker running?")?;

        let (stdout, stderr) = output_text(&output);
        let success = output.status.success();

        debug!(
            success = %success,
            "tool: docker_build complete"
        );

        if success {
            Ok(ToolOutput {
                success: true,
                result: Some(json!({
                    "stdout": stdout,
                    "stderr": stderr,
                })),
                error: None,
                verification: Some("docker_build_completed".to_string()),
                audit_log: Some(format!("docker_build: built {} with tag {}", path, tag)),
                pua_report: Some(tool_execution_report(
                    "docker_build",
                    Some("docker_build_completed"),
                )),
            })
        } else {
            Ok(ToolOutput {
                success: false,
                result: Some(json!({
                    "stdout": stdout,
                    "stderr": stderr,
                    "error": "docker build failed",
                })),
                error: Some(format!("docker build failed: {}", stderr.trim())),
                verification: Some("docker_build_completed".to_string()),
                audit_log: Some(format!("docker_build failed: {}", stderr.trim())),
                pua_report: Some(tool_execution_report(
                    "docker_build",
                    Some("docker_build_completed"),
                )),
            })
        }
    }
}

// ── DockerPushTool ────────────────────────────────────────────────────────

pub struct DockerPushTool;

impl Tool for DockerPushTool {
    fn name(&self) -> &'static str {
        "docker_push"
    }

    fn description(&self) -> &str {
        "Push a Docker image to a registry. Wraps `docker push`."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "image": {"type": "string", "description": "Image name with tag (e.g. 'myapp:latest')"},
                "registry": {"type": "string", "description": "Registry URL (e.g. 'docker.io/user')"},
            },
            "required": ["image"]
        })
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let image = input
            .payload
            .get("image")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("docker_push requires arguments.image"))?;

        let registry = input.payload.get("registry").and_then(|v| v.as_str());
        let full_image = match registry {
            Some(reg) => crate::shared::url_join::join_url(reg, image),
            None => image.to_string(),
        };

        debug!(
            image = %full_image,
            "tool: docker_push"
        );

        let output = run_docker_command(["push", &full_image])
            .context("failed to execute `docker push` — is Docker running?")?;

        let (stdout, stderr) = output_text(&output);
        let success = output.status.success();

        debug!(
            image = %full_image,
            success = %success,
            "tool: docker_push complete"
        );

        if success {
            Ok(ToolOutput {
                success: true,
                result: Some(json!({
                    "image": full_image,
                    "stdout": stdout,
                    "stderr": stderr,
                })),
                error: None,
                verification: Some("docker_push_completed".to_string()),
                audit_log: Some(format!("docker_push: pushed {}", full_image)),
                pua_report: Some(tool_execution_report(
                    "docker_push",
                    Some("docker_push_completed"),
                )),
            })
        } else {
            Ok(ToolOutput {
                success: false,
                result: Some(json!({
                    "image": full_image,
                    "stdout": stdout,
                    "stderr": stderr,
                    "error": "docker push failed",
                })),
                error: Some(format!("docker push failed: {}", stderr.trim())),
                verification: Some("docker_push_completed".to_string()),
                audit_log: Some(format!("docker_push failed: {}", stderr.trim())),
                pua_report: Some(tool_execution_report(
                    "docker_push",
                    Some("docker_push_completed"),
                )),
            })
        }
    }
}

// ── DockerComposeTool ─────────────────────────────────────────────────────

pub struct DockerComposeTool;

impl Tool for DockerComposeTool {
    fn name(&self) -> &'static str {
        "docker_compose"
    }

    fn description(&self) -> &str {
        "Run docker-compose commands (up, down, build, logs, ps). Wraps `docker compose`."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "subcommand": {
                    "type": "string",
                    "enum": ["up", "down", "build", "logs", "ps", "restart", "stop", "start"],
                    "description": "Docker compose subcommand"
                },
                "file": {"type": "string", "description": "Path to compose file (default: docker-compose.yml)"},
                "service": {"type": "string", "description": "Target service name (optional)"},
                "detach": {"type": "boolean", "description": "Run containers in background (default: true for up)"},
                "build": {"type": "boolean", "description": "Build images before starting (for up)"},
                "tail": {"type": "string", "description": "Number of log lines to show (for logs)"},
            },
            "required": ["subcommand"]
        })
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let subcommand = input
            .payload
            .get("subcommand")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("docker_compose requires arguments.subcommand"))?;

        let file = input.payload.get("file").and_then(|v| v.as_str());
        let service = input.payload.get("service").and_then(|v| v.as_str());
        let detach = input
            .payload
            .get("detach")
            .and_then(|v| v.as_bool())
            .unwrap_or(subcommand == "up");
        let build = input
            .payload
            .get("build")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let tail = input.payload.get("tail").and_then(|v| v.as_str());

        debug!(
            subcommand = %subcommand,
            file = ?file,
            service = ?service,
            "tool: docker_compose"
        );

        let mut args = vec!["compose"];
        if let Some(f) = file {
            args.push("-f");
            args.push(f);
        }
        args.push(subcommand);

        match subcommand {
            "up" => {
                if detach {
                    args.push("-d");
                }
                if build {
                    args.push("--build");
                }
            }
            "logs" => {
                if let Some(t) = tail {
                    args.push("--tail");
                    args.push(t);
                }
            }
            "build" => {
                // no extra args needed
            }
            _ => {}
        }

        if let Some(s) = service {
            args.push(s);
        }

        let output = run_docker_command(&args)
            .context("failed to execute `docker compose` — is Docker running?")?;

        let (stdout, stderr) = output_text(&output);
        let success = output.status.success();

        debug!(
            subcommand = %subcommand,
            success = %success,
            "tool: docker_compose complete"
        );

        if success {
            Ok(ToolOutput {
                success: true,
                result: Some(json!({
                    "stdout": stdout,
                    "stderr": stderr,
                })),
                error: None,
                verification: Some("docker_compose_completed".to_string()),
                audit_log: Some(format!("docker_compose: {} completed", subcommand)),
                pua_report: Some(tool_execution_report(
                    "docker_compose",
                    Some("docker_compose_completed"),
                )),
            })
        } else {
            Ok(ToolOutput {
                success: false,
                result: Some(json!({
                    "stdout": stdout,
                    "stderr": stderr,
                    "error": "docker compose failed",
                })),
                error: Some(format!(
                    "docker compose {} failed: {}",
                    subcommand,
                    stderr.trim()
                )),
                verification: Some("docker_compose_completed".to_string()),
                audit_log: Some(format!(
                    "docker_compose {} failed: {}",
                    subcommand,
                    stderr.trim()
                )),
                pua_report: Some(tool_execution_report(
                    "docker_compose",
                    Some("docker_compose_completed"),
                )),
            })
        }
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
