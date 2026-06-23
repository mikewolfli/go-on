//! 3D model tools (STL format)
//!
//! Provides `StlReadTool` for reading STL (stereolithography) 3D model files
//! and extracting metadata. Only compiled when `feature = "model-3d"` is enabled.

#[cfg(feature = "model-3d")]
use crate::governance::pua::tool_execution_report;
#[cfg(feature = "model-3d")]
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
#[cfg(feature = "model-3d")]
use anyhow::{Context, Result};
#[cfg(feature = "model-3d")]
use std::fs;
#[cfg(feature = "model-3d")]
use std::io::Cursor;
#[cfg(feature = "model-3d")]
use tracing::info;

#[cfg(feature = "model-3d")]
pub struct StlReadTool;

#[cfg(feature = "model-3d")]
impl Tool for StlReadTool {
    fn name(&self) -> &'static str {
        "stl_read"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path'"))?;
        let validated = sanitize_path(input, path)?;
        let content = fs::read(&validated)
            .with_context(|| format!("failed to read STL: {}", validated.display()))?;

        let byte_size = content.len();
        let is_ascii = byte_size >= 5 && &content[0..5] == b"solid";

        if is_ascii {
            anyhow::bail!(
                "ASCII STL is not supported by the stl crate; only binary STL files are supported"
            );
        }

        let mut cursor = Cursor::new(&content);
        let stl_file = stl::read_stl(&mut cursor)
            .map_err(|e| anyhow::anyhow!("failed to parse binary STL: {e}"))?;

        let triangle_count = stl_file.triangles.len();

        // Compute bounding box stats from all triangles
        let mut min_x = f32::MAX;
        let mut max_x = f32::MIN;
        let mut min_y = f32::MAX;
        let mut max_y = f32::MIN;
        let mut min_z = f32::MAX;
        let mut max_z = f32::MIN;

        for tri in &stl_file.triangles {
            for v in [&tri.v1, &tri.v2, &tri.v3] {
                if v[0] < min_x {
                    min_x = v[0];
                }
                if v[0] > max_x {
                    max_x = v[0];
                }
                if v[1] < min_y {
                    min_y = v[1];
                }
                if v[1] > max_y {
                    max_y = v[1];
                }
                if v[2] < min_z {
                    min_z = v[2];
                }
                if v[2] > max_z {
                    max_z = v[2];
                }
            }
        }

        info!(path = %validated.display(), triangles = triangle_count, is_binary = true, "STL model read");

        let report = tool_execution_report("stl_read", Some("read"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "triangle_count": triangle_count,
                "is_binary": true,
                "byte_size": byte_size,
                "path": validated.to_string_lossy(),
                "stats": {
                    "min_x": min_x,
                    "max_x": max_x,
                    "min_y": min_y,
                    "max_y": max_y,
                    "min_z": min_z,
                    "max_z": max_z,
                },
            })),
            error: None,
            verification: None,
            audit_log: Some(format!(
                "stl_read: {} triangles, {} bytes from {}",
                triangle_count,
                byte_size,
                validated.display()
            )),
            pua_report: Some(report),
        })
    }
}
