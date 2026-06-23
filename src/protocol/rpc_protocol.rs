//! Legacy JSON-RPC types and trace helpers.
//!
//! JSON-RPC request/response types are re-exported from `mcp::schema` to
//! avoid duplication.  Trace utilities (`RequestTraceContext`,
//! `chat_trace_context`, `child_trace_context`, `value_to_id`) live here
//! because they are used across ACP, MCP, and governance modules.

use serde_json::Value;

/// Re-export JSON-RPC types from the MCP module to eliminate duplication.
pub use crate::mcp::{JsonRpcError, JsonRpcRequest, JsonRpcResponse};

// ---------------------------------------------------------------------------
// Request trace context (shared across ACP / MCP / governance)
// ---------------------------------------------------------------------------

/// Shared trace context for request tracing across the ACP runtime.
///
/// Carries the `trace_id`, `span_id`, method name, and request ID so that
/// every subsystem (ACP handlers, MCP bridge, governance, orchestrator) can
/// correlate log / telemetry entries back to the originating request.
#[derive(Debug, Clone)]
pub struct RequestTraceContext {
    pub trace_id: String,
    pub span_id: String,
    pub method: String,
    pub request_id: String,
}

// ---------------------------------------------------------------------------
// Trace context helpers
// ---------------------------------------------------------------------------

/// Create a root trace context from an optional request id and method.
///
/// Uses a random `trace_id` (8 hex chars) and a `span_id` derived from the
/// method name.  The `request_id` is either the JSON-RPC `id` serialized to
/// a string, or a fresh timestamp-based ID.
pub fn chat_trace_context(id: &Option<Value>, method: &str) -> RequestTraceContext {
    let request_id = id
        .as_ref()
        .map(value_to_id)
        .unwrap_or_else(|| format!("ts-{}", crate::acp::prelude::now_ts_ms()));
    RequestTraceContext {
        trace_id: format!("{:08x}", fastrand::u32(..)),
        span_id: format!("root-{method}"),
        method: method.to_string(),
        request_id,
    }
}

/// Derive a child span context from a parent trace.
///
/// Keeps the same `trace_id` and generates a new `span_id` by appending
/// the child method name.
pub fn child_trace_context(parent: &RequestTraceContext, method: &str) -> RequestTraceContext {
    RequestTraceContext {
        trace_id: parent.trace_id.clone(),
        span_id: format!("{}.{method}", parent.span_id),
        method: method.to_string(),
        request_id: parent.request_id.clone(),
    }
}

/// Convert a JSON-RPC request `id` (Number or String) to a plain string.
///
/// This is the canonical way to extract a stable string key from the
/// optional `id` field of a `JsonRpcRequest`.
pub fn value_to_id(value: &Value) -> String {
    match value {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
