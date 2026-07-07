//! STEP (ISO 10303-21) CAD file reading tools
//!
//! Provides `StepReadTool` for reading STEP CAD file metadata and structure.
//! STEP files use a plain-text EXPRESS-based encoding with header and data
//! sections. Parsing is done natively without external dependencies.
//! Only compiled when `feature = "cad-step"` is enabled.

#[cfg(feature = "cad-step")]
use crate::governance::pua::tool_execution_report;
#[cfg(feature = "cad-step")]
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
#[cfg(feature = "cad-step")]
use anyhow::{Context, Result};
#[cfg(feature = "cad-step")]
use std::collections::BTreeMap;
#[cfg(feature = "cad-step")]
use std::fs;
#[cfg(feature = "cad-step")]
use tracing::info;

/// Parsed STEP file header fields.
#[cfg(feature = "cad-step")]
struct StepHeader {
    description: Vec<String>,
    name: String,
    time_stamp: Option<String>,
    author: Vec<String>,
    organization: Vec<String>,
    preprocessor_version: Option<String>,
    originating_system: Option<String>,
    authorization: Option<String>,
    schema: Vec<String>,
}

/// A simple representation of a STEP entity.
#[cfg(feature = "cad-step")]
struct StepEntity {
    /// Entity ID (used in test assertions)
    #[allow(dead_code)]
    id: i64,
    type_name: String,
}

/// Parsed STEP file summary.
#[cfg(feature = "cad-step")]
struct StepSummary {
    header: StepHeader,
    entity_type_counts: BTreeMap<String, usize>,
    entity_count: usize,
    byte_size: usize,
}

/// Parse the HEADER section of a STEP file.
#[cfg(feature = "cad-step")]
fn parse_header(lines: &[&str]) -> StepHeader {
    let mut description: Vec<String> = Vec::new();
    let mut name = String::new();
    let mut time_stamp: Option<String> = None;
    let mut author: Vec<String> = Vec::new();
    let mut organization: Vec<String> = Vec::new();
    let mut preprocessor_version: Option<String> = None;
    let mut originating_system: Option<String> = None;
    let mut authorization: Option<String> = None;
    let mut schema: Vec<String> = Vec::new();

    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with("FILE_DESCRIPTION") {
            // FILE_DESCRIPTION(('desc1','desc2'),'level');
            if let Some(inner) = extract_parenthesized(trimmed) {
                let first_arg = extract_string_list(&inner);
                description = first_arg;
            }
        } else if trimmed.starts_with("FILE_NAME") {
            // FILE_NAME('name','time',('author'),('org'),'pv','os','auth');
            if let Some(inner) = extract_parenthesized(trimmed) {
                let args = split_step_arguments(&inner);
                if let Some(v) = args.first().and_then(|s| extract_step_string(s)) {
                    name = v;
                }
                if let Some(v) = args.get(1).and_then(|s| extract_step_string(s)) {
                    time_stamp = Some(v);
                }
                if let Some(v) = args.get(2).map(|s| extract_string_list_raw(s)) {
                    author = v;
                }
                if let Some(v) = args.get(3).map(|s| extract_string_list_raw(s)) {
                    organization = v;
                }
                if let Some(v) = args.get(4).and_then(|s| extract_step_string(s)) {
                    preprocessor_version = Some(v);
                }
                if let Some(v) = args.get(5).and_then(|s| extract_step_string(s)) {
                    originating_system = Some(v);
                }
                if let Some(v) = args.get(6).and_then(|s| extract_step_string(s)) {
                    authorization = Some(v);
                }
            }
        } else if trimmed.starts_with("FILE_SCHEMA") {
            // FILE_SCHEMA(('schema1','schema2'));
            if let Some(inner) = extract_parenthesized(trimmed) {
                schema = extract_string_list(&inner);
            }
        }
    }

    StepHeader {
        description,
        name,
        time_stamp,
        author,
        organization,
        preprocessor_version,
        originating_system,
        authorization,
        schema,
    }
}

/// Extract the content between the outermost parentheses, handling nested parens.
#[cfg(feature = "cad-step")]
fn extract_parenthesized(s: &str) -> Option<String> {
    let start = s.find('(')?;
    let mut depth = 0u32;
    let mut result = String::new();
    let mut in_string = false;

    for ch in s[start..].chars() {
        match ch {
            '\'' => {
                in_string = !in_string;
                result.push(ch);
            }
            '(' if !in_string => {
                if depth > 0 {
                    result.push(ch);
                }
                depth += 1;
            }
            ')' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    return Some(result);
                }
                result.push(ch);
            }
            _ => {
                if depth > 0 {
                    result.push(ch);
                }
            }
        }
    }
    None
}

/// Extract a STEP string (single-quoted) from raw text, stripping the quotes.
#[cfg(feature = "cad-step")]
fn extract_step_string(s: &str) -> Option<String> {
    let s = s.trim();
    if s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2 {
        // Handle escaped single quotes ('' -> ')
        let inner = &s[1..s.len() - 1];
        Some(inner.replace("''", "'"))
    } else {
        None
    }
}

/// Extract a list of strings from a STEP-formatted list like ('a','b','c').
/// The input `s` should be the content inside the outer parens of a list.
#[cfg(feature = "cad-step")]
fn extract_string_list(s: &str) -> Vec<String> {
    let s = s.trim();
    // Remove surrounding parentheses if present
    let inner = if s.starts_with('(') && s.ends_with(')') {
        &s[1..s.len() - 1]
    } else {
        s
    };
    extract_string_list_raw(inner)
}

/// Extract individual strings from a comma-separated STEP string list body.
#[cfg(feature = "cad-step")]
fn extract_string_list_raw(s: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut in_string = false;
    let mut chars = s.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '\'' => {
                if in_string {
                    // Check for escaped quote ''
                    if chars.peek() == Some(&'\'') {
                        current.push('\'');
                        chars.next(); // skip the second quote
                    } else {
                        in_string = false;
                    }
                } else {
                    in_string = true;
                    current.clear();
                }
            }
            ',' if !in_string => {
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    result.push(trimmed);
                }
                current.clear();
            }
            _ => {
                if in_string {
                    current.push(ch);
                }
            }
        }
    }
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        result.push(trimmed);
    }

    result
}

/// Split the arguments inside a STEP function call, respecting string boundaries.
#[cfg(feature = "cad-step")]
fn split_step_arguments(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut depth = 0u32;
    let mut in_string = false;

    for ch in s.chars() {
        match ch {
            '\'' => {
                in_string = !in_string;
                current.push(ch);
            }
            '(' if !in_string => {
                depth += 1;
                current.push(ch);
            }
            ')' if !in_string => {
                depth -= 1;
                current.push(ch);
            }
            ',' if !in_string && depth == 0 => {
                args.push(current.trim().to_string());
                current.clear();
            }
            _ => {
                current.push(ch);
            }
        }
    }
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        args.push(trimmed);
    }

    args
}

/// Parse a STEP entity line like `#123 = CARTESIAN_POINT('name',(1.0,2.0,3.0));`
#[cfg(feature = "cad-step")]
fn parse_entity_line(line: &str) -> Option<StepEntity> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('/') || line.starts_with("!--") {
        return None;
    }

    // Strip trailing semicolon
    let content = line.strip_suffix(';').unwrap_or(line);

    // Find the entity ID (e.g., #123)
    if !content.starts_with('#') {
        return None;
    }

    let eq_pos = content.find('=')?;
    let id_str = content[1..eq_pos].trim();
    let id: i64 = id_str.parse().ok()?;

    let rest = content[eq_pos + 1..].trim();

    // The type name is until the first '('
    let paren_pos = rest.find('(')?;
    let type_name = rest[..paren_pos].trim().to_string();

    Some(StepEntity { id, type_name })
}

/// Parse the full STEP file content.
#[cfg(feature = "cad-step")]
fn parse_step(content: &str) -> StepSummary {
    let lines: Vec<&str> = content.lines().collect();
    let byte_size = content.len();

    let mut in_header = false;
    let mut in_data = false;
    let mut header_lines: Vec<&str> = Vec::new();
    let mut entity_count = 0usize;
    let mut entity_type_counts: BTreeMap<String, usize> = BTreeMap::new();

    for line in &lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if trimmed == "HEADER;" {
            in_header = true;
            continue;
        }
        if trimmed == "DATA;" {
            in_header = false;
            in_data = true;
            continue;
        }
        if trimmed == "ENDSEC;" {
            in_header = false;
            in_data = false;
            continue;
        }

        if in_header {
            header_lines.push(line);
        } else if in_data {
            if let Some(entity) = parse_entity_line(line) {
                *entity_type_counts.entry(entity.type_name).or_insert(0) += 1;
                entity_count += 1;
            }
        }
    }

    let header = if header_lines.is_empty() {
        // If no explicit HEADER section, try to parse from the beginning
        StepHeader {
            description: Vec::new(),
            name: String::new(),
            time_stamp: None,
            author: Vec::new(),
            organization: Vec::new(),
            preprocessor_version: None,
            originating_system: None,
            authorization: None,
            schema: Vec::new(),
        }
    } else {
        parse_header(&header_lines)
    };

    StepSummary {
        header,
        entity_count,
        entity_type_counts,
        byte_size,
    }
}

#[cfg(feature = "cad-step")]
pub struct StepReadTool;

#[cfg(feature = "cad-step")]
impl Tool for StepReadTool {
    fn name(&self) -> &'static str {
        "step_read"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing 'path'"))?;
        let validated = sanitize_path(input, path)?;

        let content = fs::read_to_string(&validated)
            .with_context(|| format!("failed to read STEP: {}", validated.display()))?;

        // Basic validation: must be a STEP file
        if !content.contains("ISO-10303-21") {
            anyhow::bail!(
                "not a valid STEP file (missing ISO-10303-21 header): {}",
                validated.display()
            );
        }

        let summary = parse_step(&content);

        let entity_types: Vec<serde_json::Value> = summary
            .entity_type_counts
            .iter()
            .map(|(type_name, count)| {
                serde_json::json!({
                    "type": type_name,
                    "count": count,
                })
            })
            .collect();

        info!(
            path = %validated.display(),
            entities = summary.entity_count,
            "STEP file read"
        );

        let report = tool_execution_report("step_read", Some("cad_read"));

        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({
                "entity_count": summary.entity_count,
                "entity_types": entity_types,
                "description": summary.header.description,
                "name": summary.header.name,
                "time_stamp": summary.header.time_stamp,
                "author": summary.header.author,
                "organization": summary.header.organization,
                "preprocessor_version": summary.header.preprocessor_version,
                "originating_system": summary.header.originating_system,
                "authorization": summary.header.authorization,
                "schema": summary.header.schema,
                "byte_size": summary.byte_size,
                "path": validated.to_string_lossy(),
            })),
            error: None,
            verification: None,
            audit_log: Some(format!(
                "step_read: {} entities from {}",
                summary.entity_count,
                validated.display()
            )),
            pua_report: Some(report),
        })
    }
}

#[cfg(test)]
#[cfg(feature = "cad-step")]
mod tests {
    use super::*;

    #[test]
    fn parse_extract_string_list() {
        let result = extract_string_list_raw("'Hello World'");
        assert_eq!(result, vec!["Hello World"]);
    }

    #[test]
    fn parse_extract_multiple_strings() {
        let result = extract_string_list_raw("'First','Second','Third'");
        assert_eq!(result, vec!["First", "Second", "Third"]);
    }

    #[test]
    fn parse_step_entity_line() {
        let line = "#10 = CARTESIAN_POINT('Origin',(0.0,0.0,0.0));";
        let entity = parse_entity_line(line).expect("should parse entity");
        assert_eq!(entity.id, 10);
        assert_eq!(entity.type_name, "CARTESIAN_POINT");
    }

    #[test]
    fn parse_sample_step_file() {
        let step = r#"ISO-10303-21;
HEADER;
FILE_DESCRIPTION(('Test part'),'2;1');
FILE_NAME('test.stp','2024-01-15T12:00:00',('Author'),('Organization'),'PrePro V1','OrigSys','None');
FILE_SCHEMA(('CONFIG_CONTROL_DESIGN'));
ENDSEC;
DATA;
#1 = CARTESIAN_POINT('Origin',(0.0,0.0,0.0));
#2 = CARTESIAN_POINT('Corner',(10.0,20.0,30.0));
#3 = LINE('Edge',#1,#2);
ENDSEC;
END-ISO-10303-21;
"#;
        let summary = parse_step(step);
        assert_eq!(summary.entity_count, 3);
        assert_eq!(
            *summary.entity_type_counts.get("CARTESIAN_POINT").unwrap(),
            2
        );
        assert_eq!(*summary.entity_type_counts.get("LINE").unwrap(), 1);
        assert_eq!(summary.header.name, "test.stp");
        assert_eq!(summary.header.description, vec!["Test part", "2;1"]);
        assert_eq!(summary.header.author, vec!["Author"]);
    }

    #[test]
    fn parse_step_header_missing_section() {
        // Some STEP files may not have explicit HEADER/DATA markers
        // but still have the content. Our parser should handle gracefully.
        let step = "ISO-10303-21;\n#1 = POINT('P',(0.,0.,0.));\nEND-ISO-10303-21;\n";
        let summary = parse_step(step);
        // If no DATA section, no entities parsed
        assert_eq!(summary.entity_count, 0);
    }
}
