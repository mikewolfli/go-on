//! MCP (Model Context Protocol) compatibility layer.

use std::sync::Arc;

use crate::acp::server::AcpServer;
use crate::agent::AgentRegistry;
use crate::intelligence::token_cache::TokenMultiLevelCache;
use crate::tool::ToolRegistry;

mod handlers;
mod schema;
mod tools;

#[cfg(test)]
mod tests;

pub use schema::{JsonRpcError, JsonRpcRequest, JsonRpcResponse, McpResource, McpTool, ServerInfo};
pub use tools::error_codes;

/// MCP Protocol Version
pub const MCP_VERSION: &str = "2024-11-05";

/// MCP Server implementation
#[allow(dead_code)] // F-GAP-10 — planned wiring: multi-channel transport
pub struct McpServer {
    pub(crate) agent_registry: Arc<AgentRegistry>,
    pub(crate) tool_registry: Arc<ToolRegistry>,
    pub(crate) server_info: ServerInfo,
    /// Optional reference to the ACP server, providing access to token cache,
    /// background tasks, response cache, vector store, autotune, observability,
    /// and shutdown notification.
    pub acp_server: Option<Arc<AcpServer>>,
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
            acp_server: None,
        }
    }

    /// Create a new MCP server instance with an optional AcpServer reference
    pub fn new_with_acp(
        agent_registry: Arc<AgentRegistry>,
        tool_registry: Arc<ToolRegistry>,
        server_name: String,
        server_version: String,
        acp_server: Option<Arc<AcpServer>>,
    ) -> Self {
        Self {
            agent_registry,
            tool_registry,
            server_info: ServerInfo {
                name: server_name,
                version: server_version,
            },
            acp_server,
        }
    }

    /// Get a reference to the token cache if available
    #[allow(dead_code)] // F-GAP-09 — planned wiring: memory/caching accessor
    pub fn token_cache(&self) -> Option<&Arc<TokenMultiLevelCache>> {
        self.acp_server.as_ref().map(|s| &s.cache.token_cache)
    }
}
