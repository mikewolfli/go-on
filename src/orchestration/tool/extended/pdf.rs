//! PDF document tools
//!
//! Provides `ReadPdfTool` for extracting text from PDF files using lopdf.
//! Only compiled when `feature = "document-pdf"` is enabled.

#[cfg(feature = "document-pdf")]
use crate::governance::pua::tool_execution_report;
#[cfg(feature = "document-pdf")]
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
#[cfg(feature = "document-pdf")]
use anyhow::{Context, Result};
#[cfg(feature = "document-pdf")]
use std::fs;
#[cfg(feature = "document-pdf")]
use tracing::info;

#[cfg(feature = "document-pdf")]
pub struct ReadPdfTool;

#[cfg(feature = "document-pdf")]
impl Tool for ReadPdfTool {
    fn name(&self) -> &'static str {
        "read_pdf"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path'"))?;
        let validated = sanitize_path(input, path)?;
        let content =
            fs::read(&validated).with_context(|| format!("failed to read PDF: {validated}"))?;

        let doc = lopdf::Document::load_mem(&content)
            .with_context(|| format!("failed to parse PDF: {validated}"))?;

        let mut text = String::new();
        let pages = doc.get_pages();
        let mut page_num = 0u32;
        for (_, page_id) in pages.iter() {
            page_num += 1;
            if let Ok(page_text) = doc.extract_text(page_id) {
                text.push_str(&format!("--- Page {page_num} ---\n"));
                text.push_str(&page_text);
                text.push('\n');
            }
        }

        let page_count = pages.len();
        let byte_size = content.len();

        info!(path = %validated, pages = page_count, "PDF text extracted");

        let report = tool_execution_report("read_pdf", "read", &validated.to_string_lossy(), true);

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "text": text,
                "page_count": page_count,
                "byte_size": byte_size,
                "path": validated.to_string_lossy(),
            })),
            error: None,
            verification: None,
            audit_log: Some(format!(
                "read_pdf: {} pages from {}",
                page_count,
                validated.display()
            )),
            pua_report: Some(report),
        })
    }
}
