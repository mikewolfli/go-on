//! SVG drawing tools
//!
//! Provides `SvgReadTool` for reading SVG file metadata and structure,
//! and `SvgGenerateTool` for generating SVG documents from parameters.
//! Only compiled when `feature = "drawing-svg"` is enabled.

#[cfg(feature = "drawing-svg")]
use crate::governance::pua::tool_execution_report;
#[cfg(feature = "drawing-svg")]
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
#[cfg(feature = "drawing-svg")]
use anyhow::{Context, Result};
#[cfg(feature = "drawing-svg")]
use std::collections::BTreeSet;
#[cfg(feature = "drawing-svg")]
use std::fs;
#[cfg(feature = "drawing-svg")]
use svg::parser::Event;
#[cfg(feature = "drawing-svg")]
use tracing::info;

#[cfg(feature = "drawing-svg")]
pub struct SvgReadTool;

#[cfg(feature = "drawing-svg")]
impl Tool for SvgReadTool {
    fn name(&self) -> &'static str {
        "svg_read"
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
            .with_context(|| format!("failed to read SVG: {}", validated.display()))?;

        let mut parser =
            svg::read(&content).map_err(|e| anyhow::anyhow!("failed to parse SVG: {e}"))?;

        let mut width: Option<String> = None;
        let mut height: Option<String> = None;
        let mut view_box: Option<String> = None;
        let mut element_types: BTreeSet<String> = BTreeSet::new();
        let mut node_count: usize = 0;

        for event in &mut parser {
            if let Event::Tag(name, _, attributes) = event {
                element_types.insert(name.to_string());
                node_count += 1;

                // Capture SVG root element attributes
                if name == "svg" {
                    if let Some(v) = attributes.get("width") {
                        width = Some(v.to_string());
                    }
                    if let Some(v) = attributes.get("height") {
                        height = Some(v.to_string());
                    }
                    if let Some(v) = attributes.get("viewBox") {
                        view_box = Some(v.to_string());
                    }
                }
            }
        }

        let byte_size = content.len();
        let element_types: Vec<String> = element_types.into_iter().collect();

        info!(path = %validated.display(), nodes = node_count, "SVG metadata read");

        let report = tool_execution_report("svg_read", Some("read"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "width": width,
                "height": height,
                "view_box": view_box,
                "element_types": element_types,
                "node_count": node_count,
                "byte_size": byte_size,
                "path": validated.to_string_lossy(),
            })),
            error: None,
            verification: None,
            audit_log: Some(format!(
                "svg_read: {} nodes, {} element types from {}",
                node_count,
                element_types.len(),
                validated.display()
            )),
            pua_report: Some(report),
        })
    }
}

// ── SvgGenerateTool ─────────────────────────────────────────────────────────

#[cfg(feature = "drawing-svg")]
pub struct SvgGenerateTool;

#[cfg(feature = "drawing-svg")]
impl Tool for SvgGenerateTool {
    fn name(&self) -> &'static str {
        "svg_generate"
    }

    fn exposure(&self) -> crate::orchestration::tool::ToolExposure {
        crate::orchestration::tool::ToolExposure::Deferred
    }


    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let width = input.payload["width"].as_u64().unwrap_or(800) as f64;
        let height = input.payload["height"].as_u64().unwrap_or(600) as f64;
        let output_path = input.payload["path"].as_str();

        let mut document = svg::Document::new()
            .set("xmlns", "http://www.w3.org/2000/svg")
            .set("width", width)
            .set("height", height)
            .set("viewBox", (0, 0, width as i64, height as i64));

        // Parse shapes
        if let Some(shapes) = input.payload["shapes"].as_array() {
            for shape in shapes {
                let shape_type = shape["type"].as_str().unwrap_or("");
                match shape_type {
                    "rect" | "rectangle" => {
                        if let Some(rect) = build_rect(shape) {
                            document = document.add(rect);
                        }
                    }
                    "circle" => {
                        if let Some(circle) = build_circle(shape) {
                            document = document.add(circle);
                        }
                    }
                    "ellipse" => {
                        if let Some(ellipse) = build_ellipse(shape) {
                            document = document.add(ellipse);
                        }
                    }
                    "line" => {
                        if let Some(line) = build_line(shape) {
                            document = document.add(line);
                        }
                    }
                    "polyline" => {
                        if let Some(polyline) = build_polyline(shape) {
                            document = document.add(polyline);
                        }
                    }
                    "polygon" => {
                        if let Some(polygon) = build_polygon(shape) {
                            document = document.add(polygon);
                        }
                    }
                    "text" => {
                        if let Some(text) = build_text(shape) {
                            document = document.add(text);
                        }
                    }
                    "path" => {
                        if let Some(path) = build_path(shape) {
                            document = document.add(path);
                        }
                    }
                    _ => {
                        info!(type = shape_type, "unknown shape type in svg_generate");
                    }
                }
            }
        }

        let svg_string = document.to_string();

        let shape_count = input.payload["shapes"]
            .as_array()
            .map(|a| a.len())
            .unwrap_or(0);
        let byte_size = svg_string.len();

        // Optionally write to file
        let written_path = if let Some(path_str) = output_path {
            let validated = sanitize_path(input, path_str)?;
            fs::write(&validated, &svg_string)
                .with_context(|| format!("failed to write SVG: {}", validated.display()))?;
            Some(validated.to_string_lossy().to_string())
        } else {
            None
        };

        info!(shapes = shape_count, byte_size = byte_size, "SVG generated");

        let report = tool_execution_report("svg_generate", Some("generate"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "svg": svg_string,
                "shape_count": shape_count,
                "width": width,
                "height": height,
                "byte_size": byte_size,
                "path": written_path,
            })),
            error: None,
            verification: None,
            audit_log: Some(format!(
                "svg_generate: {} shapes, {} bytes {} ",
                shape_count,
                byte_size,
                written_path
                    .as_deref()
                    .map(|p| format!("written to {p}"))
                    .unwrap_or_default(),
            )),
            pua_report: Some(report),
        })
    }
}

#[cfg(feature = "drawing-svg")]
fn parse_opt_f64(val: &serde_json::Value) -> Option<f64> {
    val.as_f64()
}

#[cfg(feature = "drawing-svg")]
fn parse_opt_str(val: &serde_json::Value) -> Option<&str> {
    val.as_str()
}

/// Generic setter for common SVG attributes on any Element.
#[cfg(feature = "drawing-svg")]
trait WithCommonAttrs: Sized {
    fn with_common(self, shape: &serde_json::Value) -> Self;
}

#[cfg(feature = "drawing-svg")]
impl WithCommonAttrs for svg::node::element::Rectangle {
    fn with_common(self, shape: &serde_json::Value) -> Self {
        let mut el = self;
        if let Some(fill) = parse_opt_str(&shape["fill"]) {
            el = el.set("fill", fill);
        }
        if let Some(stroke) = parse_opt_str(&shape["stroke"]) {
            el = el.set("stroke", stroke);
        }
        if let Some(w) = parse_opt_f64(&shape["stroke_width"]) {
            el = el.set("stroke-width", w);
        }
        if let Some(op) = parse_opt_f64(&shape["opacity"]) {
            el = el.set("opacity", op);
        }
        if let Some(cls) = parse_opt_str(&shape["class"]) {
            el = el.set("class", cls);
        }
        if let Some(id) = parse_opt_str(&shape["id"]) {
            el = el.set("id", id);
        }
        if let Some(tx) = parse_opt_f64(&shape["transform_rotate"]) {
            el = el.set("transform", format!("rotate({tx})"));
        }
        el
    }
}

#[cfg(feature = "drawing-svg")]
impl WithCommonAttrs for svg::node::element::Circle {
    fn with_common(self, shape: &serde_json::Value) -> Self {
        let mut el = self;
        if let Some(fill) = parse_opt_str(&shape["fill"]) {
            el = el.set("fill", fill);
        }
        if let Some(stroke) = parse_opt_str(&shape["stroke"]) {
            el = el.set("stroke", stroke);
        }
        if let Some(w) = parse_opt_f64(&shape["stroke_width"]) {
            el = el.set("stroke-width", w);
        }
        if let Some(op) = parse_opt_f64(&shape["opacity"]) {
            el = el.set("opacity", op);
        }
        if let Some(cls) = parse_opt_str(&shape["class"]) {
            el = el.set("class", cls);
        }
        if let Some(id) = parse_opt_str(&shape["id"]) {
            el = el.set("id", id);
        }
        if let Some(tx) = parse_opt_f64(&shape["transform_rotate"]) {
            el = el.set("transform", format!("rotate({tx})"));
        }
        el
    }
}

#[cfg(feature = "drawing-svg")]
impl WithCommonAttrs for svg::node::element::Ellipse {
    fn with_common(self, shape: &serde_json::Value) -> Self {
        let mut el = self;
        if let Some(fill) = parse_opt_str(&shape["fill"]) {
            el = el.set("fill", fill);
        }
        if let Some(stroke) = parse_opt_str(&shape["stroke"]) {
            el = el.set("stroke", stroke);
        }
        if let Some(w) = parse_opt_f64(&shape["stroke_width"]) {
            el = el.set("stroke-width", w);
        }
        if let Some(op) = parse_opt_f64(&shape["opacity"]) {
            el = el.set("opacity", op);
        }
        if let Some(cls) = parse_opt_str(&shape["class"]) {
            el = el.set("class", cls);
        }
        if let Some(id) = parse_opt_str(&shape["id"]) {
            el = el.set("id", id);
        }
        if let Some(tx) = parse_opt_f64(&shape["transform_rotate"]) {
            el = el.set("transform", format!("rotate({tx})"));
        }
        el
    }
}

#[cfg(feature = "drawing-svg")]
impl WithCommonAttrs for svg::node::element::Line {
    fn with_common(self, shape: &serde_json::Value) -> Self {
        let mut el = self;
        if let Some(stroke) = parse_opt_str(&shape["stroke"]) {
            el = el.set("stroke", stroke);
        }
        if let Some(w) = parse_opt_f64(&shape["stroke_width"]) {
            el = el.set("stroke-width", w);
        }
        if let Some(op) = parse_opt_f64(&shape["opacity"]) {
            el = el.set("opacity", op);
        }
        if let Some(cls) = parse_opt_str(&shape["class"]) {
            el = el.set("class", cls);
        }
        if let Some(id) = parse_opt_str(&shape["id"]) {
            el = el.set("id", id);
        }
        el
    }
}

#[cfg(feature = "drawing-svg")]
impl WithCommonAttrs for svg::node::element::Polyline {
    fn with_common(self, shape: &serde_json::Value) -> Self {
        let mut el = self;
        if let Some(fill) = parse_opt_str(&shape["fill"]) {
            el = el.set("fill", fill);
        }
        if let Some(stroke) = parse_opt_str(&shape["stroke"]) {
            el = el.set("stroke", stroke);
        }
        if let Some(w) = parse_opt_f64(&shape["stroke_width"]) {
            el = el.set("stroke-width", w);
        }
        if let Some(op) = parse_opt_f64(&shape["opacity"]) {
            el = el.set("opacity", op);
        }
        if let Some(cls) = parse_opt_str(&shape["class"]) {
            el = el.set("class", cls);
        }
        if let Some(id) = parse_opt_str(&shape["id"]) {
            el = el.set("id", id);
        }
        el
    }
}

#[cfg(feature = "drawing-svg")]
impl WithCommonAttrs for svg::node::element::Polygon {
    fn with_common(self, shape: &serde_json::Value) -> Self {
        let mut el = self;
        if let Some(fill) = parse_opt_str(&shape["fill"]) {
            el = el.set("fill", fill);
        }
        if let Some(stroke) = parse_opt_str(&shape["stroke"]) {
            el = el.set("stroke", stroke);
        }
        if let Some(w) = parse_opt_f64(&shape["stroke_width"]) {
            el = el.set("stroke-width", w);
        }
        if let Some(op) = parse_opt_f64(&shape["opacity"]) {
            el = el.set("opacity", op);
        }
        if let Some(cls) = parse_opt_str(&shape["class"]) {
            el = el.set("class", cls);
        }
        if let Some(id) = parse_opt_str(&shape["id"]) {
            el = el.set("id", id);
        }
        el
    }
}

#[cfg(feature = "drawing-svg")]
impl WithCommonAttrs for svg::node::element::Path {
    fn with_common(self, shape: &serde_json::Value) -> Self {
        let mut el = self;
        if let Some(fill) = parse_opt_str(&shape["fill"]) {
            el = el.set("fill", fill);
        }
        if let Some(stroke) = parse_opt_str(&shape["stroke"]) {
            el = el.set("stroke", stroke);
        }
        if let Some(w) = parse_opt_f64(&shape["stroke_width"]) {
            el = el.set("stroke-width", w);
        }
        if let Some(op) = parse_opt_f64(&shape["opacity"]) {
            el = el.set("opacity", op);
        }
        if let Some(cls) = parse_opt_str(&shape["class"]) {
            el = el.set("class", cls);
        }
        if let Some(id) = parse_opt_str(&shape["id"]) {
            el = el.set("id", id);
        }
        el
    }
}

#[cfg(feature = "drawing-svg")]
impl WithCommonAttrs for svg::node::element::Text {
    fn with_common(self, shape: &serde_json::Value) -> Self {
        let mut el = self;
        if let Some(fill) = parse_opt_str(&shape["fill"]) {
            el = el.set("fill", fill);
        }
        if let Some(stroke) = parse_opt_str(&shape["stroke"]) {
            el = el.set("stroke", stroke);
        }
        if let Some(w) = parse_opt_f64(&shape["stroke_width"]) {
            el = el.set("stroke-width", w);
        }
        if let Some(op) = parse_opt_f64(&shape["opacity"]) {
            el = el.set("opacity", op);
        }
        if let Some(cls) = parse_opt_str(&shape["class"]) {
            el = el.set("class", cls);
        }
        if let Some(id) = parse_opt_str(&shape["id"]) {
            el = el.set("id", id);
        }
        el
    }
}

#[cfg(feature = "drawing-svg")]
fn build_rect(shape: &serde_json::Value) -> Option<svg::node::element::Rectangle> {
    let x = parse_opt_f64(&shape["x"])?;
    let y = parse_opt_f64(&shape["y"])?;
    let w = parse_opt_f64(&shape["width"])?;
    let h = parse_opt_f64(&shape["height"])?;
    let mut rect = svg::node::element::Rectangle::new()
        .set("x", x)
        .set("y", y)
        .set("width", w)
        .set("height", h);
    if let Some(rx) = parse_opt_f64(&shape["rx"]) {
        rect = rect.set("rx", rx);
    }
    if let Some(ry) = parse_opt_f64(&shape["ry"]) {
        rect = rect.set("ry", ry);
    }
    Some(rect.with_common(shape))
}

#[cfg(feature = "drawing-svg")]
fn build_circle(shape: &serde_json::Value) -> Option<svg::node::element::Circle> {
    let cx = parse_opt_f64(&shape["cx"])?;
    let cy = parse_opt_f64(&shape["cy"])?;
    let r = parse_opt_f64(&shape["r"])?;
    Some(
        svg::node::element::Circle::new()
            .set("cx", cx)
            .set("cy", cy)
            .set("r", r)
            .with_common(shape),
    )
}

#[cfg(feature = "drawing-svg")]
fn build_ellipse(shape: &serde_json::Value) -> Option<svg::node::element::Ellipse> {
    let cx = parse_opt_f64(&shape["cx"])?;
    let cy = parse_opt_f64(&shape["cy"])?;
    let rx = parse_opt_f64(&shape["rx"])?;
    let ry = parse_opt_f64(&shape["ry"])?;
    Some(
        svg::node::element::Ellipse::new()
            .set("cx", cx)
            .set("cy", cy)
            .set("rx", rx)
            .set("ry", ry)
            .with_common(shape),
    )
}

#[cfg(feature = "drawing-svg")]
fn build_line(shape: &serde_json::Value) -> Option<svg::node::element::Line> {
    let x1 = parse_opt_f64(&shape["x1"])?;
    let y1 = parse_opt_f64(&shape["y1"])?;
    let x2 = parse_opt_f64(&shape["x2"])?;
    let y2 = parse_opt_f64(&shape["y2"])?;
    Some(
        svg::node::element::Line::new()
            .set("x1", x1)
            .set("y1", y1)
            .set("x2", x2)
            .set("y2", y2)
            .with_common(shape),
    )
}

#[cfg(feature = "drawing-svg")]
fn build_polyline(shape: &serde_json::Value) -> Option<svg::node::element::Polyline> {
    let points = shape["points"].as_str()?;
    Some(
        svg::node::element::Polyline::new()
            .set("points", points)
            .with_common(shape),
    )
}

#[cfg(feature = "drawing-svg")]
fn build_polygon(shape: &serde_json::Value) -> Option<svg::node::element::Polygon> {
    let points = shape["points"].as_str()?;
    Some(
        svg::node::element::Polygon::new()
            .set("points", points)
            .with_common(shape),
    )
}

#[cfg(feature = "drawing-svg")]
fn build_text(shape: &serde_json::Value) -> Option<svg::node::element::Text> {
    let x = parse_opt_f64(&shape["x"])?;
    let y = parse_opt_f64(&shape["y"])?;
    let content = shape["content"].as_str().unwrap_or("");
    let mut text = svg::node::element::Text::new(content)
        .set("x", x)
        .set("y", y);
    if let Some(size) = parse_opt_f64(&shape["font_size"]) {
        text = text.set("font-size", size);
    }
    if let Some(family) = parse_opt_str(&shape["font_family"]) {
        text = text.set("font-family", family);
    }
    if let Some(anchor) = parse_opt_str(&shape["text_anchor"]) {
        text = text.set("text-anchor", anchor);
    }
    Some(text.with_common(shape))
}

#[cfg(feature = "drawing-svg")]
fn build_path(shape: &serde_json::Value) -> Option<svg::node::element::Path> {
    let d = shape["d"].as_str()?;
    Some(
        svg::node::element::Path::new()
            .set("d", d)
            .with_common(shape),
    )
}

// ── SvgExportTool ────────────────────────────────────────────────────────────
//
// Converts a simple DXF-like entity description to an SVG string.
// Accepts `entities`: an array of `{type: "line"|"circle"|"arc", ...}` objects.

#[cfg(feature = "drawing-svg")]
pub struct SvgExportTool;

#[cfg(feature = "drawing-svg")]
impl Tool for SvgExportTool {
    fn name(&self) -> &'static str {
        "svg_export"
    }

    fn exposure(&self) -> crate::orchestration::tool::ToolExposure {
        crate::orchestration::tool::ToolExposure::Deferred
    }


    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        use svg::node::element::path::Data;

        let entities = input.payload["entities"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("missing 'entities' array"))?;

        let width = input.payload["width"].as_f64().unwrap_or(800.0);
        let height = input.payload["height"].as_f64().unwrap_or(600.0);

        let mut document = svg::Document::new()
            .set("xmlns", "http://www.w3.org/2000/svg")
            .set("width", width)
            .set("height", height)
            .set("viewBox", (0, 0, width as i64, height as i64));

        for (i, entity) in entities.iter().enumerate() {
            let entity_type = entity["type"].as_str().unwrap_or("");
            match entity_type {
                "line" => {
                    let x1 = entity["x1"].as_f64().unwrap_or(0.0);
                    let y1 = entity["y1"].as_f64().unwrap_or(0.0);
                    let x2 = entity["x2"].as_f64().unwrap_or(0.0);
                    let y2 = entity["y2"].as_f64().unwrap_or(0.0);
                    let stroke = entity["color"].as_str().unwrap_or("black");
                    let stroke_width = entity["stroke_width"].as_f64().unwrap_or(1.0);
                    let line = svg::node::element::Line::new()
                        .set("x1", x1)
                        .set("y1", y1)
                        .set("x2", x2)
                        .set("y2", y2)
                        .set("stroke", stroke)
                        .set("stroke-width", stroke_width);
                    document = document.add(line);
                }
                "circle" => {
                    let cx = entity["cx"].as_f64().unwrap_or(0.0);
                    let cy = entity["cy"].as_f64().unwrap_or(0.0);
                    let r = entity["r"]
                        .as_f64()
                        .ok_or_else(|| anyhow::anyhow!("entity[{i}] 'circle' missing 'r'"))?;
                    let stroke = entity["color"].as_str().unwrap_or("black");
                    let fill = entity["fill"].as_str().unwrap_or("none");
                    let circle = svg::node::element::Circle::new()
                        .set("cx", cx)
                        .set("cy", cy)
                        .set("r", r)
                        .set("stroke", stroke)
                        .set(
                            "stroke-width",
                            entity["stroke_width"].as_f64().unwrap_or(1.0),
                        )
                        .set("fill", fill);
                    document = document.add(circle);
                }
                "arc" => {
                    let cx = entity["cx"].as_f64().unwrap_or(0.0);
                    let cy = entity["cy"].as_f64().unwrap_or(0.0);
                    let r = entity["r"]
                        .as_f64()
                        .ok_or_else(|| anyhow::anyhow!("entity[{i}] 'arc' missing 'r'"))?;
                    let start_angle = entity["start_angle"].as_f64().unwrap_or(0.0).to_radians();
                    let end_angle = entity["end_angle"].as_f64().unwrap_or(360.0).to_radians();
                    let stroke = entity["color"].as_str().unwrap_or("black");
                    let fill = entity["fill"].as_str().unwrap_or("none");

                    // Compute arc endpoints
                    let x1 = cx + r * start_angle.cos();
                    let y1 = cy + r * start_angle.sin();
                    let x2 = cx + r * end_angle.cos();
                    let y2 = cy + r * end_angle.sin();

                    let large_arc = if (end_angle - start_angle).abs() > std::f64::consts::PI {
                        1
                    } else {
                        0
                    };

                    let data = Data::new()
                        .move_to((x1, y1))
                        .elliptical_arc_to((r, r, 0, large_arc, 0, x2, y2));

                    let path = svg::node::element::Path::new()
                        .set("d", data)
                        .set("stroke", stroke)
                        .set(
                            "stroke-width",
                            entity["stroke_width"].as_f64().unwrap_or(1.0),
                        )
                        .set("fill", fill);
                    document = document.add(path);
                }
                other => {
                    info!(type = other, index = i, "unknown entity type in svg_export");
                }
            }
        }

        let svg_string = document.to_string();
        let byte_size = svg_string.len();
        let entity_count = entities.len();

        info!(
            entities = entity_count,
            byte_size = byte_size,
            "SVG exported from entities"
        );

        let report = tool_execution_report("svg_export", Some("export"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "svg": svg_string,
                "entity_count": entity_count,
                "byte_size": byte_size,
                "width": width,
                "height": height,
            })),
            error: None,
            verification: None,
            audit_log: Some(format!(
                "svg_export: {} entities, {} bytes",
                entity_count, byte_size,
            )),
            pua_report: Some(report),
        })
    }
}

#[cfg(test)]
#[cfg(feature = "drawing-svg")]
mod tests {
    use super::*;

    fn test_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "svg-export-test".to_string(),
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
    fn export_line_entity() {
        let tool = SvgExportTool;
        let input = test_input(serde_json::json!({
            "entities": [
                {"type": "line", "x1": 10.0, "y1": 20.0, "x2": 100.0, "y2": 200.0},
            ],
        }));
        let output = tool.run(&input).expect("svg_export should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        let svg = result["svg"].as_str().unwrap();
        assert!(svg.contains("<line"));
        assert_eq!(result["entity_count"].as_u64().unwrap(), 1);
    }

    #[test]
    fn export_circle_entity() {
        let tool = SvgExportTool;
        let input = test_input(serde_json::json!({
            "entities": [
                {"type": "circle", "cx": 50.0, "cy": 50.0, "r": 30.0},
            ],
        }));
        let output = tool.run(&input).expect("svg_export should succeed");
        assert!(output.success);
        let svg = output.result.unwrap()["svg"].as_str().unwrap().to_string();
        assert!(svg.contains("<circle"));
    }

    #[test]
    fn export_arc_entity() {
        let tool = SvgExportTool;
        let input = test_input(serde_json::json!({
            "entities": [
                {"type": "arc", "cx": 100.0, "cy": 100.0, "r": 50.0, "start_angle": 0.0, "end_angle": 90.0},
            ],
        }));
        let output = tool.run(&input).expect("svg_export should succeed");
        assert!(output.success);
        let svg = output.result.unwrap()["svg"].as_str().unwrap().to_string();
        assert!(svg.contains("<path"));
    }

    #[test]
    fn export_missing_entities_errors() {
        let tool = SvgExportTool;
        let input = test_input(serde_json::json!({}));
        let result = tool.run(&input);
        assert!(result.is_err());
    }

    #[test]
    fn export_mixed_entities() {
        let tool = SvgExportTool;
        let input = test_input(serde_json::json!({
            "entities": [
                {"type": "line", "x1": 0.0, "y1": 0.0, "x2": 100.0, "y2": 100.0},
                {"type": "circle", "cx": 50.0, "cy": 50.0, "r": 25.0},
                {"type": "arc", "cx": 50.0, "cy": 50.0, "r": 40.0, "start_angle": 0.0, "end_angle": 180.0},
            ],
            "width": 400,
            "height": 300,
        }));
        let output = tool.run(&input).expect("svg_export should succeed");
        assert!(output.success);
        let svg = output.result.unwrap()["svg"].as_str().unwrap().to_string();
        assert!(svg.contains("<line"));
        assert!(svg.contains("<circle"));
        assert!(svg.contains("<path"));
        assert!(svg.contains("viewBox=\"0 0 400 300\""));
    }
}
