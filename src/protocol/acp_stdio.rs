//! ACP Stdio Runner — lightweight line-delimited JSON-RPC over stdin/stdout.
//!
//! Designed for Zed's subprocess agent server integration pattern.
//! Reads one JSON-RPC 2.0 request per line from stdin, dispatches to
//! the ACP request handler, and writes responses to stdout.
//!
//! Response output is handled automatically by
//! [`crate::acp::r#impl::io::write_json_line`], which writes directly to
//! tokio::io::stdout() when no per-request `RPC_BUFFER` task-local is set
//! (the fallback path for stdio mode).  This means the stdio runner does
//! *not* need to set up any buffer plumbing — every `send_result` /
//! `send_error` / `send_notification` call made by `handle_request` will
//! naturally emit JSON lines to stdout.

use std::sync::Arc;

use anyhow::Result;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::signal;
use tokio::sync::Notify;
use tracing::{error, info, warn};

use crate::acp::r#impl::request::handle_request;
use crate::acp::server::AcpServer;
use crate::rpc_protocol::JsonRpcRequest;

#[allow(dead_code)] // Public API for Zed agent server subprocess integration
/// Run the ACP stdio event loop.
///
/// Reads line-delimited JSON-RPC 2.0 requests from stdin, dispatches each
/// to [`handle_request`], and writes responses to stdout via
/// `write_json_line`'s built-in fallback path.
///
/// # Arguments
///
/// * `server` - Shared ACP server reference for request dispatch (holds
///   profiles, tools, prompts, resources, and authentication context).
/// * `shutdown` - Optional external shutdown notification.  When `None`,
///   only SIGTERM / SIGINT will trigger graceful shutdown.
pub async fn run_stdio_server(server: Arc<AcpServer>, shutdown: Option<Arc<Notify>>) -> Result<()> {
    // ── Shutdown coordination ──────────────────────────────────────
    // Priority: explicit shutdown signal > SIGTERM > SIGINT.
    let shutdown_signal = shutdown.unwrap_or_else(|| Arc::new(Notify::new()));
    let signal_notify = shutdown_signal.clone();

    // Spawn a signal handler that fires the Notify on first signal.
    tokio::spawn(async move {
        let mut term_signal = signal::unix::signal(signal::unix::SignalKind::terminate()).ok();
        let mut int_signal = signal::unix::signal(signal::unix::SignalKind::interrupt()).ok();

        tokio::select! {
            _ = async {
                if let Some(ref mut sig) = term_signal {
                    sig.recv().await;
                    info!("ACP stdio: received SIGTERM");
                }
            } => {}
            _ = async {
                if let Some(ref mut sig) = int_signal {
                    sig.recv().await;
                    info!("ACP stdio: received SIGINT");
                }
            } => {}
        }
        signal_notify.notify_one();
    });

    // ── I/O setup ─────────────────────────────────────────────────
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin);
    let mut line = String::new();

    info!("ACP stdio server started, waiting for JSON-RPC requests");

    // ── Event loop ─────────────────────────────────────────────────
    loop {
        tokio::select! {
            // Graceful shutdown requested (signal or external trigger).
            _ = shutdown_signal.notified() => {
                info!("ACP stdio: shutting down gracefully");
                break;
            }

            // Read one line from stdin.
            result = reader.read_line(&mut line) => {
                match result {
                    Ok(0) => {
                        // EOF — the parent process closed stdin.
                        info!("ACP stdio: stdin closed, shutting down");
                        break;
                    }
                    Ok(_) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            line.clear();
                            continue;
                        }

                        // Parse JSON-RPC 2.0 request.
                        match serde_json::from_str::<JsonRpcRequest>(trimmed) {
                            Ok(request) => {
                                // Dispatch through the ACP handler.
                                //
                                // Responses are written automatically to stdout via
                                // write_json_line's fallback path (no RPC_BUFFER is
                                // set in stdio mode).  The http_headers parameter is
                                // None because stdio has no HTTP context.
                                if let Err(e) = handle_request(&server, request, None).await {
                                    error!(error = %e, "ACP stdio: request handler failed");
                                }
                            }
                            Err(e) => {
                                // Write a JSON-RPC Parse Error response (-32700)
                                // directly to stdout.  This covers syntax errors
                                // before any request handler is involved.
                                let error_response = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "error": {
                                        "code": -32700,
                                        "message": "Parse error",
                                        "data": e.to_string()
                                    },
                                    "id": null
                                });
                                let mut buf = serde_json::to_string(&error_response)?;
                                buf.push('\n');
                                tokio::io::stdout().write_all(buf.as_bytes()).await?;
                                tokio::io::stdout().flush().await?;
                                warn!(error = %e, "ACP stdio: failed to parse request");
                            }
                        }
                        line.clear();
                    }
                    Err(e) => {
                        error!(error = %e, "ACP stdio: read error");
                        break;
                    }
                }
            }
        }
    }

    // Best-effort flush of any remaining output.
    let _ = tokio::io::stdout().flush().await;
    info!("ACP stdio server stopped");
    Ok(())
}
