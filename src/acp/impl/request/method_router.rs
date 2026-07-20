//! MethodRouter — registration-based dispatch (B51-28).
//!
//! Replaces the monolithic `match method.as_ref() { ... }` block in
//! `handle_request` with a pluggable registration-based dispatch so
//! that new methods can be added without touching the giant dispatch match.

use crate::acp::r#impl::request::protocol_pack;
use crate::rpc_protocol::RequestTraceContext;
use anyhow::Result;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;

/// A handler for a single JSON-RPC method.
#[async_trait::async_trait]
pub trait MethodHandler: Send + Sync {
    async fn handle(
        &self,
        server: &crate::acp::server::AcpServer,
        params: Value,
        request_id: Option<Value>,
        trace: &RequestTraceContext,
    ) -> Result<()>;
}

/// Registration-based method router.
pub struct MethodRouter {
    handlers: HashMap<&'static str, Box<dyn MethodHandler>>,
}

impl MethodRouter {
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    /// Register a handler for a method name.
    pub fn register(&mut self, method: &'static str, handler: Box<dyn MethodHandler>) {
        self.handlers.insert(method, handler);
    }

    /// Dispatch to a registered handler.
    pub async fn dispatch(
        &self,
        method: &str,
        server: &crate::acp::server::AcpServer,
        params: Value,
        request_id: Option<Value>,
        trace: &RequestTraceContext,
    ) -> Option<Result<()>> {
        if let Some(handler) = self.handlers.get(method) {
            Some(handler.handle(server, params, request_id, trace).await)
        } else {
            None
        }
    }
}

// ── Handler structs and macro ──────────────────────────────────────────

/// Macro to generate handler structs that delegate to a protocol_pack function.
/// Use with_params variant when the payload function takes `(server, params)`.
macro_rules! make_handler {
    // With params
    ($name:ident, $payload_fn:path, params) => {
        struct $name;
        #[async_trait::async_trait]
        impl MethodHandler for $name {
            async fn handle(
                &self,
                server: &crate::acp::server::AcpServer,
                params: Value,
                request_id: Option<Value>,
                trace: &RequestTraceContext,
            ) -> Result<()> {
                let _ = trace;
                crate::acp::r#impl::io::respond(
                    server,
                    request_id,
                    $payload_fn(server, params).await,
                )
                .await
            }
        }
    };
    // Without params
    ($name:ident, $payload_fn:path) => {
        struct $name;
        #[async_trait::async_trait]
        impl MethodHandler for $name {
            async fn handle(
                &self,
                server: &crate::acp::server::AcpServer,
                _params: Value,
                request_id: Option<Value>,
                trace: &RequestTraceContext,
            ) -> Result<()> {
                let _ = trace;
                crate::acp::r#impl::io::respond(server, request_id, $payload_fn(server).await).await
            }
        }
    };
}

// ── Session lifecycle handlers ─────────────────────────────────────────
make_handler!(
    SessionNewHandler,
    protocol_pack::session_new_payload,
    params
);
make_handler!(
    SessionLoadHandler,
    protocol_pack::session_load_payload,
    params
);
make_handler!(
    SessionPromptHandler,
    protocol_pack::session_prompt_payload,
    params
);
make_handler!(
    SessionCancelHandler,
    protocol_pack::session_cancel_payload,
    params
);
make_handler!(
    SessionListHandler,
    protocol_pack::session_list_payload,
    params
);
make_handler!(
    SessionSetModeHandler,
    protocol_pack::session_set_mode_payload,
    params
);
make_handler!(
    SessionSetConfigOptionHandler,
    protocol_pack::session_set_config_option_payload,
    params
);
make_handler!(
    SessionResumeHandler,
    protocol_pack::session_resume_payload,
    params
);
make_handler!(
    SessionCloseHandler,
    protocol_pack::session_close_payload,
    params
);
make_handler!(
    SessionRequestPermissionHandler,
    protocol_pack::session_request_permission_payload,
    params
);
make_handler!(
    SessionDeleteHandler,
    protocol_pack::session_delete_payload,
    params
);
make_handler!(
    SessionConfigSetHandler,
    protocol_pack::session_config_set_payload,
    params
);
make_handler!(
    SessionConfigGetHandler,
    protocol_pack::session_config_get_payload,
    params
);

// ── Protocol handlers ──────────────────────────────────────────────────
make_handler!(InitializeHandler, protocol_pack::initialize_payload);
make_handler!(
    AuthenticateHandler,
    protocol_pack::authenticate_payload,
    params
);
make_handler!(LogoutHandler, protocol_pack::logout_payload, params);

// ── MCP bridge handlers ────────────────────────────────────────────────
make_handler!(McpInitializeHandler, protocol_pack::mcp_initialize_payload);
make_handler!(McpPingHandler, protocol_pack::mcp_ping_payload);
make_handler!(McpToolsListHandler, protocol_pack::mcp_tools_list_payload);
make_handler!(
    McpToolsCallHandler,
    protocol_pack::mcp_tools_call_payload,
    params
);
make_handler!(
    McpResourcesListHandler,
    protocol_pack::mcp_resources_list_payload
);
make_handler!(
    McpResourcesReadHandler,
    protocol_pack::mcp_resources_read_payload,
    params
);
make_handler!(
    McpResourcesSubscribeHandler,
    protocol_pack::mcp_resources_subscribe_payload,
    params
);
make_handler!(
    McpLoggingSetLevelHandler,
    protocol_pack::mcp_logging_set_level_payload,
    params
);
make_handler!(
    McpCompletionCompleteHandler,
    protocol_pack::mcp_completion_complete_payload,
    params
);
make_handler!(
    McpSamplingCreateMessageHandler,
    protocol_pack::mcp_sampling_create_message_payload,
    params
);

// ── Terminal handlers ──────────────────────────────────────────────────
make_handler!(
    TerminalCreateHandler,
    protocol_pack::terminal_create_payload,
    params
);
make_handler!(
    TerminalOutputHandler,
    protocol_pack::terminal_output_payload,
    params
);
make_handler!(
    TerminalWaitForExitHandler,
    protocol_pack::terminal_wait_for_exit_payload,
    params
);

// TerminalReleaseHandler uses dispatch_to_client because handle_terminal_release returns DispatchOutput
struct TerminalReleaseHandler;
#[async_trait::async_trait]
impl MethodHandler for TerminalReleaseHandler {
    async fn handle(
        &self,
        server: &crate::acp::server::AcpServer,
        params: Value,
        request_id: Option<Value>,
        _trace: &RequestTraceContext,
    ) -> Result<()> {
        super::dispatch_to_client(
            server,
            request_id,
            protocol_pack::handle_terminal_release(server, params).await,
        )
        .await
    }
}

// TerminalKillHandler uses dispatch_to_client because handle_terminal_kill returns DispatchOutput
struct TerminalKillHandler;
#[async_trait::async_trait]
impl MethodHandler for TerminalKillHandler {
    async fn handle(
        &self,
        server: &crate::acp::server::AcpServer,
        params: Value,
        request_id: Option<Value>,
        _trace: &RequestTraceContext,
    ) -> Result<()> {
        super::dispatch_to_client(
            server,
            request_id,
            protocol_pack::handle_terminal_kill(server, params).await,
        )
        .await
    }
}

// ── Skill handlers ─────────────────────────────────────────────────────
make_handler!(SkillListHandler, protocol_pack::skill_list_imported_payload);
make_handler!(
    SkillListImportedHandler,
    protocol_pack::skill_list_imported_payload
);
make_handler!(
    SkillImportHandler,
    protocol_pack::skill_import_payload,
    params
);
make_handler!(
    SkillCreateHandler,
    protocol_pack::skill_create_payload,
    params
);
make_handler!(
    SkillRemoveHandler,
    protocol_pack::skill_remove_payload,
    params
);

// ── Tool handlers ──────────────────────────────────────────────────────

// Custom handler for tools/list
struct ToolsListHandler;
#[async_trait::async_trait]
impl MethodHandler for ToolsListHandler {
    async fn handle(
        &self,
        server: &crate::acp::server::AcpServer,
        _params: Value,
        request_id: Option<Value>,
        _trace: &RequestTraceContext,
    ) -> Result<()> {
        crate::acp::r#impl::io::respond(
            server,
            request_id,
            protocol_pack::tools_list_payload(server).await,
        )
        .await
    }
}

// Custom handler for tools/call
struct ToolsCallHandler;
#[async_trait::async_trait]
impl MethodHandler for ToolsCallHandler {
    async fn handle(
        &self,
        server: &crate::acp::server::AcpServer,
        params: Value,
        request_id: Option<Value>,
        _trace: &RequestTraceContext,
    ) -> Result<()> {
        crate::acp::r#impl::io::respond(
            server,
            request_id,
            protocol_pack::tools_call_payload(server, params).await,
        )
        .await
    }
}

// ── Global router singleton ────────────────────────────────────────────

static GLOBAL_ROUTER: OnceLock<MethodRouter> = OnceLock::new();

/// Get or initialize the global method router.
pub fn global_router() -> &'static MethodRouter {
    GLOBAL_ROUTER.get_or_init(|| {
        let mut router = MethodRouter::new();
        // Session lifecycle handlers
        router.register("session/new", Box::new(SessionNewHandler));
        router.register("session/load", Box::new(SessionLoadHandler));
        router.register("session/prompt", Box::new(SessionPromptHandler));
        router.register("session/cancel", Box::new(SessionCancelHandler));
        router.register("session/list", Box::new(SessionListHandler));
        router.register("session/set_mode", Box::new(SessionSetModeHandler));
        router.register(
            "session/set_config_option",
            Box::new(SessionSetConfigOptionHandler),
        );
        router.register("session/delete", Box::new(SessionDeleteHandler));
        router.register("session/config/set", Box::new(SessionConfigSetHandler));
        router.register("session/config/get", Box::new(SessionConfigGetHandler));
        router.register("session/resume", Box::new(SessionResumeHandler));
        router.register("session/close", Box::new(SessionCloseHandler));
        router.register(
            "session/request_permission",
            Box::new(SessionRequestPermissionHandler),
        );
        // Protocol handlers
        router.register("initialize", Box::new(InitializeHandler));
        router.register("authenticate", Box::new(AuthenticateHandler));
        router.register("logout", Box::new(LogoutHandler));
        // Tool handlers
        router.register("tools/list", Box::new(ToolsListHandler));
        router.register("tools/call", Box::new(ToolsCallHandler));
        // MCP bridge handlers
        router.register("mcp.initialize", Box::new(McpInitializeHandler));
        router.register("mcp.ping", Box::new(McpPingHandler));
        router.register("mcp.tools.list", Box::new(McpToolsListHandler));
        router.register("mcp.tools.call", Box::new(McpToolsCallHandler));
        router.register("mcp.resources.list", Box::new(McpResourcesListHandler));
        router.register("mcp.resources.read", Box::new(McpResourcesReadHandler));
        router.register(
            "mcp.resources.subscribe",
            Box::new(McpResourcesSubscribeHandler),
        );
        router.register("mcp.logging.setLevel", Box::new(McpLoggingSetLevelHandler));
        router.register(
            "mcp.completion.complete",
            Box::new(McpCompletionCompleteHandler),
        );
        router.register(
            "mcp.sampling.createMessage",
            Box::new(McpSamplingCreateMessageHandler),
        );
        // Terminal handlers
        router.register("terminal/create", Box::new(TerminalCreateHandler));
        router.register("terminal/output", Box::new(TerminalOutputHandler));
        router.register("terminal/release", Box::new(TerminalReleaseHandler));
        router.register("terminal/kill", Box::new(TerminalKillHandler));
        router.register(
            "terminal/wait_for_exit",
            Box::new(TerminalWaitForExitHandler),
        );
        // Skill handlers
        router.register("skill.list", Box::new(SkillListHandler));
        router.register("skill.list_imported", Box::new(SkillListImportedHandler));
        router.register("skill.import", Box::new(SkillImportHandler));
        router.register("skill.create", Box::new(SkillCreateHandler));
        router.register("skill.remove", Box::new(SkillRemoveHandler));
        router
    })
}
