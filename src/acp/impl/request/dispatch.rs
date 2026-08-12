//! Dispatcher — unified output types for request handlers.
//!
//! All handlers return `Result<DispatchOutput>`. The dispatch layer
//! (`dispatch_to_client`) converts each variant to the appropriate
//! JSON-RPC or transport-level response, eliminating manual
//! `send_result`/`send_error` calls inside handler bodies.

use crate::acp::r#impl::chat::streaming::StreamFrame;
use crate::acp::server::AcpServer;
use crate::rpc_protocol::JsonRpcResponse;
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
        receiver: mpsc::UnboundedReceiver<StreamFrame>,
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
            // Delegate to the single error choke point (io::send_error): it
            // marks the request id for outcome accounting AND injects the
            // `acp.error` platform context, keeping dispatch-phase errors
            // consistent with every other error path.
            crate::acp::r#impl::io::send_error(server, Some(id), code, message, data).await
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
            while let Some(frame) = receiver.recv().await {
                match frame.event {
                    "chunk" => {
                        send_notification(server, "chat.stream.chunk", frame.payload).await?;
                    }
                    "done" => {
                        send_notification(server, "chat.stream.done", frame.payload).await?;
                    }
                    "telemetry" => {
                        send_notification(server, "chat.stream.telemetry", frame.payload).await?;
                    }
                    "result" => {
                        send_result(server, Some(id.clone()), frame.payload).await?;
                    }
                    "error" => {
                        let err_str = crate::i18n::runtime::t("acp.error.stream_error");
                        let msg = frame
                            .payload
                            .get("message")
                            .and_then(|v| v.as_str())
                            .unwrap_or(&err_str);
                        send_error(
                            server,
                            Some(id.clone()),
                            crate::acp::r#impl::request::protocol::AcpErrorCode::InternalError
                                as i32,
                            msg.to_string(),
                            None,
                        )
                        .await?;
                    }
                    "status" => {
                        send_notification(server, "chat.stream.status", frame.payload).await?;
                    }
                    "progress" => {
                        send_notification(server, "chat.stream.progress", frame.payload).await?;
                    }
                    "tool_approval" => {
                        send_notification(server, "chat.stream.tool_approval", frame.payload)
                            .await?;
                    }
                    "phase_start" | "phase_end" => {
                        send_notification(server, "chat.stream.phase", frame.payload).await?;
                    }
                    _ => {} // unknown events are ignored
                }
            }
            Ok(())
        }
        Ok(DispatchOutput::Silent) => Ok(()),
        Err(e) => {
            crate::acp::r#impl::io::send_error(
                server,
                Some(id),
                crate::acp::r#impl::request::protocol::AcpErrorCode::InvalidParams as i32,
                format!("{:#}", e),
                Some(serde_json::json!({"code": "DISPATCH_ERROR"})),
            )
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::transport::{with_transport, RpcBufferTransport};
    use crate::acp::ServerBuilder;
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    /// Helper: capture dispatch_to_client output into a Vec<u8> buffer.
    /// Runs inside a task-local transport scope (RpcBufferTransport) so the
    /// output lands in the per-test buffer.
    async fn capture_dispatch(output: Result<DispatchOutput>) -> Vec<u8> {
        let server = ServerBuilder::new().build();
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let buf_for_transport = buffer.clone();
        with_transport(
            Arc::new(RpcBufferTransport::new(buf_for_transport)),
            async {
                dispatch_to_client(&server, Some(serde_json::json!("test-id")), output)
                    .await
                    .expect("dispatch should succeed");
            },
        )
        .await;
        let locked = buffer.lock().await;
        locked.clone()
    }

    /// Helper: parse captured JSON-RPC response.
    fn parse_response(raw: &[u8]) -> serde_json::Value {
        let line = raw
            .split(|&b| b == b'\n')
            .find(|line| !line.is_empty())
            .expect("should have a non-empty line");
        serde_json::from_slice(line).expect("should be valid JSON")
    }

    #[tokio::test]
    async fn dispatch_json_returns_success() {
        let raw = capture_dispatch(Ok(DispatchOutput::ok(json!(
            {"hello": "world"}
        ))))
        .await;
        let resp = parse_response(&raw);
        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], "test-id");
        assert_eq!(resp["result"]["hello"], "world");
        assert!(resp.get("error").is_none());
    }

    #[tokio::test]
    async fn dispatch_json_empty_returns_empty_object() {
        let raw = capture_dispatch(Ok(DispatchOutput::empty())).await;
        let resp = parse_response(&raw);
        assert_eq!(resp["result"], serde_json::json!({}));
    }

    #[tokio::test]
    async fn dispatch_error_returns_error_with_code() {
        let raw = capture_dispatch(Ok(DispatchOutput::error(-32001, "custom error"))).await;
        let resp = parse_response(&raw);
        assert!(resp.get("result").is_none());
        assert_eq!(resp["error"]["code"], -32001);
        assert_eq!(resp["error"]["message"], "custom error");
    }

    #[tokio::test]
    async fn dispatch_silent_returns_no_response() {
        let raw = capture_dispatch(Ok(DispatchOutput::silent())).await;
        // Silent produces no output
        assert!(raw.is_empty() || raw.iter().all(|&b| b == b'\n'));
    }

    #[tokio::test]
    async fn dispatch_text_wraps_in_sentinel() {
        let raw = capture_dispatch(Ok(DispatchOutput::text("metric_value 42".to_string()))).await;
        let resp = parse_response(&raw);
        assert_eq!(resp["result"]["__text_plain__"], "metric_value 42");
    }

    #[tokio::test]
    async fn dispatch_err_returns_generic_error() {
        let raw = capture_dispatch(Err(anyhow::anyhow!("something broke"))).await;
        let resp = parse_response(&raw);
        assert!(resp.get("result").is_none());
        assert_eq!(resp["error"]["code"], -32602);
        assert!(resp["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("something broke"));
    }

    #[tokio::test]
    async fn dispatch_none_id_returns_ok_no_output() {
        let server = ServerBuilder::new().build();
        let result = dispatch_to_client(&server, None, Ok(DispatchOutput::ok(json!({})))).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn dispatch_checkpoint_created_success() {
        let raw = capture_dispatch(Ok(DispatchOutput::created(json!(
            {"id": "cp-1", "messages": []}
        ))))
        .await;
        let resp = parse_response(&raw);
        assert_eq!(resp["result"]["ok"], true);
        assert_eq!(resp["result"]["checkpoint"]["id"], "cp-1");
    }

    #[tokio::test]
    async fn dispatch_checkpoint_deleted_success() {
        let raw = capture_dispatch(Ok(DispatchOutput::deleted(json!(
            {"conversation_id": "conv-1", "count": 3}
        ))))
        .await;
        let resp = parse_response(&raw);
        assert_eq!(resp["result"]["ok"], true);
        assert_eq!(resp["result"]["deleted"]["conversation_id"], "conv-1");
    }

    #[tokio::test]
    async fn dispatch_stream_forwards_chunk_and_done() {
        let (tx, rx) = mpsc::unbounded_channel::<StreamFrame>();
        let server = ServerBuilder::new().build();
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let buf_clone = buffer.clone();

        // Spawn: send events then close channel
        tokio::spawn(async move {
            tx.send(StreamFrame {
                event: "chunk",
                payload: json!({"token": "Hello"}),
                status: None,
            })
            .ok();
            tx.send(StreamFrame {
                event: "done",
                payload: json!({"response": "Hello world"}),
                status: None,
            })
            .ok();
        });

        with_transport(Arc::new(RpcBufferTransport::new(buf_clone)), async {
            dispatch_to_client(
                &server,
                Some(json!("stream-test")),
                Ok(DispatchOutput::Stream { receiver: rx }),
            )
            .await
            .expect("stream dispatch should succeed");
        })
        .await;

        let locked = buffer.lock().await;
        let lines: Vec<&[u8]> = locked
            .split(|&b| b == b'\n')
            .filter(|l| !l.is_empty())
            .collect();

        // First line: chat.stream.chunk notification
        let chunk: serde_json::Value =
            serde_json::from_slice(lines[0]).expect("chunk should be valid JSON");
        assert_eq!(chunk["method"], "chat.stream.chunk");
        assert_eq!(chunk["params"]["token"], "Hello");

        // Second line: chat.stream.done notification
        let done: serde_json::Value =
            serde_json::from_slice(lines[1]).expect("done should be valid JSON");
        assert_eq!(done["method"], "chat.stream.done");
    }

    #[tokio::test]
    async fn dispatch_stream_error_event_sends_error() {
        let (tx, rx) = mpsc::unbounded_channel::<StreamFrame>();
        let server = ServerBuilder::new().build();
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let buf_clone = buffer.clone();

        tokio::spawn(async move {
            tx.send(StreamFrame {
                event: "error",
                payload: json!({"message": "stream failed"}),
                status: None,
            })
            .ok();
        });

        with_transport(Arc::new(RpcBufferTransport::new(buf_clone)), async {
            dispatch_to_client(
                &server,
                Some(json!("stream-error-test")),
                Ok(DispatchOutput::Stream { receiver: rx }),
            )
            .await
            .expect("error dispatch should not panic");
        })
        .await;

        let locked = buffer.lock().await;
        let line_count = locked
            .split(|&b| b == b'\n')
            .filter(|l| !l.is_empty())
            .count();
        assert!(line_count >= 1, "should have at least one output line");
    }

    #[tokio::test]
    async fn dispatch_stream_unknown_event_ignored() {
        let (tx, rx) = mpsc::unbounded_channel::<StreamFrame>();
        let server = ServerBuilder::new().build();
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let buf_clone = buffer.clone();

        tokio::spawn(async move {
            tx.send(StreamFrame {
                event: "unknown_event_type",
                payload: json!({}),
                status: None,
            })
            .ok();
        });

        with_transport(Arc::new(RpcBufferTransport::new(buf_clone)), async {
            dispatch_to_client(
                &server,
                Some(json!("unknown-test")),
                Ok(DispatchOutput::Stream { receiver: rx }),
            )
            .await
            .expect("unknown event dispatch should not panic");
        })
        .await;

        // Unknown events produce no output — channel just closes
        // The dispatch loop processes nothing and returns Ok(())
    }

    #[tokio::test]
    async fn dispatch_constructors_ok_vs_empty_vs_error() {
        // ok() produces Json variant with value
        match DispatchOutput::ok(json!("data")) {
            DispatchOutput::Json(v) => assert_eq!(v, "data"),
            _ => panic!("expected Json variant"),
        }
        // empty() produces Json variant with empty object
        match DispatchOutput::empty() {
            DispatchOutput::Json(v) => assert_eq!(v, serde_json::json!({})),
            _ => panic!("expected Json variant"),
        }
        // error() produces Error variant with correct code
        match DispatchOutput::error(-32001, "err") {
            DispatchOutput::Error {
                code,
                message,
                data,
            } => {
                assert_eq!(code, -32001);
                assert_eq!(message, "err");
                assert!(data.is_none());
            }
            _ => panic!("expected Error variant"),
        }
        // silent() produces Silent
        assert!(matches!(DispatchOutput::silent(), DispatchOutput::Silent));
        // text() produces Text variant
        match DispatchOutput::text("plain".to_string()) {
            DispatchOutput::Text(t) => assert_eq!(t, "plain"),
            _ => panic!("expected Text variant"),
        }
    }

    #[tokio::test]
    async fn dispatch_checkpoint_constructors() {
        let v = json!({"id": "x"});
        match DispatchOutput::created(v.clone()) {
            DispatchOutput::Checkpoint(CheckpointResult::Created(c)) => assert_eq!(c, v),
            _ => panic!("expected Checkpoint::Created"),
        }
        match DispatchOutput::deleted(v.clone()) {
            DispatchOutput::Checkpoint(CheckpointResult::Deleted(d)) => assert_eq!(d, v),
            _ => panic!("expected Checkpoint::Deleted"),
        }
    }
}
