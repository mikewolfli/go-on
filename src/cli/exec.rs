//! Headless executor subcommand (`go-on exec`): run one bounded agent turn
//! without any interactive input and emit a machine-readable JSONL event
//! stream on stdout.
//!
//! # Usage
//!
//! - `go-on exec --json "prompt"` — JSONL mode (one flat JSON object per line).
//! - `go-on exec "prompt"` — plain mode: prints only the final response text.
//! - `--input-stdin` — read `{"prompt": "...", "session_id": "..."}` from stdin
//!   instead of (or in addition to) the positional prompt.
//! - `--approval-mode never|auto` — tool approval policy (default `never`).
//!
//! # JSONL event contract
//!
//! Every event is a single flat JSON object on its own line, discriminated by
//! a top-level `"type"` field. The guaranteed sequence per run is:
//!
//! 1. `{"type":"thread.started","thread_id":"...","model":"...?"}`
//! 2. `{"type":"turn.started","turn_id":"..."}`
//! 3. zero or more `{"type":"item", ...}` events (see below)
//! 4. `{"type":"turn.completed","turn_id":"...","success":true,"response":"...","duration_ms":123}`
//!
//! Item events carry a `kind` discriminator:
//!
//! - `{"type":"item","kind":"message","delta":"..."}` — one streamed content chunk.
//! - `{"type":"item","kind":"message","text":"..."}` — the final consolidated response.
//! - `{"type":"item","kind":"tool_call","name":"read_file","input":{...}}` — a tool call the agent requested.
//! - `{"type":"item","kind":"tool_result","name":"read_file","ok":true,"output":"..."}` — the tool outcome.
//!
//! # Output discipline
//!
//! In JSONL mode stdout carries **only** the event lines above. All
//! diagnostics go to stderr: `tracing` logs are routed there by
//! `init_telemetry` (`RedactingMakeWriter`), and this module never writes
//! anything else to stdout. In plain mode stdout carries only the final
//! response text.
//!
//! Errors before the first event (config load, agent registry build,
//! `--input-stdin` JSON, missing prompt) produce no JSONL: the error goes to
//! stderr and the process exits non-zero. In-turn failures (agent chat error,
//! streaming timeout) are reported inside the stream via `turn.completed`
//! with `success:false`, and the process still exits non-zero so pipelines
//! can detect the failure without parsing.
//!
//! # Approval modes (headless — never interactive)
//!
//! - `never` (default): tool calls are approved without prompting and executed
//!   exactly like the interactive chat path executes its tools.
//! - `auto`: each tool call must pass the lightweight governance gate
//!   (`governance::status::quick_check_tool`, the same gate the interactive
//!   simple-tool path uses); denied calls produce a `tool_result` item with
//!   `ok:false`.
//!
//! Neither mode ever prompts the user: `--input-stdin` consumes stdin up
//! front, and the turn engine never reads from a terminal.
//!
//! # Not wired yet (honest limits)
//!
//! - Exactly **one** agent turn per run; tool results are reported as `item`
//!   events but are **not** fed back to the model in a follow-up round (no
//!   follow-up loop). Callers drive further rounds by issuing another `exec`.
//! - `session_id` in `--input-stdin` input is echoed back in `thread.started`
//!   for correlation, but session **resume is not implemented** on this path —
//!   exec always starts a fresh thread. Resume requires the interactive chat
//!   session path.
//! - Reasoning tokens are not emitted as events; when the agent produces only
//!   reasoning, it becomes the `response` (mirroring the ACP collection core).

use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use crate::acp::helpers::conversation::stream_would_exceed_limits;
use crate::acp::r#impl::request::tools_pack::global_tool_registry;
use crate::agents::agent::{Agent, AgentRegistry, Message, StreamingSender};
use crate::config::AppConfig;
use crate::governance::status::quick_check_tool;
use crate::intelligence::capability_graph::CapabilityGraph;
use crate::orchestration::autonomy_runtime::{classify_agent_token, AgentToken};
use crate::orchestration::tool::executor::{execute_tools_concurrent, ToolExecConfig};

/// Per-chunk receive timeout for agent streaming (mirrors the ACP pipeline).
const PER_CHUNK_TIMEOUT: Duration = Duration::from_secs(120);
/// Overall cap for one agent-chat collection (mirrors the ACP pipeline).
const OVERALL_TURN_TIMEOUT: Duration = Duration::from_secs(600);
/// Maximum number of tools executed concurrently (mirrors the CLI chat path).
const MAX_CONCURRENT_TOOLS: usize = 10;

// ─────────────────────────────────────────────────────────────────────────────
// Approval mode
// ─────────────────────────────────────────────────────────────────────────────

/// Tool approval policy for a headless exec run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalMode {
    /// Headless default: tool calls are approved without prompting.
    Never,
    /// Each tool call must pass the `quick_check_tool` governance gate.
    Auto,
}

/// Parse the `--approval-mode` CLI value. `None` (flag absent) means `never`.
pub fn parse_approval_mode(raw: &str) -> Result<ApprovalMode> {
    match raw {
        "never" => Ok(ApprovalMode::Never),
        "auto" => Ok(ApprovalMode::Auto),
        other => bail!("invalid --approval-mode '{other}'; allowed: never, auto"),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// JSONL event types (flat objects, `type` discriminator)
// ─────────────────────────────────────────────────────────────────────────────

/// `item` event kind discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    /// Streamed or final assistant text.
    Message,
    /// A tool call requested by the agent.
    ToolCall,
    /// The outcome of an executed (or denied) tool call.
    ToolResult,
}

/// `thread.started` — emitted once at the beginning of an exec run.
#[derive(Debug, Clone, Serialize)]
pub struct ThreadStartedEvent {
    #[serde(rename = "type")]
    pub r#type: &'static str,
    pub thread_id: String,
    /// Default model of the primary agent, when the agent reports one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Echoed from `--input-stdin` when provided; informational only (see
    /// the module docs: resume is not wired on this path).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

impl ThreadStartedEvent {
    pub fn new(thread_id: String, model: Option<String>, session_id: Option<String>) -> Self {
        Self {
            r#type: "thread.started",
            thread_id,
            model,
            session_id,
        }
    }
}

/// `turn.started` — emitted immediately before the single agent turn.
#[derive(Debug, Clone, Serialize)]
pub struct TurnStartedEvent {
    #[serde(rename = "type")]
    pub r#type: &'static str,
    pub turn_id: String,
}

impl TurnStartedEvent {
    pub fn new(turn_id: String) -> Self {
        Self {
            r#type: "turn.started",
            turn_id,
        }
    }
}

/// `item` — streamed message chunks, the final message, tool calls, and tool
/// results. Exactly one of the payload fields is set per `kind`; the
/// constructors enforce that invariant.
#[derive(Debug, Clone, Serialize)]
pub struct ItemEvent {
    #[serde(rename = "type")]
    pub r#type: &'static str,
    pub kind: ItemKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delta: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

impl ItemEvent {
    /// One streamed content chunk (`kind:"message"`, `delta`).
    pub fn message_delta(delta: String) -> Self {
        Self {
            r#type: "item",
            kind: ItemKind::Message,
            delta: Some(delta),
            text: None,
            name: None,
            input: None,
            ok: None,
            output: None,
        }
    }

    /// The final consolidated response (`kind:"message"`, `text`).
    pub fn message_text(text: String) -> Self {
        Self {
            r#type: "item",
            kind: ItemKind::Message,
            delta: None,
            text: Some(text),
            name: None,
            input: None,
            ok: None,
            output: None,
        }
    }

    /// A tool call requested by the agent (`kind:"tool_call"`, `name`, `input`).
    pub fn tool_call(name: String, input: Value) -> Self {
        Self {
            r#type: "item",
            kind: ItemKind::ToolCall,
            delta: None,
            text: None,
            name: Some(name),
            input: Some(input),
            ok: None,
            output: None,
        }
    }

    /// The outcome of an executed or denied tool call
    /// (`kind:"tool_result"`, `name`, `ok`, `output`).
    pub fn tool_result(name: String, ok: bool, output: String) -> Self {
        Self {
            r#type: "item",
            kind: ItemKind::ToolResult,
            delta: None,
            text: None,
            name: Some(name),
            input: None,
            ok: Some(ok),
            output: Some(output),
        }
    }
}

/// `turn.completed` — emitted once after the single turn finishes.
#[derive(Debug, Clone, Serialize)]
pub struct TurnCompletedEvent {
    #[serde(rename = "type")]
    pub r#type: &'static str,
    pub turn_id: String,
    /// `true` when the agent chat completed without error or timeout. Tool
    /// failures do not flip this; they are reported per-call via
    /// `kind:"tool_result"` items.
    pub success: bool,
    pub response: String,
    pub duration_ms: u64,
}

impl TurnCompletedEvent {
    pub fn new(turn_id: String, success: bool, response: String, duration_ms: u64) -> Self {
        Self {
            r#type: "turn.completed",
            turn_id,
            success,
            response,
            duration_ms,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Event emitter
// ─────────────────────────────────────────────────────────────────────────────

/// Serializes an event struct into one JSONL line. These are plain flat
/// structs, so serialization cannot fail.
fn to_jsonl(event: &impl Serialize) -> String {
    serde_json::to_string(event).expect("exec event serialization cannot fail (plain flat struct)")
}

/// Output sink for an exec run. In JSONL mode every event is written as one
/// flat JSON object per line to the writer (stdout in production). In plain
/// mode every method is a no-op and the caller prints the final response
/// itself, so stdout still carries nothing but the response.
pub struct EventEmitter {
    json: bool,
    out: Box<dyn Write + Send>,
}

impl EventEmitter {
    /// Create an emitter writing to stdout. `json:false` disables all events.
    pub fn new(json: bool) -> Self {
        Self {
            json,
            out: Box::new(std::io::stdout()),
        }
    }

    /// Event sink for tests: buffers serialized lines instead of stdout.
    #[cfg(test)]
    fn with_buffer(json: bool, buf: Arc<std::sync::Mutex<Vec<u8>>>) -> Self {
        Self {
            json,
            out: Box::new(SharedBuf(buf)),
        }
    }

    fn write_line(&mut self, line: &str) {
        if !self.json {
            return;
        }
        let _ = writeln!(self.out, "{line}");
        let _ = self.out.flush();
    }

    pub fn thread_started(&mut self, thread_id: &str, model: Option<&str>, session_id: Option<&str>) {
        let event = ThreadStartedEvent::new(
            thread_id.to_string(),
            model.map(str::to_string),
            session_id.map(str::to_string),
        );
        self.write_line(&to_jsonl(&event));
    }

    pub fn turn_started(&mut self, turn_id: &str) {
        let event = TurnStartedEvent::new(turn_id.to_string());
        self.write_line(&to_jsonl(&event));
    }

    pub fn item(&mut self, item: &ItemEvent) {
        self.write_line(&to_jsonl(item));
    }

    pub fn turn_completed(&mut self, turn_id: &str, success: bool, response: &str, duration_ms: u64) {
        let event = TurnCompletedEvent::new(turn_id.to_string(), success, response.to_string(), duration_ms);
        self.write_line(&to_jsonl(&event));
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// --input-stdin parsing
// ─────────────────────────────────────────────────────────────────────────────

/// Parsed `--input-stdin` payload: `{"prompt": "...", "session_id": "..."}`.
#[derive(Debug)]
struct StdinInput {
    prompt: Option<String>,
    session_id: Option<String>,
}

/// Parse the `--input-stdin` JSON payload. Unknown fields are ignored so the
/// input format can grow without breaking older binaries.
fn parse_stdin_input(raw: &str) -> Result<StdinInput> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|e| anyhow!("--input-stdin: invalid JSON on stdin: {e}"))?;
    let obj = value.as_object().ok_or_else(|| {
        anyhow!("--input-stdin: expected a JSON object like {{\"prompt\": \"...\", \"session_id\": \"...\"}}")
    })?;
    let prompt = obj.get("prompt").and_then(Value::as_str).map(str::to_string);
    let session_id = obj
        .get("session_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok(StdinInput { prompt, session_id })
}

// ─────────────────────────────────────────────────────────────────────────────
// Runtime construction
// ─────────────────────────────────────────────────────────────────────────────

/// The minimal runtime needed for one headless turn.
struct ExecRuntime {
    agent: Arc<dyn Agent>,
    agent_name: String,
    /// Default model of the primary agent, when reported (used in
    /// `thread.started`).
    model_hint: Option<String>,
}

/// Load the config file and build the full exec runtime. Mirrors the server
/// path: a missing/blank config gets the non-AI bootstrap config so the error
/// message is "no agents configured" instead of a raw parse error.
async fn build_runtime(config_path: &Path) -> Result<ExecRuntime> {
    crate::config::defaults::ensure_bootstrap_config(config_path)?;
    let config = Arc::new(AppConfig::load(config_path)?);
    build_runtime_from_config(config, config_path).await
}

/// Build the exec runtime from an already-loaded config. Kept separate from
/// [`build_runtime`] so tests can inject a hand-built config.
async fn build_runtime_from_config(config: Arc<AppConfig>, config_path: &Path) -> Result<ExecRuntime> {
    if config.agents().is_empty() {
        bail!("no agents configured; run `go-on --setup` first");
    }

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .http1_only()
        .build()?;
    let capability_graph = Arc::new(std::sync::Mutex::new(CapabilityGraph::new()));
    let registry = Arc::new(AgentRegistry::from_config(
        Arc::clone(&config),
        http_client.clone(),
        Arc::clone(&capability_graph),
    )?);

    // SpawnAgentTool globals — mirror cli/chat setup_chat_environment so
    // spawn_agent works when the agent calls it.
    crate::orchestration::tool_extended::spawn_agent::init_spawn_agent_registry(registry.clone());
    let communication_bus =
        Arc::new(crate::agents::communication::bus::CommunicationBus::new());
    crate::orchestration::tool_extended::spawn_agent::init_spawn_agent_communication_bus(
        communication_bus,
    );
    crate::orchestration::tool_extended::spawn_agent::init_spawn_agent_budget();

    // Skill registry + i18n via the canonical bootstrap; skills must be
    // registered before skill_execute can resolve them.
    let skill_registry = match crate::core::bootstrap::perform_bootstrap(
        &crate::core::bootstrap::BootstrapConfig {
            enable_i18n: true,
            config_path: config_path.to_path_buf(),
        },
    )
    .await
    {
        Ok(registry) => Arc::new(std::sync::RwLock::new(registry)),
        Err(e) => {
            tracing::warn!(target: "go_on::cli::exec", "bootstrap skipped: {e}");
            Arc::new(std::sync::RwLock::new(
                crate::orchestration::skill::SkillRegistry::default(),
            ))
        }
    };
    crate::orchestration::tool::set_skill_registry(skill_registry);

    // Command sandbox must obey config, exactly like chat mode.
    crate::security::sandbox::init_command_sandbox(config.security.command_sandbox.clone());

    // The primary agent is the first configured agent by name (same rule as
    // the terminal chat path).
    let mut names: Vec<String> = config.agents().keys().cloned().collect();
    names.sort();
    let agent_name = names[0].clone();
    let agent = registry
        .get(&agent_name)
        .ok_or_else(|| anyhow!("agent '{agent_name}' not found in registry"))?;
    let model_hint = agent.default_model().map(|m| m.id);

    Ok(ExecRuntime {
        agent,
        agent_name,
        model_hint,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Single-turn engine
// ─────────────────────────────────────────────────────────────────────────────

/// Outcome of one exec turn.
struct TurnOutcome {
    response: String,
    success: bool,
}

/// Build the system message + user message and the principles list for the
/// agent chat call.
fn build_messages(runtime: &ExecRuntime, prompt: &str) -> (Vec<Message>, Vec<String>) {
    let tool_names = global_tool_registry().all_names().join(", ");
    let messages = vec![
        Message {
            role: "system".to_string(),
            content: format!(
                "You are go-on, an AI coding assistant running in headless exec mode as agent '{}'.\n\
                 You have access to the following tools via the __tool_call__:tool_name:{{\"arg\": \"value\"}} protocol:\n{tool_names}",
                runtime.agent_name
            ),
        },
        Message {
            role: "user".to_string(),
            content: prompt.to_string(),
        },
    ];
    let principles = vec![
        "You are a helpful AI coding assistant with access to tools.".to_string(),
        "Use the __tool_call__:tool_name:json_args protocol to invoke tools.".to_string(),
        format!("Built-in tools: {tool_names}"),
    ];
    (messages, principles)
}

/// Run the single agent chat call, streaming content deltas as `item`
/// events. Returns `(response, tool_calls, chat_ok)`; `chat_ok` is `false`
/// when the agent errored or the collection timed out.
///
/// Bounded by construction: per-chunk and overall timeouts mirror the ACP
/// pipeline, and there is no follow-up loop after this function returns.
async fn collect_turn_response(
    runtime: &ExecRuntime,
    prompt: &str,
    emitter: &mut EventEmitter,
) -> (String, Vec<(String, String)>, bool) {
    let (messages, principles) = build_messages(runtime, prompt);
    let principles_opt = if principles.is_empty() {
        None
    } else {
        Some(principles)
    };

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let sender = StreamingSender::from(tx);
    let agent = Arc::clone(&runtime.agent);
    let task = tokio::spawn(async move {
        agent.chat(messages, principles_opt, None, sender).await
    });
    let abort = task.abort_handle();

    let started = Instant::now();
    let mut response = String::new();
    let mut reasoning = String::new();
    let mut tool_calls: Vec<(String, String)> = Vec::new();
    let mut chunks = 0usize;
    let mut total_chars = 0usize;
    let mut interrupted = false; // timeout or stream-cap truncation

    loop {
        let elapsed = started.elapsed();
        if elapsed >= OVERALL_TURN_TIMEOUT {
            tracing::warn!(
                target: "go_on::cli::exec",
                "agent streaming overall timeout after {}s — aborting",
                OVERALL_TURN_TIMEOUT.as_secs()
            );
            interrupted = true;
            break;
        }
        let remaining = OVERALL_TURN_TIMEOUT - elapsed;
        let token = match tokio::time::timeout(PER_CHUNK_TIMEOUT.min(remaining), rx.recv()).await {
            Ok(Some(token)) => token,
            // Receiver closed — the agent finished streaming.
            Ok(None) => break,
            Err(_) => {
                tracing::warn!(
                    target: "go_on::cli::exec",
                    "agent streaming recv() timed out after {}s — aborting",
                    PER_CHUNK_TIMEOUT.as_secs()
                );
                interrupted = true;
                break;
            }
        };

        match classify_agent_token(&token) {
            AgentToken::ModelUsed(_) => {}
            AgentToken::ToolCall(name, args) => tool_calls.push((name, args)),
            AgentToken::ReasoningMarker | AgentToken::Telemetry => {}
            AgentToken::Reasoning(text) => {
                let next_chars = text.chars().count();
                if stream_would_exceed_limits(chunks, total_chars, next_chars) {
                    tracing::warn!(
                        target: "go_on::cli::exec",
                        "output truncated at {total_chars} chars (chunks {chunks})"
                    );
                    interrupted = true;
                    break;
                }
                reasoning.push_str(&text);
                chunks += 1;
                total_chars += next_chars;
            }
            AgentToken::Content(text) => {
                let next_chars = text.chars().count();
                if stream_would_exceed_limits(chunks, total_chars, next_chars) {
                    tracing::warn!(
                        target: "go_on::cli::exec",
                        "output truncated at {total_chars} chars (chunks {chunks})"
                    );
                    interrupted = true;
                    break;
                }
                response.push_str(&text);
                chunks += 1;
                total_chars += next_chars;
                emitter.item(&ItemEvent::message_delta(text));
            }
        }
    }

    // If we bailed out early the agent task may still be streaming — stop it.
    if interrupted {
        abort.abort();
    }

    let chat_failed = match tokio::time::timeout(PER_CHUNK_TIMEOUT, task).await {
        Ok(Ok(Ok(()))) => false,
        Ok(Ok(Err(e))) => {
            tracing::warn!(target: "go_on::cli::exec", "agent chat failed: {e}");
            true
        }
        Ok(Err(e)) => {
            tracing::warn!(target: "go_on::cli::exec", "agent chat task panicked: {e}");
            true
        }
        Err(_) => {
            tracing::warn!(
                target: "go_on::cli::exec",
                "agent chat join timed out — aborting"
            );
            abort.abort();
            true
        }
    };

    // When the agent produced only reasoning, use it as the response
    // (mirrors the ACP collection core).
    if response.trim().is_empty() && !reasoning.trim().is_empty() {
        response = reasoning;
    }

    (response, tool_calls, !chat_failed && !interrupted)
}

/// Parse a tool-call arguments string into a JSON value, falling back to the
/// raw string when it is not valid JSON.
fn parse_tool_input(args: &str) -> Value {
    serde_json::from_str(args).unwrap_or_else(|_| json!(args))
}

/// Execute the intercepted tool calls under the approval policy, emitting
/// `tool_call` and `tool_result` items. Never prompts.
async fn execute_tool_calls(
    tool_calls: &[(String, String)],
    approval: ApprovalMode,
    emitter: &mut EventEmitter,
) {
    if tool_calls.is_empty() {
        return;
    }

    // Skill dedup (shared with the CLI chat / ACP paths): when the agent
    // emits several skill calls at once, keep every non-skill tool and only
    // the best-scored skill.
    let calls: Vec<(String, String)> = match crate::orchestration::tool::skill_registry() {
        Some(registry) => crate::orchestration::tool::dedup_skill_calls(tool_calls, registry).0,
        None => tool_calls.to_vec(),
    };

    let mut approved: Vec<(String, String)> = Vec::new();
    for (name, args) in &calls {
        emitter.item(&ItemEvent::tool_call(name.clone(), parse_tool_input(args)));
        let gate = match approval {
            ApprovalMode::Never => Ok(()),
            ApprovalMode::Auto => {
                let args_value = serde_json::from_str::<Value>(args).unwrap_or(Value::Null);
                quick_check_tool(name, &args_value)
            }
        };
        match gate {
            Ok(()) => approved.push((name.clone(), args.clone())),
            Err(reason) => {
                let message = format!("governance denied: {reason}");
                emitter.item(&ItemEvent::tool_result(name.clone(), false, message));
            }
        }
    }

    if approved.is_empty() {
        return;
    }

    // Same execution config as the interactive chat path: no in-executor
    // governance (approval was already decided above) and no ACP notifications.
    let exec_result = execute_tools_concurrent(
        &approved,
        global_tool_registry(),
        &ToolExecConfig {
            max_concurrency: MAX_CONCURRENT_TOOLS,
            circuit_breaker_limit: 0,
            operation_mode: "ask".to_string(),
            governance_required: false,
            is_safeguard: false,
            acp_session_id: None,
        },
        None,
        "go-on exec",
        0,
    )
    .await;

    for item in exec_result.tool_results {
        let output = item
            .output
            .result
            .as_ref()
            .map(|r| r.to_string())
            .or_else(|| item.output.error.clone())
            .unwrap_or_else(|| "success".to_string());
        emitter.item(&ItemEvent::tool_result(item.tool_name.clone(), item.success, output));
    }
}

/// Run one bounded exec turn, emitting the full JSONL event sequence
/// (`thread.started` → `turn.started` → `item.*` → `turn.completed`).
async fn execute_turn(
    runtime: &ExecRuntime,
    prompt: &str,
    session_id: Option<&str>,
    approval: ApprovalMode,
    thread_id: &str,
    turn_id: &str,
    emitter: &mut EventEmitter,
) -> TurnOutcome {
    emitter.thread_started(thread_id, runtime.model_hint.as_deref(), session_id);
    emitter.turn_started(turn_id);

    let turn_started = Instant::now();
    let (response, tool_calls, chat_ok) =
        collect_turn_response(runtime, prompt, &mut *emitter).await;

    if !tool_calls.is_empty() {
        execute_tool_calls(&tool_calls, approval, &mut *emitter).await;
    }

    if !response.trim().is_empty() {
        emitter.item(&ItemEvent::message_text(response.clone()));
    }

    let duration_ms = turn_started.elapsed().as_millis() as u64;
    emitter.turn_completed(turn_id, chat_ok, &response, duration_ms);

    TurnOutcome {
        response,
        success: chat_ok,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Entry point (wired in src/main/mod.rs)
// ─────────────────────────────────────────────────────────────────────────────

/// Run the `go-on exec` subcommand. `config_path` is the resolved config
/// file, `json` selects JSONL mode, `approval_mode` is `--approval-mode`
/// (`None` → `never`), `input_stdin` enables `--input-stdin`, and `prompt` is
/// the positional prompt.
pub async fn run_exec(
    config_path: &Path,
    json: bool,
    approval_mode: Option<&str>,
    input_stdin: bool,
    prompt: Option<&str>,
) -> Result<()> {
    let approval = parse_approval_mode(approval_mode.unwrap_or("never"))?;

    let (prompt, session_id) = if input_stdin {
        let mut raw = String::new();
        std::io::stdin().read_to_string(&mut raw)?;
        let input = parse_stdin_input(&raw)?;
        let prompt = input
            .prompt
            .or_else(|| prompt.map(str::to_string))
            .ok_or_else(|| {
                anyhow!("--input-stdin JSON must include a \"prompt\" field (or pass a positional prompt)")
            })?;
        (prompt, input.session_id)
    } else {
        let prompt = prompt
            .map(str::to_string)
            .ok_or_else(|| anyhow!("missing prompt: pass a positional prompt or use --input-stdin"))?;
        (prompt, None)
    };

    let runtime = build_runtime(config_path).await?;
    let thread_id = uuid::Uuid::new_v4().to_string();
    let turn_id = uuid::Uuid::new_v4().to_string();

    let mut emitter = EventEmitter::new(json);
    let outcome =
        execute_turn(&runtime, &prompt, session_id.as_deref(), approval, &thread_id, &turn_id, &mut emitter).await;

    if !json && !outcome.response.trim().is_empty() {
        println!("{}", outcome.response.trim());
    }

    if !outcome.success {
        bail!(
            "exec turn failed — see the JSONL event stream (turn.completed success=false) for details"
        );
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

/// Write sink that appends into a shared byte buffer (stdout stand-in for
/// tests).
#[cfg(test)]
struct SharedBuf(Arc<std::sync::Mutex<Vec<u8>>>);

#[cfg(test)]
impl Write for SharedBuf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use crate::config::{
        AgentConfig, FeatureConfig, FlowConfig, ProviderConfig, RuntimeConfig, SecurityConfig,
    };

    fn capture_emitter(json: bool) -> (EventEmitter, Arc<std::sync::Mutex<Vec<u8>>>) {
        let buf = Arc::new(std::sync::Mutex::new(Vec::new()));
        let emitter = EventEmitter::with_buffer(json, Arc::clone(&buf));
        (emitter, buf)
    }

    fn parse_lines(buf: &Arc<std::sync::Mutex<Vec<u8>>>) -> Vec<Value> {
        let bytes = buf.lock().unwrap();
        String::from_utf8_lossy(&bytes)
            .lines()
            .map(|l| serde_json::from_str(l).expect("each line must be valid JSON"))
            .collect()
    }

    // ── Event serialization contract ──

    #[test]
    fn thread_started_serializes_flat_shape_with_optional_fields() {
        let event = ThreadStartedEvent::new(
            "thread-1".to_string(),
            Some("gpt-4.1".to_string()),
            Some("sess-9".to_string()),
        );
        let line = serde_json::to_string(&event).unwrap();
        assert_eq!(
            line,
            r#"{"type":"thread.started","thread_id":"thread-1","model":"gpt-4.1","session_id":"sess-9"}"#
        );

        let minimal = ThreadStartedEvent::new("thread-1".to_string(), None, None);
        let line = serde_json::to_string(&minimal).unwrap();
        assert_eq!(line, r#"{"type":"thread.started","thread_id":"thread-1"}"#);
    }

    #[test]
    fn turn_started_serializes_flat_shape() {
        let event = TurnStartedEvent::new("turn-1".to_string());
        let line = serde_json::to_string(&event).unwrap();
        assert_eq!(line, r#"{"type":"turn.started","turn_id":"turn-1"}"#);
    }

    #[test]
    fn message_item_serializes_delta_and_text_forms() {
        let delta = ItemEvent::message_delta("hello ".to_string());
        assert_eq!(
            serde_json::to_string(&delta).unwrap(),
            r#"{"type":"item","kind":"message","delta":"hello "}"#
        );

        let text = ItemEvent::message_text("hello world".to_string());
        assert_eq!(
            serde_json::to_string(&text).unwrap(),
            r#"{"type":"item","kind":"message","text":"hello world"}"#
        );
    }

    #[test]
    fn tool_call_item_serializes_flat_shape() {
        let event = ItemEvent::tool_call("read_file".to_string(), json!({"path": "a.txt"}));
        assert_eq!(
            serde_json::to_string(&event).unwrap(),
            r#"{"type":"item","kind":"tool_call","name":"read_file","input":{"path":"a.txt"}}"#
        );
    }

    #[test]
    fn tool_result_item_serializes_flat_shape() {
        let event = ItemEvent::tool_result("read_file".to_string(), true, "ok".to_string());
        assert_eq!(
            serde_json::to_string(&event).unwrap(),
            r#"{"type":"item","kind":"tool_result","name":"read_file","ok":true,"output":"ok"}"#
        );
    }

    #[test]
    fn turn_completed_serializes_flat_shape() {
        let event = TurnCompletedEvent::new("turn-1".to_string(), true, "done".to_string(), 123);
        assert_eq!(
            serde_json::to_string(&event).unwrap(),
            r#"{"type":"turn.completed","turn_id":"turn-1","success":true,"response":"done","duration_ms":123}"#
        );
    }

    // ── CLI input parsing ──

    #[test]
    fn approval_mode_parses_valid_values_and_rejects_others() {
        assert_eq!(parse_approval_mode("never").unwrap(), ApprovalMode::Never);
        assert_eq!(parse_approval_mode("auto").unwrap(), ApprovalMode::Auto);
        let err = parse_approval_mode("always").unwrap_err();
        assert!(err.to_string().contains("invalid --approval-mode 'always'"));
    }

    #[test]
    fn stdin_input_parses_prompt_and_session_id() {
        let input = parse_stdin_input(r#"{"prompt":"say hi","session_id":"sess-9"}"#).unwrap();
        assert_eq!(input.prompt.as_deref(), Some("say hi"));
        assert_eq!(input.session_id.as_deref(), Some("sess-9"));

        let partial = parse_stdin_input(r#"{"prompt":"only prompt"}"#).unwrap();
        assert_eq!(partial.prompt.as_deref(), Some("only prompt"));
        assert_eq!(partial.session_id, None);

        let err = parse_stdin_input("not json").unwrap_err();
        assert!(err.to_string().contains("invalid JSON on stdin"));

        let err = parse_stdin_input("[1,2]").unwrap_err();
        assert!(err.to_string().contains("expected a JSON object"));
    }

    // ── Headless end-to-end turn with a local echo agent ──

    fn local_echo_config() -> AppConfig {
        let mut agents = HashMap::new();
        agents.insert(
            "primary".to_string(),
            AgentConfig {
                agent_type: "local_echo".to_string(),
                url: None,
                chat_path: None,
                api_key_env: None,
                secret_key_env: None,
                anthropic_version: None,
                model: None,
                max_tokens: None,
                supports_system: None,
                supports_vision: None,
            },
        );
        AppConfig {
            schema_version: "1.0.0".to_string(),
            layered_merge: false,
            provider: ProviderConfig {
                default_phase: "coding".to_string(),
                agents,
                role_registry: HashMap::new(),
            },
            flow: FlowConfig::default(),
            phases: HashMap::new(),
            runtime: Some(RuntimeConfig::default()),
            cache: None,
            vector: None,
            autotune: None,
            security: SecurityConfig::default(),
            feature: FeatureConfig::default(),
            compliance: None,
            startup_context: None,
            protocol: None,
        }
    }

    #[tokio::test]
    async fn exec_turn_emits_contract_jsonl_sequence() {
        // Local test agents are gated behind this env var (build_agent).
        std::env::set_var("GO_ON_ENABLE_LOCAL_TEST_AGENTS", "true");
        let config = Arc::new(local_echo_config());
        let tmp = tempfile::tempdir().expect("tempdir");
        let runtime = build_runtime_from_config(config, tmp.path())
            .await
            .expect("runtime must build with a local_echo agent");

        let (mut emitter, buf) = capture_emitter(true);
        let outcome = execute_turn(
            &runtime,
            "say hi",
            Some("sess-9"),
            ApprovalMode::Never,
            "thread-1",
            "turn-1",
            &mut emitter,
        )
        .await;

        let events = parse_lines(&buf);
        assert!(events.len() >= 4, "expected at least 4 events, got {events:?}");

        assert_eq!(events[0]["type"], "thread.started");
        assert_eq!(events[0]["thread_id"], "thread-1");
        assert_eq!(events[0]["session_id"], "sess-9");
        assert_eq!(events[1]["type"], "turn.started");
        assert_eq!(events[1]["turn_id"], "turn-1");

        let items: Vec<&Value> = events
            .iter()
            .filter(|e| e["type"] == "item")
            .collect();
        assert!(!items.is_empty(), "expected at least one item event");
        for item in &items {
            let kind = item["kind"].as_str().expect("item must carry kind");
            assert!(matches!(kind, "message" | "tool_call" | "tool_result"));
        }

        let last = events.last().expect("turn.completed must be present");
        assert_eq!(last["type"], "turn.completed");
        assert_eq!(last["turn_id"], "turn-1");
        assert_eq!(last["success"], true);
        assert_eq!(last["response"], "say hi");
        assert!(last["duration_ms"].as_u64().is_some());

        assert!(outcome.success);
        assert_eq!(outcome.response, "say hi");
    }

    #[tokio::test]
    async fn plain_emitter_writes_nothing_to_stdout() {
        std::env::set_var("GO_ON_ENABLE_LOCAL_TEST_AGENTS", "true");
        let config = Arc::new(local_echo_config());
        let tmp = tempfile::tempdir().expect("tempdir");
        let runtime = build_runtime_from_config(config, tmp.path())
            .await
            .expect("runtime must build with a local_echo agent");

        let (mut emitter, buf) = capture_emitter(false);
        let outcome = execute_turn(
            &runtime,
            "say hi",
            None,
            ApprovalMode::Never,
            "thread-1",
            "turn-1",
            &mut emitter,
        )
        .await;

        // Plain mode: no JSONL lines at all; the response is returned for the
        // caller to print itself.
        assert!(buf.lock().unwrap().is_empty());
        assert_eq!(outcome.response, "say hi");
    }
}
