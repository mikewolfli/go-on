// Request dispatch for the workflow method family.
//
// Extracted from the single dispatch table in `request/mod.rs` (module split
// M0.4): each family matches the normalized method name exactly as the
// original `handle_request` match did, so dispatch behavior is unchanged.
// Workflow-family dispatch: workflow lifecycle, task planning/execution,
// and the distributed-memory ingest bridge.
use anyhow::Result;
use serde_json::Value;

use crate::acp::server::AcpServer;
use crate::rpc_protocol::{JsonRpcRequest, RequestTraceContext};

use super::dispatch_to_client;
use super::exec_pack;
use super::exec_pack::{handle_task_execute, handle_workflow_execute};
use super::workflow_pack;

pub(crate) async fn dispatch_workflow(
    server: &AcpServer,
    request: JsonRpcRequest,
    request_id: Option<Value>,
    _http_headers: Option<&str>,
    trace: &RequestTraceContext,
    method: &str,
) -> Result<()> {
    match method {
        "workflow.confirm" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                workflow_pack::workflow_confirm_payload(server, request.params.unwrap_or_default())
                    .await,
            )
            .await
        }
        "workflow.clarify" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                workflow_pack::workflow_clarify_payload(server, request.params.unwrap_or_default())
                    .await,
            )
            .await
        }
        "workflow.research" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                workflow_pack::workflow_research_payload(
                    server,
                    request.params.unwrap_or_default(),
                )
                .await,
            )
            .await
        }
        "workflow.consult" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                workflow_pack::workflow_consult_payload(server, request.params.unwrap_or_default())
                    .await,
            )
            .await
        }
        "workflow.ask" => {
            dispatch_to_client(
                server,
                request_id,
                workflow_pack::handle_workflow_ask(
                    server,
                    request.params.unwrap_or_default(),
                    trace,
                )
                .await,
            )
            .await
        }
        "workflow.generate_from_chat" => {
            dispatch_to_client(
                server,
                request_id,
                workflow_pack::handle_workflow_generate_from_chat(
                    server,
                    request.params.unwrap_or_default(),
                    trace,
                )
                .await,
            )
            .await
        }
        "workflow.generate" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                workflow_pack::workflow_generate_payload(
                    server,
                    request.params.unwrap_or_default(),
                    trace,
                )
                .await,
            )
            .await
        }
        "workflow.execute" => {
            dispatch_to_client(
                server,
                request_id,
                handle_workflow_execute(server, request.params.unwrap_or_default(), trace).await,
            )
            .await
        }
        "workflow.run.list" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                exec_pack::workflow_run_list_payload(&request.params.unwrap_or_default()),
            )
            .await
        }
        "workflow.run.get" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                exec_pack::workflow_run_get_payload(&request.params.unwrap_or_default()),
            )
            .await
        }
        "workflow.run.cancel" => {
            let params = request.params.unwrap_or_default();
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                exec_pack::workflow::workflow_run_transition_payload(&params, "cancelled"),
            )
            .await
        }
        "workflow.run.pause" => {
            let params = request.params.unwrap_or_default();
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                exec_pack::workflow::workflow_run_transition_payload(&params, "paused"),
            )
            .await
        }
        "workflow.run.resume" => {
            let params = request.params.unwrap_or_default();
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                exec_pack::workflow::workflow_run_transition_payload(&params, "running"),
            )
            .await
        }
        "task.plan" => {
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                workflow_pack::task_plan_payload(server, request.params.unwrap_or_default(), trace)
                    .await,
            )
            .await
        }
        #[cfg(feature = "sub-bus-distributed-memory")]
        "memory.ingest" => {
            // Receiving side of the DistributedMemoryBus HTTP transport:
            // entries pushed by a peer node's do_sync are ingested into
            // this node's shared-entries buffer (previously the ACP
            // `/rpc` endpoint had no handler, so peer syncs were only
            // observable in the hub vault and never reached the bus).
            // Gate matches hub/server.rs and DistributedMemoryBus so
            // simple-server (which enables sub-bus-distributed-memory)
            // compiles this arm too.
            let params = request.params.unwrap_or_default();
            let entries_json = serde_json::to_string(
                &params
                    .get("entries")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!([])),
            )
            .unwrap_or_else(|_| "[]".to_string());
            let ingested = match server.governance_deps.capability_bus.as_ref() {
                Some(cb) => match cb.distributed_memory_bus.ingest_shared(&entries_json) {
                    Ok(n) => n as u64,
                    Err(e) => {
                        tracing::warn!("memory.ingest: failed to ingest shared entries: {}", e);
                        0
                    }
                },
                None => 0,
            };
            crate::acp::r#impl::io::respond(
                server,
                request_id,
                Ok(serde_json::json!({"ok": true, "stored": ingested})),
            )
            .await
        }
        "task.execute" => {
            dispatch_to_client(
                server,
                request_id,
                handle_task_execute(server, request.params.unwrap_or_default(), trace).await,
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
