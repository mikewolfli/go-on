use super::*;
use crate::views::chat::types::{CommandRecord, SubAgentRecord};
use std::sync::mpsc::TrySendError;
use std::time::Duration;

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

impl ChatView {
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

        // Concurrent generation limit is enforced atomically below via fetch_add.
        // Do NOT check before fetch_add — that would be a TOCTOU race.
        let expanded_msg =
            self.expand_prompt_command_with_fallback(&msg, Some(&self.prompts_command_templates));
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

        // Add user message immediately
        self.session().push_message(Message {
            role: "user".to_string(),
            content: expanded_msg.clone(),
            timestamp: now,
            attachments: atts,
            model: String::new(),
            comparison_id: 0,
            input_tokens: Self::estimate_tokens_improved(&expanded_msg),
            output_tokens: 0,
            total_tokens: 0,
            thinking: String::new(),
            sub_agent_records: Vec::new(),
            command_records: Vec::new(),
        });
        self.save_sessions_to_disk();

        self.last_token_estimate = 0;
        self.input_token_estimate = Self::estimate_tokens_improved(&expanded_msg);
        self.output_token_estimate = 0;

        // Add a "running" phase record
        let now_ts = crate::fs_util::epoch_secs();
        let running_phase = if phase.is_empty() { "think" } else { &phase };
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

        self.sync_model_selection();

        let comparison_id = now;
        let stream_chunk_flush_interval = self.stream_chunk_flush_interval;
        let stream_client = self.stream_client.clone();
        let active_gen_count = self.active_generations.clone();
        let existing_generations = self.generation_states.len();

        let model_name = self.selected_model.clone();
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
                timestamp: now,
                attachments: Vec::new(),
                model: model_name.clone(),
                comparison_id,
                input_tokens,
                output_tokens: 0,
                total_tokens: 0,
                thinking: String::new(),
                sub_agent_records: Vec::new(),
                command_records: Vec::new(),
            });
            let msg_idx = self.session().messages.len().saturating_sub(1);

            let tx = self.pending_tx.clone();
            let backend_clone = backend.clone();
            let mode_clone = mode.clone();
            let phase_clone = phase.clone();
            let outbound_clone = outbound_msg.clone();
            let model_clone = model_name.clone();
            let base_url_clone = base_url.clone();
            // Build full conversation history (excluding empty assistant placeholders)
            let history_messages: Vec<serde_json::Value> = self.sessions[self.active_session]
                .messages
                .iter()
                .filter(|m| !m.content.is_empty() || m.role != "assistant")
                .take(50)
                .map(|m| serde_json::json!({ "role": m.role, "content": m.content }))
                .collect();
            let conv_id_clone = self.sessions[self.active_session].conversation_id.clone();
            let branch_id_clone = self.sessions[self.active_session].branch_id.clone();
            let selected_agent_clone = self.selected_agent.trim().to_string();
            let mut request_options = Self::merge_options_with_tracking(
                autotune_extra.clone(),
                conv_id_clone.as_deref(),
                branch_id_clone.as_deref(),
            );
            // Always send preferred_agent when the user has explicitly selected an agent,
            // regardless of model selection (auto vs specific). The model and agent
            // are independent concerns — a user may want auto model selection but a
            // specific provider/agent.
            if !selected_agent_clone.is_empty() {
                if let Some(serde_json::Value::Object(ref mut options_map)) = request_options {
                    options_map.insert(
                        "preferred_agent".to_string(),
                        serde_json::Value::String(selected_agent_clone.clone()),
                    );
                } else {
                    request_options = Some(serde_json::json!({
                        "preferred_agent": selected_agent_clone,
                    }));
                }
            }
            // Create and store an abort controller for this generation
            let abort_ctrl = AbortController::new();
            self.abort_controller = Some(abort_ctrl.clone());
            self.stream_processor = Some(StreamProcessor::new());
            self.stream_progress = TokenProgress::default();
            let active_gen_guard = ActiveGenerationGuard::new(active_gen_count.clone());
            let sc = stream_client.clone();
            let abort_ctrl_task = abort_ctrl.clone();
            let handle = tokio::spawn(async move {
                // Guard ensures active_generations is decremented when this task exits
                let _guard = active_gen_guard;

                let _phase_val = if phase_clone.is_empty() {
                    serde_json::Value::Null
                } else {
                    serde_json::Value::String(phase_clone.clone())
                };

                let use_workflow_rpc = mode_clone == "workflow";

                let mut body = if use_workflow_rpc {
                    serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": 1,
                        "method": "workflow.ask",
                        "params": {
                            "task": outbound_clone.clone(),
                            "auto_create_skills": true,
                            "auto_create_workflow": true,
                        }
                    })
                } else {
                    serde_json::json!({
                        "messages": history_messages,
                        "mode": mode_clone,
                    })
                };

                if !use_workflow_rpc {
                    // ── Model selection ──
                    //   * "auto"         → backend picks the default phase model.
                    //   * "copilot-auto" → backend routes to copilot agent only,
                    //                      Copilot service auto-selects model.
                    //   * any other ID   → explicit model override sent to backend.
                    if !model_clone.trim().is_empty() && model_clone != "auto" {
                        body["options"] = serde_json::json!({
                            "model": model_clone,
                        });
                    }

                    if let Some(extra) = request_options.clone() {
                        if body.get("options").is_none() {
                            body["options"] = serde_json::json!({});
                        }
                        // Flatten extra values into options, NOT under "extra" key
                        if let Some(obj) = extra.as_object() {
                            for (k, v) in obj {
                                body["options"][k] = v.clone();
                            }
                        }
                    }

                    if let Some(conv_id) = conv_id_clone.clone() {
                        body["conversation_id"] = serde_json::json!(conv_id);
                    }
                    if let Some(b_id) = branch_id_clone.clone() {
                        body["branch_id"] = serde_json::json!(b_id);
                    }
                }

                // Handle workflow mode via RPC (non-streaming) — return early
                if use_workflow_rpc {
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
                        .post(format!("{}/rpc", base_url_clone.trim_end_matches('/')))
                        .json(&body)
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            if let Ok(value) = resp.json::<serde_json::Value>().await {
                                // Check for JSON-RPC error response
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
                                        &tx,
                                        PendingResponse::Error {
                                            generation_id: Some(generation_id),
                                            message: format!("workflow.ask failed: {error_msg}"),
                                        },
                                    )
                                    .await;
                                } else {
                                    let result_text = serde_json::to_string_pretty(
                                        value.get("result").unwrap_or(&value),
                                    )
                                    .unwrap_or_default();
                                    #[cfg(debug_assertions)]
                                    eprintln!(
                                        "[Gen] Workflow generation {} completed",
                                        generation_id
                                    );
                                    send_pending(
                                        &tx,
                                        PendingResponse::ChatCompleted {
                                            generation_id,
                                            content: result_text,
                                            thinking: String::new(),
                                            agent: "workflow".to_string(),
                                            model: None,
                                            conversation_id: None,
                                            branch_id: None,
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
                                    &tx,
                                    PendingResponse::Error {
                                        generation_id: Some(generation_id),
                                        message: "workflow response parse error".to_string(),
                                    },
                                )
                                .await;
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                "[Gen] Workflow generation {} failed: {}",
                                generation_id,
                                e
                            );
                            send_pending(
                                &tx,
                                PendingResponse::Error {
                                    generation_id: Some(generation_id),
                                    message: format!("workflow.ask error: {e}"),
                                },
                            )
                            .await;
                        }
                    }
                    return; // Skip the normal chat/stream flow
                }

                // Always use /chat/stream (native Go-On endpoint) which directly accepts
                // ChatParams format (mode, phase, options, preferred_agent, etc.).
                // /v1/chat/completions (OpenAI compat) only works with OpenAI-format bodies
                // and will crash on GUI-specific fields.
                let endpoint = format!("{}/chat/stream", base_url_clone.trim_end_matches('/'));
                let stream_resp = sc.post(&endpoint).json(&body).send().await;

                match stream_resp {
                    Ok(resp) => {
                        let status = resp.status();
                        let mut resp = if !status.is_success() {
                            // Capture the response body for better diagnostics
                            let err_body = resp.text().await.unwrap_or_default();
                            let err_msg = if err_body.is_empty() {
                                format!(
                                    "HTTP {} {}",
                                    status.as_u16(),
                                    status.canonical_reason().unwrap_or("Unknown")
                                )
                            } else {
                                // Truncate long error bodies
                                let truncated = if err_body.len() > 500 {
                                    format!("{}...", &err_body[..500])
                                } else {
                                    err_body
                                };
                                format!("HTTP {}: {}", status.as_u16(), truncated)
                            };
                            let fallback = backend_clone
                                .chat_with_options(
                                    &outbound_clone,
                                    &mode_clone,
                                    &phase_clone,
                                    Some(&model_clone),
                                    request_options.clone(),
                                    None, // history not available in this scope
                                    Some(abort_ctrl_task.clone()),
                                )
                                .await
                                .map(|(content, thinking, agent, selected_model)| {
                                    PendingResponse::ChatCompleted {
                                        generation_id,
                                        content,
                                        thinking,
                                        agent,
                                        model: selected_model,
                                        conversation_id: None,
                                        branch_id: None,
                                    }
                                })
                                .unwrap_or_else(|e| PendingResponse::Error {
                                    generation_id: Some(generation_id),
                                    message: format!("stream error: {err_msg}; fallback: {e}"),
                                });
                            send_pending(&tx, fallback).await;
                            return;
                        } else {
                            resp
                        };

                        let mut final_content: Option<String> = None;
                        let mut final_thinking: Option<String> = None;
                        let mut final_agent: Option<String> = None;
                        let mut final_used_model: Option<String> = None;
                        let mut final_conv_id: Option<String> = None;
                        let mut final_branch_id: Option<String> = None;
                        let mut buffered_token = String::with_capacity(4096);
                        let mut buffered_reasoning = String::with_capacity(2048);
                        let mut last_stream_flush = std::time::Instant::now();
                        let mut total_buffer_bytes = 0usize;
                        let mut sse_parse_error_count = 0u32;

                        // Use StreamProcessor as the single SSE parser for all GUI stream parsing.
                        // This replaces the previous inline frame splitting and JSON parsing.
                        let mut processor = StreamProcessor::new();

                        loop {
                            let chunk = match resp.chunk().await {
                                Ok(Some(c)) => c,
                                Ok(None) => break,
                                Err(e) => {
                                    send_pending(
                                        &tx,
                                        PendingResponse::Error {
                                            generation_id: Some(generation_id),
                                            message: format!("read error: {e}"),
                                        },
                                    )
                                    .await;
                                    return;
                                }
                            };

                            // Check for abort before processing the chunk
                            if abort_ctrl_task.is_cancelled() {
                                return;
                            }

                            // Delegate SSE parsing to StreamProcessor, which handles
                            // frame splitting, CRLF normalization, JSON parsing, and
                            // event type injection via the "_event_type" field.
                            let events = processor.push_chunk(&chunk);
                            for event_result in events {
                                match event_result {
                                    Ok(val) => {
                                        // Handle [DONE] sentinel (used by non-streaming fallback)
                                        if val.is_string() && val.as_str() == Some("[DONE]") {
                                            break;
                                        }
                                        if val.get("data").and_then(|v| v.as_str())
                                            == Some("[DONE]")
                                        {
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
                                                    // Track buffer usage and flush if needed
                                                    let token_bytes = token.len();
                                                    let reasoning_bytes = reasoning.len();
                                                    total_buffer_bytes +=
                                                        token_bytes + reasoning_bytes;

                                                    buffered_token.push_str(&token);
                                                    buffered_reasoning.push_str(&reasoning);

                                                    // Force flush if buffer exceeds max accumulated size
                                                    if total_buffer_bytes
                                                        > MAX_BUFFERED_TOKENS_BYTES
                                                    {
                                                        send_pending(
                                                            &tx,
                                                            PendingResponse::StreamChunk {
                                                                generation_id,
                                                                token: std::mem::take(
                                                                    &mut buffered_token,
                                                                ),
                                                                reasoning: std::mem::take(
                                                                    &mut buffered_reasoning,
                                                                ),
                                                            },
                                                        )
                                                        .await;
                                                        total_buffer_bytes = 0;
                                                        last_stream_flush =
                                                            std::time::Instant::now();
                                                    }
                                                }
                                            }
                                            "telemetry" => {
                                                if let Some(te) = val.get("token_economy") {
                                                    let input_tokens = te
                                                        .get("input_tokens")
                                                        .and_then(|v| v.as_u64())
                                                        .unwrap_or(0)
                                                        as usize;
                                                    let output_tokens = te
                                                        .get("output_tokens")
                                                        .and_then(|v| v.as_u64())
                                                        .unwrap_or(0)
                                                        as usize;
                                                    let total_tokens = te
                                                        .get("total_tokens")
                                                        .and_then(|v| v.as_u64())
                                                        .unwrap_or(0)
                                                        as usize;
                                                    send_pending(
                                                        &tx,
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
                                                    &tx,
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
                                                let exit_code = val
                                                    .get("exit_code")
                                                    .and_then(|v| v.as_i64())
                                                    .unwrap_or(-1)
                                                    as i32;
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
                                                let duration_ms = val
                                                    .get("duration_ms")
                                                    .and_then(|v| v.as_u64())
                                                    .unwrap_or(0);
                                                send_pending(
                                                    &tx,
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
                                                final_content = val
                                                    .get("response")
                                                    .or_else(|| val.get("content"))
                                                    .and_then(|v| v.as_str())
                                                    .map(ToOwned::to_owned);
                                                final_thinking = val
                                                    .get("thinking")
                                                    .and_then(|v| v.as_str())
                                                    .map(ToOwned::to_owned);
                                                final_agent = val
                                                    .get("agent")
                                                    .or_else(|| val.get("selected_agent"))
                                                    .or_else(|| {
                                                        val.pointer(
                                                            "/capability_routing/selected_agent",
                                                        )
                                                    })
                                                    .and_then(|v| v.as_str())
                                                    .map(String::from);
                                                final_used_model = val
                                                    .get("selected_model")
                                                    .and_then(|v| v.as_str())
                                                    .map(String::from);
                                                final_conv_id = val
                                                    .get("conversation_id")
                                                    .and_then(|v| v.as_str())
                                                    .map(String::from);
                                                final_branch_id = val
                                                    .get("branch_id")
                                                    .and_then(|v| v.as_str())
                                                    .map(String::from);
                                                // Capture plan_output from Plan mode responses
                                                if let Some(plan_output) = val.get("plan_output") {
                                                    if let Some(obj) = plan_output.as_object() {
                                                        // Plan output is available for the UI to display
                                                        // the structured plan steps and recommended mode
                                                        #[cfg(debug_assertions)]
                                                        {
                                                            eprintln!("[Plan] Captured plan output: {} mode, {} step(s)",
                                                                obj.get("recommended_mode").and_then(|v| v.as_str()).unwrap_or("?"),
                                                                obj.get("steps").and_then(|a| a.as_array()).map(|a| a.len()).unwrap_or(0)
                                                            );
                                                        }
                                                    }
                                                }
                                            }
                                            "error" => {
                                                if !buffered_token.is_empty()
                                                    || !buffered_reasoning.is_empty()
                                                {
                                                    send_pending(
                                                        &tx,
                                                        PendingResponse::StreamChunk {
                                                            generation_id,
                                                            token: std::mem::take(
                                                                &mut buffered_token,
                                                            ),
                                                            reasoning: std::mem::take(
                                                                &mut buffered_reasoning,
                                                            ),
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
                                                        // Fallback: serialize the whole payload for debugging
                                                        format!(
                                                            "stream error: {}",
                                                            serde_json::to_string(&val)
                                                                .unwrap_or_default()
                                                        )
                                                    });
                                                send_pending(
                                                    &tx,
                                                    PendingResponse::Error {
                                                        generation_id: Some(generation_id),
                                                        message,
                                                    },
                                                )
                                                .await;
                                                return;
                                            }
                                            _ => {
                                                #[cfg(debug_assertions)]
                                                eprintln!(
                                                    "[SSE] Unknown event type: {}",
                                                    event_type
                                                );
                                                sse_parse_error_count =
                                                    sse_parse_error_count.saturating_add(1);
                                            }
                                        }

                                        // Time-based flush to maintain responsive UI
                                        if (!buffered_token.is_empty()
                                            || !buffered_reasoning.is_empty())
                                            && last_stream_flush.elapsed()
                                                >= stream_chunk_flush_interval
                                        {
                                            send_pending(
                                                &tx,
                                                PendingResponse::StreamChunk {
                                                    generation_id,
                                                    token: std::mem::take(&mut buffered_token),
                                                    reasoning: std::mem::take(
                                                        &mut buffered_reasoning,
                                                    ),
                                                },
                                            )
                                            .await;
                                            last_stream_flush = std::time::Instant::now();
                                            total_buffer_bytes = 0;
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!("[SSE] Parse error: {}", e);
                                        sse_parse_error_count =
                                            sse_parse_error_count.saturating_add(1);
                                    }
                                }
                            }
                        }

                        // Flush any remaining buffered content
                        if !buffered_token.is_empty() || !buffered_reasoning.is_empty() {
                            send_pending(
                                &tx,
                                PendingResponse::StreamChunk {
                                    generation_id,
                                    token: buffered_token,
                                    reasoning: buffered_reasoning,
                                },
                            )
                            .await;
                        }

                        let _status_log = format!(
                            "tokens:{}, thinking:{}, agent:{}",
                            final_content.as_ref().map(|c| c.len()).unwrap_or(0),
                            !final_thinking
                                .as_ref()
                                .map(|t| t.is_empty())
                                .unwrap_or(true),
                            final_agent.as_deref().unwrap_or("unknown")
                        );
                        #[cfg(debug_assertions)]
                        eprintln!(
                            "[Gen] Generation {} completed ({})",
                            generation_id, _status_log
                        );

                        // Emit SSE parse error summary warning if any errors occurred
                        if sse_parse_error_count > 0 {
                            let warn_msg = format!(
                                "[SSE] {} JSON parse error(s) occurred during streaming",
                                sse_parse_error_count
                            );
                            tracing::warn!("{}", warn_msg);
                            send_pending(&tx, PendingResponse::UiMessage(warn_msg)).await;
                        }

                        let is_empty = final_content.as_ref().is_none_or(|c| c.is_empty());
                        if is_empty && !final_agent.as_ref().is_some_and(|a| a.is_empty()) {
                            // Response was empty even after successful streaming.
                            // This happens when the backend sends a "done" event without
                            // a "response" field. Send an error so the user sees feedback.
                            send_pending(
                                &tx,
                                PendingResponse::Error {
                                    generation_id: Some(generation_id),
                                    message: "The model returned an empty response.
The backend may be misconfigured or overloaded."
                                        .to_string(),
                                },
                            )
                            .await;
                        } else {
                            send_pending(
                                &tx,
                                PendingResponse::ChatCompleted {
                                    generation_id,
                                    content: final_content.unwrap_or_default(),
                                    thinking: final_thinking.unwrap_or_default(),
                                    agent: final_agent.unwrap_or_default(),
                                    model: final_used_model,
                                    conversation_id: final_conv_id,
                                    branch_id: final_branch_id,
                                },
                            )
                            .await;
                        }
                    }
                    Err(err) => {
                        tracing::warn!(
                            "[Gen] Generation {} stream request failed (attempting fallback): {}",
                            generation_id,
                            err
                        );
                        let fallback = backend_clone
                            .chat_with_options(
                                &outbound_clone,
                                &mode_clone,
                                &phase_clone,
                                Some(&model_clone),
                                request_options.clone(),
                                None, // history not available in this scope
                                Some(abort_ctrl_task.clone()),
                            )
                            .await
                            .map(|(content, thinking, agent, selected_model)| {
                                PendingResponse::ChatCompleted {
                                    generation_id,
                                    content,
                                    thinking,
                                    agent,
                                    model: selected_model,
                                    conversation_id: None,
                                    branch_id: None,
                                }
                            })
                            .unwrap_or_else(|e| PendingResponse::Error {
                                generation_id: Some(generation_id),
                                message: format!("request error: {err}; fallback: {e}"),
                            });
                        send_pending(&tx, fallback).await;
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

        if self.generation_states.len() == existing_generations {
            // Nothing started.
            self.sending = false;
            self.ai_status = AiStatus::Error;
            self.set_phase_record_status("error");
        }

        self.save_sessions_to_disk();
        // Trigger immediate repaint to show the placeholder message.
        ctx.request_repaint();
    }

    /// Drain any pending async responses and update the session / `ai_status`.
    pub(super) fn process_pending(&mut self, i18n: &I18n, ctx: &egui::Context) {
        let mut had_events = false;
        for _ in 0..self.max_pending_events_per_frame {
            let Ok(pending) = self.pending_rx.try_recv() else {
                break;
            };
            had_events = true;
            match pending {
                PendingResponse::Phases(list) => {
                    self.phases = list;
                    self.phases_loaded = true;
                }
                PendingResponse::Models(agent_models) => {
                    // Keep structured map for two-level picker
                    self.available_agent_models = agent_models;
                    // Build flattened list for backward compat
                    let mut flat = Vec::new();
                    for ids in self.available_agent_models.values() {
                        flat.extend(ids.iter().cloned());
                    }
                    flat.sort();
                    flat.dedup();
                    self.available_models = if self.available_agent_models.is_empty() {
                        vec!["auto".to_string()]
                    } else {
                        flat
                    };
                    self.models_loaded = true;
                    // Preserve copilot-auto selection even though the backend
                    // does not report it as a model — it is a sentinel that tells
                    // the GUI to defer model selection to the Copilot service.
                    if self.selected_model != ChatView::COPILOT_AUTO_MODEL
                        && !self
                            .available_models
                            .iter()
                            .any(|m| m == &self.selected_model)
                    {
                        self.selected_model = "auto".to_string();
                    }
                }
                PendingResponse::StreamChunk {
                    generation_id,
                    token,
                    reasoning,
                } => {
                    // Update streaming progress counters
                    if !token.is_empty() {
                        self.stream_progress.tokens_received += 1;
                        self.stream_progress.bytes_processed += token.len();
                    }

                    if let Some(idx) = self.generation_msg_idx(generation_id) {
                        if let Some(session) = self.sessions.get_mut(self.active_session) {
                            if let Some(m) = session.messages.get_mut(idx) {
                                if !token.is_empty() {
                                    m.content.push_str(&token);
                                }
                                if !reasoning.is_empty() {
                                    // First reasoning token -> auto-expand thinking panel
                                    if m.thinking.is_empty() {
                                        self.show_thinking_idx = Some(idx);
                                    }
                                    m.thinking.push_str(&reasoning);
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
                        if let Some(session) = self.sessions.get_mut(self.active_session) {
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
                    self.stream_progress.output_tokens = output_tokens;
                    self.stream_progress.total_tokens = total_tokens;
                }
                PendingResponse::ChatCompleted {
                    generation_id,
                    content,
                    thinking,
                    agent,
                    model,
                    conversation_id,
                    branch_id,
                } => {
                    // Store conversation tracking IDs on the session
                    if let Some(conv_id) = conversation_id {
                        if let Some(session) = self.sessions.get_mut(self.active_session) {
                            session.conversation_id = Some(conv_id);
                        }
                    }
                    if let Some(b_id) = branch_id {
                        if let Some(session) = self.sessions.get_mut(self.active_session) {
                            session.branch_id = Some(b_id);
                        }
                    }

                    if !agent.is_empty() {
                        self.last_selected_agent = agent.clone();
                    }

                    let generation_meta = self.generation_meta(generation_id);
                    let mut model_name = None;
                    let mut output_tokens_to_record = self.output_token_estimate;
                    if let Some(idx) = self.generation_msg_idx(generation_id) {
                        if let Some(session) = self.sessions.get_mut(self.active_session) {
                            if let Some(m) = session.messages.get_mut(idx) {
                                if !content.is_empty() {
                                    m.content = content;
                                }
                                if !thinking.is_empty() {
                                    m.thinking = thinking;
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
                                if self.last_token_estimate == 0 {
                                    self.output_token_estimate =
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
                    self.stream_progress = TokenProgress::default();
                    self.stream_processor = None;
                    self.abort_controller = None;
                }
                PendingResponse::Error {
                    generation_id,
                    message,
                } => {
                    self.error = i18n.t("chat.chatError").replace("{message}", &message);
                    if let Some(id) = generation_id {
                        if let Some((_, model, _)) = self.generation_meta(id) {
                            let stats = self.model_stats.entry(model).or_default();
                            stats.error_count = stats.error_count.saturating_add(1);
                        }
                    }
                    self.set_phase_record_status("error");

                    // Drop empty placeholder assistant message on failure.
                    if let Some(idx) = generation_id.and_then(|id| self.generation_msg_idx(id)) {
                        let should_remove = self
                            .sessions
                            .get(self.active_session)
                            .map(|session| {
                                idx < session.messages.len()
                                    && session.messages[idx].content.is_empty()
                            })
                            .unwrap_or(false);
                        if should_remove {
                            self.remove_message_at(idx);
                        }
                    }

                    if let Some(id) = generation_id {
                        self.remove_generation(id);
                    }
                    self.sending = !self.generation_states.is_empty();
                    self.ai_status = AiStatus::Error;
                    self.stop_requested = false;
                    // Reset streaming progress on error
                    self.stream_progress = TokenProgress::default();
                    self.stream_processor = None;
                    self.abort_controller = None;
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
                        if let Some(session) = self.sessions.get_mut(self.active_session) {
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
                        if let Some(session) = self.sessions.get_mut(self.active_session) {
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
                PendingResponse::UiMessage(msg) => {
                    self.success_message = Some(msg);
                }
                PendingResponse::ExternalEditorResult(content) => {
                    self.input = content;
                }
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
