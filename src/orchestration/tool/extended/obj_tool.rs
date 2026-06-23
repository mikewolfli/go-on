//! 3D model tools (OBJ format)
//!
//! Provides `ObjModelReadTool` for reading Wavefront OBJ 3D model files
//! and extracting metadata using the `tobj` crate.
//! Only compiled when `feature = "model-3d-extra"` is enabled.

#[cfg(feature = "model-3d-extra")]
use crate::governance::pua::tool_execution_report;
#[cfg(feature = "model-3d-extra")]
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
#[cfg(feature = "model-3d-extra")]
use anyhow::{Context, Result};
#[cfg(feature = "model-3d-extra")]
use std::fs;
#[cfg(feature = "model-3d-extra")]
use std::io::BufReader;
#[cfg(feature = "model-3d-extra")]
use tracing::info;

#[cfg(feature = "model-3d-extra")]
pub struct ObjModelReadTool;

#[cfg(feature = "model-3d-extra")]
impl Tool for ObjModelReadTool {
    fn name(&self) -> &'static str {
        "obj_model_read"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path'"))?;
        let validated = sanitize_path(input, path)?;
        let content = fs::read(&validated)
            .with_context(|| format!("failed to read OBJ: {}", validated.display()))?;

        let byte_size = content.len();
        let (models, materials_result) = tobj::load_obj_buf(
            &mut BufReader::new(&content[..]),
            &tobj::LoadOptions {
                single_index: false,
                triangulate: true,
                ignore_points: true,
                ignore_lines: true,
            },
            |_| Ok((Default::default(), Default::default())),
        )
        .map_err(|e| anyhow::anyhow!("failed to parse OBJ: {e}"))?;

        let model_count = models.len();
        let material_count = match &materials_result {
            Ok(mats) => mats.len(),
            Err(_) => 0,
        };
        let mut total_vertices = 0u64;
        let mut total_indices = 0u64;
        let mut model_names = Vec::new();

        for model in &models {
            total_vertices += model.mesh.positions.len() as u64 / 3;
            total_indices += model.mesh.indices.len() as u64;
            model_names.push(model.name.clone());
        }

        info!(path = %validated.display(), models = model_count, vertices = total_vertices, "OBJ model read");

        let report = tool_execution_report("obj_model_read", Some("read"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "model_count": model_count,
                "material_count": material_count,
                "total_vertices": total_vertices,
                "total_indices": total_indices,
                "model_names": model_names,
                "byte_size": byte_size,
                "path": validated.to_string_lossy(),
            })),
            error: None,
            verification: None,
            audit_log: Some(format!(
                "obj_model_read: {} models, {} vertices from {}",
                model_count,
                total_vertices,
                validated.display()
            )),
            pua_report: Some(report),
        })
    }
}
