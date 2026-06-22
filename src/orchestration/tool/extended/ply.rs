//! PLY (Stanford Triangle Format) 3D mesh reading tools
//!
//! Provides `PlyReadTool` for reading PLY files and extracting vertex count,
//! face count, element types, and bounding box. PLY has an ASCII header with
//! element/property definitions followed by data. Parsing is done natively
//! without external dependencies.
//! Only compiled when `feature = "cad-ply"` is enabled.

#[cfg(feature = "cad-ply")]
use crate::governance::pua::tool_execution_report;
#[cfg(feature = "cad-ply")]
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
#[cfg(feature = "cad-ply")]
use anyhow::{Context, Result};
#[cfg(feature = "cad-ply")]
use std::collections::BTreeMap;
#[cfg(feature = "cad-ply")]
use std::fs;
#[cfg(feature = "cad-ply")]
use tracing::info;

/// Parsed PLY summary.
#[cfg(feature = "cad-ply")]
struct PlySummary {
    vertex_count: usize,
    face_count: usize,
    element_types: BTreeMap<String, usize>,
    bounding_box: Option<((f64, f64, f64), (f64, f64, f64))>,
    format: String,
}

/// Parse a PLY file from its text content and return a summary.
/// Supports both ASCII and binary (little-endian and big-endian) PLY files
/// by reading the header and extracting element/property definitions.
#[cfg(feature = "cad-ply")]
fn parse_ply(content: &str) -> Result<PlySummary> {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return Err(anyhow::anyhow!("empty PLY file"));
    }

    // First line must be "ply"
    let first = lines[0].trim();
    if first != "ply" {
        return Err(anyhow::anyhow!(
            "not a PLY file: expected 'ply', got {first:?}"
        ));
    }

    // Find format line (should be line 2)
    let format = if lines.len() > 1 {
        let fmt_line = lines[1].trim();
        if fmt_line.starts_with("format ") {
            fmt_line[7..].to_string()
        } else {
            return Err(anyhow::anyhow!(
                "expected 'format' on line 2, got {fmt_line:?}"
            ));
        }
    } else {
        return Err(anyhow::anyhow!("unexpected EOF after 'ply'"));
    };

    // Track elements and their properties
    let mut elements: BTreeMap<String, usize> = BTreeMap::new();
    let mut vertex_count = 0usize;
    let mut face_count = 0usize;
    let mut header_end = 0usize;

    for (i, line) in lines.iter().enumerate().skip(2) {
        let trimmed = line.trim();
        if trimmed.starts_with("comment") {
            continue;
        }
        if trimmed == "end_header" {
            header_end = i + 1;
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("element ") {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.len() >= 2 {
                let elem_name = parts[0].to_string();
                let count = parts[1].parse::<usize>().unwrap_or(0);
                elements.insert(elem_name.clone(), count);
                if elem_name == "vertex" {
                    vertex_count = count;
                } else if elem_name == "face" {
                    face_count = count;
                }
            }
        }
        // property lines are informational for parsing but we skip their details
    }

    // If ASCII format, parse vertex positions for bounding box
    // Skip past header, parse element data
    let is_ascii = format.contains("ascii");
    let mut bounding_box: Option<((f64, f64, f64), (f64, f64, f64))> = None;

    if is_ascii && header_end > 0 {
        let data_lines: Vec<&str> = lines[header_end..].to_vec();
        bounding_box = parse_ascii_ply_data(&data_lines, &elements, vertex_count);
    }

    Ok(PlySummary {
        vertex_count,
        face_count,
        element_types: elements,
        bounding_box,
        format,
    })
}

/// Parse ASCII PLY data lines to compute bounding box.
/// We read vertex_count number of lines starting from the data section.
/// Each vertex line has at least x y z as the first three values.
#[cfg(feature = "cad-ply")]
fn parse_ascii_ply_data(
    data_lines: &[&str],
    _elements: &BTreeMap<String, usize>,
    vertex_count: usize,
) -> Option<((f64, f64, f64), (f64, f64, f64))> {
    if vertex_count == 0 || data_lines.is_empty() {
        return None;
    }

    let mut min_x = f64::MAX;
    let mut min_y = f64::MAX;
    let mut min_z = f64::MAX;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    let mut max_z = f64::NEG_INFINITY;
    let mut found_any = false;

    for line in data_lines.iter().take(vertex_count) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() >= 3 {
            if let (Ok(x), Ok(y), Ok(z)) = (
                parts[0].parse::<f64>(),
                parts[1].parse::<f64>(),
                parts[2].parse::<f64>(),
            ) {
                found_any = true;
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
    }

    if found_any {
        Some(((min_x, min_y, min_z), (max_x, max_y, max_z)))
    } else {
        None
    }
}

#[cfg(feature = "cad-ply")]
pub struct PlyReadTool;

#[cfg(feature = "cad-ply")]
impl Tool for PlyReadTool {
    fn name(&self) -> &'static str {
        "ply_read"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path'"))?;
        let validated = sanitize_path(input, path)?;

        let content = fs::read_to_string(&validated)
            .with_context(|| format!("failed to read PLY: {}", validated.display()))?;

        let summary = parse_ply(&content)?;
        let byte_size = content.len();

        info!(
            path = %validated.display(),
            vertices = summary.vertex_count,
            faces = summary.face_count,
            format = %summary.format,
            "PLY mesh read"
        );

        let report = tool_execution_report("ply_read", Some("cad_read"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "vertex_count": summary.vertex_count,
                "face_count": summary.face_count,
                "element_types": summary.element_types,
                "bounding_box": summary.bounding_box.map(|(min, max)| {
                    serde_json::json!({
                        "min": { "x": min.0, "y": min.1, "z": min.2 },
                        "max": { "x": max.0, "y": max.1, "z": max.2 },
                    })
                }),
                "format": summary.format,
                "byte_size": byte_size,
                "path": validated.to_string_lossy(),
            })),
            error: None,
            verification: None,
            audit_log: Some(format!(
                "ply_read: {} vertices, {} faces, format={} from {}",
                summary.vertex_count,
                summary.face_count,
                summary.format,
                validated.display()
            )),
            pua_report: Some(report),
        })
    }
}

#[cfg(test)]
#[cfg(feature = "cad-ply")]
mod tests {
    use super::*;

    fn test_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "ply-test".to_string(),
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
    fn parse_ascii_ply_cube() {
        let ply = r#"ply
format ascii 1.0
element vertex 8
property float x
property float y
property float z
element face 6
property list uchar int vertex_indices
end_header
0 0 0
1 0 0
1 1 0
0 1 0
0 0 1
1 0 1
1 1 1
0 1 1
3 0 1 2
3 2 3 0
3 4 5 6
3 6 7 4
3 0 4 5
3 5 1 0
"#;
        let summary = parse_ply(ply).expect("valid PLY");
        assert_eq!(summary.vertex_count, 8);
        assert_eq!(summary.face_count, 6);
        assert_eq!(*summary.element_types.get("vertex").unwrap(), 8);
        assert_eq!(*summary.element_types.get("face").unwrap(), 6);
        assert!(summary.bounding_box.is_some());
        let (min, max) = summary.bounding_box.unwrap();
        assert!((min.0 - 0.0).abs() < 1e-10);
        assert!((max.0 - 1.0).abs() < 1e-10);
        assert!((min.1 - 0.0).abs() < 1e-10);
        assert!((max.1 - 1.0).abs() < 1e-10);
        assert!((min.2 - 0.0).abs() < 1e-10);
        assert!((max.2 - 1.0).abs() < 1e-10);
    }

    #[test]
    fn parse_ply_with_comments() {
        let ply = r#"ply
format ascii 1.0
comment Generated by test
element vertex 3
property float x
property float y
property float z
element face 1
property list uchar int vertex_indices
end_header
0.0 0.0 0.0
1.0 0.0 0.0
0.0 1.0 0.0
3 0 1 2
"#;
        let summary = parse_ply(ply).expect("valid PLY");
        assert_eq!(summary.vertex_count, 3);
        assert_eq!(summary.face_count, 1);
        assert_eq!(summary.element_types.len(), 2);
    }

    #[test]
    fn parse_ply_no_vertices() {
        let ply = r#"ply
format ascii 1.0
element vertex 0
property float x
property float y
property float z
end_header
"#;
        let summary = parse_ply(ply).expect("valid PLY");
        assert_eq!(summary.vertex_count, 0);
        assert_eq!(summary.face_count, 0);
        assert!(summary.bounding_box.is_none());
    }
}
