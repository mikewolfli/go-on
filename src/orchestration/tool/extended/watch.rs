//! File watching tool.
//!
//! Watches files or directories for changes.
//! Uses the `notify` crate (already a dependency).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::LazyLock;
use std::sync::Mutex;

use anyhow::{Context, Result};
use serde_json::{json, Value};
use tracing::debug;

use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};

/// Global snapshot store mapping a watch session ID to file modification times.
///
/// This allows the tool to be called multiple times and detect which files
/// changed between calls.
static SNAPSHOTS: LazyLock<Mutex<HashMap<String, HashMap<PathBuf, std::time::SystemTime>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub struct FileWatchTool;

impl Tool for FileWatchTool {
    fn name(&self) -> &'static str {
        "file_watch"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let directory = input
            .payload
            .get("directory")
            .and_then(|v| v.as_str())
            .unwrap_or(".");

        let pattern = input
            .payload
            .get("pattern")
            .and_then(|v| v.as_str())
            .unwrap_or("**/*");

        // Session ID allows multiple independent watch sessions.
        let session = input
            .payload
            .get("session")
            .and_then(|v| v.as_str())
            .unwrap_or("default");

        let reset = input
            .payload
            .get("reset")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let base_dir = sanitize_path(input, directory)?;
        debug!(
            directory = %directory,
            pattern = %pattern,
            session = %session,
            reset = %reset,
            "tool: file_watch"
        );

        // Collect current file modification times.
        let current_snapshot = collect_files(&base_dir, pattern)?;

        if current_snapshot.is_empty() {
            return Ok(ToolOutput {
                success: true,
                result: Some(json!({
                    "changed_files": [],
                    "total_files": 0,
                    "is_baseline": true,
                    "note": "No files matched the given pattern in the directory"
                })),
                error: None,
                verification: Some("file_watch_completed".to_string()),
                audit_log: Some("file_watch: no files matched".to_string()),
                pua_report: Some(tool_execution_report(
                    "file_watch",
                    Some("file_watch_completed"),
                )),
            });
        }

        let total_files = current_snapshot.len();
        let mut snapshots = SNAPSHOTS.lock().unwrap();

        if reset || !snapshots.contains_key(session) {
            // Store baseline and return no changes.
            let file_list: Vec<String> = current_snapshot
                .keys()
                .map(|p| p.to_string_lossy().to_string())
                .collect();

            snapshots.insert(session.to_string(), current_snapshot);

            debug!(
                session = %session,
                files = %file_list.len(),
                "tool: file_watch baseline set"
            );

            return Ok(ToolOutput {
                success: true,
                result: Some(json!({
                    "changed_files": [],
                    "total_files": file_list.len(),
                    "is_baseline": true,
                    "files": file_list,
                })),
                error: None,
                verification: Some("file_watch_completed".to_string()),
                audit_log: Some(format!(
                    "file_watch: baseline session '{}' with {} files",
                    session,
                    file_list.len()
                )),
                pua_report: Some(tool_execution_report(
                    "file_watch",
                    Some("file_watch_completed"),
                )),
            });
        }

        // Compare against stored baseline.
        let baseline = snapshots.get(session).unwrap();
        let mut changed_files: Vec<Value> = Vec::new();
        let mut removed_files: Vec<String> = Vec::new();
        let mut added_files: Vec<String> = Vec::new();
        let mut modified_files: Vec<String> = Vec::new();

        // Check for new or modified files.
        for (path, mtime) in &current_snapshot {
            let path_str = path.to_string_lossy().to_string();
            match baseline.get(path) {
                Some(old_mtime) if *old_mtime != *mtime => {
                    modified_files.push(path_str.clone());
                    changed_files.push(json!({
                        "path": path_str,
                        "change": "modified",
                        "new_mtime": format_mtime(*mtime),
                        "old_mtime": format_mtime(*old_mtime),
                    }));
                }
                None => {
                    added_files.push(path_str.clone());
                    changed_files.push(json!({
                        "path": path_str,
                        "change": "added",
                        "mtime": format_mtime(*mtime),
                    }));
                }
                // File exists and mtime unchanged — no change to report.
                Some(_) => {}
            }
        }

        // Check for removed files.
        for (path, old_mtime) in baseline {
            let path_str = path.to_string_lossy().to_string();
            if !current_snapshot.contains_key(path) {
                removed_files.push(path_str.clone());
                changed_files.push(json!({
                    "path": path_str,
                    "change": "removed",
                    "old_mtime": format_mtime(*old_mtime),
                }));
            }
        }

        // Update baseline with current snapshot.
        snapshots.insert(session.to_string(), current_snapshot);

        debug!(
            session = %session,
            changed = %changed_files.len(),
            added = %added_files.len(),
            modified = %modified_files.len(),
            removed = %removed_files.len(),
            "tool: file_watch complete"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(json!({
                "changed_files": changed_files,
                "added": added_files,
                "modified": modified_files,
                "removed": removed_files,
                "total_files": total_files,
                "is_baseline": false,
            })),
            error: None,
            verification: Some("file_watch_completed".to_string()),
            audit_log: Some(format!(
                "file_watch: session '{}' — {} changed ({} added, {} modified, {} removed)",
                session,
                changed_files.len(),
                added_files.len(),
                modified_files.len(),
                removed_files.len()
            )),
            pua_report: Some(tool_execution_report(
                "file_watch",
                Some("file_watch_completed"),
            )),
        })
    }
}

/// Collect all files matching the glob pattern and their modification times.
fn collect_files(
    base_dir: &std::path::Path,
    pattern: &str,
) -> Result<HashMap<PathBuf, std::time::SystemTime>> {
    let mut files = HashMap::new();

    let full_pattern = base_dir.join(pattern);
    let pattern_str = full_pattern.to_string_lossy().to_string();

    let walker = glob::glob(&pattern_str).context("failed to parse glob pattern for file watch")?;

    for entry in walker.filter_map(Result::ok) {
        if !entry.is_file() {
            continue;
        }

        let metadata = match std::fs::metadata(&entry) {
            Ok(m) => m,
            Err(_) => continue,
        };

        let mtime = match metadata.modified() {
            Ok(t) => t,
            Err(_) => continue,
        };

        files.insert(entry, mtime);
    }

    Ok(files)
}

/// Format a SystemTime to an ISO-like string for human readability.
fn format_mtime(time: std::time::SystemTime) -> String {
    use std::time::UNIX_EPOCH;

    match time.duration_since(UNIX_EPOCH) {
        Ok(dur) => {
            let secs = dur.as_secs();
            // Simple ISO-like format.
            let days = secs / 86400;
            let time_secs = secs % 86400;
            let hours = time_secs / 3600;
            let minutes = (time_secs % 3600) / 60;
            let seconds = time_secs % 60;
            format!("{}+{:02}:{:02}:{:02}", days, hours, minutes, seconds)
        }
        Err(_) => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::tool::ToolInput;
    use tempfile::TempDir;

    fn tool_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "test-watch".to_string(),
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
    fn file_watch_creates_baseline_on_first_call() {
        // Clear the session to avoid interference from other tests.
        SNAPSHOTS.lock().unwrap().remove("test_session");

        let tmp = TempDir::new().expect("temp dir");
        std::fs::write(tmp.path().join("hello.txt"), "world").unwrap();

        let input = tool_input(json!({
            "directory": tmp.path().to_string_lossy(),
            "pattern": "*.txt",
            "session": "test_session",
        }));
        let tool = FileWatchTool;
        let output = tool.run(&input).expect("file_watch should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        assert!(
            result["is_baseline"].as_bool().unwrap(),
            "first call should be baseline"
        );
        assert_eq!(result["total_files"].as_u64().unwrap(), 1);
    }

    #[test]
    fn file_watch_detects_new_file() {
        SNAPSHOTS.lock().unwrap().remove("test_new");

        let tmp = TempDir::new().expect("temp dir");
        std::fs::write(tmp.path().join("a.txt"), "initial").unwrap();

        let tool = FileWatchTool;

        // First call: baseline.
        let input1 = tool_input(json!({
            "directory": tmp.path().to_string_lossy(),
            "pattern": "*.txt",
            "session": "test_new",
        }));
        tool.run(&input1).expect("baseline should succeed");

        // Add a file.
        std::fs::write(tmp.path().join("b.txt"), "new file").unwrap();

        // Second call: should detect the new file.
        let input2 = tool_input(json!({
            "directory": tmp.path().to_string_lossy(),
            "pattern": "*.txt",
            "session": "test_new",
        }));
        let output2 = tool.run(&input2).expect("second call should succeed");
        assert!(output2.success);
        let result2 = output2.result.unwrap();
        assert!(
            !result2["is_baseline"].as_bool().unwrap(),
            "second call should not be baseline"
        );
        assert_eq!(
            result2["added"].as_array().unwrap().len(),
            1,
            "should detect 1 added file"
        );
    }

    #[test]
    fn file_watch_reset_clears_baseline() {
        SNAPSHOTS.lock().unwrap().remove("test_reset");

        let tmp = TempDir::new().expect("temp dir");
        std::fs::write(tmp.path().join("f.txt"), "data").unwrap();

        let tool = FileWatchTool;

        // First call: baseline.
        let input1 = tool_input(json!({
            "directory": tmp.path().to_string_lossy(),
            "pattern": "*.txt",
            "session": "test_reset",
        }));
        tool.run(&input1).expect("baseline should succeed");

        // Call with reset=true — should create new baseline.
        let input2 = tool_input(json!({
            "directory": tmp.path().to_string_lossy(),
            "pattern": "*.txt",
            "session": "test_reset",
            "reset": true,
        }));
        let output2 = tool.run(&input2).expect("reset call should succeed");
        assert!(output2.success);
        let result2 = output2.result.unwrap();
        assert!(
            result2["is_baseline"].as_bool().unwrap(),
            "reset should create new baseline"
        );
    }
}
