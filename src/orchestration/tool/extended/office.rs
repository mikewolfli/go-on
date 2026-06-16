//! Office document tools (Excel and PowerPoint)
//!
//! Provides `ReadExcelTool` for reading `.xlsx` files, `WriteExcelTool` for
//! writing `.xlsx` files, and `ReadPptTool` for reading `.pptx` files. All
//! delegate to the multimodal infrastructure.
//!
//! - `ReadExcelTool` is only compiled when `feature = "document-excel"` is enabled.
//! - `WriteExcelTool` is only compiled when `feature = "document-excel-write"` is enabled.
//! - `ReadPptTool` is only compiled when `feature = "document-ppt"` is enabled.

#[cfg(any(
    feature = "document-excel",
    feature = "document-ppt",
    feature = "document-excel-write"
))]
use crate::governance::pua::tool_execution_report;
#[cfg(any(
    feature = "document-excel",
    feature = "document-ppt",
    feature = "document-excel-write"
))]
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
#[cfg(any(
    feature = "document-excel",
    feature = "document-ppt",
    feature = "document-excel-write"
))]
use anyhow::{Context, Result};
#[cfg(any(
    feature = "document-excel",
    feature = "document-ppt",
    feature = "document-excel-write"
))]
use std::fs;
#[cfg(any(
    feature = "document-excel",
    feature = "document-ppt",
    feature = "document-excel-write"
))]
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

// ── WriteExcelTool ─────────────────────────────────────────────────────────

#[cfg(feature = "document-excel-write")]
pub struct WriteExcelTool;

#[cfg(feature = "document-excel-write")]
impl Tool for WriteExcelTool {
    fn name(&self) -> &'static str {
        "write_excel"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path' in payload"))?;

        let validated = sanitize_path(input, path)?;

        // Parse the sheet configuration from the payload
        let config: crate::multimodal::excel_writer::WriteExcelConfig =
            serde_json::from_value(input.payload["config"].clone())
                .map_err(|e| anyhow::anyhow!("invalid 'config' in payload: {e}"))?;

        let bytes = crate::multimodal::excel_writer::write_excel_bytes(&config)
            .map_err(|e| anyhow::anyhow!("Excel write error: {e}"))?;

        fs::write(&validated, &bytes).context("failed to write Excel file")?;

        let sheet_count = config.sheets.len();
        let row_count: usize = config.sheets.iter().map(|s| s.rows.len()).sum();

        info!(
            path = %validated.display(),
            sheets = sheet_count,
            rows = row_count,
            "tool: Excel file written successfully"
        );

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "path": validated.to_string_lossy(),
                "sheets": sheet_count,
                "rows": row_count,
                "size_bytes": bytes.len(),
            })),
            error: None,
            verification: Some("excel_write".to_string()),
            audit_log: Some(format!(
                "Wrote Excel file: {} ({} sheets, {} rows, {} bytes)",
                validated.display(),
                sheet_count,
                row_count,
                bytes.len(),
            )),
            pua_report: Some(tool_execution_report("write_excel", Some("excel_write"))),
        })
    }
}
