use super::*;
use crate::schema::{
    ConfigOptionUpdate, ContentBlock, ContentChunk, SessionConfigSelectOptions, SessionUpdate,
    TextContent,
};

const ACP_PERMISSION_OPTION_APPROVE: &str = "approve";

// ── Session handlers ─────────────────────────────────────────────────────

/// Handle `session/new` — creates a new ACP session.
pub async fn session_new_payload(server: &AcpServer, params: Value) -> Result<Value> {
    let session_id = super::generate_acp_session_id();
    let modes = super::build_default_modes();
    let explicit_mode = params
        .get("mode")
        .and_then(|m| m.as_str())
        .filter(|m| !m.trim().is_empty());
    let current_mode = super::normalize_acp_mode(explicit_mode);
    let cwd = params
        .get("cwd")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let additional_directories = super::extract_additional_directories(&params);

    let work_dirs: Vec<String> = params
        .get("work_dirs")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let cwd = cwd.or_else(|| work_dirs.first().cloned());

    let additional_directories = {
        let mut merged = additional_directories;
        for wd in &work_dirs {
            if cwd.as_ref() != Some(wd) && !merged.contains(wd) {
                merged.push(wd.clone());
            }
        }
        merged
    };

    let mut modes = modes;
    modes.current_mode_id = crate::schema::SessionModeId::new(current_mode.clone());

    let config_options_init = HashMap::new();
    {
        let mut state = super::acp_session_state().lock().await;
        state.insert(
            session_id.clone(),
            super::AcpSessionState {
                cwd: cwd.clone(),
                mode: current_mode.clone(),
                additional_directories: additional_directories.clone(),
                config_options: config_options_init.clone(),
                favorite_config_values: Default::default(),
            },
        );
    }

    #[cfg(feature = "backend-sqlite")]
    {
        let now = crate::shared::timestamps::now_ts_ms();
        if let Some(ref store) = server.session_store {
            use crate::acp::session_persistence::PersistedSession;
            let persisted = PersistedSession {
                session_id: session_id.clone(),
                cwd: cwd.clone(),
                mode: current_mode.clone(),
                additional_directories: additional_directories.clone(),
                config_options: config_options_init.clone(),
                created_at_ms: now,
                last_active_ms: now,
            };
            if let Err(e) = store.upsert(&persisted).await {
                tracing::warn!(error = %e, session_id = %session_id, "Failed to persist new session to SQLite");
            }
        }
    }

    let config_options = super::build_model_config_options(server, None);

    Ok(serde_json::to_value(
        crate::schema::NewSessionResponse::new(crate::schema::SessionId::new(session_id))
            .modes(modes)
            .config_options(config_options),
    )?)
}

/// Handle `session/load` — loads an existing session.
pub async fn session_load_payload(server: &AcpServer, params: Value) -> Result<Value> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or_default();

    #[cfg(feature = "backend-sqlite")]
    if !session_id.is_empty() {
        if let Some(ref store) = server.session_store {
            let mut state = super::acp_session_state().lock().await;
            if !state.contains_key(session_id) {
                if let Ok(Some(persisted)) = store.load(session_id).await {
                    state.insert(
                        session_id.to_string(),
                        super::AcpSessionState {
                            cwd: persisted.cwd.clone(),
                            mode: persisted.mode.clone(),
                            additional_directories: persisted.additional_directories.clone(),
                            config_options: persisted.config_options.clone(),
                            favorite_config_values: Default::default(),
                        },
                    );
                }
            }
        }
    }

    // Build response
    let stored = {
        let state = super::acp_session_state().lock().await;
        state.get(session_id).cloned().unwrap_or_default()
    };
    let current_mode = super::normalize_acp_mode(Some(stored.mode.as_str()));
    let mut modes = super::build_default_modes();
    modes.current_mode_id = crate::schema::SessionModeId::new(current_mode);

    let current_model = stored.config_options.get("model").and_then(Value::as_str);
    let config_options = super::build_model_config_options(server, current_model);

    Ok(serde_json::to_value(&crate::schema::LoadSessionResponse {
        modes: Some(modes),
        config_options: Some(config_options),
        meta: None,
    })?)
}

/// Handle `session/prompt` — processes a user prompt within a session.
pub async fn session_prompt_payload(server: &AcpServer, params: Value) -> Result<Value> {
    use crate::acp::r#impl::chat::streaming::StreamObserver;
    use crate::acp::r#impl::chat::{process_chat_request, ChatParams};
    use crate::rpc_protocol::chat_trace_context;

    let session_state = super::session_state_for_prompt(&params).await;

    let session_id_for_notification = params
        .get("sessionId")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let chat_params_value = super::build_chat_params_from_acp(params, &session_state);
    let mut chat_params: ChatParams = match serde_json::from_value(chat_params_value) {
        Ok(p) => p,
        Err(e) => {
            return Err(anyhow::anyhow!("invalid chat params: {}", e));
        }
    };

    let pipeline_trace = chat_trace_context(&None, "session.prompt");

    tracing::info!("ACP session/prompt: delegating to process_chat_request");

    // ── Create real-time streaming bridge ─────────────────────────────
    // Create a channel that receives StreamFrame events from the autonomy
    // loop / pipeline, and bridge them to ACP session/update notifications
    // (agent_thought_chunk / agent_message_chunk) so Zed sees streaming
    // text and reasoning in real-time instead of a blank wait.
    let (sse_tx, mut sse_rx) =
        tokio::sync::mpsc::unbounded_channel::<crate::acp::r#impl::chat::streaming::StreamFrame>();
    // Clone the sender: one copy goes to StreamObserver (shared with pipeline),
    // the other stays here to signal bridge exit via drop(sse_signal) below.
    let sse_signal = sse_tx.clone();
    let stream_observer = StreamObserver::sse(sse_tx);

    // Spawn bridge task: StreamFrame → session/update JSON-RPC notification
    let bridge_sid = session_id_for_notification.clone();
    let bridge_handle: tokio::task::JoinHandle<()> = tokio::spawn(async move {
        let sid = match bridge_sid {
            Some(ref s) => s.clone(),
            None => return, // No session ID, nothing to bridge
        };
        let mut thinking_buf = String::new();
        let mut message_buf = String::new();
        // Flush threshold: flush accumulated content when buffer exceeds this length
        const FLUSH_THRESHOLD: usize = 20;

        while let Some(frame) = sse_rx.recv().await {
            match frame.event {
                "chunk" => {
                    // Reasoning token — accumulate into thinking buffer
                    if let Some(reasoning) = frame.payload.get("reasoning").and_then(|v| v.as_str())
                    {
                        if !reasoning.is_empty() {
                            // Flush any pending message text before starting thought block
                            if !message_buf.is_empty() {
                                send_session_update_notification(
                                    &sid,
                                    "agent_message_chunk",
                                    &message_buf,
                                )
                                .await;
                                message_buf.clear();
                            }
                            thinking_buf.push_str(reasoning);
                            if thinking_buf.len() >= FLUSH_THRESHOLD {
                                send_session_update_notification(
                                    &sid,
                                    "agent_thought_chunk",
                                    &thinking_buf,
                                )
                                .await;
                                thinking_buf.clear();
                            }
                        }
                    }
                    // Regular token — accumulate into message buffer
                    if let Some(token) = frame.payload.get("token").and_then(|v| v.as_str()) {
                        if !token.is_empty() {
                            // Flush any pending thinking before starting message
                            if !thinking_buf.is_empty() {
                                send_session_update_notification(
                                    &sid,
                                    "agent_thought_chunk",
                                    &thinking_buf,
                                )
                                .await;
                                thinking_buf.clear();
                            }
                            message_buf.push_str(token);
                            if message_buf.len() >= FLUSH_THRESHOLD {
                                send_session_update_notification(
                                    &sid,
                                    "agent_message_chunk",
                                    &message_buf,
                                )
                                .await;
                                message_buf.clear();
                            }
                        }
                    }
                }
                "phase_start" | "phase_end" => {
                    // Send phase transitions as lightweight thought markers
                    if let Some(desc) = frame.payload.get("description").and_then(|v| v.as_str()) {
                        let phase = frame
                            .payload
                            .get("phase")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if !phase.is_empty() && !desc.is_empty() {
                            send_session_update_notification(
                                &sid,
                                "agent_thought_chunk",
                                &format!("[{}] {}", phase, desc),
                            )
                            .await;
                        }
                    }
                }
                "tool_approval" => {
                    if !thinking_buf.is_empty() {
                        send_session_update(
                            &sid,
                            SessionUpdate::AgentThoughtChunk(ContentChunk::new(
                                ContentBlock::Text(TextContent::new(std::mem::take(
                                    &mut thinking_buf,
                                ))),
                            )),
                        )
                        .await;
                    }
                    if !message_buf.is_empty() {
                        send_session_update(
                            &sid,
                            SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                ContentBlock::Text(TextContent::new(std::mem::take(
                                    &mut message_buf,
                                ))),
                            )),
                        )
                        .await;
                    }

                    let tool_name = frame
                        .payload
                        .get("tool_name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let tool_args = frame
                        .payload
                        .get("tool_args")
                        .cloned()
                        .unwrap_or(Value::Null);
                    let mode = frame
                        .payload
                        .get("mode")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let risk_score = frame
                        .payload
                        .get("risk_score")
                        .and_then(Value::as_f64)
                        .unwrap_or_default();

                    // Register pending permission request for the inbound
                    // session/request_permission handler to consume.
                    // The actual authorization JSON-RPC request/response flow
                    // is handled by AcpServer::request_client_permission() via
                    // ensure_tool_permission in the autonomy loop. This pending
                    // registration ensures that when the ACP client responds to
                    // the session/request_permission JSON-RPC request, the
                    // inbound handler can still route the decision back to the
                    // governance layer (evaluator.approve_tool / .revoke_tool_approval).
                    register_pending_permission_request(
                        &sid,
                        tool_name.clone(),
                        tool_args.clone(),
                        mode.clone(),
                        risk_score,
                    )
                    .await;

                    // NOTE: We no longer send SessionUpdate::PermissionRequest here.
                    // The AcpServer::request_client_permission() method (called by
                    // ensure_tool_permission in the autonomy loop) now centrally
                    // emits both:
                    //   1. session/update::PermissionRequest notification (UI hint)
                    //   2. session/request_permission JSON-RPC request (authoritative)
                    // This eliminates the double-send that occurred when both
                    // the bridge and the autonomy loop emitted permission requests.
                }
                "result" | "done" => {
                    // Stream ending — flush remaining buffers
                    if !thinking_buf.is_empty() {
                        send_session_update_notification(
                            &sid,
                            "agent_thought_chunk",
                            &thinking_buf,
                        )
                        .await;
                        thinking_buf.clear();
                    }
                    if !message_buf.is_empty() {
                        send_session_update_notification(&sid, "agent_message_chunk", &message_buf)
                            .await;
                        message_buf.clear();
                    }
                }
                _ => {}
            }
        }
    });

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        process_chat_request(
            server,
            &mut chat_params,
            Some(stream_observer),
            &pipeline_trace,
            None,
            None,
        ),
    )
    .await;

    // Ensure bridge task finishes — drop our clone of the sender so the
    // receiver gets None and the bridge loop exits.
    drop(sse_signal);
    let _ = bridge_handle.await;

    match result {
        Ok(Ok(chat_result)) => {
            let response_text = chat_result
                .get("response")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            tracing::info!(
                response_chars = response_text.chars().count(),
                "ACP session/prompt: completed successfully"
            );

            // ✅ Real-time streaming via bridge task above handles all
            // `agent_thought_chunk` and `agent_message_chunk` notifications.
            // No post-hoc <thinking> parsing needed.

            let prompt_response = serde_json::to_value(crate::schema::PromptResponse::new(
                crate::schema::StopReason::EndTurn,
            ))?;

            Ok(prompt_response)
        }
        Ok(Err(err)) => {
            let msg = err.to_string();
            tracing::warn!("ACP session/prompt: error: {}", msg);
            if super::audit::is_rate_limited_message(&msg) {
                Err(anyhow::anyhow!(
                    super::audit::normalize_rate_limited_message(&msg)
                ))
            } else {
                Err(anyhow::anyhow!(msg))
            }
        }
        Err(_elapsed) => {
            let msg = "prompt request timed out after 300s".to_string();
            tracing::warn!("ACP session/prompt: timeout");
            Err(anyhow::anyhow!(msg))
        }
    }
}

/// Send a typed session/update notification chunk.
pub async fn send_chunk(server: &AcpServer, session_id: &str, chunk_type: &str, text: &str) {
    use crate::schema::{
        ContentBlock, ContentChunk, SessionNotification, SessionUpdate, TextContent,
    };
    let update = match chunk_type {
        "agent_thought_chunk" => SessionUpdate::AgentThoughtChunk(ContentChunk::new(
            ContentBlock::Text(TextContent::new(text)),
        )),
        _ => SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
            TextContent::new(text),
        ))),
    };
    let notif = SessionNotification::new(session_id.into(), update);
    if let Ok(value) = serde_json::to_value(&notif) {
        let _ = crate::acp::r#impl::io::send_notification(server, "session/update", value).await;
    }
}

/// Send a session/update notification via the global transport (no &AcpServer needed).
///
/// Used by the streaming bridge task (`session_prompt_payload`) to send
/// real-time `agent_thought_chunk` and `agent_message_chunk` notifications
/// from the autonomy loop's `StreamFrame` events directly through the global
/// transport, without borrowing the `AcpServer` across a `tokio::spawn` boundary.
async fn send_session_update_notification(session_id: &str, chunk_type: &str, text: &str) {
    use crate::schema::{ContentBlock, ContentChunk, SessionUpdate, TextContent};
    let update = match chunk_type {
        "agent_thought_chunk" => SessionUpdate::AgentThoughtChunk(ContentChunk::new(
            ContentBlock::Text(TextContent::new(text)),
        )),
        _ => SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(
            TextContent::new(text),
        ))),
    };
    send_session_update(session_id, update).await;
}

async fn send_session_update(session_id: &str, update: SessionUpdate) {
    use crate::schema::SessionNotification;
    let notif = SessionNotification::new(session_id.into(), update);
    if let Ok(value) = serde_json::to_value(&notif) {
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": value,
        });
        if let Some(transport) = crate::acp::transport::get_current_transport() {
            let _ = transport.write_json_line(&payload).await;
        }
    }
}

async fn register_pending_permission_request(
    session_id: &str,
    tool_name: String,
    tool_args: Value,
    mode: String,
    risk_score: f64,
) {
    {
        let mut decisions = super::acp_permission_state().lock().await;
        decisions.remove(session_id);
    }
    let mut pending = super::acp_pending_permission_requests().lock().await;
    pending.insert(
        session_id.to_string(),
        super::PendingPermissionRequest {
            tool_name,
            tool_args,
            mode,
            risk_score,
        },
    );
}

/// Handle `session/cancel` — cancels a session.
pub async fn session_cancel_payload(_server: &AcpServer, params: Value) -> Result<Value> {
    let session_id = params
        .get("sessionId")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    tracing::warn!(
        target: "acp::protocol_pack",
        session_id = %session_id,
        "session_cancel_payload: session {} cancelled via notification",
        session_id
    );

    Err(anyhow::anyhow!("session {} cancelled", session_id))
}

/// Handle `session/list` — lists existing sessions.
pub async fn session_list_payload(_server: &AcpServer, _params: Value) -> Result<Value> {
    let mut sessions = vec![];

    {
        let state = super::acp_session_state().lock().await;
        for sid in state.keys() {
            sessions.push(serde_json::json!({
                "id": sid,
            }));
        }
    }

    #[cfg(feature = "backend-sqlite")]
    if let Some(ref store) = _server.session_store {
        if let Ok(persisted) = store.list_all().await {
            let existing: std::collections::HashSet<String> = sessions
                .iter()
                .filter_map(|s| s.get("id").and_then(Value::as_str).map(String::from))
                .collect();
            for s in &persisted {
                if !existing.contains(&s.session_id) {
                    sessions.push(serde_json::json!({
                        "id": s.session_id,
                    }));
                }
            }
        }
    }

    Ok(serde_json::to_value(
        &crate::schema::ListSessionsResponse {
            sessions,
            next_cursor: None,
            meta: None,
        },
    )?)
}

/// Handle `session/set_mode` — sets the current mode for a session.
pub async fn session_set_mode_payload(server: &AcpServer, params: Value) -> Result<Value> {
    use crate::schema::{
        CurrentModeUpdate, SessionId, SessionNotification, SessionUpdate, SetSessionModeResponse,
    };
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mode_id = super::normalize_acp_mode(params.get("modeId").and_then(Value::as_str));
    if !session_id.is_empty() {
        let _snapshot = {
            let mut state = super::acp_session_state().lock().await;
            let entry = state.entry(session_id.to_string()).or_default();
            entry.mode = mode_id.clone();
            #[cfg(feature = "backend-sqlite")]
            let _snapshot = (
                entry.cwd.clone(),
                entry.mode.clone(),
                entry.additional_directories.clone(),
                entry.config_options.clone(),
            );
            #[cfg(not(feature = "backend-sqlite"))]
            let _snapshot = ();
            _snapshot
        };

        #[cfg(feature = "backend-sqlite")]
        if let Some(ref store) = server.session_store {
            use crate::acp::session_persistence::PersistedSession;
            let now = crate::shared::timestamps::now_ts_ms();
            let persisted = PersistedSession {
                session_id: session_id.to_string(),
                cwd: _snapshot.0,
                mode: _snapshot.1,
                additional_directories: _snapshot.2,
                config_options: _snapshot.3,
                created_at_ms: now,
                last_active_ms: now,
            };
            if let Err(e) = store.upsert(&persisted).await {
                tracing::warn!(error = %e, session_id = %session_id, "Failed to persist mode change to SQLite");
            }
        }

        let notif = SessionNotification::new(
            SessionId::new(session_id.to_string()),
            SessionUpdate::CurrentModeUpdate(CurrentModeUpdate {
                current_mode_id: crate::schema::SessionModeId::new(mode_id),
                meta: None,
            }),
        );
        if let Ok(value) = serde_json::to_value(&notif) {
            let _ =
                crate::acp::r#impl::io::send_notification(server, "session/update", value).await;
        }
    }
    Ok(serde_json::to_value(&SetSessionModeResponse {
        meta: None,
    })?)
}

/// Handle `session/resume` — resumes an existing session.
pub async fn session_resume_payload(server: &AcpServer, params: Value) -> Result<Value> {
    use crate::schema::{ResumeSessionResponse, SessionModeId};
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let cwd = params
        .get("cwd")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(ToString::to_string);

    #[cfg(feature = "backend-sqlite")]
    if !session_id.is_empty() {
        if let Some(ref store) = server.session_store {
            let mut state = super::acp_session_state().lock().await;
            if !state.contains_key(session_id) {
                if let Ok(Some(persisted)) = store.load(session_id).await {
                    state.insert(
                        session_id.to_string(),
                        super::AcpSessionState {
                            cwd: persisted.cwd.clone(),
                            mode: persisted.mode.clone(),
                            additional_directories: persisted.additional_directories.clone(),
                            config_options: persisted.config_options.clone(),
                            favorite_config_values: Default::default(),
                        },
                    );
                }
            }
        }
    }

    let (current_mode, _additional_dirs) = if !session_id.is_empty() {
        let mut state = super::acp_session_state().lock().await;
        let entry = state.entry(session_id.to_string()).or_default();
        if let Some(ref new_cwd) = cwd {
            entry.cwd = Some(new_cwd.clone());
        }
        (entry.mode.clone(), entry.additional_directories.clone())
    } else {
        ("ask".to_string(), vec![])
    };

    let mut modes = super::build_default_modes();
    modes.current_mode_id = SessionModeId::new(current_mode);
    let current_model = {
        let state = super::acp_session_state().lock().await;
        state
            .get(session_id)
            .and_then(|entry| entry.config_options.get("model"))
            .and_then(Value::as_str)
            .map(ToString::to_string)
    };
    let config_options = super::build_model_config_options(server, current_model.as_deref());

    Ok(serde_json::to_value(&ResumeSessionResponse {
        modes: Some(modes),
        config_options: Some(config_options),
        meta: None,
    })?)
}

/// Handle `session/close` — closes and cleans up a session.
pub async fn session_close_payload(server: &AcpServer, params: Value) -> Result<Value> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !session_id.is_empty() {
        // 1. Remove the session from the in-memory map
        {
            let mut state = super::acp_session_state().lock().await;
            state.remove(session_id);
        }

        // 2. Clean up any permission state associated with this session
        {
            let mut permissions = super::acp_permission_state().lock().await;
            permissions.remove(session_id);
        }
        {
            let mut pending = super::acp_pending_permission_requests().lock().await;
            pending.remove(session_id);
        }

        #[cfg(feature = "multi-users-server")]
        if let Some(ref limiter) = server.rate_limiting.rate_limit_middleware {
            limiter.evict_tenant(session_id).await;
        }
        #[cfg(feature = "backend-sqlite")]
        if let Some(ref store) = server.session_store {
            if let Err(e) = store.delete(session_id).await {
                tracing::warn!(error = %e, session_id = %session_id, "Failed to delete session from SQLite");
            }
        }
    }
    Ok(serde_json::to_value(
        &crate::schema::CloseSessionResponse { meta: None },
    )?)
}

/// Handle `session/request_permission` — client responds to a permission request.
pub async fn session_request_permission_payload(
    server: &AcpServer,
    params: Value,
) -> Result<Value> {
    use crate::schema::PermissionOptionId;
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let option_id = params
        .get("optionId")
        .and_then(Value::as_str)
        .unwrap_or_default();

    if !session_id.is_empty() && !option_id.is_empty() {
        let mut permissions = super::acp_permission_state().lock().await;
        permissions.insert(
            session_id.to_string(),
            PermissionOptionId::new(option_id.to_string()),
        );

        let pending_request = {
            let mut pending = super::acp_pending_permission_requests().lock().await;
            pending.remove(session_id)
        };

        if let Some(pending_request) = pending_request {
            if let Some(ref harness_bus) = server.governance_deps.harness_bus {
                if option_id.eq_ignore_ascii_case(ACP_PERMISSION_OPTION_APPROVE) {
                    harness_bus
                        .evaluator
                        .approve_tool(&pending_request.tool_name);
                } else {
                    harness_bus
                        .evaluator
                        .revoke_tool_approval(&pending_request.tool_name);
                }
            }

            tracing::info!(
                session_id = %session_id,
                tool_name = %pending_request.tool_name,
                mode = %pending_request.mode,
                risk_score = pending_request.risk_score,
                tool_args = %pending_request.tool_args,
                decision = %option_id,
                "ACP permission response recorded"
            );
        }
    }

    Ok(serde_json::Value::Object(serde_json::Map::new()))
}

/// Handle `session/set_config_option` — sets a configuration option for a session.
///
/// ✅ Per-session verification: config options are stored in the in-memory
/// `acp_session_state()` map under the session's `AcpSessionState.config_options`.
/// Each session gets its own entry (via `state.entry(session_id).or_default()`),
/// so options from different sessions never collide. The value is also persisted
/// to SQLite when feature `backend-sqlite` is enabled.
pub async fn session_set_config_option_payload(server: &AcpServer, params: Value) -> Result<Value> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let config_id = params
        .get("configId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let value = params.get("value").cloned().unwrap_or(Value::Null);

    if !session_id.is_empty() && !config_id.is_empty() {
        let _snapshot = {
            let mut state = super::acp_session_state().lock().await;
            let session = state.entry(session_id.to_string()).or_default();
            session
                .config_options
                .insert(config_id.to_string(), value.clone());
            if config_id == "mode" {
                session.mode = super::normalize_acp_mode(value.as_str());
            }
            #[cfg(feature = "backend-sqlite")]
            let _snapshot = (
                session.cwd.clone(),
                session.mode.clone(),
                session.additional_directories.clone(),
                session.config_options.clone(),
            );
            #[cfg(not(feature = "backend-sqlite"))]
            let _snapshot = ();
            _snapshot
        };

        #[cfg(feature = "backend-sqlite")]
        if let Some(ref store) = server.session_store {
            use crate::acp::session_persistence::PersistedSession;
            let now = crate::shared::timestamps::now_ts_ms();
            let persisted = PersistedSession {
                session_id: session_id.to_string(),
                cwd: _snapshot.0,
                mode: _snapshot.1,
                additional_directories: _snapshot.2,
                config_options: _snapshot.3,
                created_at_ms: now,
                last_active_ms: now,
            };
            if let Err(e) = store.upsert(&persisted).await {
                tracing::warn!(error = %e, session_id = %session_id, "Failed to persist config option change to SQLite");
            }
        }

        let current_model = if config_id == "model" {
            value.as_str()
        } else {
            None
        };
        let config_options = super::build_model_config_options(server, current_model);
        send_session_update(
            session_id,
            SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate {
                config_options: config_options.clone(),
                meta: None,
            }),
        )
        .await;

        return Ok(serde_json::to_value(
            &crate::schema::SetSessionConfigOptionResponse {
                config_options,
                meta: None,
            },
        )?);
    }

    Ok(serde_json::to_value(
        &crate::schema::SetSessionConfigOptionResponse {
            config_options: vec![],
            meta: None,
        },
    )?)
}

// ── Session delete / config get/set ──────────────────────────────────────

pub async fn session_delete_payload(_server: &AcpServer, params: Value) -> Result<Value> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or_default();

    let mut deleted = false;

    if !session_id.is_empty() {
        let removed = {
            let mut state = super::acp_session_state().lock().await;
            state.remove(session_id)
        };
        if removed.is_some() {
            deleted = true;
            tracing::info!(
                target: "acp::protocol_pack",
                session_id = %session_id,
                "session/delete: session deleted"
            );

            // Clean up permission state associated with this session
            {
                let mut permissions = super::acp_permission_state().lock().await;
                permissions.remove(session_id);
            }
            {
                let mut pending = super::acp_pending_permission_requests().lock().await;
                pending.remove(session_id);
            }

            #[cfg(feature = "multi-users-server")]
            if let Some(ref limiter) = _server.rate_limiting.rate_limit_middleware {
                limiter.evict_tenant(session_id).await;
            }

            #[cfg(feature = "backend-sqlite")]
            if let Some(ref store) = _server.session_store {
                if let Err(e) = store.delete(session_id).await {
                    tracing::warn!(error = %e, session_id = %session_id, "Failed to delete session from SQLite");
                }
            }
        } else {
            tracing::warn!(
                target: "acp::protocol_pack",
                session_id = %session_id,
                "session/delete: session not found"
            );
        }
    }

    Ok(serde_json::json!({
        "deleted": deleted,
        "sessionId": session_id
    }))
}

pub async fn session_config_set_payload(_server: &AcpServer, params: Value) -> Result<Value> {
    session_set_config_option_payload(_server, params).await
}

pub async fn session_config_get_payload(_server: &AcpServer, params: Value) -> Result<Value> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or_default();

    let config_options = if !session_id.is_empty() {
        let state = super::acp_session_state().lock().await;
        state
            .get(session_id)
            .map(|s| s.config_options.clone())
            .unwrap_or_default()
    } else {
        HashMap::new()
    };

    let mode = if !session_id.is_empty() {
        let state = super::acp_session_state().lock().await;
        state
            .get(session_id)
            .map(|s| s.mode.clone())
            .unwrap_or_default()
    } else {
        String::new()
    };

    let mut all_options = config_options;
    if !mode.is_empty() && !all_options.contains_key("mode") {
        all_options.insert("mode".to_string(), Value::String(mode));
    }

    Ok(serde_json::json!({
        "configOptions": all_options,
        "sessionId": session_id
    }))
}

/// Handle `session/config/favorite/toggle` — toggle the favorite status of a config option value.
///
/// Zed's ACP client supports marking config option values as favorites ("starred").
/// This endpoint toggles the favorite state and returns the updated config options
/// with the new favorite state reflected.
pub async fn session_config_favorite_toggle_payload(
    server: &AcpServer,
    params: Value,
) -> Result<Value> {
    use crate::schema::{SessionConfigId, SessionConfigKind, SessionConfigValueId};

    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let config_id = params
        .get("configId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let value_id = params
        .get("valueId")
        .and_then(Value::as_str)
        .unwrap_or_default();

    if session_id.is_empty() || config_id.is_empty() || value_id.is_empty() {
        return Ok(serde_json::json!({
            "configId": config_id,
            "valueId": value_id,
            "favorited": false,
            "configOptions": [],
        }));
    }

    let favorited = {
        let mut state = super::acp_session_state().lock().await;
        let session = state.entry(session_id.to_string()).or_default();
        let favorites = session
            .favorite_config_values
            .entry(config_id.to_string())
            .or_default();
        if !favorites.insert(value_id.to_string()) {
            // Already favorited — remove it (toggle off)
            favorites.remove(value_id);
            false
        } else {
            true
        }
    };

    // Mark the version timestamp for config option change
    tracing::info!(
        target: "acp::protocol_pack",
        session_id = %session_id,
        config_id = %config_id,
        value_id = %value_id,
        favorited = %favorited,
        "session/config/favorite/toggle"
    );

    // Build updated config options with favorite state
    let current_model = {
        let state = super::acp_session_state().lock().await;
        state.get(session_id).and_then(|s| {
            s.config_options
                .get("model")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
    };
    let mut all_options = super::build_model_config_options(server, current_model.as_deref());

    // Apply favorite state to the returned config options
    for option in &mut all_options {
        if option.id.as_str() == config_id {
            // SessionConfigKind::Select is the only variant
            let SessionConfigKind::Select(ref mut select) = option.kind;
            apply_favorites_to_select(&mut select.options, session_id, config_id).await;
        }
    }

    Ok(serde_json::to_value(
        &crate::schema::SessionConfigFavoriteToggleResponse {
            config_id: SessionConfigId::new(config_id),
            value_id: SessionConfigValueId::new(value_id),
            favorited,
            config_options: all_options,
            meta: None,
        },
    )?)
}

/// Apply favorite state to select options based on stored session favorites.
async fn apply_favorites_to_select(
    options: &mut SessionConfigSelectOptions,
    session_id: &str,
    config_id: &str,
) {
    let favorites = {
        let state = super::acp_session_state().lock().await;
        state
            .get(session_id)
            .and_then(|s| s.favorite_config_values.get(config_id))
            .cloned()
            .unwrap_or_default()
    };

    match options {
        SessionConfigSelectOptions::Ungrouped(ref mut list) => {
            for opt in list.iter_mut() {
                if favorites.contains(opt.value.as_str()) {
                    opt.favorite = true;
                }
            }
        }
        SessionConfigSelectOptions::Grouped(ref mut groups) => {
            for group in groups.iter_mut() {
                for opt in group.options.iter_mut() {
                    if favorites.contains(opt.value.as_str()) {
                        opt.favorite = true;
                    }
                }
            }
        }
    }
}
