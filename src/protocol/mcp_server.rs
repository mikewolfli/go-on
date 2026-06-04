//! MCP Server implementation with stdio transport
//!
//! Provides a JSON-RPC 2.0 server that communicates over stdin/stdout,
//! implementing the Model Context Protocol specification.

use anyhow::Result;
use serde_json::json;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{
    AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader, ReadBuf,
};
use tokio::net::TcpListener;
use tokio::signal;
use tokio::sync::{Mutex, Notify, Semaphore};
use tracing::{debug, info, warn};

use crate::acp::r#impl::cors::{
    build_cors_headers, build_preflight_response_headers, is_origin_allowed,
};
use crate::acp::r#impl::request::inject_platform_profiles_if_absent;
use crate::acp::server::AcpServer;
use crate::agent::AgentRegistry;
use crate::governance::rbac::{AccessDecision, Permission, Principal};
use crate::i18n::runtime::{t, tf};
use crate::mcp::{JsonRpcRequest, JsonRpcResponse, McpServer};
use crate::tool::ToolRegistry;

/// MCP Server with stdio transport
pub struct McpStdioServer {
    mcp_server: Arc<McpServer>,
}

impl McpStdioServer {
    /// Create a new MCP stdio server
    pub fn new(
        agent_registry: Arc<AgentRegistry>,
        tool_registry: Arc<ToolRegistry>,
        server_name: String,
        server_version: String,
    ) -> Self {
        let mcp_server = McpServer::new(agent_registry, tool_registry, server_name, server_version);
        Self {
            mcp_server: Arc::new(mcp_server),
        }
    }

    /// Create a new MCP stdio server with an optional AcpServer reference
    pub fn new_with_acp(
        agent_registry: Arc<AgentRegistry>,
        tool_registry: Arc<ToolRegistry>,
        server_name: String,
        server_version: String,
        acp_server: Option<Arc<AcpServer>>,
    ) -> Self {
        let mcp_server = McpServer::new_with_acp(
            agent_registry,
            tool_registry,
            server_name,
            server_version,
            acp_server,
        );
        Self {
            mcp_server: Arc::new(mcp_server),
        }
    }

    /// Run the server (reads from stdin, writes to stdout)
    pub async fn run(&self) -> Result<()> {
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();

        let mut reader = BufReader::new(stdin);
        let stdout = Arc::new(Mutex::new(stdout));

        let mut line = String::new();
        loop {
            line.clear();
            let n = reader.read_line(&mut line).await?;

            // EOF
            if n == 0 {
                break;
            }

            // Skip empty lines
            if line.trim().is_empty() {
                continue;
            }

            // Parse request
            match serde_json::from_str::<JsonRpcRequest>(&line) {
                Ok(request) => {
                    let response = self.mcp_server.handle_request(request).await;
                    match response {
                        Ok(resp) => {
                            // MCP notifications (JSON-RPC with id=null) must not produce
                            // any response per JSON-RPC 2.0 spec (§notifications).
                            if resp.id.is_none() {
                                continue;
                            }
                            let mut stdout = stdout.lock().await;
                            let response_line = serde_json::to_string(&resp)?;
                            stdout.write_all(response_line.as_bytes()).await?;
                            stdout.write_all(b"\n").await?;
                            stdout.flush().await?;
                        }
                        Err(e) => {
                            warn!(
                                "{}",
                                tf("error.handling_request", &[("error", &format!("{}", e))])
                            );
                        }
                    }
                }
                Err(parse_error) => {
                    warn!(
                        "{}",
                        tf(
                            "error.parse_error",
                            &[("error", &format!("{}", parse_error))],
                        )
                    );
                    let mut stdout = stdout.lock().await;
                    send_parse_error(&mut *stdout).await?;
                }
            }
        }

        Ok(())
    }
}

/// MCP Server with HTTP transport
/// A stream that is either plain TCP or wrapped in TLS.
///
/// Enables optional TLS on MCP HTTP connections without changing
/// the handler function signatures across all call sites.
enum MaybeTlsStream {
    Plain(tokio::net::TcpStream),
    Tls(Box<tokio_rustls::server::TlsStream<tokio::net::TcpStream>>),
}

impl AsyncRead for MaybeTlsStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MaybeTlsStream::Plain(s) => Pin::new(s).poll_read(cx, buf),
            MaybeTlsStream::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for MaybeTlsStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            MaybeTlsStream::Plain(s) => Pin::new(s).poll_write(cx, buf),
            MaybeTlsStream::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MaybeTlsStream::Plain(s) => Pin::new(s).poll_flush(cx),
            MaybeTlsStream::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            MaybeTlsStream::Plain(s) => Pin::new(s).poll_shutdown(cx),
            MaybeTlsStream::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

pub struct McpHttpServer {
    mcp_server: Arc<McpServer>,
    bind_addr: String,
    shutdown_notify: Arc<Notify>,
    connection_semaphore: Arc<Semaphore>,
    acp_server: Option<Arc<AcpServer>>,
    /// Optional TLS acceptor. When `Some`, all accepted TCP streams are
    /// wrapped with TLS before handling HTTP requests. Defaults to `None`
    /// for local development / plaintext operation.
    tls_acceptor: Option<tokio_rustls::TlsAcceptor>,
}

impl McpHttpServer {
    /// Create a new MCP HTTP server
    pub fn new(
        agent_registry: Arc<AgentRegistry>,
        tool_registry: Arc<ToolRegistry>,
        server_name: String,
        server_version: String,
        bind_addr: String,
    ) -> Self {
        let mcp_server = McpServer::new(agent_registry, tool_registry, server_name, server_version);
        Self {
            mcp_server: Arc::new(mcp_server),
            bind_addr,
            shutdown_notify: Arc::new(Notify::new()),
            connection_semaphore: Arc::new(Semaphore::new(256)),
            acp_server: None,
            tls_acceptor: None,
        }
    }

    /// Create a new MCP HTTP server with an optional AcpServer reference
    pub fn new_with_acp(
        agent_registry: Arc<AgentRegistry>,
        tool_registry: Arc<ToolRegistry>,
        server_name: String,
        server_version: String,
        bind_addr: String,
        acp_server: Option<Arc<AcpServer>>,
    ) -> Self {
        let mcp_server = McpServer::new_with_acp(
            agent_registry,
            tool_registry,
            server_name,
            server_version,
            acp_server.clone(),
        );
        Self {
            mcp_server: Arc::new(mcp_server),
            bind_addr,
            shutdown_notify: Arc::new(Notify::new()),
            connection_semaphore: Arc::new(Semaphore::new(256)),
            acp_server,
            tls_acceptor: None,
        }
    }

    /// Run the HTTP server
    pub async fn run(&self) -> Result<()> {
        info!(
            "{}",
            tf("info.mcp_server_listening", &[("address", &self.bind_addr)])
        );
        let listener = TcpListener::bind(&self.bind_addr).await?;

        info!("{}", t("info.mcp_server_operational"));
        debug!(
            "{}",
            tf(
                "debug.mcp_server_accepting",
                &[("address", &self.bind_addr)]
            )
        );

        // Signal handling for graceful shutdown
        let mut sigterm = std::pin::pin!(async {
            #[cfg(unix)]
            {
                match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
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
                _ = self.shutdown_notify.notified() => {
                    info!("MCP HTTP server shutting down");
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
                result = listener.accept() => {
                    let permit = Arc::clone(&self.connection_semaphore)
                        .acquire_owned()
                        .await;
                    let permit_guard = match permit {
                        Ok(p) => p,
                        Err(_) => break,
                    };
                    let (socket, peer_addr) = result?;
                    let mcp_server = Arc::clone(&self.mcp_server);
                    let acp_server = self.acp_server.clone();
                    let tls_acceptor = self.tls_acceptor.clone();

                    tokio::spawn(async move {
                        // Hold permit for the whole connection handler lifetime.
                        let _permit = permit_guard;
                        let mut stream = match tls_acceptor {
                            Some(ref acceptor) => {
                                match acceptor.accept(socket).await {
                                    Ok(tls) => MaybeTlsStream::Tls(Box::new(tls)),
                                    Err(e) => {
                                        warn!(
                                            "{}",
                                            tf(
                                                "error.tls_handshake",
                                                &[
                                                    ("address", &peer_addr.to_string()),
                                                    ("error", &format!("{}", e))
                                                ]
                                            )
                                        );
                                        return;
                                    }
                                }
                            }
                            None => MaybeTlsStream::Plain(socket),
                        };
                        if let Err(err) = handle_http_connection(&mut stream, mcp_server, acp_server).await {
                            warn!(
                                "{}",
                                tf(
                                    "error.http_connection",
                                    &[
                                        ("address", &peer_addr.to_string()),
                                        ("error", &format!("{}", err))
                                    ]
                                )
                            );
                        }
                    });
                }
            }
        }

        // Drain active connections before full shutdown.
        let drain_seconds = self
            .acp_server
            .as_ref()
            .map(|s| s.runtime_config.shutdown_drain_seconds)
            .unwrap_or(30);
        if drain_seconds > 0 {
            info!("Draining connections for {} seconds...", drain_seconds);
            tokio::time::sleep(std::time::Duration::from_secs(drain_seconds)).await;
        }

        info!("MCP HTTP server stopped");
        Ok(())
    }

    /// Configure this server with a TLS acceptor, enabling TLS on all
    /// accepted connections.
    ///
    /// This is a builder-style method that consumes `self` and returns
    /// the updated server. Call it after construction:
    /// ```ignore
    /// let server = McpHttpServer::new(...).with_tls_acceptor(acceptor);
    /// ```
    pub fn with_tls_acceptor(mut self, acceptor: tokio_rustls::TlsAcceptor) -> Self {
        self.tls_acceptor = Some(acceptor);
        self
    }

    /// Request a graceful shutdown of the HTTP server.
    pub fn shutdown(&self) {
        self.shutdown_notify.notify_waiters();
    }
}

async fn handle_http_connection(
    socket: &mut MaybeTlsStream,
    mcp_server: Arc<McpServer>,
    acp_server: Option<Arc<AcpServer>>,
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
    let header_end = request_text.find("\r\n\r\n").ok_or_else(|| {
        warn!("MCP HTTP: invalid request --missing header terminator");
        anyhow::anyhow!("{}", t("error.http_missing_header"))
    })?;

    let (header_part, body_initial_part) = request_text.split_at(header_end + 4);
    let mut lines = header_part.lines();
    let request_line = lines.next().ok_or_else(|| {
        warn!("MCP HTTP: invalid request --missing request line");
        anyhow::anyhow!("{}", t("error.http_missing_request_line"))
    })?;

    let mut request_line_parts = request_line.split_whitespace();
    let method = request_line_parts.next().ok_or_else(|| {
        warn!(
            "MCP HTTP: invalid request --missing method in request line: {}",
            request_line
        );
        anyhow::anyhow!("{}", t("error.http_missing_method"))
    })?;
    let path = request_line_parts.next().ok_or_else(|| {
        warn!(
            "MCP HTTP: invalid request --missing path in request line: {}",
            request_line
        );
        anyhow::anyhow!("{}", t("error.http_missing_path"))
    })?;

    // ── Content-Length validation (before any auth processing) ────────
    // Check Content-Length before allocating buffers to prevent OOM.
    const MAX_BODY_SIZE: usize = 10 * 1024 * 1024; // 10MB
    let content_length = extract_content_length(header_part).unwrap_or(0);
    if content_length > MAX_BODY_SIZE {
        let error_body = inject_platform_profiles_if_absent(
            serde_json::json!({
                "error": tf("error.http_body_too_large", &[
                    ("size", &content_length.to_string()),
                    ("max", &MAX_BODY_SIZE.to_string())
                ]),
                "code": "PAYLOAD_TOO_LARGE"
            }),
            "mcp.payload_too_large",
        );
        let cors = compute_mcp_cors_headers(header_part, &acp_server);
        write_http_json_response(socket, 413, error_body, &cors).await?;
        return Ok(());
    }

    // ── CORS headers ─────────────────────────────────────────────────────
    let cors_headers = compute_mcp_cors_headers(header_part, &acp_server);

    // ── Health endpoint (no auth) ────────────────────────────────────────
    if method == "GET" && path == "/health" {
        let body = inject_platform_profiles_if_absent(
            serde_json::json!({
                "status": "ok",
                "protocolVersion": crate::mcp::MCP_VERSION,
            }),
            "health",
        );
        write_http_json_response(socket, 200, body, &cors_headers).await?;
        return Ok(());
    }

    // ── CORS preflight (OPTIONS) ─────────────────────────────────────────
    if method == "OPTIONS" {
        if let Some(ref server) = acp_server {
            if let Some(ref cfg) = server.runtime_config.cors_config() {
                let origin = extract_mcp_header_value(header_part, "origin");
                let preflight_headers = build_preflight_response_headers(origin, cfg);
                let origin_val: &str = origin.filter(|o| is_origin_allowed(o, cfg)).unwrap_or("*");

                let mut extra = format!("Access-Control-Allow-Origin: {}\r\n", origin_val);
                for (k, v) in &preflight_headers {
                    extra.push_str(&format!("{}: {}\r\n", k, v));
                }
                extra.push_str(&format!(
                    "Access-Control-Max-Age: {}\r\n",
                    cfg.max_age_seconds
                ));

                // CORS preflight — return 204 No Content per HTTP spec
                write_http_json_response(socket, 204, serde_json::json!(null), &extra).await?;
                return Ok(());
            }
        }
        // No CORS config → reject OPTIONS
        write_http_json_response(
            socket,
            405,
            serde_json::json!({"error": "Method Not Allowed"}),
            "",
        )
        .await?;
        return Ok(());
    }

    if method != "POST" {
        let body = inject_platform_profiles_if_absent(
            serde_json::json!({"error": t("error.method_not_allowed")}),
            "mcp.unknown_method",
        );
        write_http_json_response(socket, 405, body, &cors_headers).await?;
        return Ok(());
    }

    // ── Entry auth (same pattern as ACP HTTP server) ─────────────────────
    if let Some(ref server) = acp_server {
        if server.runtime_config.entry_auth_enabled {
            let env_name = server.runtime_config.entry_auth_api_key_env.trim();
            let expected_key = crate::shared::secret_override::get_secret(env_name)
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty());

            if let Some(ref expected) = expected_key {
                let provided = extract_mcp_entry_token(header_part)
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty());

                if !provided.is_some_and(|ref p| constant_time_eq(p, expected)) {
                    write_http_json_response(
                        socket,
                        401,
                        serde_json::json!({
                            "error": t("error.entry_auth_required"),
                            "code": "ENTRY_AUTH_REQUIRED"
                        }),
                        &cors_headers,
                    )
                    .await?;
                    return Ok(());
                }
            } else {
                write_http_json_response(
                    socket,
                    503,
                    serde_json::json!({
                        "error": t("error.entry_auth_misconfigured"),
                        "code": "ENTRY_AUTH_MISCONFIGURED"
                    }),
                    &cors_headers,
                )
                .await?;
                return Ok(());
            }
        }
    }

    // ── User auth and RBAC ───────────────────────────────────────────────
    if let Some(ref server) = acp_server {
        let user_session = server
            .session_manager
            .as_ref()
            .and_then(|sm| sm.extract_user_from_request(header_part));

        if server.runtime_config.user_auth_enabled {
            let session = match user_session {
                Some(ref s) => s,
                None => {
                    write_http_json_response(
                        socket,
                        401,
                        serde_json::json!({
                            "error": t("error.auth_required"),
                            "code": "AUTH_REQUIRED"
                        }),
                        &cors_headers,
                    )
                    .await?;
                    return Ok(());
                }
            };

            // Route-based permission check.
            // The enforcer guard is NOT `Send`, so we scope the RBAC logic to
            // a synchronous block and drop the guard before any `.await`.
            let required_perm = Permission::Execute;
            if let Some(ref enforcer) = server.governance_deps.rbac_enforcer {
                let access_decision = {
                    let enforcer_guard = enforcer
                        .read()
                        .map_err(|_| anyhow::anyhow!("rbac lock poisoned"))?;
                    let mut principal = Principal::new(
                        &session.user_id,
                        session.roles.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                        session.tenant_id.as_deref(),
                    );
                    enforcer_guard.resolve_permissions(&mut principal);
                    match enforcer_guard.check_access(&principal, &required_perm) {
                        AccessDecision::Allow => None,
                        AccessDecision::Deny { reason } => Some(serde_json::json!({
                            "error": tf("error.auth_forbidden", &[("reason", &reason)]),
                            "code": "ACCESS_DENIED",
                            "reason": reason
                        })),
                        AccessDecision::Escalate { required_role } => Some(serde_json::json!({
                            "error": tf("error.auth_insufficient_privileges", &[("required_role", &required_role)]),
                            "code": "PRIVILEGE_ESCALATION_REQUIRED",
                            "required_role": required_role
                        })),
                    }
                }; // enforcer_guard dropped here

                if let Some(error_body) = access_decision {
                    write_http_json_response(socket, 403, error_body, &cors_headers).await?;
                    return Ok(());
                }
            }
        }
    }

    let content_length = extract_content_length(header_part).unwrap_or(0);
    let mut body_bytes = body_initial_part.as_bytes().to_vec();
    if body_bytes.len() < content_length {
        let mut remaining = vec![0u8; content_length - body_bytes.len()];
        tokio::time::timeout(Duration::from_secs(30), socket.read_exact(&mut remaining))
            .await
            .map_err(|_| anyhow::anyhow!("{}", t("error.http_body_timeout")))??;
        body_bytes.extend_from_slice(&remaining);
    }
    body_bytes.truncate(content_length);

    let body_str = String::from_utf8_lossy(&body_bytes);
    let request = match serde_json::from_str::<JsonRpcRequest>(&body_str) {
        Ok(req) => req,
        Err(parse_error) => {
            warn!(
                "MCP HTTP: JSON-RPC parse error from {} {}: {}",
                method, path, parse_error
            );
            let error_data =
                inject_platform_profiles_if_absent(serde_json::json!({}), "mcp.parse_error");
            let error_response = JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                result: None,
                error: Some(crate::mcp::JsonRpcError {
                    code: crate::mcp::error_codes::PARSE_ERROR,
                    message: tf(
                        "error.http_parse_error",
                        &[("error", &parse_error.to_string())],
                    ),
                    data: Some(error_data),
                }),
                id: None,
            };
            write_http_json_response(
                socket,
                200,
                serde_json::to_value(error_response)?,
                &cors_headers,
            )
            .await?;
            return Ok(());
        }
    };

    let response = mcp_server.handle_request(request).await?;
    debug!("MCP HTTP: dispatched {} {} -> ok", method, path);

    // MCP notifications (JSON-RPC with id=null) must not produce
    // any response per JSON-RPC 2.0 spec (§notifications).
    if response.id.is_none() {
        // Must still send some HTTP response to satisfy the TCP layer,
        // but it should be a 202 Accepted with no body.
        let empty_body = serde_json::Value::Null;
        write_http_json_response(socket, 202, empty_body, &cors_headers).await?;
        return Ok(());
    }

    write_http_json_response(socket, 200, serde_json::to_value(response)?, &cors_headers).await?;

    Ok(())
}

fn extract_content_length(headers: &str) -> Option<usize> {
    let mut found: Option<usize> = None;
    for line in headers.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if !name.trim().eq_ignore_ascii_case("content-length") {
            continue;
        }
        let val: usize = value.trim().parse().ok()?;
        match found {
            None => found = Some(val),
            Some(prev) if prev == val => {} // duplicate with same value — OK
            Some(prev) => {
                // Conflict — RFC 7230 forbids differing Content-Length headers.
                // Log a warning and use the last value to avoid body truncation.
                warn!(
                    "conflicting Content-Length headers: {} vs {}; using last value",
                    prev, val
                );
                found = Some(val);
            }
        }
    }
    found
}

async fn write_http_json_response(
    socket: &mut MaybeTlsStream,
    status: u16,
    body: serde_json::Value,
    extra_headers: &str,
) -> Result<()> {
    let status_text = match status {
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        413 => "Payload Too Large",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        other => return Err(anyhow::anyhow!("unsupported HTTP status code: {}", other)),
    };

    // 204 No Content MUST NOT include a message body per HTTP/1.1 §6.4.1
    if status == 204 {
        let mut response = format!(
            "HTTP/1.1 {} {}\r\nConnection: close\r\n",
            status, status_text,
        );
        if !extra_headers.is_empty() {
            response.push_str(extra_headers);
            if !extra_headers.ends_with("\r\n") {
                response.push_str("\r\n");
            }
        }
        response.push_str("\r\n");
        socket.write_all(response.as_bytes()).await?;
        socket.flush().await?;
        return Ok(());
    }

    let body_text = serde_json::to_string(&body)?;
    let mut response = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        status,
        status_text,
        body_text.len(),
    );
    if !extra_headers.is_empty() {
        response.push_str(extra_headers);
        if !extra_headers.ends_with("\r\n") {
            response.push_str("\r\n");
        }
    }
    response.push_str("\r\n");
    response.push_str(&body_text);

    socket.write_all(response.as_bytes()).await?;
    socket.flush().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Helper functions for MCP HTTP security hardening
// ---------------------------------------------------------------------------

/// Extract a Bearer token from MCP HTTP request headers.
/// Checks `Authorization: Bearer <token>` first, then falls back to
/// `X-Api-Key` and `X-Go-On-Key` headers.
/// Constant-time string comparison to prevent timing side-channel attacks.
/// Compares two strings in constant time to prevent timing side-channel attacks.
///
/// Uses the `subtle` crate's `ConstantTimeEq` trait which guarantees
/// constant-time comparison via compiler barriers and careful implementation,
/// protecting against attackers who measure response latency to guess tokens
/// character-by-character.
fn constant_time_eq(a: &str, b: &str) -> bool {
    use subtle::ConstantTimeEq;
    a.as_bytes().ct_eq(b.as_bytes()).into()
}

fn extract_mcp_entry_token(headers: &str) -> Option<String> {
    if let Some(auth) = extract_mcp_header_value(headers, "authorization") {
        let lower = auth.to_ascii_lowercase();
        if lower.starts_with("bearer ") {
            return Some(auth[7..].trim().to_string());
        }
    }
    extract_mcp_header_value(headers, "x-api-key")
        .or_else(|| extract_mcp_header_value(headers, "x-go-on-key"))
        .filter(|value| !value.trim().is_empty())
        .map(|s| s.to_string())
}

/// Extract a single header value from raw HTTP headers.
fn extract_mcp_header_value<'a>(headers: &'a str, name: &str) -> Option<&'a str> {
    for line in headers.lines() {
        if let Some((key, value)) = line.split_once(':') {
            if key.trim().eq_ignore_ascii_case(name) {
                return Some(value.trim());
            }
        }
    }
    None
}

/// Compute CORS response headers for the MCP HTTP server.
/// Returns an empty string when no CORS config is present or the origin
/// is not allowed.
fn compute_mcp_cors_headers(headers: &str, acp_server: &Option<Arc<AcpServer>>) -> String {
    let config = match acp_server {
        Some(ref server) => server.runtime_config.cors_config(),
        None => return String::new(),
    };
    let config = match config {
        Some(c) => c,
        None => return String::new(),
    };
    let origin = extract_mcp_header_value(headers, "origin");
    let cors_headers = build_cors_headers(origin, &config);
    if cors_headers.is_empty() {
        return String::new();
    }
    cors_headers
        .iter()
        .map(|(k, v)| format!("{}: {}\r\n", k, v))
        .collect()
}

#[cfg(test)]
fn parse_request_target_for_test(raw_request: &str) -> Option<(String, String)> {
    let first_line = raw_request.lines().next()?;
    let mut parts = first_line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();
    Some((method, path))
}

#[cfg(test)]
fn content_length_for_test(headers: &str) -> Option<usize> {
    extract_content_length(headers)
}

/// Send a JSON-RPC Parse error response (-32700) to the client.
///
/// Per the JSON-RPC 2.0 specification, when a request cannot be parsed
/// as valid JSON, the server must respond with a Parse error.
async fn send_parse_error(
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
) -> std::io::Result<()> {
    let error = json!({
        "jsonrpc": "2.0",
        "id": null,
        "error": {
            "code": -32700,
            "message": "Parse error"
        }
    });
    let line = serde_json::to_string(&error).map_err(|e| std::io::Error::other(e))?;
    writer.write_all(line.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mcp_stdio_server_creation() {
        let agent_registry = Arc::new(AgentRegistry::new());
        let tool_registry = Arc::new(ToolRegistry::new());
        let _server = McpStdioServer::new(
            agent_registry,
            tool_registry,
            "go-on".to_string(),
            "1.0.0".to_string(),
        );

        // Server was created successfully
    }

    #[tokio::test]
    async fn test_mcp_http_server_creation() {
        let agent_registry = Arc::new(AgentRegistry::new());
        let tool_registry = Arc::new(ToolRegistry::new());
        let server = McpHttpServer::new(
            agent_registry,
            tool_registry,
            "go-on".to_string(),
            "1.0.0".to_string(),
            "127.0.0.1:8080".to_string(),
        );

        // Verify server was created successfully
        assert_eq!(
            server.bind_addr, "127.0.0.1:8080",
            "Bind address should be set correctly"
        );
    }

    #[test]
    fn test_extract_content_length() {
        let headers = "POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 42\r\n\r\n";
        assert_eq!(content_length_for_test(headers), Some(42));
    }

    #[test]
    fn test_parse_request_target() {
        let request = "GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n";
        let parsed = parse_request_target_for_test(request).expect("request line should parse");
        assert_eq!(parsed.0, "GET");
        assert_eq!(parsed.1, "/health");
    }

    #[test]
    fn test_content_length_missing_returns_none() {
        let headers = "POST / HTTP/1.1\r\nHost: localhost\r\n\r\n";
        assert_eq!(content_length_for_test(headers), None);
    }

    #[test]
    fn test_content_length_with_different_casing() {
        let headers = "POST / HTTP/1.1\r\ncontent-length: 100\r\n\r\n";
        assert_eq!(content_length_for_test(headers), Some(100));
    }

    #[test]
    fn test_parse_request_target_post() {
        let request = "POST /api/v1/chat HTTP/1.1\r\nHost: localhost\r\n\r\n";
        let parsed = parse_request_target_for_test(request).expect("request line should parse");
        assert_eq!(parsed.0, "POST");
        assert_eq!(parsed.1, "/api/v1/chat");
    }
}
