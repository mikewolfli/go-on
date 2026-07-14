//! Dispatcher — unified output types for request handlers.
//!
//! All handlers return `Result<DispatchOutput>`. The dispatch layer
//! (`dispatch_to_client`) converts each variant to the appropriate
//! JSON-RPC or transport-level response, eliminating manual
//! `send_result`/`send_error` calls inside handler bodies.

use crate::acp::r#impl::chat::streaming::StreamFrame;
use crate::acp::server::AcpServer;
use crate::rpc_protocol::{JsonRpcError, JsonRpcResponse};
use anyhow::Result;
use serde_json::Value;
use tokio::sync::mpsc;

/// Unified handler output — replaces ad-hoc `send_result`/`send_error` calls.
#[derive(Debug)]
pub enum DispatchOutput {
    /// Standard JSON-RPC success response.
    Json(Value),
    /// JSON-RPC error with specific error code.
    Error {
        code: i32,
        message: String,
        data: Option<Value>,
    },
    /// Text/plain response (Prometheus metrics, etc.).
    Text(String),
    /// Multi-variant checkpoint result.
    Checkpoint(CheckpointResult),
    /// No response expected (JSON-RPC notification or shutdown).
    Silent,
    /// Streaming response (chat): the dispatch layer drains events and forwards as notifications.
    Stream {
        receiver: mpsc::Receiver<StreamFrame>,
    },
}

/// Multi-variant result for checkpoint operations.
#[derive(Debug)]
pub enum CheckpointResult {
    Created(Value),
    Deleted(Value),
}

// ── Helper constructors ────────────────────────────────────────────────────

impl DispatchOutput {
    pub fn ok(value: Value) -> Self {
        DispatchOutput::Json(value)
    }

    pub fn empty() -> Self {
        DispatchOutput::Json(Value::Object(serde_json::Map::new()))
    }

    pub fn error(code: i32, message: impl Into<String>) -> Self {
        DispatchOutput::Error {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn silent() -> Self {
        DispatchOutput::Silent
    }

    pub fn text(text: String) -> Self {
        DispatchOutput::Text(text)
    }

    pub fn created(value: Value) -> Self {
        DispatchOutput::Checkpoint(CheckpointResult::Created(value))
    }

    pub fn deleted(value: Value) -> Self {
        DispatchOutput::Checkpoint(CheckpointResult::Deleted(value))
    }
}

/// Dispatch a handler's `DispatchOutput` to the JSON-RPC client.
///
/// Replaces the simpler `respond()` for handlers that need non-standard
/// response shapes (text/plain, multi-variant, silent).
pub async fn dispatch_to_client(
    server: &AcpServer,
    request_id: Option<Value>,
    output: Result<DispatchOutput>,
) -> Result<()> {
    let id = match request_id {
        Some(id) => id,
        None => return Ok(()), // JSON-RPC notification — no response
    };

    match output {
        Ok(DispatchOutput::Json(value)) => {
            crate::acp::r#impl::io::write_response(
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
        Ok(DispatchOutput::Error {
            code,
            message,
            data,
        }) => {
            crate::acp::r#impl::io::write_response(
                server,
                JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: Some(id),
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
        Ok(DispatchOutput::Text(text)) => {
            crate::acp::r#impl::io::write_response(
                server,
                JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: Some(id),
                    result: Some(serde_json::json!({ "__text_plain__": text })),
                    error: None,
                },
            )
            .await
        }
        Ok(DispatchOutput::Checkpoint(ck)) => match ck {
            CheckpointResult::Created(v) => {
                crate::acp::r#impl::io::send_result(
                    server,
                    Some(id),
                    serde_json::json!({"ok": true, "checkpoint": v}),
                )
                .await
            }
            CheckpointResult::Deleted(v) => {
                crate::acp::r#impl::io::send_result(
                    server,
                    Some(id),
                    serde_json::json!({"ok": true, "deleted": v}),
                )
                .await
            }
        },
        Ok(DispatchOutput::Stream { mut receiver }) => {
            use crate::acp::r#impl::io::{send_error, send_notification, send_result};
            loop {
                match receiver.recv().await {
                    Some(frame) => match frame.event {
                        "chunk" => {
                            send_notification(server, "chat.stream.chunk", frame.payload).await?;
                        }
                        "done" => {
                            send_notification(server, "chat.stream.done", frame.payload).await?;
                        }
                        "telemetry" => {
                            send_notification(server, "chat.stream.telemetry", frame.payload)
                                .await?;
                        }
                        "result" => {
                            send_result(server, Some(id.clone()), frame.payload).await?;
                        }
                        "error" => {
                            let msg = frame
                                .payload
                                .get("message")
                                .and_then(|v| v.as_str())
                                .unwrap_or("stream error");
                            send_error(server, Some(id.clone()), -32603, msg.to_string(), None)
                                .await?;
                        }
                        _ => {} // unknown events are ignored
                    },
                    None => break, // channel closed — stream ended
                }
            }
            Ok(())
        }
        Ok(DispatchOutput::Silent) => Ok(()),
        Err(e) => {
            crate::acp::r#impl::io::send_error(
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
