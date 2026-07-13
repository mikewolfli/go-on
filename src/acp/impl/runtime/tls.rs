//! TLS/mTLS HTTP connection handling
//!
//! Contains the TLS and mTLS HTTP connection handler functions extracted
//! from the parent `runtime.rs` to reduce the monolithic file size.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tracing::warn;

use super::http::http_trace_context;
use super::protocol::parse_http_request;
use super::sse::{write_sse_event, write_sse_headers};
use crate::acp::r#impl::request::handle_request;
use crate::acp::server::AcpServer;
use crate::rpc_protocol::JsonRpcRequest;

/// Write an HTTP JSON response through a generic async writer.
async fn tls_write_http_json<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    status: u16,
    value: serde_json::Value,
    extra_headers: &str,
) -> Result<()> {
    use tokio::io::AsyncWriteExt;
    let status_text = match status {
        200 => "OK",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        _ => "OK",
    };
    let body = serde_json::to_vec(&value)?;
    let headers = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n",
        status,
        status_text,
        body.len(),
        extra_headers
    );
    writer.write_all(headers.as_bytes()).await?;
    writer.write_all(&body).await?;
    writer.shutdown().await?;
    Ok(())
}

/// Build the root capabilities response payload.
pub(crate) fn build_root_capabilities_response() -> serde_json::Value {
    serde_json::json!({
        "service": "go-on",
        "protocol": "acp-http",
        "health": "/health",
        "endpoints": {
            "chat": ["/chat", "/chat/stream"],
            "openai": ["/v1/models", "/v1/model", "/models", "/v1/chat/completions", "/chat/completions"],
            "responses": ["/v1/responses", "/v1/responses/{id}"],
        }
    })
}

/// Process a JSON-RPC request over an mTLS connection and return the
/// JSON-RPC response value.
async fn route_rpc_over_tls(server: &AcpServer, request: JsonRpcRequest) -> serde_json::Value {
    let method = request.method.clone();

    // Use per-request buffer via task-local, same as HTTP /rpc handler.
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let result = crate::acp::r#impl::io::RPC_BUFFER
        .scope(buffer, async {
            handle_request(server, request, None).await
        })
        .await;

    match result {
        Ok(()) => {
            serde_json::json!({
                "ok": true,
                "method": method,
            })
        }
        Err(e) => {
            serde_json::json!({
                "ok": false,
                "error": format!("{:#}", e),
                "method": method,
            })
        }
    }
}

/// Process an HTTP request received over a TLS stream.
///
/// Shared by both mTLS and plain-TLS handlers — reads the HTTP request from
/// the already-established TLS stream, routes it, and writes the response.
async fn handle_tls_http_stream(
    tls_stream: &mut tokio_rustls::TlsStream<tokio::net::TcpStream>,
    server: &Arc<AcpServer>,
    peer_addr: SocketAddr,
) -> Result<()> {
    // Read the HTTP request through the TLS stream
    let mut buffer = vec![0u8; 64 * 1024];
    let bytes_read = match tokio::time::timeout(
        std::time::Duration::from_secs(30),
        tls_stream.read(&mut buffer),
    )
    .await
    {
        Ok(Ok(n)) => n,
        Ok(Err(e)) => {
            warn!("TLS read error from {}: {}", peer_addr, e);
            return Ok(());
        }
        Err(_) => {
            warn!("TLS read timeout from {}", peer_addr);
            return Ok(());
        }
    };

    if bytes_read == 0 {
        return Ok(());
    }

    let request_text = String::from_utf8_lossy(&buffer[..bytes_read]);
    let parsed = match parse_http_request(&request_text) {
        Ok(p) => p,
        Err(e) => {
            tls_write_http_json(
                tls_stream,
                400,
                serde_json::json!({"error": format!("Invalid HTTP request: {}", e)}),
                "",
            )
            .await?;
            return Ok(());
        }
    };

    // Compute CORS headers
    let cors_headers =
        super::http::compute_cors_response_headers(parsed.header_part, server.as_ref());

    // Route the request
    if parsed.method == "OPTIONS" {
        tls_write_http_json(
            tls_stream,
            200,
            serde_json::json!({"ok": true}),
            &cors_headers,
        )
        .await?;
        return Ok(());
    }

    // ── SSE streaming endpoints over TLS ─────────────────────────────
    if parsed.path == "/chat/stream"
        || parsed.path == "/v1/chat/completions"
        || parsed.path == "/v1/responses"
    {
        let body_value: serde_json::Value = match serde_json::from_str(parsed.body_initial_part) {
            Ok(v) => v,
            Err(_) => {
                tls_write_http_json(
                    tls_stream,
                    400,
                    serde_json::json!({"error": "Invalid JSON body"}),
                    &cors_headers,
                )
                .await?;
                return Ok(());
            }
        };

        write_sse_headers(tls_stream, &cors_headers).await?;

        let mut params: crate::acp::r#impl::chat::ChatParams =
            match serde_json::from_value(body_value) {
                Ok(p) => p,
                Err(e) => {
                    write_sse_event(
                        tls_stream,
                        "error",
                        &serde_json::json!({"message": format!("Invalid chat params: {}", e)}),
                    )
                    .await?;
                    let _ = tls_stream.shutdown().await;
                    return Ok(());
                }
            };

        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        let trace = http_trace_context("chat.stream");
        let server_ref = Arc::clone(server);
        let sse_tx = tx.clone();
        let task = tokio::spawn(async move {
            if let Err(err) = crate::acp::r#impl::chat::process_chat_request(
                server_ref.as_ref(),
                &mut params,
                Some(crate::acp::r#impl::chat::StreamObserver::sse(tx)),
                &trace,
                None,
                None,
            )
            .await
            {
                let _ = sse_tx
                    .send(crate::acp::r#impl::chat::streaming::StreamFrame {
                        event: "error",
                        payload: serde_json::json!({
                            "error": err.to_string(),
                        }),
                    })
                    .await;
            }
        });

        while let Some(frame) = rx.recv().await {
            if let Err(err) = write_sse_event(tls_stream, frame.event, &frame.payload).await {
                task.abort();
                warn!("TLS SSE write error: {}", err);
                let _ = tls_stream.shutdown().await;
                return Ok(());
            }
        }

        // The spawned task has already sent any error events via the SSE channel.
        if let Err(join_err) = task.await {
            write_sse_event(
                tls_stream,
                "error",
                &serde_json::json!({"message": format!("task panicked: {}", join_err)}),
            )
            .await?;
        }

        let _ = tls_stream.shutdown().await;
        return Ok(());
    }

    // GET requests: health and root capabilities
    if parsed.method == "GET" {
        match parsed.path {
            "/health" => {
                tls_write_http_json(
                    tls_stream,
                    200,
                    serde_json::json!({"status": "ok"}),
                    &cors_headers,
                )
                .await?;
            }
            "/" | "/capabilities" => {
                let caps = build_root_capabilities_response();
                tls_write_http_json(tls_stream, 200, caps, &cors_headers).await?;
            }
            _ => {
                tls_write_http_json(
                    tls_stream,
                    404,
                    serde_json::json!({"error": "Not Found"}),
                    &cors_headers,
                )
                .await?;
            }
        }
        return Ok(());
    }

    // POST requests: route to the appropriate handler
    if parsed.method == "POST" {
        if parsed.path == "/rpc" {
            let body = parsed.body_initial_part;
            match serde_json::from_str::<JsonRpcRequest>(body) {
                Ok(request) => {
                    let response_value = route_rpc_over_tls(server.as_ref(), request).await;
                    let response_bytes = serde_json::to_vec(&response_value).unwrap_or_default();
                    let status_text = "OK";
                    let headers = format!(
                        "HTTP/1.1 200 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n",
                        status_text,
                        response_bytes.len(),
                        cors_headers
                    );
                    tls_stream.write_all(headers.as_bytes()).await?;
                    tls_stream.write_all(&response_bytes).await?;
                    let _ = tls_stream.shutdown().await;
                }
                Err(e) => {
                    tls_write_http_json(
                        tls_stream,
                        400,
                        serde_json::json!({
                            "error": "Invalid JSON-RPC request",
                            "detail": e.to_string(),
                        }),
                        &cors_headers,
                    )
                    .await?;
                }
            }
        } else if parsed.path == "/chat" {
            tls_write_http_json(
                tls_stream,
                501,
                serde_json::json!({
                    "error": "Not Implemented",
                    "message": "Chat endpoint over TLS is not yet fully supported.",
                    "code": "TLS_CHAT_NOT_SUPPORTED",
                }),
                &cors_headers,
            )
            .await?;
        } else {
            tls_write_http_json(
                tls_stream,
                404,
                serde_json::json!({"error": "Not Found"}),
                &cors_headers,
            )
            .await?;
        }
        return Ok(());
    }

    tls_write_http_json(
        tls_stream,
        405,
        serde_json::json!({"error": "Method Not Allowed"}),
        &cors_headers,
    )
    .await
}

/// Handle an HTTP connection secured by mTLS.
///
/// Performs TLS handshake using MtlsAcceptor, then delegates to the
/// shared HTTP routing logic.
pub(crate) async fn handle_mtls_http_connection(
    mtls_acceptor: &crate::security::mtls::MtlsAcceptor,
    socket: TcpStream,
    server: Arc<AcpServer>,
    peer_addr: SocketAddr,
) -> Result<()> {
    let (mut tls_stream, _cn) = match mtls_acceptor.accept(socket).await {
        Ok((stream, cn)) => {
            if cn != "unknown" {
                tracing::trace!("mTLS client CN: {} from {}", cn, peer_addr);
            }
            (stream, cn)
        }
        Err(e) => {
            warn!("mTLS handshake failed for {}: {}", peer_addr, e);
            return Ok(());
        }
    };

    handle_tls_http_stream(&mut tls_stream, &server, peer_addr).await
}

/// Handle an HTTP connection secured by plain TLS (non-mTLS).
///
/// Performs TLS handshake (without client cert), then delegates to the
/// shared HTTP routing logic.
pub(crate) async fn handle_tls_http_connection(
    tls_acceptor: &tokio_rustls::TlsAcceptor,
    socket: TcpStream,
    server: Arc<AcpServer>,
    peer_addr: SocketAddr,
) -> Result<()> {
    let tls_stream = match tls_acceptor.accept(socket).await {
        Ok(stream) => stream,
        Err(e) => {
            warn!("TLS handshake failed for {}: {}", peer_addr, e);
            return Ok(());
        }
    };
    let mut tls_stream = tokio_rustls::TlsStream::Server(tls_stream);

    handle_tls_http_stream(&mut tls_stream, &server, peer_addr).await
}
