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
use crate::mcp::error_codes;
use crate::mcp::{JsonRpcError, JsonRpcRequest, JsonRpcResponse, McpServer};
use crate::security::mtls::{MtlsAcceptor, MtlsConfig};
use crate::tool::ToolRegistry;

/// MCP Server with stdio transport
pub struct McpStdioServer {
    mcp_server: Arc<McpServer>,
}

impl McpStdioServer {
    /// Create a new MCP stdio server.
    ///
    /// Pass `Some(acp_server)` to enable workflow tools (`workflow_execute`,
    /// `workflow_ask`, `workflow_generate`), prompt templates, and completion
    /// support. Pass `None` for a minimal server without ACP features.
    pub fn new(
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

            let line = line.trim().to_string();

            // Attempt batch (JSON array) first, then fall back to single request.
            if line.starts_with('[') {
                match serde_json::from_str::<Vec<JsonRpcRequest>>(&line) {
                    Ok(requests) => {
                        for req in requests {
                            let req_id = req.id.clone();
                            match self.mcp_server.handle_request(req).await {
                                Ok(resp) => {
                                    // Notifications (id=null) don't produce a response.
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
                                    let err_msg = format!("{}", e);
                                    warn!(
                                        "{}",
                                        tf("error.handling_request", &[("error", &err_msg)])
                                    );
                                    let mut stdout = stdout.lock().await;
                                    send_handler_error(&mut *stdout, req_id, &err_msg).await?;
                                }
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
            } else {
                match serde_json::from_str::<JsonRpcRequest>(&line) {
                    Ok(request) => {
                        let request_id = request.id.clone();
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
                                let err_msg = format!("{}", e);
                                warn!("{}", tf("error.handling_request", &[("error", &err_msg)]));
                                let mut stdout = stdout.lock().await;
                                send_handler_error(&mut *stdout, request_id, &err_msg).await?;
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

/// Shared SSE client registry for MCP HTTP server.
/// Uses a broadcast channel so that resource-change notifications can be
/// pushed to all connected SSE clients in real time.
pub(crate) type SseBroadcaster = tokio::sync::broadcast::Sender<String>;

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
    /// Optional mTLS configuration. When set, a `TlsAcceptor` is built from
    /// this config (with client CA certificate verification) during `run()`.
    /// This is a configuration wiring field — the actual acceptor is lazily
    /// initialised if `tls_acceptor` is `None` and `tls_config` is `Some`.
    tls_config: Option<MtlsConfig>,
    /// Optional rate limit middleware. When set, `check()` is called before
    /// processing each request in `handle_http_connection`.
    rate_limiter: Option<Arc<crate::protocol::rate_limit::RateLimitMiddleware>>,
    /// SSE broadcaster for pushing MCP notifications to connected SSE clients.
    /// Subscription-based (resource change, tool list change, etc.) notifications
    /// are sent through this channel.
    sse_broadcaster: Arc<SseBroadcaster>,
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
        let sse_broadcaster = Arc::new(tokio::sync::broadcast::channel::<String>(256).0);
        let mcp_server = McpServer::new(agent_registry, tool_registry, server_name, server_version)
            .with_sse_broadcaster(Arc::clone(&sse_broadcaster));
        Self {
            mcp_server: Arc::new(mcp_server),
            bind_addr,
            shutdown_notify: Arc::new(Notify::new()),
            connection_semaphore: Arc::new(Semaphore::new(256)),
            acp_server: None,
            tls_acceptor: None,
            tls_config: None,
            rate_limiter: None,
            sse_broadcaster,
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
        let sse_broadcaster = Arc::new(tokio::sync::broadcast::channel::<String>(256).0);
        let mcp_server = McpServer::new_with_acp(
            agent_registry,
            tool_registry,
            server_name,
            server_version,
            acp_server.clone(),
        )
        .with_sse_broadcaster(Arc::clone(&sse_broadcaster));
        Self {
            mcp_server: Arc::new(mcp_server),
            bind_addr,
            shutdown_notify: Arc::new(Notify::new()),
            connection_semaphore: Arc::new(Semaphore::new(256)),
            acp_server,
            tls_acceptor: None,
            tls_config: None,
            rate_limiter: None,
            sse_broadcaster,
        }
    }

    /// Run the HTTP server
    pub async fn run(&self) -> Result<()> {
        // Lazy initialise the TLS acceptor from mTLS config when the
        // `tls_acceptor` has not been explicitly set but `tls_config` is
        // provided. This wires client CA certificate verification through
        // the MtlsAcceptor's build_server_config.
        let effective_acceptor: Option<tokio_rustls::TlsAcceptor> = if self.tls_acceptor.is_some() {
            self.tls_acceptor.clone()
        } else if let Some(ref cfg) = self.tls_config {
            let mtls_acceptor = MtlsAcceptor::new(cfg.clone());
            let server_config = mtls_acceptor
                .build_server_config()
                .map_err(|e| anyhow::anyhow!("failed to build mTLS server config: {e}"))?;
            info!("MCP HTTP: TLS acceptor configured from mTLS config");
            Some(tokio_rustls::TlsAcceptor::from(server_config))
        } else {
            None
        };

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
                    let tls_acceptor = effective_acceptor.clone();
                    let rate_limiter = self.rate_limiter.clone();
                    let sse_broadcaster = Arc::clone(&self.sse_broadcaster);

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
                        if let Err(err) = handle_http_connection(&mut stream, mcp_server, acp_server, rate_limiter, sse_broadcaster).await {
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

    /// Configure the server with an mTLS config. If `tls_acceptor` has not
    /// been set directly, the `TlsAcceptor` will be built from this config
    /// when `run()` is called (lazy initialisation of the TLS acceptor from
    /// the mTLS configuration with client CA certificate verification).
    pub fn with_tls_config(mut self, config: MtlsConfig) -> Self {
        self.tls_config = Some(config);
        self
    }

    /// Configure the server with a rate limit middleware.
    /// Every request processed by `handle_http_connection` will be checked
    /// against this rate limiter before processing.
    pub fn with_rate_limiter(
        mut self,
        rate_limiter: Arc<crate::protocol::rate_limit::RateLimitMiddleware>,
    ) -> Self {
        self.rate_limiter = Some(rate_limiter);
        self
    }

    /// Request a graceful shutdown of the HTTP server.
    pub fn shutdown(&self) {
        self.shutdown_notify.notify_waiters();
    }

    /// Get a reference to the SSE broadcaster for pushing resource-change
    /// and other subscription-based notifications to connected SSE clients.
    pub fn sse_broadcaster(&self) -> Arc<SseBroadcaster> {
        Arc::clone(&self.sse_broadcaster)
    }

    /// Broadcast a JSON-RPC notification to all connected SSE clients.
    ///
    /// The notification is serialised as an SSE `event: message` frame with
    /// the JSON-RPC notification body as the `data:` field.
    pub fn broadcast_sse(&self, method: &str, params: &serde_json::Value) {
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        let payload = serde_json::to_string(&notification).unwrap_or_default();
        let _ = self.sse_broadcaster.send(payload);
    }
}

async fn handle_http_connection(
    socket: &mut MaybeTlsStream,
    mcp_server: Arc<McpServer>,
    acp_server: Option<Arc<AcpServer>>,
    rate_limiter: Option<Arc<crate::protocol::rate_limit::RateLimitMiddleware>>,
    sse_broadcaster: Arc<SseBroadcaster>,
) -> Result<()> {
    // ── Connection: keep-alive — compliance only, no multiplexing ──────
    // Responses include `Connection: keep-alive` (set in
    // write_http_json_response) for HTTP/1.1 spec compliance.  However,
    // this handler currently processes exactly **one** request per TCP
    // connection and then returns, so the keep-alive header is
    // metadata-only — no multiplexing occurs.
    //
    // Future enhancement: wrap the handler body in a loop that reads
    // subsequent requests on the same connection while `Connection:
    // keep-alive` is present, and breaks when `Connection: close` is
    // received.
    //
    // ── Header buffer: start small, grow dynamically ────────────────────
    // Allocate only INITIAL_HEADER_BUFFER_SIZE bytes up front, then grow
    // as needed when reading the HTTP header, up to MAX_HEADER_BUFFER_SIZE.
    // This avoids wasting 64KB on small requests.
    const INITIAL_HEADER_BUFFER_SIZE: usize = 4096;
    const MAX_HEADER_BUFFER_SIZE: usize = 64 * 1024;

    let mut buffer = vec![0u8; INITIAL_HEADER_BUFFER_SIZE];
    let mut total_bytes_read: usize = 0;

    let header_end = loop {
        if total_bytes_read >= buffer.len() {
            // Grow the buffer (double until max)
            let new_size = std::cmp::min(buffer.len() * 2, MAX_HEADER_BUFFER_SIZE);
            if new_size <= buffer.len() {
                warn!(
                    "MCP HTTP: request header exceeds {} bytes",
                    MAX_HEADER_BUFFER_SIZE
                );
                anyhow::bail!("{}", t("error.http_header_too_large"));
            }
            buffer.resize(new_size, 0u8);
        }

        let bytes_read = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            socket.read(&mut buffer[total_bytes_read..]),
        )
        .await
        .map_err(|_| anyhow::anyhow!("timeout reading HTTP request"))??;

        if bytes_read == 0 {
            if total_bytes_read == 0 {
                return Ok(());
            }
            // Partial header with no data left — missing terminator
            warn!("MCP HTTP: incomplete request header --missing header terminator");
            anyhow::bail!("{}", t("error.http_missing_header"));
        }

        total_bytes_read += bytes_read;

        let request_text = String::from_utf8_lossy(&buffer[..total_bytes_read]);
        if let Some(pos) = request_text.find("\r\n\r\n") {
            break pos;
        }
    };

    let request_text = String::from_utf8_lossy(&buffer[..total_bytes_read]);
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

    // ── SSE endpoint (GET /sse or /mcp-sse) — must be checked before the
    //     POST-only guard below so SSE connections bypass the POST requirement.
    //     MCP SSE Streamable HTTP transport per MCP spec.
    // MCP Streamable HTTP Transport §6.3
    if method == "GET" && (path == "/sse" || path == "/mcp-sse") {
        return handle_mcp_sse_connection(socket, sse_broadcaster).await;
    }

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
            .session
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

    // ── Rate limit check (if middleware configured) ───────────────────────
    if let Some(ref limiter) = rate_limiter {
        // Derive tenant identifier from the session (if auth is enabled) or
        // fall back to a default tenant for unauthenticated requests.
        let tenant_id = acp_server
            .as_ref()
            .and_then(|s| s.session.session_manager.as_ref())
            .and_then(|sm| sm.extract_user_from_request(header_part))
            .and_then(|u| u.tenant_id)
            .unwrap_or_else(|| "default".to_string());

        if let Err(retry_after) = limiter.check(&tenant_id).await {
            warn!(
                tenant = %tenant_id,
                retry_after = retry_after,
                "rate limit exceeded"
            );
            let error_body = inject_platform_profiles_if_absent(
                serde_json::json!({
                    "error": "rate limit exceeded",
                    "code": "RATE_LIMITED",
                    "retryAfter": retry_after
                }),
                "mcp.rate_limited",
            );
            let cors = compute_mcp_cors_headers(header_part, &acp_server);
            write_http_json_response(socket, 429, error_body, &cors).await?;
            return Ok(());
        }
    }

    let content_length = extract_content_length(header_part).unwrap_or(0);
    let mut body_bytes = body_initial_part.as_bytes().to_vec();
    if body_bytes.len() < content_length {
        // Safety check: body size bounded by MAX_BODY_SIZE (10MB) to
        // prevent OOM from malicious oversized Content-Length headers.
        // The primary check is at line ~517; this is a redundant guard.
        if content_length > MAX_BODY_SIZE {
            anyhow::bail!(
                "body content-length {} exceeds maximum allowed {} bytes",
                content_length,
                MAX_BODY_SIZE
            );
        }
        let mut remaining = vec![0u8; content_length - body_bytes.len()];
        tokio::time::timeout(Duration::from_secs(30), socket.read_exact(&mut remaining))
            .await
            .map_err(|_| anyhow::anyhow!("{}", t("error.http_body_timeout")))??;
        body_bytes.extend_from_slice(&remaining);
    }
    body_bytes.truncate(content_length);

    let body_str = String::from_utf8_lossy(&body_bytes);

    // Attempt batch (JSON array) first, then fall back to single request.
    if body_str.trim_start().starts_with('[') {
        let requests: Vec<JsonRpcRequest> = match serde_json::from_str(&body_str) {
            Ok(reqs) => reqs,
            Err(parse_error) => {
                warn!(
                    "MCP HTTP: JSON-RPC batch parse error from {} {}: {}",
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

        // Process each request in the batch and collect responses.
        let mut responses: Vec<JsonRpcResponse> = Vec::with_capacity(requests.len());
        for req in requests {
            let req_id = req.id.clone();
            let resp = match mcp_server.handle_request(req).await {
                Ok(resp) => resp,
                Err(e) => {
                    warn!(
                        "MCP HTTP: error handling batch request from {} {}: {}",
                        method, path, e
                    );
                    JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        result: None,
                        error: Some(JsonRpcError {
                            code: error_codes::INTERNAL_ERROR,
                            message: format!("{}", e),
                            data: None,
                        }),
                        id: req_id,
                    }
                }
            };
            // Per JSON-RPC 2.0, notifications (id=null) in a batch do not
            // produce a response entry.
            if resp.id.is_some() {
                responses.push(resp);
            }
        }

        debug!(
            "MCP HTTP: dispatched {} {} -> {} batch responses",
            method,
            path,
            responses.len()
        );
        write_http_json_response(socket, 200, serde_json::to_value(responses)?, &cors_headers)
            .await?;
        return Ok(());
    }

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

/// Handle an MCP SSE (Server-Sent Events) connection.
///
/// Sends the SSE headers, an initial `endpoint` event advertising the
/// JSON-RPC POST URL, then enters a loop forwarding broadcast notifications
/// (resource changes, tool list changes, etc.) to the connected SSE client.
/// The connection remains open until the client disconnects or the server
/// shuts down.
async fn handle_mcp_sse_connection(
    socket: &mut MaybeTlsStream,
    sse_broadcaster: Arc<SseBroadcaster>,
) -> Result<()> {
    // ── SSE headers ───────────────────────────────────────────────────
    // Per the MCP Streamable HTTP spec, the SSE endpoint must advertise
    // the POST endpoint URL and keep the connection alive.
    let extra_headers = "Access-Control-Allow-Origin: *\r\n";
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\nX-Accel-Buffering: no\r\n{}\r\n\r\n",
        extra_headers
    );
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        socket.write_all(header.as_bytes()),
    )
    .await
    .map_err(|_| anyhow::anyhow!("timeout writing SSE headers"))??;
    socket.flush().await?;

    // ── Initial endpoint event ─────────────────────────────────────────
    // Advertise the JSON-RPC POST endpoint so the client knows where to
    // send its requests.
    let endpoint_event = "event: endpoint\ndata: /mcp\n\n".to_string();
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        socket.write_all(endpoint_event.as_bytes()),
    )
    .await
    .map_err(|_| anyhow::anyhow!("timeout writing SSE endpoint event"))??;
    socket.flush().await?;

    // ── Subscribe to broadcast channel ─────────────────────────────────
    let mut rx = sse_broadcaster.subscribe();
    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(30));

    loop {
        tokio::select! {
            result = rx.recv() => {
                match result {
                    Ok(payload) => {
                        let frame = format!("event: message\ndata: {}\n\n", payload);
                        if tokio::time::timeout(
                            std::time::Duration::from_secs(30),
                            socket.write_all(frame.as_bytes()),
                        )
                        .await
                        .is_err()
                        {
                            // Client disconnected
                            break;
                        }
                        let _ = socket.flush().await;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        debug!("MCP SSE consumer lagged by {} messages", n);
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        // Broadcaster was closed — stop
                        break;
                    }
                }
            }
            _ = heartbeat.tick() => {
                // SSE keepalive heartbeat — prevents proxies from closing
                // idle connections.
                let heartbeat_event = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "ping",
                    "params": {}
                });
                let payload = serde_json::to_string(&heartbeat_event).unwrap_or_default();
                let frame = format!("event: message\ndata: {}\n\n", payload);
                if tokio::time::timeout(
                    std::time::Duration::from_secs(30),
                    socket.write_all(frame.as_bytes()),
                )
                .await
                .is_err()
                {
                    break;
                }
                let _ = socket.flush().await;
            }
        }
    }

    let _ = socket.shutdown().await;
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
            "HTTP/1.1 {} {}\r\nConnection: keep-alive\r\n",
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
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n",
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
    let line = serde_json::to_string(&error).map_err(std::io::Error::other)?;
    writer.write_all(line.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

/// Send a JSON-RPC Internal error response (-32603) to the client.
///
/// Used when `handle_request` returns an `Err` after successfully parsing
/// the JSON-RPC request.  If the original request `id` is unavailable, a
/// `null` id is sent per the JSON-RPC 2.0 spec.
async fn send_handler_error(
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    id: Option<serde_json::Value>,
    error_message: &str,
) -> std::io::Result<()> {
    let error = json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": -32603,
            "message": error_message
        }
    });
    let line = serde_json::to_string(&error).map_err(std::io::Error::other)?;
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
            None,
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
