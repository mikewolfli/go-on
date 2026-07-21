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
use std::time::Duration;

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::signal;
use tracing::{error, info};

use crate::acp::r#impl::io::send_error;
use crate::acp::r#impl::request::handle_request;
use crate::acp::server::AcpServer;
use crate::acp::transport::{set_current_transport, StdioTransport};
use crate::agent::AgentRegistry;
use crate::flow::FlowManager;
use crate::rpc_protocol::JsonRpcRequest;

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
    tokio::time::timeout(std::time::Duration::from_secs(30), socket.write_all(data))
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
    let _ = set_current_transport(Arc::new(StdioTransport));

    info!("ACP server starting");

    let shutdown_notify = Arc::clone(&server.shutdown_notify);

    // ── All background init runs concurrently with stdin loop ──
    let bg_server = Arc::clone(&server);
    let bg_shutdown = shutdown_notify.clone();
    tokio::spawn(async move {
        // Spawn memory bridge promote (BOTTLENECK-03) — small delay lets stdin loop start first
        // Uses lazy initialization (S1 startup optimization) to defer SQLite connection creation.
        if let Some(mp) = bg_server.get_or_init_memory_persistence() {
            let memory_store = bg_server.persistence.memory_store.clone();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            if let Err(e) = crate::memory::memory_bridge::bridge_promote(&memory_store, &mp) {
                tracing::warn!("memory bridge: initial promote failed (background): {e}");
            }
        }

        // Start background tasks (BOTTLENECK-01) — GC, maintenance, security scans, etc.
        if let Err(e) =
            crate::acp::background::start_background_tasks(&bg_server, bg_shutdown.clone()).await
        {
            tracing::error!("Background tasks ultimately failed: {e}");
        }
    });

    // ── Spawn EvolutionLoop (BLUE56-B03) — never blocks stdin ──
    if let Some(ref evo) = server.governance_deps.evolution_loop {
        let evo_clone = Arc::clone(evo);
        tokio::spawn(async move {
            loop {
                let mut guard = evo_clone.lock().await;
                if let Err(e) = guard.run().await {
                    tracing::warn!("Evolution loop cycle ended: {}; retrying after 60s", e);
                }
                drop(guard);
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        });
        tracing::info!(target: "intelligence", "EvolutionLoop spawned");
    }

    info!("ACP server running");

    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();

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

        let next_line = tokio::select! {
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
            line = lines.next_line() => line?,
        };

        let Some(line) = next_line else {
            break;
        };

        if server.shutdown_requested() {
            break;
        }

        if line.trim().is_empty() {
            continue;
        }

        let request = match serde_json::from_str::<JsonRpcRequest>(&line) {
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

        if let Err(err) = handle_request(&server, request, None).await {
            error!("request failed: {err:#}");
        }
    }

    // ── Graceful shutdown ──────────────────────────────────────────
    server.begin_shutdown();
    shutdown_notify.notify_waiters();
    info!("ACP server shutting down");
    Ok(())
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
