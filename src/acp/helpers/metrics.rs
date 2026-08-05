//! Metrics helper functions for ACP server
//!
//! This module provides utility functions for metrics collection,
//! streaming notifications, and Prometheus metric formatting.

use serde_json::{json, Map, Value};

/// Common `chunk` fields shared by the JSON-RPC notification
/// (`chat.stream.chunk`) and the SSE `chunk` frame, so the two transports
/// cannot drift. Transport-specific extras (id/cache_level vs
/// mode/risk_score/degrade_policy) are added by the callers.
pub fn chunk_core_fields(
    agent: &str,
    token: &str,
    chunk_index: usize,
    total_chars: usize,
    phase: Option<&str>,
    trace_id: Option<&str>,
    reasoning: Option<&str>,
) -> Map<String, Value> {
    let mut payload = Map::new();
    payload.insert("agent".to_string(), Value::String(agent.to_string()));
    payload.insert("token".to_string(), Value::String(token.to_string()));
    payload.insert("chunk_index".to_string(), json!(chunk_index));
    payload.insert("total_chars".to_string(), json!(total_chars));
    if let Some(phase_name) = phase {
        payload.insert("phase".to_string(), Value::String(phase_name.to_string()));
    }
    if let Some(trace) = trace_id {
        payload.insert("trace_id".to_string(), Value::String(trace.to_string()));
    }
    if let Some(reasoning_text) = reasoning {
        if !reasoning_text.is_empty() {
            payload.insert(
                "reasoning".to_string(),
                Value::String(reasoning_text.to_string()),
            );
        }
    }
    payload
}

/// Common `done` fields shared by the JSON-RPC notification
/// (`chat.stream.done`) and the SSE `done` frame, so the two transports
/// cannot drift. Transport-specific extras are added by the callers.
pub fn done_core_fields(
    agent: &str,
    chunks: usize,
    total_chars: usize,
    phase: Option<&str>,
    trace_id: Option<&str>,
    duration_ms: u64,
) -> Map<String, Value> {
    let mut payload = Map::new();
    payload.insert("agent".to_string(), Value::String(agent.to_string()));
    payload.insert("done".to_string(), Value::Bool(true));
    payload.insert("chunks".to_string(), json!(chunks));
    payload.insert("total_chars".to_string(), json!(total_chars));
    payload.insert("duration_ms".to_string(), json!(duration_ms));
    if let Some(phase_name) = phase {
        payload.insert("phase".to_string(), Value::String(phase_name.to_string()));
    }
    if let Some(trace) = trace_id {
        payload.insert("trace_id".to_string(), Value::String(trace.to_string()));
    }
    payload
}

/// Stream chunk notification
#[allow(clippy::too_many_arguments)]
pub fn stream_chunk_notification(
    id: Option<&Value>,
    agent: &str,
    token: &str,
    chunk_index: usize,
    total_chars: usize,
    cache_level: Option<&str>,
    phase: Option<&str>,
    trace_id: Option<&str>,
    reasoning: Option<&str>,
) -> Value {
    let mut payload = chunk_core_fields(
        agent,
        token,
        chunk_index,
        total_chars,
        phase,
        trace_id,
        reasoning,
    );
    payload.insert("id".to_string(), id.cloned().unwrap_or(Value::Null));
    if let Some(level) = cache_level {
        payload.insert("cached".to_string(), Value::Bool(true));
        payload.insert("cache_level".to_string(), Value::String(level.to_string()));
    }
    Value::Object(payload)
}

/// Stream done notification
#[allow(clippy::too_many_arguments)]
pub fn stream_done_notification(
    id: Option<&Value>,
    agent: &str,
    chunks: usize,
    total_chars: usize,
    cache_level: Option<&str>,
    phase: Option<&str>,
    trace_id: Option<&str>,
    duration_ms: u64,
) -> Value {
    let mut payload = done_core_fields(agent, chunks, total_chars, phase, trace_id, duration_ms);
    payload.insert("id".to_string(), id.cloned().unwrap_or(Value::Null));
    if let Some(level) = cache_level {
        payload.insert("cached".to_string(), Value::Bool(true));
        payload.insert("cache_level".to_string(), Value::String(level.to_string()));
    }
    Value::Object(payload)
}
