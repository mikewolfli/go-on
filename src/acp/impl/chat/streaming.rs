//! SSE streaming functions for ACP chat
//!
//! This module contains the SSE (Server-Sent Events) streaming functions
//! extracted from the parent `chat.rs` to reduce the monolithic file size.
//!
//! Functions here manage SSE buffer pooling, stream event emission (chunk,
//! done, telemetry), and the observer types that bridge JSON-RPC notification
//! and SSE transport backends.

use std::sync::OnceLock;

use anyhow::Result;
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::acp::helpers::metrics::{stream_chunk_notification, stream_done_notification};
use crate::acp::server::AcpServer;
use crate::agents::sse_optimizer::SseBufferPool;
use crate::orchestration::autonomy_runtime::TOKEN_THINKING_PREFIX;

// Stream event type constants to avoid repeated allocations
const STREAM_EVENT_CHUNK: &str = "chunk";
const STREAM_EVENT_DONE: &str = "done";
const STREAM_EVENT_TELEMETRY: &str = "telemetry";
#[allow(dead_code)]
const STREAM_EVENT_PHASE_START: &str = "phase_start";
#[allow(dead_code)]
const STREAM_EVENT_PHASE_END: &str = "phase_end";
#[allow(dead_code)]
const STREAM_EVENT_STATUS: &str = "status";

// ── SseBufferPool (GAP-46-12 / BLUE48 Step 2) ────────────────────────
// Global pool of pre-allocated byte buffers for SSE event serialization.
// Avoids allocation churn during high-frequency streaming by reusing
// buffers across requests.  Pre-initialized at server startup to avoid
// first-request latency penalty.
static SSE_BUFFER_POOL: OnceLock<SseBufferPool> = OnceLock::new();

/// Pre-initialize the SSE buffer pool at server startup.
/// Call once during server initialization to avoid first-request latency.
pub fn pre_init_sse_buffer_pool() {
    SSE_BUFFER_POOL.get_or_init(|| SseBufferPool::new(4, 4096));
    tracing::info!("SSE buffer pool pre-initialized (4 buffers x 4096 bytes)");
}

/// Acquire a buffer from the global SSE buffer pool.
/// Returns a pre-allocated (empty) `Vec<u8>` suitable for building an SSE frame.
pub(crate) fn acquire_sse_buffer() -> Vec<u8> {
    SSE_BUFFER_POOL
        .get_or_init(|| SseBufferPool::new(4, 4096))
        .acquire()
}

/// Release a buffer back to the global SSE buffer pool for reuse.
pub(crate) fn release_sse_buffer(buf: Vec<u8>) {
    if let Some(pool) = SSE_BUFFER_POOL.get() {
        pool.release(buf);
    }
}

pub(crate) async fn emit_stream_chunk(
    server: &AcpServer,
    observer: Option<&StreamObserver>,
    meta: StreamEventMeta<'_>,
    token: &str,
    chunk_index: usize,
    total_chars: usize,
) -> Result<()> {
    let Some(observer) = observer else {
        return Ok(());
    };

    // Check if this is a reasoning token (prefixed with __thinking__)
    let (display_token, reasoning_token) =
        if let Some(rest) = token.strip_prefix(TOKEN_THINKING_PREFIX) {
            ("", rest)
        } else {
            (token, "")
        };

    // Use as_ref() to avoid cloning response_id
    if let Some(response_id) = observer.jsonrpc_response_id.as_ref() {
        crate::acp::r#impl::io::send_notification(
            server,
            "chat.stream.chunk",
            stream_chunk_notification(
                Some(response_id),
                meta.agent_name,
                display_token,
                chunk_index,
                total_chars,
                None,
                Some(meta.phase_name),
                Some(meta.trace_id),
                if reasoning_token.is_empty() {
                    None
                } else {
                    Some(reasoning_token)
                },
            ),
        )
        .await?;
    }

    if let Some(sender) = &observer.sse_sender {
        let mut payload = json!({
            "agent": meta.agent_name,
            "chunk_index": chunk_index,
            "phase": meta.phase_name,
            "token": display_token,
            "total_chars": total_chars,
            "trace_id": meta.trace_id,
        });
        if !reasoning_token.is_empty() {
            payload["reasoning"] = json!(reasoning_token);
        }
        let status = if reasoning_token.is_empty() && display_token.is_empty() {
            Some("thinking")
        } else {
            None
        };
        // Send failure is expected when client disconnects — non-critical.
        let _ = sender.send(StreamFrame {
            event: STREAM_EVENT_CHUNK,
            payload,
            status,
        });
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn emit_stream_done(
    server: &AcpServer,
    observer: Option<&StreamObserver>,
    meta: StreamEventMeta<'_>,
    chunk_index: usize,
    total_chars: usize,
    duration_ms: u64,
    // Actual model name reported by the agent (e.g. "gemini-2.5-pro" for copilot).
    // Passed through to SSE payload so the GUI can display it.
    selected_model: Option<String>,
    // Optional response text to include in the SSE done event so the GUI
    // doesn't depend solely on a separate "result" event.
    response: Option<&str>,
) -> Result<()> {
    let Some(observer) = observer else {
        return Ok(());
    };

    // Use as_ref() to avoid cloning response_id
    if let Some(response_id) = observer.jsonrpc_response_id.as_ref() {
        crate::acp::r#impl::io::send_notification(
            server,
            "chat.stream.done",
            stream_done_notification(
                Some(response_id),
                meta.agent_name,
                chunk_index,
                total_chars,
                None,
                Some(meta.phase_name),
                Some(meta.trace_id),
                duration_ms,
            ),
        )
        .await?;
    }

    if let Some(sender) = &observer.sse_sender {
        // NOTE: This SSE frame structure should match helpers/metrics::stream_done_notification
        let mut payload = json!({
            "agent": meta.agent_name,
            "chunks": chunk_index,
            "done": true,
            "duration_ms": duration_ms,
            "phase": meta.phase_name,
            "total_chars": total_chars,
            "trace_id": meta.trace_id,
        });
        if let Some(ref m) = selected_model {
            if let Some(obj) = payload.as_object_mut() {
                obj.insert("selected_model".to_string(), json!(m));
            }
        }
        // Always include response in the done event when available, so the GUI
        // frontend can set final_content from either "done" or "result".
        // This prevents the "chat stream ended without producing a response" error
        // when the "result" event arrives after the HTTP stream has ended.
        if let Some(response_text) = response {
            if let Some(obj) = payload.as_object_mut() {
                obj.insert("response".to_string(), json!(response_text));
            }
        }
        // Send failure is expected when client disconnects — non-critical.
        let _ = sender.send(StreamFrame {
            event: STREAM_EVENT_DONE,
            payload,
            status: None,
        });
    }

    Ok(())
}

pub(crate) async fn emit_stream_token_economy(
    server: &AcpServer,
    observer: Option<&StreamObserver>,
    meta: StreamEventMeta<'_>,
    token_economy: &Value,
) -> Result<()> {
    let Some(observer) = observer else {
        return Ok(());
    };

    // Use as_ref() to avoid cloning response_id
    if let Some(response_id) = observer.jsonrpc_response_id.as_ref() {
        crate::acp::r#impl::io::send_notification(
            server,
            "chat.stream.telemetry",
            json!({
                "id": response_id,
                "agent": meta.agent_name,
                "phase": meta.phase_name,
                "trace_id": meta.trace_id,
                "token_economy": token_economy,
            }),
        )
        .await?;
    }

    if let Some(sender) = &observer.sse_sender {
        // Send failure is expected when client disconnects — non-critical.
        let _ = sender.send(StreamFrame {
            event: STREAM_EVENT_TELEMETRY,
            payload: json!({
                "agent": meta.agent_name,
                "phase": meta.phase_name,
                "trace_id": meta.trace_id,
                "token_economy": token_economy,
            }),
            status: None,
        });
    }

    Ok(())
}

/// Emit a phase lifecycle event (phase_start / phase_end) with description and progress.
///
/// This enables the client (Zed / GUI / CLI) to show structured progress
/// indicators like "Scanning project structure... (1/4)" instead of blank waiting.
pub(crate) async fn emit_phase_event(
    _server: &AcpServer,
    observer: Option<&StreamObserver>,
    event_type: &'static str,
    phase_name: &str,
    description: &str,
    progress: Option<(u32, u32)>,
) -> Result<()> {
    let Some(observer) = observer else {
        return Ok(());
    };

    let mut payload = json!({
        "phase": phase_name,
        "description": description,
    });
    if let Some((current, total)) = progress {
        payload["progress_current"] = json!(current);
        payload["progress_total"] = json!(total);
    }

    if let Some(sender) = &observer.sse_sender {
        let _ = sender.send(StreamFrame {
            event: event_type,
            payload,
            status: Some("idle"),
        });
    }

    Ok(())
}

/// Emit a lightweight status indicator without a full phase transition.
/// Used for "Thinking...", "Analyzing...", "Scanning..." quick status updates.
pub(crate) async fn emit_status_event(
    observer: Option<&StreamObserver>,
    message: &str,
    status: &'static str,
) -> Result<()> {
    let Some(observer) = observer else {
        return Ok(());
    };
    if let Some(sender) = &observer.sse_sender {
        let _ = sender.send(StreamFrame {
            event: STREAM_EVENT_STATUS,
            payload: json!({"message": message}),
            status: Some(status),
        });
    }
    Ok(())
}

/// Context for streaming notifications during agent execution.
///
/// Bundles the stream observer, agent name, phase name, and trace ID
/// so they can be carried together through the autonomy pipeline.
#[derive(Debug, Clone)]
pub(crate) struct StreamNotificationContext<'a> {
    pub(crate) stream_observer: Option<StreamObserver>,
    pub(crate) agent_name: &'a str,
    pub(crate) phase_name: &'a str,
    pub(crate) trace_id: &'a str,
}

/// Metadata about a streaming event (agent, phase, trace).
#[derive(Debug, Clone, Copy)]
pub(crate) struct StreamEventMeta<'a> {
    pub(crate) agent_name: &'a str,
    pub(crate) phase_name: &'a str,
    pub(crate) trace_id: &'a str,
}

/// A single SSE frame to be sent to the client.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct StreamFrame {
    pub event: &'static str,
    pub payload: Value,
    /// Optional status hint: "thinking" | "analyzing" | "scanning" | "generating" | "idle"
    /// When set, the client may display a status indicator instead of the raw token.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<&'static str>,
}

/// Observer pattern for streaming responses.
///
/// Can deliver streaming events via either JSON-RPC notifications or
/// SSE (Server-Sent Events) depending on how the observer was constructed.
#[derive(Debug, Clone)]
pub(crate) struct StreamObserver {
    jsonrpc_response_id: Option<Value>,
    sse_sender: Option<mpsc::UnboundedSender<StreamFrame>>,
}

impl StreamObserver {
    /// Send an SSE frame through this observer, if SSE is configured.
    /// Returns `true` if the frame was sent, `false` if no SSE sender is available.
    pub(crate) fn send_sse(&self, frame: StreamFrame) -> bool {
        if let Some(sender) = &self.sse_sender {
            sender.send(frame).is_ok()
        } else {
            false
        }
    }

    pub(crate) fn jsonrpc(response_id: Option<Value>) -> Self {
        Self {
            jsonrpc_response_id: response_id,
            sse_sender: None,
        }
    }

    pub(crate) fn sse(sender: mpsc::UnboundedSender<StreamFrame>) -> Self {
        Self {
            jsonrpc_response_id: None,
            sse_sender: Some(sender),
        }
    }

    /// Clone the SSE sender, if present, so callers can send progress
    /// frames through the same channel without owning the observer.
    pub(crate) fn sse_sender(&self) -> Option<mpsc::UnboundedSender<StreamFrame>> {
        self.sse_sender.clone()
    }
}
