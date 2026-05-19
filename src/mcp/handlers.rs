use anyhow::Result;
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::acp::r#impl::request::{
    inject_platform_profiles_if_absent, record_tool_call_audit_with_protocol,
};
use crate::protocol::rpc_protocol::RequestTraceContext;
use crate::tool::ToolInput;

use super::tools::validate_required_arguments;
use super::{
    JsonRpcError, JsonRpcRequest, JsonRpcResponse, McpResource, McpServer, McpTool, MCP_VERSION,
};

/// Signals an invalid / missing parameter in an MCP request.
/// Dispatched as JSON-RPC INVALID_PARAMS (-32602).
#[derive(Debug)]
struct McpParamError(String);

impl std::fmt::Display for McpParamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for McpParamError {}

fn invalid_params(msg: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(McpParamError(msg.into()))
}

fn error_code_for(err: &anyhow::Error) -> i32 {
    if err.downcast_ref::<McpParamError>().is_some() {
        super::error_codes::INVALID_PARAMS
    } else {
        super::error_codes::INTERNAL_ERROR
    }
}

impl McpServer {
    fn resolve_prompt_lang(&self, request: &JsonRpcRequest) -> String {
        if let Some(lang) = request
            .params
            .as_ref()
            .and_then(|p| p.get("lang"))
            .and_then(|v| v.as_str())
        {
            return lang.to_string();
        }

        if let Some(acp) = &self.acp_server {
            return acp.runtime_config.i18n_default_language.clone();
        }

        "en".to_string()
    }

    pub async fn handle_request(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse> {
        let result = match request.method.as_str() {
            "initialize" => Ok(self.handle_initialize(&request).await),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(self.handle_list_tools(&request).await),
            "tools/call" => self.handle_call_tool(&request).await,
            "resources/list" => Ok(self.handle_list_resources(&request).await),
            "resources/read" => self.handle_read_resource(&request).await,
            "resources/subscribe" => Ok(json!({"meta": {}})),
            "prompts/list" => Ok(self.handle_list_prompts(&request).await),
            "prompts/get" => Ok(self.handle_get_prompt(&request).await),
            "agents/list" => Ok(self.handle_list_agents(&request).await),
            "notifications/initialized" => {
                // MCP notification — no response expected per JSON-RPC spec.
                // Zed's context_server client logs an error for id:null responses,
                // so we skip sending any response at all.
                info!("MCP: received notifications/initialized (no response sent)");
                return Ok(JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: None,
                    result: None,
                    error: None,
                });
            }
            "logging/setLevel" => {
                // Accept logging level changes silently.
                Ok(json!({}))
            }
            "completion/complete" => Ok(json!({"completion": []})),
            "sampling/createMessage" => {
                warn!("MCP: sampling/createMessage is not supported");
                return Ok(JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    result: None,
                    error: Some(JsonRpcError {
                        code: super::error_codes::METHOD_NOT_FOUND,
                        message: "sampling/createMessage is not supported".to_string(),
                        data: None,
                    }),
                    id: request.id,
                });
            }
            "models/list" => Ok(self.handle_list_models(&request).await),
            _ => {
                warn!("MCP: unknown method '{}'", request.method);
                let error_data =
                    inject_platform_profiles_if_absent(json!({}), "mcp.unknown_method");
                return Ok(JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    result: None,
                    error: Some(JsonRpcError {
                        code: super::error_codes::METHOD_NOT_FOUND,
                        message: format!("Unknown method: {}", request.method),
                        data: Some(error_data),
                    }),
                    id: request.id,
                });
            }
        };

        let (response_result, response_error) = match result {
            Ok(value) => {
                let value = inject_platform_profiles_if_absent(value, request.method.as_str());
                (Some(value), None)
            }
            Err(err) => {
                let error_data =
                    inject_platform_profiles_if_absent(json!({}), request.method.as_str());
                (
                    None,
                    Some(JsonRpcError {
                        code: error_code_for(&err),
                        message: err.to_string(),
                        data: Some(error_data),
                    }),
                )
            }
        };

        Ok(JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            result: response_result,
            error: response_error,
            id: request.id,
        })
    }

    async fn handle_initialize(&self, _request: &JsonRpcRequest) -> Value {
        json!({
            "protocolVersion": MCP_VERSION,
            "capabilities": {
                "resources": {},
                "tools": {},
                "prompts": {}
            },
            "serverInfo": {
                "name": self.server_info.name,
                "version": self.server_info.version,
            }
        })
    }

    async fn handle_list_tools(&self, _request: &JsonRpcRequest) -> Value {
        let mut tools = self
            .tool_registry
            .names()
            .into_iter()
            .map(crate::mcp::tools::tool_descriptor)
            .collect::<Vec<_>>();

        // Inject registered skills from ACP server (if available)
        if let Some(registry) = self.skill_registry() {
            if let Ok(guard) = registry.lock() {
                for descriptor in guard.list() {
                    tools.push(McpTool {
                        name: descriptor.name,
                        description: Some(descriptor.description),
                        input_schema: Some(descriptor.input_schema),
                    });
                }
            }
        }

        // Sort: tools first, then skills (alphabetically within groups)
        let tool_names: std::collections::HashSet<String> = self
            .tool_registry
            .names()
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        tools.sort_by(|a, b| {
            let a_is_tool = tool_names.contains(&a.name);
            let b_is_tool = tool_names.contains(&b.name);
            if a_is_tool != b_is_tool {
                // Tools before skills
                if a_is_tool {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                }
            } else {
                a.name.cmp(&b.name)
            }
        });

        let count = tools.len();
        info!("MCP: Listing {} tools/skills", count);
        json!({
            "tools": tools,
            "x_skills_available": self.skill_registry().is_some(),
        })
    }

    async fn handle_call_tool(&self, request: &JsonRpcRequest) -> Result<Value> {
        let params = request
            .params
            .as_ref()
            .ok_or_else(|| invalid_params("Missing parameters"))?;

        let tool_name = params["name"]
            .as_str()
            .ok_or_else(|| invalid_params("Missing tool name"))?
            .to_string();
        let tool_input = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));

        info!(
            "MCP: Calling tool '{}' with input: {:?}",
            tool_name, tool_input
        );

        // Step 0: Workflow and skill creation tools (require ACP server)
        if let Some(ref acp) = self.acp_server {
            match tool_name.as_str() {
                "workflow_execute" => {
                    let task = tool_input
                        .get("task")
                        .and_then(Value::as_str)
                        .ok_or_else(|| invalid_params("Missing required parameter: task"))?;
                    let params = json!({
                        "task": task,
                        "phase": tool_input.get("phase").and_then(Value::as_str),
                    });
                    let trace = RequestTraceContext {
                        trace_id: "mcp-call".to_string(),
                        span_id: "workflow-execute".to_string(),
                        method: tool_name.clone(),
                        request_id: "mcp-tool-call".to_string(),
                    };
                    crate::acp::r#impl::request::exec_pack::handle_workflow_execute(
                        acp, params, None, &trace,
                    )
                    .await?;
                    record_tool_call_audit_with_protocol(
                        &tool_name,
                        &tool_input,
                        true,
                        "workflow executed via mcp",
                        "mcp_stdio",
                    );
                    return Ok(json!({
                        "content": [{"type": "text", "text": format!("Workflow executed for task: {}", task)}],
                        "structuredContent": json!({"ok": true, "task": task}),
                    }));
                }
                "workflow_ask" => {
                    let task = tool_input
                        .get("task")
                        .and_then(Value::as_str)
                        .ok_or_else(|| invalid_params("Missing required parameter: task"))?;
                    let params = json!({
                        "task": task,
                        "auto_create_skills": tool_input.get("auto_create_skills").cloned().unwrap_or(json!(true)),
                        "auto_create_workflow": true,
                    });
                    let trace = RequestTraceContext {
                        trace_id: "mcp-call".to_string(),
                        span_id: "workflow-ask".to_string(),
                        method: tool_name.clone(),
                        request_id: "mcp-tool-call".to_string(),
                    };
                    crate::acp::r#impl::request::workflow_pack::handle_workflow_ask(
                        acp, params, None, &trace,
                    )
                    .await?;
                    record_tool_call_audit_with_protocol(
                        &tool_name,
                        &tool_input,
                        true,
                        "workflow.ask executed via mcp",
                        "mcp_stdio",
                    );
                    return Ok(json!({
                        "content": [{"type": "text", "text": format!("Workflow.ask completed for: {}", task)}],
                        "structuredContent": json!({"ok": true, "task": task}),
                    }));
                }
                "workflow_generate" => {
                    let task = tool_input
                        .get("task")
                        .and_then(Value::as_str)
                        .ok_or_else(|| invalid_params("Missing required parameter: task"))?;
                    let params = json!({"task": task});
                    let trace = RequestTraceContext {
                        trace_id: "mcp-call".to_string(),
                        span_id: "workflow-generate".to_string(),
                        method: tool_name.clone(),
                        request_id: "mcp-tool-call".to_string(),
                    };
                    crate::acp::r#impl::request::workflow_pack::handle_workflow_generate(
                        acp, params, None, &trace,
                    )
                    .await?;
                    record_tool_call_audit_with_protocol(
                        &tool_name,
                        &tool_input,
                        true,
                        "workflow.generate executed via mcp",
                        "mcp_stdio",
                    );
                    return Ok(json!({
                        "content": [{"type": "text", "text": format!("Workflow generated for: {}", task)}],
                        "structuredContent": json!({"ok": true, "task": task}),
                    }));
                }
                _ => {} // Fall through to tool_registry + skill_registry
            }
        }

        // Step 1: Try tool_registry first (existing behavior)
        if let Some(tool) = self.tool_registry.get(&tool_name) {
            validate_required_arguments(&tool_name, &tool_input)
                .map_err(|e| invalid_params(e.to_string()))?;
            let result = tool.run(&ToolInput {
                task_id: request
                    .id
                    .as_ref()
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "mcp-tool-call".to_string()),
                phase: "mcp".to_string(),
                agent_role: "tool".to_string(),
                objective: format!("Execute MCP tool '{}'", tool_name),
                constraints: None,
                evidence: None,
                payload: tool_input.clone(),
                allowed_base_dir: None,
            })?;

            info!("MCP: Tool '{}' returned: {:?}", tool_name, result);
            record_tool_call_audit_with_protocol(
                &tool_name,
                &tool_input,
                true,
                "tool executed via mcp",
                "mcp_stdio",
            );
            return Ok(json!({
                "content": [{
                    "type": "text",
                    "text": serde_json::to_string(&result)?
                }],
                "structuredContent": result,
                "isError": false,
            }));
        }

        // Step 2: Try skill registry fallback
        if let Some(registry) = self.skill_registry() {
            // Extract skill from lock, then drop the guard before async execution
            let skill_to_call = match registry.lock() {
                Ok(guard) => {
                    // Try exact name match first
                    if let Some(skill) = guard.get(&tool_name) {
                        // Clone while lock is held so skill is fully owned data
                        Some((tool_name.clone(), skill.clone()))
                    } else if let Some(best_match) =
                        guard.best_match_with_input(&tool_name, &tool_input)
                    {
                        guard
                            .get(&best_match)
                            .map(|skill| (best_match.clone(), skill.clone()))
                    } else {
                        None
                    }
                }
                Err(_) => None,
            };

            if let Some((resolved_name, skill)) = skill_to_call {
                info!(
                    "MCP: Calling skill '{}' with input: {:?}",
                    resolved_name, tool_input
                );
                let result = skill.execute(&tool_input).await?;

                record_tool_call_audit_with_protocol(
                    &resolved_name,
                    &tool_input,
                    true,
                    "skill executed via mcp",
                    "mcp_stdio",
                );

                info!("MCP: Skill '{}' returned: {:?}", resolved_name, result);

                let mut response = json!({
                    "content": [{"type": "text", "text": serde_json::to_string_pretty(&result)?}],
                    "structuredContent": result,
                    "isError": false,
                });
                if resolved_name != tool_name {
                    response["x_resolved_skill"] = json!(resolved_name);
                }
                return Ok(response);
            }
        }

        // Step 4: Not found — error
        warn!("MCP: Unknown tool or skill '{}'", tool_name);
        Err(invalid_params(format!(
            "Unknown tool or skill: {}",
            tool_name
        )))
    }

    async fn handle_list_resources(&self, _request: &JsonRpcRequest) -> Value {
        let resources = vec![
            McpResource {
                uri: "go-on://agents".to_string(),
                name: "Available Agents".to_string(),
                description: Some("List of deployed agents".to_string()),
                mime_type: "application/json".to_string(),
            },
            McpResource {
                uri: "go-on://tools".to_string(),
                name: "Available Tools".to_string(),
                description: Some("List of available tools".to_string()),
                mime_type: "application/json".to_string(),
            },
        ];

        json!({ "resources": resources })
    }

    async fn handle_read_resource(&self, request: &JsonRpcRequest) -> Result<Value> {
        let params = request
            .params
            .as_ref()
            .ok_or_else(|| invalid_params("Missing parameters"))?;

        let uri = params["uri"]
            .as_str()
            .ok_or_else(|| invalid_params("Missing URI"))?;

        match uri {
            "go-on://agents" => Ok(json!({
                "contents": [{
                    "uri": "go-on://agents",
                    "mimeType": "application/json",
                    "text": serde_json::to_string(&json!({"agents": self.agent_registry.names()}))?
                }]
            })),
            "go-on://tools" => Ok(json!({
                "contents": [{
                    "uri": "go-on://tools",
                    "mimeType": "application/json",
                    "text": serde_json::to_string(&json!({"tools": self.tool_registry.names()}))?
                }]
            })),
            _ => {
                warn!("MCP: unknown resource '{}'", uri);
                Err(invalid_params(format!("Unknown resource: {}", uri)))
            }
        }
    }

    async fn handle_list_agents(&self, _request: &JsonRpcRequest) -> Value {
        info!("MCP: Listing available agents from agent_registry");
        json!({ "agents": self.agent_registry.names() })
    }

    async fn handle_list_models(&self, _request: &JsonRpcRequest) -> Value {
        info!("MCP: Listing available models");
        let models = self
            .agent_registry
            .models()
            .into_iter()
            .map(|(agent, default_model, models)| {
                json!({
                    "agent": agent,
                    "default_model": default_model,
                    "models": models,
                })
            })
            .collect::<Vec<_>>();
        json!({ "models": models })
    }

    /// Handler for `prompts/list`.
    ///
    /// Returns available prompt templates. Currently supports:
    /// - system prompts derived from agent configurations
    /// - phase-specific task prompts
    ///
    /// This is a lightweight implementation that surfaces the agent system prompts
    /// as discoverable prompt templates.  Full template parameterisation is a
    /// future enhancement.
    async fn handle_list_prompts(&self, request: &JsonRpcRequest) -> Value {
        let lang = self.resolve_prompt_lang(request);
        let prompts = self.build_prompt_list(&lang);
        json!({ "prompts": prompts })
    }

    /// Handler for `prompts/get`.
    ///
    /// Returns a resolved prompt template by name.  Supports agent system prompts
    /// (prefixed with `agent://`) and built-in skill prompts (prefixed with `skill://`).
    async fn handle_get_prompt(&self, request: &JsonRpcRequest) -> Value {
        let name = request
            .params
            .as_ref()
            .and_then(|p| p.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let lang = self.resolve_prompt_lang(request);

        if let Some(resolved) = self.resolve_template_prompt(name, &lang, request.params.as_ref()) {
            return resolved;
        }

        // Try to resolve as an agent system prompt
        if let Some(messages) = self.resolve_agent_prompt(name) {
            return json!({
                "description": format!("Agent system prompt for '{}'", name),
                "messages": messages
            });
        }

        // Fallback: return a minimal placeholder prompt message so the client
        // can still function (rather than an error).
        json!({
            "description": format!("Prompt '{}' not found; returning default", name),
            "messages": [
                {
                    "role": "system",
                    "content": format!("You are a helpful assistant.  Context: {}", name)
                }
            ]
        })
    }

    /// Build a list of discoverable prompt templates from agent configurations
    /// and registered skills.
    fn build_prompt_list(&self, lang: &str) -> Vec<Value> {
        let mut prompts = Vec::new();

        // Prompt templates from ACP prompt manager
        if let Some(acp) = &self.acp_server {
            if let Ok(collection) = acp.prompt_manager.get_all_templates(lang) {
                for category in collection.categories {
                    for template in category.templates {
                        prompts.push(json!({
                            "name": format!("template://{}.{}", category.id, template.id),
                            "description": template.description,
                            "arguments": [
                                {
                                    "name": "input",
                                    "description": "Optional input for replacing {{input}} placeholder",
                                    "required": false
                                }
                            ]
                        }));
                    }
                }
            }
        }

        // Agent system prompts
        for name in self.agent_registry.names() {
            prompts.push(json!({
                "name": format!("agent://{}", name),
                "description": format!("System prompt for '{}' agent", name),
                "arguments": []
            }));
        }

        prompts
    }

    fn resolve_template_prompt(
        &self,
        name: &str,
        lang: &str,
        params: Option<&Value>,
    ) -> Option<Value> {
        let acp = self.acp_server.as_ref()?;

        let normalized = name
            .strip_prefix("template://")
            .unwrap_or(name)
            .trim()
            .trim_start_matches('/');

        let (cat_id, cat_name, tpl) = acp.prompt_manager.get_template(lang, normalized)?;

        let input = params
            .and_then(|p| p.get("arguments"))
            .and_then(|a| a.get("input"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();

        let content = if tpl.content.contains("{{input}}") {
            tpl.content.replace("{{input}}", input)
        } else if input.is_empty() {
            tpl.content
        } else {
            format!("{}\n\n{}", tpl.content, input)
        };

        Some(json!({
            "description": format!("Template prompt '{}.{}'", cat_id, tpl.id),
            "messages": [
                {
                    "role": "system",
                    "content": content
                }
            ],
            "template": {
                "category_id": cat_id,
                "category_name": cat_name,
                "id": tpl.id,
                "title": tpl.title,
            }
        }))
    }

    /// Resolve an agent prompt by name.
    /// Returns `None` if no matching agent is found.
    fn resolve_agent_prompt(&self, name: &str) -> Option<Vec<Value>> {
        let agent_name = name.strip_prefix("agent://").unwrap_or(name);
        // Check agent exists and resolve available models.
        let models = self.agent_registry.get(agent_name).map(|agent| {
            agent
                .available_models()
                .into_iter()
                .map(|m| m.id)
                .collect::<Vec<_>>()
                .join(", ")
        });

        let model_hint = models
            .filter(|m| !m.is_empty())
            .map(|m| format!(" Available models: {}.", m))
            .unwrap_or_default();

        Some(vec![
            json!({
                "role": "system",
                "content": format!(
                    "You are a '{}' agent providing AI assistance.{}",
                    agent_name, model_hint
                )
            }),
            json!({
                "role": "user",
                "content": "Hello!"
            }),
        ])
    }
}
