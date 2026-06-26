//! Geometry utility tools
//!
//! Provides `GeoUtilTool` for geometry utility operations including
//! 3D point distance calculation, centroid computation, and bounding
//! box calculation. All math is done natively without external dependencies.
//! Only compiled when `feature = "cad-geo"` is enabled.

#[cfg(feature = "cad-geo")]
use crate::governance::pua::tool_execution_report;
#[cfg(feature = "cad-geo")]
use crate::orchestration::tool::{Tool, ToolInput, ToolOutput};
#[cfg(feature = "cad-geo")]
use anyhow::{Context, Result};
#[cfg(feature = "cad-geo")]
use tracing::info;

/// A 3D point with f64 coordinates.
#[cfg(feature = "cad-geo")]
#[derive(Debug, Clone, Copy)]
struct Point3 {
    x: f64,
    y: f64,
    z: f64,
}

/// Parse a JSON value into a Point3, expecting `{"x": ..., "y": ..., "z": ...}`.
#[cfg(feature = "cad-geo")]
fn parse_point(value: &serde_json::Value, index: usize) -> Result<Point3> {
    let x = value["x"]
        .as_f64()
        .with_context(|| format!("point[{index}]: missing or non-numeric 'x'"))?;
    let y = value["y"]
        .as_f64()
        .with_context(|| format!("point[{index}]: missing or non-numeric 'y'"))?;
    let z = value["z"]
        .as_f64()
        .with_context(|| format!("point[{index}]: missing or non-numeric 'z'"))?;
    Ok(Point3 { x, y, z })
}

/// Compute Euclidean distance between two 3D points.
#[cfg(feature = "cad-geo")]
fn distance_3d(a: &Point3, b: &Point3) -> f64 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    let dz = a.z - b.z;
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// Compute the centroid (average) of multiple 3D points.
#[cfg(feature = "cad-geo")]
fn centroid(points: &[Point3]) -> Option<Point3> {
    if points.is_empty() {
        return None;
    }
    let n = points.len() as f64;
    let sum_x: f64 = points.iter().map(|p| p.x).sum();
    let sum_y: f64 = points.iter().map(|p| p.y).sum();
    let sum_z: f64 = points.iter().map(|p| p.z).sum();
    Some(Point3 {
        x: sum_x / n,
        y: sum_y / n,
        z: sum_z / n,
    })
}

/// Compute the bounding box of multiple 3D points.
/// Returns `(min, max)` if there is at least one point.
#[cfg(feature = "cad-geo")]
fn bounding_box(points: &[Point3]) -> Option<(Point3, Point3)> {
    if points.is_empty() {
        return None;
    }
    let mut min_x = f64::MAX;
    let mut min_y = f64::MAX;
    let mut min_z = f64::MAX;
    let mut max_x = f64::MIN;
    let mut max_y = f64::MIN;
    let mut max_z = f64::MIN;

    for p in points {
        if p.x < min_x {
            min_x = p.x;
        }
        if p.y < min_y {
            min_y = p.y;
        }
        if p.z < min_z {
            min_z = p.z;
        }
        if p.x > max_x {
            max_x = p.x;
        }
        if p.y > max_y {
            max_y = p.y;
        }
        if p.z > max_z {
            max_z = p.z;
        }
    }

    Some((
        Point3 {
            x: min_x,
            y: min_y,
            z: min_z,
        },
        Point3 {
            x: max_x,
            y: max_y,
            z: max_z,
        },
    ))
}

#[cfg(feature = "cad-geo")]
pub struct GeoUtilTool;

#[cfg(feature = "cad-geo")]
impl Tool for GeoUtilTool {
    fn name(&self) -> &'static str {
        "geo_util"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let operation = input.payload["operation"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'operation' field"))?;

        match operation {
            "distance" => {
                let p1 = parse_point(&input.payload["point1"], 0)?;
                let p2 = parse_point(&input.payload["point2"], 1)?;
                let dist = distance_3d(&p1, &p2);

                info!(operation = "distance", distance = dist, "GeoUtilTool");

                let report = tool_execution_report("geo_util", Some("distance"));

                Ok(ToolOutput {
                    success: true,
                    result: Some(serde_json::json!({
                        "operation": "distance",
                        "distance": dist,
                        "point1": { "x": p1.x, "y": p1.y, "z": p1.z },
                        "point2": { "x": p2.x, "y": p2.y, "z": p2.z },
                    })),
                    error: None,
                    verification: None,
                    audit_log: Some(format!(
                        "geo_util: distance = {} between ({}, {}, {}) and ({}, {}, {})",
                        dist, p1.x, p1.y, p1.z, p2.x, p2.y, p2.z
                    )),
                    pua_report: Some(report),
                })
            }
            "centroid" => {
                let points_arr = input.payload["points"]
                    .as_array()
                    .ok_or_else(|| anyhow::anyhow!("missing 'points' array"))?;
                let points: Vec<Point3> = points_arr
                    .iter()
                    .enumerate()
                    .map(|(i, v)| parse_point(v, i))
                    .collect::<Result<Vec<_>>>()?;

                if points.is_empty() {
                    anyhow::bail!("'points' array must not be empty for centroid operation");
                }

                let c = centroid(&points).unwrap();
                let point_count = points.len();

                info!(
                    operation = "centroid",
                    point_count = point_count,
                    "GeoUtilTool"
                );

                let report = tool_execution_report("geo_util", Some("centroid"));

                Ok(ToolOutput {
                    success: true,
                    result: Some(serde_json::json!({
                        "operation": "centroid",
                        "centroid": { "x": c.x, "y": c.y, "z": c.z },
                        "point_count": point_count,
                    })),
                    error: None,
                    verification: None,
                    audit_log: Some(format!(
                        "geo_util: centroid = ({}, {}, {}) from {} points",
                        c.x, c.y, c.z, point_count
                    )),
                    pua_report: Some(report),
                })
            }
            "bounding_box" => {
                let points_arr = input.payload["points"]
                    .as_array()
                    .ok_or_else(|| anyhow::anyhow!("missing 'points' array"))?;
                let points: Vec<Point3> = points_arr
                    .iter()
                    .enumerate()
                    .map(|(i, v)| parse_point(v, i))
                    .collect::<Result<Vec<_>>>()?;

                if points.is_empty() {
                    anyhow::bail!("'points' array must not be empty for bounding_box operation");
                }

                let (bb_min, bb_max) = bounding_box(&points).unwrap();
                let point_count = points.len();

                info!(
                    operation = "bounding_box",
                    point_count = point_count,
                    "GeoUtilTool"
                );

                let report = tool_execution_report("geo_util", Some("bounding_box"));

                Ok(ToolOutput {
                    success: true,
                    result: Some(serde_json::json!({
                        "operation": "bounding_box",
                        "bounding_box": {
                            "min": { "x": bb_min.x, "y": bb_min.y, "z": bb_min.z },
                            "max": { "x": bb_max.x, "y": bb_max.y, "z": bb_max.z },
                        },
                        "point_count": point_count,
                    })),
                    error: None,
                    verification: None,
                    audit_log: Some(format!(
                        "geo_util: bounding_box min=({}, {}, {}) max=({}, {}, {}) from {} points",
                        bb_min.x, bb_min.y, bb_min.z, bb_max.x, bb_max.y, bb_max.z, point_count
                    )),
                    pua_report: Some(report),
                })
            }
            _ => {
                anyhow::bail!(
                    "unknown operation '{}'; expected 'distance', 'centroid', or 'bounding_box'",
                    operation
                );
            }
        }
    }
}

#[cfg(test)]
#[cfg(feature = "cad-geo")]
mod tests {
    use super::*;

    fn test_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "geo-test".to_string(),
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
    fn distance_calculation() {
        let tool = GeoUtilTool;
        let input = test_input(serde_json::json!({
            "operation": "distance",
            "point1": { "x": 0.0, "y": 0.0, "z": 0.0 },
            "point2": { "x": 3.0, "y": 4.0, "z": 0.0 },
        }));
        let output = tool.run(&input).expect("distance should succeed");
        assert!(output.success);
        let dist = output.result.unwrap()["distance"].as_f64().unwrap();
        assert!((dist - 5.0).abs() < 1e-10);
    }

    #[test]
    fn centroid_calculation() {
        let tool = GeoUtilTool;
        let input = test_input(serde_json::json!({
            "operation": "centroid",
            "points": [
                { "x": 0.0, "y": 0.0, "z": 0.0 },
                { "x": 2.0, "y": 4.0, "z": 6.0 },
            ],
        }));
        let output = tool.run(&input).expect("centroid should succeed");
        assert!(output.success);
        let c = output.result.unwrap()["centroid"].clone();
        assert!((c["x"].as_f64().unwrap() - 1.0).abs() < 1e-10);
        assert!((c["y"].as_f64().unwrap() - 2.0).abs() < 1e-10);
        assert!((c["z"].as_f64().unwrap() - 3.0).abs() < 1e-10);
    }

    #[test]
    fn bounding_box_calculation() {
        let tool = GeoUtilTool;
        let input = test_input(serde_json::json!({
            "operation": "bounding_box",
            "points": [
                { "x": -1.0, "y": -2.0, "z": -3.0 },
                { "x": 4.0, "y": 5.0, "z": 6.0 },
                { "x": 0.0, "y": 0.0, "z": 0.0 },
            ],
        }));
        let output = tool.run(&input).expect("bounding_box should succeed");
        assert!(output.success);
        let bb = output.result.unwrap()["bounding_box"].clone();
        assert!((bb["min"]["x"].as_f64().unwrap() - (-1.0)).abs() < 1e-10);
        assert!((bb["min"]["y"].as_f64().unwrap() - (-2.0)).abs() < 1e-10);
        assert!((bb["min"]["z"].as_f64().unwrap() - (-3.0)).abs() < 1e-10);
        assert!((bb["max"]["x"].as_f64().unwrap() - 4.0).abs() < 1e-10);
        assert!((bb["max"]["y"].as_f64().unwrap() - 5.0).abs() < 1e-10);
        assert!((bb["max"]["z"].as_f64().unwrap() - 6.0).abs() < 1e-10);
    }

    #[test]
    fn distance_3d_function() {
        let a = Point3 {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        };
        let b = Point3 {
            x: 4.0,
            y: 6.0,
            z: 3.0,
        };
        let d = distance_3d(&a, &b);
        assert!((d - 5.0).abs() < 1e-10);
    }

    #[test]
    fn centroid_empty_returns_none() {
        let result = centroid(&[]);
        assert!(result.is_none());
    }

    #[test]
    fn bounding_box_empty_returns_none() {
        let result = bounding_box(&[]);
        assert!(result.is_none());
    }
}
