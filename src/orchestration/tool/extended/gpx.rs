//! GIS/GPX GPS data reading tools
//!
//! Provides `GpxReadTool` for reading GPX (GPS Exchange Format) files.
//! Only compiled when `feature = "gis-gpx"` is enabled.

#[cfg(feature = "gis-gpx")]
use crate::governance::pua::tool_execution_report;
#[cfg(feature = "gis-gpx")]
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
#[cfg(feature = "gis-gpx")]
use anyhow::{Context, Result};
#[cfg(feature = "gis-gpx")]
use std::fs;
#[cfg(feature = "gis-gpx")]
use tracing::info;

#[cfg(feature = "gis-gpx")]
pub struct GpxReadTool;

#[cfg(feature = "gis-gpx")]
impl Tool for GpxReadTool {
    fn name(&self) -> &'static str {
        "gpx_read"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path'"))?;
        let validated = sanitize_path(input, path)?;
        let content = fs::read_to_string(&validated)
            .with_context(|| format!("failed to read GPX: {}", validated.display()))?;

        let byte_size = content.len();
        let lower = content.to_lowercase();

        // Count waypoints, tracks, routes using tag counting
        let waypoint_count =
            (lower.matches("<wpt ").count() + lower.matches("<wpt>").count()) as u64;
        let track_count = lower.matches("<trk>").count() as u64;
        let route_count = lower.matches("<rte>").count() as u64;
        let trackpoint_count =
            (lower.matches("<trkpt ").count() + lower.matches("<trkpt>").count()) as u64;

        // Extract metadata name
        let name = extract_xml_text(&content, "name");

        // Extract elevations
        let mut search_pos = 0usize;
        let mut elevations: Vec<f64> = Vec::new();
        while let Some(ele_start) = lower[search_pos..].find("<ele>") {
            let abs_start = search_pos + ele_start + 5;
            if let Some(ele_end) = lower[abs_start..].find("</ele>") {
                let num_str = content[abs_start..abs_start + ele_end].trim();
                if let Ok(val) = num_str.parse::<f64>() {
                    elevations.push(val);
                }
                search_pos = abs_start + ele_end + 6;
            } else {
                break;
            }
        }

        let min_ele = elevations.iter().cloned().fold(f64::INFINITY, f64::min);
        let max_ele = elevations.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let total_ascent: f64 = elevations.windows(2).map(|w| (w[1] - w[0]).max(0.0)).sum();

        info!(path = ?validated, waypoints = waypoint_count, tracks = track_count, "GPX read");

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "name": name,
                "waypoint_count": waypoint_count,
                "track_count": track_count,
                "route_count": route_count,
                "trackpoint_count": trackpoint_count,
                "min_elevation": if elevations.is_empty() { serde_json::Value::Null } else { serde_json::json!(min_ele) },
                "max_elevation": if elevations.is_empty() { serde_json::Value::Null } else { serde_json::json!(max_ele) },
                "total_ascent_m": total_ascent as u64,
                "byte_size": byte_size,
                "path": validated.to_string_lossy(),
            })),
            error: None,
            verification: None,
            audit_log: Some(format!(
                "gpx_read: {} waypoints, {} tracks from {}",
                waypoint_count,
                track_count,
                validated.display()
            )),
            pua_report: Some(tool_execution_report("gpx_read", Some("read"))),
        })
    }
}

#[cfg(feature = "gis-gpx")]
fn extract_xml_text(content: &str, tag: &str) -> Option<String> {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let lower = content.to_lowercase();
    let open_lower = open.to_lowercase();
    let close_lower = close.to_lowercase();

    if let Some(start) = lower.find(&open_lower) {
        let content_start = start + open.len();
        if let Some(end) = lower[content_start..].find(&close_lower) {
            return Some(content[content_start..content_start + end].to_string());
        }
    }
    None
}
