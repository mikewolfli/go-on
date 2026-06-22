//! glTF 3D model reading tools
//!
//! Provides `GltfReadTool` for reading glTF (GL Transmission Format) JSON files
//! and extracting scene, mesh, vertex, triangle, material, texture, and animation
//! counts. glTF 2.0 is JSON-based, so parsing is done natively without external
//! dependencies.
//! Only compiled when `feature = "cad-gltf"` is enabled.

#[cfg(feature = "cad-gltf")]
use crate::governance::pua::tool_execution_report;
#[cfg(feature = "cad-gltf")]
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
#[cfg(feature = "cad-gltf")]
use anyhow::{Context, Result};
#[cfg(feature = "cad-gltf")]
use std::fs;
#[cfg(feature = "cad-gltf")]
use tracing::info;

/// Parsed glTF summary extracted from the JSON document.
#[cfg(feature = "cad-gltf")]
struct GltfSummary {
    scene_count: usize,
    mesh_count: usize,
    vertex_count: usize,
    triangle_count: usize,
    material_count: usize,
    texture_count: usize,
    animation_count: usize,
}

/// Parse a glTF file from its JSON string content and return a summary.
#[cfg(feature = "cad-gltf")]
fn parse_gltf(content: &str) -> Result<GltfSummary> {
    let doc: serde_json::Value =
        serde_json::from_str(content).with_context(|| "failed to parse glTF JSON")?;

    // Scenes: array at root "scenes"
    let scene_count = doc["scenes"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);

    // Meshes: array at root "meshes"
    let mesh_count = doc["meshes"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);

    // Accessors: each has a "count" field giving the number of vertex attributes
    let vertex_count = doc["accessors"]
        .as_array()
        .map(|accessors| {
            accessors
                .iter()
                .filter_map(|acc| acc["count"].as_u64())
                .max()
                .unwrap_or(0) as usize
        })
        .unwrap_or(0);

    // Triangle count: sum of primitives with mode=4 (triangles) or mode=5 (triangle strip)
    // For each mesh, iterate its primitives and sum up index counts / 3
    let triangle_count = doc["meshes"]
        .as_array()
        .map(|meshes| {
            meshes
                .iter()
                .filter_map(|mesh| mesh["primitives"].as_array())
                .flat_map(|prims| {
                    prims.iter().filter_map(|prim| {
                        let mode = prim["mode"].as_u64().unwrap_or(4);
                        // mode 4 = TRIANGLES, mode 5 = TRIANGLE_STRIP, mode 6 = TRIANGLE_FAN
                        // We approximate: if it has an indices accessor, count indices/3
                        if mode == 4 || mode == 5 || mode == 6 || prim.get("indices").is_some() {
                            // Try to get the actual count from the indices accessor
                            let indices_acc = prim["indices"].as_u64();
                            if let Some(idx) = indices_acc {
                                // Look up the accessor to get count
                                if let Some(acc) = doc["accessors"]
                                    .as_array()
                                    .and_then(|arr| arr.get(idx as usize))
                                {
                                    if let Some(cnt) = acc["count"].as_u64() {
                                        return Some(cnt / 3);
                                    }
                                }
                            }
                            // If primitives lack indices, count from attributes
                            if let Some(attrs) = prim["attributes"].as_object() {
                                for val in attrs.values() {
                                    if let Some(acc_idx) = val.as_u64() {
                                        if let Some(acc) = doc["accessors"]
                                            .as_array()
                                            .and_then(|arr| arr.get(acc_idx as usize))
                                        {
                                            if let Some(cnt) = acc["count"].as_u64() {
                                                return Some(cnt / 3);
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        None
                    })
                })
                .sum::<u64>() as usize
        })
        .unwrap_or(0);

    // Materials: array at root "materials"
    let material_count = doc["materials"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);

    // Textures: array at root "textures" or "images"
    let texture_count = doc["textures"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);

    // Images (may overlap with textures but provides image-level count)
    let image_count = doc["images"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);

    // Animations: array at root "animations"
    let animation_count = doc["animations"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);

    Ok(GltfSummary {
        scene_count,
        mesh_count,
        vertex_count,
        triangle_count,
        material_count,
        texture_count: texture_count.max(image_count),
        animation_count,
    })
}

#[cfg(feature = "cad-gltf")]
pub struct GltfReadTool;

#[cfg(feature = "cad-gltf")]
impl Tool for GltfReadTool {
    fn name(&self) -> &'static str {
        "gltf_read"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path'"))?;
        let validated = sanitize_path(input, path)?;

        let content = fs::read_to_string(&validated)
            .with_context(|| format!("failed to read glTF: {}", validated.display()))?;

        let summary = parse_gltf(&content)?;
        let byte_size = content.len();

        info!(
            path = %validated.display(),
            meshes = summary.mesh_count,
            vertices = summary.vertex_count,
            triangles = summary.triangle_count,
            "glTF mesh read"
        );

        let report = tool_execution_report("gltf_read", Some("cad_read"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "scene_count": summary.scene_count,
                "mesh_count": summary.mesh_count,
                "vertex_count": summary.vertex_count,
                "triangle_count": summary.triangle_count,
                "material_count": summary.material_count,
                "texture_count": summary.texture_count,
                "animation_count": summary.animation_count,
                "byte_size": byte_size,
                "path": validated.to_string_lossy(),
            })),
            error: None,
            verification: None,
            audit_log: Some(format!(
                "gltf_read: {} meshes, {} vertices, {} triangles from {}",
                summary.mesh_count,
                summary.vertex_count,
                summary.triangle_count,
                validated.display()
            )),
            pua_report: Some(report),
        })
    }
}

#[cfg(test)]
#[cfg(feature = "cad-gltf")]
mod tests {
    use super::*;

    fn test_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "gltf-test".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload,
            allowed_base_dir: None,
        }
    }

    #[test]
    fn parse_minimal_gltf() {
        let gltf = r#"{
            "asset": { "version": "2.0" },
            "scenes": [ { "nodes": [0] } ],
            "nodes": [ { "mesh": 0 } ],
            "meshes": [ {
                "primitives": [ {
                    "attributes": { "POSITION": 0 },
                    "indices": 1,
                    "mode": 4
                } ]
            } ],
            "accessors": [
                { "count": 24, "type": "VEC3", "componentType": 5126 },
                { "count": 36, "type": "SCALAR", "componentType": 5123 }
            ],
            "materials": [ { "name": "Default" } ],
            "textures": [],
            "images": [ { "uri": "tex.png" } ]
        }"#;
        let summary = parse_gltf(gltf).expect("valid glTF");
        assert_eq!(summary.scene_count, 1);
        assert_eq!(summary.mesh_count, 1);
        assert_eq!(summary.vertex_count, 24);
        assert_eq!(summary.triangle_count, 12); // 36 / 3
        assert_eq!(summary.material_count, 1);
        assert_eq!(summary.texture_count, 1);
        assert_eq!(summary.animation_count, 0);
    }

    #[test]
    fn parse_empty_gltf() {
        let gltf = r#"{"asset":{"version":"2.0"}}"#;
        let summary = parse_gltf(gltf).expect("valid glTF");
        assert_eq!(summary.scene_count, 0);
        assert_eq!(summary.mesh_count, 0);
        assert_eq!(summary.vertex_count, 0);
        assert_eq!(summary.triangle_count, 0);
        assert_eq!(summary.material_count, 0);
        assert_eq!(summary.texture_count, 0);
        assert_eq!(summary.animation_count, 0);
    }

    #[test]
    fn parse_gltf_with_animations() {
        let gltf = r#"{
            "asset": { "version": "2.0" },
            "scenes": [ { "nodes": [0] } ],
            "nodes": [ { "mesh": 0 } ],
            "meshes": [ {
                "primitives": [ {
                    "attributes": { "POSITION": 0 },
                    "mode": 4
                } ]
            } ],
            "accessors": [ { "count": 3, "type": "VEC3", "componentType": 5126 } ],
            "animations": [
                { "name": "walk" },
                { "name": "run" }
            ]
        }"#;
        let summary = parse_gltf(gltf).expect("valid glTF");
        assert_eq!(summary.animation_count, 2);
    }
}
