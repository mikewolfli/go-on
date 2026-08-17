//! Descriptors for document / office tools.

use crate::mcp::McpTool;
use serde_json::json;

/// Returns the MCP tool descriptor for a known document/office tool name, or `None`.
pub(super) fn descriptor(name: &'static str) -> Option<McpTool> {
    match name {
        // ── Document / office tools ────────────────────────────────
        "read_docx" => Some(McpTool {
            name: name.to_string(),
            description: Some("Read a Word .docx file and extract its text content.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the .docx file"}
                },
                "required": ["path"]
            })),
        }),
        "write_docx" => Some(McpTool {
            name: name.to_string(),
            description: Some("Create a Word .docx file from paragraphs and a title.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Output .docx path"},
                    "title": {"type": "string", "description": "Document title"},
                    "paragraphs": {"type": "array", "items": {"type": "string"}, "description": "Paragraph texts"}
                },
                "required": ["path", "paragraphs"]
            })),
        }),
        "read_pdf" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "Read a PDF file and extract text from one or more pages.".to_string(),
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the .pdf file"},
                    "paths": {"type": "array", "items": {"type": "string"}, "description": "Multiple PDF paths to read"},
                    "output_path": {"type": "string", "description": "Optional text output path"}
                },
                "required": ["path"]
            })),
        }),
        "pdf_merge" => Some(McpTool {
            name: name.to_string(),
            description: Some("Merge multiple PDF files into one.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "paths": {"type": "array", "items": {"type": "string"}, "description": "Input PDF paths"},
                    "output_path": {"type": "string", "description": "Output .pdf path"}
                },
                "required": ["paths", "output_path"]
            })),
        }),
        "pdf_split" => Some(McpTool {
            name: name.to_string(),
            description: Some("Split a PDF file into a page range.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Input .pdf path"},
                    "start_page": {"type": "integer", "description": "First page (1-based)"},
                    "end_page": {"type": "integer", "description": "Last page (1-based)"},
                    "output_path": {"type": "string", "description": "Output .pdf path"}
                },
                "required": ["path", "output_path"]
            })),
        }),
        "read_excel" => Some(McpTool {
            name: name.to_string(),
            description: Some("Read an Excel .xlsx file and return sheet data.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the .xlsx file"},
                    "config": {"type": "object", "description": "Optional read configuration"}
                },
                "required": ["path"]
            })),
        }),
        "write_excel" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "Create an Excel .xlsx workbook with sheets from row data.".to_string(),
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Output .xlsx path"},
                    "config": {"type": "object", "description": "Workbook configuration"},
                    "slides": {"type": "array", "description": "Sheet/row data"}
                },
                "required": ["path"]
            })),
        }),
        "read_ppt" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "Read a PowerPoint .pptx file and extract slide content.".to_string(),
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the .pptx file"},
                    "config": {"type": "object", "description": "Optional read configuration"}
                },
                "required": ["path"]
            })),
        }),
        "write_ppt" => Some(McpTool {
            name: name.to_string(),
            description: Some("Create a PowerPoint .pptx file from slide definitions.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Output .pptx path"},
                    "slides": {"type": "array", "description": "Slide definitions"}
                },
                "required": ["path", "slides"]
            })),
        }),
        "email_parse" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "Parse an email message file (.eml) into structured fields.".to_string(),
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the email file"}
                },
                "required": ["path"]
            })),
        }),
        "invoice_parse" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "Parse an invoice from file or text into structured fields.".to_string(),
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the invoice file"},
                    "text": {"type": "string", "description": "Invoice text (alternative to path)"}
                },
                "required": []
            })),
        }),
        "web_scrape" => Some(McpTool {
            name: name.to_string(),
            description: Some(
                "Scrape structured content from a web page using a CSS selector.".to_string(),
            ),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string", "description": "Page URL to scrape"},
                    "selector": {"type": "string", "description": "CSS selector for content extraction"},
                    "timeout_ms": {"type": "integer", "description": "Timeout in milliseconds"}
                },
                "required": ["url"]
            })),
        }),
        "sqlite_query" => Some(McpTool {
            name: name.to_string(),
            description: Some("Run a SQL query against a SQLite database file.".to_string()),
            input_schema: Some(json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "Path to the .db file"},
                    "sql": {"type": "string", "description": "SQL query to execute"},
                    "max_rows": {"type": "integer", "description": "Maximum result rows"}
                },
                "required": ["path", "sql"]
            })),
        }),
        _ => None,
    }
}
