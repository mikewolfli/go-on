// Request dispatch for the session method family.
//
// Extracted from the single dispatch table in `request/mod.rs` (module split
// M0.4): each family matches the normalized method name exactly as the
// original `handle_request` match did, so dispatch behavior is unchanged.
// Session-family dispatch: ACP session lifecycle, authentication,
// protocol-level notifications, the MCP bridge, and terminal methods.
use anyhow::Result;
use serde_json::Value;

use crate::acp::server::AcpServer;
use crate::rpc_protocol::{JsonRpcRequest, RequestTraceContext};

use super::protocol_pack;
use super::{dispatch_to_client, DispatchOutput};

pub(crate) async fn dispatch_session(
    server: &AcpServer,
    request: JsonRpcRequest,
    request_id: Option<Value>,
    http_headers: Option<&str>,
    _trace: &RequestTraceContext,
    method: &str,
) -> Result<()> {
    match method {
        "initialize" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::initialize_payload(server, &request.params).await,
            )
            .await
        }
        // Standard ACP session lifecycle methods
        "session/new" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::session_new_payload(
                    server,
                    request.params.clone().unwrap_or_default(),
                )
                .await,
            )
            .await
        }
        "session/load" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::session_load_payload(server, request.params.unwrap_or_default())
                    .await,
            )
            .await
        }
        "session/prompt" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::session_prompt_payload(server, request.params.unwrap_or_default())
                    .await,
            )
            .await
        }
        "session/cancel" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::session_cancel_payload(server, request.params.unwrap_or_default())
                    .await,
            )
            .await
        }
        "session/list" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::session_list_payload(server, request.params.unwrap_or_default())
                    .await,
            )
            .await
        }
        "session/set_mode" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::session_set_mode_payload(server, request.params.unwrap_or_default())
                    .await,
            )
            .await
        }
        "session/set_config_option" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::session_set_config_option_payload(
                    server,
                    request.params.unwrap_or_default(),
                )
                .await,
            )
            .await
        }
        "session/resume" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::session_resume_payload(server, request.params.unwrap_or_default())
                    .await,
            )
            .await
        }
        "session/close" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::session_close_payload(server, request.params.unwrap_or_default())
                    .await,
            )
            .await
        }
        "session/request_permission" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::session_request_permission_payload(
                    server,
                    request.params.unwrap_or_default(),
                )
                .await,
            )
            .await
        }
        // Former MethodRouter-only session methods (kept reachable in
        // Auto mode; ACP mode still gates them via is_acp_request).
        "session/delete" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::session_delete_payload(server, request.params.unwrap_or_default())
                    .await,
            )
            .await
        }
        "session/config/set" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::session_config_set_payload(
                    server,
                    request.params.unwrap_or_default(),
                )
                .await,
            )
            .await
        }
        "session/config/get" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::session_config_get_payload(
                    server,
                    request.params.unwrap_or_default(),
                )
                .await,
            )
            .await
        }
        "session/config/favorite/toggle" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::session_config_favorite_toggle_payload(
                    server,
                    request.params.unwrap_or_default(),
                )
                .await,
            )
            .await
        }
        // Standard ACP authentication methods
        "authenticate" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::authenticate_payload(server, request.params.unwrap_or_default())
                    .await,
            )
            .await
        }
        "logout" => {
            // HTTP clients authenticate via the `Authorization` header
            // (not params), so pull the presented token from the headers
            // when params don't carry `bearer_token` — otherwise the
            // revocation below would silently no-op for header auth.
            let mut params = request.params.unwrap_or_default();
            if params
                .get("bearer_token")
                .is_none_or(|v| v.as_str().is_none_or(|s| s.is_empty()))
            {
                if let Some(headers) = http_headers {
                    use crate::acp::r#impl::runtime::protocol::extract_header_values;
                    if let Some(auth) = extract_header_values(headers, "authorization")
                        .into_iter()
                        .next()
                    {
                        let (scheme, rest) = auth
                            .split_once(char::is_whitespace)
                            .unwrap_or(("", auth.as_str()));
                        if scheme.eq_ignore_ascii_case("bearer") && !rest.trim().is_empty() {
                            params["bearer_token"] =
                                serde_json::Value::String(rest.trim().to_string());
                        }
                    }
                }
            }
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::logout_payload(server, params).await,
            )
            .await
        }
        // Protocol-level notifications
        "$/cancel_request" => {
            // $/cancel_request is a notification per JSON-RPC spec — no
            // response. Mark the target request id in the shared
            // cancelled-request registry so the in-flight request's
            // token loops (run_agent_collecting / autonomy loop) abort
            // early; the mark is cleared when the target request
            // completes. Previously this branch only logged.
            let target_id = request
                .params
                .as_ref()
                .and_then(|p| p.get("id"))
                .map(crate::rpc_protocol::value_to_id)
                .unwrap_or_else(|| "unknown".to_string());
            protocol_pack::mark_acp_request_cancelled(&target_id);
            tracing::info!(
                target: "acp::protocol_pack",
                target_request = %target_id,
                "$/cancel_request: marked request {} for cancellation",
                target_id
            );
            dispatch_to_client(server, request_id, Ok(DispatchOutput::silent())).await
        }
        // MCP methods bridged through ACP dispatch
        "mcp.client.connect" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                crate::acp::r#impl::request::mcp_client_pack::mcp_client_connect_payload(
                    request.params.unwrap_or_default(),
                )
                .await,
            )
            .await
        }
        "mcp.client.list" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                crate::acp::r#impl::request::mcp_client_pack::mcp_client_list_payload().await,
            )
            .await
        }
        "mcp.client.call" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                crate::acp::r#impl::request::mcp_client_pack::mcp_client_call_payload(
                    request.params.unwrap_or_default(),
                )
                .await,
            )
            .await
        }
        "mcp.initialize" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::mcp_initialize_payload(server, &request.params).await,
            )
            .await
        }
        "mcp.notifications_initialized" => {
            // MCP notification — no response expected per JSON-RPC spec
            dispatch_to_client(server, request_id, Ok(DispatchOutput::silent())).await
        }
        "mcp.notifications_cancelled" => {
            // MCP notification — no response expected per JSON-RPC
            // spec. Mirror the native MCP arm (handlers.rs
            // `notifications/cancelled`): mark the target request id
            // so an in-flight request's loops abort early. The bridge
            // arm holds only `&AcpServer` (no McpServer reference), so
            // it marks the same shared cancelled-request registry the
            // ACP `$/cancel_request` arm uses and the chat session
            // checks — the closest honest cancellation hook here.
            match request
                .params
                .as_ref()
                .and_then(|p| p.get("requestId"))
                .map(crate::rpc_protocol::value_to_id)
            {
                Some(target_id) => {
                    protocol_pack::mark_acp_request_cancelled(&target_id);
                    tracing::info!(
                        target: "acp::protocol_pack",
                        target_request = %target_id,
                        "mcp.notifications_cancelled: marked request {} for cancellation",
                        target_id
                    );
                }
                None => {
                    tracing::warn!("mcp.notifications_cancelled: missing requestId");
                }
            }
            dispatch_to_client(server, request_id, Ok(DispatchOutput::silent())).await
        }
        "mcp.ping" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::mcp_ping_payload(server).await,
            )
            .await
        }
        "mcp.tools.list" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::mcp_tools_list_payload(server).await,
            )
            .await
        }
        "mcp.tools.call" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::mcp_tools_call_payload(server, request.params.unwrap_or_default())
                    .await,
            )
            .await
        }
        "mcp.resources.list" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::mcp_resources_list_payload(server).await,
            )
            .await
        }
        "mcp.resources.read" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::mcp_resources_read_payload(
                    server,
                    request.params.unwrap_or_default(),
                )
                .await,
            )
            .await
        }
        "mcp.resources.subscribe" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::mcp_resources_subscribe_payload(
                    server,
                    request.params.unwrap_or_default(),
                )
                .await,
            )
            .await
        }
        "mcp.logging.setLevel" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::mcp_logging_set_level_payload(
                    server,
                    request.params.unwrap_or_default(),
                )
                .await,
            )
            .await
        }
        "mcp.completion.complete" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::mcp_completion_complete_payload(
                    server,
                    request.params.unwrap_or_default(),
                )
                .await,
            )
            .await
        }
        "mcp.sampling.createMessage" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::mcp_sampling_create_message_payload(
                    server,
                    request.params.unwrap_or_default(),
                )
                .await,
            )
            .await
        }
        "mcp.prompts.list" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::mcp_prompts_list_payload(server, request.params.unwrap_or_default())
                    .await,
            )
            .await
        }
        "mcp.prompts.get" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::mcp_prompts_get_payload(server, request.params.unwrap_or_default())
                    .await,
            )
            .await
        }
        // Terminal methods
        "terminal/create" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::terminal_create_payload(server, request.params.unwrap_or_default())
                    .await,
            )
            .await
        }
        "terminal/output" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::terminal_output_payload(server, request.params.unwrap_or_default())
                    .await,
            )
            .await
        }
        "terminal/release" => {
            dispatch_to_client(
                server,
                request_id,
                protocol_pack::handle_terminal_release(server, request.params.unwrap_or_default())
                    .await,
            )
            .await
        }
        "terminal/kill" => {
            dispatch_to_client(
                server,
                request_id,
                protocol_pack::handle_terminal_kill(server, request.params.unwrap_or_default())
                    .await,
            )
            .await
        }
        "terminal/wait_for_exit" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                protocol_pack::terminal_wait_for_exit_payload(
                    server,
                    request.params.unwrap_or_default(),
                )
                .await,
            )
            .await
        }
        _ => {
            // Unreachable: handle_request routes only methods belonging to this
            // family to this dispatcher; the MethodNotFound fallback stays in
            // the parent module's dispatch table.
            Ok(())
        }
    }
}
