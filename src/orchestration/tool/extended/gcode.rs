//! CAM/CNC G-code reading tools
//!
//! Provides `GcodeReadTool` for reading G-code (RS-274) files used in
//! CNC machining and 3D printing. Only compiled when `feature = "cam-gcode"` is enabled.

#[cfg(feature = "cam-gcode")]
use crate::governance::pua::tool_execution_report;
#[cfg(feature = "cam-gcode")]
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
#[cfg(feature = "cam-gcode")]
use anyhow::{Context, Result};
#[cfg(feature = "cam-gcode")]
use std::collections::BTreeSet;
#[cfg(feature = "cam-gcode")]
use tracing::info;

#[cfg(feature = "cam-gcode")]
pub struct GcodeReadTool;

#[cfg(feature = "cam-gcode")]
impl Tool for GcodeReadTool {
    fn name(&self) -> &'static str {
        "gcode_read"
    }

    fn exposure(&self) -> crate::orchestration::tool::ToolExposure {
        crate::orchestration::tool::ToolExposure::Deferred
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path'"))?;
        let validated = sanitize_path(input, path)?;
        let content = crate::orchestration::tool::exec_common::read_text_capped(
            &validated,
            crate::orchestration::tool::exec_common::MAX_TOOL_FILE_READ_BYTES,
        )
        .with_context(|| format!("failed to read G-code: {}", validated.display()))?;

        let byte_size = content.len();
        let mut line_count = 0u64;
        let mut g_codes = BTreeSet::new();
        let mut m_codes = BTreeSet::new();
        let mut has_tool_change = false;
        let mut has_spindle = false;
        let mut has_coolant = false;
        let mut max_feed_rate = 0.0f64;
        let mut max_spindle_speed = 0.0f64;
        let mut comment_count = 0u64;
        let mut total_x: f64 = 0.0;
        let mut total_y: f64 = 0.0;
        let mut total_z: f64 = 0.0;

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            line_count += 1;

            // Strip comments (; or ( ... ))
            let code_part = if let Some(idx) = trimmed.find(';') {
                comment_count += 1;
                &trimmed[..idx]
            } else if let Some(idx) = trimmed.find('(') {
                comment_count += 1;
                &trimmed[..idx]
            } else {
                trimmed
            };

            let upper = code_part.to_uppercase();
            for word in upper.split_whitespace() {
                if word.starts_with('G') && word.len() > 1 {
                    if let Ok(num) = word[1..].parse::<u32>() {
                        g_codes.insert(num);
                    }
                } else if word.starts_with('M') && word.len() > 1 {
                    if let Ok(num) = word[1..].parse::<u32>() {
                        m_codes.insert(num);
                    }
                } else if word.starts_with('T') && word.len() > 1 {
                    has_tool_change = true;
                } else if word.starts_with('S') && word.len() > 1 {
                    if let Ok(val) = word[1..].parse::<f64>() {
                        max_spindle_speed = max_spindle_speed.max(val);
                        has_spindle = true;
                    }
                } else if word.starts_with('F') && word.len() > 1 {
                    if let Ok(val) = word[1..].parse::<f64>() {
                        max_feed_rate = max_feed_rate.max(val);
                    }
                } else if word.starts_with('X') && word.len() > 1 {
                    if let Ok(val) = word[1..].parse::<f64>() {
                        total_x += val.abs();
                    }
                } else if word.starts_with('Y') && word.len() > 1 {
                    if let Ok(val) = word[1..].parse::<f64>() {
                        total_y += val.abs();
                    }
                } else if word.starts_with('Z') && word.len() > 1 {
                    if let Ok(val) = word[1..].parse::<f64>() {
                        total_z += val.abs();
                    }
                }
            }

            if upper.contains("M08") || upper.contains("M09") {
                has_coolant = true;
            }
        }

        info!(path = ?validated, lines = line_count, g_codes = g_codes.len(), "G-code read");

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "line_count": line_count,
                "g_codes": g_codes.iter().map(|g| format!("G{g}")).collect::<Vec<_>>(),
                "m_codes": m_codes.iter().map(|m| format!("M{m}")).collect::<Vec<_>>(),
                "has_tool_change": has_tool_change,
                "has_spindle": has_spindle,
                "has_coolant": has_coolant,
                "max_feed_rate": max_feed_rate,
                "max_spindle_speed": max_spindle_speed,
                "total_travel_mm": (total_x + total_y + total_z) as u64,
                "comment_count": comment_count,
                "byte_size": byte_size,
                "path": validated.to_string_lossy(),
            })),
            error: None,
            verification: None,
            audit_log: Some(format!(
                "gcode_read: {} lines, {} G-codes from {}",
                line_count,
                g_codes.len(),
                validated.display()
            )),
            pua_report: Some(tool_execution_report("gcode_read", Some("read"))),
        })
    }
}
