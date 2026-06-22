//! STL 3D mesh reading tools
//!
//! Provides `StlReadTool` for reading ASCII and binary STL (stereolithography)
//! files and extracting vertex count, facet count, bounding box, and volume.
//! Parsing is done natively without external dependencies.
//! Only compiled when `feature = "cad-stl"` is enabled.

#[cfg(feature = "cad-stl")]
use crate::governance::pua::tool_execution_report;
#[cfg(feature = "cad-stl")]
use crate::orchestration::tool::{
    sanitize_path, sanitize_path_for_write, Tool, ToolInput, ToolOutput,
};
#[cfg(feature = "cad-stl")]
use anyhow::{Context, Result};
#[cfg(feature = "cad-stl")]
use std::fs;
#[cfg(feature = "cad-stl")]
use tracing::info;

/// A single 3D vertex.
#[cfg(feature = "cad-stl")]
#[derive(Debug, Clone, Copy, Default)]
struct Vertex {
    x: f64,
    y: f64,
    z: f64,
}

/// A single triangular facet with its normal.
#[cfg(feature = "cad-stl")]
#[derive(Debug, Clone)]
struct Facet {
    #[allow(dead_code, reason = "F-GAP reserved: face normal data")]
    normal: Vertex,
    v0: Vertex,
    v1: Vertex,
    v2: Vertex,
}

#[cfg(feature = "cad-stl")]
impl Vertex {
    fn from_f32_le(bytes: &[u8]) -> Self {
        let x = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64;
        let y = f32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as f64;
        let z = f32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as f64;
        Self { x, y, z }
    }

    fn cross(&self, other: &Vertex) -> Vertex {
        Vertex {
            x: self.y * other.z - self.z * other.y,
            y: self.z * other.x - self.x * other.z,
            z: self.x * other.y - self.y * other.x,
        }
    }

    fn dot(&self, other: &Vertex) -> f64 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }
}

#[cfg(feature = "cad-stl")]
impl std::ops::Sub for Vertex {
    type Output = Vertex;
    fn sub(self, other: Vertex) -> Vertex {
        Vertex {
            x: self.x - other.x,
            y: self.y - other.y,
            z: self.z - other.z,
        }
    }
}

/// Parse an ASCII STL file from its text content.
#[cfg(feature = "cad-stl")]
fn parse_ascii_stl(content: &str) -> Result<(Vec<Facet>, Option<String>)> {
    let mut facets = Vec::new();
    let mut solid_name: Option<String> = None;

    let trimmed = content.trim();
    let mut lines = trimmed.lines().peekable();

    // Skip leading empty lines, look for "solid" keyword at start of any non-empty line
    // Actually STL ASCII format can have comments? No, official format has no comments.
    // First non-empty line should be "solid [name]"
    while let Some(line) = lines.next() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(name) = line.strip_prefix("solid ") {
            let name = name.trim();
            if !name.is_empty() {
                solid_name = Some(name.to_string());
            }
            break;
        } else if line == "solid" {
            break;
        } else {
            return Err(anyhow::anyhow!(
                "expected 'solid' keyword at start of ASCII STL, got: {line:?}"
            ));
        }
    }

    loop {
        // Skip blank lines
        let line = loop {
            let l = lines.next();
            match l {
                None => break None,
                Some(l) if l.trim().is_empty() => continue,
                Some(l) => break Some(l),
            }
        };
        let line = match line {
            None => break,
            Some(l) => l.trim().to_string(),
        };

        if line.starts_with("endsolid") {
            break;
        }

        // Expect "facet normal nx ny nz"
        let normal = if let Some(rest) = line.strip_prefix("facet normal ") {
            let parts: Vec<f64> = rest
                .split_whitespace()
                .filter_map(|s| s.parse::<f64>().ok())
                .collect();
            if parts.len() != 3 {
                return Err(anyhow::anyhow!("invalid facet normal line: {line}"));
            }
            Vertex {
                x: parts[0],
                y: parts[1],
                z: parts[2],
            }
        } else if line.starts_with("endsolid") {
            break;
        } else {
            return Err(anyhow::anyhow!("expected 'facet normal', got: {line}"));
        };

        // Expect "outer loop"
        let outer_line = lines
            .next()
            .ok_or_else(|| anyhow::anyhow!("unexpected EOF: expected 'outer loop'"))?
            .trim()
            .to_string();
        if outer_line != "outer loop" {
            return Err(anyhow::anyhow!("expected 'outer loop', got: {outer_line}"));
        }

        // Three vertex lines
        let mut vertices = Vec::with_capacity(3);
        for _ in 0..3 {
            let v_line = lines
                .next()
                .ok_or_else(|| anyhow::anyhow!("unexpected EOF: expected vertex"))?
                .trim()
                .to_string();
            if let Some(rest) = v_line.strip_prefix("vertex ") {
                let parts: Vec<f64> = rest
                    .split_whitespace()
                    .filter_map(|s| s.parse::<f64>().ok())
                    .collect();
                if parts.len() != 3 {
                    return Err(anyhow::anyhow!("invalid vertex line: {v_line}"));
                }
                vertices.push(Vertex {
                    x: parts[0],
                    y: parts[1],
                    z: parts[2],
                });
            } else {
                return Err(anyhow::anyhow!("expected 'vertex', got: {v_line}"));
            }
        }

        // Expect "endloop"
        let endloop_line = lines
            .next()
            .ok_or_else(|| anyhow::anyhow!("unexpected EOF: expected 'endloop'"))?
            .trim()
            .to_string();
        if endloop_line != "endloop" {
            return Err(anyhow::anyhow!("expected 'endloop', got: {endloop_line}"));
        }

        // Expect "endfacet"
        let endfacet_line = lines
            .next()
            .ok_or_else(|| anyhow::anyhow!("unexpected EOF: expected 'endfacet'"))?
            .trim()
            .to_string();
        if endfacet_line != "endfacet" {
            return Err(anyhow::anyhow!("expected 'endfacet', got: {endfacet_line}"));
        }

        facets.push(Facet {
            normal,
            v0: vertices[0],
            v1: vertices[1],
            v2: vertices[2],
        });
    }

    Ok((facets, solid_name))
}

/// Parse a binary STL file from its raw bytes.
#[cfg(feature = "cad-stl")]
fn parse_binary_stl(bytes: &[u8]) -> Result<(Vec<Facet>, Option<String>)> {
    if bytes.len() < 84 {
        return Err(anyhow::anyhow!(
            "binary STL too short: {} bytes (need at least 84)",
            bytes.len()
        ));
    }

    // First 80 bytes: header (often contains name or metadata)
    let header = &bytes[0..80];
    let header_str = String::from_utf8_lossy(header);
    let solid_name = {
        let s = header_str.trim();
        if s.is_empty() {
            None
        } else {
            Some(s.to_string())
        }
    };

    // Bytes 80-83: triangle count (u32, little-endian)
    let count_bytes: [u8; 4] = [bytes[80], bytes[81], bytes[82], bytes[83]];
    let triangle_count = u32::from_le_bytes(count_bytes) as usize;

    let expected_size = 84 + triangle_count * 50;
    if bytes.len() < expected_size {
        return Err(anyhow::anyhow!(
            "binary STL truncated: expected {} bytes, got {}",
            expected_size,
            bytes.len()
        ));
    }

    let mut facets = Vec::with_capacity(triangle_count);
    let mut offset = 84;

    for _ in 0..triangle_count {
        if offset + 50 > bytes.len() {
            return Err(anyhow::anyhow!(
                "binary STL truncated at triangle {}",
                facets.len()
            ));
        }

        let normal_bytes = &bytes[offset..offset + 12];
        let v0_bytes = &bytes[offset + 12..offset + 24];
        let v1_bytes = &bytes[offset + 24..offset + 36];
        let v2_bytes = &bytes[offset + 36..offset + 48];

        let normal = Vertex::from_f32_le(normal_bytes);
        let v0 = Vertex::from_f32_le(v0_bytes);
        let v1 = Vertex::from_f32_le(v1_bytes);
        let v2 = Vertex::from_f32_le(v2_bytes);

        facets.push(Facet { normal, v0, v1, v2 });

        // Skip 2-byte attribute byte count
        offset += 50;
    }

    Ok((facets, solid_name))
}

/// Determine whether STL content is binary (starts with non-ASCII or has
/// an 80-byte header followed by a reasonable triangle count) or ASCII.
#[cfg(feature = "cad-stl")]
fn is_binary_stl(bytes: &[u8]) -> bool {
    if bytes.len() < 84 {
        // Too short to be binary; try ASCII
        return false;
    }
    // Check if it starts with "solid" — ASCII always does
    let start = &bytes[0..5];
    if start == b"solid" {
        // Could be ASCII; verify by looking for a "facet" keyword after
        let sample = String::from_utf8_lossy(&bytes[..bytes.len().min(200)]);
        return !sample.contains("facet");
    }
    // Doesn't start with "solid" => binary
    true
}

/// Compute axis-aligned bounding box for a set of facets.
#[cfg(feature = "cad-stl")]
fn bounding_box(facets: &[Facet]) -> (Vertex, Vertex) {
    let mut min = Vertex {
        x: f64::INFINITY,
        y: f64::INFINITY,
        z: f64::INFINITY,
    };
    let mut max = Vertex {
        x: f64::NEG_INFINITY,
        y: f64::NEG_INFINITY,
        z: f64::NEG_INFINITY,
    };

    for facet in facets {
        for v in [&facet.v0, &facet.v1, &facet.v2] {
            if v.x < min.x {
                min.x = v.x;
            }
            if v.y < min.y {
                min.y = v.y;
            }
            if v.z < min.z {
                min.z = v.z;
            }
            if v.x > max.x {
                max.x = v.x;
            }
            if v.y > max.y {
                max.y = v.y;
            }
            if v.z > max.z {
                max.z = v.z;
            }
        }
    }

    (min, max)
}

/// Estimate the volume of a closed STL mesh using the divergence theorem.
/// Volume = sum over triangles of (v0 · (v1 × v2)) / 6
/// Returns the absolute value (unsigned).
#[cfg(feature = "cad-stl")]
fn estimate_volume(facets: &[Facet]) -> f64 {
    let mut volume = 0.0_f64;
    for facet in facets {
        // Signed volume of tetrahedron from origin to triangle vertices
        let cross = facet.v1.cross(&facet.v2);
        volume += facet.v0.dot(&cross);
    }
    (volume / 6.0).abs()
}

/// Extract a count of unique vertices (deduplicated by floating-point position).
#[cfg(feature = "cad-stl")]
fn unique_vertex_count(facets: &[Facet]) -> usize {
    use std::collections::HashSet;
    // Use a tolerance for deduplication by rounding to 6 decimal places
    let mut seen = HashSet::new();
    for facet in facets {
        for v in [&facet.v0, &facet.v1, &facet.v2] {
            let key = (
                (v.x * 1_000_000.0).round() as i64,
                (v.y * 1_000_000.0).round() as i64,
                (v.z * 1_000_000.0).round() as i64,
            );
            seen.insert(key);
        }
    }
    seen.len()
}

#[cfg(feature = "cad-stl")]
pub struct StlReadTool;

#[cfg(feature = "cad-stl")]
impl Tool for StlReadTool {
    fn name(&self) -> &'static str {
        "stl_read"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path'"))?;
        let validated = sanitize_path(input, path)?;

        let bytes = fs::read(&validated)
            .with_context(|| format!("failed to read STL: {}", validated.display()))?;

        let is_binary = is_binary_stl(&bytes);

        let (facets, solid_name) = if is_binary {
            parse_binary_stl(&bytes)
                .with_context(|| format!("failed to parse binary STL: {}", validated.display()))?
        } else {
            let content = String::from_utf8_lossy(&bytes);
            parse_ascii_stl(&content)
                .with_context(|| format!("failed to parse ASCII STL: {}", validated.display()))?
        };

        let (bb_min, bb_max) = bounding_box(&facets);
        let volume = estimate_volume(&facets);
        let unique_verts = unique_vertex_count(&facets);
        let byte_size = bytes.len();

        info!(
            path = %validated.display(),
            facets = facets.len(),
            vertices = unique_verts,
            volume = volume,
            format = if is_binary { "binary" } else { "ascii" },
            "STL mesh read"
        );

        let report = tool_execution_report("stl_read", Some("cad_read"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "facet_count": facets.len(),
                "unique_vertex_count": unique_verts,
                "bounding_box": {
                    "min": { "x": bb_min.x, "y": bb_min.y, "z": bb_min.z },
                    "max": { "x": bb_max.x, "y": bb_max.y, "z": bb_max.z },
                },
                "volume_estimate": volume,
                "format": if is_binary { "binary" } else { "ascii" },
                "solid_name": solid_name,
                "byte_size": byte_size,
                "path": validated.to_string_lossy(),
            })),
            error: None,
            verification: None,
            audit_log: Some(format!(
                "stl_read: {} facets, {} vertices, volume={} from {}",
                facets.len(),
                unique_verts,
                volume,
                validated.display()
            )),
            pua_report: Some(report),
        })
    }
}

/// Generate an ASCII STL file from vertex and face data payload.
///
/// Accepts `"vertices"`: array of [x, y, z] and `"faces"`: array of [i, j, k]
/// (0-indexed vertex indices). Writes the ASCII STL to the specified `"path"`.
#[cfg(feature = "cad-stl")]
pub struct StlGenerateTool;

#[cfg(feature = "cad-stl")]
impl Tool for StlGenerateTool {
    fn name(&self) -> &'static str {
        "stl_generate"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path'"))?;
        let validated = sanitize_path_for_write(input, path)?;

        let vertices = input.payload["vertices"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("missing or invalid 'vertices' array"))?;
        let faces = input.payload["faces"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("missing or invalid 'faces' array"))?;

        if vertices.is_empty() {
            return Err(anyhow::anyhow!("'vertices' array is empty"));
        }
        if faces.is_empty() {
            return Err(anyhow::anyhow!("'faces' array is empty"));
        }

        // Parse vertices into Vec<Vertex>
        let parsed_verts: Vec<Vertex> = vertices
            .iter()
            .map(|v| {
                let arr = v
                    .as_array()
                    .ok_or_else(|| anyhow::anyhow!("each vertex must be an array [x, y, z]"))?;
                if arr.len() < 3 {
                    return Err(anyhow::anyhow!("each vertex must have at least 3 elements"));
                }
                let x = arr[0]
                    .as_f64()
                    .ok_or_else(|| anyhow::anyhow!("vertex x must be a number"))?;
                let y = arr[1]
                    .as_f64()
                    .ok_or_else(|| anyhow::anyhow!("vertex y must be a number"))?;
                let z = arr[2]
                    .as_f64()
                    .ok_or_else(|| anyhow::anyhow!("vertex z must be a number"))?;
                Ok(Vertex { x, y, z })
            })
            .collect::<Result<Vec<Vertex>>>()
            .context("failed to parse vertices")?;

        // Parse faces into Vec<(usize, usize, usize)>
        let parsed_faces: Vec<(usize, usize, usize)> =
            faces
                .iter()
                .map(|f| {
                    let arr = f
                        .as_array()
                        .ok_or_else(|| anyhow::anyhow!("each face must be an array [i, j, k]"))?;
                    if arr.len() < 3 {
                        return Err(anyhow::anyhow!("each face must have at least 3 elements"));
                    }
                    let i = arr[0].as_u64().ok_or_else(|| {
                        anyhow::anyhow!("face index i must be a non-negative integer")
                    })? as usize;
                    let j = arr[1].as_u64().ok_or_else(|| {
                        anyhow::anyhow!("face index j must be a non-negative integer")
                    })? as usize;
                    let k = arr[2].as_u64().ok_or_else(|| {
                        anyhow::anyhow!("face index k must be a non-negative integer")
                    })? as usize;
                    Ok((i, j, k))
                })
                .collect::<Result<Vec<(usize, usize, usize)>>>()
                .context("failed to parse faces")?;

        // Validate indices
        let vert_count = parsed_verts.len();
        for (idx, (i, j, k)) in parsed_faces.iter().enumerate() {
            if *i >= vert_count || *j >= vert_count || *k >= vert_count {
                return Err(anyhow::anyhow!(
                    "face {} contains vertex index out of bounds: ({}, {}, {}) with {} vertices available",
                    idx, i, j, k, vert_count
                ));
            }
        }

        // Build facets with computed normals
        let facets: Vec<Facet> = parsed_faces
            .iter()
            .map(|(i, j, k)| {
                let v0 = parsed_verts[*i];
                let v1 = parsed_verts[*j];
                let v2 = parsed_verts[*k];
                // Compute face normal: cross product of edges and normalize
                let edge1 = v1 - v0;
                let edge2 = v2 - v0;
                let mut normal = edge1.cross(&edge2);
                let len = (normal.x * normal.x + normal.y * normal.y + normal.z * normal.z).sqrt();
                if len > 0.0 {
                    normal.x /= len;
                    normal.y /= len;
                    normal.z /= len;
                }
                Facet { normal, v0, v1, v2 }
            })
            .collect();

        // Generate ASCII STL content
        let mut stl_content = String::new();
        stl_content.push_str("solid generated\n");
        for facet in &facets {
            stl_content.push_str(&format!(
                "  facet normal {} {} {}\n",
                facet.normal.x, facet.normal.y, facet.normal.z
            ));
            stl_content.push_str("    outer loop\n");
            stl_content.push_str(&format!(
                "      vertex {} {} {}\n",
                facet.v0.x, facet.v0.y, facet.v0.z
            ));
            stl_content.push_str(&format!(
                "      vertex {} {} {}\n",
                facet.v1.x, facet.v1.y, facet.v1.z
            ));
            stl_content.push_str(&format!(
                "      vertex {} {} {}\n",
                facet.v2.x, facet.v2.y, facet.v2.z
            ));
            stl_content.push_str("    endloop\n");
            stl_content.push_str("  endfacet\n");
        }
        stl_content.push_str("endsolid generated\n");

        // Write to file
        fs::write(&validated, &stl_content)
            .with_context(|| format!("failed to write STL: {}", validated.display()))?;

        let byte_size = stl_content.len();

        info!(
            path = %validated.display(),
            facets = facets.len(),
            "STL mesh generated"
        );

        let report = tool_execution_report("stl_generate", Some("cad_generate"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "facet_count": facets.len(),
                "vertex_count": parsed_verts.len(),
                "byte_size": byte_size,
                "path": validated.to_string_lossy(),
            })),
            error: None,
            verification: None,
            audit_log: Some(format!(
                "stl_generate: {} facets, {} vertices written to {}",
                facets.len(),
                parsed_verts.len(),
                validated.display()
            )),
            pua_report: Some(report),
        })
    }
}

#[cfg(test)]
#[cfg(feature = "cad-stl")]
mod tests {
    use super::*;
    use crate::orchestration::tool::{Tool, ToolInput};

    fn tool_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "test-stl".to_string(),
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
    fn parse_ascii_cube() {
        // A simple ASCII STL cube — 12 triangles (2 per face)
        let stl = r#"solid cube
  facet normal 0 0 -1
    outer loop
      vertex 0 0 0
      vertex 1 0 0
      vertex 0 1 0
    endloop
  endfacet
  facet normal 0 0 -1
    outer loop
      vertex 1 0 0
      vertex 1 1 0
      vertex 0 1 0
    endloop
  endfacet
  facet normal 0 0 1
    outer loop
      vertex 0 0 1
      vertex 0 1 1
      vertex 1 0 1
    endloop
  endfacet
  facet normal 0 0 1
    outer loop
      vertex 1 0 1
      vertex 0 1 1
      vertex 1 1 1
    endloop
  endfacet
  facet normal -1 0 0
    outer loop
      vertex 0 0 0
      vertex 0 1 0
      vertex 0 0 1
    endloop
  endfacet
  facet normal -1 0 0
    outer loop
      vertex 0 0 1
      vertex 0 1 0
      vertex 0 1 1
    endloop
  endfacet
  facet normal 1 0 0
    outer loop
      vertex 1 0 0
      vertex 1 0 1
      vertex 1 1 0
    endloop
  endfacet
  facet normal 1 0 0
    outer loop
      vertex 1 0 1
      vertex 1 1 1
      vertex 1 1 0
    endloop
  endfacet
  facet normal 0 -1 0
    outer loop
      vertex 0 0 0
      vertex 0 0 1
      vertex 1 0 0
    endloop
  endfacet
  facet normal 0 -1 0
    outer loop
      vertex 0 0 1
      vertex 1 0 1
      vertex 1 0 0
    endloop
  endfacet
  facet normal 0 1 0
    outer loop
      vertex 0 1 0
      vertex 1 1 0
      vertex 0 1 1
    endloop
  endfacet
  facet normal 0 1 0
    outer loop
      vertex 0 1 1
      vertex 1 1 0
      vertex 1 1 1
    endloop
  endfacet
endsolid cube
"#;

        let (facets, name) = parse_ascii_stl(stl).expect("should parse ASCII STL");
        assert_eq!(facets.len(), 12);
        assert_eq!(name.as_deref(), Some("cube"));

        let (bb_min, bb_max) = bounding_box(&facets);
        assert!((bb_min.x - 0.0).abs() < 1e-10);
        assert!((bb_max.x - 1.0).abs() < 1e-10);
        assert!((bb_min.y - 0.0).abs() < 1e-10);
        assert!((bb_max.y - 1.0).abs() < 1e-10);
        assert!((bb_min.z - 0.0).abs() < 1e-10);
        assert!((bb_max.z - 1.0).abs() < 1e-10);

        // A unit cube should have volume ≈ 1.0
        let vol = estimate_volume(&facets);
        assert!((vol - 1.0).abs() < 0.01, "volume should be ~1.0, got {vol}");
    }

    #[test]
    fn parse_binary_cube() {
        // Build a minimal binary STL: 80-byte header + 4-byte count + 12 triangles * 50 bytes
        let mut bytes = Vec::new();
        // Header
        bytes.extend_from_slice(
            b"binary_cube                                                                     ",
        );
        assert_eq!(bytes.len(), 80);
        // Triangle count (12)
        bytes.extend_from_slice(&12u32.to_le_bytes());

        // Helper: write a triangle (3 vertices + normal + 2-byte attribute)
        fn write_triangle(
            buf: &mut Vec<u8>,
            nx: f32,
            ny: f32,
            nz: f32,
            v0: (f32, f32, f32),
            v1: (f32, f32, f32),
            v2: (f32, f32, f32),
        ) {
            for v in [
                nx, ny, nz, v0.0, v0.1, v0.2, v1.0, v1.1, v1.2, v2.0, v2.1, v2.2,
            ] {
                buf.extend_from_slice(&v.to_le_bytes());
            }
            buf.extend_from_slice(&[0u8; 2]); // attribute byte count
        }

        // Unit cube triangles (same as ASCII test but with correct normals)
        // Face z=0
        write_triangle(
            &mut bytes,
            0.0,
            0.0,
            -1.0,
            (0., 0., 0.),
            (1., 0., 0.),
            (0., 1., 0.),
        );
        write_triangle(
            &mut bytes,
            0.0,
            0.0,
            -1.0,
            (1., 0., 0.),
            (1., 1., 0.),
            (0., 1., 0.),
        );
        // Face z=1
        write_triangle(
            &mut bytes,
            0.0,
            0.0,
            1.0,
            (0., 0., 1.),
            (0., 1., 1.),
            (1., 0., 1.),
        );
        write_triangle(
            &mut bytes,
            0.0,
            0.0,
            1.0,
            (1., 0., 1.),
            (0., 1., 1.),
            (1., 1., 1.),
        );
        // Face x=0
        write_triangle(
            &mut bytes,
            -1.0,
            0.0,
            0.0,
            (0., 0., 0.),
            (0., 1., 0.),
            (0., 0., 1.),
        );
        write_triangle(
            &mut bytes,
            -1.0,
            0.0,
            0.0,
            (0., 0., 1.),
            (0., 1., 0.),
            (0., 1., 1.),
        );
        // Face x=1
        write_triangle(
            &mut bytes,
            1.0,
            0.0,
            0.0,
            (1., 0., 0.),
            (1., 0., 1.),
            (1., 1., 0.),
        );
        write_triangle(
            &mut bytes,
            1.0,
            0.0,
            0.0,
            (1., 0., 1.),
            (1., 1., 1.),
            (1., 1., 0.),
        );
        // Face y=0
        write_triangle(
            &mut bytes,
            0.0,
            -1.0,
            0.0,
            (0., 0., 0.),
            (0., 0., 1.),
            (1., 0., 0.),
        );
        write_triangle(
            &mut bytes,
            0.0,
            -1.0,
            0.0,
            (0., 0., 1.),
            (1., 0., 1.),
            (1., 0., 0.),
        );
        // Face y=1
        write_triangle(
            &mut bytes,
            0.0,
            1.0,
            0.0,
            (0., 1., 0.),
            (1., 1., 0.),
            (0., 1., 1.),
        );
        write_triangle(
            &mut bytes,
            0.0,
            1.0,
            0.0,
            (0., 1., 1.),
            (1., 1., 0.),
            (1., 1., 1.),
        );

        assert_eq!(bytes.len(), 84 + 12 * 50);

        let (facets, name) = parse_binary_stl(&bytes).expect("should parse binary STL");
        assert_eq!(facets.len(), 12);
        assert_eq!(name.as_deref(), Some("binary_cube"));

        let vol = estimate_volume(&facets);
        assert!((vol - 1.0).abs() < 0.01, "volume should be ~1.0, got {vol}");
    }

    #[test]
    fn detect_binary_vs_ascii() {
        let ascii_bytes = b"solid test\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid test\n";
        assert!(!is_binary_stl(ascii_bytes), "should detect ASCII");

        let mut bin = vec![0u8; 84];
        bin[0..5].copy_from_slice(b"solid");
        // Fill with enough to make it look not-ASCII-like
        for i in 5..80 {
            bin[i] = 0xFF;
        }
        // Triangle count = 0
        bin[80..84].copy_from_slice(&0u32.to_le_bytes());
        assert!(is_binary_stl(&bin), "should detect binary");
    }

    #[test]
    fn generate_stl_from_vertices_and_faces() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let out_path = tmp.path().join("output.stl");

        let input = ToolInput {
            task_id: "stl-gen-test".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "path": out_path.to_string_lossy(),
                "vertices": [
                    [0.0, 0.0, 0.0],
                    [1.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0],
                    [0.0, 0.0, 1.0]
                ],
                "faces": [
                    [0, 1, 2],
                    [0, 2, 3],
                    [0, 3, 1],
                    [1, 3, 2]
                ]
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let tool = StlGenerateTool;
        let output = tool.run(&input).expect("stl_generate should succeed");

        assert!(output.success);
        assert!(out_path.exists());

        // Verify the generated file is valid ASCII STL
        let generated = std::fs::read_to_string(&out_path).expect("read generated STL");
        assert!(generated.starts_with("solid generated"));
        assert!(generated.contains("endsolid generated"));
        assert!(generated.contains("facet normal"));
        assert!(generated.contains("vertex"));

        // Validate it can be read back
        let (facets, name) =
            parse_ascii_stl(&generated).expect("generated STL should be valid ASCII STL");
        assert_eq!(facets.len(), 4);
        assert_eq!(name.as_deref(), Some("generated"));

        // Check result JSON
        let result = output.result.expect("should have result");
        assert_eq!(result["facet_count"].as_u64().unwrap(), 4);
        assert_eq!(result["vertex_count"].as_u64().unwrap(), 4);
    }
}
