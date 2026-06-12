//! ACP HTTP server main loop
//!
//! Contains the main HTTP server accept loop and connection management.
//! Extracted from the parent `runtime.rs` to reduce the monolithic file size.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use tokio::net::{TcpListener, TcpSocket};
use tokio::signal;
use tokio::sync::Semaphore;
use tracing::{error, info, warn};

use crate::acp::background::start_background_tasks;
use crate::acp::server::AcpServer;

/// Limits concurrent ACP HTTP connections to prevent unbounded tokio task growth.
static CONNECTION_SEMAPHORE: Semaphore = Semaphore::const_new(1000);

use super::http::handle_http_connection;
use super::tls::{handle_mtls_http_connection, handle_tls_http_connection};

/// Start the ACP HTTP server.
///
/// Creates a TCP listener, optionally configures TLS/mTLS, and enters the
/// accept loop. Handles graceful shutdown on SIGINT, SIGTERM, or notification.
pub async fn run_acp_http_server(server: Arc<AcpServer>, bind_addr: String) -> Result<()> {
    info!("ACP HTTP server starting on {}", bind_addr);

    let shutdown_notify = Arc::clone(&server.shutdown_notify);

    if let Err(err) = start_background_tasks(server.as_ref(), Arc::clone(&shutdown_notify)).await {
        error!("Failed to start background tasks: {}", err);
        return Err(err);
    }

    // Create socket with SO_REUSEADDR to avoid "Address already in use" after restart
    let listener = match bind_addr.parse::<SocketAddr>() {
        Ok(addr) => {
            let s = match addr {
                SocketAddr::V4(_) => TcpSocket::new_v4()?,
                SocketAddr::V6(_) => TcpSocket::new_v6()?,
            };
            s.set_reuseaddr(true)?;
            s.bind(addr)?;
            s.listen(1024)?
        }
        Err(_) => TcpListener::bind(&bind_addr).await?,
    };

    // GAP-B52-24: Configure mTLS acceptor when enabled
    // The MtlsAcceptor wraps each accepted TCP stream with mTLS before
    // passing to the HTTP connection handler.
    let mtls_acceptor: Option<Arc<crate::security::mtls::MtlsAcceptor>> = {
        if server.runtime_config.mtls_enabled
            && !server.runtime_config.mtls_server_cert_path.is_empty()
            && !server.runtime_config.mtls_server_key_path.is_empty()
        {
            #[cfg(feature = "multi-users-server")]
            {
                use crate::security::mtls::MtlsConfig;
                let mut mtls_config = MtlsConfig::new(
                    &server.runtime_config.mtls_ca_cert_path,
                    &server.runtime_config.mtls_server_cert_path,
                    &server.runtime_config.mtls_server_key_path,
                );
                if !server.runtime_config.mtls_ca_cert_path.is_empty() {
                    mtls_config = mtls_config.with_client_cert(true);
                }
                if !server.runtime_config.mtls_allowed_cns.is_empty() {
                    let allowed: Vec<String> = server
                        .runtime_config
                        .mtls_allowed_cns
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    mtls_config = mtls_config.with_allowed_cns(allowed);
                }

                let acceptor = crate::security::mtls::MtlsAcceptor::new(mtls_config);
                // Validate the config by building once; accept() will lazily
                // build the ServerConfig on the first connection.
                match acceptor.build_server_config() {
                    Ok(_) => {
                        tracing::info!("mTLS enabled for ACP HTTP server");
                        Some(Arc::new(acceptor))
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to build mTLS server config: {} — falling back to plain TCP",
                            e
                        );
                        None
                    }
                }
            }
            #[cfg(not(feature = "multi-users-server"))]
            {
                tracing::warn!(
                    "mTLS is configured but requires multi-users-server feature"
                );
                None
            }
        } else {
            None
        }
    };

    // ── Plain TLS acceptor (non-mTLS, env-var configured) ───────────────
    // Reads GO_ON_TLS_CERT and GO_ON_TLS_KEY environment variables.
    // If both are set, wraps the TCP connection with TLS before handing off
    // to the plain HTTP handler. Falls back to plain TCP if unset.
    let tls_acceptor: Option<tokio_rustls::TlsAcceptor> = {
        let tls_cert = std::env::var("GO_ON_TLS_CERT").ok();
        let tls_key = std::env::var("GO_ON_TLS_KEY").ok();

        match (tls_cert, tls_key) {
            (Some(cert_path), Some(key_path)) => {
                let certs = match std::fs::File::open(&cert_path) {
                    Ok(file) => {
                        let mut reader = std::io::BufReader::new(file);
                        match rustls_pemfile::certs(&mut reader).collect::<Result<Vec<_>, _>>() {
                            Ok(certs) => certs,
                            Err(e) => {
                                tracing::warn!(
                                    "Failed to read TLS certs from {}: {} — falling back to plain TCP",
                                    cert_path,
                                    e
                                );
                                return Err(anyhow::anyhow!("failed to read TLS certs: {}", e));
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to open TLS cert file {}: {} — falling back to plain TCP",
                            cert_path,
                            e
                        );
                        return Err(anyhow::anyhow!("failed to open TLS cert: {}", e));
                    }
                };

                let key = match std::fs::File::open(&key_path) {
                    Ok(file) => {
                        let mut reader = std::io::BufReader::new(file);
                        match rustls_pemfile::private_key(&mut reader) {
                            Ok(Some(key)) => key,
                            Ok(None) => {
                                tracing::warn!(
                                    "No private key found in {} — falling back to plain TCP",
                                    key_path
                                );
                                return Err(anyhow::anyhow!(
                                    "no private key found in {}",
                                    key_path
                                ));
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "Failed to read TLS key from {}: {} — falling back to plain TCP",
                                    key_path,
                                    e
                                );
                                return Err(anyhow::anyhow!("failed to read TLS key: {}", e));
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to open TLS key file {}: {} — falling back to plain TCP",
                            key_path,
                            e
                        );
                        return Err(anyhow::anyhow!("failed to open TLS key: {}", e));
                    }
                };

                match rustls::ServerConfig::builder()
                    .with_no_client_auth()
                    .with_single_cert(certs, key)
                {
                    Ok(tls_config) => {
                        tracing::info!("TLS enabled (cert={}, key={})", cert_path, key_path);
                        Some(tokio_rustls::TlsAcceptor::from(std::sync::Arc::new(
                            tls_config,
                        )))
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to build TLS config: {} — falling back to plain TCP",
                            e
                        );
                        return Err(anyhow::anyhow!("failed to build TLS config: {}", e));
                    }
                }
            }
            _ => {
                // One or both env vars not set — plain TCP
                None
            }
        }
    };

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
        tokio::select! {
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
            incoming = listener.accept() => {
                // Check if draining before accepting new connections
                if server.drain_guard.is_draining() {
                    drop(incoming);
                    continue;
                }
                // Bound concurrent connections — backpressure if we already have
                // 1000 active connections, preventing unbounded tokio task growth.
                let _permit = CONNECTION_SEMAPHORE.acquire().await;
                let (socket, peer_addr) = incoming?;
                let server_ref = Arc::clone(&server);
                if let Some(ref acceptor) = mtls_acceptor {
                    // mTLS path: perform TLS handshake, then handle through
                    // the dedicated mTLS HTTP handler.
                    let acceptor_clone = acceptor.clone();
                    tokio::spawn(async move {
                        let _permit = _permit;
                        if let Err(err) = handle_mtls_http_connection(
                            acceptor_clone.as_ref(),
                            socket,
                            server_ref,
                            peer_addr,
                        )
                        .await
                        {
                            warn!("ACP mTLS connection {} failed: {}", peer_addr, err);
                        }
                    });
                } else if let Some(ref acceptor) = tls_acceptor {
                    // Plain TLS path: perform TLS handshake, then handle
                    // through the same HTTP handler (no client cert).
                    let acceptor_clone = acceptor.clone();
                    tokio::spawn(async move {
                        let _permit = _permit;
                        if let Err(err) = handle_tls_http_connection(
                            &acceptor_clone,
                            socket,
                            server_ref,
                            peer_addr,
                        )
                        .await
                        {
                            warn!("ACP TLS connection {} failed: {}", peer_addr, err);
                        }
                    });
                } else {
                    // Plain TCP path
                    tokio::spawn(async move {
                        let _permit = _permit;
                        let mut socket = socket;
                        if let Err(err) =
                            handle_http_connection(&mut socket, server_ref, peer_addr).await
                        {
                            warn!("ACP HTTP connection {} failed: {}", peer_addr, err);
                        }
                    });
                }
            }
        }
    }

    // ── Graceful shutdown with DrainGuard ───────────────────────────
    // Shutdown order: stop_accepting → drain_requests → stop_bg_tasks → close_db → exit
    tracing::info!("Shutdown phase 1/5: stop_accepting — beginning drain");
    server.drain_guard.start_drain();

    tracing::info!(
        "Shutdown phase 2/5: drain_requests — waiting up to {:?}",
        server.drain_guard.drain_timeout
    );
    server.drain_guard.wait_for_drain().await;

    tracing::info!("Shutdown phase 3/5: stop_bg_tasks — notifying background tasks");
    server.shutdown_notify.notify_waiters();

    tracing::info!("Shutdown phase 4/5: close_db — closing database connections");
    // Database connections are closed on drop via SQLite connection lifecycle.

    tracing::info!("Shutdown phase 5/5: exit — server shutdown complete");
    info!("ACP HTTP server shutting down");
    Ok(())
}
