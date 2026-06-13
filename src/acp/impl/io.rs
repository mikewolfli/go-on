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
    send_result(
        server,
        id,
        serde_json::Value::Object(serde_json::Map::new()),
    )
    .await
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
