//! Game process tools: launching and monitoring game processes
//! (feature `game-process`).

use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};
use anyhow::{anyhow, Context, Result};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, info};

// ═══════════════════════════════════════════════════════════════════════════════
// Section 2: Game Process & Window Tools   #[cfg(feature = "game-process")]
// ═══════════════════════════════════════════════════════════════════════════════

/// Launches a game process with optional arguments.
/// Uses `std::process::Command` to spawn the executable.
#[cfg(feature = "game-process")]
pub struct GameLaunchTool;
#[cfg(feature = "game-process")]
impl Tool for GameLaunchTool {
    fn name(&self) -> &'static str {
        "game_launch"
    }

    fn exposure(&self) -> crate::orchestration::tool::ToolExposure {
        crate::orchestration::tool::ToolExposure::Deferred
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let exe = input.payload["executable"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'executable'"))?;
        let args: Vec<String> = input.payload["args"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let working_dir = input.payload["working_directory"].as_str();
        let detached = input.payload["detached"].as_bool().unwrap_or(true);

        let exe_path = std::path::Path::new(exe);
        if !exe_path.exists() {
            anyhow::bail!(
                "executable not found: {}. Provide a full path to the game executable.",
                exe
            );
        }

        debug!(executable = %exe, args = ?args, "game_launch: launching game");

        let mut cmd = std::process::Command::new(exe);
        cmd.args(&args);
        if let Some(wd) = working_dir {
            cmd.current_dir(wd);
        }

        // If detached, spawn and forget; otherwise wait briefly and capture
        if detached {
            let child = cmd
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .stdin(std::process::Stdio::null())
                .spawn()
                .context("failed to spawn game process")?;
            let pid = child.id();

            info!(executable = %exe, pid = %pid, "game_launch: game launched (detached)");

            let report = tool_execution_report("game_launch", Some("game_launched"));

            Ok(ToolOutput {
                success: true,
                result: Some(json!({
                    "executable": exe,
                    "pid": pid,
                    "detached": true,
                    "status": "launched",
                })),
                error: None,
                verification: Some("game_launched".to_string()),
                audit_log: Some(format!("game_launch: launched {} (pid {})", exe, pid)),
                pua_report: Some(report),
            })
        } else {
            // Run and collect output (useful for launchers that output to stdout)
            let output = cmd.output().context("failed to run game process")?;
            let pid = 0; // process already exited
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let exit_code = output.status.code();

            info!(executable = %exe, exit_code = ?exit_code, "game_launch: game process exited");

            let report = tool_execution_report("game_launch", Some("game_launched"));

            Ok(ToolOutput {
                success: output.status.success() || exit_code.is_none(),
                result: Some(json!({
                    "executable": exe,
                    "pid": pid,
                    "detached": false,
                    "exit_code": exit_code,
                    "stdout": stdout,
                    "stderr": stderr,
                })),
                error: if !output.status.success() {
                    Some(format!(
                        "process exited with code {:?}: {}",
                        exit_code, stderr
                    ))
                } else {
                    None
                },
                verification: Some("game_launched".to_string()),
                audit_log: Some(format!("game_launch: ran {} (exit {:?})", exe, exit_code)),
                pua_report: Some(report),
            })
        }
    }
}

/// Monitors a running game process (CPU, memory, window state).
/// On Linux reads `/proc/<pid>/stat` and `/proc/<pid>/status` for resource usage.
#[cfg(feature = "game-process")]
pub struct GameMonitorTool;
#[cfg(feature = "game-process")]
impl Tool for GameMonitorTool {
    fn name(&self) -> &'static str {
        "game_monitor"
    }

    fn exposure(&self) -> crate::orchestration::tool::ToolExposure {
        crate::orchestration::tool::ToolExposure::Deferred
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let pid = input.payload["pid"]
            .as_u64()
            .ok_or_else(|| anyhow!("missing 'pid'"))?;

        debug!(pid = %pid, "game_monitor: monitoring process");

        // Check if process exists
        let proc_path = format!("/proc/{}", pid);
        if !std::path::Path::new(&proc_path).exists() {
            anyhow::bail!("process with PID {} is not running", pid);
        }

        // Read process name from /proc/pid/comm
        let proc_name = std::fs::read_to_string(format!("/proc/{}/comm", pid))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "unknown".to_string());

        // Parse /proc/pid/stat for CPU and state info
        let stat = std::fs::read_to_string(format!("/proc/{}/stat", pid)).ok();
        let (state, utime, stime, rss_pages) = stat
            .as_ref()
            .and_then(|s| {
                // Format: pid (comm) state ppid pgrp session tty_nr tpgid flags minflt cminflt majflt cmajflt utime stime cutime cstime ...
                // Find the closing paren of comm, then skip spaces. The paren
                // is ASCII, so `closing_paren + 1` is a valid boundary; the
                // trailing `)` byte is always ASCII too.
                let closing_paren = s.rfind(')')?;
                let after = s[closing_paren + 1..].trim_start();
                let fields: Vec<&str> = after.split_whitespace().collect();
                if fields.len() < 23 {
                    return None;
                }
                Some((
                    fields[0].to_string(),           // state
                    fields[11].parse::<u64>().ok()?, // utime (clock ticks)
                    fields[12].parse::<u64>().ok()?, // stime (clock ticks)
                    fields[21].parse::<u64>().ok()?, // rss (pages)
                ))
            })
            .unwrap_or_default();

        // Parse /proc/pid/status for memory and other info
        let status_text = std::fs::read_to_string(format!("/proc/{}/status", pid)).ok();
        let vm_rss_kb = status_text
            .as_ref()
            .and_then(|t| {
                t.lines().find_map(|line| {
                    if line.starts_with("VmRSS:") {
                        line.split_whitespace()
                            .nth(1)
                            .and_then(|v| v.parse::<u64>().ok())
                    } else {
                        None
                    }
                })
            })
            .unwrap_or(0);

        let threads = status_text
            .as_ref()
            .and_then(|t| {
                t.lines().find_map(|line| {
                    if line.starts_with("Threads:") {
                        line.split_whitespace()
                            .nth(1)
                            .and_then(|v| v.parse::<u32>().ok())
                    } else {
                        None
                    }
                })
            })
            .unwrap_or(0);

        // Convert to meaningful units
        // On Linux, clock ticks per second is 100 (USER_HZ)
        let clock_ticks_per_sec = 100;
        let cpu_time_secs = (utime + stime) as f64 / clock_ticks_per_sec as f64;
        let page_size = 4096u64; // standard 4KB pages
        let memory_bytes = rss_pages * page_size;
        // Use VmRSS for more accurate memory reporting
        let memory_kb = if vm_rss_kb > 0 {
            vm_rss_kb
        } else {
            memory_bytes / 1024
        };

        // Window activity is derived from the real process state read from
        // /proc/<pid>/stat: running (R), sleeping (S), or disk-wait (D) means
        // the process is actively executing; stopped (T) or zombie (Z) does not.
        let window_active = matches!(state.as_str(), "R" | "S" | "D");

        let report = tool_execution_report("game_monitor", Some("process_monitored"));

        Ok(ToolOutput {
            success: true,
            result: Some(json!({
                "pid": pid,
                "name": proc_name,
                "state": state,
                "cpu_time_secs": cpu_time_secs,
                "memory_kb": memory_kb,
                "memory_mb": (memory_kb as f64 / 1024.0 * 100.0).round() / 100.0,
                "threads": threads,
                "window_active": window_active,
                "monitored_at": SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            })),
            error: None,
            verification: Some("process_monitored".to_string()),
            audit_log: Some(format!("game_monitor: monitored pid {}", pid)),
            pua_report: Some(report),
        })
    }
}
