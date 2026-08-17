//! Shell execution tool
//!
//! Uses shared execution infrastructure from `exec_common` for timeout handling,
//! output truncation, blocked-command filtering, and result building.

use crate::i18n::runtime::t;
use crate::orchestration::tool::exec_common::{
    build_blocked_tool_output, build_shell_tool_output, build_timeout_tool_output,
    cap_timeout_secs, is_blocked_command, truncate_output,
};
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
use anyhow::Result;
use tracing::{debug, info, warn};

pub struct ShellExecTool;

impl Tool for ShellExecTool {
    fn name(&self) -> &'static str {
        "shell_exec"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let command = input.payload["command"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_command")))?;
        let timeout_ms = input.payload["timeout_ms"].as_u64().unwrap_or(30_000);
        let directory = input.payload["directory"].as_str().unwrap_or(".");

        // ── LAYER 2: Runtime sandbox ────────────────────────────────────
        // Block dangerous commands that could harm the system.
        if let Some(pattern) = is_blocked_command(command) {
            warn!(
                "shell_exec BLOCKED: command matches blocked pattern '{}' — cmd={}",
                pattern, command
            );
            return Ok(build_blocked_tool_output(pattern, command, "shell_exec"));
        }

        debug!(command = %command, timeout_ms = %timeout_ms, directory = %directory, "tool: executing shell command");

        let current_dir = sanitize_path(input, directory)?;

        // Environment variables from payload["env"] as a JSON object
        let env_vars: Vec<(String, String)> = input.payload["env"]
            .as_object()
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|val_str| (k.clone(), val_str.to_string())))
                    .collect()
            })
            .unwrap_or_default();

        // stdin input from payload["stdin"] as a string
        let stdin_input = input.payload["stdin"].as_str().map(|s| s.to_string());

        // Determine the shell to use: cmd.exe on Windows, sh on Unix.
        let (shell, shell_arg) = if cfg!(target_os = "windows") {
            ("cmd.exe", "/C")
        } else {
            ("sh", "-c")
        };
        let shell_args = vec![shell_arg.to_string(), command.to_string()];

        // ── LAYER 3: OS-level command sandbox (bubblewrap) ──────────────
        // The whole command (including the GNU `timeout` wrapper) runs inside
        // a new user/pid namespace with a read-only host filesystem when the
        // effective sandbox mode requires it and bwrap is available — so a
        // blocked-command-list bypass in the command text cannot escape the
        // workspace or touch credential files. Spawn failure (bwrap missing /
        // unprivileged user namespaces denied) degrades to direct execution
        // with a warning; the policy gates and blocked-command filter above
        // still apply in that case. The sandbox wrap, env scrubbing, degrade
        // retry, and timeout supervision (GNU `timeout` inside the namespace,
        // kill-thread fallback without it) are all handled by the shared
        // runner in `exec_common`.
        let timeout_secs = (timeout_ms as f64 / 1000.0).ceil() as u64;
        let max_timeout = cap_timeout_secs(timeout_secs);
        let outcome = crate::orchestration::tool::exec_common::with_blocking_runtime(|rt| {
            rt.block_on(
                crate::orchestration::tool::exec_common::run_sandboxed_capped_timeout_async(
                    &current_dir,
                    shell,
                    &shell_args,
                    crate::orchestration::tool::exec_common::TimeoutRunOptions {
                        cap: crate::orchestration::tool::exec_common::MAX_OUTPUT_BYTES,
                        timeout_ms,
                        max_timeout_secs: max_timeout,
                        stdin_input,
                    },
                    |cmd| {
                        for (key, val) in &env_vars {
                            cmd.env(key, val);
                        }
                    },
                ),
            )
        })?;

        if outcome.timed_out {
            let stdout = String::from_utf8_lossy(&outcome.stdout).to_string();
            let stderr = String::from_utf8_lossy(&outcome.stderr).to_string();
            warn!(
                command = %command,
                timeout_ms = %timeout_ms,
                "tool: shell command timed out"
            );
            return Ok(build_timeout_tool_output(
                stdout,
                stderr,
                command,
                directory,
                timeout_ms,
                "shell_exec",
                &outcome.sandbox,
            ));
        }

        let success = outcome.status == Some(0);
        let mut stdout = String::from_utf8_lossy(&outcome.stdout).to_string();
        let mut stderr = String::from_utf8_lossy(&outcome.stderr).to_string();
        let exit_code = outcome.status;

        // ── LAYER 2: Output size limit ──────────────────────────
        truncate_output(&mut stdout);
        truncate_output(&mut stderr);

        if !success {
            warn!(
                command = %command,
                exit_code = ?exit_code,
                stderr = %stderr.trim(),
                "tool: shell command failed"
            );
        } else {
            info!(command = %command, exit_code = ?exit_code, "tool: shell command succeeded");
        }

        Ok(build_shell_tool_output(
            success,
            stdout,
            stderr,
            exit_code,
            command,
            directory,
            "shell_exec",
            &outcome.sandbox,
        ))
    }
}

/// bwrap "spawned fine but could not set up the namespace" detection lives in
/// `crate::security::sandbox::is_bwrap_setup_failure` — the local copy was
/// removed so the signature cannot drift between shell_exec and the
/// exec_common runners.
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    // Serializes against the startup_context chdir tests (see
    // bwrap_setup_failure_detection below).
    use serial_test::serial;
    use std::process::Command;

    fn tool_input(payload: serde_json::Value, base: std::path::PathBuf) -> ToolInput {
        ToolInput {
            task_id: "sandbox-test".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "sandbox containment test".to_string(),
            constraints: None,
            evidence: None,
            payload,
            allowed_base_dir: Some(base),
        }
    }

    #[test]
    #[serial]
    fn bwrap_setup_failure_detection() {
        // `sh` inherits the process CWD; the startup_context tests temporarily
        // `set_current_dir` into a temp dir that is deleted on drop, and a
        // concurrent spawn there makes sh print "getcwd() failed" as the first
        // stderr line (breaking the "bwrap:" prefix check). The shared
        // `serial_test` lock serializes against those chdir tests (same lock
        // the startup_context/skill_market tests use). Additionally, a process
        // spawn can transiently fail (fork EAGAIN/EMFILE) or be reaped without
        // an exit code under extreme parallel load, so retry those cases.
        fn sh_output(script: &str) -> std::process::Output {
            for attempt in 0..3 {
                match Command::new("sh").args(["-c", script]).output() {
                    Ok(o) if o.status.code().is_some() => return o,
                    Ok(o) => eprintln!(
                        "sh attempt {} exited without a code (killed?): {:?}",
                        attempt, o.status
                    ),
                    Err(e) => {
                        eprintln!("sh spawn attempt {} failed: {}", attempt, e);
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Command::new("sh").args(["-c", script]).output().unwrap()
        }

        let setup = sh_output("echo \"bwrap: Can't make progress\" >&2; exit 1");
        assert!(crate::security::sandbox::is_bwrap_setup_failure(
            setup.status.code(),
            &String::from_utf8_lossy(&setup.stderr)
        ));

        let normal = sh_output("echo compile error >&2; exit 1");
        assert!(!crate::security::sandbox::is_bwrap_setup_failure(
            normal.status.code(),
            &String::from_utf8_lossy(&normal.stderr)
        ));

        let success = sh_output("echo ok >&2; exit 0");
        assert!(!crate::security::sandbox::is_bwrap_setup_failure(
            success.status.code(),
            &String::from_utf8_lossy(&success.stderr)
        ));
    }

    /// End-to-end wiring check: ShellExecTool runs model-issued commands
    /// through the sandbox runner, so writes outside the workspace fail while
    /// normal commands inside it succeed. Skipped when bwrap namespaces are
    /// unavailable (the feature then degrades to direct execution).
    #[cfg(target_os = "linux")]
    #[test]
    #[serial]
    fn shell_exec_sandbox_confines_writes() {
        if !crate::security::sandbox::bwrap_probe_works() {
            eprintln!("skipping shell_exec sandbox test — bwrap namespaces unavailable");
            return;
        }
        let dir = tempfile::TempDir::new().unwrap();
        let ws = dir.path().join("ws");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let base = dir.path().to_path_buf();

        // Inside the workspace: writable, succeeds.
        let ok = ShellExecTool
            .run(&tool_input(
                json!({
                    "command": "touch in_ws && test -f in_ws",
                    "directory": ws.to_string_lossy(),
                    "timeout_ms": 10_000,
                }),
                base.clone(),
            ))
            .expect("tool run should not error");
        assert!(ok.success, "workspace write should succeed: {:?}", ok.error);

        // Outside the workspace: denied by the kernel mount, not by the model.
        // bwrap can transiently degrade to direct execution under extreme
        // parallel load (setup failure → retry-without-sandbox), so retry the
        // containment check; a persistent failure still fails the assertion.
        let mut last_error: Option<String> = None;
        for _attempt in 0..3 {
            let denied = ShellExecTool
                .run(&tool_input(
                    json!({
                        "command": format!("touch {} 2>/dev/null", outside.join("x").display()),
                        "directory": ws.to_string_lossy(),
                        "timeout_ms": 10_000,
                    }),
                    base.clone(),
                ))
                .expect("tool run should not error");
            if !denied.success {
                return; // containment verified
            }
            last_error = denied.error;
        }
        panic!(
            "outside-workspace write must be denied (3 attempts): {:?}",
            last_error
        );
    }

    /// The root-workspace guard must keep the tool functional: workspace "/"
    /// skips the sandbox wrapper (a `--bind / /` would disable containment)
    /// and runs the command directly instead.
    #[test]
    fn shell_exec_root_workspace_runs_direct() {
        let out = ShellExecTool
            .run(&tool_input(
                json!({
                    "command": "echo root-ok",
                    "directory": "/",
                    "timeout_ms": 10_000,
                }),
                std::path::PathBuf::from("/"),
            ))
            .expect("tool run should not error");
        assert!(
            out.success,
            "root workspace must degrade to direct execution: {:?}",
            out.error
        );
    }
}
