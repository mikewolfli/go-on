//! Required-argument validation for built-in tools.
//!
//! The validation contract is the shared tool-descriptor table's `required`
//! array (single source of truth — previously a hand-written match lived here
//! and silently drifted from the table: tools whose table rows declared
//! required fields got no validation, and one arm was duplicated). Tools
//! without a descriptor row (e.g. the `find_files` alias) fall back to the
//! legacy arms below.

use anyhow::Result;
use serde_json::Value;

/// Validate required arguments for a built-in tool.
///
/// Checks that the tool's required arguments are present in the provided input.
/// Returns an error with a descriptive message if any required argument is missing.
pub fn validate_required_arguments(tool_name: &str, tool_input: &Value) -> Result<()> {
    // Table-derived validation: the descriptor's `required` list IS the
    // contract. Error text keeps the historical `<tool> requires arguments.<field>`
    // shape so callers/tests that match on it keep working. `McpTool`
    // serializes the schema under camelCase `inputSchema`; accept both spellings.
    let desc = crate::shared::tool_descriptors::tool_descriptor_value(tool_name);
    let schema = desc.get("input_schema").or_else(|| desc.get("inputSchema"));
    if let Some(schema) = schema {
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            if !required.is_empty() {
                let missing: Vec<&str> = required
                    .iter()
                    .filter_map(|f| {
                        let field = f.as_str()?;
                        tool_input.get(field).is_none().then_some(field)
                    })
                    .collect();
                if !missing.is_empty() {
                    anyhow::bail!(
                        "{}",
                        missing
                            .iter()
                            .map(|f| format!("{tool_name} requires arguments.{f}"))
                            .collect::<Vec<_>>()
                            .join("; ")
                    );
                }
                return Ok(());
            }
        }
    }

    // ── Legacy fallback: tools without a descriptor row (aliases) ────────
    match tool_name {
        "find_files" => {
            tool_input
                .get("pattern")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("find_files requires arguments.pattern"))?;
        }
        "file_move" => {
            tool_input
                .get("source")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("file_move requires arguments.source"))?;
            tool_input
                .get("destination")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("file_move requires arguments.destination"))?;
        }
        "file_delete" => {
            tool_input
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("file_delete requires arguments.path"))?;
            tool_input
                .get("confirm")
                .and_then(|v| v.as_bool())
                .ok_or_else(|| {
                    anyhow::anyhow!("file_delete requires arguments.confirm (boolean)")
                })?;
        }
        _ => {}
    }
    Ok(())
}
