//! Game screen tools: screen capture and replay recording
//! (feature `game-screen`).

use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};
use anyhow::Result;
use serde_json::json;
use tracing::debug;

// ═══════════════════════════════════════════════════════════════════════════════
// Section 3: Screen Capture & Replay Tools   #[cfg(feature = "game-screen")]
// ═══════════════════════════════════════════════════════════════════════════════

/// Captures the game window screen for analysis (accessibility/coaching).
/// Attempts to use ImageMagick's `import` or `maim`/`scrot` if available;
/// otherwise returns guidance on how to capture.
#[cfg(feature = "game-screen")]
pub struct GameScreenCaptureTool;
#[cfg(feature = "game-screen")]
impl Tool for GameScreenCaptureTool {
    fn name(&self) -> &'static str {
        "game_screen_capture"
    }

    fn exposure(&self) -> crate::orchestration::tool::ToolExposure {
        crate::orchestration::tool::ToolExposure::Deferred
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let window = input.payload["window_title"].as_str().unwrap_or("game");
        let output_path = input.payload["output_path"]
            .as_str()
            .unwrap_or("/tmp/game_capture.png");
        let output = std::path::Path::new(output_path);

        // Ensure parent directory exists
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        debug!(window = %window, output = %output_path, "game_screen_capture: capturing");

        // Try available capture tools
        let result = try_capture_with_import(window, output)
            .or_else(|| try_capture_with_maim(window, output))
            .or_else(|| try_capture_with_scrot(output));

        let report = tool_execution_report("game_screen_capture", Some("screen_captured"));

        if let Some(captured_path) = result {
            let metadata = std::fs::metadata(&captured_path).ok();
            let file_size = metadata.map(|m| m.len()).unwrap_or(0);

            Ok(ToolOutput {
                success: true,
                result: Some(json!({
                    "window_title": window,
                    "output_path": captured_path.to_string_lossy(),
                    "file_size_bytes": file_size,
                    "tool": "system_capture",
                })),
                error: None,
                verification: Some("screen_captured".to_string()),
                audit_log: Some(format!(
                    "game_screen_capture: captured '{}' to {}",
                    window,
                    captured_path.display()
                )),
                pua_report: Some(report),
            })
        } else {
            // No capture binary available — report failure honestly instead
            // of returning success with a "note".
            let note = "No screen capture tool found (tried: import, maim, scrot). Install imagemagick ('import') or maim.";
            Ok(ToolOutput {
                success: false,
                result: Some(json!({
                    "window_title": window,
                    "output_path": output_path,
                    "note": note,
                    "suggestion": "sudo apt install imagemagick  # or: brew install imagemagick"
                })),
                error: Some(note.to_string()),
                verification: None,
                audit_log: Some(format!(
                    "game_screen_capture: failed, no tool available for '{}'",
                    window
                )),
                pua_report: Some(report),
            })
        }
    }
}

#[cfg(feature = "game-screen")]
fn try_capture_with_import(window: &str, output: &std::path::Path) -> Option<std::path::PathBuf> {
    // `import -window <title> <output>` from ImageMagick
    let result = std::process::Command::new("import")
        .arg("-window")
        .arg(window)
        .arg(output.as_os_str())
        .output()
        .ok()?;
    if result.status.success() && output.exists() {
        Some(output.to_path_buf())
    } else {
        None
    }
}

#[cfg(feature = "game-screen")]
fn try_capture_with_maim(window: &str, output: &std::path::Path) -> Option<std::path::PathBuf> {
    // `maim -i <window_id> <output>` — requires window ID, not title
    // Try to find window ID via xdotool first
    let window_id = std::process::Command::new("xdotool")
        .args(["search", "--name", window])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout)
                    .ok()
                    .and_then(|s| s.lines().next().map(|l| l.to_string()))
            } else {
                None
            }
        })?;

    let result = std::process::Command::new("maim")
        .args(["-i", &window_id, output.as_os_str().to_str()?])
        .output()
        .ok()?;
    if result.status.success() && output.exists() {
        Some(output.to_path_buf())
    } else {
        None
    }
}

#[cfg(feature = "game-screen")]
fn try_capture_with_scrot(output: &std::path::Path) -> Option<std::path::PathBuf> {
    let result = std::process::Command::new("scrot")
        .arg(output.as_os_str())
        .output()
        .ok()?;
    if result.status.success() && output.exists() {
        Some(output.to_path_buf())
    } else {
        None
    }
}

/// Records a replay from the game window (accessibility/content creation).
/// Uses `ffmpeg` for screen recording if available.
#[cfg(feature = "game-screen")]
pub struct GameReplayRecorderTool;
#[cfg(feature = "game-screen")]
impl Tool for GameReplayRecorderTool {
    fn name(&self) -> &'static str {
        "game_replay_recorder"
    }

    fn exposure(&self) -> crate::orchestration::tool::ToolExposure {
        crate::orchestration::tool::ToolExposure::Deferred
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let duration_secs = input.payload["duration_secs"].as_u64().unwrap_or(30);
        let output_path = input.payload["output_path"]
            .as_str()
            .unwrap_or("/tmp/game_replay.mp4");
        let fps = input.payload["fps"].as_u64().unwrap_or(30);
        let display = input.payload["display"].as_str().unwrap_or(":0.0");

        let output = std::path::Path::new(output_path);
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        // Check if ffmpeg is available
        let ffmpeg_check = std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .ok();

        let report = tool_execution_report("game_replay_recorder", Some("replay_recorded"));

        if let Some(_check) = ffmpeg_check {
            // Actually record the screen with ffmpeg x11grab instead of only
            // reporting that recording is "ready".
            let duration = duration_secs.to_string();
            let fps_str = fps.to_string();
            let display_input = display.to_string();
            let result = std::process::Command::new("ffmpeg")
                .args([
                    "-y",
                    "-f",
                    "x11grab",
                    "-framerate",
                    &fps_str,
                    "-i",
                    &display_input,
                    "-t",
                    &duration,
                    "-c:v",
                    "libx264",
                    "-preset",
                    "ultrafast",
                    "-pix_fmt",
                    "yuv420p",
                    output_path,
                ])
                .output();
            match result {
                Ok(out) if out.status.success() && output.exists() => {
                    let file_size = std::fs::metadata(output).map(|m| m.len()).unwrap_or(0);
                    Ok(ToolOutput {
                        success: true,
                        result: Some(json!({
                            "duration_secs": duration_secs,
                            "output_path": output_path,
                            "fps": fps,
                            "format": "mp4",
                            "status": "recorded",
                            "file_size_bytes": file_size,
                        })),
                        error: None,
                        verification: Some("replay_recorded".to_string()),
                        audit_log: Some(format!(
                            "game_replay_recorder: recorded {}s to {} ({} bytes)",
                            duration_secs, output_path, file_size
                        )),
                        pua_report: Some(report),
                    })
                }
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    let tail: String = stderr.lines().rev().take(5).collect::<Vec<_>>().join("\n");
                    let err_msg =
                        format!("ffmpeg recording failed for '{}': {}", output_path, tail);
                    Ok(ToolOutput {
                        success: false,
                        result: Some(json!({
                            "duration_secs": duration_secs,
                            "output_path": output_path,
                            "fps": fps,
                            "format": "mp4",
                            "status": "failed",
                        })),
                        error: Some(err_msg.clone()),
                        verification: None,
                        audit_log: Some(format!(
                            "game_replay_recorder: recording failed for '{}'",
                            output_path
                        )),
                        pua_report: Some(report),
                    })
                }
                Err(e) => Ok(ToolOutput {
                    success: false,
                    result: Some(json!({
                        "duration_secs": duration_secs,
                        "output_path": output_path,
                        "fps": fps,
                        "status": "failed",
                    })),
                    error: Some(format!("ffmpeg failed to start: {}", e)),
                    verification: None,
                    audit_log: Some(format!(
                        "game_replay_recorder: ffmpeg failed to start for '{}'",
                        output_path
                    )),
                    pua_report: Some(report),
                }),
            }
        } else {
            Ok(ToolOutput {
                success: false,
                result: Some(json!({
                    "duration_secs": duration_secs,
                    "output_path": output_path,
                    "fps": fps,
                    "format": "mp4",
                    "status": "ffmpeg_not_found",
                    "note": "ffmpeg is not installed. Install it to enable screen recording.",
                    "suggestion": "sudo apt install ffmpeg  # or: brew install ffmpeg"
                })),
                error: Some("ffmpeg is not installed; cannot record screen replay".to_string()),
                verification: None,
                audit_log: Some("game_replay_recorder: ffmpeg unavailable".to_string()),
                pua_report: Some(report),
            })
        }
    }
}
