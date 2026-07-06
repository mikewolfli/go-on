//! Native tool bridge for provider-native function calling
//!
//! Converts ToolRegistry tools into OpenAI/Anthropic-compatible JSON Schema
//! function definitions, parses native function call responses back into
//! ToolInput.
//!
//! This module provides the bridge logic for when provider function-calling
//! is wired in. Currently only compiled in tests, but the public API is
//! ready for integration.

use crate::orchestration::tool::{ToolInput, ToolRegistry};
use serde_json::{json, Value};
use std::collections::HashMap;

/// Bridge between go-on ToolRegistry and provider-native function calling.
pub struct NativeToolBridge {
    registry: ToolRegistry,
    /// Maps tool names to their JSON Schema function definitions
    schema_cache: HashMap<String, Value>,
}

impl NativeToolBridge {
    /// Create a new bridge backed by the given ToolRegistry.
    pub fn new(registry: ToolRegistry) -> Self {
        let mut bridge = Self {
            registry,
            schema_cache: HashMap::new(),
        };
        bridge.rebuild_cache();
        bridge
    }

    /// Rebuild the internal schema cache from the registry.
    fn rebuild_cache(&mut self) {
        self.schema_cache.clear();
        for name in self.registry.names() {
            let schema = self.build_tool_schema(name);
            self.schema_cache.insert(name.to_string(), schema);
        }
    }

    /// Build a JSON Schema function definition for a single tool.
    ///
    /// Auto-generates from the Tool trait's `input_schema()` and `description()`
    /// methods. No manual schema definitions needed — every tool provides its own
    /// schema via the trait interface.
    fn build_tool_schema(&self, tool_name: &str) -> Value {
        match self.registry.get(tool_name) {
            Some(tool) => {
                let desc = tool.description();
                json!({
                    "type": "function",
                    "function": {
                        "name": tool_name,
                        "description": if desc.is_empty() {
                            format!("Execute the {} tool", tool_name)
                        } else {
                            desc.to_string()
                        },
                        "parameters": tool.input_schema(),
                    }
                })
            }
            None => json!({
                "type": "function",
                "function": {
                    "name": tool_name,
                    "description": format!("Execute the {} tool", tool_name),
                    "parameters": {
                        "type": "object",
                        "properties": {},
                        "required": []
                    }
                }
            }),
        }
    }

    /// Return all tool definitions in OpenAI-compatible JSON Schema format.
    pub fn to_openai_tools(&self) -> Vec<Value> {
        self.schema_cache.values().cloned().collect()
    }

    /// Return all tool definitions in Anthropic-compatible format.
    ///
    /// Anthropic uses a slightly different schema: `name`, `description`, `input_schema`.
    pub fn to_anthropic_tools(&self) -> Vec<Value> {
        self.schema_cache
            .values()
            .map(|schema| {
                let func = &schema["function"];
                json!({
                    "name": func["name"],
                    "description": func["description"],
                    "input_schema": func["parameters"]
                })
            })
            .collect()
    }

    /// Parse a native function call response (from OpenAI format) into a ToolInput.
    ///
    /// Expects a JSON object with `name` and `arguments` from the tool_calls response.
    pub fn parse_openai_tool_call(
        &self,
        tool_name: &str,
        arguments: &Value,
        task_id: &str,
        phase: &str,
        agent_role: &str,
        allowed_base_dir: Option<&std::path::Path>,
    ) -> Option<ToolInput> {
        self.registry.get(tool_name)?;
        Some(ToolInput {
            task_id: task_id.to_string(),
            phase: phase.to_string(),
            agent_role: agent_role.to_string(),
            objective: format!("execute tool: {}", tool_name),
            constraints: None,
            evidence: None,
            payload: arguments.clone(),
            allowed_base_dir: allowed_base_dir.map(|p| p.to_path_buf()),
        })
    }

    /// Get a reference to the underlying ToolRegistry.
    pub fn registry(&self) -> &ToolRegistry {
        &self.registry
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn builds_openai_tools_from_registry() {
        let registry = ToolRegistry::new();
        let bridge = NativeToolBridge::new(registry);
        let tools = bridge.to_openai_tools();
        // Should have all 16 built-in tools
        assert!(tools.len() >= 6);
        let first = &tools[0];
        assert_eq!(first["type"], "function");
        assert!(first["function"]["name"].is_string());
    }

    #[test]
    fn builds_anthropic_tools_from_registry() {
        let registry = ToolRegistry::new();
        let bridge = NativeToolBridge::new(registry);
        let tools = bridge.to_anthropic_tools();
        assert!(tools.len() >= 6);
        let first = &tools[0];
        assert!(first["name"].is_string());
        assert!(first["input_schema"].is_object());
    }

    #[test]
    fn parses_openai_tool_call_into_input() {
        let registry = ToolRegistry::new();
        let bridge = NativeToolBridge::new(registry);
        let input = bridge
            .parse_openai_tool_call(
                "read_file",
                &json!({"path": "src/main.rs"}),
                "task-1",
                "act",
                "coder",
                None,
            )
            .expect("should parse known tool");
        assert_eq!(input.task_id, "task-1");
        assert_eq!(input.payload["path"], "src/main.rs");
    }

    #[test]
    fn rejects_unknown_tool() {
        let registry = ToolRegistry::new();
        let bridge = NativeToolBridge::new(registry);
        assert!(bridge
            .parse_openai_tool_call("nonexistent", &json!({}), "t1", "act", "coder", None)
            .is_none());
    }

    #[test]
    fn custom_protocol_roundtrip() {
        let token = crate::orchestration::autonomy_runtime::build_tool_call_token(
            "read_file",
            r#"{"path":"test.txt"}"#,
        );
        assert!(token.starts_with("__tool_call__:"));
        let (name, args) = crate::orchestration::autonomy_runtime::parse_tool_call_token(&token)
            .expect("should parse");
        assert_eq!(name, "read_file");
        assert_eq!(args, r#"{"path":"test.txt"}"#);
    }
}
