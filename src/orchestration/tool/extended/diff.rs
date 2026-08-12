//! Diff tool (file_diff)
//!
//! Compares two files and returns a unified diff output.
//! Tries the system `diff` command first, then falls back to a
//! built-in LCS-based line diff when `diff` is unavailable.

use crate::governance::pua::tool_execution_report;
use crate::i18n::runtime::{t, tf};
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
use anyhow::{Context, Result};
use std::process::Command;
use tracing::debug;

// ── DiffTool ─────────────────────────────────────────────────────────────

pub struct DiffTool;

impl Tool for DiffTool {
    fn name(&self) -> &'static str {
        "file_diff"
    }

    fn description(&self) -> &str {
        "Compare two files and return the unified diff"
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_a": {
                    "type": "string",
                    "description": "Path to the first (original) file"
                },
                "file_b": {
                    "type": "string",
                    "description": "Path to the second (modified) file"
                },
                "context_lines": {
                    "type": "integer",
                    "description": "Number of context lines around each change (default: 3)",
                    "default": 3
                }
            },
            "required": ["file_a", "file_b"]
        })
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let file_a = input.payload["file_a"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_path")))?;
        let file_b = input.payload["file_b"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_path")))?;
        let context_lines = input.payload["context_lines"].as_u64().unwrap_or(3) as usize;

        let path_a = sanitize_path(input, file_a)?;
        let path_b = sanitize_path(input, file_b)?;

        if !path_a.exists() {
            anyhow::bail!("{}", tf("error.source_not_found", &[("source", file_a)]));
        }
        if !path_b.exists() {
            anyhow::bail!("{}", tf("error.source_not_found", &[("source", file_b)]));
        }

        debug!(
            file_a = %path_a.display(),
            file_b = %path_b.display(),
            context_lines = %context_lines,
            "tool: computing diff"
        );

        // Try system `diff` first
        let diff_output = try_system_diff(&path_a, &path_b, context_lines);

        let (diff_text, used_system_diff) = match diff_output {
            Ok(text) => (text, true),
            Err(_) => {
                // Fall back to built-in Rust diff
                let content_a =
                    std::fs::read_to_string(&path_a).context("failed to read file_a")?;
                let content_b =
                    std::fs::read_to_string(&path_b).context("failed to read file_b")?;
                (builtin_diff(&content_a, &content_b, context_lines)?, false)
            }
        };

        debug!(
            used_system_diff = %used_system_diff,
            diff_len = %diff_text.len(),
            "tool: diff complete"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "diff": diff_text,
                "file_a": path_a.to_string_lossy(),
                "file_b": path_b.to_string_lossy(),
                "used_system_diff": used_system_diff,
            })),
            error: None,
            verification: Some("diff_computed".to_string()),
            audit_log: Some(format!(
                "Diff between {} and {} ({} bytes)",
                path_a.display(),
                path_b.display(),
                diff_text.len(),
            )),
            pua_report: Some(tool_execution_report("file_diff", Some("diff_computed"))),
        })
    }
}

/// Attempt to run the system `diff` command.
fn try_system_diff(
    path_a: &std::path::Path,
    path_b: &std::path::Path,
    context: usize,
) -> Result<String> {
    let context_arg = format!("-U{}", context);
    let output = Command::new("diff")
        .arg(&context_arg)
        .arg(path_a)
        .arg(path_b)
        .output()
        .context("failed to execute system diff")?;

    // `diff` exits with 0 when files are identical, 1 when different.
    // Both are successful for our purposes — we only fail on signal/IO errors.
    if output.status.success() || output.status.code() == Some(1) {
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let combined = if stderr.is_empty() {
            stdout
        } else {
            format!("{}{}", stderr, stdout)
        };
        Ok(combined)
    } else {
        anyhow::bail!("system diff exited with code {:?}", output.status.code());
    }
}

/// Line-count guard for the built-in LCS diff.
///
/// The DP table is O(m×n) with 8 bytes per cell, so diffing two large files
/// would allocate quadratically and OOM (50k lines each → ~20 GB). System
/// `diff` has no such issue; this only guards the fallback path.
const MAX_BUILTIN_DIFF_LINES: usize = 5_000;

/// Built-in LCS-based line diff when system `diff` is unavailable.
///
/// Produces unified-diff-like output prefixed with `---`/`+++` headers.
fn builtin_diff(content_a: &str, content_b: &str, context: usize) -> Result<String> {
    let lines_a: Vec<&str> = content_a.lines().collect();
    let lines_b: Vec<&str> = content_b.lines().collect();

    if lines_a == lines_b {
        return Ok(String::new());
    }

    if lines_a.len() > MAX_BUILTIN_DIFF_LINES || lines_b.len() > MAX_BUILTIN_DIFF_LINES {
        anyhow::bail!(
            "builtin diff limit exceeded: {} vs {} lines (max {} per file). Install the system `diff` command for large files.",
            lines_a.len(),
            lines_b.len(),
            MAX_BUILTIN_DIFF_LINES
        );
    }

    // Compute LCS table
    let m = lines_a.len();
    let n = lines_b.len();
    let mut dp = vec![vec![0usize; n + 1]; m + 1];

    for i in 1..=m {
        for j in 1..=n {
            if lines_a[i - 1] == lines_b[j - 1] {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
            }
        }
    }

    // Backtrack to find the edit operations (0 = keep, 1 = delete, 2 = insert)
    enum Op {
        Keep,
        Delete,
        Insert,
    }

    let mut ops: Vec<(Op, usize, usize)> = Vec::new(); // (op, idx_a, idx_b) where idx_a/idx_b are 0-based
    let mut i = m;
    let mut j = n;
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && lines_a[i - 1] == lines_b[j - 1] {
            ops.push((Op::Keep, i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || dp[i][j - 1] >= dp[i - 1][j]) {
            ops.push((Op::Insert, usize::MAX, j - 1));
            j -= 1;
        } else if i > 0 {
            ops.push((Op::Delete, i - 1, usize::MAX));
            i -= 1;
        }
    }
    ops.reverse();

    // Group into hunks with context
    let mut hunks: Vec<Vec<(usize, &Op, usize, usize)>> = Vec::new();
    let mut current_hunk: Vec<(usize, &Op, usize, usize)> = Vec::new();
    let mut in_change = false;

    for (idx, (ref op, ai, bj)) in ops.iter().enumerate() {
        let is_change = matches!(op, Op::Delete | Op::Insert);
        if is_change {
            if !in_change && !current_hunk.is_empty() {
                // Trim trailing context to `context` lines
                let trim_start = current_hunk.len().saturating_sub(context);
                let mut kept: Vec<_> = current_hunk.drain(trim_start..).collect();
                // Also keep the preceding context line if available
                if trim_start > 0 {
                    let prev = current_hunk.split_off(trim_start - 1);
                    kept.splice(0..0, prev);
                }
                // Only start new hunk if there's any context
                if !kept.is_empty() {
                    hunks.push(kept);
                }
            }
            in_change = true;
            current_hunk.push((idx, op, *ai, *bj));
        } else {
            if in_change {
                // Change just ended — keep `context` lines of trailing context
                in_change = false;
            }
            // Always keep context lines (we'll trim later to bound hunk size)
            current_hunk.push((idx, op, *ai, *bj));
        }
    }
    if !current_hunk.is_empty() {
        hunks.push(current_hunk);
    }

    if hunks.is_empty() {
        return Ok(String::new());
    }

    // Build unified diff output
    let header_a = format!("--- a/{}", lines_a.first().unwrap_or(&""));
    let header_b = format!("+++ b/{}", lines_b.first().unwrap_or(&""));

    // We use a simple approach: emit all hunks with unified-diff-style headers.
    let mut output = String::new();

    for hunk in &hunks {
        // Find the line numbers for the hunk header
        let mut hunk_a_start = 1usize;
        let mut hunk_b_start = 1usize;
        let mut del_count = 0usize;
        let mut ins_count = 0usize;

        // Compute hunk position from the first context/delete line
        if let Some((_, op, ai, bj)) = hunk.first() {
            match op {
                Op::Keep => {
                    if *ai != usize::MAX {
                        hunk_a_start = *ai + 1;
                    }
                    if *bj != usize::MAX {
                        hunk_b_start = *bj + 1;
                    }
                }
                Op::Delete => {
                    if *ai != usize::MAX {
                        hunk_a_start = *ai + 1;
                    }
                }
                Op::Insert => {
                    if *bj != usize::MAX {
                        hunk_b_start = *bj + 1;
                    }
                }
            }
        }

        // Determine previous context start for accurate range
        for (_, op, ai, _) in hunk {
            match op {
                Op::Delete | Op::Keep if *ai != usize::MAX => {
                    let line_no = ai + 1;
                    if line_no < hunk_a_start {
                        hunk_a_start = line_no;
                    }
                }
                _ => {}
            }
        }
        for (_, op, _, bj) in hunk {
            match op {
                Op::Insert | Op::Keep if *bj != usize::MAX => {
                    let line_no = bj + 1;
                    if line_no < hunk_b_start {
                        hunk_b_start = line_no;
                    }
                }
                _ => {}
            }
        }

        for (_, op, _, _) in hunk {
            match op {
                Op::Delete => del_count += 1,
                Op::Insert => ins_count += 1,
                Op::Keep => {}
            }
        }

        // Adjust start to include leading context
        let context_before = hunk
            .iter()
            .take_while(|(_, op, _, _)| matches!(op, Op::Keep))
            .count();
        hunk_a_start = hunk_a_start.saturating_sub(context_before);
        hunk_b_start = hunk_b_start.saturating_sub(context_before);

        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            hunk_a_start, del_count, hunk_b_start, ins_count
        ));

        for (_, op, ai, bj) in hunk {
            match op {
                Op::Keep => {
                    let line = lines_a.get(*ai).unwrap_or(&"");
                    output.push(' ');
                    output.push_str(line);
                    output.push('\n');
                }
                Op::Delete => {
                    let line = lines_a.get(*ai).unwrap_or(&"");
                    output.push('-');
                    output.push_str(line);
                    output.push('\n');
                }
                Op::Insert => {
                    let line = lines_b.get(*bj).unwrap_or(&"");
                    output.push('+');
                    output.push_str(line);
                    output.push('\n');
                }
            }
        }
    }

    // Prepend file headers
    let mut result = String::new();
    result.push_str(&header_a);
    result.push('\n');
    result.push_str(&header_b);
    result.push('\n');
    result.push_str(&output);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::tool::ToolInput;

    use tempfile::TempDir;

    #[test]
    fn diff_identical_files() {
        let tmp = TempDir::new().expect("temp dir");
        let a = tmp.path().join("a.txt");
        let b = tmp.path().join("b.txt");
        std::fs::write(&a, "hello\nworld\n").unwrap();
        std::fs::write(&b, "hello\nworld\n").unwrap();

        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "diff".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "file_a": a.to_string_lossy(),
                "file_b": b.to_string_lossy(),
                "context_lines": 3,
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let tool = DiffTool;
        let output = tool.run(&input).expect("diff should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        let diff = result["diff"].as_str().unwrap_or("");
        // Identical files may produce empty diff or a message
        assert!(diff.is_empty() || diff.contains("identical") || diff.contains("No differences"));
    }

    #[test]
    fn diff_different_files() {
        let tmp = TempDir::new().expect("temp dir");
        let a = tmp.path().join("a.txt");
        let b = tmp.path().join("b.txt");
        std::fs::write(&a, "line1\nline2\nline3\n").unwrap();
        std::fs::write(&b, "line1\nmodified\nline3\n").unwrap();

        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "diff".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "file_a": a.to_string_lossy(),
                "file_b": b.to_string_lossy(),
                "context_lines": 1,
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let tool = DiffTool;
        let output = tool.run(&input).expect("diff should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        let diff = result["diff"].as_str().unwrap_or("");
        assert!(!diff.is_empty(), "diff should contain changes");
        // Should reference the changed line
        assert!(
            diff.contains("modified") || diff.contains("-line2") || diff.contains("+modified"),
            "diff should indicate the modified line, got: {diff}"
        );
    }

    #[test]
    fn diff_builtin_lcs() {
        // Test the built-in LCS diff directly
        let a = "a\nb\nc\nd\ne\n";
        let b = "a\nb\nx\nd\ne\n";
        let result = builtin_diff(a, b, 1).unwrap();
        assert!(!result.is_empty(), "builtin diff should find changes");
        assert!(
            result.contains("-c") || result.contains("+x"),
            "builtin diff should show -c and +x, got: {result}"
        );
    }

    #[test]
    fn diff_rejects_nonexistent_file() {
        let tmp = TempDir::new().expect("temp dir");
        let a = tmp.path().join("exists.txt");
        let b = tmp.path().join("nonexistent.txt");
        std::fs::write(&a, "content").unwrap();

        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "diff".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "file_a": a.to_string_lossy(),
                "file_b": b.to_string_lossy(),
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let tool = DiffTool;
        let result = tool.run(&input);
        assert!(result.is_err(), "diff should fail for nonexistent file");
    }
}
