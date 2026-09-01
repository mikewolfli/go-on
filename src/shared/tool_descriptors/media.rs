//! Descriptors for image tools.

use crate::mcp::McpTool;
use serde_json::json;

/// Returns the MCP tool descriptor for a known image tool name, or `None`.
pub(super) fn descriptor(name: &str) -> Option<McpTool> {
    match name {
        // ── Image tools ──────────────────────────────────────────────
        "image_analyze" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "Analyze an image: dimensions, color statistics, and kind detection.".to_string(),
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the image file"},
                    "output_path": {"type": "string", "description": "Optional analysis report output path"},
                    "kind": {"type": "string", "description": "Analysis kind"},
                    "color": {"type": "boolean", "description": "Include color statistics"},
                    "width": {"type": "integer", "description": "Resize width before analysis"},
                    "height": {"type": "integer", "description": "Resize height before analysis"}
                },
                "required": ["path"]
            })),
        }),
        "image_convert" => Some(McpTool {
            name: name.to_string(),
            description: Some("Convert an image between formats (png/jpeg/gif/webp).".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Input image path"},
                    "output_path": {"type": "string", "description": "Output image path"},
                    "format": {"type": "string", "enum": ["png", "jpeg", "gif", "webp"], "description": "Target format"}
                },
                "required": ["path", "output_path"]
            })),
        }),
        "image_resize" => Some(McpTool {
            name: name.to_string(),
            description: Some("Resize or crop an image.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Input image path"},
                    "output_path": {"type": "string", "description": "Output image path"},
                    "width": {"type": "integer", "description": "Target width"},
                    "height": {"type": "integer", "description": "Target height"},
                    "maintain_aspect": {"type": "boolean", "description": "Maintain aspect ratio"},
                    "crop": {"type": "boolean", "description": "Crop to exact dimensions"}
                },
                "required": ["path", "output_path", "width", "height"]
            })),
        }),
        "image_generate" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "Generate a synthetic image (grid, gradient, or pattern).".to_string(),
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "output_path": {"type": "string", "description": "Output image path"},
                    "kind": {"type": "string", "description": "Generation kind (grid/gradient/...)"},
                    "width": {"type": "integer", "description": "Image width"},
                    "height": {"type": "integer", "description": "Image height"},
                    "color": {"type": "string", "description": "Base color"},
                    "cell_size": {"type": "integer", "description": "Grid cell size"},
                    "direction": {"type": "string", "description": "Gradient direction"}
                },
                "required": ["output_path", "kind"]
            })),
        }),
        _ => None,
    }
}
