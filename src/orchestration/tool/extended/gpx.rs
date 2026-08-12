//! GIS/GPX GPS data reading tools
//!
//! Provides `GpxReadTool` for reading GPX (GPS Exchange Format) files.
//! Only compiled when `feature = "gis-gpx"` is enabled.

#[cfg(feature = "gis-gpx")]
use crate::governance::pua::tool_execution_report;
#[cfg(feature = "gis-gpx")]
use crate::orchestration::tool::extended::utils::extract_xml_tag;
#[cfg(feature = "gis-gpx")]
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
#[cfg(feature = "gis-gpx")]
use crate::shared::text::find_ascii_case_insensitive;
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

    fn exposure(&self) -> crate::orchestration::tool::ToolExposure {
        crate::orchestration::tool::ToolExposure::Deferred
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path'"))?;
        let validated = sanitize_path(input, path)?;
        let content = fs::read_to_string(&validated)
            .with_context(|| format!("failed to read GPX: {}", validated.display()))?;

        let byte_size = content.len();

        // Count waypoints, tracks, routes using tag counting (byte-wise
        // ASCII case-insensitive on the original — no full lowercased copy,
        // which would double memory for large track files).
        let waypoint_count = (count_ascii_case_insensitive(&content, "<wpt ")
            + count_ascii_case_insensitive(&content, "<wpt>")) as u64;
        let track_count = count_ascii_case_insensitive(&content, "<trk>") as u64;
        let route_count = count_ascii_case_insensitive(&content, "<rte>") as u64;
        let trackpoint_count = (count_ascii_case_insensitive(&content, "<trkpt ")
            + count_ascii_case_insensitive(&content, "<trkpt>"))
            as u64;

        // Extract metadata name
        let name = extract_xml_tag(&content, "name");

        // Extract elevations. Tag offsets come from byte-wise matching on the
        // original (tags start with `<`, an ASCII byte, so every match and
        // every `+tag.len()` resume point is a char boundary) — the numeric
        // value between them parses identically to a lowercased copy.
        let mut elevations: Vec<f64> = Vec::new();
        let mut rest = content.as_str();
        while let Some(ele_start) = find_ascii_case_insensitive(rest, "<ele>") {
            let after_open = &rest[ele_start + 5..];
            if let Some(ele_end) = find_ascii_case_insensitive(after_open, "</ele>") {
                let num_str = after_open[..ele_end].trim();
                if let Ok(val) = num_str.parse::<f64>() {
                    elevations.push(val);
                }
                rest = &after_open[ele_end + 6..];
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

/// Count non-overlapping ASCII case-insensitive occurrences of `needle` in
/// `text` (byte-wise, no allocation). Only used here, under the `gis-gpx`
/// feature, so it lives in this feature-gated module.
fn count_ascii_case_insensitive(text: &str, needle: &str) -> usize {
    let hay = text.as_bytes();
    let needle_b = needle.as_bytes();
    if needle_b.is_empty() || needle_b.len() > hay.len() {
        return 0;
    }
    let mut count = 0;
    let mut i = 0;
    while i <= hay.len() - needle_b.len() {
        if hay[i..i + needle_b.len()].eq_ignore_ascii_case(needle_b) {
            count += 1;
            i += needle_b.len();
        } else {
            i += 1;
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shared::text::find_ascii_case_insensitive;

    #[test]
    fn extract_xml_tag_preserves_case_and_multibyte_content() {
        // Byte-wise ASCII matching must preserve original case in the content
        // and never panic on CJK between the tags.
        let gpx = "<gpx><name>My Track</name><wpt lat=\"1\" lon=\"2\"><ele>123.5</ele></wpt></gpx>";
        assert_eq!(extract_xml_tag(gpx, "name").as_deref(), Some("My Track"));
        let cjk = "<name>轨迹K测试</name>";
        assert_eq!(extract_xml_tag(cjk, "name").as_deref(), Some("轨迹K测试"));
    }

    #[test]
    fn elevation_scan_uses_case_insensitive_tags() {
        // The run loop scans `<ele>`/`</ele>` byte-wise; a close tag ending
        // exactly at the end of the content (inclusive-range regression) must
        // still be found.
        let content = "<trkpt><ele>10</ele></trkpt>";
        let mut rest = content;
        let mut elevations: Vec<f64> = Vec::new();
        while let Some(ele_start) = find_ascii_case_insensitive(rest, "<ele>") {
            let after_open = &rest[ele_start + 5..];
            if let Some(ele_end) = find_ascii_case_insensitive(after_open, "</ele>") {
                let num_str = after_open[..ele_end].trim();
                if let Ok(val) = num_str.parse::<f64>() {
                    elevations.push(val);
                }
                rest = &after_open[ele_end + 6..];
            } else {
                break;
            }
        }
        assert_eq!(elevations, vec![10.0]);
    }

    #[test]
    fn counting_is_case_insensitive() {
        assert_eq!(count_ascii_case_insensitive("<WPT><wpt><wpt>", "<wpt>"), 3);
        assert_eq!(count_ascii_case_insensitive("<trk><trkpt>", "<trkpt>"), 1);
    }
}
