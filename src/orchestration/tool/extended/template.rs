//! Template rendering tool.
//!
//! Fills template variables using {{placeholder}} syntax.
//! When the `template-engine` feature is enabled, uses minijinja for
//! full Jinja2-style rendering. Otherwise falls back to simple string replacement.

#[cfg(feature = "template-engine")]
use std::collections::HashMap;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::{sanitize_path_for_write, Tool, ToolInput, ToolOutput};
use tracing::debug;

// ── Minijinja rendering (feature-gated) ────────────────────────────────

/// Render a template using minijinja (full Jinja2 syntax).
/// Only available when `template-engine` feature is enabled.
#[cfg(feature = "template-engine")]
fn render_minijinja(template: &str, variables: &Value) -> Result<String> {
    let mut env = minijinja::Environment::new();
    env.add_template("tpl", template)
        .map_err(|e| anyhow::anyhow!("invalid template: {e}"))?;
    let tpl = env
        .get_template("tpl")
        .map_err(|e| anyhow::anyhow!("template not found: {e}"))?;
    let vars: HashMap<String, serde_json::Value> =
        serde_json::from_value(variables.clone()).unwrap_or_default();
    let result = tpl
        .render(&vars)
        .map_err(|e| anyhow::anyhow!("template rendering error: {e}"))?;
    Ok(result)
}

// ── Tool struct and implementation ───────────────────────────────────────

pub struct TemplateRenderTool;

impl TemplateRenderTool {
    /// Render a template string by replacing {{variable}} placeholders,
    /// {{#each list}}...{{/each}} blocks, and {{#if var}}...{{/if}} blocks.
    ///
    /// When the `template-engine` feature is enabled, delegates to minijinja
    /// for full Jinja2 syntax support. Otherwise uses simple string replacement.
    ///
    /// Processing order (simple mode):
    /// 1. `{{#each}}` blocks (recursively renders their content per item)
    /// 2. `{{#if}}` blocks (conditionally includes content)
    /// 3. `{{variable}}` placeholders (simple replacement)
    fn render_template(template: &str, variables: &Value) -> String {
        #[cfg(feature = "template-engine")]
        {
            match render_minijinja(template, variables) {
                Ok(result) => return result,
                Err(e) => {
                    tracing::warn!("minijinja rendering failed, falling back to simple mode: {e}");
                }
            }
        }
        Self::render_simple(template, variables)
    }

    /// Simple string-replacement rendering (fallback when `template-engine`
    /// feature is disabled or minijinja fails).
    fn render_simple(template: &str, variables: &Value) -> String {
        let result = Self::render_each_blocks(template, variables);
        let result = Self::render_if_blocks(&result, variables);
        Self::render_variables(&result, variables)
    }

    /// Replace {{variable}} and {{variable|default}} placeholders.
    fn render_variables(template: &str, variables: &Value) -> String {
        let mut result = String::new();
        let mut rest = template;

        while let Some(start) = rest.find("{{") {
            // Push everything before the opening
            result.push_str(&rest[..start]);
            rest = &rest[start + 2..];

            // Find the closing }}
            if let Some(end) = rest.find("}}") {
                let placeholder = &rest[..end];
                rest = &rest[end + 2..];

                // Check for default value syntax: var|default_value
                let (var_name, default_val) = if let Some(pipe_pos) = placeholder.find('|') {
                    let name = placeholder[..pipe_pos].trim();
                    let default = placeholder[pipe_pos + 1..].trim();
                    (name, Some(default))
                } else {
                    (placeholder.trim(), None)
                };

                // Look up the variable value
                let value = resolve_variable(var_name, variables);
                match value {
                    Some(v) => {
                        // Convert to string: for strings use as-is, for other types use JSON
                        let s = v
                            .as_str()
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| v.to_string());
                        result.push_str(&s);
                    }
                    None => {
                        if let Some(default) = default_val {
                            result.push_str(default);
                        }
                        // If no default, leave empty string (removed)
                    }
                }
            } else {
                // No closing }} found, treat as literal
                result.push_str("{{");
                result.push_str(rest);
                break;
            }
        }

        result.push_str(rest);
        result
    }

    /// Render {{#each list}}...{{/each}} blocks.
    ///
    /// For each item in the list, the block content is rendered recursively
    /// with a scoped variables map that includes the item's fields merged
    /// with the outer variables. This allows {{#if field}} and {{field}}
    /// to reference properties of the current item.
    fn render_each_blocks(template: &str, variables: &Value) -> String {
        let mut result = String::new();
        let mut rest = template;

        while let Some(start) = rest.find("{{#each ") {
            result.push_str(&rest[..start]);
            rest = &rest[start + 8..]; // skip "{{#each "

            // Extract list variable name
            if let Some(end) = rest.find("}}") {
                let list_name = rest[..end].trim();
                rest = &rest[end + 2..]; // skip "}}"

                // Find the closing {{/each}}
                if let Some(close_start) = rest.find("{{/each}}") {
                    let block_content = &rest[..close_start];
                    rest = &rest[close_start + 9..]; // skip "{{/each}}"

                    // Resolve the list variable
                    let list_value = resolve_variable(list_name, variables);
                    let rendered = if let Some(Value::Array(items)) = list_value {
                        items
                            .iter()
                            .map(|item| {
                                // Build a scoped variables map: merge item fields into outer vars.
                                // Item fields take precedence.
                                let scoped = merge_variables(variables, item);
                                // Recursively render the block content with the scoped vars.
                                // This handles {{#if}} and {{variable}} inside the each block.
                                Self::render_template(block_content, &scoped)
                            })
                            .collect::<Vec<_>>()
                            .join("")
                    } else {
                        // If list not found or not an array, render empty
                        String::new()
                    };

                    result.push_str(&rendered);
                } else {
                    // No closing tag, treat as literal
                    result.push_str(&format!("{{{{#each {} }}}}", list_name));
                    result.push_str(rest);
                    break;
                }
            } else {
                result.push_str("{{#each ");
                result.push_str(rest);
                break;
            }
        }

        result.push_str(rest);
        result
    }

    /// Render {{#if var}}...{{/if}} blocks.
    ///
    /// If `var` resolves to a truthy value (present, non-empty, non-zero, non-false),
    /// the block content is rendered. Otherwise it is removed.
    fn render_if_blocks(template: &str, variables: &Value) -> String {
        let mut result = String::new();
        let mut rest = template;

        while let Some(start) = rest.find("{{#if ") {
            result.push_str(&rest[..start]);
            rest = &rest[start + 6..]; // skip "{{#if "

            // Extract variable name
            if let Some(end) = rest.find("}}") {
                let var_name = rest[..end].trim();
                rest = &rest[end + 2..]; // skip "}}"

                // Find the closing {{/if}}
                if let Some(close_start) = rest.find("{{/if}}") {
                    let block_content = &rest[..close_start];
                    rest = &rest[close_start + 7..]; // skip "{{/if}}"

                    // Check the condition
                    let value = resolve_variable(var_name, variables);
                    let is_truthy = is_truthy(value);

                    if is_truthy {
                        // Recursively render inner blocks too
                        let rendered = Self::render_template(block_content, variables);
                        result.push_str(&rendered);
                    }
                    // If falsy, skip the block entirely
                } else {
                    // No closing tag, treat as literal
                    result.push_str(&format!("{{{{#if {} }}}}", var_name));
                    result.push_str(rest);
                    break;
                }
            } else {
                result.push_str("{{#if ");
                result.push_str(rest);
                break;
            }
        }

        result.push_str(rest);
        result
    }
}

/// Resolve a dot-separated variable path from the variables map.
/// E.g., "user.name" resolves to variables["user"]["name"]
/// Special case: "." resolves to variables["."] for {{.}} in each blocks.
fn resolve_variable(path: &str, variables: &Value) -> Option<Value> {
    let trimmed = path.trim();
    if trimmed == "." {
        // Special case for {{.}} — look up literal key "." in the variables map
        return variables.get(".").cloned();
    }
    // Strip leading dot for paths like ".name" (from {{.name}})
    let cleaned = trimmed.strip_prefix('.').unwrap_or(trimmed);
    let parts: Vec<&str> = cleaned.split('.').collect();
    let mut current = variables;
    for part in parts {
        let part = part.trim();
        if part.is_empty() {
            return None;
        }
        match current {
            Value::Object(map) => {
                current = map.get(part)?;
            }
            Value::Array(arr) => {
                let idx: usize = part.parse().ok()?;
                current = arr.get(idx)?;
            }
            _ => return None,
        }
    }
    Some(current.clone())
}

/// Merge item fields into outer variables for scoped rendering in {{#each}} blocks.
/// Item fields take precedence over outer variables.
fn merge_variables(outer: &Value, item: &Value) -> Value {
    let mut merged = serde_json::Map::new();

    // Copy outer variables first
    if let Some(obj) = outer.as_object() {
        for (k, v) in obj {
            merged.insert(k.clone(), v.clone());
        }
    }

    // Merge item fields on top (item takes precedence)
    if let Some(obj) = item.as_object() {
        for (k, v) in obj {
            merged.insert(k.clone(), v.clone());
        }
    } else if !item.is_null() {
        // If item is a scalar, store it as "." so {{.}} works
        merged.insert(".".to_string(), item.clone());
    }

    Value::Object(merged)
}

/// Determine if a value is "truthy" for {{#if}} blocks.
fn is_truthy(value: Option<Value>) -> bool {
    match value {
        None => false,
        Some(Value::Null) => false,
        Some(Value::Bool(b)) => b,
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

impl Tool for TemplateRenderTool {
    fn name(&self) -> &'static str {
        "template_render"
    }
    fn description(&self) -> &str {
        "Render a template with {{variable}} replacement, {{#each}} loops, and {{#if}} conditionals"
    }
    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let template = input.payload["template"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required field: template"))?;

        let variables = input.payload["variables"]
            .as_object()
            .map(|obj| Value::Object(obj.clone()))
            .unwrap_or(Value::Object(serde_json::Map::new()));
        let variables = &variables;

        // Optional file output
        let output_path = input.payload["output_path"].as_str();

        debug!(
            template_len = %template.len(),
            variables_count = %variables.as_object().map(|m| m.len()).unwrap_or(0),
            output = ?output_path,
            "tool: template_render starting"
        );

        let rendered = Self::render_template(template, variables);

        // Write to file if output_path is specified
        if let Some(out_path) = output_path {
            let out_file = sanitize_path_for_write(input, out_path)
                .map_err(|e| anyhow::anyhow!("invalid output path '{}': {}", out_path, e))?;
            std::fs::write(&out_file, &rendered).with_context(|| {
                format!(
                    "failed to write rendered template to '{}'",
                    out_file.display()
                )
            })?;
            debug!(
                path = %out_file.display(),
                bytes = %rendered.len(),
                "tool: template_render wrote output file"
            );
        }

        Ok(ToolOutput {
            success: true,
            result: Some(json!({
                "rendered": rendered,
                "rendered_length": rendered.len(),
                "output_path": output_path,
            })),
            error: None,
            verification: Some("template_rendered".to_string()),
            audit_log: Some(format!(
                "template_render {} chars -> {} variables, output: {:?}",
                template.len(),
                variables.as_object().map(|m| m.len()).unwrap_or(0),
                output_path,
            )),
            pua_report: Some(tool_execution_report(
                "template_render",
                Some("template_rendered"),
            )),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::tool::ToolInput;
    use serde_json::json;
    use tempfile::TempDir;

    fn tool_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "test-tpl".to_string(),
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
    fn render_simple_variable() {
        let result =
            TemplateRenderTool::render_template("Hello, {{name}}!", &json!({"name": "World"}));
        assert_eq!(result, "Hello, World!");
    }

    #[test]
    fn render_multiple_variables() {
        let result = TemplateRenderTool::render_template(
            "{{greeting}}, {{name}}!",
            &json!({"greeting": "Hi", "name": "Alice"}),
        );
        assert_eq!(result, "Hi, Alice!");
    }

    #[test]
    fn render_with_default() {
        let result = TemplateRenderTool::render_template("Hello, {{name|Guest}}!", &json!({}));
        assert_eq!(result, "Hello, Guest!");
    }

    #[test]
    fn render_with_default_and_value() {
        let result = TemplateRenderTool::render_template(
            "Hello, {{name|Guest}}!",
            &json!({"name": "Alice"}),
        );
        assert_eq!(result, "Hello, Alice!");
    }

    #[test]
    fn render_if_true() {
        let result = TemplateRenderTool::render_template(
            "{{#if show}}visible{{/if}}",
            &json!({"show": true}),
        );
        assert_eq!(result, "visible");
    }

    #[test]
    fn render_if_false() {
        let result = TemplateRenderTool::render_template(
            "{{#if show}}visible{{/if}}",
            &json!({"show": false}),
        );
        assert_eq!(result, "");
    }

    #[test]
    fn render_if_missing_variable() {
        let result =
            TemplateRenderTool::render_template("{{#if nonexistent}}visible{{/if}}", &json!({}));
        assert_eq!(result, "");
    }

    #[test]
    fn render_each_basic() {
        let result = TemplateRenderTool::render_template(
            "{{#each items}}{{.}},{{/each}}",
            &json!({"items": ["a", "b", "c"]}),
        );
        assert_eq!(result, "a,b,c,");
    }

    #[test]
    fn render_each_with_object() {
        let result = TemplateRenderTool::render_template(
            "{{#each items}}{{.name}}: {{.value}}\n{{/each}}",
            &json!({"items": [
                {"name": "x", "value": 1},
                {"name": "y", "value": 2},
            ]}),
        );
        assert_eq!(result, "x: 1\ny: 2\n");
    }

    #[test]
    fn render_each_empty() {
        let result = TemplateRenderTool::render_template(
            "before{{#each items}}{{.}}{{/each}}after",
            &json!({"items": []}),
        );
        assert_eq!(result, "beforeafter");
    }

    #[test]
    fn render_nested_if_in_each() {
        let result = TemplateRenderTool::render_template(
            "{{#each items}}{{#if visible}}{{.name}}{{/if}}{{/each}}",
            &json!({"items": [
                {"name": "a", "visible": true},
                {"name": "b", "visible": false},
                {"name": "c", "visible": true},
            ]}),
        );
        assert_eq!(result, "ac");
    }

    #[test]
    fn render_missing_variable_removed() {
        let result = TemplateRenderTool::render_template("Hello {{name}}!", &json!({}));
        assert_eq!(result, "Hello !");
    }

    #[test]
    fn render_complex_template() {
        let result = TemplateRenderTool::render_template(
            "Report: {{title}}\n{{#if show_details}}\nDetails:\n{{#each items}}- {{.}}\n{{/each}}{{/if}}",
            &json!({
                "title": "Test Report",
                "show_details": true,
                "items": ["item1", "item2"],
            }),
        );
        assert!(result.contains("Test Report"));
        assert!(result.contains("item1"));
        assert!(result.contains("item2"));
    }

    #[test]
    fn template_render_requires_template() {
        let input = tool_input(json!({}));
        let tool = TemplateRenderTool;
        let result = tool.run(&input);
        assert!(result.is_err());
    }

    #[test]
    fn template_render_writes_to_file() {
        let tmp = TempDir::new().expect("temp dir");
        let out = tmp.path().join("output.txt");

        let input = ToolInput {
            task_id: "t1".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload: json!({
                "template": "Hello {{name}}!",
                "variables": {"name": "World"},
                "output_path": out.to_string_lossy(),
            }),
            allowed_base_dir: Some(tmp.path().to_path_buf()),
        };

        let tool = TemplateRenderTool;
        let output = tool.run(&input).expect("template_render should succeed");
        assert!(output.success);
        assert!(out.exists());
        let content = std::fs::read_to_string(&out).unwrap();
        assert_eq!(content, "Hello World!");
    }
}
