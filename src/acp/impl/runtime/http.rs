//! HTTP server routing, response writing, and CORS handling
//!
//! Contains the main HTTP connection handler, request routing (GET/POST),
//! JSON response writing, CORS preflight and header computation.
//! Extracted from the parent `runtime.rs` to reduce the monolithic file size.

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
use crate::acp::transport::{set_current_transport, RpcBufferTransport, SseTransport};
use crate::core::error::error_code_from_status;
use crate::i18n::runtime::{t, tf};
use crate::rpc_protocol::{chat_trace_context, JsonRpcRequest, RequestTraceContext};

/// Clone a [`tokio::net::TcpStream`] by duplicating the underlying file descriptor.
///
/// Tokio's `TcpStream` does not implement `Clone` or `TryClone`, so we use
/// `std::net::TcpStream::try_clone` via the raw file descriptor.
pub(crate) fn clone_tcp_stream(
    socket: &tokio::net::TcpStream,
) -> std::io::Result<tokio::net::TcpStream> {
    use std::os::unix::io::{AsRawFd, FromRawFd};
    let raw_fd = socket.as_raw_fd();
    // Safety: the original socket keeps the fd alive while we temporarily wrap it.
    let std_socket = unsafe { std::net::TcpStream::from_raw_fd(raw_fd) };
    let cloned = std_socket.try_clone()?;
    // Forget the wrapper so the fd is not closed when it drops.
    std::mem::forget(std_socket);
    tokio::net::TcpStream::from_std(cloned)
}

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
                    let mut params: crate::acp::r#impl::chat::ChatParams =
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
                        &mut params,
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
                    fn handle_chat_stream_task_error(
                        sse_tx: &tokio::sync::mpsc::UnboundedSender<
                            crate::acp::r#impl::chat::streaming::StreamFrame,
                        >,
                        err: anyhow::Error,
                    ) {
                        let err_str = err.to_string();
                        // Send an "error" event (not "done") so the GUI correctly
                        // treats this as a failure instead of overwriting the
                        // previous "done" event from agent streaming with
                        // an empty response + "system" agent.
                        // The TLS handler already does this correctly.
                        let _ = sse_tx.send(crate::acp::r#impl::chat::streaming::StreamFrame {
                            event: "error",
                            payload: serde_json::json!({
                                "error": err_str,
                            }),
                            status: None,
                        });
                    }
                    let mut params: crate::acp::r#impl::chat::ChatParams =
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
                    use super::sse::{flush_sse, write_sse_event, write_sse_headers};
                    write_sse_headers(socket, cors_headers).await?;
                    let _ = set_current_transport(Arc::new(SseTransport::new(clone_tcp_stream(socket)?)));

                    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
                    let trace = http_trace_context("chat.stream");
                    let ctx = Some(crate::acp::r#impl::chat::ChatRequestContext::new(
                        user_session,
                    ));
                    let server_ref = Arc::clone(&server);
                    let sse_tx = tx.clone();
                    let task = tokio::spawn(async move {
                        if let Err(err) = crate::acp::r#impl::chat::process_chat_request(
                            server_ref.as_ref(),
                            &mut params,
                            Some(crate::acp::r#impl::chat::StreamObserver::sse(tx)),
                            &trace,
                            None,
                            ctx,
                        )
                        .await
                        {
                            handle_chat_stream_task_error(&sse_tx, err);
                        }
                    });

                    // Inactivity timeout for the chat stream.
                    // If no events arrive within the timeout window (e.g. pipeline hang
                    // during long-running tool execution), abort and return error.
                    // The timeout resets on each received event, so long-running tool
                    // chains that produce periodic progress events are fine.
                    const SSE_FLUSH_INTERVAL: usize = 4;
                    const STREAM_INACTIVITY_TIMEOUT_SECS: u64 = 120;
                    let mut sse_event_count: usize = 0;

                    let stream_timeout =
                        tokio::time::sleep(std::time::Duration::from_secs(STREAM_INACTIVITY_TIMEOUT_SECS));
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
                                        sse_event_count += 1;
                                        // Reset inactivity timer on each received event.
                                        stream_timeout.as_mut().reset(
                                            tokio::time::Instant::now() +
                                            std::time::Duration::from_secs(STREAM_INACTIVITY_TIMEOUT_SECS)
                                        );
                                        // Periodic flush: every SSE_FLUSH_INTERVAL events.
                                        // This batches syscalls while keeping latency low.
                                        if sse_event_count.is_multiple_of(SSE_FLUSH_INTERVAL) {
                                            let _ = flush_sse(socket).await;
                                        }
                                    }
                                    None => break,
                                }
                            }
                            _ = &mut stream_timeout => {
                                task.abort();
                                let payload = serde_json::json!({"error": t("error.chat.stream_timeout")});
                                let _ = write_sse_event(socket, "error", &payload).await;
                                let _ = flush_sse(socket).await;
                                return Ok(());
                            }
                        }
                    }

                    // Final flush after all events are sent.
                    let _ = flush_sse(socket).await;

                    // The spawned task has already sent any error events via the SSE channel.
                    // Await the task to ensure it finishes, but errors are already handled.
                    if let Err(join_err) = task.await {
                        let payload = inject_platform_profiles_if_absent(
                            serde_json::json!({"message": format!("chat task panicked: {join_err}")}),
                            "chat",
                        );
                        let _ = write_sse_event(socket, "error", &payload).await;
                        let _ = flush_sse(socket).await;
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
                    let request: JsonRpcRequest = match serde_json::from_value(body) {
                        Ok(r) => r,
                        Err(e) => {
                            write_http_json_response_with_context(
                                socket,
                                400,
                                serde_json::json!({"error": format!("{}: {}", t("error.invalid_request"), e)}),
                                path,
                                cors_headers,
                            )
                            .await?;
                            return Ok(());
                        }
                    };

                    // Per-request output buffer via Transport trait.
                    // RpcBufferTransport captures JSON-RPC output into this buffer;
                    // after handle_request completes, the buffer contains the response body.
                    let buffer = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
                    let transport_buffer = buffer.clone();
                    let headers_owned = header_part.to_string();
                    let server_ref = Arc::clone(&server);

                    let _ = set_current_transport(Arc::new(RpcBufferTransport::new(transport_buffer)));
                    let rpc_result = handle_request(server_ref.as_ref(), request, Some(&headers_owned)).await;

                    if let Err(err) = &rpc_result {
                        write_http_json_response_with_context(
                            socket,
                            500,
                            serde_json::json!({"error": format!("{}: {}", t("error.internal_server"), err)}),
                            path,
                            cors_headers,
                        )
                        .await?;
                        return Ok(());
                    }

                    let response_bytes = buffer.lock().await.clone();
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

                    // Check for __text_plain__ sentinel key — serve as text/plain
                    if let Some(text) = response_value
                        .get("result")
                        .and_then(|r| r.get("__text_plain__"))
                        .and_then(|v| v.as_str())
                    {
                        write_http_text_response(socket, 200, text, cors_headers).await?;
                    } else {
                        write_http_json_response(socket, 200, response_value, cors_headers).await?;
                    }
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
                serde_json::json!({"error": t("error.method_not_allowed")}),
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
    mut value: serde_json::Value,
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
    // Inject machine-readable error code into error responses
    if status >= 400 {
        if let Some(obj) = value.as_object_mut() {
            if obj.contains_key("error") && !obj.contains_key("code") {
                if let Some(code) = error_code_from_status(status) {
                    obj.insert("code".to_string(), serde_json::json!(code));
                }
            }
        }
    }
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

/// Write HTTP text/plain response.
///
/// Used when the JSON-RPC result contains a `__text_plain__` sentinel key,
/// instructing the HTTP transport to serve the value as text/plain instead of
/// the default application/json.
pub(crate) async fn write_http_text_response(
    socket: &mut TcpStream,
    status: u16,
    text: &str,
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
    let body = text.as_bytes();
    let headers = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n",
        status,
        status_text,
        body.len(),
        extra_headers
    );
    tcp_write_timeout(socket, headers.as_bytes()).await?;
    tcp_write_timeout(socket, body).await?;
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
