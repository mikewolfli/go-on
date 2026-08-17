//! Game input tools: keyboard and mouse simulation via xdotool
//! (feature `game-input`).

use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};
use anyhow::{anyhow, Context, Result};
use serde_json::json;
use tracing::{debug, info, warn};

// ═══════════════════════════════════════════════════════════════════════════════
// Section 4: Game Input Tools (Accessibility/Automation)
//   #[cfg(feature = "game-input")]
// ═══════════════════════════════════════════════════════════════════════════════

/// Simulates keyboard input for accessibility or single-player automation.
/// Uses `xdotool` on Linux if available.
#[cfg(feature = "game-input")]
pub struct GameKeyboardInputTool;
#[cfg(feature = "game-input")]
impl Tool for GameKeyboardInputTool {
    fn name(&self) -> &'static str {
        "game_keyboard_input"
    }

    fn exposure(&self) -> crate::orchestration::tool::ToolExposure {
        crate::orchestration::tool::ToolExposure::Deferred
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let keys = input.payload["keys"]
            .as_str()
            .ok_or_else(|| anyhow!("missing 'keys' — e.g. 'w', 'Return', 'Escape', 'ctrl+c'"))?;
        let delay_ms = input.payload["delay_ms"].as_u64().unwrap_or(50);
        let window = input.payload["window_title"].as_str();

        debug!(keys = %keys, "game_keyboard_input: simulating keys");

        let mut cmd = std::process::Command::new("xdotool");

        // If a window title is specified, focus it first
        if let Some(title) = window {
            cmd.args(["search", "--name", title, "windowactivate"]);
        }

        cmd.args(["--delay", &delay_ms.to_string(), "key", keys]);

        let result = cmd
            .output()
            .context("failed to run xdotool. Install it: sudo apt install xdotool")?;

        let report = tool_execution_report("game_keyboard_input", Some("input_sent"));

        if result.status.success() {
            info!(keys = %keys, "game_keyboard_input: sent successfully");
            Ok(ToolOutput {
                success: true,
                result: Some(json!({
                    "action": "key_press",
                    "keys": keys,
                    "window": window,
                    "status": "sent",
                })),
                error: None,
                verification: Some("input_sent".to_string()),
                audit_log: Some(format!("game_keyboard_input: sent '{}'", keys)),
                pua_report: Some(report),
            })
        } else {
            let stderr = String::from_utf8_lossy(&result.stderr).to_string();
            warn!(keys = %keys, error = %stderr, "game_keyboard_input: xdotool failed");
            Ok(ToolOutput {
                success: false,
                result: Some(json!({
                    "action": "key_press",
                    "keys": keys,
                    "error": stderr,
                    "note": "xdotool failed. Ensure a graphical session is active and xdotool is installed.",
                })),
                error: Some(stderr),
                verification: Some("input_failed".to_string()),
                audit_log: Some(format!("game_keyboard_input: failed '{}'", keys)),
                pua_report: Some(report),
            })
        }
    }
}

/// Simulates mouse input for accessibility or single-player automation.
/// Uses `xdotool` on Linux if available.
#[cfg(feature = "game-input")]
pub struct GameMouseInputTool;
#[cfg(feature = "game-input")]
impl Tool for GameMouseInputTool {
    fn name(&self) -> &'static str {
        "game_mouse_input"
    }

    fn exposure(&self) -> crate::orchestration::tool::ToolExposure {
        crate::orchestration::tool::ToolExposure::Deferred
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let x = input.payload["x"]
            .as_f64()
            .ok_or_else(|| anyhow!("missing 'x' coordinate"))?;
        let y = input.payload["y"]
            .as_f64()
            .ok_or_else(|| anyhow!("missing 'y' coordinate"))?;
        let action = input.payload["action"].as_str().unwrap_or("click");
        let window = input.payload["window_title"].as_str();
        let button = input.payload["button"].as_str().unwrap_or("1");

        debug!(x = %x, y = %y, action = %action, "game_mouse_input: simulating mouse");

        let mut cmd = std::process::Command::new("xdotool");

        // If a window title is specified, focus it first
        if let Some(title) = window {
            cmd.args(["search", "--name", title, "windowactivate"]);
            // Small sleep to let window focus
            cmd.args(["sleep", "0.1"]);
        }

        // Move mouse to position
        cmd.args(["mousemove", &x.to_string(), &y.to_string()]);

        match action {
            "click" => {
                cmd.args(["click", button]);
            }
            "doubleclick" => {
                cmd.args(["click", "--repeat", "2", button]);
            }
            "mousedown" => {
                cmd.args(["mousedown", button]);
            }
            "mouseup" => {
                cmd.args(["mouseup", button]);
            }
            "move" => {
                // Already moved above — just a no-op
            }
            "scroll" => {
                let amount = input.payload["amount"].as_i64().unwrap_or(1);
                let direction = if amount > 0 { "4" } else { "5" }; // 4=up, 5=down
                cmd.args([
                    "click",
                    "--repeat",
                    &amount.unsigned_abs().to_string(),
                    direction,
                ]);
            }
            other => {
                anyhow::bail!("unsupported mouse action: '{}'. Supported: click, doubleclick, mousedown, mouseup, move, scroll", other);
            }
        }

        let result = cmd
            .output()
            .context("failed to run xdotool. Install it: sudo apt install xdotool")?;

        let report = tool_execution_report("game_mouse_input", Some("input_sent"));

        if result.status.success() {
            info!(action = %action, x = %x, y = %y, "game_mouse_input: sent successfully");
            Ok(ToolOutput {
                success: true,
                result: Some(json!({
                    "x": x,
                    "y": y,
                    "action": action,
                    "button": button,
                    "window": window,
                    "status": "sent",
                })),
                error: None,
                verification: Some("input_sent".to_string()),
                audit_log: Some(format!("game_mouse_input: {} at ({}, {})", action, x, y)),
                pua_report: Some(report),
            })
        } else {
            let stderr = String::from_utf8_lossy(&result.stderr).to_string();
            warn!(action = %action, error = %stderr, "game_mouse_input: xdotool failed");
            Ok(ToolOutput {
                success: false,
                result: Some(json!({
                    "x": x,
                    "y": y,
                    "action": action,
                    "error": stderr,
                    "note": "xdotool failed. Ensure a graphical session is active and xdotool is installed.",
                })),
                error: Some(stderr),
                verification: Some("input_failed".to_string()),
                audit_log: Some(format!(
                    "game_mouse_input: failed {} at ({}, {})",
                    action, x, y
                )),
                pua_report: Some(report),
            })
        }
    }
}
