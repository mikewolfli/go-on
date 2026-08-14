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
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::Duration;
use tracing::{debug, info, warn};

/// Cached result of the GNU timeout availability check.
/// Once detected, the result is reused for the lifetime of the process.
fn gnu_timeout_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        if cfg!(target_os = "windows") {
            return false;
        }
        Command::new("timeout")
            .arg("1")
            .arg("sh")
            .arg("-c")
            .arg("true")
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    })
}

/// Write stdin content to the child process if provided.
fn write_stdin_if_needed(child: &mut std::process::Child, stdin_input: &Option<String>) {
    if let Some(stdin_text) = stdin_input {
        if let Some(mut stdin_writer) = child.stdin.take() {
            let _ = stdin_writer.write_all(stdin_text.as_bytes());
        }
    }
}

pub struct ShellExecTool;

impl Tool for ShellExecTool {
    fn name(&self) -> &'static str {
        "shell_exec"
    }
    fn description(&self) -> &str {
        "Execute a shell command with timeout and capture output"
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

        let timeout_secs = (timeout_ms as f64 / 1000.0).ceil() as u64;
        let max_timeout = cap_timeout_secs(timeout_secs);

        // Use cached result — GNU timeout detection runs only once per process
        let use_gnu_timeout = gnu_timeout_available();

        // ── LAYER 3: OS-level command sandbox (bubblewrap) ──────────────
        // The whole command (including the `timeout` wrapper) runs inside a
        // new user/pid namespace with a read-only host filesystem when the
        // effective sandbox mode requires it and bwrap is available — so a
        // blocked-command-list bypass in the command text cannot escape the
        // workspace or touch credential files. Spawn failure (bwrap missing /
        // unprivileged user namespaces denied) degrades to direct execution
        // with a warning; the policy gates and blocked-command filter above
        // still apply in that case.
        let base_program: &str = if use_gnu_timeout { "timeout" } else { shell };
        let base_args: Vec<String> = if use_gnu_timeout {
            vec![
                format!("{max_timeout}"),
                shell.to_string(),
                shell_arg.to_string(),
                command.to_string(),
            ]
        } else {
            vec![shell_arg.to_string(), command.to_string()]
        };
        let wrapped = crate::security::sandbox::wrap_command(
            crate::security::sandbox::effective_mode(),
            &current_dir,
            base_program,
            &base_args,
        );
        // Credential env vars are scrubbed from commands whenever a sandbox
        // mode is requested (even when bwrap is unavailable and execution
        // degrades to direct, or the platform has no bwrap): env leakage is
        // independent of filesystem containment. `passthrough_env` is the
        // explicit escape hatch.
        let scrub_env = wrapped.mode != crate::security::sandbox::SandboxMode::None;
        let passthrough_env = crate::security::sandbox::sandbox_config()
            .map(|c| c.passthrough_env)
            .unwrap_or_default();

        let spawn_once =
            |program: &str, args: &[String], scrub: bool| -> std::io::Result<std::process::Child> {
                let mut cmd = Command::new(program);
                cmd.args(args)
                    .current_dir(&current_dir)
                    .stdin(if stdin_input.is_some() {
                        Stdio::piped()
                    } else {
                        Stdio::null()
                    })
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                if scrub {
                    // The sandbox isolates the filesystem but not the inherited
                    // environment: drop credential-bearing vars so `printenv`
                    // inside the sandbox cannot leak keys/tokens to the command.
                    cmd.env_clear();
                    for (key, val) in crate::security::sandbox::sanitized_env(&passthrough_env) {
                        cmd.env(key, val);
                    }
                }
                for (key, val) in &env_vars {
                    cmd.env(key, val);
                }
                cmd.spawn()
            };

        // ── Run supervision ──────────────────────────────────────────────
        // run_one spawns `program args` (applying env scrub when requested),
        // feeds stdin, and waits with timeout enforcement:
        // - GNU `timeout` caps the inner command (works identically inside the
        //   bwrap namespace); its 124 exit convention maps to a timeout report.
        // - Without GNU timeout, a DETACHED kill thread SIGKILLs the spawned
        //   PID after `timeout_ms` — for a sandboxed command that is the bwrap
        //   PID, and killing it terminates the whole namespace (bwrap is PID 1
        //   there with --die-with-parent). The thread checks `/proc/<pid>`
        //   liveness first so a recycled PID is never killed, and it is never
        //   joined, so a fast command returns immediately instead of blocking
        //   for the full timeout window.
        let run_one = |program: &str,
                       args: &[String],
                       scrub: bool|
         -> std::io::Result<(std::process::Output, bool)> {
            let mut child = spawn_once(program, args, scrub)?;
            write_stdin_if_needed(&mut child, &stdin_input);
            if use_gnu_timeout {
                let out = child.wait_with_output()?;
                // GNU timeout's exit status 124 == timed out.
                let timed_out = out.status.code() == Some(124);
                return Ok((out, timed_out));
            }
            let kill_after = Duration::from_millis(timeout_ms);
            let pid = child.id();
            let killed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let child_finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let (killed_clone, finished_clone) = (killed.clone(), child_finished.clone());
            let _ = std::thread::spawn(move || {
                std::thread::sleep(kill_after);
                if finished_clone.load(std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                #[cfg(unix)]
                if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
                    return;
                }
                killed_clone.store(true, std::sync::atomic::Ordering::SeqCst);
                if cfg!(target_os = "windows") {
                    let _ = Command::new("taskkill")
                        .arg("/F")
                        .arg("/T")
                        .arg("/PID")
                        .arg(pid.to_string())
                        .output();
                } else {
                    let _ = Command::new("kill").arg("--").arg(pid.to_string()).output();
                    let _ = Command::new("kill")
                        .arg("-9")
                        .arg("--")
                        .arg(pid.to_string())
                        .output();
                }
            });
            let out = child.wait_with_output()?;
            child_finished.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok((out, killed.load(std::sync::atomic::Ordering::SeqCst)))
        };

        // Try the sandboxed command first; degrade to direct execution on a
        // spawn-level failure (sandbox unavailable/denied at runtime) or on a
        // bwrap *setup* failure (spawns fine but exits 1 with a `bwrap:`
        // stderr prefix — e.g. unprivileged user namespaces denied, in which
        // case the inner command never ran). A plain non-zero exit is never
        // degraded — that is the sandbox doing its job.
        let mut sandbox_effective = wrapped.applied;
        let mut result = match run_one(&wrapped.program, &wrapped.args, scrub_env) {
            Ok(r) => r,
            Err(e) if wrapped.applied => {
                crate::security::sandbox::record_sandbox_degraded();
                sandbox_effective = false;
                warn!(
                    error = %e,
                    mode = %wrapped.mode.as_str(),
                    "sandboxed spawn failed — falling back to direct execution"
                );
                run_one(base_program, &base_args, scrub_env)?
            }
            Err(e) => return Err(e.into()),
        };
        if wrapped.applied && crate::security::sandbox::is_bwrap_setup_failure(
            result.0.status.code(),
            &String::from_utf8_lossy(&result.0.stderr),
        ) {
            crate::security::sandbox::record_sandbox_degraded();
            sandbox_effective = false;
            warn!(
                mode = %wrapped.mode.as_str(),
                "sandboxed command failed at setup (bwrap error) — retrying without sandbox"
            );
            result = run_one(base_program, &base_args, scrub_env)?;
        }
        let (output, timed_out) = result;
        // Audit/result reports the mode that actually ran, not the requested
        // one: after degrade the command ran direct, so report "none".
        let audit_sandbox = if sandbox_effective {
            wrapped.mode.as_str()
        } else {
            "none"
        };

        if timed_out {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
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
                audit_sandbox,
            ));
        }

        let success = output.status.success();
        let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let mut stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code();

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
            audit_sandbox,
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
