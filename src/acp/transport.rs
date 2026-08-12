//! Transport trait — unified I/O abstraction for ACP server.
//!
//! Replaces the implicit `RPC_BUFFER` task-local mechanism with an explicit
//! trait, making transport dispatch visible in function signatures and
//! extensible to new transports (WebSocket, Unix socket).
//!
//! # Architecture
//!
//! Three implementations exist:
//! - `StdioTransport` — writes JSON-RPC to tokio::stdout (no buffer)
//! - `RpcBufferTransport` — writes JSON-RPC to Arc<Mutex<Vec<u8>>> (HTTP RPC)
//! - `SseTransport` — writes SSE events to TcpStream (HTTP SSE/streaming)
//!
//! # Current Transport
//!
//! The current transport is a *task-local* (`with_transport`), set during
//! server startup (stdio mode) or per-request (HTTP RPC/SSE mode). When set,
//! it takes priority over the stdout fallback. Task-local scoping means
//! concurrent HTTP requests each write through their own transport — the
//! previous process-wide global was overwritten by concurrent connections,
//! crossing responses between requests.
use anyhow::Result;
use serde_json::Value;
use std::sync::Arc;
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
/// Used by the ACP HTTP RPC mode (`/rpc` endpoint). Also tracks the most
/// recent response (value carrying an `id`), so the HTTP handler can emit it
/// directly without re-parsing the serialized buffer (the old bytes → String →
/// Value round trip existed only to pick the last `id`-bearing response).
pub(crate) struct RpcBufferTransport {
    buffer: Arc<Mutex<Vec<u8>>>,
    last_response: Arc<Mutex<Option<Value>>>,
}

impl RpcBufferTransport {
    pub(crate) fn new(buffer: Arc<Mutex<Vec<u8>>>) -> Self {
        Self {
            buffer,
            last_response: Arc::new(Mutex::new(None)),
        }
    }

    /// Most recent response written through this transport (value with an `id`).
    pub(crate) async fn last_response(&self) -> Option<Value> {
        self.last_response.lock().await.clone()
    }
}

#[async_trait::async_trait]
impl Transport for RpcBufferTransport {
    async fn write_json_line(&self, value: &Value) -> Result<()> {
        let mut buf = self.buffer.lock().await;
        let mut encoded = serde_json::to_vec(value)?;
        encoded.push(b'\n');
        buf.extend_from_slice(&encoded);
        if value.get("id").is_some() {
            *self.last_response.lock().await = Some(value.clone());
        }
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

// ── Task-local current transport ────────────────────────────────────────────

// Task-local current transport for ACP server output.
//
// Previously a process-wide global (`RwLock<Option<Arc<dyn Transport>>>`)
// which concurrent HTTP connections overwrote: request A's session/update or
// permission writes landed in request B's buffer/socket (responses crossed
// between requests), and `clear_current_transport()` after one request could
// clear the transport a concurrent request had just set. Task-local storage
// is per-task and propagates to `tokio::spawn`ed subtasks, so every request
// writes through its own transport for its whole lifetime.
tokio::task_local! {
    static CURRENT_TRANSPORT: Arc<dyn Transport>;
}

/// Run `fut` with `transport` as the current transport for this task (and any
/// tasks it spawns). The value is scoped to the returned future: when it
/// completes, the task-local is cleared automatically (no explicit clear).
pub(crate) async fn with_transport<T, F>(transport: Arc<dyn Transport>, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    CURRENT_TRANSPORT.scope(transport, fut).await
}

/// Get the current task's transport, if any.
pub(crate) fn get_current_transport() -> Option<Arc<dyn Transport>> {
    CURRENT_TRANSPORT.try_with(|t| t.clone()).ok()
}
