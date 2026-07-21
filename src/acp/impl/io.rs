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
use crate::acp::transport::get_current_transport;
use crate::rpc_protocol::{JsonRpcError, JsonRpcResponse};

// RPC_BUFFER task-local has been removed in Phase 4.
// All output now routes through the Transport trait (CURRENT_TRANSPORT).
// HTTP RPC mode sets RpcBufferTransport, SSE mode sets SseTransport,
// stdio mode sets StdioTransport.

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
/// Routes through the Transport trait (CURRENT_TRANSPORT) if set:
/// - **Stdio mode**: StdioTransport writes to stdout.
/// - **HTTP RPC mode**: RpcBufferTransport captures into a response buffer.
/// - **SSE mode**: SseTransport writes SSE frames.
///
/// Fallback: If no transport is set (tests, initialization), writes to stdout.
pub async fn write_json_line(_server: &AcpServer, value: &Value) -> Result<()> {
    // Use global Transport trait if set
    if let Some(transport) = get_current_transport() {
        return transport.write_json_line(value).await;
    }
    // Fallback: stdout (direct stdio mode or no transport configured)
    let mut encoded = serde_json::to_vec(value)?;
    encoded.push(b'\n');
    tokio::io::stdout().write_all(&encoded).await?;
    tokio::io::stdout().flush().await?;
    Ok(())
}
