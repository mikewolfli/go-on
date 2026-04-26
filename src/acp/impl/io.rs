//! I/O implementation functions for ACP server
//!
//! This module contains standalone functions that implement I/O-related
//! functionality previously in the `impl AcpServer` block in `impl/io.rs`.
//! These functions take `AcpServer` as their first parameter to maintain
//! compatibility with the original implementation.

use anyhow::Result;
use serde_json::Value;
use std::io::Read;
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
    Ok(())
}

/// Read JSON line from input
///
/// This function replaces the `AcpServer::read_json_line` method.
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
pub async fn flush_output(server: &AcpServer) -> Result<()> {
    let mut stdout = server.output.lock().await;
    stdout.flush().await?;
    Ok(())
}

/// Check if input is available without consuming any bytes.
///
/// Uses `spawn_blocking` to perform a brief blocking read with a timeout.
/// This is a best-effort heuristic used for detecting ACP protocol mode.
/// If the timeout fires, we conservatively return false (no input).
pub async fn has_input() -> Result<bool> {
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(50),
        tokio::task::spawn_blocking(|| {
            let mut buf = [0u8; 1];
            match std::io::stdin().read(&mut buf) {
                Ok(0) => false,  // EOF
                Ok(_) => true,   // data available (consumed one byte)
                Err(_) => false, // error
            }
        }),
    )
    .await;

    match result {
        Ok(Ok(has_data)) => Ok(has_data),
        Ok(Err(e)) => Err(anyhow::anyhow!("stdin poll task failed: {}", e)),
        Err(_) => Ok(false), // timeout - no data
    }
}
