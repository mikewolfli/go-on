//! DOCX document tools
//!
//! Provides `ReadDocxTool` for extracting text from DOCX files using docx-rs.
//! Only compiled when `feature = "document-docx"` is enabled.

#[cfg(feature = "document-docx")]
use crate::governance::pua::tool_execution_report;
#[cfg(feature = "document-docx")]
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
#[cfg(feature = "document-docx")]
use anyhow::{Context, Result};
#[cfg(feature = "document-docx")]
use std::fs;
#[cfg(feature = "document-docx")]
use tracing::info;

#[cfg(feature = "document-docx")]
pub struct ReadDocxTool;

#[cfg(feature = "document-docx")]
impl Tool for ReadDocxTool {
    fn name(&self) -> &'static str {
        "read_docx"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path'"))?;
        let validated = sanitize_path(input, path)?;
        let content = fs::read(&validated)
            .with_context(|| format!("failed to read DOCX: {}", validated.display()))?;

        // Single DOCX extraction implementation (shared with the multimodal
        // pipeline — the previous inline docx-rs copy was removed).
        let parser = crate::multimodal::document_parser::DocumentParser::default();
        let parsed = parser
            .parse_bytes(&content, "docx")
            .map_err(|e| anyhow::anyhow!("DOCX parse error: {}", e))?;

        let paragraph_count = parsed
            .metadata
            .get("paragraph_count")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0);
        let byte_size = content.len();

        info!(
            path = %validated.display(),
            paragraphs = paragraph_count,
            tables = parsed.tables.len(),
            "DOCX text extracted"
        );

        let report = tool_execution_report("read_docx", Some("read"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "text": parsed.text_content,
                "paragraph_count": paragraph_count,
                "byte_size": byte_size,
                "tables": parsed.tables,
                "metadata": parsed.metadata,
                "path": validated.to_string_lossy(),
            })),
            error: None,
            verification: None,
            audit_log: Some(format!(
                "read_docx: {} paragraphs from {}",
                paragraph_count,
                validated.display()
            )),
            pua_report: Some(report),
        })
    }
}
