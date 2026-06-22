//! CAD/DXF drawing tools
//!
//! Provides `DxfReadTool` for reading DXF (AutoCAD) file metadata and structure.
//! Only compiled when `feature = "cad-dxf"` is enabled.

#[cfg(feature = "cad-dxf")]
use crate::governance::pua::tool_execution_report;
#[cfg(feature = "cad-dxf")]
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
#[cfg(feature = "cad-dxf")]
use anyhow::{Context, Result};
#[cfg(feature = "cad-dxf")]
use tracing::info;

#[cfg(feature = "cad-dxf")]
pub struct DxfReadTool;

#[cfg(feature = "cad-dxf")]
impl Tool for DxfReadTool {
    fn name(&self) -> &'static str {
        "dxf_read"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path'"))?;
        let validated = sanitize_path(input, path)?;

        let drawing = dxf::Drawing::load_file(&validated)
            .with_context(|| format!("failed to read DXF: {}", validated.display()))?;

        let entities: Vec<&dxf::entities::Entity> = drawing.entities().collect();
        let layers: Vec<&dxf::tables::Layer> = drawing.layers().collect();

        let entity_types: Vec<String> = {
            let mut seen = std::collections::BTreeSet::new();
            for e in &entities {
                let name = format!("{:?}", e.specific);
                seen.insert(name);
            }
            seen.into_iter().collect()
        };

        let layer_names: Vec<String> = layers.iter().map(|l| l.name.clone()).collect();
        let entity_count = entities.len();
        let byte_size = std::fs::metadata(&validated)
            .map(|m| m.len() as usize)
            .unwrap_or(0);

        info!(path = %validated.display(), entities = entity_count, layers = layer_names.len(), "DXF metadata read");

        let report = tool_execution_report("dxf_read", Some("read"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "entity_types": entity_types,
                "entity_count": entity_count,
                "layers": layer_names,
                "layer_count": layer_names.len(),
                "byte_size": byte_size,
                "path": validated.to_string_lossy(),
            })),
            error: None,
            verification: None,
            audit_log: Some(format!(
                "dxf_read: {} entities, {} layers from {}",
                entity_count,
                layer_names.len(),
                validated.display()
            )),
            pua_report: Some(report),
        })
    }
}
