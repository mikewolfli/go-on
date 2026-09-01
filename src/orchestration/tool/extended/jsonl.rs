//! JSON Lines (jsonl) data streaming tools
//!
//! Provides `JsonlReadTool` and `JsonlWriteTool` for streaming JSON Lines
//! data format. Uses existing `serde_json` — no new dependencies.

use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
use anyhow::{Context, Result};
use serde_json::Value;
use std::fs;
use tracing::info;

pub struct JsonlReadTool;

impl Tool for JsonlReadTool {
    fn name(&self) -> &'static str {
        "jsonl_read"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path'"))?;
        // Param name follows the descriptor table (`limit`, not `max_lines`).
        let max_lines = input.payload["limit"].as_u64().unwrap_or(1000) as usize;

        let validated = sanitize_path(input, path)?;
        // Byte cap (input-side OOM guard, same limit as read_file): a
        // model-picked 10GB JSONL must not be fully buffered.
        let content =
            String::from_utf8_lossy(&crate::orchestration::tool::exec_common::read_file_capped(
                &validated,
                crate::orchestration::tool::exec_common::MAX_TOOL_FILE_READ_BYTES,
            )?)
            .into_owned();

        let mut records = Vec::new();
        let mut parse_errors = 0u64;

        for line in content.lines() {
            if records.len() >= max_lines {
                break;
            }
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            match serde_json::from_str::<Value>(trimmed) {
                Ok(val) => records.push(val),
                Err(_) => parse_errors += 1,
            }
        }

        let total_lines = content.lines().count();
        let byte_size = content.len();

        info!(path = %validated.display(), records = records.len(), total_lines, "JSONL data read");

        let report = tool_execution_report("jsonl_read", Some("read"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "records": records,
                "record_count": records.len(),
                "total_lines": total_lines,
                "parse_errors": parse_errors,
                "byte_size": byte_size,
                "path": validated.to_string_lossy(),
            })),
            error: None,
            verification: None,
            audit_log: Some(format!(
                "jsonl_read: {} records from {}",
                records.len(),
                validated.display()
            )),
            pua_report: Some(report),
        })
    }
}

pub struct JsonlWriteTool;

impl Tool for JsonlWriteTool {
    fn name(&self) -> &'static str {
        "jsonl_write"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path'"))?;
        // Param name follows the descriptor table (`data`, not `records`).
        let records = input.payload["data"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("missing 'data' array"))?;
        // Write path uses the shared write-sandbox pattern (containment +
        // disk-exhaustion cap + system-path blocklist), matching write_file /
        // edit_file — previously only read-path containment was enforced.
        let validated = crate::orchestration::tool::sanitize_path_for_write(input, path)?;

        let mut output = String::new();
        for record in records {
            let line = serde_json::to_string(record)
                .with_context(|| format!("failed to serialize record: {record}"))?;
            output.push_str(&line);
            output.push('\n');
        }

        crate::orchestration::tool::enforce_write_sandbox(&validated, &output)?;
        let _lock = crate::orchestration::tool::acquire_tool_write_lock(&validated)?;

        fs::write(&validated, &output)
            .with_context(|| format!("failed to write JSONL: {}", validated.display()))?;

        let byte_size = output.len();

        info!(path = %validated.display(), records = records.len(), "JSONL data written");

        let report = tool_execution_report("jsonl_write", Some("write"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "record_count": records.len(),
                "byte_size": byte_size,
                "path": validated.to_string_lossy(),
            })),
            error: None,
            verification: None,
            audit_log: Some(format!(
                "jsonl_write: {} records to {}",
                records.len(),
                validated.display()
            )),
            pua_report: Some(report),
        })
    }
}
