//! Dispatcher — unified output types for request handlers.
//!
//! All handlers return `Result<DispatchOutput>`. The dispatch layer
//! (`dispatch_to_client`) converts each variant to the appropriate
//! JSON-RPC or transport-level response, eliminating manual
//! `send_result`/`send_error` calls inside handler bodies.

use crate::acp::server::AcpServer;
use crate::rpc_protocol::{JsonRpcError, JsonRpcResponse};
use anyhow::Result;
use serde_json::Value;

/// Unified handler output — replaces ad-hoc `send_result`/`send_error` calls.
#[derive(Debug)]
pub enum DispatchOutput {
    /// Standard JSON-RPC success response.
    Json(Value),
    /// Text/plain response (Prometheus metrics, etc.).
    Text(String),
    /// Multi-variant checkpoint result.
    Checkpoint(CheckpointResult),
    /// No response expected (JSON-RPC notification or shutdown).
    Silent,
}

/// Multi-variant result for checkpoint operations.
#[derive(Debug)]
pub enum CheckpointResult {
    Created(Value),
    AlreadyExists(Value),
    Conflict(Value),
    NotFound(String),
    Deleted(Value),
    Listed(Value),
}

// ── Helper constructors ────────────────────────────────────────────────────

impl DispatchOutput {
    /// Wrap a `Result<Value>` (the most common handler signature) into Json.
    pub fn json(result: Result<Value>) -> Self {
        match result {
            Ok(v) => DispatchOutput::Json(v),
            Err(e) => DispatchOutput::Json(Value::Object(Default::default())),
        }
    }

    pub fn ok(value: Value) -> Self {
        DispatchOutput::Json(value)
    }

    pub fn empty() -> Self {
        DispatchOutput::Json(Value::Object(serde_json::Map::new()))
    }

    pub fn silent() -> Self {
        DispatchOutput::Silent
    }

    pub fn text(text: String) -> Self {
        DispatchOutput::Text(text)
    }

    pub fn checkpoint(ck: CheckpointResult) -> Self {
        DispatchOutput::Checkpoint(ck)
    }

    pub fn created(value: Value) -> Self {
        DispatchOutput::Checkpoint(CheckpointResult::Created(value))
    }

    pub fn already_exists(value: Value) -> Self {
        DispatchOutput::Checkpoint(CheckpointResult::AlreadyExists(value))
    }

    pub fn conflict(value: Value) -> Self {
        DispatchOutput::Checkpoint(CheckpointResult::Conflict(value))
    }

    pub fn not_found(id: String) -> Self {
        DispatchOutput::Checkpoint(CheckpointResult::NotFound(id))
    }

    pub fn deleted(value: Value) -> Self {
        DispatchOutput::Checkpoint(CheckpointResult::Deleted(value))
    }

    pub fn listed(value: Value) -> Self {
        DispatchOutput::Checkpoint(CheckpointResult::Listed(value))
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
        Ok(DispatchOutput::Text(text)) => {
            // Text/plain: wrap as JSON-RPC result with a sentinel so the
            // HTTP transport layer can detect and serve as text/plain.
            crate::acp::r#impl::io::send_result(
                server,
                Some(id),
                serde_json::json!({ "__text_plain__": text }),
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
            CheckpointResult::AlreadyExists(v) => {
                crate::acp::r#impl::io::send_error(
                    server,
                    Some(id),
                    -32001,
                    "checkpoint already exists".into(),
                    Some(v),
                )
                .await
            }
            CheckpointResult::Conflict(v) => {
                crate::acp::r#impl::io::send_error(
                    server,
                    Some(id),
                    -32002,
                    "checkpoint conflict".into(),
                    Some(v),
                )
                .await
            }
            CheckpointResult::NotFound(name) => {
                crate::acp::r#impl::io::send_error(
                    server,
                    Some(id),
                    -32004,
                    format!("checkpoint '{}' not found", name),
                    None,
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
            CheckpointResult::Listed(v) => {
                crate::acp::r#impl::io::send_result(
                    server,
                    Some(id),
                    serde_json::json!({"ok": true, "checkpoints": v}),
                )
                .await
            }
        },
        Ok(DispatchOutput::Silent) => Ok(()),
        Err(e) => {
            crate::acp::r#impl::io::write_response(
                server,
                JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: Some(id),
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: format!("{:#}", e),
                        data: Some(serde_json::json!({"code": "DISPATCH_ERROR"})),
                    }),
                },
            )
            .await
        }
    }
}
