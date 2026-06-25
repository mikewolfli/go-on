//! Wavefront OBJ 3D mesh reading tools
//!
//! Provides `ObjReadTool` for reading Wavefront OBJ files and extracting
//! vertex, texture coordinate, normal, and face data, plus bounding box
//! and material reference information. Parsing is done natively without
//! external dependencies.
//! Only compiled when `feature = "cad-obj"` is enabled.

#[cfg(feature = "cad-obj")]
use crate::governance::pua::tool_execution_report;
#[cfg(feature = "cad-obj")]
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
#[cfg(feature = "cad-obj")]
use anyhow::{Context, Result};
#[cfg(feature = "cad-obj")]
use std::collections::BTreeSet;
#[cfg(feature = "cad-obj")]
use std::fs;
#[cfg(feature = "cad-obj")]
use tracing::info;

/// Parse a single OBJ vertex/line from a whitespace-split token list.
/// Returns the parsed (x, y, z) if available.
#[cfg(feature = "cad-obj")]
fn parse_triple(tokens: &[&str], start: usize) -> Option<(f64, f64, f64)> {
    let x = tokens.get(start)?.parse::<f64>().ok()?;
    let y = tokens.get(start + 1)?.parse::<f64>().ok()?;
    let z = tokens.get(start + 2)?.parse::<f64>().ok()?;
    Some((x, y, z))
}

/// Parse a single OBJ texture coordinate pair from tokens.
#[cfg(feature = "cad-obj")]
#[allow(dead_code, reason = "F-GAP reserved: OBJ uv parsing")]
fn parse_pair(tokens: &[&str], start: usize) -> Option<(f64, f64)> {
    let u = tokens.get(start)?.parse::<f64>().ok()?;
    let v = tokens.get(start + 1)?.parse::<f64>().ok()?;
    Some((u, v))
}

/// Parse a face element string like "1", "1/2", "1/2/3", or "1//3"
/// into vertex/texture/normal indices (1-based). Returns `None` on parse failure.
#[cfg(feature = "cad-obj")]
#[allow(dead_code, reason = "F-GAP reserved: OBJ face parsing")]
fn parse_face_element(s: &str) -> Option<(i64, Option<i64>, Option<i64>)> {
    let parts: Vec<&str> = s.split('/').collect();
    let v = parts[0].parse::<i64>().ok()?;
    let vt = if parts.len() > 1 && !parts[1].is_empty() {
        Some(parts[1].parse::<i64>().ok()?)
    } else {
        None
    };
    let vn = if parts.len() > 2 && !parts[2].is_empty() {
        Some(parts[2].parse::<i64>().ok()?)
    } else {
        None
    };
    Some((v, vt, vn))
}

/// Axis-aligned bounding box: (min, max) as ((x, y, z), (x, y, z)).
#[cfg(feature = "cad-obj")]
type BoundingBox = ((f64, f64, f64), (f64, f64, f64));

/// Parsed OBJ data summary.
#[cfg(feature = "cad-obj")]
struct ObjSummary {
    vertex_count: usize,
    texcoord_count: usize,
    normal_count: usize,
    face_count: usize,
    objects: Vec<String>,
    materials: Vec<String>,
    material_libraries: Vec<String>,
    bounding_box: Option<BoundingBox>,
}

/// Parse an OBJ file from its text content and return a summary.
#[cfg(feature = "cad-obj")]
fn parse_obj(content: &str) -> ObjSummary {
    let mut vertex_count = 0usize;
    let mut texcoord_count = 0usize;
    let mut normal_count = 0usize;
    let mut face_count = 0usize;
    let mut objects: Vec<String> = Vec::new();
    let mut materials: BTreeSet<String> = BTreeSet::new();
    let mut material_libraries: BTreeSet<String> = BTreeSet::new();

    let mut has_vertices = false;
    let mut min_x = f64::MAX;
    let mut min_y = f64::MAX;
    let mut min_z = f64::MAX;
    let mut max_x = f64::MIN;
    let mut max_y = f64::MIN;
    let mut max_z = f64::MIN;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        let tokens: Vec<&str> = trimmed.split_whitespace().collect();
        if tokens.is_empty() {
            continue;
        }

        match tokens[0] {
            "v" => {
                vertex_count += 1;
                if let Some((x, y, z)) = parse_triple(&tokens, 1) {
                    has_vertices = true;
                    if x < min_x {
                        min_x = x;
                    }
                    if y < min_y {
                        min_y = y;
                    }
                    if z < min_z {
                        min_z = z;
                    }
                    if x > max_x {
                        max_x = x;
                    }
                    if y > max_y {
                        max_y = y;
                    }
                    if z > max_z {
                        max_z = z;
                    }
                }
            }
            "vt" => {
                texcoord_count += 1;
            }
            "vn" => {
                normal_count += 1;
            }
            "f" => {
                face_count += 1;
            }
            "o" | "g" => {
                if let Some(name) = tokens.get(1) {
                    objects.push(name.to_string());
                }
            }
            "usemtl" => {
                if let Some(name) = tokens.get(1) {
                    materials.insert(name.to_string());
                }
            }
            "mtllib" => {
                if let Some(name) = tokens.get(1) {
                    material_libraries.insert(name.to_string());
                }
            }
            _ => {}
        }
    }

    let bounding_box = if has_vertices {
        Some(((min_x, min_y, min_z), (max_x, max_y, max_z)))
    } else {
        None
    };

    ObjSummary {
        vertex_count,
        texcoord_count,
        normal_count,
        face_count,
        objects,
        materials: materials.into_iter().collect(),
        material_libraries: material_libraries.into_iter().collect(),
        bounding_box,
    }
}

#[cfg(feature = "cad-obj")]
pub struct ObjReadTool;

#[cfg(feature = "cad-obj")]
impl Tool for ObjReadTool {
    fn name(&self) -> &'static str {
        "obj_read"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path'"))?;
        let validated = sanitize_path(input, path)?;

        let content = fs::read_to_string(&validated)
            .with_context(|| format!("failed to read OBJ: {}", validated.display()))?;

        let summary = parse_obj(&content);
        let byte_size = content.len();

        info!(
            path = %validated.display(),
            vertices = summary.vertex_count,
            faces = summary.face_count,
            "OBJ mesh read"
        );

        let report = tool_execution_report("obj_read", Some("cad_read"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "vertex_count": summary.vertex_count,
                "texcoord_count": summary.texcoord_count,
                "normal_count": summary.normal_count,
                "face_count": summary.face_count,
                "object_count": summary.objects.len(),
                "objects": summary.objects,
                "materials": summary.materials,
                "material_libraries": summary.material_libraries,
                "bounding_box": summary.bounding_box.map(|(min, max)| {
                    serde_json::json!({
                        "min": { "x": min.0, "y": min.1, "z": min.2 },
                        "max": { "x": max.0, "y": max.1, "z": max.2 },
                    })
                }),
                "byte_size": byte_size,
                "path": validated.to_string_lossy(),
            })),
            error: None,
            verification: None,
            audit_log: Some(format!(
                "obj_read: {} vertices, {} faces from {}",
                summary.vertex_count,
                summary.face_count,
                validated.display()
            )),
            pua_report: Some(report),
        })
    }
}

#[cfg(test)]
#[cfg(feature = "cad-obj")]
mod tests {
    use super::*;

    #[allow(dead_code)]
    fn test_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "obj-test".to_string(),
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
    fn parse_simple_cube_obj() {
        let obj = r#"# Simple cube
o Cube
v 0.0 0.0 0.0
v 1.0 0.0 0.0
v 1.0 1.0 0.0
v 0.0 1.0 0.0
v 0.0 0.0 1.0
v 1.0 0.0 1.0
v 1.0 1.0 1.0
v 0.0 1.0 1.0
f 1 2 3 4
f 5 6 7 8
"#;
        let summary = parse_obj(obj);
        assert_eq!(summary.vertex_count, 8);
        assert_eq!(summary.face_count, 2);
        assert_eq!(summary.objects, vec!["Cube"]);
        assert!(summary.bounding_box.is_some());
        let (min, max) = summary.bounding_box.unwrap();
        assert!((min.0 - 0.0).abs() < 1e-10);
        assert!((max.0 - 1.0).abs() < 1e-10);
    }

    #[test]
    fn parse_obj_with_texture_and_normals() {
        let obj = r#"v 0 0 0
v 1 0 0
v 0 1 0
vt 0.0 0.0
vt 1.0 0.0
vt 0.0 1.0
vn 0 0 1
vn 0 0 -1
usemtl Material1
f 1/1/1 2/2/1 3/3/1
"#;
        let summary = parse_obj(obj);
        assert_eq!(summary.vertex_count, 3);
        assert_eq!(summary.texcoord_count, 3);
        assert_eq!(summary.normal_count, 2);
        assert_eq!(summary.face_count, 1);
        assert_eq!(summary.materials, vec!["Material1"]);
    }

    #[test]
    fn parse_obj_with_material_library() {
        let obj = "mtllib materials.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
        let summary = parse_obj(obj);
        assert_eq!(summary.material_libraries, vec!["materials.mtl"]);
    }
}
