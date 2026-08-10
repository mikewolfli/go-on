//! HTTP server routing, response writing, and CORS handling
//!
//! Contains the main HTTP connection handler, request routing (GET/POST),
//! JSON response writing, CORS preflight and header computation.
//! Extracted from the parent `runtime.rs` to reduce the monolithic file size.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
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

/// A connection stream that is either plaintext TCP or TLS-wrapped TCP.
///
/// Both the ACP plaintext path and the ACP TLS path previously had separate
/// HTTP handlers; this enum lets them share one routing implementation (the
/// TLS arm now enforces the same auth/health/chat/OpenAI behavior as plaintext).
pub(crate) enum HttpStream {
    Plain(tokio::net::TcpStream),
    Tls(Box<tokio_rustls::TlsStream<tokio::net::TcpStream>>),
}

impl tokio::io::AsyncRead for HttpStream {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match &mut *self {
            HttpStream::Plain(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            HttpStream::Tls(s) => std::pin::Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl tokio::io::AsyncWrite for HttpStream {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        match &mut *self {
            HttpStream::Plain(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            HttpStream::Tls(s) => std::pin::Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        match &mut *self {
            HttpStream::Plain(s) => std::pin::Pin::new(s).poll_flush(cx),
            HttpStream::Tls(s) => std::pin::Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        match &mut *self {
            HttpStream::Plain(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            HttpStream::Tls(s) => std::pin::Pin::new(s).poll_shutdown(cx),
        }
    }
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
    socket: &mut HttpStream,
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

    let post_result = route_http_post(
        socket,
        server,
        parsed.path,
        parsed.header_part,
        parsed.body_initial_part,
        user_session,
        &cors_headers,
    )
    .await;

    // Clear the global transport on every path: SseTransport holds an Arc to
    // this request's socket (and RpcBufferTransport holds the response buffer),
    // so leaving it in the global would pin the TCP connection open indefinitely
    // (observed as a hanging /chat/stream response).
    crate::acp::transport::clear_current_transport();

    post_result?;
    Ok(())
}

/// Route an HTTP GET request based on the path and write the response back to the socket.
async fn route_http_get(
    socket: &mut HttpStream,
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
                    "server": "go-on",
                    "server_version": env!("CARGO_PKG_VERSION"),
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
async fn handle_state_events_sse(socket: &mut HttpStream, cors_headers: &str) -> Result<()> {
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

/// Map legacy top-level `model` / `temperature` / `max_tokens` fields
/// (pre-`options.*` SDK payloads) into `options.extra` so the chat pipeline
/// honors them. Explicit `options.*` values take precedence.
fn apply_legacy_chat_top_level_params(params: &mut crate::acp::r#impl::chat::ChatParams) {
    if params.model.is_none() && params.temperature.is_none() && params.max_tokens.is_none() {
        return;
    }
    let opts = params
        .options
        .get_or_insert_with(crate::config::PhaseOptions::default);
    if let Some(model) = params.model.take() {
        opts.extra
            .entry("model".to_string())
            .or_insert(serde_json::Value::String(model));
    }
    if let Some(temperature) = params.temperature.take() {
        opts.extra
            .entry("temperature".to_string())
            .or_insert(serde_json::json!(temperature));
    }
    if let Some(max_tokens) = params.max_tokens.take() {
        opts.extra
            .entry("max_tokens".to_string())
            .or_insert(serde_json::json!(max_tokens));
    }
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
    socket: &mut HttpStream,
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
                    apply_legacy_chat_top_level_params(&mut params);
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
                    apply_legacy_chat_top_level_params(&mut params);
                    use super::sse::{flush_sse, write_sse_event, write_sse_headers};
                    write_sse_headers(socket, cors_headers).await?;
                    // Out-of-band SSE transport requires an fd-cloneable plain TCP
                    // stream; on the TLS arm this global transport is not set
                    // (matches the pre-merge TLS behavior).
                    if let HttpStream::Plain(plain) = socket {
                        set_current_transport(Arc::new(SseTransport::new(clone_tcp_stream(
                            plain,
                        )?)));
                    }

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
                    // it also records the last id-bearing response value directly,
                    // so no re-parse of the serialized buffer is needed.
                    let buffer = std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
                    let transport_buffer = buffer.clone();
                    let headers_owned = header_part.to_string();
                    let server_ref = Arc::clone(&server);

                    let rpc_transport = Arc::new(RpcBufferTransport::new(transport_buffer));
                    set_current_transport(rpc_transport.clone() as Arc<dyn crate::acp::transport::Transport>);
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

                    let response_value = rpc_transport
                        .last_response()
                        .await
                        .unwrap_or_else(|| serde_json::json!({}));

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
    socket: &mut HttpStream,
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
    socket: &mut HttpStream,
    status: u16,
    body: serde_json::Value,
    method: &str,
    extra_headers: &str,
) -> Result<()> {
    let body = inject_platform_profiles_if_absent(body, method);
    write_http_json_response(socket, status, body, extra_headers).await
}

/// Map an HTTP status code to its reason phrase. Single source of truth for
/// the JSON/text writers — the union of the three former copies (ACP HTTP,
/// ACP TLS, MCP HTTP), which had drifted (e.g. 403/500 previously fell back
/// to `"OK"`, producing `HTTP/1.1 403 OK`).
pub(crate) fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "OK",
    }
}

/// Shared body of the JSON response writers. `keep_alive` selects the
/// `Connection` header and the write tail (flush vs shutdown).
async fn write_http_json_inner<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    status: u16,
    mut value: serde_json::Value,
    extra_headers: &str,
    keep_alive: bool,
) -> Result<()> {
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
    // Normalize extra_headers to end with a single CRLF; the format string's
    // trailing CRLF then provides the mandatory blank line before the body.
    // Accepts callers that pass headers with or without the trailing CRLF.
    let extra = if extra_headers.is_empty() {
        String::new()
    } else if extra_headers.ends_with("\r\n") {
        extra_headers.to_string()
    } else {
        format!("{}\r\n", extra_headers)
    };
    let connection = if keep_alive { "keep-alive" } else { "close" };
    // 204 No Content carries no body per HTTP/1.1 §6.4.1.
    let body_len = if status == 204 { 0 } else { body.len() };
    let headers = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: {}\r\n{}\r\n",
        status,
        status_text(status),
        body_len,
        connection,
        extra
    );
    tcp_write_timeout(writer, headers.as_bytes()).await?;
    if body_len > 0 {
        tcp_write_timeout(writer, &body).await?;
    }
    if keep_alive {
        let _ = writer.flush().await;
    } else {
        let _ = writer.shutdown().await;
    }
    Ok(())
}

/// Write a raw HTTP JSON response to any async writer (TcpStream / TLS stream).
/// Uses `Connection: close` and shuts the writer down after the body.
pub(crate) async fn write_http_json_response<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    status: u16,
    value: serde_json::Value,
    extra_headers: &str,
) -> Result<()> {
    write_http_json_inner(writer, status, value, extra_headers, false).await
}

/// Write a raw HTTP JSON response with `Connection: keep-alive` and no
/// shutdown (used by the MCP HTTP arm, which keeps its existing semantics).
pub(crate) async fn write_http_json_response_keep_alive<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    status: u16,
    value: serde_json::Value,
    extra_headers: &str,
) -> Result<()> {
    write_http_json_inner(writer, status, value, extra_headers, true).await
}

/// Write HTTP text/plain response.
///
/// Used when the JSON-RPC result contains a `__text_plain__` sentinel key,
/// instructing the HTTP transport to serve the value as text/plain instead of
/// the default application/json.
pub(crate) async fn write_http_text_response<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    status: u16,
    text: &str,
    extra_headers: &str,
) -> Result<()> {
    let body = text.as_bytes();
    let extra = if extra_headers.is_empty() {
        String::new()
    } else if extra_headers.ends_with("\r\n") {
        extra_headers.to_string()
    } else {
        format!("{}\r\n", extra_headers)
    };
    let headers = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n",
        status,
        status_text(status),
        body.len(),
        extra
    );
    tcp_write_timeout(writer, headers.as_bytes()).await?;
    tcp_write_timeout(writer, body).await?;
    let _ = writer.shutdown().await;
    Ok(())
}

/// Build a trace context for HTTP requests.
pub(crate) fn http_trace_context(method: &str) -> RequestTraceContext {
    let request_id = format!("http-{}", crate::shared::timestamps::now_ts_ms());
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn read_all(mut r: tokio::io::DuplexStream) -> String {
        let mut buf = Vec::new();
        r.read_to_end(&mut buf).await.unwrap();
        String::from_utf8_lossy(&buf).to_string()
    }

    #[tokio::test]
    async fn json_writer_emits_full_status_code_table() {
        // Every status code in the unified table must map to its reason phrase
        // (regression for the old `403 OK` / `500 OK` / `405 OK` drift).
        let cases = [
            (200u16, "OK"),
            (202, "Accepted"),
            (204, "No Content"),
            (400, "Bad Request"),
            (401, "Unauthorized"),
            (403, "Forbidden"),
            (404, "Not Found"),
            (405, "Method Not Allowed"),
            (413, "Payload Too Large"),
            (429, "Too Many Requests"),
            (500, "Internal Server Error"),
            (501, "Not Implemented"),
            (502, "Bad Gateway"),
            (503, "Service Unavailable"),
        ];
        for (status, phrase) in cases {
            let (mut client, server) = tokio::io::duplex(8192);
            write_http_json_response(&mut client, status, json!({}), "")
                .await
                .unwrap();
            let raw = read_all(server).await;
            assert!(
                raw.starts_with(&format!("HTTP/1.1 {} {}\r\n", status, phrase)),
                "status {} produced wrong reason phrase: {raw:?}",
                status
            );
        }
    }

    #[tokio::test]
    async fn json_writer_injects_error_code_and_respects_existing() {
        let (mut client, server) = tokio::io::duplex(8192);
        write_http_json_response(&mut client, 400, json!({"error": "bad"}), "")
            .await
            .unwrap();
        let raw = read_all(server).await;
        assert!(raw.contains("\"code\":\"INVALID_REQUEST\"") || raw.contains("\"code\":"));

        let (mut client2, server2) = tokio::io::duplex(8192);
        write_http_json_response(
            &mut client2,
            400,
            json!({"error": "bad", "code": "KEEP"}),
            "",
        )
        .await
        .unwrap();
        let raw2 = read_all(server2).await;
        assert!(raw2.contains("\"code\":\"KEEP\""));
        assert!(!raw2.contains("INVALID_REQUEST"));
    }

    #[tokio::test]
    async fn json_writer_204_has_no_body_keep_alive_variant_flushes() {
        let (mut client, server) = tokio::io::duplex(8192);
        write_http_json_response_keep_alive(&mut client, 204, json!(null), "")
            .await
            .unwrap();
        // The keep-alive writer does not shut the socket down; dropping the
        // client end closes the duplex pair so the reader sees EOF.
        drop(client);
        let raw = read_all(server).await;
        assert!(raw.starts_with("HTTP/1.1 204 No Content\r\n"));
        assert!(raw.contains("Content-Length: 0"));
        assert!(raw.contains("Connection: keep-alive"));
        // No body after the terminating empty line.
        let body_start = raw.find("\r\n\r\n").map(|i| i + 4).unwrap_or(raw.len());
        assert_eq!(&raw[body_start..], "");
    }

    #[tokio::test]
    async fn json_writer_normalizes_extra_headers_trailing_crlf() {
        let (mut client, server) = tokio::io::duplex(8192);
        write_http_json_response(&mut client, 200, json!({}), "X-Test: 1\r\n")
            .await
            .unwrap();
        let raw = read_all(server).await;
        assert!(raw.contains("X-Test: 1\r\n"));
        // Exactly one blank separator line between headers and body.
        assert_eq!(raw.matches("\r\n\r\n").count(), 1, "raw: {raw:?}");
    }
}
