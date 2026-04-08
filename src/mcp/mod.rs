//! MCP (Model Context Protocol) compatibility layer.

#![allow(dead_code)]

use std::sync::Arc;

use crate::agent::AgentRegistry;
use crate::tool::ToolRegistry;

mod handlers;
mod schema;
mod tools;

#[cfg(test)]
mod tests;

#[allow(unused_imports)]
pub use schema::{
    JsonRpcError, JsonRpcRequest, JsonRpcResponse, McpResource, McpTool, ServerInfo, TextBlock,
    ToolUseBlock,
};
pub use tools::error_codes;

/// MCP Protocol Version
pub const MCP_VERSION: &str = "2024-11-05";

/// MCP Server implementation
pub struct McpServer {
    pub(crate) agent_registry: Arc<AgentRegistry>,
    pub(crate) tool_registry: Arc<ToolRegistry>,
    pub(crate) server_info: ServerInfo,
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
}
