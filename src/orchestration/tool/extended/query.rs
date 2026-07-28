//! Structured data query tools.
//!
//! Query JSON, YAML, and other structured data formats using jq-like syntax.

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::{sanitize_path, Tool, ToolInput, ToolOutput};
use tracing::debug;

/// Resolve a simple dot-separated path against a JSON value.
///
/// Supports:
/// - `obj.key` for object field access
/// - `arr[0]` for array index access
/// - `arr[0].nested.key` for chained access
///
/// Examples:
/// - `"users[0].name"` → gets the `name` field of the first user
/// - `"config.database.host"` → gets nested field
fn query_json_path<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    if path.is_empty() || path == "." {
        return Some(root);
    }

    // Tokenize path: split on '.' but keep bracket index access intact
    let mut current = root;

    // Simple path parser: handle both dot-separated keys and bracket index access
    let tokens = tokenize_path(path);

    for token in tokens {
        match token {
            PathToken::Key(key) => {
                current = current.get(key.as_str())?;
            }
            PathToken::Index(idx) => {
                current = current.get(idx)?;
            }
        }
    }

    Some(current)
}

enum PathToken {
    Key(String),
    Index(usize),
}

/// Tokenize a query path like "users[0].name" into a sequence of path tokens.
fn tokenize_path(path: &str) -> Vec<PathToken> {
    let mut tokens = Vec::new();
    let mut chars = path.chars().peekable();
    let mut current_key = String::new();

    while let Some(ch) = chars.next() {
        match ch {
            '[' => {
                // Flush accumulated key
                if !current_key.is_empty() {
                    tokens.push(PathToken::Key(std::mem::take(&mut current_key)));
                }
                // Read index until ']'
                let mut index_str = String::new();
                for c in chars.by_ref() {
                    if c == ']' {
                        break;
                    }
                    index_str.push(c);
                }
                if let Ok(idx) = index_str.parse::<usize>() {
                    tokens.push(PathToken::Index(idx));
                }
            }
            '.' => {
                if !current_key.is_empty() {
                    tokens.push(PathToken::Key(std::mem::take(&mut current_key)));
                }
                // Skip leading dots and handle consecutive dots
            }
            _ => {
                current_key.push(ch);
            }
        }
    }

    // Flush remaining key
    if !current_key.is_empty() {
        tokens.push(PathToken::Key(current_key));
    }

    tokens
}

// ── JsonQueryTool ──────────────────────────────────────────────────────────

pub struct JsonQueryTool;

impl Tool for JsonQueryTool {
    fn name(&self) -> &'static str {
        "json_query"
    }
    fn description(&self) -> &str {
        "Read a JSON file and query it using a simple path syntax like 'obj.key[0].nested'"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required field: path"))?;

        let query = input.payload["query"].as_str().unwrap_or(".");

        let file_path = sanitize_path(input, path)?;
        let query_str = query.to_string();

        debug!(
            path = %file_path.display(),
            query = %query_str,
            "tool: json_query reading file"
        );

        let content = std::fs::read_to_string(&file_path)
            .with_context(|| format!("failed to read JSON file '{}'", file_path.display()))?;

        let parsed: Value = serde_json::from_str(&content)
            .with_context(|| format!("failed to parse JSON from '{}'", file_path.display()))?;

        let result = query_json_path(&parsed, &query_str);

        match result {
            Some(value) => {
                debug!(
                    path = %file_path.display(),
                    query = %query_str,
                    "tool: json_query found result"
                );
                Ok(ToolOutput {
                    success: true,
                    result: Some(json!({
                        "path": path,
                        "query": query_str,
                        "found": true,
                        "value": value,
                        "value_type": json_type_name(value),
                    })),
                    error: None,
                    verification: Some("json_query_success".to_string()),
                    audit_log: Some(format!(
                        "json_query '{}' with query '{}'",
                        file_path.display(),
                        query_str
                    )),
                    pua_report: Some(tool_execution_report(
                        "json_query",
                        Some("json_query_success"),
                    )),
                })
            }
            None => {
                debug!(
                    path = %file_path.display(),
                    query = %query_str,
                    "tool: json_query path not found"
                );
                Ok(ToolOutput {
                    success: false,
                    result: Some(json!({
                        "path": path,
                        "query": query_str,
                        "found": false,
                        "value": null,
                    })),
                    error: Some(format!("path '{}' not found in JSON document", query_str)),
                    verification: Some("json_query_not_found".to_string()),
                    audit_log: Some(format!(
                        "json_query '{}' query '{}' not found",
                        file_path.display(),
                        query_str
                    )),
                    pua_report: Some(tool_execution_report(
                        "json_query",
                        Some("json_query_not_found"),
                    )),
                })
            }
        }
    }
}

/// Return a human-friendly type name for a JSON value.
fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

// ── YamlQueryTool ──────────────────────────────────────────────────────────

#[cfg(feature = "data-export")]
pub struct YamlQueryTool;

#[cfg(feature = "data-export")]
impl Tool for YamlQueryTool {
    fn name(&self) -> &'static str {
        "yaml_query"
    }
    fn description(&self) -> &str {
        "Read a YAML file and query it using a simple path syntax like 'obj.key[0].nested'"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let path = input.payload["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required field: path"))?;

        let query = input.payload["query"].as_str().unwrap_or(".");

        let file_path = sanitize_path(input, path)?;
        let query_str = query.to_string();

        debug!(
            path = %file_path.display(),
            query = %query_str,
            "tool: yaml_query reading file"
        );

        let content = std::fs::read_to_string(&file_path)
            .with_context(|| format!("failed to read YAML file '{}'", file_path.display()))?;

        let parsed: Value = serde_yaml::from_str(&content)
            .with_context(|| format!("failed to parse YAML from '{}'", file_path.display()))?;

        let result = query_json_path(&parsed, &query_str);

        match result {
            Some(value) => {
                debug!(
                    path = %file_path.display(),
                    query = %query_str,
                    "tool: yaml_query found result"
                );
                Ok(ToolOutput {
                    success: true,
                    result: Some(json!({
                        "path": path,
                        "query": query_str,
                        "found": true,
                        "value": value,
                        "value_type": json_type_name(value),
                    })),
                    error: None,
                    verification: Some("yaml_query_success".to_string()),
                    audit_log: Some(format!(
                        "yaml_query '{}' with query '{}'",
                        file_path.display(),
                        query_str
                    )),
                    pua_report: Some(tool_execution_report(
                        "yaml_query",
                        Some("yaml_query_success"),
                    )),
                })
            }
            None => {
                debug!(
                    path = %file_path.display(),
                    query = %query_str,
                    "tool: yaml_query path not found"
                );
                Ok(ToolOutput {
                    success: false,
                    result: Some(json!({
                        "path": path,
                        "query": query_str,
                        "found": false,
                        "value": null,
                    })),
                    error: Some(format!("path '{}' not found in YAML document", query_str)),
                    verification: Some("yaml_query_not_found".to_string()),
                    audit_log: Some(format!(
                        "yaml_query '{}' query '{}' not found",
                        file_path.display(),
                        query_str
                    )),
                    pua_report: Some(tool_execution_report(
                        "yaml_query",
                        Some("yaml_query_not_found"),
                    )),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::tool::ToolInput;
    use tempfile::TempDir;

    fn tool_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "test-query".to_string(),
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
    fn test_tokenize_simple_key() {
        let tokens = tokenize_path("name");
        assert_eq!(tokens.len(), 1);
        match &tokens[0] {
            PathToken::Key(k) => assert_eq!(k, "name"),
            _ => panic!("expected Key token"),
        }
    }

    #[test]
    fn test_tokenize_nested_keys() {
        let tokens = tokenize_path("config.database.host");
        assert_eq!(tokens.len(), 3);
    }

    #[test]
    fn test_tokenize_with_index() {
        let tokens = tokenize_path("users[0].name");
        assert_eq!(tokens.len(), 3);
        match &tokens[1] {
            PathToken::Index(i) => assert_eq!(*i, 0),
            _ => panic!("expected Index token at position 1"),
        }
    }

    #[test]
    fn test_query_json_simple() {
        let data = json!({"name": "hello", "count": 42});
        let result = query_json_path(&data, "name");
        assert!(result.is_some());
        assert_eq!(result.unwrap().as_str(), Some("hello"));
    }

    #[test]
    fn test_query_json_nested() {
        let data = json!({"config": {"host": "localhost", "port": 8080}});
        let result = query_json_path(&data, "config.host");
        assert!(result.is_some());
        assert_eq!(result.unwrap().as_str(), Some("localhost"));
    }

    #[test]
    fn test_query_json_array_index() {
        let data = json!({"users": [{"id": 1}, {"id": 2}]});
        let result = query_json_path(&data, "users[0].id");
        assert!(result.is_some());
        assert_eq!(result.unwrap().as_u64(), Some(1));
    }

    #[test]
    fn test_query_json_not_found() {
        let data = json!({"name": "hello"});
        let result = query_json_path(&data, "nonexistent");
        assert!(result.is_none());
    }

    #[test]
    fn test_query_json_root() {
        let data = json!({"key": "value"});
        let result = query_json_path(&data, ".");
        assert!(result.is_some());
        assert_eq!(result.unwrap(), &data);
    }

    #[test]
    fn json_query_reads_json_file() {
        let tmp = TempDir::new().expect("temp dir");
        let file_path = tmp.path().join("data.json");
        std::fs::write(&file_path, r#"{"name": "test", "values": [1, 2, 3]}"#).unwrap();

        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload: json!({
                "path": file_path.to_string_lossy(),
                "query": "name",
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let tool = JsonQueryTool;
        let output = tool.run(&input).expect("json_query should succeed");
        assert!(output.success);
        let result = output.result.unwrap();
        assert_eq!(result["value"].as_str(), Some("test"));
        assert_eq!(result["found"].as_bool(), Some(true));
    }

    #[test]
    fn json_query_not_found_returns_failure() {
        let tmp = TempDir::new().expect("temp dir");
        let file_path = tmp.path().join("data.json");
        std::fs::write(&file_path, r#"{"name": "test"}"#).unwrap();

        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload: json!({
                "path": file_path.to_string_lossy(),
                "query": "nonexistent",
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let tool = JsonQueryTool;
        let output = tool.run(&input).expect("json_query should return output");
        assert!(!output.success);
        let result = output.result.unwrap();
        assert_eq!(result["found"].as_bool(), Some(false));
    }

    #[test]
    fn json_query_requires_path() {
        let input = tool_input(json!({}));
        let tool = JsonQueryTool;
        let result = tool.run(&input);
        assert!(result.is_err());
    }

    #[test]
    fn json_query_non_existent_file() {
        let input = tool_input(json!({
            "path": "/nonexistent-file-12345.json",
        }));
        let tool = JsonQueryTool;
        let result = tool.run(&input);
        assert!(result.is_err());
    }
}
