use super::*;

const MAX_INLINE_ATTACHMENT_B64_CHARS: usize = 8_192;
const MAX_SSE_BUFFER_BYTES: usize = 1024 * 1024; // 1 MB max SSE frame buffer
const MAX_BUFFERED_TOKENS_BYTES: usize = 256 * 1024; // 256 KB accumulated token buffer

/// RAII guard to decrement active_generations counter on drop.
/// Ensures counter is decremented even if async task exits early or panics.
struct ActiveGenerationGuard(Arc<std::sync::atomic::AtomicU64>);

impl Drop for ActiveGenerationGuard {
    fn drop(&mut self) {
        // Relaxed ordering is sufficient: this is a counter decrement,
        // not a memory barrier. The saturating_sub handles the 0 case.
        self.0.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
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
        _ctx: &egui::Context,
        autotune_chain_enabled: bool,
    ) {
        let msg = self.input.trim().to_string();
        if msg.is_empty() || self.sending {
            return;
        }

        // Check concurrent generation limit to prevent resource exhaustion
        let current_generations = self
            .active_generations
            .load(std::sync::atomic::Ordering::SeqCst);
        if current_generations >= MAX_CONCURRENT_GENERATIONS as u64 {
            self.error = format!(
                "Too many concurrent generations ({}). Please wait for some to complete.",
                current_generations
            );
            return;
        }

        let expanded_msg = self.expand_prompt_command(&msg);
        let mode = self.selected_mode.clone();
        // Validate phase before sending — if the phase is not in the known list
        // fetched from backend, default to empty (backend will use its default phase).
        let phase = if self.phases_loaded
            && !self.selected_phase.is_empty()
            && !self.phases.contains(&self.selected_phase)
        {
            eprintln!(
                "chat: phase '{}' not in known phases {:?}, resetting to empty",
                self.selected_phase, self.phases
            );
            String::new()
        } else {
            self.selected_phase.clone()
        };
        let base_url = backend.base_url().to_string();
        let selected_models = Self::normalize_models(&self.selected_models);
        let autotune_extra = if autotune_chain_enabled {
            Some(AutoTuneView::load_runtime_options())
        } else {
            None
        };

        self.input.clear();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let atts = std::mem::take(&mut self.attachments);
        let attachment_summary = Self::build_attachment_summary(&atts);
        let outbound_msg = format!("{expanded_msg}{attachment_summary}");

        // Add user message immediately
        self.session().messages.push(Message {
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
        });
        self.save_sessions_to_disk();

        self.last_token_estimate = 0;
        self.input_token_estimate = Self::estimate_tokens_improved(&expanded_msg);
        self.output_token_estimate = 0;

        // Add a "running" phase record
        let now_ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
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
        let mut skipped_models: Vec<String> = Vec::new();

        for model_name in selected_models {
            // Reserve one generation slot atomically for this model.
            let previous = active_gen_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let current_gen_count = previous + 1;
            if current_gen_count > MAX_CONCURRENT_GENERATIONS as u64 {
                active_gen_count.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                skipped_models.push(model_name.clone());
                continue;
            }

            let generation_id = self.next_generation_id();
            let input_tokens = self.input_token_estimate;

            eprintln!(
                "[Gen] Started generation {} for model '{}' (active: {}/{})",
                generation_id, model_name, current_gen_count, MAX_CONCURRENT_GENERATIONS
            );
            self.session().messages.push(Message {
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
            });
            let msg_idx = self.session().messages.len().saturating_sub(1);

            let tx = self.pending_tx.clone();
            let backend_clone = backend.clone();
            let mode_clone = mode.clone();
            let phase_clone = phase.clone();
            let outbound_clone = outbound_msg.clone();
            let model_clone = model_name.clone();
            let base_url_clone = base_url.clone();
            let conv_id_clone = self.sessions[self.active_session].conversation_id.clone();
            let branch_id_clone = self.sessions[self.active_session].branch_id.clone();
            let fallback_options = Self::merge_options_with_tracking(
                autotune_extra.clone(),
                conv_id_clone.as_deref(),
                branch_id_clone.as_deref(),
            );
            let active_gen_guard = ActiveGenerationGuard::new(active_gen_count.clone());
            let sc = stream_client.clone();
            let handle = tokio::spawn(async move {
                // Guard ensures active_generations is decremented when this task exits
                let _guard = active_gen_guard;

                let phase_val = if phase_clone.is_empty() {
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
                        "messages": [{"role": "user", "content": outbound_clone}],
                        "mode": mode_clone,
                        "phase": phase_val,
                    })
                };

                if !use_workflow_rpc {
                    if !model_clone.trim().is_empty() && model_clone != "auto" {
                        body["options"] = serde_json::json!({
                            "model": model_clone,
                        });
                    }

                    if let Some(extra) = fallback_options.clone() {
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
                                    eprintln!(
                                        "[Gen] Workflow generation {} returned error: {}",
                                        generation_id, error_msg
                                    );
                                    let _ = tx.send(PendingResponse::Error {
                                        generation_id: Some(generation_id),
                                        message: format!("workflow.ask failed: {error_msg}"),
                                    });
                                } else {
                                    let result_text = serde_json::to_string_pretty(
                                        value.get("result").unwrap_or(&value),
                                    )
                                    .unwrap_or_default();
                                    eprintln!(
                                        "[Gen] Workflow generation {} completed",
                                        generation_id
                                    );
                                    let _ = tx.send(PendingResponse::ChatCompleted {
                                        generation_id,
                                        content: result_text,
                                        thinking: String::new(),
                                        agent: "workflow".to_string(),
                                        conversation_id: None,
                                        branch_id: None,
                                    });
                                }
                            } else {
                                eprintln!(
                                    "[Gen] Workflow generation {} - JSON parse failed",
                                    generation_id
                                );
                                let _ = tx.send(PendingResponse::Error {
                                    generation_id: Some(generation_id),
                                    message: "workflow response parse error".to_string(),
                                });
                            }
                        }
                        Err(e) => {
                            eprintln!("[Gen] Workflow generation {} failed: {}", generation_id, e);
                            let _ = tx.send(PendingResponse::Error {
                                generation_id: Some(generation_id),
                                message: format!("workflow.ask error: {e}"),
                            });
                        }
                    }
                    return; // Skip the normal chat/stream flow
                }

                let endpoint = format!("{}/chat/stream", base_url_clone.trim_end_matches('/'));
                let stream_resp = sc.post(&endpoint).json(&body).send().await;

                match stream_resp {
                    Ok(resp) => {
                        let mut resp = if let Err(err) = resp.error_for_status_ref() {
                            let fallback = backend_clone
                                .chat_with_options(
                                    &outbound_clone,
                                    &mode_clone,
                                    &phase_clone,
                                    Some(&model_clone),
                                    fallback_options.clone(),
                                )
                                .await
                                .map(
                                    |(content, thinking, agent)| PendingResponse::ChatCompleted {
                                        generation_id,
                                        content,
                                        thinking,
                                        agent,
                                        conversation_id: None,
                                        branch_id: None,
                                    },
                                )
                                .unwrap_or_else(|e| PendingResponse::Error {
                                    generation_id: Some(generation_id),
                                    message: format!("stream error: {err}; fallback: {e}"),
                                });
                            let _ = tx.send(fallback);
                            return;
                        } else {
                            resp
                        };

                        let mut sse_buffer = String::with_capacity(16384);
                        let mut final_content: Option<String> = None;
                        let mut final_thinking: Option<String> = None;
                        let mut final_agent: Option<String> = None;
                        let mut final_conv_id: Option<String> = None;
                        let mut final_branch_id: Option<String> = None;
                        let mut buffered_token = String::with_capacity(4096);
                        let mut buffered_reasoning = String::with_capacity(2048);
                        let mut last_stream_flush = std::time::Instant::now();
                        let mut total_buffer_bytes = 0usize;

                        loop {
                            let chunk = match resp.chunk().await {
                                Ok(Some(c)) => c,
                                Ok(None) => break,
                                Err(e) => {
                                    let _ = tx.send(PendingResponse::Error {
                                        generation_id: Some(generation_id),
                                        message: format!("read error: {e}"),
                                    });
                                    return;
                                }
                            };

                            // Safely append chunk to buffer with overflow protection
                            let chunk_len = chunk.len();
                            if sse_buffer.len() + chunk_len > MAX_SSE_BUFFER_BYTES {
                                let _ = tx.send(PendingResponse::Error {
                                    generation_id: Some(generation_id),
                                    message: format!(
                                        "stream frame overflow ({}+{} > {}MB)",
                                        sse_buffer.len(),
                                        chunk_len,
                                        MAX_SSE_BUFFER_BYTES / (1024 * 1024)
                                    ),
                                });
                                return;
                            }

                            // Normalize CRLF to LF so frame splitting is consistent across
                            // backends and platforms that may emit different line endings.
                            let part = String::from_utf8_lossy(&chunk);
                            if part.contains('\r') {
                                for ch in part.chars() {
                                    if ch != '\r' {
                                        sse_buffer.push(ch);
                                    }
                                }
                            } else {
                                sse_buffer.push_str(&part);
                            }

                            // Process complete frames only (delimited by \n\n)
                            while let Some(split_at) = sse_buffer.find("\n\n") {
                                let frame = sse_buffer[..split_at].to_string();
                                sse_buffer.drain(..split_at + 2);

                                // Parse event and data from frame
                                let mut event_name = String::new();
                                let mut data_payload = String::new();
                                for line in frame.lines() {
                                    if let Some(rest) = line.strip_prefix("event:") {
                                        event_name = rest.trim().to_string();
                                    } else if let Some(rest) = line.strip_prefix("data:") {
                                        if !data_payload.is_empty() {
                                            data_payload.push('\n');
                                        }
                                        data_payload.push_str(rest.trim());
                                    }
                                }

                                if data_payload.is_empty() {
                                    continue;
                                }

                                // Parse JSON data safely
                                let data: Value = match serde_json::from_str(&data_payload) {
                                    Ok(v) => v,
                                    Err(e) => {
                                        eprintln!(
                                            "[SSE] JSON parse error in {}: {}",
                                            event_name, e
                                        );
                                        continue;
                                    }
                                };

                                match event_name.as_str() {
                                    "chunk" => {
                                        let token = data
                                            .get("token")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string();
                                        let reasoning = data
                                            .get("reasoning")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or_default()
                                            .to_string();

                                        if !token.is_empty() || !reasoning.is_empty() {
                                            // Track buffer usage and flush if needed
                                            let token_bytes = token.len();
                                            let reasoning_bytes = reasoning.len();
                                            total_buffer_bytes += token_bytes + reasoning_bytes;

                                            buffered_token.push_str(&token);
                                            buffered_reasoning.push_str(&reasoning);

                                            // Force flush if buffer exceeds max accumulated size
                                            if total_buffer_bytes > MAX_BUFFERED_TOKENS_BYTES {
                                                let _ = tx.send(PendingResponse::StreamChunk {
                                                    generation_id,
                                                    token: std::mem::take(&mut buffered_token),
                                                    reasoning: std::mem::take(
                                                        &mut buffered_reasoning,
                                                    ),
                                                });
                                                total_buffer_bytes = 0;
                                                last_stream_flush = std::time::Instant::now();
                                            }
                                        }
                                    }
                                    "telemetry" => {
                                        if let Some(te) = data.get("token_economy") {
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
                                            let _ = tx.send(PendingResponse::TokenEconomy {
                                                generation_id,
                                                input_tokens,
                                                output_tokens,
                                                total_tokens,
                                            });
                                        }
                                    }
                                    "result" => {
                                        final_content = data
                                            .get("response")
                                            .and_then(|v| v.as_str())
                                            .map(ToOwned::to_owned);
                                        final_thinking = data
                                            .get("thinking")
                                            .and_then(|v| v.as_str())
                                            .map(ToOwned::to_owned);
                                        final_agent = data
                                            .get("agent")
                                            .or_else(|| data.get("selected_agent"))
                                            .or_else(|| {
                                                data.pointer("/capability_routing/selected_agent")
                                            })
                                            .and_then(|v| v.as_str())
                                            .map(String::from);
                                        final_conv_id = data
                                            .get("conversation_id")
                                            .and_then(|v| v.as_str())
                                            .map(String::from);
                                        final_branch_id = data
                                            .get("branch_id")
                                            .and_then(|v| v.as_str())
                                            .map(String::from);
                                    }
                                    "error" => {
                                        if !buffered_token.is_empty()
                                            || !buffered_reasoning.is_empty()
                                        {
                                            let _ = tx.send(PendingResponse::StreamChunk {
                                                generation_id,
                                                token: std::mem::take(&mut buffered_token),
                                                reasoning: std::mem::take(&mut buffered_reasoning),
                                            });
                                        }
                                        let message = data
                                            .get("message")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("unknown stream error")
                                            .to_string();
                                        let _ = tx.send(PendingResponse::Error {
                                            generation_id: Some(generation_id),
                                            message,
                                        });
                                        return;
                                    }
                                    _ => {
                                        eprintln!("[SSE] Unknown event type: {}", event_name);
                                    }
                                }

                                // Time-based flush to maintain responsive UI
                                if (!buffered_token.is_empty() || !buffered_reasoning.is_empty())
                                    && last_stream_flush.elapsed() >= stream_chunk_flush_interval
                                {
                                    let _ = tx.send(PendingResponse::StreamChunk {
                                        generation_id,
                                        token: std::mem::take(&mut buffered_token),
                                        reasoning: std::mem::take(&mut buffered_reasoning),
                                    });
                                    last_stream_flush = std::time::Instant::now();
                                }
                            }
                        }

                        // Flush any remaining buffered content
                        if !buffered_token.is_empty() || !buffered_reasoning.is_empty() {
                            let _ = tx.send(PendingResponse::StreamChunk {
                                generation_id,
                                token: buffered_token,
                                reasoning: buffered_reasoning,
                            });
                        }

                        let status_log = format!(
                            "tokens:{}, thinking:{}, agent:{}",
                            final_content.as_ref().map(|c| c.len()).unwrap_or(0),
                            !final_thinking
                                .as_ref()
                                .map(|t| t.is_empty())
                                .unwrap_or(true),
                            final_agent
                                .as_ref()
                                .map(|a| a.as_str())
                                .unwrap_or("unknown")
                        );
                        eprintln!(
                            "[Gen] Generation {} completed ({})",
                            generation_id, status_log
                        );

                        let _ = tx.send(PendingResponse::ChatCompleted {
                            generation_id,
                            content: final_content.unwrap_or_default(),
                            thinking: final_thinking.unwrap_or_default(),
                            agent: final_agent.unwrap_or_default(),
                            conversation_id: final_conv_id,
                            branch_id: final_branch_id,
                        });
                    }
                    Err(err) => {
                        eprintln!(
                            "[Gen] Generation {} stream request failed (attempting fallback): {}",
                            generation_id, err
                        );
                        let fallback = backend_clone
                            .chat_with_options(
                                &outbound_clone,
                                &mode_clone,
                                &phase_clone,
                                Some(&model_clone),
                                fallback_options.clone(),
                            )
                            .await
                            .map(
                                |(content, thinking, agent)| PendingResponse::ChatCompleted {
                                    generation_id,
                                    content,
                                    thinking,
                                    agent,
                                    conversation_id: None,
                                    branch_id: None,
                                },
                            )
                            .unwrap_or_else(|e| PendingResponse::Error {
                                generation_id: Some(generation_id),
                                message: format!("request error: {err}; fallback: {e}"),
                            });
                        let _ = tx.send(fallback);
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

        if !skipped_models.is_empty() {
            self.error = format!(
                "Skipped {} model(s) due to concurrency limit ({}): {}",
                skipped_models.len(),
                MAX_CONCURRENT_GENERATIONS,
                skipped_models.join(", ")
            );
        }

        if self.generation_states.len() == existing_generations {
            // Nothing started (for example all selected models were skipped).
            self.sending = false;
            self.ai_status = AiStatus::Error;
            self.set_phase_record_status("error");
        }

        self.save_sessions_to_disk();
    }

    /// Drain any pending async responses and update the session / `ai_status`.
    pub(super) fn process_pending(&mut self, i18n: &I18n) {
        for _ in 0..self.max_pending_events_per_frame {
            let Ok(pending) = self.pending_rx.try_recv() else {
                break;
            };
            match pending {
                PendingResponse::Phases(list) => {
                    self.phases = list;
                    self.phases_loaded = true;
                }
                PendingResponse::Models(list) => {
                    self.available_models = if list.is_empty() {
                        vec!["auto".to_string()]
                    } else {
                        list
                    };
                    if !self
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
                    if let Some(idx) = self.generation_msg_idx(generation_id) {
                        if let Some(session) = self.sessions.get_mut(self.active_session) {
                            if let Some(m) = session.messages.get_mut(idx) {
                                if !token.is_empty() {
                                    m.content.push_str(&token);
                                }
                                if !reasoning.is_empty() {
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
                }
                PendingResponse::ChatCompleted {
                    generation_id,
                    content,
                    thinking,
                    agent,
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
                }
            }
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
