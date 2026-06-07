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

use std::sync::{Arc, LazyLock};
use std::time::Duration;

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::signal;
use tracing::{error, info, warn};

use crate::acp::background::start_background_tasks;
use crate::acp::r#impl::io::send_error;
use crate::acp::r#impl::request::handle_request;
use crate::acp::server::AcpServer;
use crate::agent::AgentRegistry;
use crate::config::{AutoTuneState, VectorConfig};
use crate::flow::FlowManager;
use crate::rpc_protocol::JsonRpcRequest;
use crate::vector::VectorStore;

// ---------------------------------------------------------------------------
// Sub-module declarations
// ---------------------------------------------------------------------------

pub mod auth;
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
// Shared statics
// ---------------------------------------------------------------------------

/// Serializes concurrent `/rpc` calls to prevent pipe-swapping race conditions.
/// `server.output` is a global singleton — without this guard, two concurrent
/// `/rpc` requests would corrupt each other's response capture pipes.
static RPC_SERIAL: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

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
pub async fn run_acp_server(server: &mut AcpServer) -> Result<()> {
    info!("ACP server starting");

    // GAP-B58-B13: Wire memory bridge — run initial promotion on startup
    if let Some(mp) = server.governance_deps.memory_persistence.as_ref() {
        let memory_store = &server.persistence.memory_store;
        match crate::memory::memory_bridge::bridge_promote(memory_store, mp) {
            Ok(report) => {
                if report.promoted_count > 0 {
                    tracing::info!(
                        "memory bridge: initial promote moved {} entries",
                        report.promoted_count
                    );
                }
            }
            Err(e) => {
                tracing::warn!("memory bridge: initial bridge_promote failed: {e}");
            }
        }
    }

    let shutdown_notify = Arc::clone(&server.shutdown_notify);

    // Start background tasks
    if let Err(e) = start_background_tasks(server, Arc::clone(&shutdown_notify)).await {
        error!("Failed to start background tasks: {}", e);
        return Err(e);
    }

    // ── Spawn EvolutionLoop (BLUE56-B03) ─────────────────────────────
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
                    warn!("failed to register SIGTERM handler: {e}; graceful shutdown via SIGTERM disabled");
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
                    server,
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
                server,
                request.id,
                -32600,
                crate::i18n::runtime::t("error.jsonrpc_must_be_2_0").to_string(),
                None,
            )
            .await?;
            continue;
        }

        if let Err(err) = handle_request(server, request, None).await {
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

/// Get cache handle.
#[allow(dead_code)]
#[must_use]
pub fn cache_handle(server: &AcpServer) -> Option<Arc<crate::cache::ResponseCache>> {
    server.cache_deps.cache.response_cache.clone()
}

/// Get artifact ledger.
pub fn artifact_ledger(_server: &AcpServer) -> crate::reinforcement::ArtifactLedger {
    _server
        .persistence
        .artifact_ledger
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_else(|_| {
            crate::reinforcement::ArtifactLedger::new(
                _server.config_path.as_deref().map(std::path::Path::new),
            )
        })
}

/// Get vector store handle.
#[allow(dead_code)]
#[must_use]
pub fn vector_store_handle(server: &AcpServer) -> Option<Arc<VectorStore>> {
    server.cache_deps.cache.vector_store.clone()
}

/// Get vector configuration snapshot.
#[allow(dead_code)]
#[must_use]
pub fn vector_config_snapshot(server: &AcpServer) -> Option<VectorConfig> {
    server.cache_deps.vector_config.clone()
}

/// Get autotune handle.
#[allow(dead_code)]
#[must_use]
pub fn autotune_handle(server: &AcpServer) -> Option<Arc<tokio::sync::Mutex<AutoTuneState>>> {
    server.cache_deps.autotune.clone()
}
