//! Shared TCP accept-loop skeleton for HTTP servers.
//!
//! Both the ACP HTTP server (`acp/impl/runtime/http_server.rs`) and the MCP
//! HTTP server (`protocol/mcp_server.rs`) previously duplicated the accept
//! loop: signal handling (SIGINT/SIGTERM/shutdown-notify), the `select!`
//! dispatch over accept vs shutdown, and the per-connection `tokio::spawn`.
//! Protocol-specific concerns (connection handler, TLS wrapping, concurrency
//! limiting, graceful-drain details) stay in the caller via the handler
//! closure, which captures whatever state it needs (Arc handles, acceptors).
//!
//! This unifies the skeleton so the two servers share one signal/shutdown
//! implementation and cannot drift apart on shutdown semantics.

use std::sync::Arc;

use anyhow::Result;
use tokio::net::{TcpListener, TcpStream};
use tokio::signal;
use tracing::info;

/// Signal watcher for graceful shutdown (SIGINT, SIGTERM on Unix).
async fn shutdown_signal() {
    let ctrl_c = signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut sigterm = match signal::unix::signal(signal::unix::SignalKind::terminate()) {
            Ok(stream) => stream,
            Err(e) => {
                tracing::warn!(
                    "failed to register SIGTERM handler: {e}; graceful shutdown via SIGTERM disabled"
                );
                let _ = ctrl_c.await;
                return;
            }
        };
        tokio::select! {
            _ = ctrl_c => {}
            _ = sigterm.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = ctrl_c.await;
    }
}

/// Run the shared HTTP accept loop.
///
/// Loops until a shutdown signal (SIGINT/SIGTERM) or `shutdown_notify` fires:
/// - `should_stop()` is polled each iteration so callers can inject an
///   additional stop condition (e.g. a drain-guard).
/// - Accepted connections are handed to `on_connection(stream, peer_addr)`
///   in a spawned task, bounded by `max_connections` (backpressure).
///
/// `on_connection` is a closure capturing protocol-specific state (Arc
/// handles, TLS acceptors). Returning from the loop means the caller should
/// proceed with its own graceful-drain/shutdown sequence.
pub async fn run_http_accept_loop<F, Fut>(
    listener: TcpListener,
    shutdown_notify: Arc<tokio::sync::Notify>,
    max_connections: usize,
    mut should_stop: impl FnMut() -> bool,
    on_connection: Arc<F>,
) -> Result<()>
where
    F: Fn(TcpStream, std::net::SocketAddr) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let concurrency = Arc::new(tokio::sync::Semaphore::new(max_connections.max(1)));

    loop {
        tokio::select! {
            _ = shutdown_notify.notified() => {
                info!("HTTP server shutting down (shutdown notify)");
                break;
            }
            _ = shutdown_signal() => {
                info!("Received shutdown signal (SIGINT/SIGTERM), initiating graceful shutdown...");
                break;
            }
            result = listener.accept() => {
                if should_stop() {
                    continue;
                }
                let permit = match Arc::clone(&concurrency).acquire_owned().await {
                    Ok(p) => p,
                    Err(_) => break,
                };
                let (socket, peer_addr) = match result {
                    Ok(pair) => pair,
                    Err(e) => {
                        tracing::warn!("accept failed: {e}");
                        continue;
                    }
                };
                let on_connection = Arc::clone(&on_connection);
                tokio::spawn(async move {
                    let _permit = permit;
                    (*on_connection)(socket, peer_addr).await;
                });
            }
        }
    }

    Ok(())
}

/// Build a TCP listener with SO_REUSEADDR where possible (avoids
/// "Address already in use" after a restart).
pub async fn bind_tcp_listener(bind_addr: &str) -> Result<TcpListener> {
    use tokio::net::TcpSocket;
    match bind_addr.parse::<std::net::SocketAddr>() {
        Ok(addr) => {
            let s = match addr {
                std::net::SocketAddr::V4(_) => TcpSocket::new_v4()?,
                std::net::SocketAddr::V6(_) => TcpSocket::new_v6()?,
            };
            s.set_reuseaddr(true)?;
            s.bind(addr)?;
            Ok(s.listen(1024)?)
        }
        Err(_) => Ok(TcpListener::bind(bind_addr).await?),
    }
}
