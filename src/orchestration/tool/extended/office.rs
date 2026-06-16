//! Office document tools (Excel and PowerPoint)
//!
//! Provides `ReadExcelTool` for reading `.xlsx` files and `ReadPptTool` for
//! reading `.pptx` files. Both delegate to the multimodal parser infrastructure.
//!
//! - `ReadExcelTool` is only compiled when `feature = "document-excel"` is enabled.
//! - `ReadPptTool` is only compiled when `feature = "document-ppt"` is enabled.

#[cfg(any(feature = "document-excel", feature = "document-ppt"))]
use crate::governance::pua::tool_execution_report;
#[cfg(any(feature = "document-excel", feature = "document-ppt"))]
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
#[cfg(any(feature = "document-excel", feature = "document-ppt"))]
use anyhow::{Context, Result};
#[cfg(any(feature = "document-excel", feature = "document-ppt"))]
use std::fs;
#[cfg(any(feature = "document-excel", feature = "document-ppt"))]
use tracing::info;

// ── ReadExcelTool ──────────────────────────────────────────────────────────

#[cfg(feature = "document-excel")]
pub struct ReadExcelTool;

#[cfg(feature = "document-excel")]
impl Tool for ReadExcelTool {
    fn name(&self) -> &'static str {
        "read_excel"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path' in payload"))?;

        let validated = sanitize_path(input, path)?;

        let bytes = fs::read(&validated).context("failed to read Excel file")?;

        let parsed = crate::multimodal::excel_processor::parse_excel_bytes(&bytes)
            .map_err(|e| anyhow::anyhow!("Excel parse error: {e}"))?;

        info!(
            path = %validated.display(),
            char_count = parsed.char_count(),
            "tool: Excel file read successfully"
        );

        Ok(ToolOutput {
            success: !parsed.has_error(),
            result: Some(serde_json::json!({
                "text": parsed.text_content,
                "metadata": parsed.metadata,
                "char_count": parsed.char_count(),
            })),
            error: parsed.error_message().map(|s| s.to_string()),
            verification: Some("excel_read".to_string()),
            audit_log: Some(format!(
                "Read Excel file: {} ({} chars, {} sheets)",
                validated.display(),
                parsed.char_count(),
                parsed.metadata.get("sheet_count").unwrap_or(&String::new()),
            )),
            pua_report: Some(tool_execution_report("read_excel", Some("excel_read"))),
        })
    }
}

// ── ReadPptTool ────────────────────────────────────────────────────────────

#[cfg(feature = "document-ppt")]
pub struct ReadPptTool;

#[cfg(feature = "document-ppt")]
impl Tool for ReadPptTool {
    fn name(&self) -> &'static str {
        "read_ppt"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path' in payload"))?;

        let validated = sanitize_path(input, path)?;

        let bytes = fs::read(&validated).context("failed to read PowerPoint file")?;

        let parsed = crate::multimodal::ppt_processor::parse_pptx_bytes(&bytes)
            .map_err(|e| anyhow::anyhow!("PPT parse error: {e}"))?;

        info!(
            path = %validated.display(),
            char_count = parsed.char_count(),
            "tool: PowerPoint file read successfully"
        );

        Ok(ToolOutput {
            success: !parsed.has_error(),
            result: Some(serde_json::json!({
                "text": parsed.text_content,
                "metadata": parsed.metadata,
                "char_count": parsed.char_count(),
            })),
            error: parsed.error_message().map(|s| s.to_string()),
            verification: Some("ppt_read".to_string()),
            audit_log: Some(format!(
                "Read PowerPoint file: {} ({} chars, {} slides)",
                validated.display(),
                parsed.char_count(),
                parsed.metadata.get("slide_count").unwrap_or(&String::new()),
            )),
            pua_report: Some(tool_execution_report("read_ppt", Some("ppt_read"))),
        })
    }
}
