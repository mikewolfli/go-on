//! Server-Sent Events (SSE) streaming support
//!
//! Contains SSE header writing, event framing, and OpenAI-style SSE data
//! writing functions. Extracted from the parent `runtime.rs` to reduce
//! the monolithic file size.

use anyhow::Result;
use tokio::io::AsyncWriteExt;

use super::tcp_write_timeout;

/// Write SSE response headers (Content-Type: text/event-stream) to a socket.
pub(crate) async fn write_sse_headers(
    socket: &mut (impl tokio::io::AsyncWrite + Unpin),
    extra_headers: &str,
) -> Result<()> {
    let header_bytes = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\nX-Accel-Buffering: no\r\n{}\r\n",
        extra_headers
    );
    tcp_write_timeout(socket, header_bytes.as_bytes()).await?;
    Ok(())
}

/// Write a single SSE event frame to a socket.
pub(crate) async fn write_sse_event(
    socket: &mut (impl tokio::io::AsyncWrite + Unpin),
    event: &str,
    payload: &serde_json::Value,
) -> Result<()> {
    // Use a pooled buffer to avoid allocation churn during high-frequency
    // SSE streaming.  The buffer is released back to the pool after writing.
    let mut frame = crate::acp::r#impl::chat::acquire_sse_buffer();
    frame.extend_from_slice(b"event: ");
    frame.extend_from_slice(event.as_bytes());
    frame.extend_from_slice(b"\ndata: ");
    serde_json::to_writer(&mut frame, payload)?;
    frame.extend_from_slice(b"\n\n");
    tracing::debug!("ACP SSE event: {}", event);
    tcp_write_timeout(socket, &frame).await?;
    // Release the buffer back to the pool immediately after writing;
    // the flush below only synchronises the socket, not the buffer.
    crate::acp::r#impl::chat::release_sse_buffer(frame);
    tokio::time::timeout(std::time::Duration::from_secs(30), socket.flush())
        .await
        .map_err(|_| anyhow::anyhow!("timeout flushing socket"))?
        .map_err(|e| anyhow::anyhow!("socket flush error: {e}"))?;
    Ok(())
}

/// Write an OpenAI-style SSE `data:` frame.
pub(crate) async fn write_openai_sse_data(
    socket: &mut (impl tokio::io::AsyncWrite + Unpin),
    payload: &serde_json::Value,
) -> Result<()> {
    let json_str = serde_json::to_string(payload)?;
    // Pre-allocate: "data: " (6) + json + "\n\n" (2)
    let mut frame = String::with_capacity(6 + json_str.len() + 2);
    frame.push_str("data: ");
    frame.push_str(&json_str);
    frame.push_str("\n\n");
    tcp_write_timeout(socket, frame.as_bytes()).await?;
    tokio::time::timeout(std::time::Duration::from_secs(30), socket.flush())
        .await
        .map_err(|_| anyhow::anyhow!("timeout flushing socket"))?
        .map_err(|e| anyhow::anyhow!("socket flush error: {e}"))?;
    Ok(())
}

/// Write the OpenAI SSE done signal (`data: [DONE]`) and close the socket.
pub(crate) async fn write_openai_sse_done(
    socket: &mut (impl tokio::io::AsyncWrite + Unpin),
) -> Result<()> {
    tcp_write_timeout(socket, b"data: [DONE]\n\n").await?;
    tokio::time::timeout(std::time::Duration::from_secs(30), socket.flush())
        .await
        .map_err(|_| anyhow::anyhow!("timeout flushing socket"))?
        .map_err(|e| anyhow::anyhow!("socket flush error: {e}"))?;
    let _ = socket.shutdown().await;
    Ok(())
}
