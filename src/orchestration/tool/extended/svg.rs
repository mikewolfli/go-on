//! SVG drawing tools
//!
//! Provides `SvgReadTool` for reading SVG file metadata and structure.
//! Only compiled when `feature = "drawing-svg"` is enabled.

#[cfg(feature = "drawing-svg")]
use crate::governance::pua::tool_execution_report;
#[cfg(feature = "drawing-svg")]
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
#[cfg(feature = "drawing-svg")]
use anyhow::{Context, Result};
#[cfg(feature = "drawing-svg")]
use std::collections::BTreeSet;
#[cfg(feature = "drawing-svg")]
use std::fs;
#[cfg(feature = "drawing-svg")]
use svg::parser::Event;
#[cfg(feature = "drawing-svg")]
use tracing::info;

#[cfg(feature = "drawing-svg")]
pub struct SvgReadTool;

#[cfg(feature = "drawing-svg")]
impl Tool for SvgReadTool {
    fn name(&self) -> &'static str {
        "svg_read"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path'"))?;
        let validated = sanitize_path(input, path)?;
        let content = fs::read_to_string(&validated)
            .with_context(|| format!("failed to read SVG: {}", validated.display()))?;

        let mut parser =
            svg::read(&content).map_err(|e| anyhow::anyhow!("failed to parse SVG: {e}"))?;

        let mut width: Option<String> = None;
        let mut height: Option<String> = None;
        let mut view_box: Option<String> = None;
        let mut element_types: BTreeSet<String> = BTreeSet::new();
        let mut node_count: usize = 0;

        for event in &mut parser {
            match event {
                Event::Tag(name, _, attributes) => {
                    element_types.insert(name.to_string());
                    node_count += 1;

                    // Capture SVG root element attributes
                    if name == "svg" {
                        if let Some(v) = attributes.get("width") {
                            width = Some(v.to_string());
                        }
                        if let Some(v) = attributes.get("height") {
                            height = Some(v.to_string());
                        }
                        if let Some(v) = attributes.get("viewBox") {
                            view_box = Some(v.to_string());
                        }
                    }
                }
                _ => {}
            }
        }

        let byte_size = content.len();
        let element_types: Vec<String> = element_types.into_iter().collect();

        info!(path = %validated.display(), nodes = node_count, "SVG metadata read");

        let report = tool_execution_report("svg_read", Some("read"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "width": width,
                "height": height,
                "view_box": view_box,
                "element_types": element_types,
                "node_count": node_count,
                "byte_size": byte_size,
                "path": validated.to_string_lossy(),
            })),
            error: None,
            verification: None,
            audit_log: Some(format!(
                "svg_read: {} nodes, {} element types from {}",
                node_count,
                element_types.len(),
                validated.display()
            )),
            pua_report: Some(report),
        })
    }
}
