use super::*;
use crate::views::chat::types::{CommandRecord, SubAgentRecord};
use std::sync::mpsc::TrySendError;
use std::time::Duration;

/// The `__thinking__` token prefix emitted by the agent runtime to mark
/// reasoning/thinking content in streaming output.  The GUI strips these
/// markers from the display text and moves the reasoning into a dedicated
/// thinking panel (collapsible), matching Zed's chat UX.
const THINKING_MARKER: &str = "__thinking__";

/// Split content on `__thinking__` markers, returning (cleaned_content, thinking_text).
/// When markers are nested or repeated, thinking segments are concatenated.
fn split_thinking_from_content(raw: &str) -> (String, String) {
    if !raw.contains(THINKING_MARKER) {
        return (raw.to_string(), String::new());
    }
    let mut content = String::with_capacity(raw.len());
    let mut thinking = String::new();
    let mut in_thinking = false;
    let mut cursor = 0;
    for (end, marker) in raw.match_indices(THINKING_MARKER) {
        // Text before the marker
        let segment = &raw[cursor..end];
        if in_thinking {
            // Everything since last marker is thinking content
            thinking.push_str(segment);
        } else {
            content.push_str(segment);
        }
        in_thinking = !in_thinking;
        cursor = end + marker.len();
    }
    // Remaining text after last marker
    let remaining = &raw[cursor..];
    if in_thinking {
        thinking.push_str(remaining);
    } else {
        content.push_str(remaining);
    }
    (content, thinking)
}

/// Retry `tx.try_send(msg)` with exponential backoff when the channel is full.
/// Gives up immediately if the channel has been closed.
async fn try_send_with_retry<T>(tx: &mpsc::SyncSender<T>, mut msg: T) {
    let mut delay_ms = 5u64;
    loop {
        match tx.try_send(msg) {
            Ok(_) => return,
            Err(TrySendError::Full(ret)) => {
                msg = ret;
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                delay_ms = (delay_ms * 2).min(200);
            }
            Err(TrySendError::Disconnected(_)) => return,
        }
    }
}

/// Retry helper specifically for the `pending_tx` channel in this file.
/// Uses `&mpsc::SyncSender<PendingResponse>` to avoid repeated type annotations.
async fn send_pending(tx: &mpsc::SyncSender<PendingResponse>, msg: PendingResponse) {
    try_send_with_retry(tx, msg).await;
}

const MAX_INLINE_ATTACHMENT_B64_CHARS: usize = 8_192;
const MAX_BUFFERED_TOKENS_BYTES: usize = 256 * 1024; // 256 KB accumulated token buffer

/// RAII guard to decrement active_generations counter on drop.
/// Ensures counter is decremented even if async task exits early or panics.
struct ActiveGenerationGuard(Arc<std::sync::atomic::AtomicU64>);

impl Drop for ActiveGenerationGuard {
    fn drop(&mut self) {
        // NOTE: fetch_sub(1) wraps on 0 (unsigned overflow in release, panic in debug).
        // This would allow 18 quintillion concurrent generations if a double-free occurs.
        // Use fetch_update with saturating_sub to safely clamp at zero instead of wrapping.
        let _ = self.0.fetch_update(
            std::sync::atomic::Ordering::Relaxed,
            std::sync::atomic::Ordering::Relaxed,
            |v| Some(v.saturating_sub(1)),
        );
    }
}

impl ActiveGenerationGuard {
    fn new(counter: Arc<std::sync::atomic::AtomicU64>) -> Self {
        Self(counter)
    }
}

// ── Helper types for stream processing ──

/// Accumulated result from processing a streaming SSE response.
struct StreamResult {
    content: Option<String>,
    thinking: Option<String>,
    agent: Option<String>,
    used_model: Option<String>,
    conv_id: Option<String>,
    branch_id: Option<String>,
    /// Actual mode reported by the backend (from SSE done/result event).
    actual_mode: Option<String>,
    sse_errors: u32,
}

impl StreamResult {
    fn new() -> Self {
        Self {
            content: None,
            thinking: None,
            agent: None,
            used_model: None,
            conv_id: None,
            branch_id: None,
            actual_mode: None,
            sse_errors: 0,
        }
    }
}

// ── Stream-processing free functions ──

/// Parameters for building a chat request body.
struct ChatRequestBodyParams<'a> {
    use_workflow_rpc: bool,
    mode: &'a str,
    phase: &'a str,
    model: &'a str,
    history_messages: &'a [serde_json::Value],
    outbound_msg: &'a str,
    request_options: Option<Value>,
    conv_id: Option<String>,
    branch_id: Option<String>,
    selected_agent: &'a str,
}

/// Build the JSON request body for the chat stream endpoint.
/// Supports both standard chat mode and workflow RPC mode.
/// The `phase` field is included when non-empty so the backend can use
/// the selected phase instead of default/adaptive inference.
async fn build_chat_request_body(p: ChatRequestBodyParams<'_>) -> serde_json::Value {
    let ChatRequestBodyParams {
        use_workflow_rpc,
        mode,
        phase,
        model,
        history_messages,
        outbound_msg,
        request_options,
        conv_id,
        branch_id,
        selected_agent,
    } = p;
    let mut body = if use_workflow_rpc {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "workflow.ask",
            "params": {
                "task": outbound_msg,
                "auto_create_skills": true,
                "auto_create_workflow": true,
            }
        })
    } else {
        let mut body = serde_json::json!({
            "messages": history_messages,
            "mode": mode,
        });
        // Include phase so the backend uses the user-selected phase
        if !phase.is_empty() {
            body["phase"] = serde_json::json!(phase);
        }
        body
    };

    if !use_workflow_rpc {
        // Model selection logic
        if !model.trim().is_empty() && model != "auto" {
            body["options"] = serde_json::json!({"model": model});
        }

        if let Some(extra) = request_options.clone() {
            if body.get("options").is_none() {
                body["options"] = serde_json::json!({});
            }
            if let Some(obj) = extra.as_object() {
                for (k, v) in obj {
                    body["options"][k] = v.clone();
                }
            }
        }

        if let Some(cid) = conv_id {
            body["conversation_id"] = serde_json::json!(cid);
        }
        if let Some(bid) = branch_id {
            body["branch_id"] = serde_json::json!(bid);
        }

        // Always send preferred_agent when explicitly selected
        if !selected_agent.is_empty() {
            if let Some(serde_json::Value::Object(ref mut options_map)) = body.get_mut("options") {
                options_map.insert(
                    "preferred_agent".to_string(),
                    serde_json::Value::String(selected_agent.to_string()),
                );
            } else {
                body["options"] = serde_json::json!({"preferred_agent": selected_agent});
            }
        }
    }

    body
}

/// Handle the workflow RPC path (non-streaming). Returns `true` if the request
/// was handled as a workflow (caller should return early), `false` otherwise.
async fn handle_workflow_rpc(
    base_url: &str,
    body: &serde_json::Value,
    generation_id: u64,
    tx: &mpsc::SyncSender<PendingResponse>,
) -> bool {
    let workflow_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .read_timeout(std::time::Duration::from_secs(60))
        .build()
        .unwrap_or_else(|_| {
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(300))
                .read_timeout(std::time::Duration::from_secs(60))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new())
        });
    match workflow_client
        .post(format!("{}/rpc", base_url.trim_end_matches('/')))
        .json(body)
        .send()
        .await
    {
        Ok(resp) => {
            if let Ok(value) = resp.json::<serde_json::Value>().await {
                if value.get("error").is_some() {
                    let error_msg = value["error"]
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("unknown workflow error");
                    tracing::warn!(
                        "[Gen] Workflow generation {} returned error: {}",
                        generation_id,
                        error_msg
                    );
                    send_pending(
                        tx,
                        PendingResponse::Error {
                            generation_id: Some(generation_id),
                            message: format!("workflow.ask failed: {error_msg}"),
                        },
                    )
                    .await;
                } else {
                    let result_text =
                        serde_json::to_string_pretty(value.get("result").unwrap_or(&value))
                            .unwrap_or_default();
                    #[cfg(debug_assertions)]
                    eprintln!("[Gen] Workflow generation {} completed", generation_id);
                    send_pending(
                        tx,
                        PendingResponse::ChatCompleted {
                            generation_id,
                            content: result_text,
                            thinking: String::new(),
                            agent: "workflow".to_string(),
                            model: None,
                            conversation_id: None,
                            branch_id: None,
                            actual_mode: None,
                        },
                    )
                    .await;
                }
            } else {
                tracing::warn!(
                    "[Gen] Workflow generation {} - JSON parse failed",
                    generation_id
                );
                send_pending(
                    tx,
                    PendingResponse::Error {
                        generation_id: Some(generation_id),
                        message: "workflow response parse error".to_string(),
                    },
                )
                .await;
            }
        }
        Err(e) => {
            tracing::warn!("[Gen] Workflow generation {} failed: {}", generation_id, e);
            send_pending(
                tx,
                PendingResponse::Error {
                    generation_id: Some(generation_id),
                    message: format!("workflow.ask error: {e}"),
                },
            )
            .await;
        }
    }
    true
}

/// Attempt a fallback non-streaming chat request when the stream request fails
/// or returns a non-success HTTP status.
#[allow(clippy::too_many_arguments)]
async fn fallback_chat_request(
    backend: &BackendClient,
    outbound_msg: &str,
    mode: &str,
    phase: &str,
    model: &str,
    request_options: Option<Value>,
    abort_ctrl: &AbortController,
    generation_id: u64,
    tx: &mpsc::SyncSender<PendingResponse>,
    error_context: &str,
) {
    let fallback = backend
        .chat_with_options(
            outbound_msg,
            mode,
            phase,
            Some(model),
            request_options,
            None,
            Some(abort_ctrl.clone()),
        )
        .await
        .map(
            |(content, thinking, agent, selected_model)| PendingResponse::ChatCompleted {
                generation_id,
                content,
                thinking,
                agent,
                model: selected_model,
                conversation_id: None,
                branch_id: None,
                actual_mode: None,
            },
        )
        .unwrap_or_else(|e| PendingResponse::Error {
            generation_id: Some(generation_id),
            message: format!("{error_context}; fallback: {e}"),
        });
    send_pending(tx, fallback).await;
}

/// Process the SSE stream response: read chunks, parse events, buffer tokens,
/// and forward them as PendingResponse messages. Returns the accumulated result
/// (final content, thinking, agent info, etc.) after the stream ends.
async fn process_stream_events(
    mut resp: reqwest::Response,
    generation_id: u64,
    tx: &mpsc::SyncSender<PendingResponse>,
    stream_chunk_flush_interval: std::time::Duration,
    abort_ctrl: &AbortController,
) -> StreamResult {
    let mut result = StreamResult::new();
    let mut buffered_token = String::with_capacity(4096);
    let mut buffered_reasoning = String::with_capacity(2048);
    let mut last_stream_flush = std::time::Instant::now();
    let mut total_buffer_bytes = 0usize;

    // Use StreamProcessor as the single SSE parser for all GUI stream parsing.
    let mut processor = StreamProcessor::new();

    loop {
        let chunk = match resp.chunk().await {
            Ok(Some(c)) => c,
            Ok(None) => break,
            Err(e) => {
                send_pending(
                    tx,
                    PendingResponse::Error {
                        generation_id: Some(generation_id),
                        message: format!("read error: {e}"),
                    },
                )
                .await;
                return result;
            }
        };

        // Check for abort before processing the chunk
        if abort_ctrl.is_cancelled() {
            return result;
        }

        // Delegate SSE parsing to StreamProcessor
        let events = processor.push_chunk(&chunk);
        for event_result in events {
            match event_result {
                Ok(val) => {
                    // Handle [DONE] sentinel
                    if val.is_string() && val.as_str() == Some("[DONE]") {
                        break;
                    }
                    if val.get("data").and_then(|v| v.as_str()) == Some("[DONE]") {
                        break;
                    }

                    let event_type = val
                        .get("_event_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    match event_type {
                        "chunk" | "" => {
                            let token = val
                                .get("token")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string();
                            let reasoning = val
                                .get("reasoning")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string();

                            if !token.is_empty() || !reasoning.is_empty() {
                                let token_bytes = token.len();
                                let reasoning_bytes = reasoning.len();
                                total_buffer_bytes += token_bytes + reasoning_bytes;

                                // Accumulate into final_content as a safety net
                                if !token.is_empty() {
                                    result
                                        .content
                                        .get_or_insert_with(String::new)
                                        .push_str(&token);
                                }
                                buffered_token.push_str(&token);
                                buffered_reasoning.push_str(&reasoning);

                                // Force flush if buffer exceeds max accumulated size
                                if total_buffer_bytes > MAX_BUFFERED_TOKENS_BYTES {
                                    send_pending(
                                        tx,
                                        PendingResponse::StreamChunk {
                                            generation_id,
                                            token: std::mem::take(&mut buffered_token),
                                            reasoning: std::mem::take(&mut buffered_reasoning),
                                        },
                                    )
                                    .await;
                                    total_buffer_bytes = 0;
                                    last_stream_flush = std::time::Instant::now();
                                }
                            }
                        }
                        "telemetry" => {
                            if let Some(te) = val.get("token_economy") {
                                let input_tokens =
                                    te.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0)
                                        as usize;
                                let output_tokens =
                                    te.get("output_tokens")
                                        .and_then(|v| v.as_u64())
                                        .unwrap_or(0) as usize;
                                let total_tokens =
                                    te.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(0)
                                        as usize;
                                send_pending(
                                    tx,
                                    PendingResponse::TokenEconomy {
                                        generation_id,
                                        input_tokens,
                                        output_tokens,
                                        total_tokens,
                                    },
                                )
                                .await;
                            }
                        }
                        "sub_agent" => {
                            let agent = val
                                .get("agent")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let action = val
                                .get("action")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let status = val
                                .get("status")
                                .and_then(|v| v.as_str())
                                .unwrap_or("running")
                                .to_string();
                            let input = val
                                .get("input")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let output = val
                                .get("output")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            send_pending(
                                tx,
                                PendingResponse::SubAgentEvent {
                                    generation_id,
                                    agent,
                                    action,
                                    status,
                                    input,
                                    output,
                                },
                            )
                            .await;
                        }
                        "tool_approval" => {
                            let tool_name = val
                                .get("tool_name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let tool_args = val
                                .get("tool_args")
                                .cloned()
                                .unwrap_or(serde_json::Value::Null);
                            let mode = val
                                .get("mode")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let risk_score = val
                                .get("risk_score")
                                .and_then(|v| v.as_f64())
                                .unwrap_or(0.0);
                            let message = val
                                .get("message")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            send_pending(
                                tx,
                                PendingResponse::ToolApprovalRequest {
                                    generation_id,
                                    tool_name,
                                    tool_args,
                                    mode,
                                    risk_score,
                                    message,
                                },
                            )
                            .await;
                        }
                        "command" => {
                            let command_str = val
                                .get("command")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let working_dir = val
                                .get("working_dir")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let exit_code =
                                val.get("exit_code").and_then(|v| v.as_i64()).unwrap_or(-1) as i32;
                            let stdout = val
                                .get("stdout")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let stderr = val
                                .get("stderr")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let duration_ms =
                                val.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(0);
                            send_pending(
                                tx,
                                PendingResponse::CommandOutput {
                                    generation_id,
                                    command: command_str,
                                    working_dir,
                                    exit_code,
                                    stdout,
                                    stderr,
                                    duration_ms,
                                },
                            )
                            .await;
                        }
                        "result" | "done" => {
                            let new_content = val
                                .get("response")
                                .or_else(|| val.get("content"))
                                .and_then(|v| v.as_str())
                                .map(ToOwned::to_owned);
                            if let Some(ref c) = new_content {
                                if !c.trim().is_empty() {
                                    result.content = Some(c.clone());
                                }
                            }
                            result.thinking = val
                                .get("thinking")
                                .and_then(|v| v.as_str())
                                .map(ToOwned::to_owned);
                            let new_agent = val
                                .get("agent")
                                .or_else(|| val.get("selected_agent"))
                                .or_else(|| val.pointer("/capability_routing/selected_agent"))
                                .and_then(|v| v.as_str())
                                .map(String::from);
                            if let Some(ref a) = new_agent {
                                if !a.trim().is_empty() {
                                    result.agent = Some(a.clone());
                                }
                            }
                            result.used_model = val
                                .get("selected_model")
                                .and_then(|v| v.as_str())
                                .map(String::from);
                            result.conv_id = val
                                .get("conversation_id")
                                .and_then(|v| v.as_str())
                                .map(String::from);
                            result.branch_id = val
                                .get("branch_id")
                                .and_then(|v| v.as_str())
                                .map(String::from);
                            result.actual_mode =
                                val.get("mode").and_then(|v| v.as_str()).map(String::from);
                            if val.get("plan_output").is_some() {
                                #[cfg(debug_assertions)]
                                eprintln!("[Plan] Captured plan_output");
                            }
                        }
                        "error" => {
                            if !buffered_token.is_empty() || !buffered_reasoning.is_empty() {
                                send_pending(
                                    tx,
                                    PendingResponse::StreamChunk {
                                        generation_id,
                                        token: std::mem::take(&mut buffered_token),
                                        reasoning: std::mem::take(&mut buffered_reasoning),
                                    },
                                )
                                .await;
                            }
                            let message = val
                                .get("message")
                                .or_else(|| val.get("error"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| {
                                    format!(
                                        "stream error: {}",
                                        serde_json::to_string(&val).unwrap_or_default()
                                    )
                                });

                            tracing::warn!(
                                "[Gen] SSE error event for generation {}: {}",
                                generation_id,
                                message
                            );

                            send_pending(
                                tx,
                                PendingResponse::Error {
                                    generation_id: Some(generation_id),
                                    message,
                                },
                            )
                            .await;
                            return result;
                        }
                        _ => {
                            #[cfg(debug_assertions)]
                            eprintln!("[SSE] Unknown event type: {}", event_type);
                            result.sse_errors = result.sse_errors.saturating_add(1);
                        }
                    }

                    // Time-based flush to maintain responsive UI
                    if (!buffered_token.is_empty() || !buffered_reasoning.is_empty())
                        && last_stream_flush.elapsed() >= stream_chunk_flush_interval
                    {
                        send_pending(
                            tx,
                            PendingResponse::StreamChunk {
                                generation_id,
                                token: std::mem::take(&mut buffered_token),
                                reasoning: std::mem::take(&mut buffered_reasoning),
                            },
                        )
                        .await;
                        last_stream_flush = std::time::Instant::now();
                        total_buffer_bytes = 0;
                    }
                }
                Err(e) => {
                    tracing::warn!("[SSE] Parse error: {}", e);
                    result.sse_errors = result.sse_errors.saturating_add(1);
                }
            }
        }
    }

    // Flush any remaining buffered content
    if !buffered_token.is_empty() || !buffered_reasoning.is_empty() {
        send_pending(
            tx,
            PendingResponse::StreamChunk {
                generation_id,
                token: buffered_token,
                reasoning: buffered_reasoning,
            },
        )
        .await;
    }

    result
}

/// Finalize the chat response after stream processing: emit SSE parse error
/// warnings, and send either ChatCompleted or an empty-response Error.
async fn finalize_stream_result(
    result: StreamResult,
    generation_id: u64,
    tx: &mpsc::SyncSender<PendingResponse>,
) {
    #[cfg(debug_assertions)]
    {
        let _status_log = format!(
            "tokens:{}, thinking:{}, agent:{}",
            result.content.as_ref().map(|c| c.len()).unwrap_or(0),
            !result
                .thinking
                .as_ref()
                .map(|t| t.is_empty())
                .unwrap_or(true),
            result.agent.as_deref().unwrap_or("unknown")
        );
        eprintln!(
            "[Gen] Generation {} completed ({})",
            generation_id, _status_log
        );
    }

    // Emit SSE parse error summary warning if any errors occurred
    if result.sse_errors > 0 {
        let warn_msg = format!(
            "[SSE] {} JSON parse error(s) occurred during streaming",
            result.sse_errors
        );
        tracing::warn!("{}", warn_msg);
        send_pending(tx, PendingResponse::UiMessage(warn_msg)).await;
    }

    let content_empty = result.content.as_ref().is_none_or(|c| c.is_empty());
    let agent_empty = result.agent.as_ref().is_none_or(|a| a.is_empty());
    if content_empty && agent_empty {
        send_pending(
            tx,
            PendingResponse::Error {
                generation_id: Some(generation_id),
                message: "The chat stream ended without producing a response.\n\
Possible causes:\n\
  • No agents are configured for the current phase\n\
  • API keys are missing or expired\n\
  • Backend is overloaded or unreachable\n\
\
Check the backend log and agent configuration."
                    .to_string(),
            },
        )
        .await;
    } else {
        send_pending(
            tx,
            PendingResponse::ChatCompleted {
                generation_id,
                content: result.content.unwrap_or_default(),
                thinking: result.thinking.unwrap_or_default(),
                agent: result.agent.unwrap_or_default(),
                model: result.used_model,
                conversation_id: result.conv_id,
                branch_id: result.branch_id,
                actual_mode: result.actual_mode,
            },
        )
        .await;
    }
}

impl ChatView {
    /// Zed-style: append a segment to the segments list, merging with the last
    /// segment of the same type to keep contiguous content together.
    fn append_segment(
        segments: &mut Vec<crate::views::chat::types::MessageSegment>,
        new_seg: crate::views::chat::types::MessageSegment,
    ) {
        use crate::views::chat::types::MessageSegment;
        let new_type = std::mem::discriminant(&new_seg);
        if let Some(last) = segments.last_mut() {
            if std::mem::discriminant(last) == new_type {
                // Same type — merge into last segment
                match (last, new_seg) {
                    (MessageSegment::Content(ref mut t), MessageSegment::Content(add)) => {
                        t.push_str(&add)
                    }
                    (MessageSegment::Thinking(ref mut t), MessageSegment::Thinking(add)) => {
                        t.push_str(&add)
                    }
                    _ => {}
                }
                return;
            }
        }
        segments.push(new_seg);
    }

    fn build_attachment_summary(attachments: &[Attachment]) -> String {
        if attachments.is_empty() {
            return String::new();
        }

        let details = attachments
            .iter()
            .map(|a| {
                if a.data.len() <= MAX_INLINE_ATTACHMENT_B64_CHARS {
                    format!("- {} ({}) [base64:{} chars]", a.name, a.mime, a.data.len())
                } else {
                    format!(
                        "- {} ({}) [base64:{} chars, truncated]",
                        a.name,
                        a.mime,
                        a.data.len()
                    )
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        format!("\n\n[Attachments]\n{details}")
    }

    fn merge_options_with_tracking(
        options_extra: Option<Value>,
        conversation_id: Option<&str>,
        branch_id: Option<&str>,
    ) -> Option<Value> {
        let mut merged = options_extra
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();

        if let Some(conv_id) = conversation_id {
            merged.insert(
                "conversation_id".to_string(),
                Value::String(conv_id.to_string()),
            );
        }
        if let Some(b_id) = branch_id {
            merged.insert("branch_id".to_string(), Value::String(b_id.to_string()));
        }

        if merged.is_empty() {
            None
        } else {
            Some(Value::Object(merged))
        }
    }

    /// ── Sub-function 1: Prepare request parameters ──
    /// Validates input, expands prompt commands, validates phase against
    /// known backend phases, and runs SafeGuard risk checks.
    /// Returns (expanded_msg, mode, phase, base_url, autotune_extra).
    fn prepare_chat_request(
        &mut self,
        msg: &str,
        autotune_chain_enabled: bool,
        backend: &BackendClient,
    ) -> (String, String, String, String, Option<Value>) {
        let expanded_msg = self.expand_prompt_command_with_fallback(
            msg,
            Some(&self.template_state.prompts_command_templates),
        );
        let mode = self.selected_mode.clone();
        // Validate phase before sending — if the phase is not in the known list
        // fetched from backend, default to empty (backend will use its default phase).
        let phase = if self.phases_loaded
            && !self.selected_phase.is_empty()
            && !self.phases.contains(&self.selected_phase)
        {
            #[cfg(debug_assertions)]
            eprintln!(
                "chat: phase '{}' not in known phases {:?}, resetting to empty",
                self.selected_phase, self.phases
            );
            String::new()
        } else {
            self.selected_phase.clone()
        };
        let base_url = backend.base_url().to_string();
        let autotune_extra = if autotune_chain_enabled {
            Some(AutoTuneView::load_runtime_options())
        } else {
            None
        };

        // ── Mode-aware pre-send check ──
        // For SafeGuard mode, pre-compute risk score
        if self.mode_policy.mode == "safeguard" {
            let risk = self.mode_policy.compute_risk_score(&expanded_msg);
            if risk > 0.7 {
                self.risk_is_high = true;
                self.risk_review_required = true;
                self.risk_strategy = "safeguard_review".to_string();
                self.risk_reasons = format!(
                    "Risk score: {:.2} — high-risk operation detected. SafeGuard mode requires confirmation.",
                    risk
                );
            } else {
                self.risk_is_high = false;
                self.risk_review_required = false;
                self.risk_strategy.clear();
                self.risk_reasons.clear();
            }
        }

        (expanded_msg, mode, phase, base_url, autotune_extra)
    }

    /// ── Sub-function 2 (pre-spawn part): Update UI panels before sending ──
    /// Clears the input, takes attachments, builds the outbound message,
    /// pushes the user message to the session, and sets AI status to Thinking.
    /// Returns (comparison_id, outbound_msg).
    fn update_ui_panels_before_spawn(
        &mut self,
        expanded_msg: &str,
        phase: &str,
        _backend: &BackendClient,
    ) -> (u64, String) {
        self.input.clear();
        let now = crate::fs_util::epoch_secs();
        let atts = std::mem::take(&mut self.attachments);

        // F-GAP-66: Integrate MultimodalProcessor for attachment processing.
        // Currently attachments are only included as text summary in the outbound message.
        // When backend supports multimodal content parts (OpenAI-style format with
        // content array containing {type, text|image_url|image_data} parts), the
        // attachment data should be injected as image_url parts:
        //   "content": [{ "type": "text", "text": "..." },
        //                { "type": "image_url", "image_url": { "url": "data:{mime};base64,{data}" }}]
        let attachment_summary = Self::build_attachment_summary(&atts);
        let outbound_msg = format!("{expanded_msg}{attachment_summary}");

        // Sync the current UI mode into the session before saving
        if let Some(session) = self
            .session_state
            .sessions
            .get_mut(self.session_state.active_session)
        {
            session.mode = self.selected_mode.clone();
        }

        // Add user message immediately
        self.session().push_message(Message {
            role: "user".to_string(),
            content: expanded_msg.to_string(),
            timestamp: now,
            attachments: atts,
            model: String::new(),
            comparison_id: 0,
            input_tokens: Self::estimate_tokens_improved(expanded_msg),
            output_tokens: 0,
            total_tokens: 0,
            thinking: String::new(),
            segments: Vec::new(),
            sub_agent_records: Vec::new(),
            command_records: Vec::new(),
        });
        self.save_sessions_to_disk();

        self.last_token_estimate = 0;
        self.input_token_estimate = Self::estimate_tokens_improved(expanded_msg);
        self.output_token_estimate = 0;

        // Add a "running" phase record
        let now_ts = crate::fs_util::epoch_secs();
        let running_phase = if phase.is_empty() { "think" } else { phase };
        self.session().phase_records.push(PhaseRecord {
            phase: running_phase.to_string(),
            agent: String::new(),
            status: "running".to_string(),
            timestamp: now_ts,
        });

        self.ai_status = AiStatus::Thinking;
        self.sending = true;
        self.error.clear();
        self.stop_requested = false;

        (now, outbound_msg)
    }

    /// ── Sub-function 5 (post-spawn part): Check post-spawn state ──
    /// If no generation was started, reverts the sending and ai_status.
    fn check_post_spawn_state(&mut self, existing_generations: usize) {
        if self.generation_states.len() == existing_generations {
            // Nothing started.
            self.sending = false;
            self.ai_status = AiStatus::Error;
            self.set_phase_record_status("error");
        }
    }

    /// Send a message asynchronously via the backend.
    pub fn send_message(
        &mut self,
        backend: &BackendClient,
        ctx: &egui::Context,
        autotune_chain_enabled: bool,
    ) {
        let msg = self.input.trim().to_string();
        if msg.is_empty() || self.sending {
            return;
        }

        // Reset per-turn tool call counter for max_tool_calls enforcement
        self.turn_tool_calls = 0;

        // Set generation deadline to auto-recover from hung tasks
        // 300s = HTTP client timeout; +30s grace for task cleanup
        self.generation_deadline =
            Some(std::time::Instant::now() + std::time::Duration::from_secs(330));

        // ── 1. Prepare request parameters ──
        let (expanded_msg, mode, phase, base_url, autotune_extra) =
            self.prepare_chat_request(&msg, autotune_chain_enabled, backend);

        // ── 2. Update UI panels before spawning ──
        let (comparison_id, outbound_msg) =
            self.update_ui_panels_before_spawn(&expanded_msg, &phase, backend);

        // ── 3. Set up generation slots and spawn the async task ──
        self.sync_model_selection();

        let stream_chunk_flush_interval = self.stream_state.stream_chunk_flush_interval;
        let stream_client = self.stream_state.stream_client.clone();
        let active_gen_count = self.active_generations.clone();
        let existing_generations = self.generation_states.len();

        let model_name = self.model_state.selected_model.clone();
        {
            // Reserve one generation slot atomically for this model.
            let previous = active_gen_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let current_gen_count = previous + 1;
            if current_gen_count > MAX_CONCURRENT_GENERATIONS as u64 {
                active_gen_count.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                self.error = format!("Too many concurrent generations ({})", current_gen_count);
                return;
            }

            let generation_id = self.next_generation_id();
            let input_tokens = self.input_token_estimate;

            #[cfg(debug_assertions)]
            eprintln!(
                "[Gen] Started generation {} for model '{}' (active: {}/{})",
                generation_id, model_name, current_gen_count, MAX_CONCURRENT_GENERATIONS
            );
            self.session().push_message(Message {
                role: "assistant".to_string(),
                content: String::new(),
                timestamp: comparison_id,
                attachments: Vec::new(),
                model: model_name.clone(),
                comparison_id,
                input_tokens,
                output_tokens: 0,
                total_tokens: 0,
                thinking: String::new(),
                segments: Vec::new(),
                sub_agent_records: Vec::new(),
                command_records: Vec::new(),
            });
            let msg_idx = self.session().messages.len().saturating_sub(1);

            let tx = self.stream_state.pending_tx.clone();
            let backend_clone = backend.clone();
            let mode_clone = mode.clone();
            let phase_clone = phase.clone();
            let model_clone = model_name.clone();
            let base_url_clone = base_url.clone();
            // Build full conversation history (excluding empty assistant placeholders)
            let history_messages: Vec<serde_json::Value> = self.session_state.sessions
                [self.session_state.active_session]
                .messages
                .iter()
                .filter(|m| !m.content.is_empty() || m.role != "assistant")
                .take(50)
                .map(|m| serde_json::json!({ "role": m.role, "content": m.content }))
                .collect();
            let conv_id_clone = self.session_state.sessions[self.session_state.active_session]
                .conversation_id
                .clone();
            let branch_id_clone = self.session_state.sessions[self.session_state.active_session]
                .branch_id
                .clone();
            let selected_agent_clone = self.model_state.selected_agent.trim().to_string();
            let outbound_clone = outbound_msg;
            let request_options = Self::merge_options_with_tracking(
                autotune_extra.clone(),
                conv_id_clone.as_deref(),
                branch_id_clone.as_deref(),
            );
            // Always send preferred_agent when the user has explicitly selected an agent,
            // regardless of model selection (auto vs specific). The model and agent
            // are independent concerns — a user may want auto model selection but a
            // specific provider/agent.
            let request_options = if !selected_agent_clone.is_empty() {
                if let Some(serde_json::Value::Object(mut options_map)) = request_options {
                    options_map.insert(
                        "preferred_agent".to_string(),
                        serde_json::Value::String(selected_agent_clone.clone()),
                    );
                    Some(serde_json::Value::Object(options_map))
                } else {
                    Some(serde_json::json!({
                        "preferred_agent": selected_agent_clone.clone(),
                    }))
                }
            } else {
                request_options
            };
            // Create and store an abort controller for this generation
            let abort_ctrl = AbortController::new();
            self.stream_state.abort_controller = Some(abort_ctrl.clone());
            self.stream_state.stream_progress = TokenProgress::default();
            let active_gen_guard = ActiveGenerationGuard::new(active_gen_count.clone());
            let sc = stream_client.clone();
            let abort_ctrl_task = abort_ctrl.clone();
            let handle = tokio::spawn(async move {
                // Guard ensures active_generations is decremented when this task exits
                let _guard = active_gen_guard;

                let use_workflow_rpc = mode_clone == "workflow";

                // Build request body using the extracted helper
                let body = build_chat_request_body(ChatRequestBodyParams {
                    use_workflow_rpc,
                    mode: &mode_clone,
                    phase: &phase_clone,
                    model: &model_clone,
                    history_messages: &history_messages,
                    outbound_msg: &outbound_clone,
                    request_options: request_options.clone(),
                    conv_id: conv_id_clone.clone(),
                    branch_id: branch_id_clone.clone(),
                    selected_agent: &selected_agent_clone,
                })
                .await;

                // Handle workflow mode via RPC (non-streaming) — return early
                if use_workflow_rpc {
                    handle_workflow_rpc(&base_url_clone, &body, generation_id, &tx).await;
                    return;
                }

                // Send stream request to /chat/stream endpoint
                let endpoint = format!("{}/chat/stream", base_url_clone.trim_end_matches('/'));
                let stream_resp = sc.post(&endpoint).json(&body).send().await;

                match stream_resp {
                    Ok(resp) => {
                        let status = resp.status();
                        if !status.is_success() {
                            // Non-success HTTP status — try fallback non-streaming request
                            let err_body = resp.text().await.unwrap_or_default();
                            let err_msg = if err_body.is_empty() {
                                format!(
                                    "HTTP {} {}",
                                    status.as_u16(),
                                    status.canonical_reason().unwrap_or("Unknown")
                                )
                            } else {
                                let truncated = if err_body.len() > 500 {
                                    format!("{}...", &err_body[..500])
                                } else {
                                    err_body
                                };
                                format!("HTTP {}: {}", status.as_u16(), truncated)
                            };
                            fallback_chat_request(
                                &backend_clone,
                                &outbound_clone,
                                &mode_clone,
                                &phase_clone,
                                &model_clone,
                                request_options.clone(),
                                &abort_ctrl_task,
                                generation_id,
                                &tx,
                                &format!("stream error: {err_msg}"),
                            )
                            .await;
                            return;
                        }

                        // Process the SSE stream events
                        let result = process_stream_events(
                            resp,
                            generation_id,
                            &tx,
                            stream_chunk_flush_interval,
                            &abort_ctrl_task,
                        )
                        .await;

                        // Finalize and send completion/error
                        finalize_stream_result(result, generation_id, &tx).await;
                    }
                    Err(err) => {
                        tracing::warn!(
                            "[Gen] Generation {} stream request failed (attempting fallback): {:?}",
                            generation_id,
                            err
                        );
                        fallback_chat_request(
                            &backend_clone,
                            &outbound_clone,
                            &mode_clone,
                            &phase_clone,
                            &model_clone,
                            request_options.clone(),
                            &abort_ctrl_task,
                            generation_id,
                            &tx,
                            &format!("request error: {err}"),
                        )
                        .await;
                    }
                }
            });
            self.generation_states.push(GenerationState {
                id: generation_id,
                msg_idx,
                model: model_name,
                started_at: std::time::Instant::now(),
                handle,
            });
        }

        self.check_post_spawn_state(existing_generations);

        self.save_sessions_to_disk();
        // Trigger immediate repaint to show the placeholder message.
        ctx.request_repaint();
    }

    /// Drain any pending async responses and update the session / `ai_status`.
    ///
    /// # Event drain strategy
    /// Uses `while let Ok(...)` to drain ALL pending events without a fixed cap.
    /// This prevents silent event drops (previously: fixed `max_pending_events_per_frame` cap).
    /// `ctx.request_repaint()` is called every ~5 events to amortize repaint cost
    /// while keeping UI responsive under high token throughput.
    pub(super) fn process_pending(&mut self, i18n: &I18n, ctx: &egui::Context) {
        // ── Generation deadline guard ──
        // If a generation is stuck and exceeds the 330s deadline, force-reset
        // sending to prevent permanent UI lock (the user can retry).
        if self.sending {
            if let Some(deadline) = self.generation_deadline {
                if deadline.elapsed().as_secs() > 330 {
                    tracing::warn!(
                        "Generation deadline exceeded (330s) — force-resetting sending state"
                    );
                    self.sending = false;
                    self.ai_status = AiStatus::Error;
                    self.generation_deadline = None;
                    // Reset the active generation counter to unblock future sends
                    self.active_generations
                        .store(0, std::sync::atomic::Ordering::Relaxed);
                }
            }
        }

        let mut had_events = false;
        let mut event_count = 0u32;
        loop {
            let Ok(pending) = self.stream_state.pending_rx.try_recv() else {
                break;
            };
            had_events = true;
            event_count += 1;
            match pending {
                PendingResponse::Phases(list) => {
                    self.phases = list;
                    self.phases_loaded = true;
                }
                PendingResponse::Models(agent_models) => {
                    // Keep structured map for two-level picker
                    self.model_state.available_agent_models = agent_models;
                    // Build flattened list for backward compat
                    let mut flat = Vec::new();
                    for ids in self.model_state.available_agent_models.values() {
                        flat.extend(ids.iter().cloned());
                    }
                    flat.sort();
                    flat.dedup();
                    self.model_state.available_models =
                        if self.model_state.available_agent_models.is_empty() {
                            vec!["auto".to_string()]
                        } else {
                            flat
                        };
                    // Only mark as loaded when models are actually present.
                    // On startup the backend may not be ready yet — the first
                    // fetch can return empty because the inner RPC uses a 500ms
                    // timeout.  Keeping models_loaded = false lets the 3-second
                    // retry poll in show() re-fetch once the backend is online.
                    self.model_state.models_loaded =
                        !self.model_state.available_agent_models.is_empty();
                    #[cfg(debug_assertions)]
                    if self.model_state.models_loaded {
                        eprintln!(
                            "[DEBUG] Models loaded: {} agents, {} models: {:?}",
                            self.model_state.available_agent_models.len(),
                            self.model_state.available_models.len(),
                            self.model_state
                                .available_agent_models
                                .keys()
                                .collect::<Vec<_>>()
                        );
                    }
                    // Preserve copilot-auto selection even though the backend
                    // does not report it as a model — it is a sentinel that tells
                    // the GUI to defer model selection to the Copilot service.
                    if self.model_state.selected_model != ChatView::COPILOT_AUTO_MODEL
                        && !self
                            .model_state
                            .available_models
                            .iter()
                            .any(|m| m == &self.model_state.selected_model)
                    {
                        self.model_state.selected_model = "auto".to_string();
                    }
                }
                PendingResponse::StreamChunk {
                    generation_id,
                    token,
                    reasoning,
                } => {
                    // Update streaming progress counters
                    if !token.is_empty() {
                        self.stream_state.stream_progress.tokens_received += 1;
                        self.stream_state.stream_progress.bytes_processed += token.len();
                    }

                    if let Some(idx) = self.generation_msg_idx(generation_id) {
                        if let Some(session) = self
                            .session_state
                            .sessions
                            .get_mut(self.session_state.active_session)
                        {
                            if let Some(m) = session.messages.get_mut(idx) {
                                if !token.is_empty() {
                                    // Strip __thinking__ markers from stream tokens
                                    let (clean_token, extra_thinking) =
                                        split_thinking_from_content(&token);
                                    if !clean_token.is_empty() {
                                        m.content.push_str(&clean_token);
                                        // Zed-style: append to last Content segment or add new
                                        Self::append_segment(
                                            &mut m.segments,
                                            crate::views::chat::types::MessageSegment::Content(
                                                clean_token,
                                            ),
                                        );
                                    }
                                    if !extra_thinking.is_empty() {
                                        if m.thinking.is_empty() {
                                            self.show_thinking_idx = Some(idx);
                                        }
                                        m.thinking.push_str(&extra_thinking);
                                        Self::append_segment(
                                            &mut m.segments,
                                            crate::views::chat::types::MessageSegment::Thinking(
                                                extra_thinking,
                                            ),
                                        );
                                    }
                                }
                                if !reasoning.is_empty() {
                                    if m.thinking.is_empty() {
                                        self.show_thinking_idx = Some(idx);
                                    }
                                    m.thinking.push_str(&reasoning);
                                    // Zed-style: append to last Thinking segment or add new
                                    Self::append_segment(
                                        &mut m.segments,
                                        crate::views::chat::types::MessageSegment::Thinking(
                                            reasoning,
                                        ),
                                    );
                                }
                            }
                        }
                    }
                }
                PendingResponse::TokenEconomy {
                    generation_id,
                    input_tokens,
                    output_tokens,
                    total_tokens,
                } => {
                    if let Some(idx) = self.generation_msg_idx(generation_id) {
                        if let Some(session) = self
                            .session_state
                            .sessions
                            .get_mut(self.session_state.active_session)
                        {
                            if let Some(m) = session.messages.get_mut(idx) {
                                m.input_tokens = input_tokens;
                                m.output_tokens = output_tokens;
                                m.total_tokens = total_tokens;
                            }
                        }
                    }
                    self.input_token_estimate = input_tokens;
                    self.output_token_estimate = output_tokens;
                    self.last_token_estimate = total_tokens;
                    // Sync stream progress with telemetry data
                    self.stream_state.stream_progress.output_tokens = output_tokens;
                    self.stream_state.stream_progress.total_tokens = total_tokens;
                }
                PendingResponse::ChatCompleted {
                    generation_id,
                    content,
                    thinking,
                    agent,
                    model,
                    conversation_id,
                    branch_id,
                    actual_mode,
                } => {
                    // Store conversation tracking IDs on the session
                    if let Some(conv_id) = conversation_id {
                        if let Some(session) = self
                            .session_state
                            .sessions
                            .get_mut(self.session_state.active_session)
                        {
                            session.conversation_id = Some(conv_id);
                        }
                    }
                    if let Some(b_id) = branch_id {
                        if let Some(session) = self
                            .session_state
                            .sessions
                            .get_mut(self.session_state.active_session)
                        {
                            session.branch_id = Some(b_id);
                        }
                    }

                    if !agent.is_empty() {
                        self.model_state.last_selected_agent = agent.clone();
                    }

                    let generation_meta = self.generation_meta(generation_id);
                    let mut model_name = None;
                    let mut output_tokens_to_record = self.output_token_estimate;
                    let mut is_sandbox_denial = false;
                    if let Some(idx) = self.generation_msg_idx(generation_id) {
                        if let Some(session) = self
                            .session_state
                            .sessions
                            .get_mut(self.session_state.active_session)
                        {
                            if let Some(m) = session.messages.get_mut(idx) {
                                if !content.is_empty() {
                                    // Strip any remaining __thinking__ markers from the final content
                                    let (clean_content, extra_thinking) =
                                        split_thinking_from_content(&content);
                                    m.content = clean_content;
                                    // Merge extra_thinking (from __thinking__ markers in final content)
                                    // with the authoritative thinking from the SSE done event.
                                    // If the done event has thinking, it takes precedence;
                                    // otherwise use the extracted extra_thinking.
                                    if !thinking.is_empty() {
                                        m.thinking.clone_from(&thinking);
                                    } else if !extra_thinking.is_empty() {
                                        m.thinking = extra_thinking;
                                    }
                                    if !m.thinking.is_empty() && self.show_thinking_idx.is_none() {
                                        self.show_thinking_idx = Some(idx);
                                    }
                                }

                                // Check for sandbox denial in message content
                                is_sandbox_denial = !m.content.is_empty() && {
                                    let lower = m.content.to_lowercase();
                                    lower.contains("not in sandbox whitelist")
                                        && lower.contains("requires user confirmation")
                                };
                                // Auto-collapse thinking when the response is complete.
                                // Only the first chunk of new thinking shows expanded;
                                // once ChatCompleted arrives, collapse it (Zed-style).
                                // This runs regardless of whether thinking came via
                                // stream chunks or the final ChatCompleted event.
                                if self.show_thinking_idx == Some(idx) {
                                    self.show_thinking_idx = None;
                                }
                                // Update the model used.
                                //   - Copilot auto → actual model name (e.g. "gemini-2.5-pro")
                                //   - Other agents → fall back to agent name
                                if let Some(ref used_model) = model {
                                    if !used_model.is_empty() {
                                        m.model = used_model.clone();
                                        session.model = used_model.clone();
                                    }
                                } else if !agent.is_empty() {
                                    m.model = agent.clone();
                                    session.model = agent.clone();
                                }
                                // Sync session.mode from backend's actual_mode if provided
                                // This keeps the UI mode selector in sync with what the
                                // backend actually used (e.g. capability routing may
                                // override the requested mode).
                                if let Some(ref actual_mode) = actual_mode {
                                    if !actual_mode.is_empty() && *actual_mode != self.selected_mode
                                    {
                                        self.selected_mode = actual_mode.clone();
                                        self.mode_policy = ModePolicy::new(actual_mode);
                                        session.mode = actual_mode.clone();
                                    }
                                }
                                if self.last_token_estimate == 0 {
                                    Self::estimate_tokens_improved(&m.content);
                                    self.last_token_estimate =
                                        self.input_token_estimate + self.output_token_estimate;
                                }
                                m.output_tokens = self.output_token_estimate.max(m.output_tokens);
                                m.total_tokens = self.last_token_estimate.max(m.total_tokens);
                                output_tokens_to_record = m.output_tokens;
                                model_name = Some(m.model.clone());
                            }
                        }
                    }

                    // Detect sandbox denial in completed message content
                    if is_sandbox_denial {
                        if let Some(idx) = self.generation_msg_idx(generation_id) {
                            let msg_content = self
                                .session_state
                                .sessions
                                .get(self.session_state.active_session)
                                .and_then(|s| s.messages.get(idx))
                                .map(|m| m.content.clone())
                                .unwrap_or_default();
                            let tool_name =
                                msg_content.split('\'').nth(1).unwrap_or("").to_string();
                            if !tool_name.is_empty() {
                                // Wire: filter by allowed_tools
                                if !self.mode_policy.allowed_tools.is_empty()
                                    && !self.mode_policy.allowed_tools.contains(&tool_name)
                                {
                                    self.error = format!(
                                        "🚫 Tool '{}' is not allowed in '{}' mode.",
                                        tool_name, self.mode_policy.mode
                                    );
                                    self.ai_status = AiStatus::Error;
                                }
                                // Wire: enforce max_tool_calls per turn
                                else {
                                    self.turn_tool_calls += 1;
                                    if self.turn_tool_calls > self.mode_policy.max_tool_calls {
                                        self.error = format!(
                                            "🛑 Tool call limit reached ({}/{}). Cannot execute '{}'.",
                                            self.turn_tool_calls,
                                            self.mode_policy.max_tool_calls,
                                            tool_name
                                        );
                                        self.ai_status = AiStatus::Error;
                                    } else {
                                        let last_user_idx = self
                                            .session_state
                                            .sessions
                                            .get(self.session_state.active_session)
                                            .and_then(|s| {
                                                s.messages.iter().rposition(|m| m.role == "user")
                                            })
                                            .unwrap_or(0);
                                        self.pending_tool_approval =
                                            Some((tool_name.clone(), 0.5, last_user_idx));
                                        self.error = format!(
                                            "💡 Tool '{}' requires your approval. Click Approve above or Deny to block it.",
                                            tool_name
                                        );
                                        self.ai_status = AiStatus::Error;
                                    }
                                }
                            }
                        }
                    }

                    if let Some((_, state_model, started_at)) = generation_meta {
                        let duration_ms = started_at.elapsed().as_millis() as u64;
                        self.update_model_stats(
                            model_name.as_deref().unwrap_or(&state_model),
                            output_tokens_to_record,
                            duration_ms,
                        );
                    }

                    self.set_phase_record_status("completed");

                    // Auto-name the session from first user message if still default
                    let first_user_content = self
                        .session()
                        .messages
                        .iter()
                        .find(|m| m.role == "user")
                        .map(|m| m.content.chars().take(25).collect::<String>());
                    if let Some(content) = first_user_content {
                        let is_default = Self::is_default_session_name(&self.session().name, i18n);
                        if is_default {
                            self.session().name = content;
                        }
                    }

                    self.remove_generation(generation_id);
                    self.stop_requested = false;
                    self.save_sessions_to_disk();
                    // Reset streaming progress on completion
                    self.stream_state.stream_progress = TokenProgress::default();
                    self.stream_state.abort_controller = None;
                }
                PendingResponse::Error {
                    generation_id,
                    message,
                } => {
                    // Provide user-friendly hints for common/generic errors
                    // that lack actionable details (e.g., HTTP/2 "stream error"
                    // from reqwest/hyper when the provider API call fails).
                    let enhanced_message = {
                        let lower = message.to_lowercase();
                        if lower.contains("unknown stream error")
                            || (lower.contains("stream error") && lower.contains("unknown"))
                        {
                            let hint = i18n.t("chat.error.apiKeyHint").replace(
                                "{key_env}",
                                "DEEPSEEK_API_KEY (or the appropriate env var)",
                            );
                            format!("{} \n\n💡 {}", message, hint)
                        } else if lower.contains("401")
                            || lower.contains("unauthorized")
                            || lower.contains("authentication")
                            || lower.contains("invalid api key")
                            || lower.contains("invalid_api_key")
                        {
                            let hint = i18n.t("chat.error.authHint");
                            format!("{} \n\n💡 {}", message, hint)
                        } else if lower.contains("402")
                            || lower.contains("insufficient_quota")
                            || lower.contains("rate limit")
                        {
                            let hint = i18n.t("chat.error.quotaHint");
                            format!("{} \n\n💡 {}", message, hint)
                        } else if lower.contains("not in sandbox whitelist")
                            && lower.contains("requires user confirmation")
                        {
                            // Sandbox denial — extract tool name for approval buttons
                            // Format: "tool 'agent-world-connector' is not in sandbox whitelist..."
                            let tool_name = message.split('\'').nth(1).unwrap_or("").to_string();
                            if !tool_name.is_empty() {
                                // Wire: filter by allowed_tools
                                if !self.mode_policy.allowed_tools.is_empty()
                                    && !self.mode_policy.allowed_tools.contains(&tool_name)
                                {
                                    let hint = i18n
                                        .t("chat.error.toolNotAllowed")
                                        .replace("{tool_name}", &tool_name)
                                        .replace("{mode}", &self.mode_policy.mode);
                                    format!("{} \n\n🚫 {}", message, hint)
                                // Wire: enforce max_tool_calls per turn
                                } else {
                                    self.turn_tool_calls += 1;
                                    if self.turn_tool_calls > self.mode_policy.max_tool_calls {
                                        let hint = i18n
                                            .t("chat.error.toolCallLimit")
                                            .replace("{tool_name}", &tool_name)
                                            .replace("{current}", &self.turn_tool_calls.to_string())
                                            .replace(
                                                "{max}",
                                                &self.mode_policy.max_tool_calls.to_string(),
                                            );
                                        format!("{} \n\n🛑 {}", message, hint)
                                    } else {
                                        // Find the last user message index
                                        let last_user_idx = self
                                            .session_state
                                            .sessions
                                            .get(self.session_state.active_session)
                                            .and_then(|session| {
                                                session
                                                    .messages
                                                    .iter()
                                                    .rposition(|m| m.role == "user")
                                            })
                                            .unwrap_or(0);
                                        self.pending_tool_approval =
                                            Some((tool_name.clone(), 0.5, last_user_idx));
                                        let hint = i18n
                                            .t("chat.error.toolApproval")
                                            .replace("{tool_name}", &tool_name);
                                        format!("{} \n\n💡 {}", message, hint)
                                    }
                                }
                            } else {
                                message
                            }
                        } else {
                            message
                        }
                    };
                    self.error = i18n
                        .t("chat.chatError")
                        .replace("{message}", &enhanced_message);
                    if let Some(id) = generation_id {
                        if let Some((_, model, _)) = self.generation_meta(id) {
                            let stats = self.model_state.model_stats.entry(model).or_default();
                            stats.error_count = stats.error_count.saturating_add(1);
                        }
                    }
                    self.set_phase_record_status("error");

                    // Drop empty placeholder assistant message on failure.
                    // Also handle generation_id=None: remove the most recent empty
                    // assistant message as a fallback cleanup.
                    if let Some(idx) = generation_id.and_then(|id| self.generation_msg_idx(id)) {
                        let should_remove = self
                            .session_state
                            .sessions
                            .get(self.session_state.active_session)
                            .map(|session| {
                                idx < session.messages.len()
                                    && session.messages[idx].content.is_empty()
                            })
                            .unwrap_or(false);
                        if should_remove {
                            self.remove_message_at(idx);
                        }
                    } else if generation_id.is_none() {
                        // Fallback: remove the most recent empty assistant message
                        if let Some(session) = self
                            .session_state
                            .sessions
                            .get_mut(self.session_state.active_session)
                        {
                            let empty_pos = session
                                .messages
                                .iter()
                                .rposition(|m| m.role == "assistant" && m.content.is_empty());
                            if let Some(pos) = empty_pos {
                                session.messages.remove(pos);
                            }
                        }
                    }

                    if let Some(id) = generation_id {
                        self.remove_generation(id);
                    }
                    self.sending = !self.generation_states.is_empty();
                    self.ai_status = AiStatus::Error;
                    self.stop_requested = false;
                    // Reset streaming progress on error
                    self.stream_state.stream_progress = TokenProgress::default();
                    self.stream_state.abort_controller = None;
                }
                PendingResponse::SubAgentEvent {
                    generation_id,
                    agent,
                    action,
                    status,
                    input,
                    output,
                } => {
                    if let Some(idx) = self.generation_msg_idx(generation_id) {
                        if let Some(session) = self
                            .session_state
                            .sessions
                            .get_mut(self.session_state.active_session)
                        {
                            if let Some(m) = session.messages.get_mut(idx) {
                                // Push a new sub-agent record to the message
                                m.sub_agent_records.push(SubAgentRecord {
                                    agent_name: agent,
                                    action,
                                    status,
                                    input,
                                    output,
                                    tool_calls: Vec::new(),
                                    started_at: std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_secs(),
                                    duration_ms: 0,
                                });
                            }
                        }
                    }
                }
                PendingResponse::CommandOutput {
                    generation_id,
                    command,
                    working_dir,
                    exit_code,
                    stdout,
                    stderr,
                    duration_ms,
                } => {
                    if let Some(idx) = self.generation_msg_idx(generation_id) {
                        if let Some(session) = self
                            .session_state
                            .sessions
                            .get_mut(self.session_state.active_session)
                        {
                            if let Some(m) = session.messages.get_mut(idx) {
                                m.command_records.push(CommandRecord {
                                    command,
                                    working_dir,
                                    exit_code,
                                    stdout,
                                    stderr,
                                    duration_ms,
                                });
                            }
                        }
                    }
                }
                PendingResponse::ToolApprovalRequest {
                    generation_id,
                    tool_name,
                    tool_args,
                    mode,
                    risk_score,
                    message,
                } => {
                    // Wire: filter by allowed_tools
                    if !self.mode_policy.allowed_tools.is_empty()
                        && !self.mode_policy.allowed_tools.contains(&tool_name)
                    {
                        self.error = format!(
                            "🚫 Tool '{}' is not allowed in '{}' mode.",
                            tool_name, self.mode_policy.mode
                        );
                        self.ai_status = AiStatus::Error;
                    } else {
                        self.turn_tool_calls += 1;
                        if self.turn_tool_calls > self.mode_policy.max_tool_calls {
                            self.error = format!(
                                "🛑 Tool call limit reached ({}/{}). Cannot execute '{}'.",
                                self.turn_tool_calls, self.mode_policy.max_tool_calls, tool_name
                            );
                            self.ai_status = AiStatus::Error;
                        } else {
                            let last_user_idx = self
                                .session_state
                                .sessions
                                .get(self.session_state.active_session)
                                .and_then(|session| {
                                    session.messages.iter().rposition(|m| m.role == "user")
                                })
                                .unwrap_or(0);
                            // Wire: display risk score in the tool approval UI
                            let _ = (generation_id, tool_args, mode);
                            self.pending_tool_approval =
                                Some((tool_name.clone(), risk_score, last_user_idx));
                            self.error = format!(
                                "💡 Tool '{}' requires your approval. {}",
                                tool_name, message
                            );
                            self.ai_status = AiStatus::Error;
                        }
                    }
                }
                PendingResponse::UiMessage(msg) => {
                    self.success_message = Some(msg);
                }
                PendingResponse::ExternalEditorResult(content) => {
                    self.input = content;
                }
            }

            // Request repaint every ~5 events to keep UI responsive
            // under high token throughput without excessive per-event overhead.
            if event_count.is_multiple_of(5) {
                ctx.request_repaint();
            }
        }
        if had_events {
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_attachment_summary_omits_raw_payload() {
        let attachments = vec![Attachment {
            name: "image.png".to_string(),
            mime: "image/png".to_string(),
            data: "abcd".repeat(16),
        }];

        let summary = ChatView::build_attachment_summary(&attachments);

        assert!(summary.contains("[Attachments]"));
        assert!(summary.contains("image.png"));
        assert!(summary.contains("base64:"));
        assert!(!summary.contains("abcdabcdabcd"));
    }

    #[test]
    fn merge_options_with_tracking_keeps_existing_and_adds_ids() {
        let merged = ChatView::merge_options_with_tracking(
            Some(serde_json::json!({"temperature": 0.2})),
            Some("conv-1"),
            Some("main"),
        )
        .expect("merged options expected");

        assert_eq!(merged["temperature"], serde_json::json!(0.2));
        assert_eq!(merged["conversation_id"], serde_json::json!("conv-1"));
        assert_eq!(merged["branch_id"], serde_json::json!("main"));
    }
}
