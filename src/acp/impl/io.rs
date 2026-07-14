//! I/O implementation functions for ACP server
//!
//! This module contains standalone functions that implement I/O-related
//! functionality previously in the `impl AcpServer` block in `impl/io.rs`.
//! These functions take `AcpServer` as their first parameter to maintain
//! compatibility with the original implementation.

use std::sync::Arc;

use anyhow::Result;
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::acp::server::AcpServer;
use crate::rpc_protocol::{JsonRpcError, JsonRpcResponse};

// Per-request output buffer for HTTP RPC calls.
// When set, write_json_line writes into this buffer instead of server.output.
// This eliminates the need for pipe-swapping and the rpc_serial lock.
tokio::task_local! {
    pub(crate) static RPC_BUFFER: Arc<Mutex<Vec<u8>>>;
}

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
            jsonrpc: "2.0".to_string(),
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
    code: i32,
    message: String,
    data: Option<Value>,
) -> Result<()> {
    // Inject platform context into error data for consistency with chat_pack::send_error.
    // Always inject even when data is None (creates a minimal context object).
    let data = Some(
        crate::acp::r#impl::request::inject_platform_profiles_if_absent(
            data.unwrap_or(serde_json::Value::Object(serde_json::Map::new())),
            "acp.error",
        ),
    );
    write_response(
        server,
        JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
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

/// Respond with a Result<Value>, writing the JSON-RPC response directly.
/// Skips send_result/send_error to reduce indirection for the pure-handler dispatch path.
pub async fn respond(
    server: &AcpServer,
    request_id: Option<Value>,
    result: Result<Value>,
) -> Result<()> {
    // JSON-RPC notification (no id) must not produce a response.
    let id = match request_id {
        Some(id) => id,
        None => return Ok(()),
    };
    match result {
        Ok(value) => {
            write_response(
                server,
                JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: Some(id),
                    result: Some(value),
                    error: None,
                },
            )
            .await
        }
        Err(e) => {
            send_error(
                server,
                Some(id),
                -32602,
                format!("{:#}", e),
                Some(serde_json::json!({"code": "DISPATCH_ERROR"})),
            )
            .await
        }
    }
}

/// Write JSON line to output.
///
/// In HTTP RPC mode, writes to the per-request buffer (set via RPC_BUFFER task-local).
/// In stdio mode, writes directly to tokio::io::stdout() — no lock, no heap-allocated writer.
pub async fn write_json_line(_server: &AcpServer, value: &Value) -> Result<()> {
    // Prefer per-request RPC buffer over global output
    if let Ok(buffer) = RPC_BUFFER.try_with(|b| b.clone()) {
        let mut buf = buffer.lock().await;
        let mut encoded = serde_json::to_vec(value)?;
        encoded.push(b'\n');
        buf.extend_from_slice(&encoded);
        return Ok(());
    }
    // Fallback: stdout (stdio mode) — direct write, no Box<dyn> indirection
    let mut encoded = serde_json::to_vec(value)?;
    encoded.push(b'\n');
    tokio::io::stdout().write_all(&encoded).await?;
    tokio::io::stdout().flush().await?;
    Ok(())
}
