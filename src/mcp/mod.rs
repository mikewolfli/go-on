//! MCP (Model Context Protocol) compatibility layer.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::acp::server::AcpServer;
use crate::agent::AgentRegistry;
use crate::intelligence::token_cache::TokenMultiLevelCache;
use crate::tool::ToolRegistry;

mod handlers;
mod schema;
mod tools;

#[cfg(test)]
mod tests;

pub use schema::{
    JsonRpcError, JsonRpcRequest, JsonRpcResponse, McpCallToolResult, McpInitializeResult,
    McpListResourcesResult, McpListToolsResult, McpResource, McpTool, ServerInfo, JSONRPC_VERSION,
};
pub use tools::error_codes;

/// MCP Protocol Version — latest stable spec
pub const MCP_VERSION: &str = "2024-11-05";

/// All MCP protocol versions supported by this server, ordered from oldest
/// to newest. Used for version negotiation during `initialize`.
pub const SUPPORTED_MCP_VERSIONS: &[&str] = &["2024-11-05"];

/// MCP Server implementation — struct IS used via new/new_with_acp, serve, etc.
pub struct McpServer {
    pub(crate) agent_registry: Arc<AgentRegistry>,
    pub(crate) tool_registry: Arc<ToolRegistry>,
    pub(crate) server_info: ServerInfo,
    /// Optional reference to the ACP server, providing access to token cache,
    /// background tasks, response cache, vector store, autotune, observability,
    /// and shutdown notification.
    pub acp_server: Option<Arc<AcpServer>>,

    /// Current logging level, set via logging/setLevel.
    pub logging_level: Arc<Mutex<Option<String>>>,
    /// Request IDs flagged by `notifications/cancelled`.
    pub(crate) cancelled_requests: Arc<Mutex<HashSet<String>>>,
    /// Resource subscription tracking: resource URI → set of subscriber identifiers.
    pub(crate) resource_subscriptions: Arc<Mutex<HashMap<String, HashSet<String>>>>,
    /// Optional SSE broadcaster for pushing real-time notifications to
    /// connected SSE clients (resource changes, tool list changes, etc.).
    /// Set by `McpHttpServer` during construction.
    pub(crate) sse_broadcaster: Option<Arc<crate::protocol::mcp_server::SseBroadcaster>>,
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
            logging_level: Arc::new(Mutex::new(None)),
            cancelled_requests: Arc::new(Mutex::new(HashSet::new())),
            resource_subscriptions: Arc::new(Mutex::new(HashMap::new())),
            acp_server: None,
            sse_broadcaster: None,
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
            logging_level: Arc::new(Mutex::new(None)),
            cancelled_requests: Arc::new(Mutex::new(HashSet::new())),
            resource_subscriptions: Arc::new(Mutex::new(HashMap::new())),
            acp_server,
            sse_broadcaster: None,
        }
    }

    /// Attach an SSE broadcaster to this MCP server instance.
    /// Enables real-time push of resource-change and other subscription-based
    /// notifications to SSE-connected clients.
    pub fn with_sse_broadcaster(
        mut self,
        broadcaster: Arc<crate::protocol::mcp_server::SseBroadcaster>,
    ) -> Self {
        self.sse_broadcaster = Some(broadcaster);
        self
    }

    /// Get a reference to the token cache if available
    #[allow(dead_code)] // F-GAP-49 — planned wiring: memory/caching accessor
    pub fn token_cache(&self) -> Option<&Arc<TokenMultiLevelCache>> {
        self.acp_server
            .as_ref()
            .map(|s| &s.cache_deps.cache.token_cache)
    }

    /// Get the skill registry if connected to an ACP server.
    pub fn skill_registry(
        &self,
    ) -> Option<&Arc<std::sync::Mutex<crate::orchestration::skill::SkillRegistry>>> {
        self.acp_server
            .as_ref()
            .map(|s| &s.orchestration_deps.skill_registry)
    }
}
