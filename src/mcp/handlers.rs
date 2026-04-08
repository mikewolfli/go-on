use anyhow::Result;
use serde_json::{json, Value};
use tracing::{info, warn};

use crate::tool::ToolInput;

use super::tools::{tool_descriptor, validate_required_arguments};
use super::{JsonRpcError, JsonRpcRequest, JsonRpcResponse, McpResource, McpServer, MCP_VERSION};

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
                Err(anyhow::anyhow!("Unknown method: {}", request.method))
            }
        };

        let (response_result, response_error) = match result {
            Ok(value) => (Some(value), None),
            Err(err) => (
                None,
                Some(JsonRpcError {
                    code: super::error_codes::INTERNAL_ERROR,
                    message: err.to_string(),
                    data: None,
                }),
            ),
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
            .ok_or_else(|| anyhow::anyhow!("Missing parameters"))?;

        let tool_name = params["name"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing tool name"))?;
        let tool_input = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));

        info!("MCP: Calling tool '{}' with input: {:?}", tool_name, tool_input);

        let tool = self
            .tool_registry
            .get(tool_name)
            .ok_or_else(|| anyhow::anyhow!("Unknown tool: {}", tool_name))?;
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
        })?;

        info!("MCP: Tool '{}' returned: {:?}", tool_name, result);
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
            .ok_or_else(|| anyhow::anyhow!("Missing parameters"))?;

        let uri = params["uri"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing URI"))?;

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
                Err(anyhow::anyhow!("Unknown resource: {}", uri))
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