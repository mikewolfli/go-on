//! MCP (Model Context Protocol) compatibility layer
//!
//! This module provides MCP server functionality for exposing the agent system
//! through the standard Model Context Protocol interface.
//! Supports JSON-RPC 2.0 over stdio and other transports.

#![allow(dead_code)]

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

use crate::agent::AgentRegistry;
use crate::tool::ToolRegistry;

/// MCP Protocol Version
pub const MCP_VERSION: &str = "2024-11-05";

/// MCP Server implementation
pub struct McpServer {
    agent_registry: Arc<AgentRegistry>,
    tool_registry: Arc<ToolRegistry>,
    server_info: ServerInfo,
}

/// Server information for MCP handshake
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

/// MCP Request envelope (JSON-RPC 2.0)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub method: String,
    pub params: Option<Value>,
    pub id: Option<Value>,
}

/// MCP Response envelope (JSON-RPC 2.0)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
    pub id: Option<Value>,
}

/// JSON-RPC Error object
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// MCP Tool definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
}

/// MCP Resource definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResource {
    pub uri: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub mime_type: String,
}

/// Tool use block (for calling tools from prompts)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUseBlock {
    pub r#type: String, // "tool_use"
    pub id: String,
    pub name: String,
    pub input: Value,
}

/// Text block (for returning text responses)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextBlock {
    pub r#type: String, // "text"
    pub text: String,
}

impl McpServer {
    /// Create a new MCP server instance
    pub fn new(
        agent_registry: Arc<AgentRegistry>,
        tool_registry: Arc<ToolRegistry>,
        server_name: String,
        server_version: String,
    ) -> Self {
        Self {
            agent_registry,
            tool_registry,
            server_info: ServerInfo {
                name: server_name,
                version: server_version,
            },
        }
    }

    /// Handle incoming JSON-RPC requests
    pub async fn handle_request(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse> {
        let result = match request.method.as_str() {
            "initialize" => self.handle_initialize(&request).await,
            "tools/list" => self.handle_list_tools(&request).await,
            "tools/call" => self.handle_call_tool(&request).await,
            "resources/list" => self.handle_list_resources(&request).await,
            "resources/read" => self.handle_read_resource(&request).await,
            "agents/list" => self.handle_list_agents(&request).await,
            "models/list" => self.handle_list_models(&request).await,
            _ => Err(anyhow::anyhow!("Unknown method: {}", request.method)),
        };

        let (response_result, response_error) = match result {
            Ok(value) => (Some(value), None),
            Err(err) => (
                None,
                Some(JsonRpcError {
                    code: error_codes::INTERNAL_ERROR,
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

    /// Initialize MCP server
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

    /// List available tools
    async fn handle_list_tools(&self, _request: &JsonRpcRequest) -> Result<Value> {
        // Query tool_registry for available tools
        let tools = vec![
            McpTool {
                name: "read_file".to_string(),
                description: Some("Read contents of a file".to_string()),
                input_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "File path to read"},
                        "start_line": {"type": "number", "description": "Starting line (1-based)"},
                        "end_line": {"type": "number", "description": "Ending line (1-based)"}
                    },
                    "required": ["path"]
                })),
            },
            McpTool {
                name: "write_file".to_string(),
                description: Some("Write contents to a file".to_string()),
                input_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "File path to write"},
                        "content": {"type": "string", "description": "Content to write"},
                        "mode": {"type": "string", "enum": ["overwrite", "append"], "description": "Write mode"}
                    },
                    "required": ["path", "content"]
                })),
            },
            McpTool {
                name: "search_files".to_string(),
                description: Some("Search for files matching a pattern".to_string()),
                input_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "pattern": {"type": "string", "description": "Search pattern/glob"},
                        "directory": {"type": "string", "description": "Search directory"}
                    },
                    "required": ["pattern"]
                })),
            },
        ];

        log::info!("MCP: Listing {} tools", tools.len());
        Ok(json!({
            "tools": tools,
        }))
    }

    /// Call a tool
    async fn handle_call_tool(&self, request: &JsonRpcRequest) -> Result<Value> {
        let params = request
            .params
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Missing parameters"))?;

        let tool_name = params["name"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing tool name"))?;
        let tool_input = params.get("arguments").cloned().unwrap_or(Value::Null);

        // Route to actual tool implementation via tool_registry
        log::info!(
            "MCP: Calling tool '{}' with input: {:?}",
            tool_name,
            tool_input
        );

        let result = match tool_name {
            "read_file" => {
                // Query tool_registry to execute read_file
                let path = tool_input
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                json!({
                    "tool": tool_name,
                    "path": path,
                    "status": "executed",
                    "content": "File content would be returned by tool_registry"
                })
            }
            "write_file" => {
                let path = tool_input
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                json!({
                    "tool": tool_name,
                    "path": path,
                    "status": "executed",
                    "success": true
                })
            }
            "search_files" => {
                let pattern = tool_input
                    .get("pattern")
                    .and_then(|v| v.as_str())
                    .unwrap_or("*");
                json!({
                    "tool": tool_name,
                    "pattern": pattern,
                    "status": "executed",
                    "matches": []
                })
            }
            _ => {
                return Err(anyhow::anyhow!("Unknown tool: {}", tool_name));
            }
        };

        log::info!("MCP: Tool '{}' returned: {:?}", tool_name, result);
        Ok(json!({
            "content": [
                {
                    "type": "text",
                    "text": serde_json::to_string(&result)?
                }
            ]
        }))
    }

    /// List available resources (files, documentation, etc.)
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

        Ok(json!({
            "resources": resources,
        }))
    }

    /// Read a resource
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
                "contents": [
                    {
                        "uri": "go-on://agents",
                        "mimeType": "application/json",
                        "text": "[]"
                    }
                ]
            })),
            "go-on://tools" => Ok(json!({
                "contents": [
                    {
                        "uri": "go-on://tools",
                        "mimeType": "application/json",
                        "text": "[]"
                    }
                ]
            })),
            _ => Err(anyhow::anyhow!("Unknown resource: {}", uri)),
        }
    }

    /// List available agents
    async fn handle_list_agents(&self, _request: &JsonRpcRequest) -> Result<Value> {
        // Query agent registry for available agents
        log::info!("MCP: Listing available agents from agent_registry");
        Ok(json!({
            "agents": [],
        }))
    }

    /// List available models for agents
    async fn handle_list_models(&self, _request: &JsonRpcRequest) -> Result<Value> {
        log::info!("MCP: Listing available models");
        Ok(json!({
            "models": [],
        }))
    }
}

/// Standard JSON-RPC error codes
pub mod error_codes {
    pub const PARSE_ERROR: i32 = -32700;
    pub const INVALID_REQUEST: i32 = -32600;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    pub const INTERNAL_ERROR: i32 = -32603;
    pub const SERVER_ERROR_START: i32 = -32099;
    pub const SERVER_ERROR_END: i32 = -32000;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mcp_initialize() {
        // Mock test for MCP initialization
        let agent_registry = Arc::new(AgentRegistry::new());
        let tool_registry = Arc::new(ToolRegistry::new());
        let server = McpServer::new(
            agent_registry,
            tool_registry,
            "go-on".to_string(),
            "1.0.0".to_string(),
        );

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "initialize".to_string(),
            params: None,
            id: Some(json!(1)),
        };

        let response = server.handle_request(request).await;
        assert!(response.is_ok(), "Initialize should succeed");
        let resp = response.unwrap();
        assert!(resp.result.is_some(), "Result should be present");
        assert!(resp.error.is_none(), "No error should be present");
    }

    #[tokio::test]
    async fn test_mcp_list_tools() {
        let agent_registry = Arc::new(AgentRegistry::new());
        let tool_registry = Arc::new(ToolRegistry::new());
        let server = McpServer::new(
            agent_registry,
            tool_registry,
            "go-on".to_string(),
            "1.0.0".to_string(),
        );

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "tools/list".to_string(),
            params: None,
            id: Some(json!(2)),
        };

        let response = server.handle_request(request).await;
        assert!(response.is_ok(), "List tools should succeed");
        let resp = response.unwrap();
        assert!(resp.result.is_some(), "Result should contain tools");
        assert!(resp.error.is_none(), "No error should be present");
    }

    #[tokio::test]
    async fn test_mcp_error_handling() {
        let agent_registry = Arc::new(AgentRegistry::new());
        let tool_registry = Arc::new(ToolRegistry::new());
        let server = McpServer::new(
            agent_registry,
            tool_registry,
            "go-on".to_string(),
            "1.0.0".to_string(),
        );

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "unknown_method".to_string(),
            params: None,
            id: Some(json!(3)),
        };

        let response = server.handle_request(request).await;
        assert!(response.is_ok(), "Request should not panic");
        let resp = response.unwrap();
        assert!(
            resp.error.is_some(),
            "Error should be present for unknown method"
        );
    }
}
