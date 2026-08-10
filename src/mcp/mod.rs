//! MCP (Model Context Protocol) compatibility layer.

use std::collections::HashSet;
use std::sync::{atomic::AtomicBool, Arc, Mutex};

use crate::acp::server::AcpServer;
use crate::agent::AgentRegistry;
use crate::tool::ToolRegistry;

mod handlers;
mod schema;

pub mod client;

#[cfg(test)]
mod tests;

pub(crate) use schema::mcp_initialize_capabilities;
pub use schema::{
    JsonRpcError, JsonRpcRequest, JsonRpcResponse, McpCallToolResult, McpInitializeResult,
    McpListResourcesResult, McpListToolsResult, McpResource, McpTool, ServerInfo, JSONRPC_VERSION,
};

/// JSON-RPC error codes shared by the MCP layer.
pub mod error_codes {
    /// JSON-RPC Parse error
    pub const PARSE_ERROR: i32 = -32700;
    /// JSON-RPC Invalid Request
    pub const INVALID_REQUEST: i32 = -32600;
    /// JSON-RPC Method not found
    pub const METHOD_NOT_FOUND: i32 = -32601;
    /// JSON-RPC Invalid params
    pub const INVALID_PARAMS: i32 = -32602;
    /// JSON-RPC Internal error
    pub const INTERNAL_ERROR: i32 = -32603;
    /// Request cancelled by client notification.
    pub const REQUEST_CANCELLED: i32 = -32800;
    /// Request timed out before producing a result.
    pub const REQUEST_TIMEOUT: i32 = -32801;
    /// Server has not been initialized yet (MCP spec -32002).
    pub const SERVER_NOT_INITIALIZED: i32 = -32002;
}

/// MCP Protocol Version — latest stable spec
pub const MCP_VERSION: &str = "2024-11-05";

/// All MCP protocol versions supported by this server, ordered from oldest
/// to newest. Used for version negotiation during `initialize`.
pub const SUPPORTED_MCP_VERSIONS: &[&str] = &["2024-11-05"];

/// Negotiate the MCP protocol version against the client's requested version.
///
/// Picks the highest mutually supported version, falling back to
/// [`MCP_VERSION`] when the client sends none or a version we do not support.
/// Single implementation shared by the native MCP `initialize` handler
/// (`src/mcp/handlers.rs`) and the ACP-bridged `mcp.initialize`
/// (`src/acp/impl/request/protocol_pack/core.rs`) so the two entry points
/// cannot drift.
pub(crate) fn negotiate_mcp_version(client_version: &str) -> &'static str {
    SUPPORTED_MCP_VERSIONS
        .iter()
        .rev()
        .find(|v| **v == client_version)
        .copied()
        .unwrap_or(MCP_VERSION)
}

/// MCP Server implementation — struct IS used via new/new_with_acp, serve, etc.
pub struct McpServer {
    pub(crate) agent_registry: Arc<AgentRegistry>,
    pub(crate) tool_registry: Arc<ToolRegistry>,
    pub(crate) server_info: ServerInfo,
    /// Optional reference to the ACP server, providing access to token cache,
    /// background tasks, response cache, vector store, autotune, observability,
    /// and shutdown notification.
    pub acp_server: Option<Arc<AcpServer>>,

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
