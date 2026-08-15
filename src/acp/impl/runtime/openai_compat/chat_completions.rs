//! OpenAI Chat Completions compatibility.
//!
//! `/v1/chat/completions`-compatible endpoint handling: request/response
//! types, payload conversion to internal [`ChatParams`], and the SSE streaming
//! loop. Split out of the former monolithic `openai_compat.rs` (M0.4); the
//! Responses API lives in the sibling [`super::responses`] module.

use std::sync::Arc;

use anyhow::Result;
use serde::Deserialize;

use crate::acp::r#impl::chat::{ChatParams, ChatRequestContext, StreamObserver};
use crate::acp::r#impl::request::inject_platform_profiles_if_absent;
use crate::acp::r#impl::session::UserSession;
use crate::acp::server::AcpServer;
use crate::acp::transport::SseTransport;
use crate::agent::Message;
use crate::i18n::runtime::tf;

use super::super::http::{
    clone_tcp_stream, http_trace_context, write_http_json_response,
    write_http_json_response_with_context, HttpStream,
};
use super::super::sse::{write_openai_sse_data, write_openai_sse_done, write_sse_headers};

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

pub(crate) fn is_setup_or_upstream_unavailable(err: &anyhow::Error) -> bool {
    let msg = err.to_string().to_lowercase();
    msg.contains("missing environment variable")
        || msg.contains("error sending request")
        || msg.contains("connection refused")
        || msg.contains("timed out")
        || msg.contains("error.chat.agent_error_prefix")
        || msg.contains("error.chat.all_agents_failed")
}

pub(crate) fn degraded_openai_message(err: &anyhow::Error) -> String {
    // The default openai_compatible upstream is sourced from the provider
    // spec (single source) instead of a hard-coded port, so the hint matches
    // the actual default even if the spec changes.
    let default_upstream = crate::core::providers::provider_spec_by_name("openai_compatible")
        .and_then(|spec| spec.url.clone())
        .unwrap_or_else(|| crate::core::providers::DEFAULT_OPENAI_COMPAT_BASE.to_string());
    format!(
        "go-on is running, but upstream model service is unavailable. {}. Configure at least one reachable provider (for example set DEEPSEEK_API_KEY) or start your copilot-compatible upstream on {}.",
        err, default_upstream
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
        mode: "edit".to_string(),
        messages,
        conversation_id: None,
        branch_id: None,
        phase: None,
        options,
        vector_hits: None,
        model: None,
        temperature: None,
        max_tokens: None,
    }
}

pub(crate) async fn handle_openai_chat_completions(
    socket: &mut HttpStream,
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
    let request_id = format!("chatcmpl-{}", crate::shared::timestamps::now_ts_ms());
    let mut params = openai_to_chat_params(&openai_req);

    if !openai_req.stream {
        let trace = http_trace_context("openai.chat.completions");
        let ctx = Some(ChatRequestContext::new(user_session.clone()));
        // Add a 300-second timeout for the entire chat request pipeline.
        // The provider API call, keychain fallback, and agent selection
        // should complete well within this window. If it hangs (e.g.,
        // harness review gate, empty agent selection), the client gets
        // a clean error instead of hanging indefinitely.
        let result = match tokio::time::timeout(
            std::time::Duration::from_secs(crate::acp::r#impl::chat::CHAT_REQUEST_TIMEOUT_SECS),
            crate::acp::r#impl::chat::process_chat_request(
                server.as_ref(),
                &mut params,
                None,
                &trace,
                None,
                ctx,
            ),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => {
                let payload = serde_json::json!({
                    "error": {
                        "message": "chat request timed out after 300s",
                        "type": "go_on_timeout"
                    }
                });
                write_http_json_response_with_context(
                    socket,
                    504,
                    payload,
                    "openai.chat.completions",
                    cors_headers,
                )
                .await?;
                record_outcome(false);
                return Ok(());
            }
        };
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

    stream_openai_sse(
        socket,
        server,
        params,
        user_session,
        cors_headers,
        request_id,
        model,
        started,
    )
    .await
}

/// Stream the OpenAI-compatible SSE response.
///
/// Runs the receive loop inside a task-local transport scope (plain-TCP arm
/// only): permission requests and session updates emitted during chat
/// processing write to THIS connection's socket instead of a process-wide
/// global that concurrent connections overwrite. The TLS arm keeps the
/// documented pre-merge behavior (no out-of-band transport).
#[allow(clippy::too_many_arguments)]
async fn stream_openai_sse(
    socket: &mut HttpStream,
    server: Arc<AcpServer>,
    mut params: ChatParams,
    user_session: Option<UserSession>,
    cors_headers: &str,
    request_id: String,
    model: String,
    started: std::time::Instant,
) -> Result<()> {
    // Clone the Arc and copy the Instant into the outcome closure so the
    // original `server` can be moved into the async stream block below.
    let server_metrics = Arc::clone(&server);
    let record_outcome = move |success: bool| {
        server_metrics
            .observability
            .metrics
            .record_request_outcome(success, started.elapsed().as_millis() as f64);
    };

    write_sse_headers(socket, cors_headers).await?;
    // Out-of-band SSE transport requires a plain TCP stream (fd clone); on the
    // TLS arm no out-of-band transport is set.
    let out_of_band_transport = if let HttpStream::Plain(plain) = socket {
        Some(Arc::new(SseTransport::new(clone_tcp_stream(plain)?))
            as Arc<dyn crate::acp::transport::Transport>)
    } else {
        None
    };

    let stream_block = async move {
        // Periodic flush interval for SSE streaming — flushes every 4 events
        // to batch syscalls while keeping latency low (shared constant from
        // sse.rs, same pattern as http.rs).
        let mut sse_event_count: usize = 0;

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let trace = http_trace_context("openai.chat.completions.stream");
        let ctx = Some(ChatRequestContext::new(user_session));
        let server_ref = Arc::clone(&server);
        let task = tokio::spawn(async move {
            crate::acp::r#impl::chat::process_chat_request(
                server_ref.as_ref(),
                &mut params,
                Some(StreamObserver::sse(tx)),
                &trace,
                None,
                ctx,
            )
            .await
        });

        while let Some(frame) = rx.recv().await {
            if frame.event == "error" {
                // Forward error events from the chat pipeline
                let err_msg = frame
                    .payload
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown pipeline error");
                let payload = build_openai_chunk(
                    &request_id,
                    &model,
                    &format!("upstream model service error: {}", err_msg),
                    Some("stop"),
                );
                let _ = write_openai_sse_data(socket, &payload).await;
                write_openai_sse_done(socket).await?;
                record_outcome(false);
                task.abort();
                return Ok(());
            }
            // Forward "done" event: the full response has already been
            // streamed as per-token "chunk" deltas, so re-sending the whole
            // text here would duplicate the answer for any OpenAI-compatible
            // client that concatenates delta.content. Emit only an empty
            // delta carrying finish_reason="stop" to terminate the stream;
            // the "result" branch below handles responses that were never
            // chunked (e.g. a mode-runtime recovery with no chunk frames).
            if frame.event == "done" {
                let payload = build_openai_chunk(&request_id, &model, "", Some("stop"));
                let _ = write_openai_sse_data(socket, &payload).await;
                write_openai_sse_done(socket).await?;
                record_outcome(true);
                task.abort();
                return Ok(());
            }

            // Forward "result" event: the final pipeline result carries the complete
            // response text extracted from the agent output. Send it as the final
            // chunk with finish_reason="stop", then the SSE [DONE] signal.
            if frame.event == "result" {
                let response_text = frame
                    .payload
                    .get("response")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let payload = build_openai_chunk(&request_id, &model, response_text, Some("stop"));
                let _ = write_openai_sse_data(socket, &payload).await;
                write_openai_sse_done(socket).await?;
                record_outcome(true);
                task.abort();
                return Ok(());
            }

            // "telemetry" events are informational — ignore them.
            if frame.event == "telemetry" {
                continue;
            }

            // Only "chunk" events should proceed past this point.
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
            sse_event_count += 1;
            // Periodic flush: every SSE_FLUSH_INTERVAL events.
            // This batches syscalls while keeping latency low.
            if sse_event_count.is_multiple_of(super::super::sse::SSE_FLUSH_INTERVAL) {
                use super::super::sse::flush_sse;
                let _ = flush_sse(socket).await;
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
    };

    let stream_result: Result<(), anyhow::Error> = match out_of_band_transport {
        Some(transport) => crate::acp::transport::with_transport(transport, stream_block).await,
        None => stream_block.await,
    };
    stream_result?;
    Ok(())
}

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
        assert_eq!(params.mode, "edit");
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
}
