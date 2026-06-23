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
        let max_lines = input.payload["max_lines"].as_u64().unwrap_or(1000) as usize;

        let validated = sanitize_path(input, path)?;
        let content = fs::read_to_string(&validated)
            .with_context(|| format!("failed to read JSONL: {}", validated.display()))?;

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
        let records = input.payload["records"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("missing 'records' array"))?;
        let validated = sanitize_path(input, path)?;

        let mut output = String::new();
        for record in records {
            let line = serde_json::to_string(record)
                .with_context(|| format!("failed to serialize record: {record}"))?;
            output.push_str(&line);
            output.push('\n');
        }

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
