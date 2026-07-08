//! Protocol detection, error codes, and RBAC helpers.
//!
//! Extracted from `request.rs` to reduce the size of the main module.
//! Contains protocol-mode detection, MCP/ACP method classification,
//! error code definitions, and RBAC permission/principal helpers.

/// JSON-RPC standard error codes and ACP custom error codes.
///
/// Standard codes follow the JSON-RPC 2.0 specification.
/// Custom codes use the server-error range (-32000 to -32099).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcpErrorCode {
    /// The method does not exist / is not available (-32601).
    MethodNotFound = -32601,
    /// Invalid method parameter(s) (-32602).
    InvalidParams = -32602,
    /// Internal JSON-RPC error (-32603).
    InternalError = -32603,
    /// Authentication is required (-32001).
    AuthRequired = -32001,
}

use crate::acp::server::AcpServer;
use crate::protocol::access_mode::{request_dispatch_mode, RequestDispatchMode};

/// Read protocol mode from config.toml / runtime_config.
pub(super) fn get_protocol_mode(server: &AcpServer) -> RequestDispatchMode {
    // Try reading protocol_mode from runtime_config.
    request_dispatch_mode(server.runtime_config.protocol_mode.as_deref())
}

/// Returns true if the method belongs to the MCP protocol.
/// Standard MCP methods (initialize, tools/list, tools/call, etc.)
/// may be sent without the "mcp." prefix in MCP-only mode.
pub(super) fn is_mcp_request(method: &str) -> bool {
    method.starts_with("mcp.")
        || method == "mcp.initialize"
        || method == "initialize"
        || method == "notifications/initialized"
        || method.starts_with("tools/")
        || method.starts_with("resources/")
        || method.starts_with("prompts/")
        || method.starts_with("logging/")
        || method.starts_with("sampling/")
        || method.starts_with("completion/")
        || method == "ping"
}

/// Convert a standard MCP method name to its "mcp." prefixed form
/// if it isn't already prefixed. Used in Mcp dispatch mode so that
/// standard MCP clients (which send `initialize`, `tools/list`, etc.)
/// are routed to the ACP dispatch's `mcp.*` handler.
pub(super) fn normalize_mcp_method(method: &str) -> String {
    if method.starts_with("mcp.") {
        return method.to_string();
    }
    match method {
        "initialize" => "mcp.initialize".to_string(),
        "notifications/initialized" | "notifications_initialized" => {
            "mcp.notifications_initialized".to_string()
        }
        "ping" => "mcp.ping".to_string(),
        _ if method.starts_with("tools/") => format!("mcp.tools.{}", &method[6..]),
        _ if method.starts_with("resources/") => format!("mcp.resources.{}", &method[10..]),
        _ if method.starts_with("prompts/") => format!("mcp.prompts.{}", &method[8..]),
        _ if method.starts_with("logging/") => format!("mcp.logging.{}", &method[8..]),
        _ if method.starts_with("sampling/") => format!("mcp.sampling.{}", &method[9..]),
        _ if method.starts_with("completion/") => format!("mcp.completion.{}", &method[11..]),
        _ => method.to_string(),
    }
}

/// Returns true if the method belongs to the ACP/A2A protocol.
pub(super) fn is_acp_request(method: &str) -> bool {
    // Common ACP/A2A JSON-RPC methods. Sorted alphabetically for binary_search.
    const ACP_METHODS: &[&str] = &[
        "$/cancel_request",
        "action.check",
        "authenticate",
        "autotune.get",
        "autotune.reset",
        "autotune.status",
        "breaker.recovery",
        "breaker.reset",
        "breaker.status",
        "build.repro",
        "cache.clear",
        "capabilities.list",
        "chat",
        "checkpoint.list",
        "config.baseline",
        "config.reload",
        "conversation.checkpoint.create",
        "conversation.checkpoint.list",
        "conversation.checkpoint.prune",
        "conversation.rollback",
        "cost.status",
        "data.lifecycle",
        "debug.panel.get",
        "debug_panel.get",
        "error.contract",
        "governance.audit.recent",
        "governance.config.save",
        "governance.plan.get",
        "governance.plan.update",
        "governance.remediate",
        "governance.status",
        "hardness.status",
        "harness.status",
        "health",
        "health.check",
        "health.probes",
        "initialize",
        "knowledge.distill",
        "learning.guardrail",
        "learning.replay",
        "learning.summary",
        "lock.status",
        "logout",
        "maintenance.gc",
        "mcp.completion.complete",
        "mcp.initialize",
        "mcp.logging.setLevel",
        "mcp.notifications_initialized",
        "mcp.ping",
        "mcp.resources.list",
        "mcp.resources.read",
        "mcp.resources.subscribe",
        "mcp.sampling.createMessage",
        "mcp.tools.call",
        "mcp.tools.list",
        "metrics",
        "metrics.errors.summary",
        "metrics.get",
        "metrics.prometheus",
        "metrics.reset",
        "metrics.window.query",
        "models.list",
        "models/list",
        "observability.alerts",
        "optimization.peak",
        "phase",
        "phase.policy.replay",
        "phase.status",
        "primary_secondary.summary",
        "prompts.create",
        "prompts.delete",
        "prompts.get",
        "prompts.list",
        "prompts.search",
        "prompts.update",
        "provider.capabilities",
        "provider.catalog",
        "provider.configure",
        "provider.copilot_device_code",
        "provider.copilot_device_code_poll",
        "provider.list_models",
        "provider.status",
        "provider.test_completion",
        "provider.test_connection",
        "release.readiness",
        "rl.alignment.offline_eval",
        "runtime.features",
        "runtime.health",
        "runtime.restart",
        "runtime.self_model",
        "runtime.stability",
        "security.baseline",
        "selector.status",
        "session/cancel",
        "session/close",
        "session/list",
        "session/load",
        "session/new",
        "session/prompt",
        "session/request_permission",
        "session/resume",
        "session/set_config_option",
        "session/set_mode",
        "shutdown",
        "skill.create",
        "skill.disable",
        "skill.enable",
        "skill.import",
        "skill.list",
        "skill.list_imported",
        "skill.remove",
        "skill.update",
        "skill.version.list",
        "skill.version.rollback",
        "summary/primary_secondary",
        "task.execute",
        "task.plan",
        "tool.approve",
        "terminal/create",
        "terminal/kill",
        "terminal/output",
        "terminal/release",
        "terminal/wait_for_exit",
        "trace.get",
        "trace.metrics",
        "vector.clear",
        "workflow.ask",
        "workflow.clarify",
        "workflow.confirm",
        "workflow.consult",
        "workflow.execute",
        "workflow.generate",
        "workflow.generate_from_chat",
        "workflow.research",
        "workflow.run.cancel",
        "workflow.run.get",
        "workflow.run.list",
        "workflow.run.pause",
        "workflow.run.resume",
    ];
    ACP_METHODS.binary_search(&method).is_ok()
}

/// Map a JSON-RPC method name to an RBAC permission (BLUE56-D06).
pub(super) fn method_to_permission(method: &str) -> crate::governance::rbac::Permission {
    use crate::governance::rbac::Permission;
    if method.starts_with("admin.") || method == "shutdown" || method == "maintenance.gc" {
        Permission::Admin
    } else if method.starts_with("governance.") || method.starts_with("tenant.") {
        Permission::ManageUsers
    } else if method.starts_with("config.") || method.starts_with("autotune.") {
        Permission::ManageConfig
    } else if method.starts_with("session/")
        || method.starts_with("chat/")
        || method == "session/prompt"
    {
        Permission::Execute
    } else if method.starts_with("tool.") {
        Permission::ManageConfig
    } else if method.starts_with("metrics.")
        || method.starts_with("trace.")
        || method == "runtime.health"
    {
        Permission::Read
    } else {
        // Default: require Execute for unknown methods
        Permission::Execute
    }
}

/// Extract a Principal from a JSON-RPC request for RBAC checks (BLUE56-D06).
pub(super) fn request_to_principal(
    request: &crate::rpc_protocol::JsonRpcRequest,
) -> crate::governance::rbac::Principal {
    use crate::governance::rbac::Principal;
    let user_id = request
        .params
        .as_ref()
        .and_then(|p| p.get("user_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("anonymous")
        .to_string();
    let roles: Vec<&str> = request
        .params
        .as_ref()
        .and_then(|p| p.get("roles"))
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_else(|| vec!["user"]);
    let tenant_id = request
        .params
        .as_ref()
        .and_then(|p| p.get("tenant_id"))
        .and_then(|v| v.as_str());
    Principal::new(&user_id, roles, tenant_id)
}
