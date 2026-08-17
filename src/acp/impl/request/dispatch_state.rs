// Request dispatch for the state method family.
//
// Extracted from the single dispatch table in `request/mod.rs` (module split
// M0.4): each family matches the normalized method name exactly as the
// original `handle_request` match did, so dispatch behavior is unchanged.
// State-family dispatch: conversation checkpoints, config reload/baseline,
// reproducibility/autotune/error-contract status, and provider configuration.
use anyhow::Result;
use serde_json::Value;

use crate::acp::server::AcpServer;
use crate::rpc_protocol::{JsonRpcRequest, RequestTraceContext};

use super::runtime_pack;
use super::repro_pack;
use super::repro_handlers;
use super::config_pack::{config_reload_payload, config_baseline_payload};
use super::dispatch_to_client;

pub(crate) async fn dispatch_state(
    server: &AcpServer,
    request: JsonRpcRequest,
    request_id: Option<Value>,
    _http_headers: Option<&str>,
    _trace: &RequestTraceContext,
    method: &str,
) -> Result<()> {
    match method {
        "conversation.checkpoint.create" => {
            dispatch_to_client(
                server,
                request_id,
                runtime_pack::handle_conversation_checkpoint_create(
                    server,
                    request.params.unwrap_or_default(),
                )
                .await,
            )
            .await
        }
        "conversation.checkpoint.list" | "checkpoint.list" => {
            dispatch_to_client(
                server,
                request_id,
                runtime_pack::handle_conversation_checkpoint_list(
                    server,
                    request.params.unwrap_or_default(),
                )
                .await,
            )
            .await
        }
        "conversation.rollback" => {
            dispatch_to_client(
                server,
                request_id,
                runtime_pack::handle_conversation_rollback(
                    server,
                    request.params.unwrap_or_default(),
                )
                .await,
            )
            .await
        }
        "conversation.checkpoint.prune" => {
            dispatch_to_client(
                server,
                request_id,
                runtime_pack::handle_conversation_checkpoint_prune(
                    server,
                    request.params.unwrap_or_default(),
                )
                .await,
            )
            .await
        }
        "config.reload" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                config_reload_payload(server).await,
            )
            .await
        }
        "config.baseline" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                config_baseline_payload(server, request.params.unwrap_or_default()).await,
            )
            .await
        }
        "build.repro" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                repro_pack::build_repro_payload(server).await,
            )
            .await
        }
        "optimization.peak" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                repro_handlers::optimization_peak_payload(
                    server,
                    request.params.unwrap_or_default(),
                )
                .await,
            )
            .await
        }
        "error.contract" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                runtime_pack::error_contract_payload(server),
            )
            .await
        }
        "autotune.get" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                runtime_pack::autotune_get_payload(server).await,
            )
            .await
        }
        "autotune.status" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                runtime_pack::autotune_status_payload(server).await,
            )
            .await
        }
        "autotune.reset" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                runtime_pack::autotune_reset_payload(server).await,
            )
            .await
        }
        "selector.status" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                runtime_pack::selector_status_payload(server),
            )
            .await
        }
        "hardness.status" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                runtime_pack::hardness_status_payload(
                    server,
                    request.params.unwrap_or_default(),
                ),
            )
            .await
        }
        "cost.status" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                runtime_pack::cost_status_payload(
                    server,
                    request.params.unwrap_or_default(),
                ),
            )
            .await
        }

        "provider.configure" => {
            dispatch_to_client(
                server,
                request_id,
                runtime_pack::handle_provider_configure(
                    server,
                    request.params.unwrap_or_default(),
                )
                .await,
            )
            .await
        }
        "provider.test_connection" => {
            let params = request.params.unwrap_or_default();
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                runtime_pack::provider_test_connection_payload(server, &params).await,
            )
            .await
        }
        "provider.test_completion" => {
            let params = request.params.unwrap_or_default();
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                runtime_pack::provider_test_completion_payload(server, &params),
            )
            .await
        }
        "provider.copilot_device_code" => {
            dispatch_to_client(
                server,
                request_id,
                runtime_pack::handle_copilot_device_code_request(
                    server,
                    request.params.unwrap_or_default(),
                )
                .await,
            )
            .await
        }
        "provider.copilot_device_code_poll" => {
            dispatch_to_client(
                server,
                request_id,
                runtime_pack::handle_copilot_device_code_poll(
                    server,
                    request.params.unwrap_or_default(),
                )
                .await,
            )
            .await
        }
        "provider.capabilities" => {
            let params = request.params.unwrap_or_default();
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                runtime_pack::provider_capabilities_payload(server, &params),
            )
            .await
        }
        "provider.catalog" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                runtime_pack::provider_catalog_payload(server),
            )
            .await
        }
        "provider.list_models" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                runtime_pack::provider_list_models_payload(
                    server,
                    request.params.unwrap_or_default(),
                )
                .await,
            )
            .await
        }
        "tool.approve" => {
            let params = request.params.unwrap_or_default();
            let tool_name = params
                .get("tool_name")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if tool_name.is_empty() {
                dispatch_to_client(
                    server,
                    request_id,
                    Err(anyhow::anyhow!(
                        "tool.approve: missing 'tool_name' parameter"
                    )),
                )
                .await
            } else {
                if let Some(ref harness_bus) = server.governance_deps.harness_bus {
                    harness_bus.evaluator.approve_tool(tool_name);
                    tracing::info!("tool.approve: user approved tool '{}'", tool_name);
                }
                crate::acp::r#impl::io::respond(
                    server,
                    request_id,
                    Ok(serde_json::json!({
                        "approved": true,
                        "tool_name": tool_name,
                    })),
                )
                .await
            }
        }
        _ => {
            // Unreachable: handle_request routes only methods belonging to this
            // family to this dispatcher; the MethodNotFound fallback stays in
            // the parent module's dispatch table.
            Ok(())
        }
    }
}
