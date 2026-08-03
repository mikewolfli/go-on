//! Transport trait — unified I/O abstraction for ACP server.
//!
//! Replaces the implicit `RPC_BUFFER` task-local mechanism with an explicit
//! trait, making transport dispatch visible in function signatures and
//! extensible to new transports (WebSocket, Unix socket).
//!
/// # Architecture
///
/// Three implementations exist:
/// - `StdioTransport` — writes JSON-RPC to tokio::stdout (no buffer)
/// - `RpcBufferTransport` — writes JSON-RPC to Arc<Mutex<Vec<u8>>> (HTTP RPC)
/// - `SseTransport` — writes SSE events to TcpStream (HTTP SSE/streaming)
///
/// # Global Transport
///
/// A global `CURRENT_TRANSPORT` (OnceLock<Arc<dyn Transport>>) is set during
/// server startup (stdio mode) or per-request (HTTP RPC mode). When set, it
/// takes priority over the legacy `RPC_BUFFER` task-local and stdout fallback.
use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;
use std::sync::RwLock;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

/// Unified transport abstraction for ACP protocol output.
///
/// Each transport implementation handles a specific output destination:
/// - Stdio: write JSON-RPC lines to stdout
/// - RPC Buffer: capture JSON-RPC lines into a Vec<u8> for HTTP response
/// - SSE: write SSE frames to a TCP socket
///
/// # Migration Strategy
/// Phase 1: Keep both `RPC_BUFFER` and `Transport` existing in parallel.
/// Phase 2: Route through global `CURRENT_TRANSPORT` with fallback.
/// Phase 3 (current): `write_json_line` routes through CURRENT_TRANSPORT,
///   then falls back to RPC_BUFFER or stdout. Both RpcBufferTransport and
///   SseTransport are wired into HTTP handlers.
/// Phase 4: Add WebSocket/Unix socket transports.
#[async_trait::async_trait]
pub(crate) trait Transport: Send + Sync {
    /// Write a JSON-RPC line to the transport.
    async fn write_json_line(&self, value: &Value) -> Result<()>;
}

/// Transport that writes JSON-RPC lines directly to stdout.
///
/// Used by the ACP stdio server mode.
pub(crate) struct StdioTransport;

#[async_trait::async_trait]
impl Transport for StdioTransport {
    async fn write_json_line(&self, value: &Value) -> Result<()> {
        let mut encoded = serde_json::to_vec(value)?;
        encoded.push(b'\n');
        tokio::io::stdout().write_all(&encoded).await?;
        tokio::io::stdout().flush().await?;
        Ok(())
    }
}

/// Transport that captures JSON-RPC lines into a buffer for HTTP response.
///
/// Used by the ACP HTTP RPC mode (`/rpc` endpoint).
pub(crate) struct RpcBufferTransport {
    buffer: Arc<Mutex<Vec<u8>>>,
}

impl RpcBufferTransport {
    pub(crate) fn new(buffer: Arc<Mutex<Vec<u8>>>) -> Self {
        Self { buffer }
    }
}

#[async_trait::async_trait]
impl Transport for RpcBufferTransport {
    async fn write_json_line(&self, value: &Value) -> Result<()> {
        let mut buf = self.buffer.lock().await;
        let mut encoded = serde_json::to_vec(value)?;
        encoded.push(b'\n');
        buf.extend_from_slice(&encoded);
        Ok(())
    }
}

/// Transport that writes SSE frames to a TCP stream.
///
/// Used by the ACP HTTP SSE mode (`/chat/stream`, `/v1/*`).
pub(crate) struct SseTransport {
    socket: Arc<tokio::sync::Mutex<tokio::net::TcpStream>>,
}

impl SseTransport {
    pub(crate) fn new(socket: tokio::net::TcpStream) -> Self {
        Self {
            socket: Arc::new(Mutex::new(socket)),
        }
    }
}

#[async_trait::async_trait]
impl Transport for SseTransport {
    async fn write_json_line(&self, value: &Value) -> Result<()> {
        // SSE doesn't carry bare JSON-RPC lines — frame them as `event: message`
        // (the JSON-RPC-over-SSE convention, matching MCP) so session updates,
        // permission round-trips, and tool-call notifications are no longer
        // silently dropped on the SSE transport.
        let mut socket = self.socket.lock().await;
        crate::acp::r#impl::runtime::sse::write_sse_event(&mut *socket, "message", value).await
    }
}

// ── Global current transport ──────────────────────────────────────────────────

/// Global current transport for ACP server output.
///
/// Set during server startup (stdio mode) or per-request (HTTP RPC/SSE mode).
/// Uses RwLock rather than OnceLock so it can be replaced during test setup
/// and when switching between stdio/HTTP/SSE modes.
static CURRENT_TRANSPORT: RwLock<Option<Arc<dyn Transport>>> = RwLock::new(None);

/// Set the global transport for JSON-RPC output.
/// Replaces any previously set transport.
pub(crate) fn set_current_transport(transport: Arc<dyn Transport>) {
    if let Ok(mut guard) = CURRENT_TRANSPORT.write() {
        *guard = Some(transport);
    }
}

/// Clear the global transport.
///
/// Must be called after per-request HTTP/SSE handling completes: the
/// `SseTransport` holds an `Arc` to the request's socket, so keeping it in
/// the global would pin the TCP connection open indefinitely.
pub(crate) fn clear_current_transport() {
    if let Ok(mut guard) = CURRENT_TRANSPORT.write() {
        *guard = None;
    }
}

/// Get the current global transport, if any.
pub(crate) fn get_current_transport() -> Option<Arc<dyn Transport>> {
    CURRENT_TRANSPORT
        .read()
        .ok()
        .and_then(|guard| guard.clone())
}
