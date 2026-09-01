//! Read file lines tool (read_file_lines)
//!
//! Reads specific line ranges from a file, useful for targeted reads
//! of large files without loading the entire content.

use crate::governance::pua::tool_execution_report;
use crate::i18n::runtime::{t, tf};
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
use anyhow::{Context, Result};
use tracing::debug;

/// Maximum line-window size per call: `end_line` is model-controlled and
/// unbounded, so the window is clamped to this many lines (the response is
/// additionally bounded by the executor's output truncation).
pub(crate) const MAX_LINES_PER_CALL: usize = 100_000;

// ── ReadFileLinesTool ─────────────────────────────────────────────────────

pub struct ReadFileLinesTool;

impl Tool for ReadFileLinesTool {
    fn name(&self) -> &'static str {
        "read_file_lines"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{}", t("error.missing_path")))?;

        let validated = sanitize_path(input, path)?;

        if !validated.exists() {
            anyhow::bail!("{}", tf("error.path_not_found", &[("path", path)]));
        }

        let start_line = input.payload["start_line"].as_u64().unwrap_or(1).max(1) as usize;
        let end_line = input.payload["end_line"].as_u64().unwrap_or(50) as usize;

        if start_line > end_line {
            anyhow::bail!(
                "start_line ({}) must be <= end_line ({})",
                start_line,
                end_line
            );
        }

        // Window bound: `end_line` is model-controlled and unbounded — a
        // request for `end_line=1e9` would scan the whole file and accumulate
        // unbounded lines. Clamp the window and report the clamp explicitly.
        let window_clamped =
            end_line.saturating_sub(start_line).saturating_add(1) > MAX_LINES_PER_CALL;
        let end_line = end_line.min(start_line.saturating_add(MAX_LINES_PER_CALL - 1));

        debug!(
            path = %validated.display(),
            start_line = %start_line,
            end_line = %end_line,
            "tool: reading file lines"
        );

        // Stream the file line-by-line instead of buffering the whole file:
        // a 10GB file read for lines 1-50 must not allocate 10GB. Each line
        // is bounded (oversized lines are reported and skipped); the scan
        // stops after `end_line`.
        use std::io::{BufRead, Read};
        const MAX_LINE_BYTES: usize = 1024 * 1024; // 1 MiB per line
        let file = std::fs::File::open(&validated).context("failed to read file")?;
        let mut reader = std::io::BufReader::new(file);
        let mut total_lines = 0usize;
        let mut lines: Vec<String> = Vec::new();
        let mut line_buf: Vec<u8> = Vec::new();
        loop {
            line_buf.clear();
            let n = (&mut reader)
                .take(MAX_LINE_BYTES as u64 + 1)
                .read_until(b'\n', &mut line_buf)
                .context("failed to read file")?;
            if n == 0 {
                break; // EOF
            }
            total_lines += 1;
            // Oversized lines must be drained to the newline REGARDLESS of
            // the requested window: a >1MiB line before `start_line` would
            // otherwise leave the reader mid-line, and each subsequent
            // 1MiB fragment would be miscounted as a separate line (silent
            // line-number corruption).
            let oversized = line_buf.len() > MAX_LINE_BYTES;
            if oversized {
                // Shared drain: advances to the next line boundary without
                // buffering the line and without consuming the next line's
                // prefix.
                crate::shared::bufread::drain_to_newline(&mut reader);
            }
            if total_lines < start_line {
                continue;
            }
            if total_lines > end_line {
                break; // scanned past the requested window
            }
            if oversized {
                lines.push(format!("[line too long — > {MAX_LINE_BYTES} bytes]"));
                continue;
            }
            while matches!(line_buf.last(), Some(b'\n') | Some(b'\r')) {
                line_buf.pop();
            }
            lines.push(String::from_utf8_lossy(&line_buf).into_owned());
        }

        let truncated = total_lines > end_line;
        let reported_total = if truncated {
            total_lines - 1
        } else {
            total_lines
        };
        // Empty file: report an empty range (start == end == 0) so the
        // response never violates the start <= end contract.
        let (actual_start, actual_end) = if reported_total == 0 {
            (0, 0)
        } else {
            (start_line.min(reported_total), end_line.min(reported_total))
        };

        debug!(
            total_lines = %reported_total,
            returned_lines = %lines.len(),
            truncated = %truncated,
            "tool: read_file_lines complete"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "lines": lines,
                "start_line": actual_start,
                "end_line": actual_end,
                "total_lines": reported_total,
                "returned_count": lines.len(),
                "truncated": truncated,
                "window_clamped": window_clamped,
                "path": validated.to_string_lossy(),
            })),
            error: None,
            verification: Some("file_lines_read".to_string()),
            audit_log: Some(format!(
                "Read lines {}-{} from {} (total: {})",
                actual_start,
                actual_end,
                validated.display(),
                reported_total,
            )),
            pua_report: Some(tool_execution_report(
                "read_file_lines",
                Some("file_lines_read"),
            )),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::tool::ToolInput;
    use tempfile::TempDir;

    fn tool_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "test-lines".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload,
            allowed_base_dir: None,
        }
    }

    fn make_file(tmp: &TempDir, name: &str, line_count: usize) -> std::path::PathBuf {
        let path = tmp.path().join(name);
        let content: String = (1..=line_count)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn oversized_window_is_clamped_and_reported() {
        // Regression: `end_line` is model-controlled and unbounded — a request
        // for end_line=1e9 must be clamped to MAX_LINES_PER_CALL and reported
        // via `window_clamped` instead of scanning the whole file.
        let tmp = TempDir::new().expect("temp dir");
        let path = make_file(&tmp, "big.txt", MAX_LINES_PER_CALL + 50_000);
        let input = tool_input(serde_json::json!({
            "path": path.to_str().unwrap(),
            "start_line": 1,
            "end_line": 1_000_000_000,
        }));
        let result = ReadFileLinesTool.run(&input).expect("run");
        assert!(result.success);
        let payload = result.result.expect("payload");
        assert!(
            payload["window_clamped"].as_bool().unwrap(),
            "oversized window must be flagged"
        );
        assert_eq!(
            payload["returned_count"].as_u64().unwrap(),
            MAX_LINES_PER_CALL as u64
        );
        assert_eq!(
            payload["end_line"].as_u64().unwrap(),
            MAX_LINES_PER_CALL as u64
        );
    }

    #[test]
    fn read_specific_range() {
        let tmp = TempDir::new().expect("temp dir");
        let f = make_file(&tmp, "test.txt", 100);

        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "read".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "path": f.to_string_lossy(),
                "start_line": 10,
                "end_line": 15,
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let tool = ReadFileLinesTool;
        let output = tool.run(&input).expect("read_file_lines should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        let lines = result["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 6);
        assert_eq!(lines[0].as_str().unwrap(), "line 10");
        assert_eq!(lines[5].as_str().unwrap(), "line 15");
    }

    #[test]
    fn read_default_range() {
        let tmp = TempDir::new().expect("temp dir");
        let f = make_file(&tmp, "test.txt", 5);

        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "read".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "path": f.to_string_lossy(),
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let tool = ReadFileLinesTool;
        let output = tool.run(&input).expect("read_file_lines should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        let lines = result["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 5);
        assert_eq!(result["total_lines"].as_u64().unwrap(), 5);
    }

    #[test]
    fn read_beyond_file_length_clamps() {
        let tmp = TempDir::new().expect("temp dir");
        let f = make_file(&tmp, "short.txt", 3);

        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "read".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "path": f.to_string_lossy(),
                "start_line": 1,
                "end_line": 100,
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let tool = ReadFileLinesTool;
        let output = tool.run(&input).expect("read_file_lines should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        let lines = result["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 3);
        assert_eq!(result["end_line"].as_u64().unwrap(), 3);
    }

    #[test]
    fn rejects_nonexistent_file() {
        let input = tool_input(serde_json::json!({
            "path": "/nonexistent-path-12345/file.txt",
            "start_line": 1,
            "end_line": 10,
        }));
        let tool = ReadFileLinesTool;
        let result = tool.run(&input);
        assert!(
            result.is_err(),
            "read_file_lines should fail for nonexistent file"
        );
    }

    #[test]
    fn rejects_invalid_range() {
        let tmp = TempDir::new().expect("temp dir");
        let f = make_file(&tmp, "test.txt", 10);

        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "read".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "path": f.to_string_lossy(),
                "start_line": 10,
                "end_line": 5,
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let tool = ReadFileLinesTool;
        let result = tool.run(&input);
        assert!(
            result.is_err(),
            "read_file_lines should fail when start_line > end_line"
        );
    }

    #[test]
    fn empty_file_does_not_panic() {
        // Regression: an empty file made `actual_end - actual_start + 1`
        // underflow (0usize - 1usize) and panic in debug builds.
        let tmp = TempDir::new().expect("temp dir");
        let path = tmp.path().join("empty.txt");
        std::fs::write(&path, "").expect("write empty file");
        let input = tool_input(serde_json::json!({
            "path": path.to_str().unwrap(),
        }));
        let tool = ReadFileLinesTool;
        let result = tool.run(&input).expect("empty file must not panic");
        assert!(result.success);
        let payload = result.result.expect("result payload");
        let lines = payload["lines"].as_array().expect("lines array");
        assert!(lines.is_empty());
        // The reported range must not violate start <= end.
        assert!(payload["start_line"].as_u64().unwrap() <= payload["end_line"].as_u64().unwrap());
    }

    #[test]
    fn oversized_line_drain_preserves_following_lines() {
        // Regression (P1): the drain loop after an oversized line must only
        // consume UP TO the newline — a naive `read()` over-consumed the
        // buffer and silently dropped the next line's prefix.
        let tmp = TempDir::new().expect("temp dir");
        let path = tmp.path().join("big_line.txt");
        // Line 1: > 1 MiB (no newline until after the cap+1 bytes).
        // Line 2: a short sentinel line whose integrity we assert.
        let mut content = "x".repeat(1024 * 1024 + 100);
        content.push_str("\nsentinel-尾行\n");
        std::fs::write(&path, content).expect("write file");

        let input = tool_input(serde_json::json!({
            "path": path.to_str().unwrap(),
            "start_line": 1,
            "end_line": 3,
        }));
        let tool = ReadFileLinesTool;
        let result = tool.run(&input).expect("run");
        assert!(result.success);
        let payload = result.result.expect("payload");
        let lines = payload["lines"].as_array().expect("lines");
        assert!(
            lines[0].as_str().unwrap().starts_with("[line too long"),
            "oversized line reported, got: {:?}",
            lines[0]
        );
        assert_eq!(
            lines[1].as_str().unwrap(),
            "sentinel-尾行",
            "line after the oversized line must be intact"
        );
        assert_eq!(payload["total_lines"].as_u64().unwrap(), 2);
    }
}
