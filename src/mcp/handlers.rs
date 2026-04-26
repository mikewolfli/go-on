use anyhow::Result;
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::acp::r#impl::request::{
    inject_platform_profiles_if_absent, record_tool_call_audit_with_protocol,
};
use crate::tool::ToolInput;

use super::tools::{tool_descriptor, validate_required_arguments};
use super::{JsonRpcError, JsonRpcRequest, JsonRpcResponse, McpResource, McpServer, MCP_VERSION};

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
    pub async fn handle_request(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse> {
        let result = match request.method.as_str() {
            "initialize" => self.handle_initialize(&request).await,
            "tools/list" => self.handle_list_tools(&request).await,
            "tools/call" => self.handle_call_tool(&request).await,
            "resources/list" => self.handle_list_resources(&request).await,
            "resources/read" => self.handle_read_resource(&request).await,
            "agents/list" => self.handle_list_agents(&request).await,
            "models/list" => self.handle_list_models(&request).await,
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

    async fn handle_initialize(&self, _request: &JsonRpcRequest) -> Result<Value> {
        Ok(json!({
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
        }))
    }

    async fn handle_list_tools(&self, _request: &JsonRpcRequest) -> Result<Value> {
        let tools = self
            .tool_registry
            .names()
            .into_iter()
            .map(tool_descriptor)
            .collect::<Vec<_>>();

        info!("MCP: Listing {} tools", tools.len());
        Ok(json!({ "tools": tools }))
    }

    async fn handle_call_tool(&self, request: &JsonRpcRequest) -> Result<Value> {
        let params = request
            .params
            .as_ref()
            .ok_or_else(|| invalid_params("Missing parameters"))?;

        let tool_name = params["name"]
            .as_str()
            .ok_or_else(|| invalid_params("Missing tool name"))?;
        let tool_input = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));

        info!(
            "MCP: Calling tool '{}' with input: {:?}",
            tool_name, tool_input
        );

        let tool = self
            .tool_registry
            .get(tool_name)
            .ok_or_else(|| invalid_params(format!("Unknown tool: {}", tool_name)))?;
        validate_required_arguments(tool_name, &tool_input)?;
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
            tool_name,
            &tool_input,
            true,
            "tool executed via mcp",
            "mcp_stdio",
        );
        Ok(json!({
            "content": [{
                "type": "text",
                "text": serde_json::to_string(&result)?
            }],
            "structuredContent": result,
        }))
    }

    async fn handle_list_resources(&self, _request: &JsonRpcRequest) -> Result<Value> {
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

        Ok(json!({ "resources": resources }))
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

    async fn handle_list_agents(&self, _request: &JsonRpcRequest) -> Result<Value> {
        info!("MCP: Listing available agents from agent_registry");
        Ok(json!({ "agents": self.agent_registry.names() }))
    }

    async fn handle_list_models(&self, _request: &JsonRpcRequest) -> Result<Value> {
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
        Ok(json!({ "models": models }))
    }
}
