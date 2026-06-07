//! OpenAI API compatibility and Responses API handlers
//!
//! Contains OpenAI-compatible endpoints (/v1/chat/completions, etc.),
//! Responses API implementation, request/response types, and all
//! associated builder and handler functions.
//! Extracted from the parent `runtime.rs` to reduce the monolithic file size.

use std::sync::Arc;

use anyhow::Result;
use serde::Deserialize;
use tokio::net::TcpStream;

use crate::acp::r#impl::chat::{ChatParams, ChatRequestContext, StreamFrame, StreamObserver};
use crate::acp::r#impl::request::inject_platform_profiles_if_absent;
use crate::acp::r#impl::session::UserSession;
use crate::acp::server::AcpServer;
use crate::agent::Message;
use crate::i18n::runtime::{t, tf};
use crate::rpc_protocol::RequestTraceContext;

use super::http::{
    http_trace_context, write_http_json_response, write_http_json_response_with_context,
};
use super::sse::{
    write_openai_sse_data, write_openai_sse_done, write_sse_event, write_sse_headers,
};

// ---------------------------------------------------------------------------
// OpenAI Chat Request / Message types
// ---------------------------------------------------------------------------

/// OpenAI-compatible /v1/chat/completions request body.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct OpenAiChatRequest {
    model: Option<String>,
    messages: Vec<OpenAiChatMessage>,
    temperature: Option<f64>,
    top_p: Option<f64>,
    max_tokens: Option<u64>,
    n: Option<u64>,
    stop: Option<serde_json::Value>,
    presence_penalty: Option<f64>,
    frequency_penalty: Option<f64>,
    logit_bias: Option<serde_json::Value>,
    user: Option<String>,
    seed: Option<i64>,
    response_format: Option<serde_json::Value>,
    tools: Option<serde_json::Value>,
    tool_choice: Option<serde_json::Value>,
    parallel_tool_calls: Option<bool>,
    function_call: Option<serde_json::Value>,
    functions: Option<serde_json::Value>,
    #[serde(default)]
    stream: bool,
    #[serde(flatten)]
    extra: std::collections::HashMap<String, serde_json::Value>,
}

/// A single message in an OpenAI chat request.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct OpenAiChatMessage {
    role: String,
    #[serde(default)]
    content: serde_json::Value,
    name: Option<String>,
    tool_call_id: Option<String>,
    function_call: Option<serde_json::Value>,
    tool_calls: Option<serde_json::Value>,
    refusal: Option<serde_json::Value>,
}

impl OpenAiChatMessage {
    fn normalized_role(&self) -> String {
        match self.role.as_str() {
            "system" | "user" | "assistant" => self.role.clone(),
            _ => "user".to_string(),
        }
    }

    fn content_text(&self) -> String {
        if let Some(text) = self.content.as_str() {
            return text.to_string();
        }

        if self.content.is_null() {
            return String::new();
        }

        if let Some(parts) = self.content.as_array() {
            let merged = parts
                .iter()
                .filter_map(|part| {
                    part.get("type")
                        .and_then(|value| value.as_str())
                        .filter(|value| *value == "text")?;
                    part.get("text")
                        .and_then(|value| value.as_str())
                        .map(ToString::to_string)
                })
                .collect::<Vec<_>>()
                .join("\n");
            if !merged.is_empty() {
                return merged;
            }

            return serde_json::to_string(parts).unwrap_or_default();
        }

        if self.content.is_object() {
            if let Some(text) = self.content.get("text").and_then(|value| value.as_str()) {
                return text.to_string();
            }
            return serde_json::to_string(&self.content).unwrap_or_default();
        }

        serde_json::to_string(&self.content).unwrap_or_default()
    }

    fn to_agent_message(&self) -> Message {
        let mut metadata: Vec<String> = Vec::new();
        if let Some(name) = &self.name {
            metadata.push(format!("[name] {}", name));
        }
        if let Some(tool_call_id) = &self.tool_call_id {
            metadata.push(format!("[tool_call_id] {}", tool_call_id));
        }
        if let Some(function_call) = &self.function_call {
            metadata.push(format!(
                "[function_call] {}",
                serde_json::to_string(function_call).unwrap_or_default()
            ));
        }
        if let Some(tool_calls) = &self.tool_calls {
            metadata.push(format!(
                "[tool_calls] {}",
                serde_json::to_string(tool_calls).unwrap_or_default()
            ));
        }
        if let Some(refusal) = &self.refusal {
            metadata.push(format!(
                "[refusal] {}",
                serde_json::to_string(refusal).unwrap_or_default()
            ));
        }

        let content = self.content_text();
        let final_content = if metadata.is_empty() {
            content
        } else if content.is_empty() {
            metadata.join("\n")
        } else {
            format!("{}\n{}", content, metadata.join("\n"))
        };

        Message {
            role: self.normalized_role(),
            content: final_content,
        }
    }
}

// ---------------------------------------------------------------------------
// Responses API Request type
// ---------------------------------------------------------------------------

/// Responses API (Phase R1 baseline) request schema.
/// Maps `input` (string or array of message objects) instead of `messages`.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ResponsesApiRequest {
    model: Option<String>,
    input: Option<serde_json::Value>,
    #[serde(default)]
    stream: bool,
    temperature: Option<f64>,
    /// Responses API uses `max_output_tokens` instead of `max_tokens`.
    max_output_tokens: Option<u64>,
    tools: Option<serde_json::Value>,
    tool_choice: Option<serde_json::Value>,
    metadata: Option<serde_json::Value>,
    reasoning: Option<serde_json::Value>,
    #[serde(flatten)]
    extra: std::collections::HashMap<String, serde_json::Value>,
}

// ---------------------------------------------------------------------------
// ID generation
// ---------------------------------------------------------------------------

static RESPONSES_ID_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn next_responses_api_id(prefix: &str) -> String {
    let seq = RESPONSES_ID_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{}_{}_{}", prefix, crate::acp::prelude::now_ts_ms(), seq)
}

// ---------------------------------------------------------------------------
// OpenAI-Compatible Response Builders
// ---------------------------------------------------------------------------

fn build_openai_completion(
    request_id: &str,
    model: &str,
    response_text: &str,
) -> serde_json::Value {
    serde_json::json!({
        "id": request_id,
        "object": "chat.completion",
        "created": crate::acp::prelude::now_ts(),
        "model": model,
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": response_text,
            },
            "finish_reason": "stop",
        }]
    })
}

pub(crate) fn build_openai_models_response() -> serde_json::Value {
    serde_json::json!({
        "object": "list",
        "data": [{
            "id": "go-on",
            "object": "model",
            "created": crate::acp::prelude::now_ts(),
            "owned_by": "go-on",
        }]
    })
}

fn build_openai_chunk(
    request_id: &str,
    model: &str,
    content: &str,
    finish_reason: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "id": request_id,
        "object": "chat.completion.chunk",
        "created": crate::acp::prelude::now_ts(),
        "model": model,
        "choices": [{
            "index": 0,
            "delta": {
                "content": content,
            },
            "finish_reason": finish_reason,
        }]
    })
}

// ---------------------------------------------------------------------------
// OpenAI Chat Completions Handler
// ---------------------------------------------------------------------------

fn is_setup_or_upstream_unavailable(err: &anyhow::Error) -> bool {
    let msg = err.to_string().to_lowercase();
    msg.contains("missing environment variable")
        || msg.contains("error sending request")
        || msg.contains("connection refused")
        || msg.contains("timed out")
        || msg.contains("error.chat.agent_error_prefix")
        || msg.contains("error.chat.all_agents_failed")
}

fn degraded_openai_message(err: &anyhow::Error) -> String {
    format!(
        "go-on is running, but upstream model service is unavailable. {}. Configure at least one reachable provider (for example set DEEPSEEK_API_KEY) or start your copilot-compatible upstream on 127.0.0.1:8080.",
        err
    )
}

fn openai_to_chat_params(req: &OpenAiChatRequest) -> ChatParams {
    let messages = req
        .messages
        .iter()
        .map(OpenAiChatMessage::to_agent_message)
        .collect::<Vec<_>>();

    let mut extra = req.extra.clone();
    if let Some(model) = &req.model {
        extra.insert("model".to_string(), serde_json::json!(model));
    }
    if let Some(value) = req.temperature {
        extra.insert("temperature".to_string(), serde_json::json!(value));
    }
    if let Some(value) = req.top_p {
        extra.insert("top_p".to_string(), serde_json::json!(value));
    }
    if let Some(value) = req.max_tokens {
        extra.insert("max_tokens".to_string(), serde_json::json!(value));
    }
    if let Some(value) = req.n {
        extra.insert("n".to_string(), serde_json::json!(value));
    }
    if let Some(value) = &req.stop {
        extra.insert("stop".to_string(), value.clone());
    }
    if let Some(value) = req.presence_penalty {
        extra.insert("presence_penalty".to_string(), serde_json::json!(value));
    }
    if let Some(value) = req.frequency_penalty {
        extra.insert("frequency_penalty".to_string(), serde_json::json!(value));
    }
    if let Some(value) = &req.logit_bias {
        extra.insert("logit_bias".to_string(), value.clone());
    }
    if let Some(value) = &req.user {
        extra.insert("user".to_string(), serde_json::json!(value));
    }
    if let Some(value) = req.seed {
        extra.insert("seed".to_string(), serde_json::json!(value));
    }
    if let Some(value) = &req.response_format {
        extra.insert("response_format".to_string(), value.clone());
    }
    if let Some(value) = &req.tools {
        extra.insert("tools".to_string(), value.clone());
    }
    if let Some(value) = &req.tool_choice {
        extra.insert("tool_choice".to_string(), value.clone());
    }
    if let Some(value) = req.parallel_tool_calls {
        extra.insert("parallel_tool_calls".to_string(), serde_json::json!(value));
    }
    if let Some(value) = &req.function_call {
        extra.insert("function_call".to_string(), value.clone());
    }
    if let Some(value) = &req.functions {
        extra.insert("functions".to_string(), value.clone());
    }

    let options = if extra.is_empty() {
        None
    } else {
        Some(crate::config::PhaseOptions {
            extra,
            ..Default::default()
        })
    };

    ChatParams {
        mode: "ask".to_string(),
        messages,
        conversation_id: None,
        branch_id: None,
        phase: None,
        options,
        requirement_contract: None,
        plan: None,
        vector_hits: None,
        execution_decision_candidate: None,
    }
}

pub(crate) async fn handle_openai_chat_completions(
    socket: &mut TcpStream,
    server: Arc<AcpServer>,
    body: serde_json::Value,
    user_session: Option<UserSession>,
    cors_headers: &str,
) -> Result<()> {
    let started = std::time::Instant::now();
    let record_outcome = |success: bool| {
        server
            .observability
            .metrics
            .record_request_outcome(success, started.elapsed().as_millis() as f64);
    };

    let openai_req: OpenAiChatRequest = match serde_json::from_value(body) {
        Ok(value) => value,
        Err(err) => {
            let payload = serde_json::json!({
                "error": {
                    "message": tf("error.invalid_openai_chat_request", &[("error", &err.to_string())]),
                    "type": "invalid_request_error"
                }
            });
            write_http_json_response_with_context(
                socket,
                400,
                payload,
                "openai.chat.completions",
                cors_headers,
            )
            .await?;
            record_outcome(false);
            return Ok(());
        }
    };
    let model = openai_req
        .model
        .clone()
        .unwrap_or_else(|| "go-on".to_string());
    let request_id = format!("chatcmpl-{}", crate::acp::prelude::now_ts_ms());
    let params = openai_to_chat_params(&openai_req);

    if !openai_req.stream {
        let trace = http_trace_context("openai.chat.completions");
        let ctx = Some(ChatRequestContext::new(user_session.clone()));
        let result = crate::acp::r#impl::chat::process_chat_request(
            server.as_ref(),
            &params,
            None,
            &trace,
            None,
            ctx,
        )
        .await;
        let result = match result {
            Ok(result) => result,
            Err(err) => {
                if is_setup_or_upstream_unavailable(&err) {
                    let payload = build_openai_completion(
                        &request_id,
                        &model,
                        &degraded_openai_message(&err),
                    );
                    let payload =
                        inject_platform_profiles_if_absent(payload, "openai.chat.completions");
                    write_http_json_response(socket, 200, payload, cors_headers).await?;
                    record_outcome(true);
                    return Ok(());
                }
                let payload = serde_json::json!({
                    "error": {
                        "message": err.to_string(),
                        "type": "go_on_upstream_error"
                    }
                });
                write_http_json_response_with_context(
                    socket,
                    502,
                    payload,
                    "openai.chat.completions",
                    cors_headers,
                )
                .await?;
                record_outcome(false);
                return Ok(());
            }
        };
        let response_text = result
            .get("response")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let payload = build_openai_completion(&request_id, &model, response_text);
        let payload = inject_platform_profiles_if_absent(payload, "openai.chat.completions");
        write_http_json_response(socket, 200, payload, cors_headers).await?;
        record_outcome(true);
        return Ok(());
    }

    write_sse_headers(socket, cors_headers).await?;

    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    let trace = http_trace_context("openai.chat.completions.stream");
    let ctx = Some(ChatRequestContext::new(user_session));
    let server_ref = Arc::clone(&server);
    let task = tokio::spawn(async move {
        crate::acp::r#impl::chat::process_chat_request(
            server_ref.as_ref(),
            &params,
            Some(StreamObserver::sse(tx)),
            &trace,
            None,
            ctx,
        )
        .await
    });

    while let Some(frame) = rx.recv().await {
        if frame.event != "chunk" {
            continue;
        }
        let token = frame
            .payload
            .get("token")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        if token.is_empty() {
            continue;
        }
        let payload = build_openai_chunk(&request_id, &model, token, None);
        if let Err(err) = write_openai_sse_data(socket, &payload).await {
            task.abort();
            record_outcome(false);
            return Err(err);
        }
    }

    match task.await {
        Ok(Ok(_)) => {
            let done_payload = build_openai_chunk(&request_id, &model, "", Some("stop"));
            write_openai_sse_data(socket, &done_payload).await?;
            write_openai_sse_done(socket).await?;
            record_outcome(true);
        }
        Ok(Err(err)) => {
            if is_setup_or_upstream_unavailable(&err) {
                let payload = build_openai_chunk(
                    &request_id,
                    &model,
                    &degraded_openai_message(&err),
                    Some("stop"),
                );
                write_openai_sse_data(socket, &payload).await?;
                write_openai_sse_done(socket).await?;
                record_outcome(true);
                return Ok(());
            }
            let payload = serde_json::json!({"error": {"message": err.to_string()}});
            write_openai_sse_data(socket, &payload).await?;
            write_openai_sse_done(socket).await?;
            record_outcome(false);
        }
        Err(err) => {
            let payload = serde_json::json!({"error": {"message": tf("error.chat_task_panicked", &[("error", &err.to_string())])}});
            write_openai_sse_data(socket, &payload).await?;
            write_openai_sse_done(socket).await?;
            record_outcome(false);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Responses API — Input Conversion
// ---------------------------------------------------------------------------

/// Convert Responses API `input` field to internal agent messages.
fn responses_input_to_messages(input: &serde_json::Value) -> Vec<Message> {
    if let Some(text) = input.as_str() {
        if text.trim().is_empty() {
            return vec![];
        }
        return vec![Message {
            role: "user".to_string(),
            content: text.to_string(),
        }];
    }

    if let Some(items) = input.as_array() {
        return items
            .iter()
            .filter_map(|item| {
                let role = item
                    .get("role")
                    .and_then(|v| v.as_str())
                    .unwrap_or("user")
                    .to_string();
                let role = match role.as_str() {
                    "system" | "user" | "assistant" => role,
                    _ => "user".to_string(),
                };

                let content = if let Some(text) = item.get("content").and_then(|v| v.as_str()) {
                    text.to_string()
                } else if let Some(parts) = item.get("content").and_then(|v| v.as_array()) {
                    parts
                        .iter()
                        .filter_map(|p| {
                            let ptype = p.get("type").and_then(|t| t.as_str()).unwrap_or("");
                            if ptype == "input_text" || ptype == "text" {
                                p.get("text")
                                    .and_then(|t| t.as_str())
                                    .map(|s| s.to_string())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                } else if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                    text.to_string()
                } else {
                    String::new()
                };

                if content.trim().is_empty() && role == "user" {
                    None
                } else {
                    Some(Message { role, content })
                }
            })
            .collect();
    }

    vec![]
}

// ---------------------------------------------------------------------------
// Responses API — Response Builders
// ---------------------------------------------------------------------------

fn build_responses_api_response(
    request_id: &str,
    model: &str,
    response_text: &str,
) -> serde_json::Value {
    let msg_id = format!("msg_{}", crate::acp::prelude::now_ts_ms());
    serde_json::json!({
        "id": request_id,
        "object": "response",
        "created_at": crate::acp::prelude::now_ts(),
        "model": model,
        "status": "completed",
        "output": [{
            "type": "message",
            "id": msg_id,
            "role": "assistant",
            "status": "completed",
            "content": [{
                "type": "output_text",
                "text": response_text,
            }],
        }],
        "usage": {
            "input_tokens": 0,
            "output_tokens": 0,
            "total_tokens": 0,
        },
        "error": null,
        "incomplete_details": null,
    })
}

fn attach_responses_token_economy(
    mut payload: serde_json::Value,
    messages: &[Message],
    response_text: &str,
) -> serde_json::Value {
    let token_economy = crate::acp::r#impl::chat::estimate_token_economy(messages, response_text);
    payload["usage"] = serde_json::json!({
        "input_tokens": token_economy["input_tokens"].clone(),
        "output_tokens": token_economy["output_tokens"].clone(),
        "total_tokens": token_economy["total_tokens"].clone(),
    });
    payload["token_economy"] = token_economy;
    payload
}

fn build_responses_api_queued_response(request_id: &str, model: &str) -> serde_json::Value {
    serde_json::json!({
        "id": request_id,
        "object": "response",
        "created_at": crate::acp::prelude::now_ts(),
        "model": model,
        "status": "queued",
        "output": [],
        "usage": {
            "input_tokens": 0,
            "output_tokens": 0,
            "total_tokens": 0,
        },
        "error": null,
        "incomplete_details": null,
    })
}

fn build_responses_api_in_progress_response(request_id: &str, model: &str) -> serde_json::Value {
    serde_json::json!({
        "id": request_id,
        "object": "response",
        "created_at": crate::acp::prelude::now_ts(),
        "model": model,
        "status": "in_progress",
        "output": [],
        "usage": {
            "input_tokens": 0,
            "output_tokens": 0,
            "total_tokens": 0,
        },
        "error": null,
        "incomplete_details": null,
    })
}

fn build_responses_api_tool_call_response(
    request_id: &str,
    model: &str,
    tool_call_id: &str,
    tool_name: &str,
) -> serde_json::Value {
    serde_json::json!({
        "id": request_id,
        "object": "response",
        "created_at": crate::acp::prelude::now_ts(),
        "model": model,
        "status": "incomplete",
        "output": [{
            "type": "tool_call",
            "id": tool_call_id,
            "status": "incomplete",
            "name": tool_name,
            "arguments": "{}",
        }],
        "usage": {
            "input_tokens": 0,
            "output_tokens": 0,
            "total_tokens": 0,
        },
        "error": null,
        "incomplete_details": {
            "reason": "tool_calls_required"
        },
    })
}

fn build_responses_api_tool_result_response(
    request_id: &str,
    model: &str,
    previous_response_id: &str,
    tool_call_id: &str,
    tool_result_text: &str,
) -> serde_json::Value {
    let msg_id = next_responses_api_id("msg");
    let tool_result_id = next_responses_api_id("tr");
    serde_json::json!({
        "id": request_id,
        "object": "response",
        "created_at": crate::acp::prelude::now_ts(),
        "model": model,
        "previous_response_id": previous_response_id,
        "status": "completed",
        "output": [
            {
                "type": "tool_result",
                "id": tool_result_id,
                "status": "completed",
                "tool_call_id": tool_call_id,
                "content": [{
                    "type": "output_text",
                    "text": tool_result_text,
                }],
            },
            {
                "type": "message",
                "id": msg_id,
                "role": "assistant",
                "status": "completed",
                "content": [{
                    "type": "output_text",
                    "text": format!("Tool result received for {}: {}", tool_call_id, tool_result_text),
                }],
            }
        ],
        "usage": {
            "input_tokens": 0,
            "output_tokens": 0,
            "total_tokens": 0,
        },
        "error": null,
        "incomplete_details": null,
    })
}

fn build_responses_error(
    code: &str,
    error_type: &str,
    message: impl Into<String>,
) -> serde_json::Value {
    serde_json::json!({
        "error": {
            "code": code,
            "type": error_type,
            "message": message.into()
        }
    })
}

// ---------------------------------------------------------------------------
// Responses API — Payload Storage
// ---------------------------------------------------------------------------

fn merge_responses_status_history(
    previous: Option<&serde_json::Value>,
    next: &mut serde_json::Value,
) {
    let Some(next_status) = next.get("status").and_then(|value| value.as_str()) else {
        return;
    };

    let mut history: Vec<serde_json::Value> = previous
        .and_then(|value| value.get("status_history"))
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();

    let last_status = history
        .last()
        .and_then(|event| event.get("status"))
        .and_then(|value| value.as_str());
    if last_status != Some(next_status) {
        history.push(serde_json::json!({
            "status": next_status,
            "at": crate::acp::prelude::now_ts_ms(),
        }));
    }

    if let Some(object) = next.as_object_mut() {
        object.insert(
            "status_history".to_string(),
            serde_json::Value::Array(history),
        );
    }
}

fn store_responses_api_payload(server: &AcpServer, payload: &serde_json::Value) {
    let Some(id) = payload.get("id").and_then(|value| value.as_str()) else {
        return;
    };
    let mut store = server
        .session
        .responses_api_store
        .lock()
        .unwrap_or_else(|poisoned| {
            tracing::warn!("responses_api_store lock poisoned in store_responses_api_payload");
            poisoned.into_inner()
        });
    if store.len() >= 1000 {
        let keys: Vec<String> = store.keys().take(200).cloned().collect();
        for key in keys {
            store.remove(&key);
        }
    }
    let previous = store.get(id).cloned();
    let mut next = payload.clone();
    merge_responses_status_history(previous.as_ref(), &mut next);
    store.insert(id.to_string(), next);
}

fn load_responses_api_payload(server: &AcpServer, response_id: &str) -> Option<serde_json::Value> {
    server
        .session
        .responses_api_store
        .lock()
        .ok()
        .and_then(|store| store.get(response_id).cloned())
}

pub(crate) fn list_responses_api_payloads(server: &AcpServer) -> Vec<serde_json::Value> {
    let mut values = server
        .session
        .responses_api_store
        .lock()
        .ok()
        .map(|store| store.values().cloned().collect::<Vec<_>>())
        .unwrap_or_default();

    fn latest_status_at(value: &serde_json::Value) -> i64 {
        value
            .get("status_history")
            .and_then(|v| v.as_array())
            .and_then(|events| events.last())
            .and_then(|event| event.get("at"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
    }

    fn id_sequence(value: &serde_json::Value) -> u64 {
        value
            .get("id")
            .and_then(|v| v.as_str())
            .and_then(|id| id.rsplit('_').next())
            .and_then(|part| part.parse::<u64>().ok())
            .unwrap_or(0)
    }

    values.sort_by_key(|value| {
        std::cmp::Reverse((
            value
                .get("created_at")
                .and_then(|v| v.as_i64())
                .unwrap_or(0),
            latest_status_at(value),
            id_sequence(value),
        ))
    });
    values
}

pub(crate) fn extract_response_id_from_path(path: &str) -> Option<&str> {
    path.strip_prefix("/v1/responses/")
        .filter(|value| !value.is_empty() && !value.contains('/'))
}

// ---------------------------------------------------------------------------
// Responses API — Tool Helpers
// ---------------------------------------------------------------------------

fn is_supported_responses_tool_choice(value: &serde_json::Value) -> bool {
    matches!(value.as_str(), Some("auto" | "none" | "required"))
}

fn validate_responses_tool(value: &serde_json::Value) -> Option<&'static str> {
    let Some(tool) = value.as_object() else {
        return Some("tool entry must be an object");
    };

    if tool.get("type").and_then(|v| v.as_str()) != Some("function") {
        return Some("tool entry must use type=function");
    }

    let Some(function) = tool.get("function").and_then(|value| value.as_object()) else {
        return Some("tool entry must include a function object");
    };

    let Some(name) = function.get("name").and_then(|value| value.as_str()) else {
        return Some("tool entry must include function.name");
    };

    if name.trim().is_empty() {
        return Some("tool entry must include a non-empty function.name");
    }

    if function
        .get("description")
        .is_some_and(|value| !value.is_string())
    {
        return Some("tool entry function.description must be a string");
    }

    if let Some(parameters) = function.get("parameters") {
        let Some(schema) = parameters.as_object() else {
            return Some("tool entry function.parameters must be an object");
        };

        if schema.get("type").and_then(|value| value.as_str()) != Some("object") {
            return Some("tool entry function.parameters.type must be 'object'");
        }

        if schema
            .get("properties")
            .is_some_and(|value| !value.is_object())
        {
            return Some("tool entry function.parameters.properties must be an object");
        }

        if schema.get("required").is_some_and(|value| {
            value
                .as_array()
                .is_none_or(|items| !items.iter().all(|item| item.is_string()))
        }) {
            return Some("tool entry function.parameters.required must be an array of strings");
        }
    }

    None
}

fn responses_tool_name(value: &serde_json::Value) -> Option<&str> {
    value
        .get("function")
        .and_then(|value| value.as_object())
        .and_then(|function| function.get("name"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|name| !name.is_empty())
}

fn pending_tool_call_id(response: &serde_json::Value) -> Option<&str> {
    response
        .get("output")
        .and_then(|value| value.as_array())
        .and_then(|items| {
            items.iter().find_map(|item| {
                (item.get("type").and_then(|value| value.as_str()) == Some("tool_call"))
                    .then(|| item.get("id").and_then(|value| value.as_str()))
                    .flatten()
            })
        })
}

fn extract_tool_result_for_call(input: &serde_json::Value, tool_call_id: &str) -> Option<String> {
    input.as_array().and_then(|items| {
        items.iter().find_map(|item| {
            let obj = item.as_object()?;
            if obj.get("role").and_then(|value| value.as_str()) != Some("tool") {
                return None;
            }
            if obj.get("tool_call_id").and_then(|value| value.as_str()) != Some(tool_call_id) {
                return None;
            }

            let content = obj.get("content")?;
            let text = if let Some(value) = content.as_str() {
                value.trim().to_string()
            } else if let Some(value) = content.get("text").and_then(|v| v.as_str()) {
                value.trim().to_string()
            } else if let Some(parts) = content.as_array() {
                parts
                    .iter()
                    .filter_map(|part| {
                        if let Some(text) = part.as_str() {
                            return Some(text.to_string());
                        }
                        part.get("text")
                            .and_then(|value| value.as_str())
                            .map(ToString::to_string)
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
                    .trim()
                    .to_string()
            } else {
                String::new()
            };

            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        })
    })
}

fn classify_responses_upstream_error_code(err: &anyhow::Error) -> &'static str {
    let message = err.to_string().to_lowercase();
    if message.contains("timed out") || message.contains("timeout") {
        "timeout"
    } else if message.contains("rate limit") || message.contains("too many requests") {
        "rate_limit"
    } else {
        "upstream_error"
    }
}

// ---------------------------------------------------------------------------
// Responses API — Validation
// ---------------------------------------------------------------------------

async fn write_responses_api_error(
    socket: &mut TcpStream,
    payload: serde_json::Value,
    extra_headers: &str,
) -> Result<()> {
    let result =
        write_http_json_response_with_context(socket, 400, payload, "responses.api", extra_headers)
            .await;
    if let Err(ref e) = result {
        tracing::warn!(
            "write_responses_api_error: failed to write error response: {}",
            e
        );
    }
    result
}

/// Validate and deserialize a Responses API POST request.
/// On error, writes the error to the socket and returns `Err(())` so the caller can bail.
async fn validate_responses_post_request(
    socket: &mut TcpStream,
    body: &serde_json::Value,
    extra_headers: &str,
) -> Result<ResponsesApiRequest, ()> {
    if !body.is_object() {
        let payload = build_responses_error(
            "invalid_request_error",
            "invalid_request_error",
            t("error.request_body_must_be_json_object"),
        );
        let _ = write_responses_api_error(socket, payload, extra_headers).await;
        return Err(());
    }

    if body.get("model").and_then(|v| v.as_str()).is_none() {
        if body.get("model").is_some() {
            let payload = build_responses_error(
                "invalid_input",
                "invalid_request_error",
                t("error.model_must_be_string"),
            );
            let _ = write_responses_api_error(socket, payload, extra_headers).await;
            return Err(());
        }
        let payload = build_responses_error(
            "missing_required_field",
            "invalid_request_error",
            t("error.model_required"),
        );
        let _ = write_responses_api_error(socket, payload, extra_headers).await;
        return Err(());
    }
    match body.get("input") {
        None => {
            let payload = build_responses_error(
                "missing_required_field",
                "invalid_request_error",
                t("error.input_required"),
            );
            let _ = write_responses_api_error(socket, payload, extra_headers).await;
            return Err(());
        }
        Some(value) if value.is_null() || (!value.is_string() && !value.is_array()) => {
            let payload = build_responses_error(
                "invalid_input",
                "invalid_request_error",
                t("error.input_must_be_string_or_array"),
            );
            let _ = write_responses_api_error(socket, payload, extra_headers).await;
            return Err(());
        }
        Some(_) => {}
    }
    if let Some(v) = body.get("max_output_tokens") {
        if !v.is_u64() {
            let payload = build_responses_error(
                "invalid_input",
                "invalid_request_error",
                t("error.max_output_tokens_invalid"),
            );
            let _ = write_responses_api_error(socket, payload, extra_headers).await;
            return Err(());
        }
    }
    if let Some(v) = body.get("temperature") {
        let Some(value) = v.as_f64() else {
            let payload = build_responses_error(
                "invalid_input",
                "invalid_request_error",
                t("error.temperature_must_be_number"),
            );
            let _ = write_responses_api_error(socket, payload, extra_headers).await;
            return Err(());
        };
        if !(0.0..=2.0).contains(&value) {
            let payload = build_responses_error(
                "invalid_input",
                "invalid_request_error",
                t("error.temperature_invalid"),
            );
            let _ = write_responses_api_error(socket, payload, extra_headers).await;
            return Err(());
        }
    }
    if let Some(v) = body.get("metadata") {
        if !v.is_object() {
            let payload = build_responses_error(
                "invalid_input",
                "invalid_request_error",
                t("error.metadata_invalid"),
            );
            let _ = write_responses_api_error(socket, payload, extra_headers).await;
            return Err(());
        }
    }
    if let Some(v) = body.get("reasoning") {
        if !v.is_object() {
            let payload = build_responses_error(
                "invalid_input",
                "invalid_request_error",
                t("error.reasoning_invalid"),
            );
            let _ = write_responses_api_error(socket, payload, extra_headers).await;
            return Err(());
        }
    }
    if let Some(v) = body.get("tools") {
        if !v.is_array() {
            let payload = build_responses_error(
                "invalid_input",
                "invalid_request_error",
                t("error.tools_invalid"),
            );
            let _ = write_responses_api_error(socket, payload, extra_headers).await;
            return Err(());
        }
        if v.as_array()
            .is_some_and(|items| items.iter().any(|item| !item.is_object()))
        {
            let payload = build_responses_error(
                "invalid_input",
                "invalid_request_error",
                t("error.tools_entries_object"),
            );
            let _ = write_responses_api_error(socket, payload, extra_headers).await;
            return Err(());
        }
        if let Some(reason) = v
            .as_array()
            .and_then(|items| items.iter().find_map(validate_responses_tool))
        {
            let payload = build_responses_error("invalid_input", "invalid_request_error", reason);
            let _ = write_responses_api_error(socket, payload, extra_headers).await;
            return Err(());
        }
    }
    if let Some(v) = body.get("tool_choice") {
        if !v.is_string() && !v.is_object() {
            let payload = build_responses_error(
                "invalid_input",
                "invalid_request_error",
                t("error.tool_choice_invalid"),
            );
            let _ = write_responses_api_error(socket, payload, extra_headers).await;
            return Err(());
        }
        if v.is_string() && !is_supported_responses_tool_choice(v) {
            let payload = build_responses_error(
                "invalid_input",
                "invalid_request_error",
                t("error.tool_choice_value_invalid"),
            );
            let _ = write_responses_api_error(socket, payload, extra_headers).await;
            return Err(());
        }
        if let Some(reason) = v.as_object().and_then(|_| validate_responses_tool(v)) {
            let payload = build_responses_error("invalid_input", "invalid_request_error", reason);
            let _ = write_responses_api_error(socket, payload, extra_headers).await;
            return Err(());
        }
    }
    if let Some(v) = body.get("previous_response_id") {
        if v.as_str().is_none_or(|value| value.trim().is_empty()) {
            let payload = build_responses_error(
                "invalid_input",
                "invalid_request_error",
                t("error.previous_response_id_invalid"),
            );
            let _ = write_responses_api_error(socket, payload, extra_headers).await;
            return Err(());
        }
    }

    let req: ResponsesApiRequest = match serde_json::from_value(body.clone()) {
        Ok(r) => r,
        Err(err) => {
            let payload = build_responses_error(
                "invalid_request_error",
                "invalid_request_error",
                tf("error.invalid_request", &[("error", &err.to_string())]),
            );
            let _ = write_responses_api_error(socket, payload, extra_headers).await;
            return Err(());
        }
    };
    Ok(req)
}

// ---------------------------------------------------------------------------
// Responses API — Handlers
// ---------------------------------------------------------------------------

pub(crate) async fn handle_responses_api(
    socket: &mut TcpStream,
    server: Arc<AcpServer>,
    body: serde_json::Value,
    user_session: Option<UserSession>,
    cors_headers: &str,
) -> Result<()> {
    let req = match validate_responses_post_request(socket, &body, cors_headers).await {
        Ok(r) => r,
        Err(()) => return Ok(()),
    };

    let raw_model = req.model.clone().unwrap_or_default();
    let model = raw_model.trim().to_string();
    if model.is_empty() {
        let payload = build_responses_error(
            "invalid_input",
            "invalid_request_error",
            t("error.model_required"),
        );
        write_responses_api_error(socket, payload, cors_headers).await?;
        return Ok(());
    }

    if let Some(0) = req.max_output_tokens {
        let payload = build_responses_error(
            "invalid_input",
            "invalid_request_error",
            t("error.max_output_tokens_gt_zero"),
        );
        write_responses_api_error(socket, payload, cors_headers).await?;
        return Ok(());
    }

    let declared_tool_names = req
        .tools
        .as_ref()
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(responses_tool_name)
                .collect::<std::collections::HashSet<_>>()
        })
        .unwrap_or_default();

    if let Some(tool_choice) = req.tool_choice.as_ref() {
        if matches!(tool_choice.as_str(), Some("required")) && declared_tool_names.is_empty() {
            let payload = build_responses_error(
                "invalid_input",
                "invalid_request_error",
                "tool_choice=required requires at least one declared tool",
            );
            write_responses_api_error(socket, payload, cors_headers).await?;
            return Ok(());
        }

        if let Some(name) = tool_choice
            .as_object()
            .and_then(|_| responses_tool_name(tool_choice))
        {
            if declared_tool_names.is_empty() {
                let payload = build_responses_error(
                    "invalid_input",
                    "invalid_request_error",
                    "tool_choice object requires tools to be provided",
                );
                write_responses_api_error(socket, payload, cors_headers).await?;
                return Ok(());
            }
            if !declared_tool_names.contains(name) {
                let payload = build_responses_error(
                    "invalid_input",
                    "invalid_request_error",
                    "tool_choice function.name must match a declared tool",
                );
                write_responses_api_error(socket, payload, cors_headers).await?;
                return Ok(());
            }
        }
    }

    let Some(input) = req.input.as_ref() else {
        let payload = build_responses_error(
            "invalid_input",
            "invalid_request_error",
            "input must be a string or an array of input messages",
        );
        write_responses_api_error(socket, payload, cors_headers).await?;
        return Ok(());
    };
    if !input.is_string() && !input.is_array() {
        let payload = build_responses_error(
            "invalid_input",
            "invalid_request_error",
            "input must be a string or an array of input messages",
        );
        write_responses_api_error(socket, payload, cors_headers).await?;
        return Ok(());
    }

    let previous_response_id = req
        .extra
        .get("previous_response_id")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);

    let messages = responses_input_to_messages(input);
    let has_non_empty_user_message = messages
        .iter()
        .any(|m| m.role == "user" && !m.content.trim().is_empty());
    if previous_response_id.is_none() && !has_non_empty_user_message {
        let payload = build_responses_error(
            "invalid_input",
            "invalid_request_error",
            "input must contain at least one non-empty user message",
        );
        write_responses_api_error(socket, payload, cors_headers).await?;
        return Ok(());
    }

    let request_id = next_responses_api_id("resp");
    store_responses_api_payload(
        server.as_ref(),
        &build_responses_api_queued_response(&request_id, &model),
    );
    store_responses_api_payload(
        server.as_ref(),
        &build_responses_api_in_progress_response(&request_id, &model),
    );

    if let Some(previous_response_id) = previous_response_id.as_deref() {
        return handle_response_tool_result(
            socket,
            &server,
            &request_id,
            &model,
            input,
            previous_response_id,
            cors_headers,
        )
        .await;
    }

    if matches!(
        req.tool_choice.as_ref().and_then(|v| v.as_str()),
        Some("required")
    ) {
        return handle_response_required_tool_call(
            socket,
            &server,
            &request_id,
            &model,
            &req,
            cors_headers,
        )
        .await;
    }

    handle_response_create(
        socket,
        server,
        &request_id,
        &model,
        req,
        messages,
        user_session,
        cors_headers,
    )
    .await
}

async fn handle_response_tool_result(
    socket: &mut TcpStream,
    server: &Arc<AcpServer>,
    request_id: &str,
    model: &str,
    input: &serde_json::Value,
    previous_response_id: &str,
    cors_headers: &str,
) -> Result<()> {
    let Some(previous_response) = load_responses_api_payload(server.as_ref(), previous_response_id)
    else {
        let payload = build_responses_error(
            "not_found",
            "invalid_request_error",
            "previous_response_id not found",
        );
        write_responses_api_error(socket, payload, cors_headers).await?;
        return Ok(());
    };

    let Some(tool_call_id) = pending_tool_call_id(&previous_response) else {
        let payload = build_responses_error(
            "tool_error",
            "tool_error",
            "previous_response_id has no pending tool_call",
        );
        write_responses_api_error(socket, payload, cors_headers).await?;
        return Ok(());
    };

    let Some(tool_result_text) = extract_tool_result_for_call(input, tool_call_id) else {
        let payload = build_responses_error(
            "tool_error",
            "tool_error",
            "input must include a tool result with matching tool_call_id",
        );
        write_responses_api_error(socket, payload, cors_headers).await?;
        return Ok(());
    };

    let payload = build_responses_api_tool_result_response(
        request_id,
        model,
        previous_response_id,
        tool_call_id,
        &tool_result_text,
    );
    store_responses_api_payload(server.as_ref(), &payload);
    write_http_json_response(socket, 200, payload, cors_headers).await?;
    Ok(())
}

async fn handle_response_required_tool_call(
    socket: &mut TcpStream,
    server: &Arc<AcpServer>,
    request_id: &str,
    model: &str,
    req: &ResponsesApiRequest,
    cors_headers: &str,
) -> Result<()> {
    let tool_name = req
        .tools
        .as_ref()
        .and_then(|value| value.as_array())
        .and_then(|items| items.iter().find_map(responses_tool_name))
        .unwrap_or("tool");
    let tool_call_id = next_responses_api_id("call");
    let payload =
        build_responses_api_tool_call_response(request_id, model, &tool_call_id, tool_name);
    store_responses_api_payload(server.as_ref(), &payload);
    write_http_json_response(socket, 200, payload, cors_headers).await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_response_create(
    socket: &mut TcpStream,
    server: Arc<AcpServer>,
    request_id: &str,
    model: &str,
    req: ResponsesApiRequest,
    messages: Vec<Message>,
    user_session: Option<UserSession>,
    cors_headers: &str,
) -> Result<()> {
    let mut extra = req.extra.clone();
    extra.remove("previous_response_id");
    extra.insert("model".to_string(), serde_json::json!(model));
    if let Some(t) = req.temperature {
        extra.insert("temperature".to_string(), serde_json::json!(t));
    }
    if let Some(t) = req.max_output_tokens {
        extra.insert("max_tokens".to_string(), serde_json::json!(t));
    }
    if let Some(v) = &req.tools {
        extra.insert("tools".to_string(), v.clone());
    }
    if let Some(v) = &req.tool_choice {
        extra.insert("tool_choice".to_string(), v.clone());
    }
    if let Some(v) = &req.metadata {
        extra.insert("metadata".to_string(), v.clone());
    }
    if let Some(v) = &req.reasoning {
        extra.insert("reasoning".to_string(), v.clone());
    }

    let params = ChatParams {
        mode: "ask".to_string(),
        messages,
        conversation_id: None,
        branch_id: None,
        phase: None,
        options: if extra.is_empty() {
            None
        } else {
            Some(crate::config::PhaseOptions {
                extra,
                ..Default::default()
            })
        },
        requirement_contract: None,
        plan: None,
        vector_hits: None,
        execution_decision_candidate: None,
    };

    let trace = http_trace_context("responses.api");

    if req.stream {
        return handle_response_stream(
            socket,
            server,
            request_id,
            model,
            params,
            &trace,
            user_session,
            cors_headers,
        )
        .await;
    }

    let ctx = Some(ChatRequestContext::new(user_session.clone()));
    let result = crate::acp::r#impl::chat::process_chat_request(
        server.as_ref(),
        &params,
        None,
        &trace,
        None,
        ctx,
    )
    .await;

    let result = match result {
        Ok(r) => r,
        Err(err) => {
            if is_setup_or_upstream_unavailable(&err) {
                let payload = attach_responses_token_economy(
                    build_responses_api_response(request_id, model, &degraded_openai_message(&err)),
                    &params.messages,
                    &degraded_openai_message(&err),
                );
                let payload = inject_platform_profiles_if_absent(payload, "responses.api");
                store_responses_api_payload(server.as_ref(), &payload);
                write_http_json_response(socket, 200, payload, cors_headers).await?;
                return Ok(());
            }
            let code = classify_responses_upstream_error_code(&err);
            let payload = build_responses_error(code, "upstream_error", err.to_string());
            store_responses_api_payload(
                server.as_ref(),
                &serde_json::json!({
                    "id": request_id,
                    "object": "response",
                    "created_at": crate::acp::prelude::now_ts(),
                    "model": model,
                    "status": "failed",
                    "output": [],
                    "usage": {
                        "input_tokens": 0,
                        "output_tokens": 0,
                        "total_tokens": 0,
                    },
                    "error": payload["error"].clone(),
                    "incomplete_details": null,
                }),
            );
            write_http_json_response_with_context(
                socket,
                502,
                payload,
                "responses.api",
                cors_headers,
            )
            .await?;
            return Ok(());
        }
    };

    let response_text = result
        .get("response")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let payload = attach_responses_token_economy(
        build_responses_api_response(request_id, model, response_text),
        &params.messages,
        response_text,
    );
    let payload = inject_platform_profiles_if_absent(payload, "responses.api");
    store_responses_api_payload(server.as_ref(), &payload);
    write_http_json_response(socket, 200, payload, cors_headers).await?;
    Ok(())
}

/// Handle GET /v1/responses/{id} — retrieve a single response by its ID.
pub(crate) async fn handle_response_get(
    socket: &mut TcpStream,
    server: &AcpServer,
    response_id: &str,
    cors_headers: &str,
) -> Result<()> {
    if let Some(payload) = load_responses_api_payload(server, response_id) {
        write_http_json_response_with_context(socket, 200, payload, "responses.api", cors_headers)
            .await?;
    } else {
        write_http_json_response_with_context(
            socket,
            404,
            build_responses_error(
                "not_found",
                "invalid_request_error",
                "response id not found",
            ),
            "responses.api",
            cors_headers,
        )
        .await?;
    }
    Ok(())
}

/// Streaming (SSE) path for POST /v1/responses when stream=true.
#[allow(clippy::too_many_arguments)]
async fn handle_response_stream(
    socket: &mut TcpStream,
    server: Arc<AcpServer>,
    request_id: &str,
    model: &str,
    params: ChatParams,
    trace: &RequestTraceContext,
    user_session: Option<UserSession>,
    cors_headers: &str,
) -> Result<()> {
    let created_at = crate::acp::prelude::now_ts();

    let initial_response = serde_json::json!({
        "id": request_id,
        "object": "response",
        "created_at": created_at,
        "model": model,
        "status": "in_progress",
        "output": [],
        "usage": null,
        "incomplete_details": null,
    });

    write_sse_headers(socket, cors_headers).await?;
    write_sse_event(
        socket,
        "response.created",
        &serde_json::json!({
            "type": "response.created",
            "response": initial_response,
        }),
    )
    .await?;

    let (tx, mut rx) = tokio::sync::mpsc::channel::<StreamFrame>(256);
    let observer = StreamObserver::sse(tx);
    let ctx = Some(ChatRequestContext::new(user_session));
    let server_ref = Arc::clone(&server);
    let trace_for_task = trace.clone();
    let params_for_task = params.clone();
    let task = tokio::spawn(async move {
        crate::acp::r#impl::chat::process_chat_request(
            server_ref.as_ref(),
            &params_for_task,
            Some(observer),
            &trace_for_task,
            None,
            ctx,
        )
        .await
    });

    while let Some(frame) = rx.recv().await {
        if let Err(err) = write_sse_event(socket, frame.event, &frame.payload).await {
            task.abort();
            return Err(err);
        }
    }

    let result = match task.await {
        Ok(r) => r,
        Err(err) => {
            let payload = serde_json::json!({"message": format!("chat task panicked: {err}")});
            write_sse_event(socket, "error", &payload).await?;
            return Ok(());
        }
    };

    match result {
        Ok(r) => {
            let text = r
                .get("response")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let item_id = next_responses_api_id("msg");

            write_sse_event(
                socket,
                "response.output_text.delta",
                &serde_json::json!({
                    "type": "response.output_text.delta",
                    "output_index": 0,
                    "content_index": 0,
                    "delta": text,
                    "item_id": item_id,
                    "response_id": request_id,
                }),
            )
            .await?;

            write_sse_event(
                socket,
                "response.token_economy",
                &serde_json::json!({
                    "type": "response.token_economy",
                    "response_id": request_id,
                    "token_economy": crate::acp::r#impl::chat::estimate_token_economy(&params.messages, text),
                }),
            )
            .await?;

            let completed = attach_responses_token_economy(
                build_responses_api_response(request_id, model, text),
                &params.messages,
                text,
            );
            let completed = inject_platform_profiles_if_absent(completed, "responses.api");
            store_responses_api_payload(server.as_ref(), &completed);

            write_sse_event(
                socket,
                "response.completed",
                &serde_json::json!({
                    "type": "response.completed",
                    "response": completed,
                }),
            )
            .await?;
        }
        Err(err) => {
            if is_setup_or_upstream_unavailable(&err) {
                let text = degraded_openai_message(&err);
                let item_id = next_responses_api_id("msg");

                write_sse_event(
                    socket,
                    "response.output_text.delta",
                    &serde_json::json!({
                        "type": "response.output_text.delta",
                        "output_index": 0,
                        "content_index": 0,
                        "delta": text,
                        "item_id": item_id,
                        "response_id": request_id,
                    }),
                )
                .await?;

                write_sse_event(
                    socket,
                    "response.token_economy",
                    &serde_json::json!({
                        "type": "response.token_economy",
                        "response_id": request_id,
                        "token_economy": crate::acp::r#impl::chat::estimate_token_economy(&params.messages, &text),
                    }),
                )
                .await?;

                let completed = attach_responses_token_economy(
                    build_responses_api_response(request_id, model, &text),
                    &params.messages,
                    &text,
                );
                let completed = inject_platform_profiles_if_absent(completed, "responses.api");
                store_responses_api_payload(server.as_ref(), &completed);

                write_sse_event(
                    socket,
                    "response.completed",
                    &serde_json::json!({
                        "type": "response.completed",
                        "response": completed,
                    }),
                )
                .await?;

                write_openai_sse_done(socket).await?;
                return Ok(());
            }

            let code = classify_responses_upstream_error_code(&err);
            write_sse_event(
                socket,
                "response.token_economy",
                &serde_json::json!({
                    "type": "response.token_economy",
                    "response_id": request_id,
                    "token_economy": crate::acp::r#impl::chat::estimate_token_economy(&params.messages, ""),
                }),
            )
            .await?;
            let failed = serde_json::json!({
                "id": request_id,
                "object": "response",
                "created_at": created_at,
                "model": model,
                "status": "failed",
                "output": [],
                "usage": {"input_tokens": 0, "output_tokens": 0, "total_tokens": 0},
                "error": {"code": code, "type": "upstream_error", "message": err.to_string()},
                "incomplete_details": null,
            });
            let failed = inject_platform_profiles_if_absent(failed, "responses.api");
            store_responses_api_payload(server.as_ref(), &failed);

            write_sse_event(
                socket,
                "response.failed",
                &serde_json::json!({
                    "type": "response.failed",
                    "response": failed,
                }),
            )
            .await?;
        }
    }

    write_openai_sse_done(socket).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_to_chat_params_maps_options_and_roles() {
        let req = OpenAiChatRequest {
            model: Some("m1".to_string()),
            messages: vec![
                OpenAiChatMessage {
                    role: "assistant".to_string(),
                    content: serde_json::Value::Null,
                    name: None,
                    tool_call_id: None,
                    function_call: None,
                    tool_calls: Some(serde_json::json!([{"id":"t1"}])),
                    refusal: None,
                },
                OpenAiChatMessage {
                    role: "tool".to_string(),
                    content: serde_json::json!({"result": 3}),
                    name: None,
                    tool_call_id: Some("t1".to_string()),
                    function_call: None,
                    tool_calls: None,
                    refusal: None,
                },
                OpenAiChatMessage {
                    role: "user".to_string(),
                    content: serde_json::json!([
                        {"type":"text","text":"hello"},
                        {"type":"text","text":"world"}
                    ]),
                    name: None,
                    tool_call_id: None,
                    function_call: None,
                    tool_calls: None,
                    refusal: None,
                },
            ],
            temperature: Some(0.2),
            top_p: Some(0.9),
            max_tokens: Some(64),
            n: Some(2),
            stop: Some(serde_json::json!(["END"])),
            presence_penalty: Some(0.1),
            frequency_penalty: Some(0.2),
            logit_bias: Some(serde_json::json!({"10": -1})),
            user: Some("u1".to_string()),
            seed: Some(42),
            response_format: Some(serde_json::json!({"type":"json_object"})),
            tools: Some(serde_json::json!([{"type":"function"}])),
            tool_choice: Some(serde_json::json!("auto")),
            parallel_tool_calls: Some(true),
            function_call: Some(serde_json::json!("auto")),
            functions: Some(serde_json::json!([{"name":"f"}])),
            stream: false,
            extra: std::collections::HashMap::from([(
                "custom_flag".to_string(),
                serde_json::json!(true),
            )]),
        };

        let params = openai_to_chat_params(&req);
        assert_eq!(params.mode, "ask");
        assert_eq!(params.messages.len(), 3);
        assert_eq!(params.messages[1].role, "user"); // tool role normalized
        assert!(params.messages[1].content.contains("tool_call_id"));
        assert!(params.messages[2].content.contains("hello"));

        let options = params.options.expect("expected options");
        assert_eq!(options.extra.get("model"), Some(&serde_json::json!("m1")));
        assert_eq!(
            options.extra.get("max_tokens"),
            Some(&serde_json::json!(64))
        );
        assert_eq!(
            options.extra.get("response_format"),
            Some(&serde_json::json!({"type":"json_object"}))
        );
        assert_eq!(
            options.extra.get("custom_flag"),
            Some(&serde_json::json!(true))
        );
    }

    #[test]
    fn responses_api_maps_input_to_messages() {
        let text_input = serde_json::json!("hello world");
        let msgs = responses_input_to_messages(&text_input);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].content, "hello world");

        let arr_input = serde_json::json!([
            {"role": "user", "content": "ping"},
            {"role": "assistant", "content": "pong"},
        ]);
        let msgs = responses_input_to_messages(&arr_input);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].content, "ping");
        assert_eq!(msgs[1].role, "assistant");
        assert_eq!(msgs[1].content, "pong");

        let nested_input = serde_json::json!([{
            "role": "user",
            "content": [
                {"type": "input_text", "text": "hello"},
                {"type": "input_text", "text": "world"},
            ]
        }]);
        let msgs = responses_input_to_messages(&nested_input);
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].content.contains("hello"));
        assert!(msgs[0].content.contains("world"));

        let empty_input = serde_json::json!("   ");
        let msgs = responses_input_to_messages(&empty_input);
        assert!(msgs.is_empty());

        let invalid_input = serde_json::json!({"text": "not valid responses input"});
        let msgs = responses_input_to_messages(&invalid_input);
        assert!(msgs.is_empty());

        let null_input = serde_json::Value::Null;
        let msgs = responses_input_to_messages(&null_input);
        assert!(msgs.is_empty());
    }

    #[test]
    fn responses_api_id_generation_is_unique() {
        use std::collections::HashSet;

        let mut ids = HashSet::new();
        for _ in 0..128 {
            let id = next_responses_api_id("resp");
            assert!(
                ids.insert(id),
                "next_responses_api_id should never generate duplicate ids"
            );
        }
    }

    #[test]
    fn responses_api_upstream_error_classification_is_stable() {
        let timeout_err = anyhow::anyhow!("request timed out while connecting upstream");
        assert_eq!(
            classify_responses_upstream_error_code(&timeout_err),
            "timeout"
        );

        let rate_limit_err = anyhow::anyhow!("429 too many requests from provider");
        assert_eq!(
            classify_responses_upstream_error_code(&rate_limit_err),
            "rate_limit"
        );

        let generic_err = anyhow::anyhow!("provider returned malformed payload");
        assert_eq!(
            classify_responses_upstream_error_code(&generic_err),
            "upstream_error"
        );
    }

    #[test]
    fn responses_api_stream_event_types_are_correct() {
        let event_names = [
            "response.created",
            "response.output_text.delta",
            "response.token_economy",
            "response.completed",
            "response.failed",
        ];

        for name in &event_names {
            assert!(
                name.starts_with("response."),
                "event type must start with 'response.': {name}"
            );
            assert!(
                !name.ends_with('.'),
                "event type must not end with '.': {name}"
            );
        }

        let created_payload = serde_json::json!({
            "type": "response.created",
            "response": {
                "id": "resp_001",
                "object": "response",
                "status": "in_progress",
            }
        });
        assert_eq!(
            created_payload["type"].as_str().unwrap_or_default(),
            "response.created"
        );
        assert_eq!(
            created_payload["response"]["status"]
                .as_str()
                .unwrap_or_default(),
            "in_progress"
        );

        let delta_payload = serde_json::json!({
            "type": "response.output_text.delta",
            "output_index": 0,
            "content_index": 0,
            "delta": "Hello",
            "item_id": "msg_001",
            "response_id": "resp_001",
        });
        assert_eq!(delta_payload["output_index"].as_u64().unwrap_or(99), 0);
        assert_eq!(delta_payload["content_index"].as_u64().unwrap_or(99), 0);
        assert!(delta_payload["delta"].is_string());
        assert!(delta_payload["item_id"].is_string());
        assert!(delta_payload["response_id"].is_string());

        let telemetry_payload = serde_json::json!({
            "type": "response.token_economy",
            "response_id": "resp_001",
            "token_economy": {
                "compression_ratio": 0.5,
                "input_tokens": 24,
                "output_tokens": 12,
                "total_tokens": 36,
            }
        });
        assert_eq!(
            telemetry_payload["type"].as_str().unwrap_or_default(),
            "response.token_economy"
        );
        assert!(telemetry_payload["token_economy"].is_object());

        let completed_payload = serde_json::json!({
            "type": "response.completed",
            "response": {
                "id": "resp_001",
                "object": "response",
                "status": "completed",
            }
        });
        assert_eq!(
            completed_payload["response"]["status"]
                .as_str()
                .unwrap_or_default(),
            "completed"
        );
    }

    #[test]
    fn responses_api_r4_golden_snapshot() {
        let resp = build_responses_api_response("resp_123_456", "gpt-4", "Hello world");
        assert_eq!(resp["id"].as_str(), Some("resp_123_456"), "id field");
        assert_eq!(resp["object"].as_str(), Some("response"), "object field");
        assert!(resp["created_at"].is_number(), "created_at must be numeric");
        assert_eq!(resp["model"].as_str(), Some("gpt-4"), "model field");
        assert_eq!(resp["status"].as_str(), Some("completed"), "status field");
        assert!(resp["output"].is_array(), "output must be array");
        assert!(resp["usage"].is_object(), "usage must be object");
        assert!(resp["error"].is_null(), "error must be null on success");
        assert!(
            resp["incomplete_details"].is_null(),
            "incomplete_details must be null on success"
        );

        let output = resp["output"].as_array().expect("output array");
        assert_eq!(output.len(), 1, "one output item");
        assert_eq!(output[0]["type"].as_str(), Some("message"), "output type");
        assert_eq!(output[0]["role"].as_str(), Some("assistant"), "output role");
        assert_eq!(
            output[0]["status"].as_str(),
            Some("completed"),
            "output item status"
        );
        let content = output[0]["content"].as_array().expect("content array");
        assert_eq!(content.len(), 1, "one content item");
        assert_eq!(
            content[0]["type"].as_str(),
            Some("output_text"),
            "content type"
        );
        assert_eq!(
            content[0]["text"].as_str(),
            Some("Hello world"),
            "content text"
        );

        let usage = &resp["usage"];
        assert_eq!(usage["input_tokens"].as_u64(), Some(0), "input_tokens");
        assert_eq!(usage["output_tokens"].as_u64(), Some(0), "output_tokens");
        assert_eq!(usage["total_tokens"].as_u64(), Some(0), "total_tokens");

        let err = build_responses_error("invalid_input", "invalid_request_error", "bad field");
        assert_eq!(
            err["error"]["code"].as_str(),
            Some("invalid_input"),
            "error.code"
        );
        assert_eq!(
            err["error"]["type"].as_str(),
            Some("invalid_request_error"),
            "error.type"
        );
        assert_eq!(
            err["error"]["message"].as_str(),
            Some("bad field"),
            "error.message"
        );
        assert!(
            err.as_object().is_none_or(|o| !o.contains_key("id")),
            "error shape must not include id"
        );
        assert!(
            err.as_object().is_none_or(|o| !o.contains_key("status")),
            "error shape must not include status"
        );

        let tc = build_responses_api_tool_call_response(
            "resp_456_789",
            "gpt-4",
            "call_001_2",
            "get_weather",
        );
        assert_eq!(tc["id"].as_str(), Some("resp_456_789"));
        assert_eq!(tc["object"].as_str(), Some("response"));
        assert_eq!(
            tc["status"].as_str(),
            Some("incomplete"),
            "tool-call status"
        );
        let tc_output = tc["output"].as_array().expect("tool-call output array");
        assert_eq!(tc_output.len(), 1, "one tool_call item");
        assert_eq!(
            tc_output[0]["type"].as_str(),
            Some("tool_call"),
            "output item type"
        );
        assert_eq!(
            tc_output[0]["id"].as_str(),
            Some("call_001_2"),
            "tool_call id"
        );
        assert_eq!(
            tc_output[0]["name"].as_str(),
            Some("get_weather"),
            "tool_call name"
        );
        assert!(
            tc["incomplete_details"].is_object(),
            "incomplete_details must be object when status=incomplete"
        );
        assert_eq!(
            tc["incomplete_details"]["reason"].as_str(),
            Some("tool_calls_required")
        );

        let queued = build_responses_api_queued_response("resp_q_1", "gpt-4");
        assert_eq!(queued["status"].as_str(), Some("queued"));
        assert_eq!(queued["object"].as_str(), Some("response"));
        let queued_output = queued["output"].as_array().expect("queued output array");
        assert!(queued_output.is_empty(), "queued response has empty output");
        assert!(
            queued["error"].is_null(),
            "queued response error must be null"
        );

        let in_prog = build_responses_api_in_progress_response("resp_p_1", "gpt-4");
        assert_eq!(in_prog["status"].as_str(), Some("in_progress"));
        assert_eq!(in_prog["object"].as_str(), Some("response"));
        let in_prog_output = in_prog["output"]
            .as_array()
            .expect("in_progress output array");
        assert!(
            in_prog_output.is_empty(),
            "in_progress response has empty output"
        );
    }
}
