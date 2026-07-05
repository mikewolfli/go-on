//! Read file lines tool (read_file_lines)
//!
//! Reads specific line ranges from a file, useful for targeted reads
//! of large files without loading the entire content.

use crate::governance::pua::tool_execution_report;
use crate::i18n::runtime::{t, tf};
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
use anyhow::{Context, Result};
use tracing::debug;

// ── ReadFileLinesTool ─────────────────────────────────────────────────────

pub struct ReadFileLinesTool;

impl Tool for ReadFileLinesTool {
    fn name(&self) -> &'static str {
        "read_file_lines"
    }

    fn description(&self) -> &str {
        "Read specific line ranges from a file"
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to read"
                },
                "start_line": {
                    "type": "integer",
                    "description": "First line number to read (1-based, inclusive)",
                    "default": 1
                },
                "end_line": {
                    "type": "integer",
                    "description": "Last line number to read (1-based, inclusive)",
                    "default": 50
                }
            },
            "required": ["path"]
        })
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

        debug!(
            path = %validated.display(),
            start_line = %start_line,
            end_line = %end_line,
            "tool: reading file lines"
        );

        let content = std::fs::read_to_string(&validated).context("failed to read file")?;

        let total_lines = content.lines().count();

        // Clamp end_line to the total number of lines
        let actual_end = end_line.min(total_lines);
        let actual_start = start_line.min(total_lines.max(1));

        let lines: Vec<&str> = content
            .lines()
            .skip(actual_start - 1)
            .take(actual_end - actual_start + 1)
            .collect();

        let truncated = actual_end < end_line || actual_end < total_lines && actual_end == end_line;

        debug!(
            total_lines = %total_lines,
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
                "total_lines": total_lines,
                "returned_count": lines.len(),
                "truncated": truncated,
                "path": validated.to_string_lossy(),
            })),
            error: None,
            verification: Some("file_lines_read".to_string()),
            audit_log: Some(format!(
                "Read lines {}-{} from {} (total: {})",
                actual_start,
                actual_end,
                validated.display(),
                total_lines,
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
}
