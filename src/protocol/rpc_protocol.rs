use serde::{Deserialize, Serialize};
use serde_json::Value;

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

pub(crate) fn value_to_id(value: &Value) -> String {
    match value {
        Value::String(v) => v.clone(),
        Value::Number(v) => v.to_string(),
        _ => value.to_string(),
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
