//! MCP (Model Context Protocol) compatibility layer.

use std::collections::HashSet;
use std::sync::{atomic::AtomicBool, Arc, Mutex};

use crate::acp::server::AcpServer;
use crate::agent::AgentRegistry;
use crate::tool::ToolRegistry;

mod handlers;
mod schema;
mod tools;

#[cfg(test)]
mod tests;

pub(crate) use schema::mcp_initialize_capabilities;
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

    /// Tracks whether the client has sent a valid `initialize` request.
    /// Used to enforce MCP two-phase initialization ordering — the server
    /// MUST reject most requests before `initialize` has been received.
    pub(crate) initialized: AtomicBool,
}

impl McpServer {
    /// Create a new MCP server instance (no ACP features).
    pub fn new(
        agent_registry: Arc<AgentRegistry>,
        tool_registry: Arc<ToolRegistry>,
        server_name: String,
        server_version: String,
    ) -> Self {
        Self::new_with_acp(
            agent_registry,
            tool_registry,
            server_name,
            server_version,
            None,
        )
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
            acp_server,
            initialized: AtomicBool::new(false),
        }
    }

    /// Get the skill registry if connected to an ACP server.
    pub fn skill_registry(
        &self,
    ) -> Option<&Arc<std::sync::RwLock<crate::orchestration::skill::SkillRegistry>>> {
        self.acp_server
            .as_ref()
            .map(|s| &s.orchestration_deps.skill_registry)
    }
}
