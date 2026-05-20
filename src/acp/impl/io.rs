//! I/O implementation functions for ACP server
//!
//! This module contains standalone functions that implement I/O-related
//! functionality previously in the `impl AcpServer` block in `impl/io.rs`.
//! These functions take `AcpServer` as their first parameter to maintain
//! compatibility with the original implementation.

use anyhow::Result;
use serde_json::Value;
use tokio::io::AsyncWriteExt;

use crate::acp::server::AcpServer;
use crate::rpc_protocol::{JsonRpcError, JsonRpcResponse};

/// Send result response
///
/// This function replaces the `AcpServer::send_result` method.
pub async fn send_result(server: &AcpServer, id: Option<Value>, result: Value) -> Result<()> {
    // JSON-RPC notification (no id) must not produce a response.
    if id.is_none() {
        return Ok(());
    }
    write_response(
        server,
        JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        },
    )
    .await
}


/// Send an empty JSON object `{}` as a successful result.
pub async fn send_empty_ok(server: &AcpServer, id: Option<Value>) -> Result<()> {
    send_result(server, id, serde_json::Value::Object(serde_json::Map::new())).await
}

/// Serialize a typed struct and send it as a result.
pub async fn send_typed<T: serde::Serialize>(
    server: &AcpServer,
    id: Option<Value>,
    value: &T,
) -> Result<()> {
    let result = serde_json::to_value(value)?;
    send_result(server, id, result).await
}

/// Send error response
///
/// This function replaces the `AcpServer::send_error` method.
pub async fn send_error(
    server: &AcpServer,
    id: Option<Value>,
    code: i64,
    message: String,
    data: Option<Value>,
) -> Result<()> {
    write_response(
        server,
        JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message,
                data,
            }),
        },
    )
    .await
}

/// Send notification
///
/// This function replaces the `AcpServer::send_notification` method.
pub async fn send_notification(server: &AcpServer, method: &str, params: Value) -> Result<()> {
    let payload = serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    });
    write_json_line(server, &payload).await
}

/// Write JSON-RPC response
///
/// This function replaces the `AcpServer::write_response` method.
pub async fn write_response(server: &AcpServer, response: JsonRpcResponse) -> Result<()> {
    let value = serde_json::to_value(response)?;
    write_json_line(server, &value).await
}

/// Write JSON line to output
///
/// This function replaces the `AcpServer::write_json_line` method.
pub async fn write_json_line(server: &AcpServer, value: &Value) -> Result<()> {
    let mut stdout = server.output.lock().await;
    let mut encoded = serde_json::to_vec(value)?;
    encoded.push(b'\n');
    stdout.write_all(&encoded).await?;
    stdout.flush().await?;
    Ok(())
}

/// Read JSON line from input
///
/// This function replaces the `AcpServer::read_json_line` method.
#[allow(dead_code)] // F-GAP-10 — planned wiring: multi-channel transport I/O
pub async fn read_json_line() -> Result<Option<Value>> {
    use tokio::io::{AsyncBufReadExt, BufReader};

    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin);
    let mut line = String::new();

    match reader.read_line(&mut line).await {
        Ok(0) => Ok(None), // EOF
        Ok(_) => {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                let value: Value = serde_json::from_str(trimmed)?;
                Ok(Some(value))
            }
        }
        Err(e) => Err(e.into()),
    }
}

/// Flush output buffer
///
/// This function replaces the `AcpServer::flush_output` method.
#[allow(dead_code)] // F-GAP-10 — planned wiring: multi-channel transport I/O
pub async fn flush_output(server: &AcpServer) -> Result<()> {
    let mut stdout = server.output.lock().await;
    stdout.flush().await?;
    Ok(())
}

/// Check if input is available without consuming any bytes.
///
/// Uses `tokio::io::Interest::READABLE` with `std::os::fd::AsRawFd`
/// to check stdin readability without reading any data.
/// This is a best-effort heuristic used for detecting ACP protocol mode
/// in auto/adaptive mode.
/// If the timeout fires, we conservatively return false (no input).
#[cfg(unix)]
#[allow(dead_code)] // F-GAP-10 — planned wiring: multi-channel transport I/O
pub async fn has_input() -> Result<bool> {
    use std::os::unix::io::AsRawFd;

    let fd = std::io::stdin().as_raw_fd();
    let async_fd = tokio::io::unix::AsyncFd::new(fd)
        .map_err(|e| anyhow::anyhow!("failed to create AsyncFd for stdin: {}", e))?;

    let poll_ready =
        tokio::time::timeout(std::time::Duration::from_millis(50), async_fd.readable()).await;

    match poll_ready {
        Ok(Ok(_guard)) => Ok(true), // stdin readable, no bytes consumed
        Ok(Err(e)) => Err(anyhow::anyhow!("stdin readable poll failed: {}", e)),
        Err(_) => Ok(false), // timeout — no input available
    }
}

/// Windows stub for has_input — always returns true (assume input available).
///
/// Windows does not support async stdin polling via `AsRawFd` in the same way
/// Unix does. Rather than blocking indefinitely on a false negative (which
/// would hang the auto/adaptive mode detection), we optimistically return
/// `true` so that the caller proceeds to actually read stdin. The actual
/// read will determine whether input truly exists.
#[cfg(windows)]
#[allow(dead_code)] // F-GAP-10 — planned wiring: multi-channel transport I/O
pub async fn has_input() -> Result<bool> {
    // Return true so the caller attempts to read stdin instead of hanging.
    Ok(true)
}
