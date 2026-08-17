//! Descriptors for CAD / 3D / drawing tools.

use crate::mcp::McpTool;
use serde_json::json;

/// Returns the MCP tool descriptor for a known CAD/3D/drawing tool name, or `None`.
pub(super) fn descriptor(name: &'static str) -> Option<McpTool> {
    match name {
        // ── CAD / 3D / drawing tools ────────────────────────────────
        "stl_read" => Some(McpTool {
            name: name.to_string(),
            description: Some("Read an STL 3D model file and return facet count, bounding box, volume estimate, unique vertex count, and format (binary/ascii).".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the .stl file"}
                },
                "required": ["path"]
            })),
        }),
        "stl_generate" => Some(McpTool {
            name: name.to_string(),
            description: Some("Generate an ASCII STL file from vertex and face data.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "vertices": {"type": "array", "items": {"type": "array", "items": {"type": "number"}}, "description": "List of [x,y,z] vertices"},
                    "faces": {"type": "array", "items": {"type": "array", "items": {"type": "integer"}}, "description": "List of [i,j,k] face vertex indices (0-based)"},
                    "path": {"type": "string", "description": "Output .stl path"}
                },
                "required": ["vertices", "faces", "path"]
            })),
        }),
        "obj_read" => Some(McpTool {
            name: name.to_string(),
            description: Some("Read a Wavefront OBJ 3D model file and return vertex/texture/normal/face counts, object names, materials, and bounding box.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the .obj file"}
                },
                "required": ["path"]
            })),
        }),
        "dxf_read" => Some(McpTool {
            name: name.to_string(),
            description: Some("Read a DXF CAD file and extract entity metadata.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the .dxf file"}
                },
                "required": ["path"]
            })),
        }),
        "step_read" => Some(McpTool {
            name: name.to_string(),
            description: Some("Read a STEP CAD file and extract model metadata.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the .step file"}
                },
                "required": ["path"]
            })),
        }),
        "iges_read" => Some(McpTool {
            name: name.to_string(),
            description: Some("Read an IGES CAD file and extract model metadata.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the .iges file"}
                },
                "required": ["path"]
            })),
        }),
        "ply_read" => Some(McpTool {
            name: name.to_string(),
            description: Some("Read a PLY 3D mesh file and return vertex/face counts and bounding box.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the .ply file"}
                },
                "required": ["path"]
            })),
        }),
        "gltf_read" => Some(McpTool {
            name: name.to_string(),
            description: Some("Read a glTF 3D model file and extract scene metadata.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the .gltf/.glb file"}
                },
                "required": ["path"]
            })),
        }),
        "gcode_read" => Some(McpTool {
            name: name.to_string(),
            description: Some("Read a G-code file and return command statistics.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the .gcode file"}
                },
                "required": ["path"]
            })),
        }),
        "gpx_read" => Some(McpTool {
            name: name.to_string(),
            description: Some("Read a GPX GPS track file and return waypoints, tracks, and routes.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the .gpx file"}
                },
                "required": ["path"]
            })),
        }),
        "geo_util" => Some(McpTool {
            name: name.to_string(),
            description: Some("Geospatial utilities: calculate distances, bearings, and operations on coordinate points.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "operation": {"type": "string", "description": "Operation to perform"},
                    "points": {"type": "array", "items": {"type": "object"}, "description": "Coordinate points"}
                },
                "required": ["operation", "points"]
            })),
        }),
        "cad_convert" => Some(McpTool {
            name: name.to_string(),
            description: Some("Convert a numeric value between CAD unit systems (e.g. feet to meters).".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "value": {"type": "number", "description": "Numeric value to convert"},
                    "from": {"type": "string", "description": "Source unit (e.g. 'ft')"},
                    "to": {"type": "string", "description": "Target unit (e.g. 'm')"},
                    "operation": {"type": "string", "description": "Optional operation name"}
                },
                "required": ["value", "from", "to"]
            })),
        }),
        "svg_read" => Some(McpTool {
            name: name.to_string(),
            description: Some("Read an SVG file and return shape/attribute information.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the .svg file"}
                },
                "required": ["path"]
            })),
        }),
        "svg_generate" => Some(McpTool {
            name: name.to_string(),
            description: Some("Generate an SVG file from shape definitions.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Output .svg path"},
                    "width": {"type": "integer", "description": "Canvas width"},
                    "height": {"type": "integer", "description": "Canvas height"},
                    "shapes": {"type": "array", "description": "Shape definitions"}
                },
                "required": ["path"]
            })),
        }),
        "svg_export" => Some(McpTool {
            name: name.to_string(),
            description: Some("Export entities to an SVG file.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "entities": {"type": "array", "description": "Entities to export"},
                    "width": {"type": "integer", "description": "Canvas width"},
                    "height": {"type": "integer", "description": "Canvas height"}
                },
                "required": ["entities"]
            })),
        }),
        "barcode_gen" => Some(McpTool {
            name: name.to_string(),
            description: Some("Generate a barcode (EAN-13, Code-128, QR) as an SVG.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "data": {"type": "string", "description": "Data to encode"},
                    "format": {"type": "string", "enum": ["ean13", "code128", "qr"], "description": "Barcode format"},
                    "width": {"type": "integer", "description": "Image width"},
                    "height": {"type": "integer", "description": "Image height"}
                },
                "required": ["data", "format"]
            })),
        }),
        _ => None,
    }
}
