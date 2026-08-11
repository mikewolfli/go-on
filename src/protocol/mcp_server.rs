//! MCP Server implementation with stdio + HTTP transports
//!
//! Provides a JSON-RPC 2.0 server that communicates over stdin/stdout
//! (`McpStdioServer`) and HTTP/SSE (`McpHttpServer`), implementing the Model
//! Context Protocol specification.

use anyhow::Result;
use futures_util::future::join_all;
use serde_json::json;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::sync::{Mutex, Notify};
use tracing::{debug, info, warn};

use crate::acp::r#impl::request::inject_platform_profiles_if_absent;
use crate::acp::r#impl::runtime::protocol::{parse_http_request, ParsedHttpRequest};
use crate::acp::r#impl::runtime::sse::write_sse_raw_event;
use crate::acp::server::AcpServer;
use crate::agent::AgentRegistry;
use crate::governance::rbac::{AccessDecision, Permission, Principal};
use crate::i18n::runtime::{t, tf};
use crate::mcp::{JsonRpcError, JsonRpcRequest, JsonRpcResponse, McpServer};
use crate::security::mtls::MtlsAcceptor;
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

    /// Run the server (reads from stdin, writes to stdout)
    pub async fn run(&self) -> Result<()> {
        let stdout = tokio::io::stdout();

        // Read stdin on a dedicated plain OS thread (shared with the ACP stdio
        // loop). tokio::io::stdin() is a blocking read on the blocking pool that
        // cannot be cancelled and hangs shutdown — see shared::stdio.
        let mut stdin_rx = crate::shared::stdio::spawn_stdin_lines();

        let stdout = Arc::new(Mutex::new(stdout));

        // ── Shutdown coordination ──────────────────────────────────────
        // Reuse the platform-gated signal watcher (SIGINT/SIGTERM on Unix,
        // Ctrl-C elsewhere) — the previous inline `signal::unix::signal`
        // calls did not compile on Windows.
        let shutdown_notify = Arc::new(Notify::new());
        let sig_notify = shutdown_notify.clone();
        tokio::spawn(async move {
            crate::shared::tcp_accept_loop::shutdown_signal().await;
            sig_notify.notify_one();
        });

        loop {
            tokio::select! {
            _ = shutdown_notify.notified() => {
                info!("MCP stdio: shutting down gracefully");
                break;
            }
            line = stdin_rx.recv() => {
                // None = stdin EOF (client closed the pipe) → shut down.
                let Some(line) = line else { break };

                // Skip empty lines
                if line.trim().is_empty() {
                    continue;
                }

                let line_str = line.trim().to_string();

                        // Attempt batch (JSON array) first, then fall back to single request.
                        if line_str.starts_with('[') {
                            match serde_json::from_str::<Vec<JsonRpcRequest>>(&line_str) {
                                Ok(requests) => {
                                    // Process each request in the batch concurrently
                                    // (matching the HTTP transport) while preserving
                                    // request order in the response stream.
                                    let responses = join_all(requests.into_iter().map(|req| async {
                                        let req_id = req.id.clone();
                                        match self.mcp_server.handle_request(req).await {
                                            Ok(resp) => {
                                                // Notifications (id=null or id=Value::Null sentinel)
                                                // don't produce a response per JSON-RPC 2.0.
                                                if resp.id.is_none()
                                                    || resp.id == Some(serde_json::Value::Null)
                                                {
                                                    None
                                                } else {
                                                    Some(resp)
                                                }
                                            }
                                            Err(e) => {
                                                let err_msg = format!("{}", e);
                                                warn!(
                                                    "{}",
                                                    tf(
                                                        "error.handling_request",
                                                        &[("error", &err_msg)],
                                                    )
                                                );
                                                // Mirror the serial path: even a failed
                                                // notification still gets an error line
                                                // (id null) so the client learns of the
                                                // failure; the code is mapped from the
                                                // underlying error, not hardcoded.
                                                Some(JsonRpcResponse {
                                                    jsonrpc: "2.0".to_string(),
                                                    result: None,
                                                    error: Some(JsonRpcError {
                                                        code: crate::mcp::error_code_for(&e),
                                                        message: err_msg,
                                                        data: None,
                                                    }),
                                                    id: req_id,
                                                })
                                            }
                                        }
                                    }))
                                    .await
                                    .into_iter()
                                    .flatten()
                                    .collect::<Vec<_>>();

                                    let mut stdout = stdout.lock().await;
                                    for resp in responses {
                                        let response_line = serde_json::to_string(&resp)?;
                                        stdout.write_all(response_line.as_bytes()).await?;
                                        stdout.write_all(b"\n").await?;
                                    }
                                    stdout.flush().await?;
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
                            match serde_json::from_str::<JsonRpcRequest>(&line_str) {
                                Ok(request) => {
                                    let request_id = request.id.clone();
                                    let response = self.mcp_server.handle_request(request).await;
                                    match response {
                                        Ok(resp) => {
                                            // MCP notifications (JSON-RPC with id=null or id=Value::Null
                                                // sentinel) must not produce any response per JSON-RPC 2.0 spec.
                                                if resp.id.is_none()
                                                    || resp.id == Some(serde_json::Value::Null)
                                                {
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
                                            send_handler_error(
                                                &mut *stdout,
                                                request_id,
                                                crate::mcp::error_code_for(&e),
                                                &err_msg,
                                            )
                                            .await?;
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

/// Build the JSON-RPC 2.0 notification payload for an MCP `tools/list_changed`
/// event (standard MCP subscription notification).
fn tools_list_changed_notification() -> String {
    serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/tools/list_changed",
        "params": {}
    })
    .to_string()
}

/// Publish an MCP server notification to all connected SSE subscribers.
///
/// Returns the number of active receivers the payload was delivered to (0 when
/// no SSE client is currently subscribed — a broadcast with zero receivers is
/// a no-op, not an error). This is the single send path for the SSE
/// broadcaster; the only payload published today is the
/// `notifications/tools/list_changed` notification (sent at server startup, on
/// SSE subscribe, and by `McpHttpServer::run`'s startup broadcast).
fn broadcast_sse_notification(sse_broadcaster: &SseBroadcaster, payload: String) -> usize {
    // `Err(SendError)` means zero receivers (no connected SSE client); the
    // broadcast channel never closes while the server owns the sender, so
    // a failed send is simply a no-op delivery of 0.
    sse_broadcaster.send(payload).unwrap_or_default()
}

pub struct McpHttpServer {
    mcp_server: Arc<McpServer>,
    bind_addr: String,
    shutdown_notify: Arc<Notify>,
    acp_server: Option<Arc<AcpServer>>,
    /// Optional CA certificate path for mTLS. When set, a `TlsAcceptor` is built
    /// with client CA certificate verification during `run()`.
    mtls_ca_cert_path: Option<String>,
    /// Optional server certificate path for mTLS.
    mtls_server_cert_path: Option<String>,
    /// Optional server key path for mTLS.
    mtls_server_key_path: Option<String>,
    /// Whether client certificates are required in the mTLS handshake
    /// (consumed from `RuntimeConfig.mtls_require_client_cert`).
    mtls_require_client_cert: bool,
    /// Optional rate limit middleware. When set, `check()` is called before
    /// processing each request in `handle_http_connection`.
    rate_limiter: Option<Arc<crate::protocol::rate_limit::RateLimitMiddleware>>,
    /// SSE broadcaster for pushing MCP notifications to connected SSE clients.
    /// Only the `notifications/tools/list_changed` notification is currently
    /// published through this channel (see `tools_list_changed_notification`).
    sse_broadcaster: Arc<SseBroadcaster>,
}

impl McpHttpServer {
    /// Create a new MCP HTTP server without ACP features.
    ///
    /// Thin wrapper over [`Self::new_with_acp`] with `acp_server = None`;
    /// kept for API convenience and test call sites. Prefer
    /// [`Self::new_with_acp`] when an `AcpServer` reference is available so
    /// CORS/entry-auth/rate-limiting hardening applies to MCP HTTP.
    pub fn new(
        agent_registry: Arc<AgentRegistry>,
        tool_registry: Arc<ToolRegistry>,
        server_name: String,
        server_version: String,
        bind_addr: String,
    ) -> Self {
        Self::new_with_acp(
            agent_registry,
            tool_registry,
            server_name,
            server_version,
            bind_addr,
            None,
        )
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
        );
        Self {
            mcp_server: Arc::new(mcp_server),
            bind_addr,
            shutdown_notify: Arc::new(Notify::new()),
            acp_server,
            mtls_ca_cert_path: None,
            mtls_server_cert_path: None,
            mtls_server_key_path: None,
            mtls_require_client_cert: false,
            rate_limiter: None,
            sse_broadcaster,
        }
    }

    /// Run the HTTP server
    pub async fn run(&self) -> Result<()> {
        // Lazy initialise the TLS acceptor from mTLS config. This wires
        // client CA certificate verification through the MtlsAcceptor's
        // build_server_config. (The former `tls_acceptor` field had no
        // setter and was always `None`, so the `is_some()` branch was
        // unreachable; the mTLS config paths are the only way to get TLS.)
        let effective_acceptor: Option<tokio_rustls::TlsAcceptor> =
            if let (Some(ca), Some(cert), Some(key)) = (
                self.mtls_ca_cert_path.as_ref(),
                self.mtls_server_cert_path.as_ref(),
                self.mtls_server_key_path.as_ref(),
            ) {
                let mtls_acceptor = MtlsAcceptor::new(ca.as_str(), cert.as_str(), key.as_str());
                // Consume `mtls_require_client_cert` (previously the MCP arm never
                // read the switch, so client-cert verification was always off while
                // the ACP arm hardcoded it on). The field defaults to `false`;
                // deployments that want mTLS must set it to `true`.
                let mtls_acceptor = mtls_acceptor.with_client_cert(self.mtls_require_client_cert);
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
        let listener = crate::shared::tcp_accept_loop::bind_tcp_listener(&self.bind_addr).await?;

        info!("{}", t("info.mcp_server_operational"));
        debug!(
            "{}",
            tf(
                "debug.mcp_server_accepting",
                &[("address", &self.bind_addr)]
            )
        );

        // ── SSE notification: tool/resource registries initialized ───────
        // The tool registry is fully registered before the server is
        // constructed (see `transport_factory::dispatch_server`), so this
        // startup broadcast marks the end of tool registration. Any SSE
        // client already connected (e.g. reconnect) receives the
        // `tools/list_changed` notification and re-fetches the tool list.
        let delivered =
            broadcast_sse_notification(&self.sse_broadcaster, tools_list_changed_notification());
        if delivered > 0 {
            info!(
                "MCP HTTP: broadcast tools/list_changed on startup to {} SSE subscriber(s)",
                delivered
            );
        }

        // Shared accept loop: signal handling (SIGINT/SIGTERM/notify), accept
        // dispatch, and per-connection spawn live in
        // `shared::tcp_accept_loop`. Protocol-specific concerns (TLS wrapping,
        // JSON-RPC dispatch) stay in the closure below.
        let shutdown_notify = Arc::clone(&self.shutdown_notify);
        let mcp_server = Arc::clone(&self.mcp_server);
        let acp_server = self.acp_server.clone();
        let tls_acceptor = effective_acceptor.clone();
        let rate_limiter = self.rate_limiter.clone();
        let sse_broadcaster = Arc::clone(&self.sse_broadcaster);
        // Stop accepting new connections once the server enters drain mode
        // (mirrors the ACP HTTP arm; previously this arm passed `|| false` so
        // drain never stopped the accept loop and the post-loop drain sleep
        // was the only shutdown delay).
        let drain_acp_server = acp_server.clone();
        crate::shared::tcp_accept_loop::run_http_accept_loop(
            listener,
            shutdown_notify,
            256,
            move || {
                drain_acp_server
                    .as_ref()
                    .map(|s| s.drain_guard.is_draining())
                    .unwrap_or(false)
            },
            std::sync::Arc::new(
                move |socket: tokio::net::TcpStream, peer_addr: std::net::SocketAddr| {
                    let mcp_server = Arc::clone(&mcp_server);
                    let acp_server = acp_server.clone();
                    let tls_acceptor = tls_acceptor.clone();
                    let rate_limiter = rate_limiter.clone();
                    let sse_broadcaster = Arc::clone(&sse_broadcaster);
                    async move {
                        let mut stream = match tls_acceptor {
                            Some(ref acceptor) => match acceptor.accept(socket).await {
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
                            },
                            None => MaybeTlsStream::Plain(socket),
                        };
                        if let Err(err) = handle_http_connection(
                            &mut stream,
                            mcp_server,
                            acp_server,
                            rate_limiter,
                            sse_broadcaster,
                            peer_addr,
                        )
                        .await
                        {
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
                    }
                },
            ),
        )
        .await?;

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

    /// Configure this server with a rate limit middleware.
    /// Every request processed by `handle_http_connection` will be checked
    /// against this rate limiter before processing.
    pub fn with_rate_limiter(
        mut self,
        rate_limiter: Arc<crate::protocol::rate_limit::RateLimitMiddleware>,
    ) -> Self {
        self.rate_limiter = Some(rate_limiter);
        self
    }

    /// Wire mTLS/TLS configuration from `RuntimeConfig`.
    ///
    /// Previously the acceptor fields were private with no setters, so MCP
    /// HTTP could never serve TLS/mTLS even when configured — the
    /// `effective_acceptor` logic in `run()` was unreachable dead code.
    /// ACP HTTP reads the same RuntimeConfig fields directly; this builder
    /// gives MCP HTTP parity. When all three paths are non-empty the
    /// acceptor is built lazily in `run()` (client CA verification follows
    /// `mtls_require_client_cert`).
    pub fn with_mtls_config(
        mut self,
        mtls_enabled: bool,
        mtls_ca_cert_path: &str,
        mtls_server_cert_path: &str,
        mtls_server_key_path: &str,
        mtls_require_client_cert: bool,
    ) -> Self {
        if mtls_enabled && !mtls_server_cert_path.is_empty() && !mtls_server_key_path.is_empty() {
            self.mtls_ca_cert_path = Some(mtls_ca_cert_path.to_string());
            self.mtls_server_cert_path = Some(mtls_server_cert_path.to_string());
            self.mtls_server_key_path = Some(mtls_server_key_path.to_string());
            self.mtls_require_client_cert = mtls_require_client_cert;
        }
        self
    }
}

/// Outcome of evaluating an MCP CORS preflight (`OPTIONS`) request.
enum McpPreflightOutcome {
    /// Origin is outside the whitelist — respond 403 with no CORS headers.
    Reject,
    /// Origin is allowed — respond 204 carrying these CORS headers.
    Allow { extra_headers: String },
}

/// Evaluate an MCP CORS preflight request against the server CORS config.
///
/// Pure decision logic used by `handle_http_connection` and covered directly
/// by unit tests. Semantics match the ACP HTTP arm (`handle_cors_preflight`
/// in `acp/impl/runtime/http.rs`):
///
/// - An origin outside the whitelist is rejected with 403 instead of echoing
///   `Access-Control-Allow-Origin: *` (the old MCP arm echoed `*` for
///   disallowed origins, which effectively whitelisted every browser origin).
/// - Header negotiation reads `Access-Control-Request-Headers`; the old arm
///   passed the `Origin` value there, so `Access-Control-Allow-Headers` was
///   never emitted for concrete whitelists.
/// - `*` in `allowed_origins` grants every origin, and the response then
///   echoes `*` (same as the ACP arm).
fn evaluate_mcp_preflight(
    header_part: &str,
    cfg: &crate::acp::r#impl::cors::CorsConfig,
) -> McpPreflightOutcome {
    let origin = crate::acp::r#impl::runtime::protocol::extract_header_value(header_part, "origin");
    let request_headers = crate::acp::r#impl::runtime::protocol::extract_header_value(
        header_part,
        "access-control-request-headers",
    );
    // Shared decision with the ACP HTTP arm — only the wire response differs.
    match crate::acp::r#impl::cors::evaluate_cors_preflight(
        origin.as_deref(),
        request_headers.as_deref(),
        cfg,
    ) {
        crate::acp::r#impl::cors::CorsPreflightDecision::Reject => McpPreflightOutcome::Reject,
        crate::acp::r#impl::cors::CorsPreflightDecision::Allow { extra_headers } => {
            McpPreflightOutcome::Allow { extra_headers }
        }
    }
}

async fn handle_http_connection(
    socket: &mut MaybeTlsStream,
    mcp_server: Arc<McpServer>,
    acp_server: Option<Arc<AcpServer>>,
    rate_limiter: Option<Arc<crate::protocol::rate_limit::RateLimitMiddleware>>,
    sse_broadcaster: Arc<SseBroadcaster>,
    peer_addr: std::net::SocketAddr,
) -> Result<()> {
    // ── Connection: keep-alive — compliance only, no multiplexing ──────
    // Responses include `Connection: keep-alive` (set in
    // write_http_json_response) for HTTP/1.1 spec compliance. This handler
    // processes exactly **one** request per TCP connection and then returns
    // (the connection is closed when the socket drops), so the keep-alive
    // header is metadata-only — no request multiplexing occurs today.
    //
    // Future enhancement: wrap the handler body in a loop that reads
    // subsequent requests on the same connection while `Connection:
    // keep-alive` is present, and breaks when `Connection: close` is
    // received. Until then the header must stay `keep-alive` (clients like
    // the MCP SDK rely on it) — only the comment documents the gap.
    //
    // ── Header buffer: shared canonical reader ────────────────────────────
    // The growth/termination/timeout loop is identical to the ACP HTTP arm,
    // so both arms delegate to the single `read_http_header` implementation
    // (only the error text differs, and it is localized here to keep the MCP
    // surface wording stable).
    let header_bytes = match crate::acp::r#impl::runtime::http::read_http_header(socket).await {
        Ok(bytes) => bytes,
        Err(err) => {
            let msg = err.to_string();
            if msg.contains("exceeds") {
                anyhow::bail!("{}", t("error.http_header_too_large"));
            }
            if msg.contains("incomplete HTTP request header") {
                anyhow::bail!("{}", t("error.http_missing_header"));
            }
            return Err(err);
        }
    };
    if header_bytes.is_empty() {
        // Clean EOF before any byte — no request; close quietly.
        return Ok(());
    }

    let request_text = String::from_utf8_lossy(&header_bytes);
    // Shared request-line parser: splits at the first `\r\n\r\n` and
    // whitespace-splits the request line into method/path. Semantics are
    // identical to the former inline copy (no path-prefix stripping in either
    // implementation), so both HTTP arms now share one parser.
    let ParsedHttpRequest {
        method,
        path,
        header_part,
        body_initial_part,
    } = parse_http_request(&request_text)?;

    // ── Content-Length validation (before any auth processing) ────────
    // Check Content-Length before allocating buffers to prevent OOM.
    const MAX_BODY_SIZE: usize = 10 * 1024 * 1024; // 10MB
    let content_length =
        crate::acp::r#impl::runtime::protocol::extract_content_length(header_part).unwrap_or(0);
    // ── CORS headers (computed once, reused by every error/response path) ──
    let cors_headers = match acp_server {
        Some(ref server) => crate::acp::r#impl::runtime::http::compute_cors_response_headers(
            header_part,
            server.as_ref(),
        ),
        None => String::new(),
    };
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
        write_http_json_response(socket, 413, error_body, &cors_headers).await?;
        return Ok(());
    }

    // ── SSE endpoint (GET /sse or /mcp-sse) — must be checked before the
    //     POST-only guard below so SSE connections bypass the POST requirement.
    //     MCP SSE Streamable HTTP transport per MCP spec.
    // MCP Streamable HTTP Transport §6.3
    if method == "GET" && (path == "/sse" || path == "/mcp-sse") {
        return handle_mcp_sse_connection(socket, sse_broadcaster, &cors_headers).await;
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
                match evaluate_mcp_preflight(header_part, cfg) {
                    McpPreflightOutcome::Reject => {
                        // Origin outside the whitelist: refuse the preflight
                        // with 403 instead of echoing `Access-Control-Allow-
                        // Origin: *` (echoing `*` would implicitly whitelist
                        // every browser origin). Same semantics as the ACP arm
                        // (`handle_cors_preflight` in acp/impl/runtime/http.rs).
                        write_http_json_response(
                            socket,
                            403,
                            serde_json::json!({"error": "Origin not allowed"}),
                            "",
                        )
                        .await?;
                        return Ok(());
                    }
                    McpPreflightOutcome::Allow { extra_headers } => {
                        // CORS preflight — return 204 No Content per HTTP spec
                        write_http_json_response(
                            socket,
                            204,
                            serde_json::json!(null),
                            &extra_headers,
                        )
                        .await?;
                        return Ok(());
                    }
                }
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

    // ── Entry auth (shared evaluator with the ACP HTTP arm) ───────────────
    if let Some(ref server) = acp_server {
        match crate::acp::r#impl::runtime::security::evaluate_entry_auth(server, header_part) {
            crate::acp::r#impl::runtime::security::EntryAuthOutcome::Pass => {}
            crate::acp::r#impl::runtime::security::EntryAuthOutcome::Reject {
                status,
                code,
                message,
            } => {
                write_http_json_response(
                    socket,
                    status,
                    serde_json::json!({ "error": message, "code": code }),
                    &cors_headers,
                )
                .await?;
                return Ok(());
            }
        }

        // Entry rate limiting (per-IP), shared with the ACP HTTP arm via the
        // single `entry_rate_limit_allowed` implementation.
        //
        // Strategy (both arms): entry rate limiting is an independent
        // per-IP flood-protection layer and is applied unconditionally — it is
        // NOT bound to `entry_auth_enabled`. Rationale: the knobs
        // `entry_rate_limit_rpm`/`entry_rate_limit_burst` are independently
        // configurable from entry auth, governance status reports the quota
        // component separately from auth, and binding the limit to auth would
        // disable per-IP protection under the default (auth-disabled)
        // configuration. The transport-level TenantRateLimit middleware is a
        // different dimension (per-tenant, not per-IP), so applying both is
        // complementing, not doubling — the ACP arm has always done so.
        let source = peer_addr.ip().to_string();
        let allowed =
            crate::acp::r#impl::runtime::security::entry_rate_limit_allowed(server, &source);
        if !allowed {
            write_http_json_response(
                socket,
                429,
                serde_json::json!({
                    "error": t("error.chat.rate_limited"),
                    "code": "ENTRY_RATE_LIMITED"
                }),
                &cors_headers,
            )
            .await?;
            return Ok(());
        }
    }

    // ── User auth and RBAC ───────────────────────────────────────────────
    // Extract the user session once; the auth block and the rate-limit tenant
    // derivation below both consume it.
    let extracted_user = acp_server
        .as_ref()
        .and_then(|s| s.session.session_manager.as_ref())
        .and_then(|sm| sm.extract_user_from_request(header_part));
    if let Some(ref server) = acp_server {
        let user_session = extracted_user.as_ref();

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
        let tenant_id = extracted_user
            .as_ref()
            .and_then(|u| u.tenant_id.clone())
            .unwrap_or_else(|| "default".to_string());

        if let Err(retry_after) = limiter.check(&tenant_id) {
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
            write_http_json_response(socket, 429, error_body, &cors_headers).await?;
            return Ok(());
        }
    }

    // Read the remaining body (content_length was already parsed and bounded
    // by MAX_BODY_SIZE at the top of this handler).
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

        // Process each request in the batch concurrently. JSON-RPC 2.0 allows
        // batch responses in any order; join_all preserves input order, so the
        // response array stays deterministic.
        let responses = join_all(requests.into_iter().map(|req| async {
            let req_id = req.id.clone();
            match mcp_server.handle_request(req).await {
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
                            // Same error-code mapping as the single-request
                            // path, so e.g. METHOD_NOT_FOUND keeps its code
                            // instead of collapsing into INTERNAL_ERROR.
                            code: crate::mcp::error_code_for(&e),
                            message: format!("{}", e),
                            data: None,
                        }),
                        id: req_id,
                    }
                }
            }
        }))
        .await
        .into_iter()
        // Per JSON-RPC 2.0, notifications (id=null) in a batch do not
        // produce a response entry. The `Some(Value::Null)` sentinel used by
        // notifications/initialized must also be filtered here, matching the
        // stdio transport (id is None OR the null sentinel).
        .filter(|resp| resp.id.is_some() && resp.id != Some(serde_json::Value::Null))
        .collect::<Vec<_>>();

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

    let req_id = request.id.clone();
    let response = match mcp_server.handle_request(request).await {
        Ok(resp) => resp,
        Err(e) => {
            warn!(
                "MCP HTTP: error handling request from {} {}: {}",
                method, path, e
            );
            // Keep the connection alive: write a JSON-RPC error response with
            // the error code mapped by `error_code_for` (same shape as the
            // parse-error branch) instead of propagating the Err, which would
            // drop the connection without a response.
            let error_data =
                inject_platform_profiles_if_absent(serde_json::json!({}), "mcp.handler_error");
            let error_response = JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                result: None,
                error: Some(JsonRpcError {
                    code: crate::mcp::error_code_for(&e),
                    message: format!("{}", e),
                    data: Some(error_data),
                }),
                id: req_id,
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
    debug!("MCP HTTP: dispatched {} {} -> ok", method, path);

    // MCP notifications (JSON-RPC with id=null) must not produce
    // any response per JSON-RPC 2.0 spec (§notifications). The
    // `Some(Value::Null)` sentinel used by notifications/initialized must
    // be treated the same way as id=None, matching the stdio transport.
    if response.id.is_none() || response.id == Some(serde_json::Value::Null) {
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
    cors_headers: &str,
) -> Result<()> {
    // ── SSE headers ───────────────────────────────────────────────────
    // Per the MCP Streamable HTTP spec, the SSE endpoint must advertise
    // the POST endpoint URL and keep the connection alive. CORS headers are
    // the same config-driven ones as every other MCP response (previously a
    // hardcoded `Access-Control-Allow-Origin: *` bypassed the whitelist).
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\nX-Accel-Buffering: no\r\n{}\r\n\r\n",
        cors_headers
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
    // send its requests. Frame written via the shared SSE framing helper
    // (same `event: <name>\ndata: <raw>\n\n` layout as the ACP runtime).
    write_sse_raw_event(socket, "endpoint", "/mcp").await?;
    socket.flush().await?;

    // ── Subscribe to broadcast channel ─────────────────────────────────
    let mut rx = sse_broadcaster.subscribe();

    // ── Push current tool-list state through the broadcast path ─────────
    // The new subscriber has already subscribed above, so this send is
    // delivered through `rx.recv()` below — guaranteeing the client receives
    // the current tool list immediately instead of waiting for the first
    // change. This also keeps the broadcast channel a live producer path.
    let _ = broadcast_sse_notification(&sse_broadcaster, tools_list_changed_notification());

    let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(30));

    loop {
        tokio::select! {
            result = rx.recv() => {
                match result {
                    Ok(payload) => {
                        if write_sse_raw_event(socket, "message", &payload).await.is_err() {
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
                if write_sse_raw_event(socket, "message", &payload).await.is_err() {
                    break;
                }
                let _ = socket.flush().await;
            }
        }
    }

    let _ = socket.shutdown().await;
    Ok(())
}

// ---------------------------------------------------------------------------
// Helper functions for MCP HTTP security hardening
// ---------------------------------------------------------------------------

/// MCP HTTP JSON response writer — delegates to the shared generic writer in
/// the ACP HTTP runtime (status-code table, extra-header handling and
/// `Connection: keep-alive` semantics are the single implementation). The old
/// per-file copy (with a drifted status table and a local 204 branch) is gone.
async fn write_http_json_response(
    socket: &mut MaybeTlsStream,
    status: u16,
    body: serde_json::Value,
    extra_headers: &str,
) -> Result<()> {
    crate::acp::r#impl::runtime::http::write_http_json_response_keep_alive(
        socket,
        status,
        body,
        extra_headers,
    )
    .await
}

#[cfg(test)]
fn content_length_for_test(headers: &str) -> Option<usize> {
    crate::acp::r#impl::runtime::protocol::extract_content_length(headers)
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

/// Send a JSON-RPC error response to the client.
///
/// Used when `handle_request` returns an `Err` after successfully parsing
/// the JSON-RPC request. The error `code` is mapped from the underlying error
/// by `crate::mcp::error_code_for` (e.g. McpCodeError keeps its own code
/// instead of always falling back to -32603). If the original request `id`
/// is unavailable, a `null` id is sent per the JSON-RPC 2.0 spec.
async fn send_handler_error(
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    id: Option<serde_json::Value>,
    code: i32,
    error_message: &str,
) -> std::io::Result<()> {
    let error = json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
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
        let server = McpStdioServer::new(
            agent_registry,
            tool_registry,
            "go-on".to_string(),
            "1.0.0".to_string(),
            None,
        );

        // Real handshake behavior on the inner MCP server: initialize must
        // negotiate the protocol version and advertise capabilities, and a
        // subsequent ping must be answered — proving the server is functional
        // after construction, not merely constructible.
        let initialize = server
            .mcp_server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "initialize".to_string(),
                params: Some(json!({ "protocolVersion": "2024-11-05" })),
                id: Some(json!(0)),
            })
            .await
            .expect("initialize must produce a response");
        assert!(
            initialize.error.is_none(),
            "initialize must not error, got: {:?}",
            initialize.error
        );
        let result = initialize.result.expect("initialize must carry a result");
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert_eq!(result["serverInfo"]["name"], "go-on");
        assert!(
            result["capabilities"].is_object(),
            "initialize must advertise capabilities"
        );

        let ping = server
            .mcp_server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "ping".to_string(),
                params: Some(json!({})),
                id: Some(json!(1)),
            })
            .await
            .expect("ping must produce a response");
        assert!(
            ping.error.is_none() && ping.result.is_some(),
            "ping must be answered after initialize, got: {:?}",
            ping
        );
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
        // Exercise the production parser (the one both HTTP arms use), not a
        // test-only reimplementation.
        let parsed =
            crate::acp::r#impl::runtime::protocol::parse_http_request(request).expect("parse");
        assert_eq!(parsed.method, "GET");
        assert_eq!(parsed.path, "/health");
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
        let parsed =
            crate::acp::r#impl::runtime::protocol::parse_http_request(request).expect("parse");
        assert_eq!(parsed.method, "POST");
        assert_eq!(parsed.path, "/api/v1/chat");
    }

    #[test]
    fn mcp_preflight_allowed_origin_echoes_origin_and_negotiates_headers() {
        let cfg = crate::acp::r#impl::cors::CorsConfig {
            allowed_origins: vec!["https://app.go-on.dev".to_string()],
            ..crate::acp::r#impl::cors::CorsConfig::default()
        };
        let headers = "OPTIONS /mcp HTTP/1.1\r\nHost: x\r\nOrigin: https://app.go-on.dev\r\nAccess-Control-Request-Headers: X-Api-Key\r\n\r\n";
        match evaluate_mcp_preflight(headers, &cfg) {
            McpPreflightOutcome::Allow { extra_headers } => {
                assert!(
                    extra_headers.contains("Access-Control-Allow-Origin: https://app.go-on.dev"),
                    "allowed origin must be echoed, got: {extra_headers:?}"
                );
                assert!(
                    !extra_headers.contains("Access-Control-Allow-Origin: *"),
                    "must not echo wildcard for a whitelisted origin, got: {extra_headers:?}"
                );
                // Header negotiation must be driven by
                // Access-Control-Request-Headers (not the Origin value), so
                // the requested allowed header is reflected.
                assert!(
                    extra_headers.contains("Access-Control-Allow-Headers: X-Api-Key"),
                    "requested allowed header must be reflected, got: {extra_headers:?}"
                );
            }
            McpPreflightOutcome::Reject => panic!("whitelisted origin must not be rejected"),
        }
    }

    #[test]
    fn mcp_preflight_disallowed_origin_is_rejected_not_wildcarded() {
        let cfg = crate::acp::r#impl::cors::CorsConfig {
            allowed_origins: vec!["https://app.go-on.dev".to_string()],
            ..crate::acp::r#impl::cors::CorsConfig::default()
        };
        let headers = "OPTIONS /mcp HTTP/1.1\r\nHost: x\r\nOrigin: https://evil.example\r\n\r\n";
        // Regression for the `Access-Control-Allow-Origin: *` echo: an origin
        // outside the whitelist must be rejected (403 at the HTTP layer),
        // never answered with a wildcard.
        assert!(
            matches!(
                evaluate_mcp_preflight(headers, &cfg),
                McpPreflightOutcome::Reject
            ),
            "disallowed origin must be rejected"
        );
    }

    #[test]
    fn mcp_preflight_wildcard_config_echoes_star() {
        let cfg = crate::acp::r#impl::cors::CorsConfig {
            allowed_origins: vec!["*".to_string()],
            ..crate::acp::r#impl::cors::CorsConfig::default()
        };
        let headers =
            "OPTIONS /mcp HTTP/1.1\r\nHost: x\r\nOrigin: https://anything.example\r\n\r\n";
        match evaluate_mcp_preflight(headers, &cfg) {
            McpPreflightOutcome::Allow { extra_headers } => {
                // Wildcard config allows any origin. Since the decision is
                // shared with the ACP arm, the concrete origin is echoed
                // exactly once (never combined with a `*` entry — two
                // Allow-Origin headers break credentialed cross-origin
                // requests per the Fetch spec).
                assert!(
                    extra_headers.contains("Access-Control-Allow-Origin: https://anything.example"),
                    "concrete origin must be echoed, got: {extra_headers:?}"
                );
                assert!(
                    !extra_headers.contains("Access-Control-Allow-Origin: *"),
                    "must not combine echo with wildcard, got: {extra_headers:?}"
                );
            }
            McpPreflightOutcome::Reject => panic!("wildcard config must not reject origins"),
        }
    }

    #[test]
    fn mcp_preflight_missing_origin_without_wildcard_is_rejected() {
        let cfg = crate::acp::r#impl::cors::CorsConfig {
            allowed_origins: vec!["https://app.go-on.dev".to_string()],
            ..crate::acp::r#impl::cors::CorsConfig::default()
        };
        let headers = "OPTIONS /mcp HTTP/1.1\r\nHost: x\r\n\r\n";
        assert!(
            matches!(
                evaluate_mcp_preflight(headers, &cfg),
                McpPreflightOutcome::Reject
            ),
            "missing origin must be rejected when no wildcard is configured"
        );
    }

    #[test]
    fn sse_broadcaster_delivers_tool_list_changed_to_subscribers() {
        // The server creates its broadcaster as `Arc::new(channel.0)` in
        // `McpHttpServer::new`/`new_with_acp`; replicate that channel here and
        // drive it through the exact send path the server uses so the test
        // proves the send has a producer that reaches subscribers.
        let broadcaster: SseBroadcaster = tokio::sync::broadcast::channel::<String>(256).0;
        let mut rx = broadcaster.subscribe();

        // The same notification `McpHttpServer::run` and
        // `handle_mcp_sse_connection` publish.
        let delivered = broadcast_sse_notification(&broadcaster, tools_list_changed_notification());
        assert_eq!(
            delivered, 1,
            "one active subscriber must receive the notification"
        );
        let received = rx.try_recv().expect("subscriber must receive the message");
        assert!(
            received.contains("notifications/tools/list_changed"),
            "payload must carry the MCP tools/list_changed method, got: {received}"
        );
        // The received payload is valid JSON-RPC 2.0 (as `handle_mcp_sse_connection`
        // serializes into an `event: message\ndata: ...` SSE frame).
        let parsed: serde_json::Value =
            serde_json::from_str(&received).expect("payload must be valid JSON");
        assert_eq!(parsed["jsonrpc"], "2.0");
        assert_eq!(parsed["method"], "notifications/tools/list_changed");

        // A send with no subscribers is a no-op (0 receivers), not an error —
        // this is the disconnect/reconnect case during startup.
        let empty: SseBroadcaster = tokio::sync::broadcast::channel::<String>(256).0;
        assert_eq!(broadcast_sse_notification(&empty, String::new()), 0);
    }
}
