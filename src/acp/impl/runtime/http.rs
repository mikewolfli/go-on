//! HTTP server routing, response writing, and CORS handling
//!
//! Contains the main HTTP connection handler, request routing (GET/POST),
//! JSON response writing, CORS preflight and header computation.
//! Extracted from the parent `runtime.rs` to reduce the monolithic file size.

use std::mem;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::{debug, info};

use crate::acp::r#impl::cors::{
    build_cors_headers, build_preflight_response_headers, is_origin_allowed,
};
use crate::acp::r#impl::request::{handle_request, inject_platform_profiles_if_absent};
use crate::acp::server::AcpServer;
use crate::i18n::runtime::{t, tf};
use crate::rpc_protocol::{chat_trace_context, JsonRpcRequest, RequestTraceContext};

use super::openai_compat::{
    build_openai_models_response, extract_response_id_from_path, handle_openai_chat_completions,
    handle_response_get, handle_responses_api, list_responses_api_payloads,
};
use super::protocol::{extract_content_length, parse_http_request};
use super::security::{check_http_authorization, http_entry_guard};
use super::sse::write_sse_event;
use super::sse::write_sse_headers;
use super::tcp_write_timeout;
use super::tls::build_root_capabilities_response;
use super::RPC_SERIAL;

/// Main HTTP connection handler — parses, guards, routes, and times the request.
pub(crate) async fn handle_http_connection(
    socket: &mut TcpStream,
    server: Arc<AcpServer>,
    peer_addr: SocketAddr,
) -> Result<()> {
    let mut buffer = vec![0u8; 64 * 1024];
    let bytes_read =
        tokio::time::timeout(std::time::Duration::from_secs(30), socket.read(&mut buffer))
            .await
            .map_err(|_| anyhow::anyhow!("timeout reading HTTP request"))??;
    if bytes_read == 0 {
        return Ok(());
    }

    let request_text = String::from_utf8_lossy(&buffer[..bytes_read]);
    let parsed = parse_http_request(&request_text)?;

    // Compute CORS headers for this request (empty string when disabled)
    let cors_headers = compute_cors_response_headers(parsed.header_part, server.as_ref());

    // Extract user session if user auth is enabled
    let user_session: Option<crate::acp::r#impl::session::UserSession> =
        server.session.session_manager.as_ref().and_then(|sm| {
            let session = sm.extract_user_from_request(parsed.header_part);
            if let Some(ref s) = session {
                debug!("Authenticated user: {} (roles: {:?})", s.user_id, s.roles);
            }
            session
        });

    // ── RBAC authorization check ──────────────────────────────
    if check_http_authorization(
        socket,
        server.as_ref(),
        user_session.as_ref(),
        parsed.method,
        parsed.path,
        &cors_headers,
    )
    .await?
    {
        return Ok(());
    }

    if parsed.method == "OPTIONS" {
        return handle_cors_preflight(socket, parsed.header_part, server.as_ref()).await;
    }

    if http_entry_guard(
        socket,
        server.as_ref(),
        parsed.header_part,
        parsed.method,
        parsed.path,
        peer_addr,
        &cors_headers,
    )
    .await?
    {
        return Ok(());
    }

    if parsed.method == "GET" {
        return route_http_get(socket, server.as_ref(), parsed.path, &cors_headers).await;
    }

    if parsed.method != "POST" {
        write_http_json_response_with_context(
            socket,
            405,
            serde_json::json!({"error": t("error.method_not_allowed")}),
            "chat",
            &cors_headers,
        )
        .await?;
        return Ok(());
    }

    let _path_label = route_http_post(
        socket,
        server,
        parsed.path,
        parsed.header_part,
        parsed.body_initial_part,
        user_session,
        &cors_headers,
    )
    .await?;

    Ok(())
}

/// Route an HTTP GET request based on the path and write the response back to the socket.
async fn route_http_get(
    socket: &mut TcpStream,
    server: &AcpServer,
    path: &str,
    cors_headers: &str,
) -> Result<()> {
    match path {
        "/metrics" => {
            let prometheus =
                crate::observability::metrics_exporter::build_prometheus_metrics(server).await;
            // Write Prometheus text format directly
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\n{}\r\n\r\n{}",
                prometheus.len(),
                cors_headers,
                prometheus
            );
            socket.write_all(response.as_bytes()).await?;
        }
        "/health" => {
            write_http_json_response_with_context(
                socket,
                200,
                serde_json::to_value(server.get_status())?,
                "health",
                cors_headers,
            )
            .await?;
        }
        "/health/ready" => {
            if server.drain_guard.is_draining() {
                write_http_json_response_with_context(
                    socket,
                    503,
                    serde_json::json!({
                        "ok": false,
                        "status": "draining",
                        "message": "Server is shutting down"
                    }),
                    "health",
                    cors_headers,
                )
                .await?;
            } else {
                write_http_json_response_with_context(
                    socket,
                    200,
                    serde_json::json!({
                        "ok": true,
                        "status": "ready",
                        "healthy": server.is_healthy(),
                    }),
                    "health",
                    cors_headers,
                )
                .await?;
            }
        }
        "/v1/responses" => {
            let data = list_responses_api_payloads(server);
            write_http_json_response_with_context(
                socket,
                200,
                serde_json::json!({
                    "object": "list",
                    "data": data,
                }),
                "responses.api",
                cors_headers,
            )
            .await?;
        }
        "/v1/models" | "/v1/model" | "/models" => {
            write_http_json_response_with_context(
                socket,
                200,
                build_openai_models_response(),
                "openai.chat.completions",
                cors_headers,
            )
            .await?;
        }
        "/v1/state/events" => {
            handle_state_events_sse(socket, cors_headers).await?;
        }
        "/protocol/version" => {
            use crate::schema::ProtocolVersion;
            let versions: Vec<u16> = ProtocolVersion::supported_versions()
                .iter()
                .map(|v| v.as_u16())
                .collect();
            write_http_json_response_with_context(
                socket,
                200,
                serde_json::json!({
                    "supported_versions": versions,
                    "latest": ProtocolVersion::LATEST.as_u16(),
                    "server": "go-on"
                }),
                "protocol.version",
                cors_headers,
            )
            .await?;
        }
        "/" => {
            write_http_json_response_with_context(
                socket,
                200,
                build_root_capabilities_response(),
                "initialize",
                cors_headers,
            )
            .await?;
        }
        _ if extract_response_id_from_path(path).is_some() => {
            let response_id = extract_response_id_from_path(path).ok_or_else(|| {
                anyhow::anyhow!("response_id extraction failed despite prior is_some check")
            })?;
            handle_response_get(socket, server, response_id, cors_headers).await?;
        }
        _ => {
            write_http_json_response_with_context(
                socket,
                404,
                serde_json::json!({"error": t("error.not_found")}),
                "chat",
                cors_headers,
            )
            .await?;
        }
    }
    Ok(())
}

/// SSE handler for `/v1/state/events` — streams state sync events to connected clients.
///
/// Writes SSE headers, then enters a loop subscribing to the `StateSyncBroadcaster`.
/// On each event, serializes and sends the event as an SSE frame. On disconnect,
/// the socket write will fail and the loop exits gracefully.
async fn handle_state_events_sse(socket: &mut TcpStream, cors_headers: &str) -> Result<()> {
    write_sse_headers(socket, cors_headers).await?;

    let mut rx = crate::protocol::state_sync::subscribe();
    let mut heartbeat_interval = tokio::time::interval(std::time::Duration::from_secs(30));

    loop {
        tokio::select! {
            result = rx.recv() => {
                match result {
                    Ok(event) => {
                        let payload = serde_json::to_value(&event)?;
                        if let Err(e) = write_sse_event(socket, "state_sync", &payload).await {
                            // Client disconnected — stop streaming
                            debug!("state sync SSE client disconnected: {}", e);
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        // Consumer fell behind; log warning and continue
                        debug!("state sync SSE consumer lagged by {} events", n);
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        // Broadcaster was closed — stop
                        break;
                    }
                }
            }
            _ = heartbeat_interval.tick() => {
                let heartbeat = crate::protocol::state_sync::StateSyncEvent::Heartbeat {
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64,
                };
                let payload = serde_json::to_value(&heartbeat)?;
                if write_sse_event(socket, "state_sync", &payload).await.is_err() {
                    break;
                }
            }
        }
    }

    let _ = socket.shutdown().await;
    Ok(())
}

/// Route a POST request — reads body, dispatches to the appropriate handler,
/// and writes the response to the socket. Returns the path label for logging.
///
/// `body_initial_part` is the portion of the body already in the initial buffer read.
#[allow(clippy::question_mark)]
// Intentional — early return for the !path check and JSON parse error below,
// where we write an error response to the socket before returning Ok(path).
// Using `?` would propagate the error upward without writing the response.
async fn route_http_post(
    socket: &mut TcpStream,
    server: Arc<AcpServer>,
    path: &str,
    header_part: &str,
    body_initial_part: &str,
    user_session: Option<crate::acp::r#impl::session::UserSession>,
    cors_headers: &str,
) -> Result<String> {
    let responses_path = path == "/v1/responses";
    let content_length = extract_content_length(header_part).unwrap_or(0);
    if content_length == 0 {
        if responses_path {
            write_http_json_response_with_context(
                socket,
                400,
                serde_json::json!({
                    "error": {
                        "code": "missing_required_field",
                        "type": "invalid_request_error",
                        "message": t("error.body_required"),
                    }
                }),
                "responses.api",
                cors_headers,
            )
            .await?;
        } else {
            write_http_json_response_with_context(
                socket,
                400,
                serde_json::json!({"error": t("error.body_required")}),
                "chat",
                cors_headers,
            )
            .await?;
        }
        return Ok(path.to_string());
    }

    let mut body_bytes = body_initial_part.as_bytes().to_vec();
    if body_bytes.len() < content_length {
        let mut remaining = vec![0u8; content_length - body_bytes.len()];
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            socket.read_exact(&mut remaining),
        )
        .await
        .map_err(|_| anyhow::anyhow!("timeout reading HTTP body"))?
        .map_err(|e| anyhow::anyhow!("HTTP body read error: {e}"))?;
        body_bytes.extend_from_slice(&remaining);
    }
    body_bytes.truncate(content_length);

    // Enforce max body size (10MB)
    const MAX_BODY_SIZE: usize = 10 * 1024 * 1024;
    if body_bytes.len() > MAX_BODY_SIZE {
        anyhow::bail!(
            "HTTP body too large: {} bytes (max {})",
            body_bytes.len(),
            MAX_BODY_SIZE
        );
    }

    let body: serde_json::Value = match serde_json::from_slice(&body_bytes) {
        Ok(value) => value,
        Err(err) => {
            if responses_path {
                write_http_json_response_with_context(
                    socket,
                    400,
                    serde_json::json!({
                        "error": {
                            "code": "invalid_request_error",
                            "type": "invalid_request_error",
                            "message": tf("error.invalid_json", &[("error", &err.to_string())]),
                        }
                    }),
                    "responses.api",
                    cors_headers,
                )
                .await?;
            } else {
                write_http_json_response_with_context(
                    socket,
                    400,
                    serde_json::json!({"error": tf("error.invalid_json", &[("error", &err.to_string())])}),
                    "chat",
                    cors_headers,
                )
                .await?;
            }
            return Ok(path.to_string());
        }
    };

    let (dispatch_result, duration) =
        crate::observability::performance::utils::measure_time_async(move || async move {
            match path {
                "/chat" => {
                    let params: crate::acp::r#impl::chat::ChatParams =
                        match serde_json::from_value(body) {
                            Ok(value) => value,
                            Err(err) => {
                                write_http_json_response_with_context(
                                socket,
                                400,
                                serde_json::json!({"error": tf("error.invalid_chat_params", &[("error", &err.to_string())])}),
                                "chat",
                                cors_headers,
                            )
                            .await?;
                                return Ok(());
                            }
                        };
                    let trace = http_trace_context("chat");
                    let ctx = Some(crate::acp::r#impl::chat::ChatRequestContext::new(
                        user_session,
                    ));
                    let result = match crate::acp::r#impl::chat::process_chat_request(
                        server.as_ref(),
                        &params,
                        None,
                        &trace,
                        None,
                        ctx,
                    )
                    .await
                    {
                        Ok(result) => result,
                        Err(err) => {
                            write_http_json_response_with_context(
                                socket,
                                502,
                                serde_json::json!({
                                    "error": {
                                        "message": err.to_string(),
                                        "type": "go_on_upstream_error"
                                    }
                                }),
                                "chat",
                                cors_headers,
                            )
                            .await?;
                            return Ok(());
                        }
                    };
                    let result = inject_platform_profiles_if_absent(result, "chat");
                    write_http_json_response(socket, 200, result, cors_headers).await?;
                }
                "/chat/stream" => {
                    let params: crate::acp::r#impl::chat::ChatParams =
                        match serde_json::from_value(body) {
                            Ok(value) => value,
                            Err(err) => {
                                write_http_json_response_with_context(
                                socket,
                                400,
                                serde_json::json!({"error": tf("error.invalid_chat_params", &[("error", &err.to_string())])}),
                                "chat",
                                cors_headers,
                            )
                            .await?;
                                return Ok(());
                            }
                        };
                    use super::sse::{write_sse_event, write_sse_headers};
                    write_sse_headers(socket, cors_headers).await?;

                    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
                    let trace = http_trace_context("chat.stream");
                    let ctx = Some(crate::acp::r#impl::chat::ChatRequestContext::new(
                        user_session,
                    ));
                    let server_ref = Arc::clone(&server);
                    let task = tokio::spawn(async move {
                        crate::acp::r#impl::chat::process_chat_request(
                            server_ref.as_ref(),
                            &params,
                            Some(crate::acp::r#impl::chat::StreamObserver::sse(tx)),
                            &trace,
                            None,
                            ctx,
                        )
                        .await
                    });

                    // Add a 30-second overall timeout for the chat stream.
                    // If no events arrive (e.g., pipeline hang), abort and return error.
                    let stream_timeout = tokio::time::sleep(std::time::Duration::from_secs(30));
                    tokio::pin!(stream_timeout);
                    loop {
                        tokio::select! {
                            frame = rx.recv() => {
                                match frame {
                                    Some(frame) => {
                                        if let Err(err) = write_sse_event(socket, frame.event, &frame.payload).await {
                                            task.abort();
                                            return Err(err);
                                        }
                                    }
                                    None => break, // channel closed
                                }
                            }
                            _ = &mut stream_timeout => {
                                task.abort();
                                let payload = serde_json::json!({"error": "chat stream timed out after 30s"});
                                let _ = write_sse_event(socket, "error", &payload).await;
                                return Ok(());
                            }
                        }
                    }

                    match task.await {
                        Ok(Ok(result)) => {
                            let result = inject_platform_profiles_if_absent(result, "chat");
                            write_sse_event(socket, "result", &result).await?
                        }
                        Ok(Err(err)) => {
                            let payload = inject_platform_profiles_if_absent(
                                serde_json::json!({"message": err.to_string()}),
                                "chat",
                            );
                            write_sse_event(socket, "error", &payload).await?
                        }
                        Err(err) => {
                            let payload = inject_platform_profiles_if_absent(
                                serde_json::json!({"message": format!("chat task panicked: {err}")}),
                                "chat",
                            );
                            write_sse_event(socket, "error", &payload).await?
                        }
                    }
                }
                "/chat/completions" | "/v1/chat/completions" | "/chat/chat/completions" => {
                    handle_openai_chat_completions(
                        socket,
                        Arc::clone(&server),
                        body,
                        user_session,
                        cors_headers,
                    )
                    .await?;
                }
                "/" | "/rpc" => {
                    // SERIALIZED: Only one RPC call at a time.
                    let _rpc_guard = RPC_SERIAL.lock().await;

                    let request: JsonRpcRequest = match serde_json::from_value(body) {
                        Ok(r) => r,
                        Err(e) => {
                            write_http_json_response_with_context(
                                socket,
                                400,
                                serde_json::json!({"error": format!("invalid RPC request: {}", e)}),
                                path,
                                cors_headers,
                            )
                            .await?;
                            return Ok(());
                        }
                    };

                    let (pipe_writer, mut pipe_reader) = tokio::io::duplex(10 * 1024 * 1024);

                    // Temporarily swap stdout with the pipe writer
                    {
                        let mut guard = server.output.lock().await;
                        let _ = mem::replace(&mut *guard, Box::new(pipe_writer));
                    }

                    let server_ref = Arc::clone(&server);
                    let headers_owned = header_part.to_string();
                    let rpc_task = tokio::spawn(async move {
                        handle_request(server_ref.as_ref(), request, Some(&headers_owned)).await
                    });

                    let rpc_result = rpc_task.await;

                    // Wait for the RPC task first, THEN restore stdout.
                    // This drops the pipe writer BEFORE we read, so
                    // read_to_end can complete (otherwise it would block
                    // indefinitely waiting for the pipe to close).
                    // Restore stdout
                    {
                        let mut guard = server.output.lock().await;
                        let _ = mem::replace(
                            &mut *guard,
                            Box::new(tokio::io::stdout()) as Box<dyn tokio::io::AsyncWrite + Send + Unpin>,
                        );
                    }

                    let mut response_bytes = Vec::new();
                    let read_result = tokio::time::timeout(
                        std::time::Duration::from_secs(60),
                        pipe_reader.read_to_end(&mut response_bytes),
                    ).await;

                    match rpc_result {
                        Err(join_err) => {
                            write_http_json_response_with_context(
                                socket,
                                500,
                                serde_json::json!({"error": format!("RPC task panicked: {}", join_err)}),
                                path,
                                cors_headers,
                            )
                            .await?;
                            return Ok(());
                        }
                        Ok(Err(err)) => {
                            write_http_json_response_with_context(
                                socket,
                                500,
                                serde_json::json!({"error": format!("RPC dispatch error: {}", err)}),
                                path,
                                cors_headers,
                            )
                            .await?;
                            return Ok(());
                        }
                        Ok(Ok(())) => {}
                    }

                    read_result
                        .map_err(|_| anyhow::anyhow!("timeout reading RPC pipe response"))?
                        .map_err(|e| anyhow::anyhow!("RPC pipe read error: {e}"))?;

                    let response_str = String::from_utf8_lossy(&response_bytes);
                    let response_value: serde_json::Value = {
                        let mut last_response =
                            serde_json::json!({"raw": response_str.to_string()});
                        for line in response_str.lines() {
                            let trimmed = line.trim();
                            if trimmed.is_empty() {
                                continue;
                            }
                            if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
                                if val.get("id").is_some() {
                                    last_response = val;
                                }
                            }
                        }
                        last_response
                    };

                    write_http_json_response(socket, 200, response_value, cors_headers).await?;
                }
                "/v1/responses" => {
                    handle_responses_api(
                        socket,
                        Arc::clone(&server),
                        body,
                        user_session,
                        cors_headers,
                    )
                    .await?;
                }
                _ => {
                    write_http_json_response_with_context(
                        socket,
                        404,
                        serde_json::json!({"error": t("error.not_found")}),
                        "chat",
                        cors_headers,
                    )
                    .await?;
                }
            }

            Ok(())
        })
        .await;

    let path_label = path.to_string();
    let success = dispatch_result.is_ok();
    crate::observability::performance::record_global_operation(
        success,
        duration.as_secs_f64() * 1000.0,
    );
    info!(
        "HTTP {} completed in {:?} (ok={})",
        path_label, duration, success,
    );

    if let Err(e) = dispatch_result {
        return Err(e);
    }
    Ok(path_label)
}

/// Compute CORS response headers for an incoming request.
pub(crate) fn compute_cors_response_headers(headers: &str, server: &AcpServer) -> String {
    let config = match server.runtime_config.cors_config() {
        Some(c) => c,
        None => return String::new(),
    };
    let origin = extract_header_value(headers, "origin");
    let cors_headers = build_cors_headers(origin.as_deref(), &config);
    if cors_headers.is_empty() {
        return String::new();
    }
    cors_headers
        .iter()
        .map(|(k, v)| format!("{}: {}\r\n", k, v))
        .collect()
}

/// Handle an OPTIONS (CORS preflight) request.
async fn handle_cors_preflight(
    socket: &mut TcpStream,
    headers: &str,
    server: &AcpServer,
) -> Result<()> {
    let config = match server.runtime_config.cors_config() {
        Some(c) => c,
        None => {
            write_http_json_response(
                socket,
                405,
                serde_json::json!({"error": "Method Not Allowed"}),
                "",
            )
            .await?;
            return Ok(());
        }
    };
    let origin = extract_header_value(headers, "origin");
    let allow_origin = origin.as_deref().filter(|o| is_origin_allowed(o, &config));

    if allow_origin.is_none() && !config.allowed_origins.contains(&"*".to_string()) {
        write_http_json_response(
            socket,
            403,
            serde_json::json!({"error": "Origin not allowed"}),
            "",
        )
        .await?;
        return Ok(());
    }

    let rh = extract_header_value(headers, "access-control-request-headers");
    let preflight_headers = build_preflight_response_headers(rh.as_deref(), &config);
    let origin_val = allow_origin.unwrap_or("*").to_string();

    let mut cors_str = format!("Access-Control-Allow-Origin: {}\r\n", origin_val);
    for (k, v) in &preflight_headers {
        cors_str.push_str(&format!("{}: {}\r\n", k, v));
    }
    cors_str.push_str("Access-Control-Max-Age: ");
    cors_str.push_str(&config.max_age_seconds.to_string());
    cors_str.push_str("\r\n");

    write_http_json_response(socket, 200, serde_json::json!({"ok": true}), &cors_str).await?;
    Ok(())
}

/// Write a standard HTTP JSON response. Thin wrapper for consistency.
#[allow(dead_code)] // F-GAP-49 — planned wiring: lifecycle/utility
async fn write_http_response(
    socket: &mut TcpStream,
    status: u16,
    body: serde_json::Value,
) -> Result<()> {
    write_http_json_response(socket, status, body, "").await
}

/// Write an HTTP JSON response with platform profiles injected.
pub(crate) async fn write_http_json_response_with_context(
    socket: &mut TcpStream,
    status: u16,
    body: serde_json::Value,
    method: &str,
    extra_headers: &str,
) -> Result<()> {
    let body = inject_platform_profiles_if_absent(body, method);
    write_http_json_response(socket, status, body, extra_headers).await
}

/// Write a raw HTTP JSON response to a TcpStream.
pub(crate) async fn write_http_json_response(
    socket: &mut TcpStream,
    status: u16,
    value: serde_json::Value,
    extra_headers: &str,
) -> Result<()> {
    let status_text = match status {
        200 => "OK",
        401 => "Unauthorized",
        429 => "Too Many Requests",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
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
    tcp_write_timeout(socket, headers.as_bytes()).await?;
    tcp_write_timeout(socket, &body).await?;
    let _ = socket.shutdown().await;
    Ok(())
}

/// Build a trace context for HTTP requests.
pub(crate) fn http_trace_context(method: &str) -> RequestTraceContext {
    let request_id = format!("http-{}", crate::acp::prelude::now_ts_ms());
    let seed = Some(serde_json::json!(request_id.clone()));
    let mut trace = chat_trace_context(&seed, "chat.http");
    trace.method = method.to_string();
    trace.request_id = request_id;
    trace
}

/// Helper to extract a header value from raw headers.
/// Delegates to `protocol::extract_header_value` for consistency.
fn extract_header_value(headers: &str, header_name: &str) -> Option<String> {
    super::protocol::extract_header_value(headers, header_name)
}
