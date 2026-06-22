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

        let docx = docx_rs::read_docx(&content)
            .map_err(|e| anyhow::anyhow!("failed to parse DOCX: {e}"))?;

        let mut text = String::new();
        let mut paragraph_count = 0u32;

        // docx-rs 0.4 API: read document structure
        for child in &docx.document.children {
            if let docx_rs::DocumentChild::Paragraph(p) = child {
                paragraph_count += 1;
                for run_child in &p.children {
                    if let docx_rs::ParagraphChild::Run(r) = run_child {
                        for rc in &r.children {
                            if let docx_rs::RunChild::Text(t) = rc {
                                text.push_str(&t.text);
                                text.push(' ');
                            }
                        }
                    }
                }
                text.push('\n');
            }
        }

        let byte_size = content.len();

        info!(path = %validated.display(), paragraphs = paragraph_count, "DOCX text extracted");

        let report = tool_execution_report("read_docx", Some("read"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "text": text,
                "paragraph_count": paragraph_count,
                "byte_size": byte_size,
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
