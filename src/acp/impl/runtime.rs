//! Runtime implementation functions for ACP server
//!
//! This module contains standalone functions that implement the core runtime
//! functionality previously in the `impl AcpServer` block.
//! These functions take `AcpServer` as their first parameter to maintain
//! compatibility with the original implementation.
//!
//! NOTE: This file has been refactored into sub-modules. The large independent
//! blocks have been extracted to:
//!   - `http` — HTTP server routing, response writing, CORS handling
//!   - `sse` — SSE streaming, events, SSE transports
//!   - `openai_compat` — OpenAI API compatibility, Responses API
//!   - `security` — mTLS, TLS, entry auth, RBAC authorization
//!   - `protocol` — Protocol negotiation, HTTP request parsing, version handshake

use std::sync::Arc;

use anyhow::Result;
use tokio::io::AsyncWriteExt;
use tokio::signal;
use tracing::{error, info};

use crate::acp::r#impl::io::send_error;
use crate::acp::r#impl::request::handle_request;
use crate::acp::server::AcpServer;
use crate::acp::transport::{set_current_transport, StdioTransport};
use crate::agent::AgentRegistry;
use crate::flow::FlowManager;
use crate::rpc_protocol::{JsonRpcRequest, JsonRpcResponse};

// ---------------------------------------------------------------------------
// Sub-module declarations
// ---------------------------------------------------------------------------

pub(crate) mod http_server;
pub(crate) mod server_builder;
pub(crate) mod tls;

// Newly extracted sub-modules
pub(crate) mod http;
pub(crate) mod openai_compat;
pub(crate) mod protocol;
pub(crate) mod security;
pub(crate) mod sse;

// ---------------------------------------------------------------------------
// Re-exports
// ---------------------------------------------------------------------------

/// Re-export the HTTP server entry point from http_server module.
pub use http_server::run_acp_http_server;

/// Re-export server builder functions from server_builder module.
pub(crate) use server_builder::new_acp_server;

// ---------------------------------------------------------------------------
// Shared utilities
// ---------------------------------------------------------------------------

/// Write data to a TcpStream with a 30-second timeout.
/// Shared by `http` and `sse` sub-modules.
pub(crate) async fn tcp_write_timeout(
    socket: &mut (impl tokio::io::AsyncWrite + Unpin),
    data: &[u8],
) -> Result<()> {
    tokio::time::timeout(
        crate::shared::http_timeouts::SOCKET_WRITE_TIMEOUT,
        socket.write_all(data),
    )
    .await
    .map_err(|_| anyhow::anyhow!("timeout writing to socket"))?
    .map_err(|e| anyhow::anyhow!("socket write error: {e}"))
}

// ---------------------------------------------------------------------------
// Main ACP server entry point (stdio-based JSON-RPC)
// ---------------------------------------------------------------------------

/// Run the ACP server (stdio-based JSON-RPC mode).
///
/// # Startup Latency Optimization (BOTTLENECK-01, BOTTLENECK-03)
///
/// All background initialization (memory bridge promote, background tasks,
/// evolution loop) is spawned as tokio tasks so the stdin JSON-RPC loop
/// starts IMMEDIATELY. This aligns with ZED's expectation that an ACP stdio
/// server responds to `initialize`/`session/new`/`tools/list` without any
/// startup delay.
///
/// The synchronous work before entering the stdin loop is minimal:
/// build AcpServer + initialize_cache (~30-100ms spawned_blocking).
pub async fn run_acp_server(server: Arc<AcpServer>) -> Result<()> {
    // Set global transport to StdioTransport for all output from this process.
    // Uses `let _ =` to ignore AlreadySetErr, which can happen in tests where
    // the transport was already set indirectly.
    set_current_transport(Arc::new(StdioTransport));
    crate::acp::server::set_current_acp_server(Arc::clone(&server));

    info!("ACP server starting");

    let shutdown_notify = Arc::clone(&server.shutdown_notify);

    // ── All background init runs concurrently with stdin loop ──
    let bg_server = Arc::clone(&server);
    let bg_shutdown = shutdown_notify.clone();
    tokio::spawn(async move {
        // Start background tasks (BOTTLENECK-01) — GC, maintenance, security
        // scans, memory bridge initial promote (all protocol modes), etc.
        // NOTE: the memory-bridge initial promote is NOT duplicated here —
        // start_background_tasks() runs it once for every protocol mode.
        if let Err(e) =
            crate::acp::background::start_background_tasks(&bg_server, bg_shutdown.clone()).await
        {
            tracing::error!("Background tasks ultimately failed: {e}");
        }
    });

    info!("ACP server running");

    // stdin is read on a dedicated plain OS thread feeding an unbounded channel
    // (shared implementation, see `shared::stdio::spawn_stdin_lines`). Tokio's
    // stdio is implemented as a blocking read on the blocking-pool thread that
    // CANNOT be cancelled; at runtime drop the pool waits for it forever unless
    // stdin reaches EOF, which hangs shutdown whenever the client keeps the
    // pipe open. A plain thread is not tracked by the blocking pool, so runtime
    // teardown never waits on it (the thread exits on EOF and is killed with
    // the process otherwise).
    let mut stdin_rx = crate::shared::stdio::spawn_stdin_lines();

    // Set up signal watchers for graceful shutdown
    let mut sigterm = std::pin::pin!(async {
        #[cfg(unix)]
        {
            match signal::unix::signal(signal::unix::SignalKind::terminate()) {
                Ok(mut stream) => {
                    stream.recv().await;
                }
                Err(e) => {
                    tracing::warn!("failed to register SIGTERM handler: {e}; graceful shutdown via SIGTERM disabled");
                    std::future::pending::<()>().await;
                }
            }
        }
        #[cfg(not(unix))]
        std::future::pending::<()>().await;
    });

    loop {
        if server.shutdown_requested() {
            break;
        }

        let line = tokio::select! {
            _ = shutdown_notify.notified() => {
                break;
            }
            _ = signal::ctrl_c() => {
                info!("Received SIGINT (Ctrl+C), initiating graceful shutdown...");
                break;
            }
            _ = sigterm.as_mut() => {
                info!("Received SIGTERM, initiating graceful shutdown...");
                break;
            }
            // Shutdown can be requested from a spawned handler task (e.g. the
            // "shutdown" RPC). tokio::sync::Notify::notify_waiters() does NOT
            // store a notification for a future waiter, so the loop can miss it
            // and stay blocked on next_line() forever. Poll the flag on a short
            // timeout so a shutdown is honored within 200ms in every interleaving.
            _ = tokio::time::sleep(std::time::Duration::from_millis(200)) => {
                if server.shutdown_requested() {
                    break;
                }
                continue;
            }
            line = stdin_rx.recv() => match line {
                Some(line) => line,
                // stdin EOF — the client closed the pipe, so shut down.
                None => break,
            },
        };

        if server.shutdown_requested() {
            break;
        }

        if line.trim().is_empty() {
            continue;
        }

        let raw_message: serde_json::Value = match serde_json::from_str(&line) {
            Ok(message) => message,
            Err(err) => {
                send_error(
                    &server,
                    None,
                    -32700,
                    crate::i18n::runtime::tf("error.parse_error", &[("error", &err.to_string())]),
                    None,
                )
                .await?;
                continue;
            }
        };

        if is_jsonrpc_response(&raw_message) {
            match serde_json::from_value::<JsonRpcResponse>(raw_message) {
                Ok(response) => {
                    if !server.resolve_pending_client_response(response).await {
                        tracing::warn!("received unmatched ACP client response");
                    }
                }
                Err(err) => {
                    tracing::warn!("invalid ACP client response: {err}");
                }
            }
            continue;
        }

        let request = match serde_json::from_value::<JsonRpcRequest>(raw_message) {
            Ok(request) => request,
            Err(err) => {
                send_error(
                    &server,
                    None,
                    -32700,
                    crate::i18n::runtime::tf("error.parse_error", &[("error", &err.to_string())]),
                    None,
                )
                .await?;
                continue;
            }
        };

        if request.jsonrpc != "2.0" {
            send_error(
                &server,
                request.id,
                -32600,
                crate::i18n::runtime::t("error.jsonrpc_must_be_2_0").to_string(),
                None,
            )
            .await?;
            continue;
        }

        let server_for_task = Arc::clone(&server);
        tokio::spawn(async move {
            // Record one global op per stdio request (parity with the HTTP
            // route-level record in `route_http_post`). The chat pipeline's
            // `act_phase` does NOT record — that would double-count HTTP chat
            // requests — so the stdio transport records here at dispatch.
            let start = std::time::Instant::now();
            let result = handle_request(&server_for_task, request, None).await;
            crate::observability::performance::record_global_operation(
                result.is_ok(),
                start.elapsed().as_secs_f64() * 1000.0,
            );
            if let Err(err) = result {
                error!("request failed: {err:#}");
            }
        });
    }

    // ── Graceful shutdown ──────────────────────────────────────────
    server.begin_shutdown();
    shutdown_notify.notify_waiters();
    info!("ACP server shutting down");
    Ok(())
}

fn is_jsonrpc_response(value: &serde_json::Value) -> bool {
    value.get("id").is_some()
        && (value.get("result").is_some() || value.get("error").is_some())
        && value.get("method").is_none()
}

// ---------------------------------------------------------------------------
// Accessor Functions
// ---------------------------------------------------------------------------

/// Get routing handles (flow manager and agent registry).
pub fn routing_handles(server: &AcpServer) -> Result<(Arc<FlowManager>, Arc<AgentRegistry>)> {
    let flow = server
        .model_deps
        .flow_manager
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("flow manager not initialized"))?;
    let registry = server
        .model_deps
        .agent_registry
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("agent registry not initialized"))?;
    Ok((Arc::clone(flow), Arc::clone(registry)))
}

/// Get artifact ledger.
pub fn artifact_ledger(server: &AcpServer) -> crate::reinforcement::ArtifactLedger {
    server
        .persistence
        .artifact_ledger
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_else(|poisoned| {
            tracing::warn!("artifact_ledger lock poisoned — recovering inner state");
            poisoned.into_inner().clone()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn jsonrpc_response_detection_distinguishes_requests() {
        assert!(is_jsonrpc_response(&json!({
            "jsonrpc": "2.0",
            "id": "req-1",
            "result": {"ok": true}
        })));
        assert!(!is_jsonrpc_response(&json!({
            "jsonrpc": "2.0",
            "id": "req-1",
            "method": "session/new",
            "params": {}
        })));
    }

    #[tokio::test]
    async fn pending_client_response_round_trips_result() {
        let server = crate::acp::server::ServerBuilder::default().build();
        let rx = server
            .register_pending_client_request("req-42".to_string())
            .await;

        let resolved = server
            .resolve_pending_client_response(JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                result: Some(json!({"optionId": "allow"})),
                error: None,
                id: Some(json!("req-42")),
            })
            .await;

        assert!(resolved);
        let value = rx.await.unwrap().unwrap();
        assert_eq!(value["optionId"], "allow");
    }
}
