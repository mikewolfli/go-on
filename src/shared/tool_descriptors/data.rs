//! Descriptors for data serialization / CSV tools.

use crate::mcp::McpTool;
use serde_json::json;

/// Returns the MCP tool descriptor for a known data serialization tool name, or `None`.
pub(super) fn descriptor(name: &'static str) -> Option<McpTool> {
    match name {
        // ── Data serialization / CSV tools ───────────────────────────
        "csv_read" => Some(McpTool {
            name: name.to_string(),
            description: Some("Read a CSV file into structured records.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the .csv file"},
                    "delimiter": {"type": "string", "description": "Field delimiter (default: ',')"},
                    "has_headers": {"type": "boolean", "description": "Whether the first row is headers (default: true)"},
                    "headers": {"type": "array", "items": {"type": "string"}, "description": "Explicit column headers"},
                    "records": {"type": "array", "description": "When reading, records are output"}
                },
                "required": ["path"]
            })),
        }),
        "csv_write" => Some(McpTool {
            name: name.to_string(),
            description: Some("Write structured records to a CSV file.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Output .csv path"},
                    "headers": {"type": "array", "items": {"type": "string"}, "description": "Column headers"},
                    "records": {"type": "array", "description": "Row records"},
                    "delimiter": {"type": "string", "description": "Field delimiter (default: ',')"}
                },
                "required": ["path", "headers", "records"]
            })),
        }),
        "csv_analyze" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "Analyze a CSV file and return column stats, types, and shape.".to_string(),
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the .csv file"},
                    "delimiter": {"type": "string", "description": "Field delimiter"},
                    "has_headers": {"type": "boolean", "description": "Whether the first row is headers"}
                },
                "required": ["path"]
            })),
        }),
        "csv_transform" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "Transform a CSV file: select, rename, and filter columns.".to_string(),
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Input .csv path"},
                    "output_path": {"type": "string", "description": "Output .csv path"},
                    "select": {"type": "array", "items": {"type": "string"}, "description": "Columns to keep"},
                    "rename": {"type": "object", "description": "Column rename map"},
                    "filter_column": {"type": "string", "description": "Column to filter on"},
                    "filter_value": {"type": "string", "description": "Filter value"},
                    "filter_invert": {"type": "boolean", "description": "Invert the filter"},
                    "delimiter": {"type": "string", "description": "Field delimiter"},
                    "has_headers": {"type": "boolean", "description": "Whether the first row is headers"}
                },
                "required": ["path", "output_path"]
            })),
        }),
        "toml_read" => Some(McpTool {
            name: name.to_string(),
            description: Some("Read a TOML file or TOML string into structured data.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the .toml file"},
                    "data": {"type": "string", "description": "TOML string (alternative to path)"}
                },
                "required": []
            })),
        }),
        "toml_write" => Some(McpTool {
            name: name.to_string(),
            description: Some("Serialize structured data into a TOML file.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Output .toml path"},
                    "data": {"type": "object", "description": "Data to serialize"}
                },
                "required": ["path", "data"]
            })),
        }),
        "yaml_read" => Some(McpTool {
            name: name.to_string(),
            description: Some("Read a YAML file or YAML string into structured data.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the .yaml file"},
                    "data": {"type": "string", "description": "YAML string (alternative to path)"}
                },
                "required": []
            })),
        }),
        "yaml_write" => Some(McpTool {
            name: name.to_string(),
            description: Some("Serialize structured data into a YAML file.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Output .yaml path"},
                    "data": {"type": "object", "description": "Data to serialize"}
                },
                "required": ["path", "data"]
            })),
        }),
        _ => None,
    }
}
