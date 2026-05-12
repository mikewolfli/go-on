//! F-GAP-99: Legacy JSON-RPC types — superseded by `mcp/schema.rs`
//!
//! These types (`JsonRpcRequest`, `JsonRpcResponse`, etc.) are kept for
//! backwards compatibility with existing callers (`runtime.rs`).
//! New code should use the types from `crate::mcp::schema`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

const MAX_TRACE_REQUEST_ID_LEN: usize = 128;

#[derive(Debug, Deserialize)]
pub(crate) struct JsonRpcRequest {
    pub(crate) jsonrpc: String,
    pub(crate) id: Option<Value>,
    pub(crate) method: String,
    pub(crate) params: Option<Value>,
}

#[derive(Debug, Clone)]
pub(crate) struct RequestTraceContext {
    pub(crate) trace_id: String,
    pub(crate) span_id: String,
    pub(crate) method: String,
    pub(crate) request_id: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct JsonRpcResponse {
    pub(crate) jsonrpc: &'static str,
    pub(crate) id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
pub(crate) struct JsonRpcError {
    pub(crate) code: i64,
    pub(crate) message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) data: Option<Value>,
}

// Optimized: return borrowed string for value_to_id when possible, clamp in place
pub(crate) fn value_to_id(value: &Value) -> String {
    match value {
        Value::String(v) if v.len() <= MAX_TRACE_REQUEST_ID_LEN => v.clone(),
        Value::String(v) => v[..MAX_TRACE_REQUEST_ID_LEN].to_string(),
        Value::Number(n) => {
            let s = n.to_string();
            if s.len() > MAX_TRACE_REQUEST_ID_LEN {
                s[..MAX_TRACE_REQUEST_ID_LEN].to_string()
            } else {
                s
            }
        }
        _ => {
            let s = value.to_string();
            if s.len() > MAX_TRACE_REQUEST_ID_LEN {
                s[..MAX_TRACE_REQUEST_ID_LEN].to_string()
            } else {
                s
            }
        }
    }
}

pub(crate) fn chat_trace_context(id: &Option<Value>, span_id: &str) -> RequestTraceContext {
    let request_id = id
        .as_ref()
        .map(value_to_id)
        .unwrap_or_else(|| "notification".to_string());
    RequestTraceContext {
        trace_id: format!("chat:{}", request_id),
        span_id: span_id.to_string(),
        method: "chat".to_string(),
        request_id,
    }
}

pub(crate) fn child_trace_context(
    parent: &RequestTraceContext,
    span_id: &str,
) -> RequestTraceContext {
    RequestTraceContext {
        trace_id: parent.trace_id.clone(),
        span_id: span_id.to_string(),
        method: parent.method.clone(),
        request_id: parent.request_id.clone(),
    }
}
