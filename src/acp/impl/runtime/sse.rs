//! Server-Sent Events (SSE) streaming support
//!
//! Contains SSE header writing, event framing, and OpenAI-style SSE data
//! writing functions. Extracted from the parent `runtime.rs` to reduce
//! the monolithic file size.

use anyhow::Result;
use tokio::io::AsyncWriteExt;

use super::tcp_write_timeout;

/// Periodic SSE flush interval (in events) — flushes the socket buffer every
/// N events to batch syscalls while keeping stream latency low. Single
/// definition shared by the ACP HTTP arm (`runtime/http.rs`) and the
/// OpenAI-compat arm (`runtime/openai_compat.rs`).
pub(crate) const SSE_FLUSH_INTERVAL: usize = 4;

/// Write SSE response headers (Content-Type: text/event-stream) to a socket.
///
/// `connection` selects the `Connection` header value: `"keep-alive"` for
/// long-lived MCP SSE sessions, `"close"` for one-shot ACP SSE streams.
pub(crate) async fn write_sse_headers(
    socket: &mut (impl tokio::io::AsyncWrite + Unpin),
    connection: &str,
    extra_headers: &str,
) -> Result<()> {
    let header_bytes = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: {}\r\nX-Accel-Buffering: no\r\n{}\r\n",
        connection, extra_headers
    );
    tcp_write_timeout(socket, header_bytes.as_bytes()).await?;
    Ok(())
}

/// Write a single SSE event frame to a socket.
///
/// # Performance
///
/// This function does NOT flush after every write — flushing is the caller's
/// responsibility.  For high-frequency streaming (token chunks), the HTTP
/// handler loop should call [`flush_sse`] periodically (every N events or
/// after a batch).  This avoids one syscall per SSE event, which is the
/// dominant cost in high-throughput streaming.
pub(crate) async fn write_sse_event(
    socket: &mut (impl tokio::io::AsyncWrite + Unpin),
    event: &str,
    payload: &serde_json::Value,
) -> Result<()> {
    write_sse_frame(socket, Some(event), |frame| {
        serde_json::to_writer(frame, payload).map_err(Into::into)
    })
    .await
}

/// Write a single SSE event frame with a pre-serialized `data:` payload.
///
/// Some consumers (e.g. the MCP SSE endpoint) broadcast already-serialized
/// JSON-RPC strings and must not re-encode them through [`write_sse_event`]
/// (that would add JSON quoting around the payload). This writes the same
/// `event: <name>\ndata: <raw>\n\n` frame layout so the framing logic lives
/// in one place.
pub(crate) async fn write_sse_raw_event(
    socket: &mut (impl tokio::io::AsyncWrite + Unpin),
    event: &str,
    data: &str,
) -> Result<()> {
    write_sse_frame(socket, Some(event), |frame| {
        frame.extend_from_slice(data.as_bytes());
        Ok(())
    })
    .await
}

/// Flush pending SSE data to the socket.
/// Call this periodically during streaming (e.g. every 10 events or every 10ms).
pub(crate) async fn flush_sse(socket: &mut (impl tokio::io::AsyncWrite + Unpin)) -> Result<()> {
    tokio::time::timeout(
        crate::shared::http_timeouts::SSE_FLUSH_TIMEOUT,
        socket.flush(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("timeout flushing socket"))?
    .map_err(|e| anyhow::anyhow!("socket flush error: {e}"))?;
    Ok(())
}

/// Write an OpenAI-style SSE `data:` frame.
///
/// # Performance
///
/// This function does NOT flush — flushing is the caller's responsibility.
/// The HTTP handler loop should call [`flush_sse`] periodically (every N
/// events or after a batch) to batch syscalls.
pub(crate) async fn write_openai_sse_data(
    socket: &mut (impl tokio::io::AsyncWrite + Unpin),
    payload: &serde_json::Value,
) -> Result<()> {
    write_sse_frame(socket, None, |frame| {
        serde_json::to_writer(frame, payload).map_err(Into::into)
    })
    .await
}

/// Write the OpenAI SSE done signal (`data: [DONE]`) and close the socket.
pub(crate) async fn write_openai_sse_done(
    socket: &mut (impl tokio::io::AsyncWrite + Unpin),
) -> Result<()> {
    write_sse_frame(socket, None, |frame| {
        frame.extend_from_slice(b"[DONE]");
        Ok(())
    })
    .await?;
    flush_sse(socket).await?;
    let _ = socket.shutdown().await;
    Ok(())
}

/// Assemble an SSE frame in the pooled buffer and write it to the socket.
///
/// Frame layout: `event: <name>\ndata: <payload>\n\n` when `event` is
/// `Some`, otherwise `data: <payload>\n\n` (OpenAI-style frames). The
/// `write_payload` closure serializes the payload directly into the pooled
/// buffer, so the streaming hot path avoids a per-frame heap allocation.
/// Single place where the frame bytes are constructed (previously each
/// writer duplicated the `event:`/`data:`/`\n\n` layout).
async fn write_sse_frame(
    socket: &mut (impl tokio::io::AsyncWrite + Unpin),
    event: Option<&str>,
    write_payload: impl FnOnce(&mut Vec<u8>) -> Result<()>,
) -> Result<()> {
    let mut frame = crate::acp::r#impl::chat::acquire_sse_buffer();
    if let Some(event) = event {
        frame.extend_from_slice(b"event: ");
        frame.extend_from_slice(event.as_bytes());
        frame.extend_from_slice(b"\ndata: ");
    } else {
        frame.extend_from_slice(b"data: ");
    }
    write_payload(&mut frame)?;
    frame.extend_from_slice(b"\n\n");
    tcp_write_timeout(socket, &frame).await?;
    // Release the buffer back to the pool immediately after writing.
    crate::acp::r#impl::chat::release_sse_buffer(frame);
    Ok(())
}
