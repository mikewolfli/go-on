use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex as StdMutex;
use std::sync::OnceLock;
use tracing::{info, warn};

use super::*;
// GovernanceAction and AutonomousEditAuditEntry used by sub-modules via super::*
use crate::mcp::{MAX_CANCELLED_REQUESTS, MCP_VERSION};
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
pub(super) use core::{handle_chat, initialize_payload, mcp_initialize_payload, phase_payload};
// models_list_payload is shared with the native MCP arm (src/mcp/handlers.rs)
// so both `models.list` entries return the same structure.
pub(crate) use core::models_list_payload;

// MCP protocol handlers
pub(super) use mcp::{
    mcp_completion_complete_payload, mcp_logging_set_level_payload, mcp_ping_payload,
    mcp_prompts_get_payload, mcp_prompts_list_payload, mcp_resources_list_payload,
    mcp_resources_read_payload, mcp_resources_subscribe_payload,
    mcp_sampling_create_message_payload, mcp_tools_call_payload, mcp_tools_list_payload,
};

// Session handlers
pub(super) use session::{
    approval_list_payload, session_cancel_payload, session_close_payload,
    session_config_favorite_toggle_payload, session_config_get_payload, session_config_set_payload,
    session_delete_payload, session_list_payload, session_load_payload, session_new_payload,
    session_prompt_payload, session_request_permission_payload, session_resume_payload,
    session_set_config_option_payload, session_set_mode_payload,
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
    /// Whether the session was cancelled via `session/cancel`. Prompts against
    /// a cancelled session are rejected (`session_prompt_payload`).
    pub(super) cancelled: bool,
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
    /// The child process handle. stdout/stderr are owned by the per-pipe
    /// reader threads (spawned at create); `child` retains the pid and the
    /// `kill`/`try_wait`/`wait` handles.
    child: std::process::Child,
    /// Captured stdout + stderr output so far (ring semantics: old bytes are
    /// dropped beyond [`MAX_TERMINAL_OUTPUT_BYTES`], see `truncated`).
    /// Written by the per-pipe reader threads, read under the state lock.
    output_buffer: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    /// Number of bytes of `output_buffer` already returned to clients via
    /// `terminal/output`. `terminal/output` returns only the delta since the
    /// last call, so a long-running process does not re-serialize history
    /// (previously the whole buffer was returned every time — unbounded
    /// growth and O(n²) total transfer).
    read_offset: usize,
    /// True once output exceeded the buffer cap and the oldest bytes were
    /// dropped (set by the reader threads).
    truncated: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Whether the process has exited.
    exited: bool,
    /// Exit status captured when process exited.
    exit_code: Option<i32>,
    /// Reader threads for stdout/stderr. Dropping the handles detaches the
    /// threads; they exit on their own once the pipes close (process exit or
    /// kill).
    readers: Vec<std::thread::JoinHandle<()>>,
}

/// Hard cap for a single terminal's captured output. Beyond this, the oldest
/// bytes are dropped (the client is expected to consume output regularly via
/// `terminal/output`).
pub(super) const MAX_TERMINAL_OUTPUT_BYTES: usize = 4 * 1024 * 1024;

// ── Static state ─────────────────────────────────────────────────────────

static ACP_SESSION_STATE: OnceLock<tokio::sync::RwLock<HashMap<String, AcpSessionState>>> =
    OnceLock::new();

pub(super) fn acp_session_state() -> &'static tokio::sync::RwLock<HashMap<String, AcpSessionState>>
{
    ACP_SESSION_STATE.get_or_init(|| tokio::sync::RwLock::new(HashMap::new()))
}

/// Resolve the work-directory base for an ACP session (the `cwd` from
/// `session/new`). Consumed by the tool executor's path sandbox
/// (`allowed_base_dir`) so file tools stay inside the session's workspace.
/// Returns `None` when the session has no recorded cwd (sandbox check is
/// then skipped by `sanitize_path`).
pub(crate) async fn session_base_dir(session_id: &str) -> Option<std::path::PathBuf> {
    let state = acp_session_state().read().await;
    state
        .get(session_id)
        .and_then(|s| s.cwd.clone())
        .map(std::path::PathBuf::from)
}

/// Memory bound for the in-memory ACP session map. `session/set_mode` and
/// `session/resume` create entries for arbitrary session ids via
/// `entry().or_default()`, so an unauthenticated client could otherwise grow
/// the map without bound. When at capacity and the id is new, an arbitrary
/// existing entry is evicted (iteration order is unspecified — the bound is
/// what matters for memory safety).
pub(super) const MAX_ACP_SESSIONS: usize = 4096;

/// Bound `acp_session_state()` before inserting a new id.
pub(super) async fn make_room_for_session(session_id: &str) {
    let mut state = acp_session_state().write().await;
    if state.len() >= MAX_ACP_SESSIONS && !state.contains_key(session_id) {
        if let Some(arbitrary) = state.keys().next().cloned() {
            state.remove(&arbitrary);
        }
    }
}

/// Memory bound for the terminal map: every entry holds a live child-process
/// handle, so `terminal/create` must not grow it without limit.
pub(super) const MAX_ACP_TERMINALS: usize = 256;

/// Bound `acp_terminal_state()` before inserting a new terminal.
pub(super) fn make_room_for_terminal() {
    let mut state = acp_terminal_state().lock().unwrap_or_else(|poisoned| {
        warn!("ACP terminal state lock poisoned in make_room_for_terminal, recovering");
        poisoned.into_inner()
    });
    if state.len() >= MAX_ACP_TERMINALS {
        if let Some(arbitrary) = state.keys().next().cloned() {
            if let Some(mut evicted) = state.remove(&arbitrary) {
                // Kill the evicted process: a dropped Child handle alone would
                // let the process keep running (and its reader threads keep
                // reading pipes nobody references). Killing closes the pipes
                // so the reader threads exit too.
                let _ = evicted.child.kill();
                warn!(
                    terminal = %arbitrary,
                    "ACP terminal map full: evicted and killed an arbitrary terminal"
                );
            }
        }
    }
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

// ── $/cancel_request support ────────────────────────────────────────────
// The ACP arm mirrors the MCP transport's `cancelled_requests` registry
// (src/mcp/handlers.rs): `$/cancel_request` marks a request id, and the
// token-collection loops of in-flight requests check the mark and abort.
// The 10K registry bound is shared with the MCP arm — see
// `crate::mcp::MAX_CANCELLED_REQUESTS`. Eviction drops an arbitrary entry
// (HashSet iteration order is unspecified), not the oldest.

/// Request ids flagged by `$/cancel_request` (keyed with `value_to_id`).
static ACP_CANCELLED_REQUESTS: OnceLock<StdMutex<HashSet<String>>> = OnceLock::new();

fn acp_cancelled_requests() -> &'static StdMutex<HashSet<String>> {
    ACP_CANCELLED_REQUESTS.get_or_init(|| StdMutex::new(HashSet::new()))
}

/// Record that the client asked to cancel the request with the given id.
pub(crate) fn mark_acp_request_cancelled(request_id: &str) {
    let mut cancelled = acp_cancelled_requests().lock().unwrap_or_else(|poisoned| {
        warn!("acp_cancelled_requests lock poisoned, recovering");
        poisoned.into_inner()
    });
    // Prevent unbounded growth: when over the limit, drop an arbitrary entry
    // (HashSet iteration order is unspecified — not insertion order, so this
    // is not an LRU eviction; the bound is what matters for memory safety).
    if cancelled.len() >= MAX_CANCELLED_REQUESTS {
        if let Some(arbitrary) = cancelled.iter().next().cloned() {
            cancelled.remove(&arbitrary);
        }
    }
    cancelled.insert(request_id.to_string());
}

/// Drop the cancellation mark once the request has finished, so a later
/// request reusing the same id is not spuriously cancelled.
pub(crate) fn clear_acp_request_cancelled(request_id: &str) {
    acp_cancelled_requests()
        .lock()
        .unwrap_or_else(|poisoned| {
            warn!("acp_cancelled_requests lock poisoned, recovering");
            poisoned.into_inner()
        })
        .remove(request_id);
}

/// Returns true when the client sent `$/cancel_request` for the given id.
pub(crate) fn is_acp_request_cancelled(request_id: &str) -> bool {
    acp_cancelled_requests()
        .lock()
        .unwrap_or_else(|poisoned| {
            warn!("acp_cancelled_requests lock poisoned, recovering");
            poisoned.into_inner()
        })
        .contains(request_id)
}

/// Error message emitted when an in-flight ACP request is aborted because the
/// client cancelled it. The chat entry maps errors containing this marker to
/// the `RequestCancelled` JSON-RPC code.
pub(crate) const REQUEST_CANCELLED_MESSAGE: &str = "request cancelled by client";

// Task-local: the JSON-RPC request id currently being dispatched (keyed with
// `value_to_id`). Set in `handle_request` so the deep token-collection loops
// (`run_agent_collecting`, the autonomy loop) can observe `$/cancel_request`
// marks without threading the id through every call site. Task-locals do not
// propagate into `tokio::spawn`ed tasks — the loops that check live in the
// request-handling task itself, so this is safe.
tokio::task_local! {
    pub(crate) static ACP_CURRENT_REQUEST_ID: Option<String>;
}

/// Returns true when the client has cancelled the request currently being
/// handled (checked against the [`ACP_CURRENT_REQUEST_ID`] task-local).
/// Returns false when no request is being handled (e.g. CLI paths that never
/// go through `handle_request`).
pub(crate) fn current_request_cancelled() -> bool {
    ACP_CURRENT_REQUEST_ID
        .try_with(|id| id.as_deref().map(is_acp_request_cancelled).unwrap_or(false))
        .unwrap_or(false)
}

/// Abort the current request with the canonical cancellation error message.
/// Returns `Err` so callers in token loops can `?`-propagate it straight out
/// of the request pipeline.
pub(crate) fn cancelled_error() -> anyhow::Error {
    anyhow::anyhow!(REQUEST_CANCELLED_MESSAGE)
}

/// Log + return the canonical cancellation error (single site for the abort
/// path so all token loops use the same message).
pub(crate) fn log_and_cancel(where_: &str) -> anyhow::Error {
    info!(
        target: "acp::protocol_pack",
        "{}: {}",
        where_,
        REQUEST_CANCELLED_MESSAGE
    );
    cancelled_error()
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
    // Nanosecond resolution for id uniqueness; the shared timestamp helpers
    // expose seconds/millis only, so this stays inline.
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let seq = ACP_SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("acp-session-{:x}-{:x}", ts, seq)
}

static ACP_TERMINAL_COUNTER: AtomicU64 = AtomicU64::new(1);

pub(super) fn generate_terminal_id() -> String {
    // Nanosecond resolution for id uniqueness; the shared timestamp helpers
    // expose seconds/millis only, so this stays inline.
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
        // PhaseOptions.extra is #[serde(flatten)]ed, so extra keys must be
        // emitted at the TOP level of the options object. The previous code
        // nested them under an "extra" key, which the flatten turned into
        // extra["extra"] — silently dropping cwd / model / directories.
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
        Some(Value::Object(extra))
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

    #[test]
    fn build_chat_params_from_acp_options_are_flat_not_nested() {
        // Regression: PhaseOptions.extra is #[serde(flatten)]ed — the ACP
        // bridge used to emit options as {"extra": {cwd, model}}, which the
        // flatten mapped to extra["extra"] and silently dropped cwd/model.
        let acp_params = json!({
            "sessionId": "repro-1",
            "prompt": [
                {"type": "text", "text": "Hello, how are you?"}
            ]
        });
        let mut session_state = AcpSessionState::default();
        session_state.cwd = Some("/Users/test/project".to_string());
        session_state.config_options.insert(
            "model".to_string(),
            Value::String("deepseek-v4-flash".to_string()),
        );
        let value = build_chat_params_from_acp(acp_params.clone(), &session_state);
        let text = acp_prompt_to_text(&acp_params);
        assert_eq!(text, "Hello, how are you?");
        let params: crate::acp::r#impl::chat::ChatParams =
            serde_json::from_value(value).expect("params parse");
        assert_eq!(params.mode, "safeguard");
        assert_eq!(params.conversation_id.as_deref(), Some("repro-1"));
        assert!(params.phase.is_none());
        let extra = params
            .options
            .as_ref()
            .expect("options present")
            .extra
            .clone();
        assert!(
            !extra.contains_key("extra"),
            "options must not be nested under an 'extra' key, got: {:?}",
            extra
        );
        assert_eq!(
            extra.get("cwd"),
            Some(&Value::String("/Users/test/project".into()))
        );
        assert_eq!(
            extra.get("model"),
            Some(&Value::String("deepseek-v4-flash".into()))
        );
    }

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

    #[tokio::test]
    async fn approval_list_returns_pending_requests() {
        let server = crate::acp::server::ServerBuilder::default().build();

        // Empty by default.
        let empty = approval_list_payload(&server, json!({})).await.unwrap();
        assert_eq!(empty["count"].as_u64(), Some(0));

        // Seed one pending request.
        {
            let mut pending = acp_pending_permission_requests().write().await;
            pending.insert(
                "sess-1".to_string(),
                PendingPermissionRequest {
                    tool_name: "apply_patch".to_string(),
                    tool_args: json!({}),
                    mode: "edit".to_string(),
                    risk_score: 0.9,
                },
            );
        }

        let result = approval_list_payload(&server, json!({})).await.unwrap();
        let requests = result["requests"].as_array().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["session_id"], "sess-1");
        assert_eq!(requests[0]["tool_name"], "apply_patch");
        assert_eq!(requests[0]["risk_score"], 0.9);
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
