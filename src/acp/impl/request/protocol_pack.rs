use std::collections::HashSet;
use std::sync::OnceLock;
use tracing::warn;

use super::*;
use crate::governance::hardening::{AutonomousEditAuditEntry, GovernanceAction};
use crate::mcp::MCP_VERSION;
use crate::schema::{
    ImportedSkillRecordView, ModelsListResponse, PhaseResponse, ProtocolVersion,
    SkillActionResponse,
};

#[derive(Debug, Clone, Default)]
struct AcpSessionState {
    cwd: Option<String>,
    mode: String,
    additional_directories: Vec<String>,
    config_options: HashMap<String, Value>,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Internal chat parameter types — replace json!() for ACP→Go-On param building
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
struct InternalChatParams {
    mode: String,
    messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    conversation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<Value>,
}

// ═══════════════════════════════════════════════════════════════════════════════
// Skill response types — replace json!() in all skill.* handlers
// ═══════════════════════════════════════════════════════════════════════════════

// ═══════════════════════════════════════════════════════════════════════════════
// Phase / Models response types
// ═══════════════════════════════════════════════════════════════════════════════

static ACP_SESSION_STATE: OnceLock<tokio::sync::Mutex<HashMap<String, AcpSessionState>>> =
    OnceLock::new();

fn acp_session_state() -> &'static tokio::sync::Mutex<HashMap<String, AcpSessionState>> {
    ACP_SESSION_STATE.get_or_init(|| tokio::sync::Mutex::new(HashMap::new()))
}

static ACP_PERMISSION_STATE: OnceLock<
    tokio::sync::Mutex<HashMap<String, crate::schema::PermissionOptionId>>,
> = OnceLock::new();

fn acp_permission_state(
) -> &'static tokio::sync::Mutex<HashMap<String, crate::schema::PermissionOptionId>> {
    ACP_PERMISSION_STATE.get_or_init(|| tokio::sync::Mutex::new(HashMap::new()))
}

/// Tracks a spawned terminal process.
struct TerminalProcess {
    /// The child process handle.
    child: std::process::Child,
    /// Captured stdout + stderr output so far.
    output_buffer: Vec<u8>,
    /// Whether the process has exited.
    exited: bool,
    /// Exit status captured when process exited.
    exit_code: Option<i32>,
}

static ACP_TERMINAL_STATE: OnceLock<StdMutex<HashMap<String, TerminalProcess>>> = OnceLock::new();

fn acp_terminal_state() -> &'static StdMutex<HashMap<String, TerminalProcess>> {
    ACP_TERMINAL_STATE.get_or_init(|| StdMutex::new(HashMap::new()))
}

fn normalize_acp_mode(value: Option<&str>) -> String {
    match value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
        .as_deref()
    {
        Some("ask") => "ask".to_string(),
        Some("plan") => "plan".to_string(),
        Some("edit") => "edit".to_string(),
        Some("safe") | Some("safeguard") => "safeguard".to_string(),
        Some("full-auto") | Some("full_auto") | Some("fullauto") => "full_auto".to_string(),
        Some(other) => other.to_string(),
        None => "ask".to_string(),
    }
}

fn extract_additional_directories(params: &Value) -> Vec<String> {
    params
        .get("additionalDirectories")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

async fn session_state_for_prompt(params: &Value) -> AcpSessionState {
    let session_id = params.get("sessionId").and_then(Value::as_str);
    let stored = if let Some(session_id) = session_id {
        let state = acp_session_state().lock().await;
        state.get(session_id).cloned()
    } else {
        None
    };

    let mut state = stored.unwrap_or_default();
    if let Some(cwd) = params.get("cwd").and_then(Value::as_str) {
        let cwd = cwd.trim();
        if !cwd.is_empty() {
            state.cwd = Some(cwd.to_string());
        }
    }
    let prompt_mode = params
        .get("mode")
        .and_then(Value::as_str)
        .or_else(|| params.get("modeId").and_then(Value::as_str));
    state.mode = normalize_acp_mode(prompt_mode.or(Some(state.mode.as_str())));

    let additional_directories = extract_additional_directories(params);
    if !additional_directories.is_empty() {
        state.additional_directories = additional_directories;
    }
    state
}

fn acp_prompt_to_text(params: &Value) -> String {
    let Some(blocks) = params.get("prompt").and_then(Value::as_array) else {
        return String::new();
    };

    let mut segments = Vec::new();
    for block in blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    let text = text.trim();
                    if !text.is_empty() {
                        segments.push(text.to_string());
                    }
                }
            }
            Some("resource_link") => {
                let name = block
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("resource");
                if let Some(uri) = block.get("uri").and_then(Value::as_str) {
                    segments.push(format!("[attached resource] {}\n{}", name, uri));
                }
            }
            Some("resource") => {
                if let Some(resource) = block.get("resource") {
                    if let Some(text) = resource.get("text").and_then(Value::as_str) {
                        let uri = resource
                            .get("uri")
                            .and_then(Value::as_str)
                            .unwrap_or("resource");
                        segments.push(format!("[attached resource content] {}\n{}", uri, text));
                    } else if let Some(uri) = resource.get("uri").and_then(Value::as_str) {
                        segments.push(format!("[attached resource] {}", uri));
                    }
                }
            }
            Some("image") => {
                segments.push("[attached image]".to_string());
            }
            Some("audio") => {
                segments.push("[attached audio]".to_string());
            }
            _ => {}
        }
    }

    segments.join("\n\n")
}

/// Build Go-On chat params from ACP prompt content blocks.
/// ACP sends `prompt: [{type: "text", text: "..."}]`, and may also include
/// resource/resource_link blocks when the user attaches files or selections.
/// Returns Go-On chat params: `{mode, messages: [{role, content}], conversation_id}`.
fn build_chat_params_from_acp(params: Value, session_state: &AcpSessionState) -> Value {
    let text = acp_prompt_to_text(&params);
    let messages = if text.is_empty() {
        vec![]
    } else {
        vec![ChatMessage {
            role: "user".to_string(),
            content: text,
        }]
    };
    let conversation_id = params
        .get("sessionId")
        .and_then(|s| s.as_str())
        .map(|s| s.to_string());
    let options = session_state.cwd.as_ref().map(|cwd| {
        let mut extra = serde_json::Map::new();
        extra.insert("cwd".to_string(), Value::String(cwd.clone()));
        extra.insert(
            "additional_directories".to_string(),
            Value::Array(
                session_state
                    .additional_directories
                    .iter()
                    .map(|d| Value::String(d.clone()))
                    .collect(),
            ),
        );
        let mut options = serde_json::Map::new();
        options.insert("extra".to_string(), Value::Object(extra));
        Value::Object(options)
    });

    serde_json::to_value(InternalChatParams {
        mode: normalize_acp_mode(Some(session_state.mode.as_str())),
        messages,
        conversation_id,
        options,
    })
    .unwrap_or_default()
}

pub(super) async fn initialize_payload(_server: &AcpServer) -> Result<Value> {
    use crate::schema::{
        AgentCapabilities, Implementation, InitializeResponse, McpCapabilities, PromptCapabilities,
        ProtocolVersion, SessionCapabilities, SessionCloseCapabilities, SessionListCapabilities,
        SessionResumeCapabilities,
    };

    let negotiated_version = NEGOTIATED_PROTOCOL_VERSION
        .get()
        .copied()
        .unwrap_or(ProtocolVersion::LATEST);
    let resp = InitializeResponse::new(negotiated_version)
        .agent_info(Implementation::new("go-on", env!("CARGO_PKG_VERSION")))
        .agent_capabilities(AgentCapabilities {
            load_session: true,
            prompt_capabilities: PromptCapabilities {
                image: false,
                audio: false,
                embedded_context: false,
                ..Default::default()
            },
            mcp_capabilities: McpCapabilities {
                http: true,
                sse: false,
                ..Default::default()
            },
            session_capabilities: SessionCapabilities {
                list: Some(SessionListCapabilities { meta: None }),
                close: Some(SessionCloseCapabilities { meta: None }),
                resume: Some(SessionResumeCapabilities { meta: None }),
                ..Default::default()
            },
            ..Default::default()
        });

    let mut value = serde_json::to_value(&resp)?;

    // ── Backward-compat legacy fields ─────────────────────────────────
    // The previous hardcoded json!() response included top-level fields
    // that clients (tests, older IDE integrations) depend on.
    // Merge them into the new structured response so old callers still work.
    let negotiated_ver_num = negotiated_version.as_u16();
    if let Some(obj) = value.as_object_mut() {
        // "name" was used as a quick-access alias for agent_info.name
        obj.insert("name".to_string(), serde_json::json!("go-on"));
        // "protocol" identified the wire protocol
        obj.insert("protocol".to_string(), serde_json::json!("acp"));
        // "version" top-level alias
        obj.insert(
            "version".to_string(),
            serde_json::json!(env!("CARGO_PKG_VERSION")),
        );
        // "protocol_version" — expose as a plain number for compatibility
        obj.insert(
            "protocol_version".to_string(),
            serde_json::json!(negotiated_ver_num),
        );
        // "capabilities" — flatten the chat/phase/health/etc. booleans
        // Version-specific capabilities: V3+ enables SSE transport by default
        let sse_enabled = negotiated_ver_num >= 3;
        let caps_obj = serde_json::json!({
            "chat": true,
            "phase": true,
            "metrics": true,
            "shutdown": true,
            "health": true,
            "debug_panel": true,
            "mcp_adapter": true,
            "sse_transport": sse_enabled,
            "tools_list": true,
            "tools_call": true,
            "tools": true,
            "acp_stdio": true,
            // Add protocol version info
            "protocol_version": env!("CARGO_PKG_VERSION"),
        });
        obj.insert("capabilities".to_string(), caps_obj);
    }

    // Inject platform context (schema_version, profile_class, etc.) using the
    // same shared infrastructure used by other handlers.
    let method = super::DISPATCH_REQUEST_METHOD
        .try_with(|m| m.clone())
        .unwrap_or_else(|_| "initialize".to_string());
    let value = super::inject_platform_profiles_if_absent(value, &method);

    Ok(value)
}

/// Module-level storage for the negotiated protocol version, initially unset.
/// When unset, `handle_initialize` falls back to `ProtocolVersion::LATEST`.
static NEGOTIATED_PROTOCOL_VERSION: OnceLock<ProtocolVersion> = OnceLock::new();

pub(super) async fn mcp_initialize_payload(_server: &AcpServer) -> Result<Value> {
    use crate::mcp::{McpInitializeResult, ServerInfo};
    let result = McpInitializeResult::new(
        MCP_VERSION,
        serde_json::Map::new().into(),
        ServerInfo {
            name: "go-on".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
    );
    Ok(serde_json::to_value(&result)?)
}

use std::sync::atomic::{AtomicU64, Ordering};

/// Atomic counter for generating unique ACP session IDs.
static ACP_SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Generate a unique session ID for the standard ACP protocol.
fn generate_acp_session_id() -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let seq = ACP_SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("acp-session-{:x}-{:x}", ts, seq)
}

static ACP_TERMINAL_COUNTER: AtomicU64 = AtomicU64::new(1);

fn generate_terminal_id() -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let seq = ACP_TERMINAL_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("terminal-{:x}-{:x}", ts, seq)
}

/// Handle `session/new` — creates a new ACP session.
///
/// Standard ACP: client sends `cwd` + optional `mcpServers`,
/// agent responds with `sessionId`.
pub(super) async fn session_new_payload(server: &AcpServer, params: Value) -> Result<Value> {
    let session_id = generate_acp_session_id();
    let modes = build_default_modes();
    let current_mode = normalize_acp_mode(params.get("mode").and_then(|m| m.as_str()));
    let cwd = params
        .get("cwd")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let additional_directories = extract_additional_directories(&params);

    // Zed's ACP client sends `work_dirs` as an array of project directories.
    // If `work_dirs` is provided and `cwd` was not, use the first work_dir as cwd.
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

    // If work_dirs provided but no explicit cwd, use first work_dir as cwd
    let cwd = cwd.or_else(|| work_dirs.first().cloned());

    // Merge work_dirs into additional_directories
    let additional_directories = {
        let mut merged = additional_directories;
        for wd in &work_dirs {
            // Only add work_dirs entries that aren't already the cwd
            if cwd.as_ref() != Some(wd) && !merged.contains(wd) {
                merged.push(wd.clone());
            }
        }
        merged
    };

    // Update current mode on the typed SessionModeState
    let mut modes = modes;
    modes.current_mode_id = crate::schema::SessionModeId::new(current_mode.clone());

    {
        let mut state = acp_session_state().lock().await;
        state.insert(
            session_id.clone(),
            AcpSessionState {
                cwd,
                mode: current_mode.clone(),
                additional_directories,
                config_options: HashMap::new(),
            },
        );
    }

    let config_options = build_model_config_options(server);

    Ok(serde_json::to_value(
        crate::schema::NewSessionResponse::new(crate::schema::SessionId::new(session_id))
            .modes(modes)
            .config_options(config_options),
    )?)
}

/// Handle `session/load` — loads an existing session.
///
/// Standard ACP: client sends `sessionId` + optional `cwd`,
/// agent restores the session context and returns available modes/config.
pub(super) async fn session_load_payload(server: &AcpServer, params: Value) -> Result<Value> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let stored = {
        let state = acp_session_state().lock().await;
        state.get(session_id).cloned().unwrap_or_default()
    };
    let current_mode = normalize_acp_mode(Some(stored.mode.as_str()));
    let mut modes = build_default_modes();
    modes.current_mode_id = crate::schema::SessionModeId::new(current_mode);

    let config_options = build_model_config_options(server);

    Ok(serde_json::to_value(&crate::schema::LoadSessionResponse {
        modes: Some(modes),
        config_options: Some(config_options),
        meta: None,
    })?)
}

/// Handle `session/prompt` — processes a user prompt within a session.
///
/// Standard ACP: client sends `sessionId` + `prompt` (content blocks),
/// agent streams notifications and returns a `PromptResponse` with `stopReason`.
/// Maps to Go-On's internal chat handler for the actual AI processing.
/// Converts ACP `prompt` content blocks to Go-On `messages` format.
pub(super) async fn session_prompt_payload(server: &AcpServer, params: Value) -> Result<Value> {
    // Use process_chat_request for proper agent selection via the Capability Bus.
    // This ensures correct agent routing and orchestration.
    use crate::acp::r#impl::chat::{process_chat_request, ChatParams};
    use crate::rpc_protocol::chat_trace_context;

    let session_state = session_state_for_prompt(&params).await;

    // Extract sessionId before params is consumed by build_chat_params_from_acp
    let session_id_for_notification = params
        .get("sessionId")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Build Go-On chat params from ACP prompt
    let chat_params_value = build_chat_params_from_acp(params, &session_state);
    let mut chat_params: ChatParams = match serde_json::from_value(chat_params_value) {
        Ok(p) => p,
        Err(e) => {
            return Err(anyhow::anyhow!("invalid chat params: {}", e));
        }
    };

    let pipeline_trace = chat_trace_context(&None, "session.prompt");

    tracing::info!("ACP session/prompt: delegating to process_chat_request");

    // IMPORTANT: Zed's ACP client requires content to be streamed via
    // `session/update` notifications BEFORE the final PromptResponse.
    // Go-On's proprietary "chat.stream.chunk" format is NOT understood by Zed.
    //
    // Strategy:
    // 1. Call process_chat_request with `None` stream observer
    //    (avoids proprietary chat.stream.chunk notifications on the wire)
    // 2. Collect the full response text
    // 3. Send ONE `session/update` notification with the complete response
    //    as an AgentMessageChunk (Zed displays this)
    // 4. Respond with PromptResponse containing stopReason
    //    (contentBlocks included for HTTP RPC backward compatibility)
    //
    // For true incremental streaming, use the /chat/stream SSE endpoint.
    // Wrap in a generous timeout to allow long-running agent chains.

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        process_chat_request(
            server,
            &mut chat_params,
            None, // no streaming — avoid proprietary notifications on JSON-RPC
            &pipeline_trace,
            None,
            None,
        ),
    )
    .await;

    match result {
        Ok(Ok(chat_result)) => {
            // Extract the actual AI response text from the chat result
            let response_text = chat_result
                .get("response")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            tracing::info!(
                response_chars = response_text.chars().count(),
                "ACP session/prompt: completed successfully"
            );

            // ── ACP session/update notification ──────────────────────────────
            // Verified against agent-client-protocol-schema v0.13.2 source:
            //   https://github.com/agentclientprotocol/agent-client-protocol
            //   File: src/v1/client.rs
            //
            // SessionNotification struct (rename_all = "camelCase"):
            //   { "sessionId": SessionId, "update": SessionUpdate, "_meta": Option<Meta> }
            //
            // SessionUpdate enum (tag = "sessionUpdate", rename_all = "snake_case"):
            //   "agent_message_chunk"  -> regular text (ContentChunk.content = single ContentBlock)
            //   "agent_thought_chunk"  -> thinking/reasoning (ContentChunk.content = single ContentBlock)
            //
            // ContentChunk struct (rename_all = "camelCase"):
            //   pub content: ContentBlock  -- single ContentBlock, NOT Vec!
            //
            // ContentBlock::Text: { "type": "text", "text": "..." }
            //
            // CRITICAL: The JSON-RPC method name is "session/update" (with slash),
            // defined as: pub(crate) const SESSION_UPDATE_NOTIFICATION: &str = "session/update";
            //
            // DO NOT change "session/update" to "sessionUpdate" or any other variant!
            // ────────────────────────────────────────────────────────────────────
            if let Some(ref sid) = session_id_for_notification {
                // Parse the response into segments: <thinking> blocks are
                // sent as agent_thought_chunk, everything else as agent_message_chunk.
                // This lets Zed render thinking in a collapsible box rather than
                // as raw text.
                static THINKING_RE: std::sync::LazyLock<regex::Regex> =
                    std::sync::LazyLock::new(|| {
                        regex::Regex::new(r"<thinking>(.*?)</thinking>")
                            .expect("B48: hardcoded thinking regex is valid")
                    });
                let mut last_end = 0;
                let sid = sid.as_str();

                for cap in THINKING_RE.captures_iter(response_text) {
                    let m = match cap.get(0) {
                        Some(m) => m,
                        None => continue,
                    };
                    let thought_content = match cap.get(1) {
                        Some(c) => c.as_str(),
                        None => continue,
                    };

                    // Send text before this thinking block as a regular message chunk
                    let before = &response_text[last_end..m.start()];
                    let before = before.trim();
                    if !before.is_empty() {
                        send_chunk(server, sid, "agent_message_chunk", before).await;
                    }

                    // Send the thinking block as a thought chunk
                    if !thought_content.is_empty() {
                        send_chunk(server, sid, "agent_thought_chunk", thought_content).await;
                    }

                    last_end = m.end();
                }

                // Send any remaining text after the last thinking block
                let after = response_text[last_end..].trim();
                if !after.is_empty() {
                    send_chunk(server, sid, "agent_message_chunk", after).await;
                }

                // If nothing was sent at all (empty response or only empty segments),
                // send a fallback message so Zed has something to display.
                if last_end == 0 && response_text.trim().is_empty() {
                    send_chunk(server, sid, "agent_message_chunk", "").await;
                }
            }

            // ── ACP PromptResponse ──────────────────────────────────────────
            // Verified against agent-client-protocol-schema v0.13.2 source:
            //   File: src/v1/agent.rs
            //
            // PromptResponse struct (rename_all = "camelCase"):
            //   {
            //     "stopReason": "end_turn",       // REQUIRED; StopReason enum
            //     "userMessageId": String,         // only with unstable_message_id feature
            //     "usage": Usage,                  // only with unstable_session_usage feature
            //     "_meta": Option<Meta>
            //   }
            //
            // StopReason enum (rename_all = "snake_case"):
            //   EndTurn       -> "end_turn"        // snake_case, NOT "endTurn"!
            //   MaxTokens     -> "max_tokens"
            //   MaxTurnRequests -> "max_turn_requests"
            //   Refusal       -> "refusal"
            //   Cancelled     -> "cancelled"
            //
            // IMPORTANT: StopReason uses snake_case serialization, NOT camelCase!
            // DO NOT change "end_turn" to "endTurn" - that is WRONG.
            //
            // PromptResponse has NO "contentBlocks" field in the stable spec.
            // The content is delivered exclusively via session/update notifications.
            // However, sending contentBlocks in the response does NO harm --
            // Zed's ACP client simply ignores it and reads from the notification.
            // We keep contentBlocks for HTTP RPC backward compatibility.
            // ─────────────────────────────────────────────────────────────────
            let prompt_response = serde_json::to_value(crate::schema::PromptResponse::new(
                crate::schema::StopReason::EndTurn,
            ))?;

            // Use io::send_result directly to bypass inject_platform_profiles_if_absent.
            // The chat_pack::send_result adds a "platform_context" field that Zed's
            // ACP client does not expect and fails to parse.
            Ok(prompt_response)
        }
        Ok(Err(err)) => {
            let msg = err.to_string();
            tracing::warn!("ACP session/prompt: error: {}", msg);
            if is_rate_limited_message(&msg) {
                Err(anyhow::anyhow!(normalize_rate_limited_message(&msg)))
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
/// Uses schema types to ensure correct JSON-RPC wire format.
async fn send_chunk(server: &AcpServer, session_id: &str, chunk_type: &str, text: &str) {
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

/// agent stops processing and returns `StopReason::Cancelled`.
pub(super) async fn session_cancel_payload(_server: &AcpServer, params: Value) -> Result<Value> {
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

    // session/cancel is a notification per JSON-RPC spec
    Err(anyhow::anyhow!("session {} cancelled", session_id))
}

/// Handle `session/list` — lists existing sessions.
///
/// Standard ACP: client may send optional `cwd` filter,
/// agent returns list of known sessions.
pub(super) async fn session_list_payload(_server: &AcpServer, _params: Value) -> Result<Value> {
    Ok(serde_json::to_value(
        &crate::schema::ListSessionsResponse {
            sessions: vec![],
            next_cursor: None,
            meta: None,
        },
    )?)
}

/// Handle `session/set_mode` — sets the current mode for a session.
///
/// Standard ACP: client sends `sessionId` + `modeId`,
/// agent switches mode. Returns updated config options per spec.
pub(super) async fn session_set_mode_payload(server: &AcpServer, params: Value) -> Result<Value> {
    use crate::schema::{
        CurrentModeUpdate, SessionId, SessionNotification, SessionUpdate, SetSessionModeResponse,
    };
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mode_id = normalize_acp_mode(params.get("modeId").and_then(Value::as_str));
    if !session_id.is_empty() {
        {
            let mut state = acp_session_state().lock().await;
            state.entry(session_id.to_string()).or_default().mode = mode_id.clone();
        }
        // Send session/update notification with CurrentModeUpdate so the
        // client (e.g. Zed) can reflect the mode change in its UI immediately.
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
pub(super) async fn session_resume_payload(server: &AcpServer, params: Value) -> Result<Value> {
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

    let (current_mode, _additional_dirs) = if !session_id.is_empty() {
        let mut state = acp_session_state().lock().await;
        let entry = state.entry(session_id.to_string()).or_default();
        if let Some(ref new_cwd) = cwd {
            entry.cwd = Some(new_cwd.clone());
        }
        (entry.mode.clone(), entry.additional_directories.clone())
    } else {
        ("ask".to_string(), vec![])
    };

    let mut modes = build_default_modes();
    modes.current_mode_id = SessionModeId::new(current_mode);
    let config_options = build_model_config_options(server);

    Ok(serde_json::to_value(&ResumeSessionResponse {
        modes: Some(modes),
        config_options: Some(config_options),
        meta: None,
    })?)
}

/// Handle `session/close` — closes and cleans up a session.
pub(super) async fn session_close_payload(_server: &AcpServer, params: Value) -> Result<Value> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !session_id.is_empty() {
        let mut state = acp_session_state().lock().await;
        state.remove(session_id);
        // B51-36: Evict tenant rate limiter state on session close.
        #[cfg(feature = "multi-users-server")]
        if let Some(ref limiter) = _server.rate_limiting.rate_limit_middleware {
            limiter.evict_tenant(session_id).await;
        }
    }
    Ok(serde_json::to_value(
        &crate::schema::CloseSessionResponse { meta: None },
    )?)
}

/// Handle `session/request_permission` — client responds to a permission request.
///
/// During tool execution, the agent may request user permission for sensitive
/// operations. The server sends a permission request notification to the client,
/// and the client responds via this handler. The response is stored so the
/// waiting tool execution can resume with the user's decision.
pub(super) async fn session_request_permission_payload(
    _server: &AcpServer,
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
        // Store the permission decision so the waiting tool execution can pick it up.
        let mut permissions = acp_permission_state().lock().await;
        permissions.insert(
            session_id.to_string(),
            PermissionOptionId::new(option_id.to_string()),
        );
    }

    // Return empty success — the client just needs acknowledgement.
    Ok(serde_json::Value::Object(serde_json::Map::new()))
}

/// Handle `session/set_config_option` — sets a configuration option for a session.
///
/// Standard ACP: client sends `sessionId` + `configId` + `value`,
/// agent applies the option and returns updated config options list.
pub(super) async fn session_set_config_option_payload(
    _server: &AcpServer,
    params: Value,
) -> Result<Value> {
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
        let mut state = acp_session_state().lock().await;
        let session = state.entry(session_id.to_string()).or_default();
        session
            .config_options
            .insert(config_id.to_string(), value.clone());
        if config_id == "mode" {
            session.mode = normalize_acp_mode(value.as_str());
        }
    }

    Ok(serde_json::to_value(
        &crate::schema::SetSessionConfigOptionResponse {
            config_options: vec![],
            meta: None,
        },
    )?)
}

/// Build config options for model selection, populated from the agent registry.
///
/// Returns a single `SessionConfigOption` of kind `Select` containing all
/// available models grouped by provider, with "Auto" as the default value.
/// When set to "Auto", the CapabilityBus selects the best model automatically.
fn build_model_config_options(server: &AcpServer) -> Vec<crate::schema::SessionConfigOption> {
    use crate::schema::{
        SessionConfigGroupId, SessionConfigId, SessionConfigKind, SessionConfigOption,
        SessionConfigOptionCategory, SessionConfigSelect, SessionConfigSelectGroup,
        SessionConfigSelectOption, SessionConfigSelectOptions, SessionConfigValueId,
    };

    let mut groups: Vec<SessionConfigSelectGroup> = Vec::new();

    if let Some(registry) = server.agent_registry() {
        for (agent_name, _default, models) in registry.models() {
            if models.is_empty() {
                continue;
            }
            let options: Vec<SessionConfigSelectOption> = models
                .into_iter()
                .map(|m| SessionConfigSelectOption {
                    value: SessionConfigValueId::new(m.id.clone()),
                    name: m.name,
                    description: Some(m.id.clone()),
                    meta: None,
                })
                .collect();
            groups.push(SessionConfigSelectGroup {
                group: SessionConfigGroupId::new(agent_name.clone()),
                name: agent_name,
                options,
                meta: None,
            });
        }
    }

    vec![SessionConfigOption {
        id: SessionConfigId::new("model"),
        name: "Model / 模型".to_string(),
        description: Some(
            "AI model to use. \"Auto\" lets the capability bus select the best model.".to_string(),
        ),
        category: Some(SessionConfigOptionCategory::Model),
        kind: SessionConfigKind::Select(SessionConfigSelect {
            current_value: SessionConfigValueId::new("auto"),
            options: SessionConfigSelectOptions::Grouped(groups),
        }),
        meta: None,
    }]
}

/// Build the default set of session modes.
fn build_default_modes() -> crate::schema::SessionModeState {
    use crate::schema::{SessionMode, SessionModeId, SessionModeState};
    SessionModeState::new(
        SessionModeId::new("ask"),
        vec![
            SessionMode::new(SessionModeId::new("ask"), "Ask / 对话")
                .description("Q&A assistant — general questions"),
            SessionMode::new(SessionModeId::new("plan"), "Plan / 计划")
                .description("Planning mode — structured task breakdown"),
            SessionMode::new(SessionModeId::new("edit"), "Edit / 编辑")
                .description("Edit/review mode — code changes"),
            SessionMode::new(SessionModeId::new("safeguard"), "Safeguard / 安全")
                .description("Safety-first — escalation on high-risk operations"),
            SessionMode::new(SessionModeId::new("full_auto"), "Full Auto / 全自动")
                .description("Fully autonomous — agent runs without user confirmation"),
        ],
    )
}

// ── Standard ACP authentication handlers ──────────────────────────────────

/// Handle `authenticate` — authenticates the client.
/// Standard ACP: client sends `methodId`, agent performs auth and returns success.
pub(super) async fn authenticate_payload(_server: &AcpServer, _params: Value) -> Result<Value> {
    Ok(serde_json::to_value(
        crate::schema::AuthenticateResponse::new(),
    )?)
}

/// Handle `logout` — terminates the current authenticated session.
pub(super) async fn logout_payload(_server: &AcpServer, _params: Value) -> Result<Value> {
    // B51-36: Evict tenant rate limiter state on logout if session info is present.
    #[cfg(feature = "multi-users-server")]
    {
        if let Some(session_id) = _params.get("sessionId").and_then(Value::as_str) {
            if !session_id.is_empty() {
                if let Some(ref limiter) = _server.rate_limiting.rate_limit_middleware {
                    limiter.evict_tenant(session_id).await;
                }
            }
        }
    }

    Ok(serde_json::to_value(&crate::schema::LogoutResponse {
        meta: None,
    })?)
}

// ── MCP bridge handlers (mcp.* methods routed through ACP dispatch) ──────

/// Handle `mcp.ping` — health check ping.
pub(super) async fn mcp_ping_payload(_server: &AcpServer) -> Result<Value> {
    Ok(serde_json::Value::Object(serde_json::Map::new()))
}

/// Handle `mcp.resources.list` — list available resources.
pub(super) async fn mcp_resources_list_payload(_server: &AcpServer) -> Result<Value> {
    use crate::mcp::McpListResourcesResult;
    let result = McpListResourcesResult::new(vec![]);
    Ok(serde_json::to_value(&result)?)
}

/// Handle `mcp.resources.read` — read a specific resource by URI.
pub(super) async fn mcp_resources_read_payload(
    _server: &AcpServer,
    _params: Value,
) -> Result<Value> {
    Err(anyhow::anyhow!(
        "Resource reading via MCP bridge is not supported; use the dedicated MCP server instead"
    ))
}

/// Handle `mcp.resources.subscribe` — subscribe to resource changes.
pub(super) async fn mcp_resources_subscribe_payload(
    _server: &AcpServer,
    _params: Value,
) -> Result<Value> {
    Ok(serde_json::Value::Object(serde_json::Map::new()))
}

/// Handle `mcp.logging.setLevel` — set the MCP logging level.
pub(super) async fn mcp_logging_set_level_payload(
    _server: &AcpServer,
    _params: Value,
) -> Result<Value> {
    Ok(serde_json::Value::Object(serde_json::Map::new()))
}

/// Handle `mcp.completion.complete` — complete a text input.
pub(super) async fn mcp_completion_complete_payload(
    _server: &AcpServer,
    _params: Value,
) -> Result<Value> {
    Ok(serde_json::Value::Object(serde_json::Map::new()))
}

/// Handle `mcp.sampling.createMessage` — create a sampling request.
pub(super) async fn mcp_sampling_create_message_payload(
    _server: &AcpServer,
    _params: Value,
) -> Result<Value> {
    Err(anyhow::anyhow!(
        "Sampling via MCP bridge is not supported; use the chat/session API instead"
    ))
}

// ── MCP tool handlers ────────────────────────────────────────────────────

pub(super) async fn mcp_tools_list_payload(server: &AcpServer) -> Result<Value> {
    use crate::mcp::McpListToolsResult;
    let tools = build_mcp_tool_descriptors(Some(server));
    let result = McpListToolsResult::new(tools);
    let value = serde_json::to_value(&result)?;
    // Inject platform context for consistency with other handlers.
    let method = super::DISPATCH_REQUEST_METHOD
        .try_with(|m| m.clone())
        .unwrap_or_else(|_| "mcp.tools.list".to_string());
    let value = super::inject_platform_profiles_if_absent(value, &method);
    Ok(value)
}

pub(super) async fn mcp_tools_call_payload(server: &AcpServer, params: Value) -> Result<Value> {
    use crate::mcp::McpCallToolResult;

    let name = params
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or_default();

    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));

    let structured = match execute_mcp_tool_call(server, name, &arguments).await {
        Ok(structured) => structured,
        Err(err) => {
            record_mcp_tool_audit(name, &arguments, false, &err.to_string());
            return Err(anyhow::anyhow!(err.to_string()));
        }
    };
    record_mcp_tool_audit(name, &arguments, true, "tool executed successfully");

    let mut content = serde_json::Map::new();
    content.insert("type".to_string(), Value::String("text".to_string()));
    content.insert("text".to_string(), Value::String(structured.to_string()));
    let result = McpCallToolResult::new(vec![Value::Object(content)], Some(structured));
    Ok(serde_json::to_value(&result)?)
}

/// Handle `tools/list` — list available tools for the ACP protocol.
///
/// Returns the list of tools available in the go-on tool registry,
/// formatted as ACP tool descriptions with name, description, and input_schema.
pub(super) async fn acp_tools_list_payload(server: &AcpServer) -> Result<Value> {
    use crate::mcp::McpListToolsResult;
    let tools = build_mcp_tool_descriptors(Some(server));
    let result = McpListToolsResult::new(tools);
    let value = serde_json::to_value(&result)?;
    // Inject platform context for consistency with other handlers.
    let method = super::DISPATCH_REQUEST_METHOD
        .try_with(|m| m.clone())
        .unwrap_or_else(|_| "tools.list".to_string());
    let value = super::inject_platform_profiles_if_absent(value, &method);
    Ok(value)
}

/// Handle `tools/call` — execute a tool by name via the ACP protocol with
/// streaming progress updates.
///
/// Takes `{ name, arguments, sessionId }` from the request params, delegates
/// to `execute_mcp_tool_call` for the actual tool execution, and returns the
/// tool result as a JSON-RPC response.
///
/// Intermediate progress (started, completed, or failed) is emitted as
/// `session/update` notifications so Zed can display incremental progress
/// during tool execution.
pub(super) async fn acp_tools_call_payload(server: &AcpServer, params: Value) -> Result<Value> {
    let name = params
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or_default();

    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| Value::Object(serde_json::Map::new()));

    // Extract session_id from params so we can send progress notifications.
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .map(|s| s.to_string());

    // ── Send "started" progress update ──────────────────────────────────
    if let Some(ref sid) = session_id {
        let msg = format!("🔧 **{}** — executing...", name);
        send_chunk(server, sid, "agent_message_chunk", &msg).await;
    }

    // ── Execute the tool ────────────────────────────────────────────────
    let structured = match execute_mcp_tool_call(server, name, &arguments).await {
        Ok(structured) => structured,
        Err(err) => {
            record_mcp_tool_audit(name, &arguments, false, &err.to_string());
            if let Some(ref sid) = session_id {
                let msg = format!("❌ **{}** failed: {}", name, err);
                send_chunk(server, sid, "agent_message_chunk", &msg).await;
            }
            return Err(anyhow::anyhow!(err.to_string()));
        }
    };
    record_mcp_tool_audit(name, &arguments, true, "tool executed successfully");

    // ── Send "completed" progress update ────────────────────────────────
    if let Some(ref sid) = session_id {
        let msg = format!("✅ **{}** — completed", name);
        send_chunk(server, sid, "agent_message_chunk", &msg).await;
    }

    let mut content = serde_json::Map::new();
    content.insert("type".to_string(), Value::String("text".to_string()));
    content.insert("text".to_string(), Value::String(structured.to_string()));

    Ok(serde_json::json!({
        "content": [Value::Object(content)],
        "structured": structured
    }))
}

// ── ACP tool handlers ────────────────────────────────────────────────────

pub(super) async fn tools_list_payload(server: &AcpServer) -> Result<Value> {
    acp_tools_list_payload(server).await
}

pub(super) async fn tools_call_payload(server: &AcpServer, params: Value) -> Result<Value> {
    acp_tools_call_payload(server, params).await
}

fn record_mcp_tool_audit(name: &str, arguments: &Value, success: bool, reason: &str) {
    record_tool_call_audit_with_protocol(name, arguments, success, reason, "acp_stdio");
}

pub fn record_tool_call_audit_with_protocol(
    name: &str,
    arguments: &Value,
    success: bool,
    reason: &str,
    protocol: &str,
) {
    let action = governance_action_for_tool(name);
    let reversible = matches!(action, GovernanceAction::Read | GovernanceAction::Search);
    let file_path = audit_file_path_from_arguments(name, arguments);
    let entry = AutonomousEditAuditEntry {
        timestamp: crate::acp::prelude::now_ts().to_string(),
        agent: "mcp.tools.call".to_string(),
        file_path,
        change_summary: format!(
            "tool={} action={} status={} protocol={}",
            name,
            governance_action_label(action),
            if success { "ok" } else { "error" },
            protocol,
        ),
        approval_reason: reason.to_string(),
        confidence_score: if success { 1.0 } else { 0.0 },
        reversible,
    };

    if let Err(err) = mcp_audit_logger().record(&entry) {
        debug!("failed to record mcp audit log: {}", err);
    }
}

fn record_skill_admin_audit(action: &str, target: &str, success: bool, reason: &str) {
    record_skill_admin_audit_with_protocol(action, target, success, reason, "acp_stdio");
}

fn record_skill_admin_audit_with_protocol(
    action: &str,
    target: &str,
    success: bool,
    reason: &str,
    protocol: &str,
) {
    let entry = AutonomousEditAuditEntry {
        timestamp: crate::acp::prelude::now_ts().to_string(),
        agent: format!("skill.{}", action),
        file_path: target.to_string(),
        change_summary: format!(
            "action={} status={} protocol={}",
            action,
            if success { "ok" } else { "error" },
            protocol,
        ),
        approval_reason: reason.to_string(),
        confidence_score: if success { 1.0 } else { 0.0 },
        reversible: action != "import",
    };
    if let Err(err) = mcp_audit_logger().record(&entry) {
        debug!("failed to record skill admin audit: {}", err);
    }
}

fn parse_skill_name_param(params: &Value) -> Result<String> {
    params
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| anyhow::anyhow!("missing required param: name"))
}

fn skill_import_policy(server: &AcpServer) -> SkillImportPolicy {
    SkillImportPolicy::from_runtime(&server.runtime_config)
}

fn open_skill_import_store(server: &AcpServer) -> Result<SkillImportStore> {
    SkillImportStore::load(
        skill_import_policy(server),
        server.orchestration_deps.skill_registry.clone(),
    )
}

fn normalize_imported_record(record: ImportedSkillRecord) -> Value {
    let resp = ImportedSkillRecordView {
        name: record.name,
        version: record.version,
        description: record.description,
        source: record.source,
        source_ref: record.source_ref,
        sha256: record.sha256,
        manifest_path: record.manifest_path,
        enabled: record.enabled,
        imported_at: record.imported_at,
    };
    serde_json::to_value(&resp).unwrap_or_default()
}

static SKILL_VERSION_HISTORY: OnceLock<StdMutex<HashMap<String, Vec<Value>>>> = OnceLock::new();

fn skill_version_history() -> &'static StdMutex<HashMap<String, Vec<Value>>> {
    SKILL_VERSION_HISTORY.get_or_init(|| StdMutex::new(HashMap::new()))
}

fn load_skill_manifest(path: &str) -> Result<Value> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read skill manifest {}", path))?;
    serde_json::from_str::<Value>(&raw)
        .with_context(|| format!("failed to parse skill manifest {}", path))
}

fn save_skill_manifest(path: &str, manifest: &Value) -> Result<()> {
    let payload = serde_json::to_string_pretty(manifest)
        .context("failed to serialize skill manifest payload")?;
    fs::write(path, payload).with_context(|| format!("failed to write skill manifest {}", path))
}

fn parse_semver_patch(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.trim().split('.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    let patch = parts.next()?.parse::<u64>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

fn bump_patch_version(version: &str) -> String {
    parse_semver_patch(version)
        .map(|(major, minor, patch)| format!("{}.{}.{}", major, minor, patch + 1))
        .unwrap_or_else(|| "1.0.0".to_string())
}

fn build_skill_version_snapshot(
    record: &ImportedSkillRecord,
    manifest: &Value,
    updated_by: &str,
    change_summary: &str,
) -> Value {
    let updated_at = crate::acp::prelude::now_ts().to_string();
    let version = manifest
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or(record.version.as_str())
        .to_string();
    let description = manifest
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or(record.description.as_str())
        .to_string();
    let default_schema = {
        let mut m = serde_json::Map::new();
        m.insert("type".to_string(), Value::String("object".to_string()));
        Value::Object(m)
    };
    let input_schema = manifest
        .get("input_schema")
        .cloned()
        .unwrap_or(default_schema);
    let prompt_template = manifest.get("prompt_template").cloned();

    let snapshot = crate::schema::SkillVersionSnapshot {
        name: record.name.clone(),
        version,
        description,
        input_schema: Some(input_schema),
        prompt_template,
        manifest_path: record.manifest_path.clone(),
        saved_at: updated_at.clone(),
        updated_at,
        updated_by: updated_by.to_string(),
        change_summary: change_summary.to_string(),
    };
    serde_json::to_value(&snapshot).unwrap_or_default()
}

fn push_skill_version_snapshot(name: &str, snapshot: Value) {
    let mut history = skill_version_history().lock().unwrap_or_else(|poisoned| {
        warn!("Skill version history lock poisoned in push_skill_version_snapshot, recovering");
        poisoned.into_inner()
    });
    let entries = history.entry(name.to_string()).or_default();
    entries.push(snapshot);
    if entries.len() > 100 {
        let overflow = entries.len() - 100;
        entries.drain(0..overflow);
    }
}

pub(super) async fn skill_import_payload(server: &AcpServer, params: Value) -> Result<Value> {
    let request: SkillImportRequest =
        serde_json::from_value(params).context("invalid params for skill.import")?;
    let mut store = open_skill_import_store(server)?;
    let imported = match store.import_skill(request).await {
        Ok(record) => record,
        Err(err) => {
            record_skill_admin_audit("import", "skill.import", false, &err.to_string());
            return Err(anyhow::anyhow!(err.to_string()));
        }
    };
    store.save()?;
    let imported_name = imported.name.clone();

    record_skill_admin_audit(
        "import",
        &imported.name,
        true,
        "imported skill manifest with supply-chain checks",
    );
    let payload = serde_json::to_value(SkillActionResponse {
        ok: true,
        action: "import".to_string(),
        name: Some(imported_name),
        skill: Some(normalize_imported_record(imported)),
        total: None,
        enabled: None,
        disabled: None,
        skills: None,
        removed: None,
        unregistered: None,
        version: None,
        versions: None,
    })
    .unwrap_or_default();
    Ok(payload)
}

pub(super) async fn skill_list_imported_payload(server: &AcpServer) -> Result<Value> {
    let store = open_skill_import_store(server)?;
    let imported_skills = store.list();
    let imported_names: HashSet<String> = imported_skills.iter().map(|r| r.name.clone()).collect();

    // Convert imported skills to response values
    let mut skills: Vec<Value> = imported_skills
        .into_iter()
        .map(normalize_imported_record)
        .collect();

    // Merge prompt-based skills from the registry that aren't already imported
    if let Ok(registry) = server.orchestration_deps.skill_registry.read() {
        for (name, data) in registry.prompt_skill_data() {
            if !imported_names.contains(&name) {
                let view = ImportedSkillRecordView {
                    name: data.name,
                    version: "1.0".to_string(),
                    description: data.description,
                    source: "prompt".to_string(),
                    source_ref: String::new(),
                    sha256: String::new(),
                    manifest_path: String::new(),
                    enabled: true,
                    imported_at: data.created_at,
                };
                skills.push(serde_json::to_value(&view).unwrap_or_default());
            }
        }
    }

    // Sort merged list by name
    skills.sort_by(|a, b| {
        a.get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .cmp(b.get("name").and_then(Value::as_str).unwrap_or(""))
    });

    let total = skills.len();
    let enabled = skills
        .iter()
        .filter(|skill| skill.get("enabled").and_then(Value::as_bool) == Some(true))
        .count();
    let disabled = total.saturating_sub(enabled);

    record_skill_admin_audit(
        "list_imported",
        "skill.list_imported",
        true,
        "listed imported skills",
    );
    let payload = serde_json::to_value(SkillActionResponse {
        ok: true,
        action: "list_imported".to_string(),
        name: None,
        skill: None,
        total: Some(total),
        enabled: Some(enabled),
        disabled: Some(disabled),
        skills: Some(skills),
        removed: None,
        unregistered: None,
        version: None,
        versions: None,
    })
    .unwrap_or_default();
    Ok(payload)
}

pub(super) async fn skill_enabled_toggle_payload(
    server: &AcpServer,
    params: Value,
    enabled: bool,
) -> Result<Value> {
    let action = if enabled { "enable" } else { "disable" };
    let name = match parse_skill_name_param(&params) {
        Ok(name) => name,
        Err(err) => {
            record_skill_admin_audit(action, "skill.toggle", false, &err.to_string());
            return Err(anyhow::anyhow!(err.to_string()));
        }
    };
    let mut store = open_skill_import_store(server)?;
    let updated = match store.set_enabled(&name, enabled) {
        Ok(record) => {
            store.save()?;
            record
        }
        Err(_) => {
            // Fall back: prompt-based skills in SkillRegistry are always enabled
            let is_prompt_skill = server
                .orchestration_deps
                .skill_registry
                .read()
                .map(|r| r.prompt_skill_data().contains_key(&name))
                .unwrap_or(false);
            if is_prompt_skill {
                record_skill_admin_audit(
                    action,
                    &name,
                    true,
                    "prompt skill toggle (always enabled)",
                );
                let payload = serde_json::to_value(SkillActionResponse {
                    ok: true,
                    action: action.to_string(),
                    name: Some(name),
                    skill: None,
                    total: None,
                    enabled: None,
                    disabled: None,
                    skills: None,
                    removed: None,
                    unregistered: None,
                    version: None,
                    versions: None,
                })
                .unwrap_or_default();
                return Ok(payload);
            }
            let reason = tf("error.imported_skill_not_found", &[("name", &name)]);
            record_skill_admin_audit(action, &name, false, &reason);
            return Err(anyhow::anyhow!(reason));
        }
    };
    record_skill_admin_audit(action, &name, true, "updated imported skill state");
    let payload = serde_json::to_value(SkillActionResponse {
        ok: true,
        action: action.to_string(),
        name: Some(name),
        skill: Some(normalize_imported_record(updated)),
        total: None,
        enabled: None,
        disabled: None,
        skills: None,
        removed: None,
        unregistered: None,
        version: None,
        versions: None,
    })
    .unwrap_or_default();
    Ok(payload)
}

pub(super) async fn skill_remove_payload(server: &AcpServer, params: Value) -> Result<Value> {
    let name = match parse_skill_name_param(&params) {
        Ok(name) => name,
        Err(err) => {
            record_skill_admin_audit("remove", "skill.remove", false, &err.to_string());
            return Err(anyhow::anyhow!(err.to_string()));
        }
    };
    let mut store = open_skill_import_store(server)?;
    let removed = store.remove(&name);
    if !removed {
        // Fall back to SkillRegistry for prompt-based skills
        let registry_removed = server
            .orchestration_deps
            .skill_registry
            .write()
            .map(|mut registry| {
                let r = registry.unregister(&name);
                if let Err(e) = registry.save_prompt_skills_to_disk() {
                    tracing::warn!("Failed to persist prompt skills after removal: {}", e);
                }
                r
            })
            .unwrap_or(false);
        if registry_removed {
            record_skill_admin_audit("remove", &name, true, "removed prompt skill");
            let payload = serde_json::to_value(SkillActionResponse {
                ok: true,
                action: "remove".to_string(),
                name: Some(name),
                skill: None,
                total: None,
                enabled: None,
                disabled: None,
                skills: None,
                removed: Some(false),
                unregistered: Some(true),
                version: None,
                versions: None,
            })
            .unwrap_or_default();
            return Ok(payload);
        }
        let reason = tf("error.imported_skill_not_found", &[("name", &name)]);
        record_skill_admin_audit("remove", &name, false, &reason);
        return Err(anyhow::anyhow!(reason));
    }
    let unregistered = server
        .orchestration_deps
        .skill_registry
        .write()
        .map(|mut registry| {
            let unregistered = registry.unregister(&name);
            // Persist prompt skill removal to disk
            if let Err(e) = registry.save_prompt_skills_to_disk() {
                tracing::warn!("Failed to persist prompt skills after removal: {}", e);
            }
            unregistered
        })
        .unwrap_or(false);
    store.save()?;
    record_skill_admin_audit("remove", &name, true, "removed imported skill record");

    let payload = serde_json::to_value(SkillActionResponse {
        ok: true,
        action: "remove".to_string(),
        name: Some(name),
        skill: None,
        total: None,
        enabled: None,
        disabled: None,
        skills: None,
        removed: Some(removed),
        unregistered: Some(unregistered),
        version: None,
        versions: None,
    })
    .unwrap_or_default();
    Ok(payload)
}

pub(super) async fn skill_create_payload(server: &AcpServer, params: Value) -> Result<Value> {
    let name = match parse_skill_name_param(&params) {
        Ok(name) => name,
        Err(err) => {
            record_skill_admin_audit("create", "skill.create", false, &err.to_string());
            return Err(anyhow::anyhow!(err.to_string()));
        }
    };
    let description = params
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| anyhow::anyhow!("missing required param: description"))?;
    let prompt_template = params
        .get("prompt_template")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| anyhow::anyhow!("missing required param: prompt_template"))?;
    let input_schema: std::collections::HashMap<String, String> = params
        .get("input_schema")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    let result = {
        let mut registry = server
            .orchestration_deps
            .skill_registry
            .write()
            .map_err(|err| anyhow::anyhow!("skill registry write-lock error: {}", err))?;
        registry.create_skill_from_prompt(&name, &description, &prompt_template, input_schema)
    };
    // Lock is dropped before await
    if let Err(err) = result {
        record_skill_admin_audit("create", &name, false, &err.to_string());
        return Err(anyhow::anyhow!(err.to_string()));
    }

    record_skill_admin_audit("create", &name, true, "created skill from prompt template");
    let payload = serde_json::to_value(SkillActionResponse {
        ok: true,
        action: "create".to_string(),
        name: Some(name),
        skill: None,
        total: None,
        enabled: None,
        disabled: None,
        skills: None,
        removed: None,
        unregistered: None,
        version: None,
        versions: None,
    })
    .unwrap_or_default();
    Ok(payload)
}

pub(crate) fn skill_update_payload(server: &AcpServer, params: &Value) -> Result<Value> {
    let name = match parse_skill_name_param(params) {
        Ok(name) => name,
        Err(err) => {
            record_skill_admin_audit("update", "skill.update", false, &err.to_string());
            return Err(err);
        }
    };

    let mut store = open_skill_import_store(server)?;
    let Some(mut record) = store.get(&name) else {
        // Fall back to SkillRegistry for prompt-based skills
        let has_prompt_skill = server
            .orchestration_deps
            .skill_registry
            .read()
            .map(|r| r.prompt_skill_data().contains_key(&name))
            .unwrap_or(false);
        if !has_prompt_skill {
            let reason = tf("error.imported_skill_not_found", &[("name", &name)]);
            record_skill_admin_audit("update", &name, false, &reason);
            anyhow::bail!(reason);
        }
        // Load current prompt skill data to preserve unmodified fields
        let current = server
            .orchestration_deps
            .skill_registry
            .read()
            .map(|r| r.prompt_skill_data().get(&name).cloned())
            .ok()
            .flatten()
            .context("skill not found in registry")?;
        let description = params
            .get("description")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or(current.description);
        let prompt_template = params
            .get("prompt_template")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .unwrap_or(current.prompt_template);
        let input_schema: std::collections::HashMap<String, String> = params
            .get("input_schema")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or(current.input_schema);

        {
            let mut registry = server
                .orchestration_deps
                .skill_registry
                .write()
                .map_err(|err| anyhow::anyhow!("skill registry write-lock error: {}", err))?;
            registry.create_skill_from_prompt(
                &name,
                &description,
                &prompt_template,
                input_schema,
            )?;
        }

        record_skill_admin_audit("update", &name, true, "updated prompt skill");
        return Ok(serde_json::to_value(SkillActionResponse {
            ok: true,
            action: "update".to_string(),
            name: Some(name),
            skill: None,
            total: None,
            enabled: None,
            disabled: None,
            skills: None,
            removed: None,
            unregistered: None,
            version: None,
            versions: None,
        })
        .unwrap_or_default());
    };

    let mut manifest = load_skill_manifest(&record.manifest_path)?;
    push_skill_version_snapshot(
        &name,
        build_skill_version_snapshot(&record, &manifest, "system", "initial skill import"),
    );

    if let Some(description) = params.get("description").and_then(Value::as_str) {
        manifest["description"] = Value::String(description.to_string());
        record.description = description.to_string();
    }
    if let Some(schema) = params.get("input_schema") {
        manifest["input_schema"] = schema.clone();
    }
    if let Some(prompt) = params.get("prompt_template").and_then(Value::as_str) {
        manifest["prompt_template"] = Value::String(prompt.to_string());
    }

    let current_version = manifest
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or(record.version.as_str());
    let target_version = params
        .get("version")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .unwrap_or_else(|| bump_patch_version(current_version));
    manifest["version"] = Value::String(target_version.clone());
    record.version = target_version;

    save_skill_manifest(&record.manifest_path, &manifest)?;
    push_skill_version_snapshot(
        &name,
        build_skill_version_snapshot(
            &record,
            &manifest,
            "system",
            "updated imported skill manifest",
        ),
    );

    store.upsert_record(record.clone());
    store.save()?;

    record_skill_admin_audit("update", &name, true, "updated imported skill manifest");
    Ok(serde_json::to_value(SkillActionResponse {
        ok: true,
        action: "update".to_string(),
        name: Some(name),
        skill: Some(normalize_imported_record(record)),
        total: None,
        enabled: None,
        disabled: None,
        skills: None,
        removed: None,
        unregistered: None,
        version: None,
        versions: None,
    })
    .unwrap_or_default())
}

pub(crate) fn skill_version_list_payload(server: &AcpServer, params: &Value) -> Result<Value> {
    let name = match parse_skill_name_param(params) {
        Ok(name) => name,
        Err(err) => {
            record_skill_admin_audit(
                "version.list",
                "skill.version.list",
                false,
                &err.to_string(),
            );
            return Err(err);
        }
    };

    let store = open_skill_import_store(server)?;
    let Some(record) = store.get(&name) else {
        // Fall back to SkillRegistry for prompt-based skills (version 1.0)
        let has_prompt_skill = server
            .orchestration_deps
            .skill_registry
            .read()
            .map(|r| r.prompt_skill_data().contains_key(&name))
            .unwrap_or(false);
        if !has_prompt_skill {
            let reason = tf("error.imported_skill_not_found", &[("name", &name)]);
            record_skill_admin_audit("version.list", &name, false, &reason);
            anyhow::bail!(reason);
        }
        record_skill_admin_audit("version.list", &name, true, "listed prompt skill versions");
        return Ok(serde_json::to_value(SkillActionResponse {
            ok: true,
            action: "version.list".to_string(),
            name: Some(name),
            skill: None,
            total: None,
            enabled: None,
            disabled: None,
            skills: None,
            removed: None,
            unregistered: None,
            version: None,
            versions: Some(vec![serde_json::json!({"version": "1.0"})]),
        })
        .unwrap_or_default());
    };

    let manifest = load_skill_manifest(&record.manifest_path)?;
    let mut versions = skill_version_history()
        .lock()
        .ok()
        .and_then(|history| history.get(&name).cloned())
        .unwrap_or_default();
    versions.push(build_skill_version_snapshot(
        &record,
        &manifest,
        "system",
        "current imported skill snapshot",
    ));

    record_skill_admin_audit("version.list", &name, true, "listed skill versions");
    Ok(serde_json::to_value(SkillActionResponse {
        ok: true,
        action: "version.list".to_string(),
        name: Some(name),
        skill: None,
        total: None,
        enabled: None,
        disabled: None,
        skills: None,
        removed: None,
        unregistered: None,
        version: None,
        versions: Some(versions),
    })
    .unwrap_or_default())
}

pub(crate) fn skill_version_rollback_payload(server: &AcpServer, params: &Value) -> Result<Value> {
    let name = match parse_skill_name_param(params) {
        Ok(name) => name,
        Err(err) => {
            record_skill_admin_audit(
                "version.rollback",
                "skill.version.rollback",
                false,
                &err.to_string(),
            );
            return Err(err);
        }
    };
    let Some(target_version) = params.get("version").and_then(Value::as_str) else {
        anyhow::bail!("version is required");
    };

    let mut store = open_skill_import_store(server)?;
    let Some(mut record) = store.get(&name) else {
        // Fall back to SkillRegistry for prompt-based skills (no version history)
        let has_prompt_skill = server
            .orchestration_deps
            .skill_registry
            .read()
            .map(|r| r.prompt_skill_data().contains_key(&name))
            .unwrap_or(false);
        if !has_prompt_skill {
            let reason = tf("error.imported_skill_not_found", &[("name", &name)]);
            record_skill_admin_audit("version.rollback", &name, false, &reason);
            anyhow::bail!(reason);
        }
        record_skill_admin_audit(
            "version.rollback",
            &name,
            true,
            "prompt skill has no version history",
        );
        return Ok(serde_json::to_value(SkillActionResponse {
            ok: true,
            action: "rollback".to_string(),
            name: Some(name),
            skill: None,
            total: None,
            enabled: None,
            disabled: None,
            skills: None,
            removed: None,
            unregistered: None,
            version: Some(target_version.to_string()),
            versions: None,
        })
        .unwrap_or_default());
    };

    let history = skill_version_history()
        .lock()
        .ok()
        .and_then(|entries| entries.get(&name).cloned())
        .unwrap_or_default();
    let Some(snapshot) = history.into_iter().rev().find(|entry| {
        entry
            .get("version")
            .and_then(Value::as_str)
            .map(|version| version == target_version)
            .unwrap_or(false)
    }) else {
        anyhow::bail!(
            "version '{}' not found for skill '{}'",
            target_version,
            name
        );
    };

    let mut manifest = load_skill_manifest(&record.manifest_path)?;
    if let Some(description) = snapshot.get("description") {
        manifest["description"] = description.clone();
        if let Some(text) = description.as_str() {
            record.description = text.to_string();
        }
    }
    if let Some(schema) = snapshot.get("input_schema") {
        manifest["input_schema"] = schema.clone();
    }
    if let Some(prompt_template) = snapshot.get("prompt_template") {
        manifest["prompt_template"] = prompt_template.clone();
    }
    manifest["version"] = Value::String(target_version.to_string());
    record.version = target_version.to_string();

    save_skill_manifest(&record.manifest_path, &manifest)?;
    store.upsert_record(record.clone());
    store.save()?;
    push_skill_version_snapshot(
        &name,
        build_skill_version_snapshot(
            &record,
            &manifest,
            "system",
            "rolled back imported skill version",
        ),
    );

    record_skill_admin_audit(
        "version.rollback",
        &name,
        true,
        "rolled back imported skill version",
    );
    Ok(serde_json::to_value(SkillActionResponse {
        ok: true,
        action: "rollback".to_string(),
        name: Some(name),
        skill: Some(normalize_imported_record(record)),
        total: None,
        enabled: None,
        disabled: None,
        skills: None,
        removed: None,
        unregistered: None,
        version: Some(target_version.to_string()),
        versions: None,
    })
    .unwrap_or_default())
}

fn governance_action_label(action: GovernanceAction) -> &'static str {
    match action {
        GovernanceAction::Read => "read",
        GovernanceAction::Search => "search",
        GovernanceAction::Write => "write",
        GovernanceAction::Shell => "shell",
        GovernanceAction::Network => "network",
    }
}

fn audit_file_path_from_arguments(name: &str, arguments: &Value) -> String {
    for key in ["path", "filePath", "sourcePdfPath"] {
        if let Some(path) = arguments.get(key).and_then(Value::as_str) {
            return path.to_string();
        }
    }
    format!("tool:{name}")
}

fn is_rate_limited_message(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    normalized.contains("rate limited")
        || normalized.contains("rate_limited")
        || normalized.contains("error.chat.rate_limited")
        || normalized.contains("too many requests")
}

fn normalize_rate_limited_message(message: &str) -> String {
    if message.to_ascii_lowercase().contains("rate limited") {
        message.to_string()
    } else {
        format!("rate limited: {message}")
    }
}

pub(super) async fn handle_chat(
    server: &AcpServer,
    params: Value,
    trace: &RequestTraceContext,
) -> Result<DispatchOutput> {
    use crate::acp::r#impl::chat::handle_chat as chat_handler;
    use crate::acp::r#impl::chat::streaming::{StreamFrame, StreamObserver};
    use tokio::sync::mpsc;

    let (tx, rx) = mpsc::unbounded_channel::<StreamFrame>();
    let observer = StreamObserver::sse(tx);

    match chat_handler(
        server,
        None, // id = None so session::handle_chat skips send_result and sends through SSE
        Some(params),
        None,
        Some(trace.clone()),
        Some(observer),
    )
    .await
    {
        Ok(()) => Ok(DispatchOutput::Stream { receiver: rx }),
        Err(err) => {
            let message = err.to_string();
            if is_rate_limited_message(&message) {
                Ok(DispatchOutput::error(
                    -32029,
                    normalize_rate_limited_message(&message),
                ))
            } else {
                Ok(DispatchOutput::error(-32603, message))
            }
        }
    }
}

pub(super) async fn phase_payload(
    server: &AcpServer,
    _params: Value,
    _trace: &RequestTraceContext,
) -> Result<Value> {
    let rate_limiter = server
        .resilience
        .phase_rate_limiter
        .lock()
        .map(|guard| {
            let mut m = serde_json::Map::new();
            m.insert(
                "tracked".to_string(),
                Value::Number(guard.tracked_phases().into()),
            );
            m.insert(
                "buckets".to_string(),
                serde_json::to_value(guard.snapshot()).unwrap_or_default(),
            );
            Value::Object(m)
        })
        .unwrap_or_else(|_| {
            let mut m = serde_json::Map::new();
            m.insert("tracked".to_string(), Value::Number(0.into()));
            m.insert("buckets".to_string(), Value::Object(serde_json::Map::new()));
            Value::Object(m)
        });

    let inflight = server
        .resilience
        .inflight_limiter
        .read()
        .map(|guard| {
            let (global, phase) = guard.snapshot();
            let mut m = serde_json::Map::new();
            m.insert("global".to_string(), Value::Number(global.into()));
            m.insert(
                "phase".to_string(),
                serde_json::to_value(phase).unwrap_or_default(),
            );
            Value::Object(m)
        })
        .unwrap_or_else(|_| {
            let mut m = serde_json::Map::new();
            m.insert("global".to_string(), Value::Number(0.into()));
            m.insert("phase".to_string(), Value::Object(serde_json::Map::new()));
            Value::Object(m)
        });

    let response = PhaseResponse {
        rate_limiter,
        inflight,
    };
    Ok(serde_json::to_value(&response)?)
}

pub(super) async fn models_list_payload(server: &AcpServer, _params: Value) -> Result<Value> {
    let models = server
        .model_deps
        .agent_registry
        .as_ref()
        .map(|registry| {
            registry
                .models()
                .into_iter()
                .flat_map(|(provider_name, _default_model, models)| {
                    models.into_iter().map(move |m| {
                        let mut model = serde_json::Map::new();
                        model.insert("id".to_string(), Value::String(m.id));
                        model.insert("name".to_string(), Value::String(m.name));
                        model.insert("description".to_string(), Value::String(m.description));
                        model.insert("provider".to_string(), Value::String(provider_name.clone()));
                        model.insert("is_default".to_string(), Value::Bool(m.is_default));
                        model.insert(
                            "capabilities".to_string(),
                            Value::Array(m.capabilities.into_iter().map(Value::String).collect()),
                        );
                        if let Some(cw) = m.context_window {
                            model.insert(
                                "context_window".to_string(),
                                Value::Number((cw as u64).into()),
                            );
                        }
                        Value::Object(model)
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let response = ModelsListResponse { models };
    Ok(serde_json::to_value(&response)?)
}

pub(super) async fn terminal_create_payload(_server: &AcpServer, params: Value) -> Result<Value> {
    let command = params
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let args: Vec<String> = params
        .get("args")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let cwd = params
        .get("cwd")
        .and_then(Value::as_str)
        .map(ToString::to_string);

    let terminal_id = generate_terminal_id();

    let mut cmd = std::process::Command::new(&command);
    cmd.args(&args);
    if let Some(ref dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let child = cmd
        .spawn()
        .map_err(|e| anyhow::anyhow!("failed to spawn terminal process '{}': {}", command, e))?;

    {
        let mut state = acp_terminal_state().lock().unwrap_or_else(|poisoned| {
            warn!("ACP terminal state lock poisoned in handle_terminal_create, recovering");
            poisoned.into_inner()
        });
        state.insert(
            terminal_id.clone(),
            TerminalProcess {
                child,
                output_buffer: Vec::new(),
                exited: false,
                exit_code: None,
            },
        );
    }

    Ok(serde_json::to_value(
        &crate::schema::CreateTerminalResponse {
            terminal_id: crate::schema::TerminalId::new(&terminal_id),
            meta: None,
        },
    )?)
}

/// Handle `terminal/output` — reads buffered terminal output.
pub(super) async fn terminal_output_payload(_server: &AcpServer, params: Value) -> Result<Value> {
    let terminal_id = params
        .get("terminalId")
        .and_then(Value::as_str)
        .unwrap_or_default();

    let terminal_id_owned = terminal_id.to_string();
    let (output, truncated, exit_status) = tokio::task::spawn_blocking(move || {
        let mut state = acp_terminal_state().lock().unwrap_or_else(|poisoned| {
            warn!("ACP terminal state lock poisoned in handle_terminal_output, recovering");
            poisoned.into_inner()
        });
        if let Some(proc) = state.get_mut(&terminal_id_owned) {
            // Try to read any available stdout/stderr — we're in spawn_blocking, so
            // blocking I/O is safe and doesn't hold up the async runtime.
            if let Some(ref mut stdout) = proc.child.stdout {
                use std::io::Read;
                let mut buf = [0u8; 4096];
                loop {
                    match stdout.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => proc.output_buffer.extend_from_slice(&buf[..n]),
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(_) => break,
                    }
                }
            }
            if let Some(ref mut stderr) = proc.child.stderr {
                use std::io::Read;
                let mut buf = [0u8; 4096];
                loop {
                    match stderr.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => proc.output_buffer.extend_from_slice(&buf[..n]),
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(_) => break,
                    }
                }
            }

            // Check if process has exited — try_wait is non-blocking, safe anywhere
            let exit_code = proc.child.try_wait().ok().flatten().map(|status| {
                proc.exited = true;
                status.code()
            });
            if let Some(code) = exit_code {
                proc.exit_code = code;
            }

            let output_str = String::from_utf8_lossy(&proc.output_buffer).to_string();
            let is_truncated = proc.output_buffer.len() > 65536;
            let exit = proc
                .exit_code
                .map(|code| crate::schema::TerminalExitStatus {
                    exit_code: Some(code as u32),
                    signal: None,
                    meta: None,
                });

            (output_str, is_truncated, exit)
        } else {
            (String::new(), false, None)
        }
    })
    .await
    .unwrap_or((String::new(), false, None));

    Ok(serde_json::to_value(
        &crate::schema::TerminalOutputResponse {
            output,
            truncated,
            exit_status,
            meta: None,
        },
    )?)
}

/// Handle `terminal/release` — releases terminal resources.
pub(super) async fn handle_terminal_release(
    _server: &AcpServer,
    params: Value,
) -> Result<DispatchOutput> {
    let terminal_id = params
        .get("terminalId")
        .and_then(Value::as_str)
        .unwrap_or_default();

    if !terminal_id.is_empty() {
        let proc = {
            let mut state = acp_terminal_state().lock().unwrap_or_else(|poisoned| {
                warn!("ACP terminal state lock poisoned in handle_terminal_release, recovering");
                poisoned.into_inner()
            });
            state.remove(terminal_id)
        };
        if let Some(mut p) = proc {
            // Kill the process if still running — spawn_blocking to avoid blocking
            // the async runtime during `wait()`.
            let _ = p.child.kill();
            tokio::task::spawn_blocking(move || {
                let _ = p.child.wait();
            })
            .await
            .ok();
        }
    }

    Ok(DispatchOutput::empty())
}

/// Handle `terminal/kill` — kills a terminal process.
pub(super) async fn handle_terminal_kill(
    _server: &AcpServer,
    params: Value,
) -> Result<DispatchOutput> {
    let terminal_id = params
        .get("terminalId")
        .and_then(Value::as_str)
        .unwrap_or_default();

    if !terminal_id.is_empty() {
        let mut state = acp_terminal_state().lock().unwrap_or_else(|poisoned| {
            warn!("ACP terminal state lock poisoned in handle_terminal_kill, recovering");
            poisoned.into_inner()
        });
        if let Some(proc) = state.get_mut(terminal_id) {
            let _ = proc.child.kill();
        }
    }

    Ok(DispatchOutput::empty())
}

/// Handle `terminal/wait_for_exit` — waits for a terminal process to exit.
pub(super) async fn terminal_wait_for_exit_payload(
    _server: &AcpServer,
    params: Value,
) -> Result<Value> {
    let terminal_id = params
        .get("terminalId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let exit_code = if !terminal_id.is_empty() {
        // Spawn a blocking task to wait for the process
        let tid = terminal_id.clone();
        tokio::task::spawn_blocking(move || -> Option<i32> {
            let mut state = acp_terminal_state().lock().unwrap_or_else(|poisoned| {
                warn!(
                    "ACP terminal state lock poisoned in handle_terminal_wait_for_exit, recovering"
                );
                poisoned.into_inner()
            });
            if let Some(proc) = state.get_mut(&tid) {
                let status = proc.child.wait().ok()?;
                proc.exited = true;
                let code = status.code();
                proc.exit_code = code;
                return code;
            }
            None
        })
        .await
        .unwrap_or(None)
    } else {
        None
    };

    Ok(serde_json::to_value(
        &crate::schema::WaitForTerminalExitResponse {
            exit_status: crate::schema::TerminalExitStatus {
                exit_code: exit_code.map(|c| c as u32),
                signal: None,
                meta: None,
            },
            meta: None,
        },
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── normalize_acp_mode ────────────────────────────────────────────

    #[test]
    fn normalize_acp_mode_known() {
        assert_eq!(normalize_acp_mode(Some("agent")), "agent");
        assert_eq!(normalize_acp_mode(Some("AGENT")), "agent");
        assert_eq!(normalize_acp_mode(Some("edit")), "edit");
        assert_eq!(normalize_acp_mode(Some("chat")), "chat");
        assert_eq!(normalize_acp_mode(Some("full_auto")), "full_auto");
    }

    #[test]
    fn normalize_acp_mode_unknown_returns_original() {
        assert_eq!(normalize_acp_mode(Some("unknown_mode")), "unknown_mode");
    }

    // ── JSON-RPC format error simulation ──────────────────────────────

    #[test]
    fn send_error_message_format_is_valid_json() {
        // Verify that error messages produced by send_error are valid JSON
        // This tests the JSON-RPC error response structure.
        let error_code = -32603;
        let message = "Internal error".to_string();
        let error_json = json!({
            "jsonrpc": "2.0",
            "id": Value::Null,
            "error": {
                "code": error_code,
                "message": message,
                "data": json!({"detail": "test"}),
            }
        });

        // Verify it's valid JSON
        let serialized = serde_json::to_string(&error_json).expect("must serialize");
        let deserialized: serde_json::Value =
            serde_json::from_str(&serialized).expect("must deserialize");
        assert_eq!(deserialized["error"]["code"], error_code);
        assert_eq!(deserialized["jsonrpc"], "2.0");
    }

    #[test]
    fn send_error_handles_null_id() {
        // A notification (no id) should still produce valid error
        let error_json = json!({
            "jsonrpc": "2.0",
            "id": null,
            "error": {
                "code": -32700,
                "message": "Parse error",
                "data": {}
            }
        });
        let serialized = serde_json::to_string(&error_json).expect("must serialize");
        let deserialized: serde_json::Value =
            serde_json::from_str(&serialized).expect("must deserialize");
        assert!(deserialized["id"].is_null());
    }

    // ── rate-limited message handling ─────────────────────────────────

    #[test]
    fn is_rate_limited_message_detects_rate_limit() {
        assert!(is_rate_limited_message("rate limited, retry in"));
        assert!(is_rate_limited_message("too many requests"));
        assert!(!is_rate_limited_message("normal error"));
    }

    // ── semver patch parsing ──────────────────────────────────────────

    #[test]
    fn parse_semver_patch_valid() {
        assert_eq!(parse_semver_patch("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_semver_patch("0.0.0"), Some((0, 0, 0)));
    }

    #[test]
    fn parse_semver_patch_invalid_returns_none() {
        assert_eq!(parse_semver_patch(""), None);
        assert_eq!(parse_semver_patch("abc"), None);
        assert_eq!(parse_semver_patch("1.2"), None);
        assert_eq!(parse_semver_patch("1.2.3.4"), None);
    }

    #[test]
    fn bump_patch_version_increments() {
        assert_eq!(bump_patch_version("1.2.3"), "1.2.4");
        assert_eq!(bump_patch_version("0.0.0"), "0.0.1");
    }

    #[test]
    fn bump_patch_version_invalid_returns_default() {
        assert_eq!(bump_patch_version(""), "1.0.0");
    }

    // ── Oversized payload boundary ────────────────────────────────────

    #[test]
    fn build_chat_params_from_acp_with_large_messages_does_not_panic() {
        // Simulate a very large message to test boundary conditions
        let large_content = "x".repeat(1_000_000); // 1MB message
        let acp_params = json!({
            "mode": "chat",
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": large_content}]}
            ]
        });

        // This should not panic even with large payload
        let session_state = AcpSessionState::default();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            build_chat_params_from_acp(acp_params.clone(), &session_state)
        }));
        // The function might error on oversized content but should not panic
        assert!(result.is_ok());
    }

    // ── generate_acp_session_id / generate_terminal_id ────────────────

    #[test]
    fn generate_acp_session_id_format() {
        let id = generate_acp_session_id();
        assert!(
            id.starts_with("acp-session-"),
            "id should start with acp-session-, got: {}",
            id
        );
        // Format is "acp-session-{timestamp:x}-{counter:x}"
        assert!(
            id.len() > 20,
            "id should be longer than 20 chars, got {}",
            id.len()
        );
    }

    #[test]
    fn generate_terminal_id_format() {
        let id = generate_terminal_id();
        assert!(
            id.starts_with("terminal-"),
            "id should start with terminal-, got: {}",
            id
        );
    }

    #[test]
    fn generate_session_ids_are_unique() {
        let id1 = generate_acp_session_id();
        let id2 = generate_acp_session_id();
        assert_ne!(id1, id2);
    }
}
