use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex as StdMutex;
use std::sync::OnceLock;
use tracing::warn;

use super::*;
// GovernanceAction and AutonomousEditAuditEntry used by sub-modules via super::*
use crate::mcp::MCP_VERSION;
use crate::schema::{ModelsListResponse, PhaseResponse, ProtocolVersion};

// ── Sub-modules ──────────────────────────────────────────────────────────

mod audit;
mod auth;
mod core;
pub(crate) mod mcp;
pub(crate) mod session;
mod skill;
mod terminal;

// ── Re-exports ───────────────────────────────────────────────────────────

// Auth handler
pub(super) use auth::{authenticate_payload, logout_payload};

// Audit helpers — is_rate_limited_message and normalize_rate_limited_message
// are used by core.rs and session.rs via super::*
pub(super) use audit::{is_rate_limited_message, normalize_rate_limited_message};

// Core handlers (initialize, mcp_initialize, handle_chat, phase, models_list)
pub(super) use core::{
    handle_chat, initialize_payload, mcp_initialize_payload, models_list_payload, phase_payload,
};

// MCP protocol handlers
pub(super) use mcp::{
    mcp_completion_complete_payload, mcp_logging_set_level_payload, mcp_ping_payload,
    mcp_prompts_get_payload, mcp_prompts_list_payload, mcp_resources_list_payload,
    mcp_resources_read_payload, mcp_resources_subscribe_payload,
    mcp_sampling_create_message_payload, mcp_tools_call_payload, mcp_tools_list_payload,
};

// Session handlers
pub(super) use session::{
    session_cancel_payload, session_close_payload, session_config_favorite_toggle_payload,
    session_config_get_payload, session_config_set_payload, session_delete_payload,
    session_list_payload, session_load_payload, session_new_payload, session_prompt_payload,
    session_request_permission_payload, session_resume_payload, session_set_config_option_payload,
    session_set_mode_payload,
};

// Skill handlers
pub(super) use skill::{
    skill_create_payload, skill_enabled_toggle_payload, skill_import_payload,
    skill_list_imported_payload, skill_remove_payload,
};
pub(crate) use skill::{
    skill_update_payload, skill_version_list_payload, skill_version_rollback_payload,
};

// Tools handlers (acp_tools_list_payload, acp_tools_call_payload)
// record_tool_call_audit_with_protocol must be pub for re-export from request.rs
pub use mcp::record_tool_call_audit_with_protocol;
pub(super) use mcp::{acp_tools_call_payload, acp_tools_list_payload};

pub(super) use terminal::{
    handle_terminal_kill, handle_terminal_release, terminal_create_payload,
    terminal_output_payload, terminal_wait_for_exit_payload,
};

// ── Type definitions ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub(super) struct AcpSessionState {
    pub(super) cwd: Option<String>,
    pub(super) mode: String,
    pub(super) additional_directories: Vec<String>,
    pub(super) config_options: HashMap<String, Value>,
    /// Per-config-id set of favorited value IDs.
    /// Key: config_id (e.g. "model"), Value: set of favorited value IDs.
    pub(super) favorite_config_values: HashMap<String, std::collections::HashSet<String>>,
}

#[derive(Debug, Clone)]
pub(crate) struct PendingPermissionRequest {
    pub(super) tool_name: String,
    pub(super) tool_args: Value,
    pub(super) mode: String,
    pub(super) risk_score: f64,
}

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

/// Tracks a spawned terminal process.
pub(super) struct TerminalProcess {
    /// The child process handle.
    child: std::process::Child,
    /// Captured stdout + stderr output so far.
    output_buffer: Vec<u8>,
    /// Whether the process has exited.
    exited: bool,
    /// Exit status captured when process exited.
    exit_code: Option<i32>,
}

// ── Static state ─────────────────────────────────────────────────────────

static ACP_SESSION_STATE: OnceLock<tokio::sync::RwLock<HashMap<String, AcpSessionState>>> =
    OnceLock::new();

pub(super) fn acp_session_state() -> &'static tokio::sync::RwLock<HashMap<String, AcpSessionState>>
{
    ACP_SESSION_STATE.get_or_init(|| tokio::sync::RwLock::new(HashMap::new()))
}

static ACP_PERMISSION_STATE: OnceLock<
    tokio::sync::RwLock<HashMap<String, crate::schema::PermissionOptionId>>,
> = OnceLock::new();

pub(crate) fn acp_permission_state(
) -> &'static tokio::sync::RwLock<HashMap<String, crate::schema::PermissionOptionId>> {
    ACP_PERMISSION_STATE.get_or_init(|| tokio::sync::RwLock::new(HashMap::new()))
}

static ACP_PENDING_PERMISSION_REQUESTS: OnceLock<
    tokio::sync::RwLock<HashMap<String, PendingPermissionRequest>>,
> = OnceLock::new();

pub(crate) fn acp_pending_permission_requests(
) -> &'static tokio::sync::RwLock<HashMap<String, PendingPermissionRequest>> {
    ACP_PENDING_PERMISSION_REQUESTS.get_or_init(|| tokio::sync::RwLock::new(HashMap::new()))
}

static ACP_TERMINAL_STATE: OnceLock<StdMutex<HashMap<String, TerminalProcess>>> = OnceLock::new();

pub(super) fn acp_terminal_state() -> &'static StdMutex<HashMap<String, TerminalProcess>> {
    ACP_TERMINAL_STATE.get_or_init(|| StdMutex::new(HashMap::new()))
}

// ── Protocol-level negotiated version ────────────────────────────────────

/// Negotiate the protocol version against the client's requested version.
///
/// Picks the highest supported version that does not exceed the client's
/// request (falling back to the latest supported version when the client
/// sends none or a version newer than anything we support). The negotiated
/// value is returned to the caller; the former process-wide `OnceLock`
/// memoisation was removed — it had no readers (the doc comment claiming
/// later handlers would honour it was never true).
pub(super) fn negotiate_protocol_version(requested: Option<ProtocolVersion>) -> ProtocolVersion {
    match requested {
        Some(client) => {
            let supported = ProtocolVersion::supported_versions();
            let mut best = ProtocolVersion::V1;
            for version in supported {
                if *version <= client {
                    best = *version;
                }
            }
            best
        }
        None => ProtocolVersion::LATEST,
    }
}

// ── Atomic counters ──────────────────────────────────────────────────────

static ACP_SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);

pub(super) fn generate_acp_session_id() -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let seq = ACP_SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("acp-session-{:x}-{:x}", ts, seq)
}

static ACP_TERMINAL_COUNTER: AtomicU64 = AtomicU64::new(1);

pub(super) fn generate_terminal_id() -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let seq = ACP_TERMINAL_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("terminal-{:x}-{:x}", ts, seq)
}

// ── Shared helper functions ──────────────────────────────────────────────

pub(super) fn normalize_acp_mode(value: Option<&str>) -> String {
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
        None => "safeguard".to_string(),
    }
}

pub(super) fn extract_additional_directories(params: &Value) -> Vec<String> {
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

pub(super) async fn session_state_for_prompt(params: &Value) -> AcpSessionState {
    let session_id = params.get("sessionId").and_then(Value::as_str);
    let stored = if let Some(session_id) = session_id {
        let state = acp_session_state().read().await;
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

pub(super) fn acp_prompt_to_text(params: &Value) -> String {
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
pub(super) fn build_chat_params_from_acp(params: Value, session_state: &AcpSessionState) -> Value {
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
    let options = {
        let cwd = session_state.cwd.as_ref();
        let mut extra = serde_json::Map::new();
        if let Some(cwd) = cwd {
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
        }
        // Pass the user's model selection from session config_options
        // so filter_agents_by_model can match the correct agent
        // (only relevant when model is specific, not "auto").
        if let Some(model) = session_state
            .config_options
            .get("model")
            .and_then(|v| v.as_str())
        {
            let m = model.trim();
            if !m.is_empty() && m != "auto" {
                extra.insert("model".to_string(), Value::String(m.to_string()));
            }
        }
        let mut options = serde_json::Map::new();
        options.insert("extra".to_string(), Value::Object(extra));
        Some(Value::Object(options))
    };

    serde_json::to_value(InternalChatParams {
        mode: normalize_acp_mode(Some(session_state.mode.as_str())),
        messages,
        conversation_id,
        options,
    })
    .unwrap_or_default()
}

/// Build config options for model selection, populated from the agent registry.
pub(super) fn build_model_config_options(
    server: &AcpServer,
    current_model: Option<&str>,
) -> Vec<crate::schema::SessionConfigOption> {
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
                    favorite: false,
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
            current_value: SessionConfigValueId::new(
                current_model
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or("auto"),
            ),
            options: SessionConfigSelectOptions::Grouped(groups),
        }),
        meta: None,
    }]
}

/// Build the default set of session modes.
pub(super) fn build_default_modes() -> crate::schema::SessionModeState {
    use crate::schema::{SessionMode, SessionModeId, SessionModeState};
    SessionModeState::new(
        SessionModeId::new("safeguard"),
        vec![
            SessionMode::new(SessionModeId::new("safeguard"), "SafeGuard / 安全")
                .description("Safety-first — escalation on high-risk operations (default)"),
            SessionMode::new(SessionModeId::new("ask"), "Ask / 对话")
                .description("Q&A assistant — general questions"),
            SessionMode::new(SessionModeId::new("plan"), "Plan / 计划")
                .description("Planning mode — structured task breakdown"),
            SessionMode::new(SessionModeId::new("edit"), "Edit / 编辑")
                .description("Edit/review mode — code changes"),
            SessionMode::new(SessionModeId::new("full_auto"), "Full Auto / 全自动")
                .description("Fully autonomous — agent runs without user confirmation"),
        ],
    )
}

// ── Tests ────────────────────────────────────────────────────────────────

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

        let serialized = serde_json::to_string(&error_json).expect("must serialize");
        let deserialized: serde_json::Value =
            serde_json::from_str(&serialized).expect("must deserialize");
        assert_eq!(deserialized["error"]["code"], error_code);
        assert_eq!(deserialized["jsonrpc"], "2.0");
    }

    #[test]
    fn send_error_handles_null_id() {
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
        assert!(super::is_rate_limited_message("rate limited, retry in"));
        assert!(super::is_rate_limited_message("too many requests"));
        assert!(!super::is_rate_limited_message("normal error"));
    }

    // ── semver patch parsing ──────────────────────────────────────────

    #[test]
    fn parse_semver_patch_valid() {
        assert_eq!(super::skill::parse_semver_patch("1.2.3"), Some((1, 2, 3)));
        assert_eq!(super::skill::parse_semver_patch("0.0.0"), Some((0, 0, 0)));
    }

    #[test]
    fn parse_semver_patch_invalid_returns_none() {
        assert_eq!(super::skill::parse_semver_patch(""), None);
        assert_eq!(super::skill::parse_semver_patch("abc"), None);
        assert_eq!(super::skill::parse_semver_patch("1.2"), None);
        assert_eq!(super::skill::parse_semver_patch("1.2.3.4"), None);
    }

    #[test]
    fn bump_patch_version_increments() {
        assert_eq!(super::skill::bump_patch_version("1.2.3"), "1.2.4");
        assert_eq!(super::skill::bump_patch_version("0.0.0"), "0.0.1");
    }

    #[test]
    fn bump_patch_version_invalid_returns_default() {
        assert_eq!(super::skill::bump_patch_version(""), "1.0.0");
    }

    // ── Oversized payload boundary ────────────────────────────────────

    #[test]
    fn build_chat_params_from_acp_with_large_messages_does_not_panic() {
        let large_content = "x".repeat(1_000_000);
        let acp_params = json!({
            "mode": "chat",
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": large_content}]}
            ]
        });

        let session_state = AcpSessionState::default();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            build_chat_params_from_acp(acp_params.clone(), &session_state)
        }));
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

    // ── session/delete ────────────────────────────────────────────────

    #[tokio::test]
    async fn session_delete_payload_removes_session() {
        let server = crate::acp::server::ServerBuilder::default().build();
        let new_params = json!({"mode": "edit"});
        let new_result = session_new_payload(&server, new_params).await.unwrap();
        let session_id = new_result["sessionId"].as_str().unwrap().to_string();

        {
            let state = acp_session_state().read().await;
            assert!(state.contains_key(&session_id));
        }

        let delete_params = json!({"sessionId": session_id});
        let delete_result = session_delete_payload(&server, delete_params)
            .await
            .unwrap();
        assert_eq!(delete_result["deleted"], true);

        {
            let state = acp_session_state().read().await;
            assert!(!state.contains_key(&session_id));
        }
    }

    #[tokio::test]
    async fn session_delete_payload_nonexistent_returns_deleted_false() {
        let server = crate::acp::server::ServerBuilder::default().build();
        let delete_params = json!({"sessionId": "nonexistent-session"});
        let result = session_delete_payload(&server, delete_params)
            .await
            .unwrap();
        assert_eq!(result["deleted"], false);
    }

    // ── session/config/set ────────────────────────────────────────────

    #[tokio::test]
    async fn session_config_set_payload_stores_option() {
        let server = crate::acp::server::ServerBuilder::default().build();
        let new_params = json!({"mode": "edit"});
        let new_result = session_new_payload(&server, new_params).await.unwrap();
        let session_id = new_result["sessionId"].as_str().unwrap().to_string();

        let set_params = json!({
            "sessionId": session_id,
            "configId": "temperature",
            "value": 0.7
        });
        let set_result = session_config_set_payload(&server, set_params)
            .await
            .unwrap();
        assert!(set_result.is_object());

        let get_params = json!({"sessionId": session_id});
        let get_result = session_config_get_payload(&server, get_params)
            .await
            .unwrap();
        assert_eq!(get_result["configOptions"]["temperature"], 0.7);
    }

    #[tokio::test]
    async fn session_config_set_payload_returns_selected_model_as_current_value() {
        let server = crate::acp::server::ServerBuilder::default().build();
        let new_result = session_new_payload(&server, json!({"mode": "edit"}))
            .await
            .unwrap();
        let session_id = new_result["sessionId"].as_str().unwrap().to_string();

        let set_result = session_config_set_payload(
            &server,
            json!({
                "sessionId": session_id,
                "configId": "model",
                "value": "gpt-test-model"
            }),
        )
        .await
        .unwrap();

        assert_eq!(
            set_result["configOptions"][0]["currentValue"],
            serde_json::Value::String("gpt-test-model".to_string())
        );
    }

    // ── session/config/get ────────────────────────────────────────────

    #[tokio::test]
    async fn session_config_get_payload_returns_options() {
        let server = crate::acp::server::ServerBuilder::default().build();
        let new_params = json!({"mode": "safeguard"});
        let new_result = session_new_payload(&server, new_params).await.unwrap();
        let session_id = new_result["sessionId"].as_str().unwrap().to_string();

        let get_params = json!({"sessionId": session_id});
        let get_result = session_config_get_payload(&server, get_params)
            .await
            .unwrap();
        assert_eq!(get_result["configOptions"]["mode"], "safeguard");
    }

    #[tokio::test]
    async fn session_config_get_payload_nonexistent_returns_empty() {
        let server = crate::acp::server::ServerBuilder::default().build();
        let get_params = json!({"sessionId": "nonexistent"});
        let get_result = session_config_get_payload(&server, get_params)
            .await
            .unwrap();
        assert!(get_result["configOptions"].as_object().unwrap().is_empty());
    }

    #[tokio::test]
    async fn session_request_permission_payload_consumes_pending_request() {
        let server = crate::acp::server::ServerBuilder::default().build();
        let new_result = session_new_payload(&server, json!({"mode": "edit"}))
            .await
            .unwrap();
        let session_id = new_result["sessionId"].as_str().unwrap().to_string();

        {
            let mut pending = acp_pending_permission_requests().write().await;
            pending.insert(
                session_id.clone(),
                PendingPermissionRequest {
                    tool_name: "apply_patch".to_string(),
                    tool_args: json!({"path": "src/lib.rs"}),
                    mode: "edit".to_string(),
                    risk_score: 0.8,
                },
            );
        }

        let result = session_request_permission_payload(
            &server,
            json!({
                "sessionId": session_id,
                "optionId": "approve"
            }),
        )
        .await
        .unwrap();

        assert!(result.is_object());
        let pending = acp_pending_permission_requests().read().await;
        assert!(!pending.contains_key(new_result["sessionId"].as_str().unwrap()));
    }

    #[test]
    fn permission_request_serializes_zed_compatible_option_fields() {
        let payload = serde_json::to_value(crate::schema::PermissionRequest {
            message: "confirm".to_string(),
            options: vec![crate::schema::PermissionOption::new(
                crate::schema::PermissionOptionId::new("allow"),
                "Approve",
                crate::schema::PermissionOptionKind::AllowOnce,
            )],
            timeout_secs: Some(30),
            meta: None,
        })
        .unwrap();

        assert_eq!(payload["options"][0]["optionId"], "allow");
        assert_eq!(payload["options"][0]["name"], "Approve");
        assert_eq!(payload["options"][0]["kind"], "allow_once");
        assert!(payload["options"][0].get("id").is_none());
        assert!(payload["options"][0].get("label").is_none());
    }

    // ── session/config/favorite/toggle ──────────────────────────────────────

    #[tokio::test]
    async fn session_config_favorite_toggle_toggle_on() {
        let server = crate::acp::server::ServerBuilder::default().build();
        let new_result = session_new_payload(&server, json!({"mode": "edit"}))
            .await
            .unwrap();
        let session_id = new_result["sessionId"].as_str().unwrap().to_string();

        let result = session_config_favorite_toggle_payload(
            &server,
            json!({
                "sessionId": session_id,
                "configId": "model",
                "valueId": "gpt-4"
            }),
        )
        .await
        .unwrap();

        assert_eq!(result["favorited"], true);
        assert_eq!(result["configId"], "model");
        assert_eq!(result["valueId"], "gpt-4");
    }

    #[tokio::test]
    async fn session_config_favorite_toggle_toggle_off() {
        let server = crate::acp::server::ServerBuilder::default().build();
        let new_result = session_new_payload(&server, json!({"mode": "edit"}))
            .await
            .unwrap();
        let session_id = new_result["sessionId"].as_str().unwrap().to_string();

        // First toggle on
        let _ = session_config_favorite_toggle_payload(
            &server,
            json!({
                "sessionId": session_id,
                "configId": "model",
                "valueId": "gpt-4"
            }),
        )
        .await
        .unwrap();

        // Second toggle off
        let result = session_config_favorite_toggle_payload(
            &server,
            json!({
                "sessionId": session_id,
                "configId": "model",
                "valueId": "gpt-4"
            }),
        )
        .await
        .unwrap();

        assert_eq!(result["favorited"], false);
    }

    #[tokio::test]
    async fn session_config_favorite_toggle_invalid_returns_false() {
        let server = crate::acp::server::ServerBuilder::default().build();

        let result = session_config_favorite_toggle_payload(
            &server,
            json!({
                "sessionId": "",
                "configId": "model",
                "valueId": "gpt-4"
            }),
        )
        .await
        .unwrap();

        assert_eq!(result["favorited"], false);
    }

    #[tokio::test]
    async fn session_config_favorite_toggle_isolation() {
        // Verify favorites are per-session, not global
        let server = crate::acp::server::ServerBuilder::default().build();
        let s1 = session_new_payload(&server, json!({"mode": "edit"}))
            .await
            .unwrap();
        let s2 = session_new_payload(&server, json!({"mode": "edit"}))
            .await
            .unwrap();
        let id1 = s1["sessionId"].as_str().unwrap().to_string();
        let id2 = s2["sessionId"].as_str().unwrap().to_string();

        // Toggle favorite in session 1
        let _ = session_config_favorite_toggle_payload(
            &server,
            json!({
                "sessionId": id1,
                "configId": "model",
                "valueId": "gpt-4"
            }),
        )
        .await
        .unwrap();

        // Verify it's not favorited in session 2 (isolation)
        let result2 = session_config_favorite_toggle_payload(
            &server,
            json!({
                "sessionId": id2,
                "configId": "model",
                "valueId": "gpt-4"
            }),
        )
        .await
        .unwrap();
        // First toggle on session 2 should be "favorited: true" since it was not yet toggled
        assert_eq!(result2["favorited"], true);
    }
}
