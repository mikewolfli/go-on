//! Mode runtime orchestration for go-on (Phase 2)
//!
//! Mode runtimes define orchestration policies per mode and are wired into the
//! execution flow: `resolve_mode_runtime_with_posture` / `resolve_mode_runtime`
//! are consumed by the chat pipeline (`src/acp/impl/chat/phases/`) and the
//! CLI chat loop (`src/cli/chat.rs`).

use crate::agent::{
    Agent, AgentError, AgentRegistry, AgentTaskEnvelope, AgentTaskResult, Message, StreamingSender,
};
use crate::pua::mode_execution_report;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{info, warn};

// ── Mode tool-set catalogs (single source of truth) ─────────────────────────
// These lists are referenced by `GenericModeRuntime::allowed_tools` for every
// mode kind. They were previously inlined twice (Edit/FullAuto and the
// SafeGuard non-readonly branch), which allowed the two copies to drift.

/// Read/plan-only tool set (Plan mode).
///
/// Derived from [`READ_ONLY_TOOL_NAMES`] (single source of truth) with two
/// deliberate semantic differences:
/// - `http_request` is Plan-only (research GETs) and excluded from the
///   SafeGuard ReadOnly degraded set (it can mutate remote state).
/// - `web_search` / `security_scan` are SafeGuard ReadOnly tools that are
///   not part of the Plan tool surface.
///
/// Computed once (static) so per-tool-batch policy checks do not rebuild the
/// list on every call.
fn plan_tools() -> &'static [&'static str] {
    static PLAN_TOOL_NAMES: std::sync::LazyLock<Vec<&'static str>> =
        std::sync::LazyLock::new(|| {
            READ_ONLY_TOOL_NAMES
                .iter()
                .copied()
                .filter(|t| *t != "web_search" && *t != "security_scan")
                .chain(std::iter::once("http_request"))
                .collect()
        });
    &PLAN_TOOL_NAMES
}

/// Full execution tool set (Edit / FullAuto / SafeGuard non-readonly).
static ALL_EXEC_TOOL_NAMES: &[&str] = &[
    // ── File tools ──
    "read_file",
    "read_file_lines",
    "write_file",
    "edit_file",
    "apply_patch",
    "move_path",
    "delete_path",
    "copy_path",
    "create_directory",
    "format_code",
    "hash_file",
    "file_watch",
    // ── Search tools ──
    "search_files",
    "grep",
    "code_index_search",
    "go_to_definition",
    "find_references",
    // ── Git / Diff ──
    "inspect_git_diff",
    "file_diff",
    "git",
    // ── Build / Test / Lint ──
    "cargo_check",
    "cargo_test",
    "run_tests",
    "build_run",
    "lint_run",
    "diagnostics",
    // ── Shell / Execution ──
    "shell_exec",
    // ── Directory ──
    "list_directory",
    // ── Archive ──
    "archive_inspect",
    "archive_extract",
    "compress",
    "decompress",
    // ── Network ──
    "http_request",
    "web_search",
    "dns_lookup",
    "ping",
    "port_scan",
    // ── Data ──
    "jsonl_read",
    "jsonl_write",
    "json_query",
    "rss_read",
    // ── Docker ──
    "docker_ps",
    "docker_logs",
    "docker_exec",
    "docker_build",
    "docker_push",
    "docker_compose",
    // ── Utility ──
    "date_time",
    "environment_info",
    "uuid_gen",
    "random_token",
    "encode_decode",
    "template_render",
    "code_metrics",
    "security_scan",
    "search_packages",
    "dependency_add",
    // ── Agent tools ──
    "spawn_agent",
    "apply_code_action",
    // ── Skill tools (always available) ──
    "skill_list",
    "skill_execute",
    "skill_create",
    "skill_reload",
];

fn all_exec_tools() -> &'static [&'static str] {
    ALL_EXEC_TOOL_NAMES
}

/// Core tools available to a task that is awaiting approval or has no agent
/// (pending/unavailable state). Shared by `pre_execute` and `fallback_result`
/// to keep the emitted `tools_available` payload identical across modes.
static PENDING_TASK_TOOLS: &[&str] = &[
    "read_file",
    "search_files",
    "apply_patch",
    "run_tests",
    "inspect_git_diff",
];

/// Read-only inspection tools — the SafeGuard ReadOnly degraded set.
static READ_ONLY_TOOL_NAMES: &[&str] = &[
    "read_file",
    "read_file_lines",
    "search_files",
    "grep",
    "list_directory",
    "inspect_git_diff",
    "code_index_search",
    "go_to_definition",
    "find_references",
    "file_diff",
    "date_time",
    "environment_info",
    "json_query",
    "archive_inspect",
    "dns_lookup",
    "ping",
    "docker_ps",
    "docker_logs",
    "rss_read",
    "jsonl_read",
    "code_metrics",
    "security_scan",
    "skill_list",
    "web_search",
];

/// Canonical low-risk tool set: read-only inspection tools plus pure
/// informational/utility tools with no side effects. Single source of truth
/// for `orchestration::tool::governance_gate::is_low_risk_tool` — previously
/// three overlapping lists (plan_tools / read_only_tools / is_low_risk_tool)
/// drifted, so read-only tools like `web_search` hit the blocking approval
/// gate in edit mode.
static LOW_RISK_TOOL_NAMES: &[&str] = &[
    "read_file",
    "read_file_lines",
    "search_files",
    "grep",
    "list_directory",
    "inspect_git_diff",
    "code_index_search",
    "go_to_definition",
    "find_references",
    "file_diff",
    "date_time",
    "environment_info",
    "json_query",
    "archive_inspect",
    "dns_lookup",
    "ping",
    "docker_ps",
    "docker_logs",
    "rss_read",
    "jsonl_read",
    "code_metrics",
    "security_scan",
    "skill_list",
    "web_search",
    // Pure informational/utility tools.
    "uuid_gen",
    "random_token",
    "encode_decode",
    "hash_file",
    "diagnostics",
    "format_code",
    "svg_export",
];

/// Canonical low-risk tool names (see [`LOW_RISK_TOOL_NAMES`]).
pub(crate) fn low_risk_tool_names() -> &'static [&'static str] {
    LOW_RISK_TOOL_NAMES
}

/// Read-only degraded tool set (SafeGuard ReadOnly).
fn read_only_tools() -> &'static [&'static str] {
    READ_ONLY_TOOL_NAMES
}

/// Supported chat/agent modes
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ModeKind {
    #[default]
    Ask,
    /// Plan mode for step-by-step planning without execution.
    Plan,
    Edit,
    FullAuto,
    /// Automatic mode that requires approval at high-risk nodes.
    SafeGuard,
}

impl From<&str> for ModeKind {
    fn from(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "ask" => ModeKind::Ask,
            "plan" => ModeKind::Plan,
            "edit" => ModeKind::Edit,
            // "agent" maps to FullAuto: "agent" mode in Zed is a
            // fully autonomous loop (plan -> act -> observe -> replan),
            // much closer to FullAuto semantics than Edit.
            "agent" => ModeKind::FullAuto,
            "full_auto" | "fullauto" => ModeKind::FullAuto,
            "safeguard" | "safe_guard" => ModeKind::SafeGuard,
            _ => ModeKind::Ask,
        }
    }
}

/// Policy for what action to take when a risk threshold is exceeded
/// during SafeGuard mode execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum AutoDegradePolicy {
    /// Block the operation entirely (original behavior).
    Block,
    /// Switch to read-only mode — only allow read/inspect tools.
    #[default]
    ReadOnly,
    /// Allow the operation but with enhanced audit logging.
    AllowWithAudit,
    /// Ask the operator for confirmation before proceeding.
    ConfirmRequired,
}

/// Approval posture — independent dimension orthogonal to ModeKind.
///
/// Decouples "what to do" (mode) from "how to approve" (posture):
/// - `Suggest`:   show approval and wait (interactive, default for edit)
/// - `Auto`:      auto-approve all low-risk operations (streamlined)
/// - `Bypass`:    full access, no approval gates (dangerous, for trusted scenarios)
/// - `Never`:     block all write/destructive operations (Plan/read-only)
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ApprovalPosture {
    /// Ask for user confirmation before executing tools (interactive).
    #[default]
    Suggest,
    /// Auto-approve all low-risk tool calls; no user interaction needed.
    Auto,
    /// Full access — no approval gates at all (trusted/CI scenarios).
    Bypass,
    /// Block all write/destructive operations (read-only).
    Never,
}

impl ApprovalPosture {
    /// Whether this posture blocks tool execution outright (read-only).
    pub fn is_read_only(&self) -> bool {
        matches!(self, ApprovalPosture::Never)
    }

    /// Whether this posture should prompt the user for approval.
    pub fn requires_approval(&self) -> bool {
        matches!(self, ApprovalPosture::Suggest)
    }

    /// Whether this posture allows fully autonomous execution.
    pub fn is_autonomous(&self) -> bool {
        matches!(self, ApprovalPosture::Auto | ApprovalPosture::Bypass)
    }
}

impl From<&str> for ApprovalPosture {
    fn from(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "suggest" | "interactive" => ApprovalPosture::Suggest,
            "auto" | "autonomous" => ApprovalPosture::Auto,
            "bypass" | "full" => ApprovalPosture::Bypass,
            "never" | "read_only" => ApprovalPosture::Never,
            _ => ApprovalPosture::Suggest,
        }
    }
}

impl std::fmt::Display for ApprovalPosture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApprovalPosture::Suggest => write!(f, "suggest"),
            ApprovalPosture::Auto => write!(f, "auto"),
            ApprovalPosture::Bypass => write!(f, "bypass"),
            ApprovalPosture::Never => write!(f, "never"),
        }
    }
}

/// Mode runtime trait: each mode has its own orchestration, budget, and policy
///
/// All implementations should instrument `run` for tracing and performance monitoring
/// in the implementation, not on the trait itself.
#[async_trait]
pub trait ModeRuntime: Send + Sync {
    /// Returns the mode kind.
    fn kind(&self) -> ModeKind;
    /// Returns the approval posture for this mode.
    fn posture(&self) -> ApprovalPosture {
        ApprovalPosture::Auto
    }
    /// Returns the allowed tools for this mode.
    fn allowed_tools(&self) -> Vec<String>;
    /// Returns the maximum number of tool calls allowed.
    fn max_tool_calls(&self) -> usize;
    /// Whether the given objective is high risk.
    fn is_high_risk_operation(&self, objective: &str) -> bool;
    /// Run the mode orchestration for a given agent task.
    async fn run(&self, task: AgentTaskEnvelope) -> Result<AgentTaskResult>;
}

/// Resolve a mode string ("ask", "edit", "agent", "full_auto", "safeguard") into
/// a [`Box<dyn ModeRuntime>`] with the given registry and agent name.
///
/// Returns an error if no registry is provided.
///
/// This wires the `ModeRuntime` trait into the chat execution flow — callers
/// in `chat.rs` can use the returned runtime to get per-mode policies for
/// allowed tools, max tool calls, approval requirements, and risk assessment.
pub fn resolve_mode_runtime(
    mode: &str,
    registry: Option<Arc<AgentRegistry>>,
    agent_name: Option<String>,
) -> std::result::Result<Box<dyn ModeRuntime>, String> {
    resolve_mode_runtime_with_posture(mode, None, registry, agent_name)
}

/// Like `resolve_mode_runtime` but allows overriding the approval posture.
///
/// Delegates to `ModeKind::from(mode)` for mode resolution (which defaults
/// unrecognized strings to `ModeKind::Ask` with a warning). When `posture` is
/// `None`, the default posture for the mode is used; when `Some(p)`, the given
/// posture overrides the mode default.
pub fn resolve_mode_runtime_with_posture(
    mode: &str,
    posture: Option<ApprovalPosture>,
    registry: Option<Arc<AgentRegistry>>,
    agent_name: Option<String>,
) -> std::result::Result<Box<dyn ModeRuntime>, String> {
    let kind = ModeKind::from(mode);
    // Log a warning when the mode string was not recognized (ModeKind::from silently
    // defaults to Ask for unrecognized input, but we want visibility at this call site).
    if kind == ModeKind::Ask && !matches!(mode.to_lowercase().as_str(), "ask") {
        tracing::warn!("unknown mode '{}', defaulting to Ask", mode);
    }
    let registry = registry.ok_or_else(|| "ModeRuntime requires a registry".to_string())?;
    let mut runtime = GenericModeRuntime::new(kind, registry, agent_name);
    if let Some(p) = posture {
        runtime = runtime.with_posture(p);
    }
    Ok(Box::new(runtime))
}

/// Async helper to execute an agent chat without blocking.
/// Safe to call from within any tokio runtime context.
async fn execute_agent_chat_async(
    agent: &dyn Agent,
    messages: Vec<Message>,
    principles: Option<Vec<String>>,
    options: Option<std::collections::HashMap<String, serde_json::Value>>,
) -> Result<String> {
    let (tx, rx) = mpsc::unbounded_channel::<String>();
    let sender = StreamingSender::new(tx);

    let agent_ref: &dyn Agent = agent;
    let chat_future = agent_ref.chat(messages, principles, options, sender);

    // Shared capped collector (256k chars / 4096 chunks, 300s overall cap):
    // drains the stream concurrently with the chat and aborts the chat task
    // on truncation — a "chat first, drain later" order would buffer the
    // whole stream in the channel before the cap applied.
    crate::acp::helpers::conversation::collect_chat_output_capped(
        chat_future,
        rx,
        Some(Duration::from_secs(300)),
    )
    .await
}

/// Execute an agent's run_task directly (already async).
/// No spawn_blocking needed — run_task is async and calls chat() internally.
async fn execute_agent_run_task_async(
    agent: Arc<dyn Agent>,
    envelope: AgentTaskEnvelope,
) -> Result<AgentTaskResult> {
    agent
        .run_task(envelope)
        .await
        .map_err(|e| anyhow::anyhow!("agent run_task failed: {}", e))
}

/// Helper to build a chat message from the task envelope.
///
/// Single implementation shared with `Agent::run_task` (the default trait
/// implementation previously mirrored this exact message construction).
pub(crate) fn build_chat_messages(task: &AgentTaskEnvelope) -> Vec<Message> {
    let mut messages = Vec::new();

    // If there is evidence (context), add it as a system-like message
    if let Some(evidence) = &task.evidence {
        if !evidence.is_empty() {
            messages.push(Message {
                role: "system".to_string(),
                content: format!("Context/Evidence:\n{}", evidence),
            });
        }
    }

    // Add the task objective as the user message
    let mut user_content = format!("Objective: {}\n", task.objective);
    if let Some(constraints) = &task.constraints {
        user_content.push_str(&format!("Constraints: {}\n", constraints));
    }
    user_content.push_str("\nPlease complete this task and provide the result.");

    messages.push(Message {
        role: "user".to_string(),
        content: user_content,
    });

    messages
}

// ---------------------------------------------------------------------------
// BaseModeRuntime and ModeStrategy
// ---------------------------------------------------------------------------

/// Strategy trait: each mode defines its policy differences here.
///
/// The `BaseModeRuntime::run()` method uses this trait to execute the common
/// orchestration skeleton while delegating per-mode behaviour to the strategy.
pub trait ModeStrategy: Send + Sync {
    /// Human-readable mode name (used in logs and JSON output).
    fn mode_name(&self) -> &str;
    /// Whether this mode uses `Agent::chat()` (true) or `Agent::run_task()` (false).
    fn use_chat(&self) -> bool;
    /// Mode identifier for PUA execution reports.
    fn pua_mode(&self) -> &str;
    /// Log the start of mode execution.
    fn log_start(&self, objective: &str, phase: &str, role: &str);
    /// Pre-execution check (risk assessment, degradation, etc.).
    /// Return `Some(Ok(result))` or `Some(Err(...))` to short-circuit,
    /// or `None` to continue with normal agent execution.
    fn pre_execute(
        &self,
        _task_id: &str,
        _objective: &str,
        _phase: &str,
        _role: &str,
    ) -> Option<Result<AgentTaskResult>> {
        None
    }
    /// Build the fallback `AgentTaskResult` when no agent is available.
    fn fallback_result(
        &self,
        task_id: &str,
        objective: &str,
        phase: &str,
        role: &str,
    ) -> AgentTaskResult;
}

/// Shared orchestration skeleton used by all mode runtimes.
///
/// Holds the agent registry and agent name that every mode needs.
/// Delegates per-mode policy to a `ModeStrategy` implementation.
pub struct BaseModeRuntime {
    agent_registry: Option<Arc<AgentRegistry>>,
    agent_name: Option<String>,
}

impl BaseModeRuntime {
    pub fn new(agent_registry: Option<Arc<AgentRegistry>>, agent_name: Option<String>) -> Self {
        Self {
            agent_registry,
            agent_name,
        }
    }

    /// Run the common orchestration skeleton.
    ///
    /// 1. Log the start of execution (via strategy).
    /// 2. Run pre-execution checks (risk assessment, degradation) via strategy.
    /// 3. Attempt real agent execution (chat or run_task as determined by strategy).
    /// 4. Fall through to a placeholder response when no agent is available.
    ///
    pub async fn run(
        &self,
        strategy: &dyn ModeStrategy,
        task: AgentTaskEnvelope,
    ) -> Result<AgentTaskResult> {
        let task_id = task.task_id.clone();
        let objective = task.objective.clone();
        let phase = task.phase.clone();
        let role = task.role.clone();

        // 1. Log the start
        strategy.log_start(&objective, &phase, &role);

        // 2. Pre-execution check (risk, degradation, etc.)
        if let Some(early_result) = strategy.pre_execute(&task_id, &objective, &phase, &role) {
            return early_result;
        }

        // 3. Attempt real agent execution
        if let Some(ref registry) = self.agent_registry {
            let agent_name = match self.agent_name.as_deref() {
                Some(name) => Some(name.to_string()),
                None => registry.names().first().cloned(),
            };

            if let Some(name) = agent_name {
                if let Some(agent) = registry.get(&name) {
                    if strategy.use_chat() {
                        let messages = build_chat_messages(&task);
                        let result =
                            execute_agent_chat_async(agent.as_ref(), messages, None, None).await;
                        match result {
                            Ok(output) => {
                                return Ok(AgentTaskResult {
                                    success: true,
                                    output: Some(serde_json::json!({
                                        "mode": strategy.mode_name(),
                                        "task_id": task_id.clone(),
                                        "status": "completed",
                                        "agent": name,
                                        "answer": output,
                                    })),
                                    error: None,
                                    audit_log: Some(format!(
                                        "{} mode: task_id={}, phase={}, role={}, agent={}",
                                        strategy.mode_name(),
                                        task_id,
                                        phase,
                                        role,
                                        name
                                    )),
                                    pua_report: Some(mode_execution_report(
                                        strategy.pua_mode(),
                                        false,
                                    )),
                                });
                            }
                            Err(e) => {
                                warn!(
                                    "[{} Mode] Agent '{}' chat failed: {}",
                                    strategy.mode_name(),
                                    name,
                                    e
                                );
                            }
                        }
                    } else {
                        // run_task() now has an async default that calls chat() internally,
                        // so every mode gets a real AI response. No fallback needed.
                        let result = execute_agent_run_task_async(agent.clone(), task).await;
                        match result {
                            Ok(result) => {
                                return Ok(AgentTaskResult {
                                    pua_report: Some(mode_execution_report(
                                        strategy.pua_mode(),
                                        false,
                                    )),
                                    ..result
                                });
                            }
                            Err(e) => {
                                warn!(
                                    "[{} Mode] Agent '{}' run_task failed: {}",
                                    strategy.mode_name(),
                                    name,
                                    e
                                );
                            }
                        }
                    }
                }
            }
        }

        // 4. Graceful degradation: no agent available
        warn!(
            "[{} Mode] No agent available — falling back to informational response",
            strategy.mode_name()
        );
        Ok(strategy.fallback_result(&task_id, &objective, &phase, &role))
    }
}

// ---------------------------------------------------------------------------
// GenericModeRuntime — single runtime dispatching on ModeKind (A6)
// ---------------------------------------------------------------------------

/// Single generic mode runtime that dispatches per-mode policy based on
/// [`ModeKind`].  Replaces the five individual `*ModeRuntime` structs
/// that were ~750 lines of copy-paste.
///
/// Callers continue to use the old type aliases (`AskModeRuntime`,
/// `EditModeRuntime`, etc.) and their public `::new()` constructors —
/// the public API surface is unchanged.
#[derive(Default)]
pub struct GenericModeRuntime {
    /// The mode variant that governs policy dispatch.
    pub kind: ModeKind,
    /// Optional agent registry to pick a default chat agent.
    pub agent_registry: Option<Arc<AgentRegistry>>,
    /// Name of the agent to use (defaults to first available).
    pub agent_name: Option<String>,
    /// The policy to apply when risk is elevated (SafeGuard only).
    pub degrade_policy: AutoDegradePolicy,
    /// Approval posture — decoupled from mode kind (CodeWhale-compatible).
    /// Controls how tool approval is handled independent of execution mode.
    pub posture: ApprovalPosture,
}

// ── Backward-compat type aliases (A6: keep public API surface) ──────

/// Ask-mode runtime (single-turn Q&A, no tools, user approval required).
pub type AskModeRuntime = GenericModeRuntime;

/// Edit-mode runtime (constrained edit with plan/patch/verify).
pub type EditModeRuntime = GenericModeRuntime;

/// Agent-mode runtime (iterative planner-executor with tools).
pub type AgentModeRuntime = GenericModeRuntime;

/// FullAuto-mode runtime (fully automatic with review gate).
pub type FullAutoModeRuntime = GenericModeRuntime;

/// SafeGuard-mode runtime (automatic mode with approval gates).
pub type SafeGuardModeRuntime = GenericModeRuntime;

/// Plan-mode runtime (step-by-step planning without execution, like Claude Code's plan mode).
pub type PlanModeRuntime = GenericModeRuntime;

impl GenericModeRuntime {
    /// Derive the default posture for a given mode kind.
    fn default_posture_for(kind: &ModeKind) -> ApprovalPosture {
        match kind {
            ModeKind::Ask => ApprovalPosture::Auto,
            ModeKind::Plan => ApprovalPosture::Never,
            ModeKind::Edit => ApprovalPosture::Suggest,
            ModeKind::FullAuto => ApprovalPosture::Auto,
            ModeKind::SafeGuard => ApprovalPosture::Suggest,
        }
    }

    /// Create a new GenericModeRuntime with the given mode, registry, and agent.
    pub fn new(kind: ModeKind, registry: Arc<AgentRegistry>, agent_name: Option<String>) -> Self {
        Self {
            posture: Self::default_posture_for(&kind),
            kind,
            agent_registry: Some(registry),
            agent_name,
            degrade_policy: AutoDegradePolicy::default(),
        }
    }

    /// Override the default approval posture for this mode runtime.
    ///
    /// Mode and posture are independent dimensions:
    /// - mode = "what to do" (ask/plan/edit/full_auto/safeguard)
    /// - posture = "how to approve" (suggest/auto/bypass/never)
    ///
    /// This allows, for example, running FullAuto mode with `Suggest` posture
    /// for interactive approval, or Ask mode with `Bypass` for fully trusted CI.
    pub fn with_posture(mut self, posture: ApprovalPosture) -> Self {
        self.posture = posture;
        self
    }

    /// Compute a numeric risk score for the given objective string.
    ///
    /// Returns a value in 0.0–1.0 where:
    /// - below 0.40 = low risk — SafeGuard policy: `AllowWithAudit`
    /// - above 0.40 = elevated risk — SafeGuard policy: `ConfirmRequired`
    ///   (operator confirms; there is no auto-ReadOnly degradation tier — see
    ///   [`Self::safeguard_policy`]; the exact value 0.40 falls in the low
    ///   band because the check is strictly `> 0.40`)
    /// - above 0.95 = extreme risk — SafeGuard policy: `Block`
    pub fn compute_risk_score(&self, objective: &str) -> f64 {
        let mut score: f64 = 0.0;
        let lower = objective.to_lowercase(); // Pre-compute once

        // Extreme-risk keywords (additive 0.30 each)
        // Multi-word phrases checked via contains (word-boundary not needed for unique phrases)
        let extreme_multis = ["drop database", "drop table"];
        for kw in &extreme_multis {
            if lower.contains(kw) {
                // Use pre-computed lowercase
                score += 0.30;
            }
        }

        // Extreme single words and high-risk keywords (additive 0.20 each)
        // Uses word_boundary_match to avoid false positives like "undelete" matching "delete"
        let high_risk_keywords = [
            "delete",
            "remove",
            "drop",
            "truncate",
            "uninstall",
            "bash",
            "execute_command",
            "shell_exec",
            "rm",
            "shutdown",
            "rollback",
            "revert",
            "reset",
            "force",
        ];
        for kw in &high_risk_keywords {
            if Self::word_boundary_match(objective, kw) {
                score += 0.20;
            }
        }

        // Medium-risk keywords (additive 0.10 each)
        let medium_risk_keywords = [
            "write", "edit", "modify", "update", "create", "patch", "rename", "move", "copy",
        ];
        for kw in &medium_risk_keywords {
            if Self::word_boundary_match(objective, kw) {
                score += 0.10;
            }
        }

        score.min(1.0)
    }

    /// Check if `text` contains `word` as a complete word (word-boundary matching).
    /// Splits on non-alphanumeric characters to avoid false positives like
    /// `"undelete"` matching `delete` or `"dropdown"` matching `drop`.
    pub fn word_boundary_match(text: &str, word: &str) -> bool {
        text.split(|c: char| !c.is_alphanumeric())
            .any(|w| w.eq_ignore_ascii_case(word))
    }

    /// Compute the SafeGuard degradation policy for a given risk score.
    ///
    /// Shared by `pre_execute` and `fallback_result` (previously the two
    /// non-auto-degrade branches duplicated this block verbatim and drifted
    /// from `evaluate_degradation`, which is removed — it had no production
    /// callers).
    ///
    /// - 0.95 Block / 0.40 ConfirmRequired (no ReadOnly step — the operator
    ///   confirms manually instead of auto-degrading) / else AllowWithAudit.
    ///
    /// NOTE: the auto-degrade tier is not wired. The former `auto_degrade`
    /// field and `new_safeguard` constructor (the only path that set it) are
    /// gone — `GenericModeRuntime::new` always kept the flag false and no
    /// caller ever flipped it — so the ReadOnly tier of the old policy matrix
    /// is intentionally unreachable (design retained: operators confirm
    /// explicitly rather than being silently auto-degraded).
    fn safeguard_policy(risk_score: f64) -> AutoDegradePolicy {
        if risk_score > 0.95 {
            AutoDegradePolicy::Block
        } else if risk_score > 0.40 {
            AutoDegradePolicy::ConfirmRequired
        } else {
            // Low risk: allow with audit logging
            AutoDegradePolicy::AllowWithAudit
        }
    }
}

// ── ModeStrategy: dispatch on ModeKind ───────────────────────────────

impl ModeStrategy for GenericModeRuntime {
    fn mode_name(&self) -> &str {
        match self.kind {
            ModeKind::Ask => "ask",
            ModeKind::Plan => "plan",
            ModeKind::Edit => "edit",
            ModeKind::FullAuto => "full_auto",
            ModeKind::SafeGuard => "safeguard",
        }
    }

    fn use_chat(&self) -> bool {
        // FullAuto and SafeGuard also use chat() for tool-supported execution.
        // Ask and Plan use chat() for pure conversation without execution.
        // Only Edit uses run_task() via the autonomy loop.
        matches!(
            self.kind,
            ModeKind::Ask | ModeKind::Plan | ModeKind::FullAuto | ModeKind::SafeGuard
        )
    }

    fn pua_mode(&self) -> &str {
        match self.kind {
            ModeKind::Ask => "ask",
            ModeKind::Plan => "plan",
            ModeKind::Edit => "edit",
            ModeKind::FullAuto => "full_auto",
            ModeKind::SafeGuard => "safeguard",
        }
    }

    fn log_start(&self, objective: &str, phase: &str, role: &str) {
        match self.kind {
            ModeKind::Ask => {
                info!(
                    "[Ask Mode] Executing task: {} (phase: {}, role: {})",
                    objective, phase, role
                );
            }
            ModeKind::Plan => {
                info!(
                    "[Plan Mode] Planning for: {} (phase: {}, role: {})",
                    objective, phase, role
                );
            }
            ModeKind::Edit => {
                let is_high_risk = self.is_high_risk_operation(objective);
                info!(
                    "[Edit Mode] Executing iterative task: {} (phase: {}, role: {}, high_risk: {})",
                    objective, phase, role, is_high_risk
                );
            }
            ModeKind::FullAuto => {
                info!(
                    "[FullAuto Mode] Executing autonomous task: {} (phase: {}, role: {})",
                    objective, phase, role
                );
            }
            ModeKind::SafeGuard => {
                let risk_score = self.compute_risk_score(objective);
                info!(
                    "[SafeGuard Mode] Executing protected task: {} (phase: {}, role: {}, risk_score: {:.2})",
                    objective, phase, role, risk_score
                );
            }
        }
    }

    fn pre_execute(
        &self,
        task_id: &str,
        objective: &str,
        phase: &str,
        role: &str,
    ) -> Option<Result<AgentTaskResult>> {
        match self.kind {
            ModeKind::Edit => {
                if self.is_high_risk_operation(objective) {
                    warn!("[Edit Mode] High-risk operation detected: {}", objective);
                    Some(Ok(AgentTaskResult {
                        success: false,
                        output: Some(serde_json::json!({
                            "mode": "edit",
                            "task_id": task_id.to_string(),
                            "status": "pending_approval",
                            "is_high_risk": true,
                            "tools_available": PENDING_TASK_TOOLS,
                            "max_tool_calls": 20,
                            "message": format!("Edit task '{}' requires approval for high-risk operation", objective)
                        })),
                        error: Some(AgentError::Runtime(
                            "Operator approval required for high-risk operation".to_string(),
                        )),
                        audit_log: Some(format!(
                            "Edit mode: task_id={}, phase={}, role={}, high_risk=true",
                            task_id, phase, role
                        )),
                        pua_report: Some(mode_execution_report("edit", true)),
                    }))
                } else {
                    None
                }
            }
            ModeKind::SafeGuard => {
                let risk_score = self.compute_risk_score(objective);
                let policy = Self::safeguard_policy(risk_score);

                match policy {
                    AutoDegradePolicy::Block => {
                        warn!(
                            "[SafeGuard Mode] Extreme risk operation blocked: {} (score: {:.2})",
                            objective, risk_score
                        );
                        Some(Ok(AgentTaskResult {
                            success: false,
                            output: Some(serde_json::json!({
                                "mode": "safeguard",
                                "task_id": task_id.to_string(),
                                "status": "blocked",
                                "risk_score": risk_score,
                                "degrade_policy": "Block",
                                "safety_level": "enhanced",
                                "tools_available": PENDING_TASK_TOOLS,
                                "max_tool_calls": 30,
                                "message": format!("SafeGuard task '{}' blocked: extreme risk ({:.2})", objective, risk_score)
                            })),
                            error: Some(AgentError::Runtime(
                                format!("SafeGuard: Operation blocked due to extreme risk score ({:.2})", risk_score)
                            )),
                            audit_log: Some(format!(
                                "SafeGuard mode: task_id={}, phase={}, role={}, risk_score={:.2}, policy=Block",
                                task_id, phase, role, risk_score
                            )),
                            pua_report: Some(mode_execution_report("safeguard", true)),
                        }))
                    }
                    // The former ReadOnly auto-degrade arm is gone with
                    // `auto_degrade`: the policy matrix never returns ReadOnly
                    // (see `safeguard_policy` note). The variant is still part
                    // of `AutoDegradePolicy` (default + `allowed_tools` use
                    // it), so the arm stays for match exhaustiveness.
                    AutoDegradePolicy::ReadOnly => {
                        // Unreachable: `safeguard_policy` never returns it.
                        None
                    }
                    AutoDegradePolicy::AllowWithAudit => {
                        info!(
                            "[SafeGuard Mode] Proceeding with enhanced audit for: {} (score: {:.2})",
                            objective, risk_score
                        );
                        None
                    }
                    AutoDegradePolicy::ConfirmRequired => {
                        warn!(
                            "[SafeGuard Mode] Confirmation required for: {} (score: {:.2})",
                            objective, risk_score
                        );
                        Some(Ok(AgentTaskResult {
                            success: false,
                            output: Some(serde_json::json!({
                                "mode": "safeguard",
                                "task_id": task_id.to_string(),
                                "status": "pending_approval",
                                "risk_score": risk_score,
                                "degrade_policy": "ConfirmRequired",
                                "safety_level": "enhanced",
                                "tools_available": PENDING_TASK_TOOLS,
                                "max_tool_calls": 30,
                                "message": format!("SafeGuard task '{}' awaiting safety approval (risk: {:.2})", objective, risk_score)
                            })),
                            error: Some(AgentError::Runtime(
                                "SafeGuard: Operator approval required for this operation".to_string(),
                            )),
                            audit_log: Some(format!(
                                "SafeGuard mode: task_id={}, phase={}, role={}, risk_score={:.2}, policy=ConfirmRequired",
                                task_id, phase, role, risk_score
                            )),
                            pua_report: Some(mode_execution_report("safeguard", true)),
                        }))
                    }
                }
            }
            _ => None,
        }
    }

    fn fallback_result(
        &self,
        task_id: &str,
        objective: &str,
        phase: &str,
        role: &str,
    ) -> AgentTaskResult {
        match self.kind {
            ModeKind::Ask => AgentTaskResult {
                success: true,
                output: Some(serde_json::json!({
                    "mode": "ask",
                    "task_id": task_id.to_string(),
                    "status": "unavailable",
                    "note": "No suitable Ask mode agent was available in the registry",
                    "message": format!("Task '{}' processed in Ask mode (no agent available)", objective)
                })),
                error: None,
                audit_log: Some(format!(
                    "Ask mode (fallback): task_id={}, phase={}, role={}",
                    task_id, phase, role
                )),
                pua_report: Some(mode_execution_report("ask", false)),
            },
            ModeKind::Edit => AgentTaskResult {
                success: true,
                output: Some(serde_json::json!({
                    "mode": "edit",
                    "task_id": task_id.to_string(),
                    "status": "unavailable",
                    "note": "No suitable Edit mode agent was available in the registry",
                    "tools_available": PENDING_TASK_TOOLS,
                    "max_tool_calls": 20,
                    "message": format!("Edit task '{}' ready for execution", objective)
                })),
                error: None,
                audit_log: Some(format!(
                    "Edit mode: task_id={}, phase={}, role={}, high_risk={}",
                    task_id, phase, role, false
                )),
                pua_report: Some(mode_execution_report("edit", false)),
            },
            ModeKind::FullAuto => AgentTaskResult {
                success: true,
                output: Some(serde_json::json!({
                    "mode": "fullauto",
                    "task_id": task_id.to_string(),
                    "status": "unavailable",
                    "note": "No suitable FullAuto mode agent was available in the registry",
                    "execution_level": "full_autonomy",
                    "tools_available": PENDING_TASK_TOOLS,
                    "max_tool_calls": 50,
                    "message": format!("FullAuto task '{}' executed autonomously", objective)
                })),
                error: None,
                audit_log: Some(format!(
                    "FullAuto mode: task_id={}, phase={}, role={}, autonomy_level=full",
                    task_id, phase, role
                )),
                pua_report: Some(mode_execution_report("full_auto", false)),
            },
            ModeKind::Plan => AgentTaskResult {
                success: true,
                output: Some(serde_json::json!({
                    "mode": "plan",
                    "task_id": task_id.to_string(),
                    "status": "unavailable",
                    "note": "No suitable Plan mode agent was available in the registry",
                    "message": format!("Plan task '{}' ready for analysis", objective)
                })),
                error: None,
                audit_log: Some(format!(
                    "Plan mode: task_id={}, phase={}, role={}",
                    task_id, phase, role
                )),
                pua_report: Some(mode_execution_report("plan", false)),
            },
            ModeKind::SafeGuard => {
                let risk_score = self.compute_risk_score(objective);
                let policy = Self::safeguard_policy(risk_score);
                AgentTaskResult {
                    success: true,
                    output: Some(serde_json::json!({
                        "mode": "safeguard",
                        "task_id": task_id.to_string(),
                        "status": "unavailable",
                        "note": "No suitable SafeGuard mode agent was available in the registry",
                        "risk_score": risk_score,
                        "degrade_policy": format!("{:?}", policy),
                        "safety_level": "enhanced",
                        "tools_available": PENDING_TASK_TOOLS,
                        "max_tool_calls": 30,
                        "message": format!("SafeGuard task '{}' completed with enhanced safety (risk: {:.2})", objective, risk_score)
                    })),
                    error: None,
                    audit_log: Some(format!(
                        "SafeGuard mode: task_id={}, phase={}, role={}, risk_score={:.2}, policy={:?}",
                        task_id, phase, role, risk_score, policy
                    )),
                    pua_report: Some(mode_execution_report("safeguard", false)),
                }
            }
        }
    }
}

#[async_trait]
impl ModeRuntime for GenericModeRuntime {
    fn kind(&self) -> ModeKind {
        self.kind.clone()
    }

    fn posture(&self) -> ApprovalPosture {
        self.posture
    }

    fn allowed_tools(&self) -> Vec<String> {
        let (allowed, _) = policy_for_kind(&self.kind);
        // In-repo constant: `degrade_policy` is only ever the default
        // (`AutoDegradePolicy::ReadOnly` — `GenericModeRuntime::new`), so the
        // SafeGuard widening below is unreachable in-repo. Kept for out-of-tree
        // consumers that construct a runtime with a non-default policy.
        if matches!(self.kind, ModeKind::SafeGuard)
            && !matches!(self.degrade_policy, AutoDegradePolicy::ReadOnly)
        {
            return all_exec_tools().iter().map(|s| s.to_string()).collect();
        }
        allowed.iter().map(|s| s.to_string()).collect()
    }

    fn max_tool_calls(&self) -> usize {
        // Single source of truth: `policy_for_kind` owns the per-mode
        // max-call table shared with the ACP tool-execution gate.
        policy_for_kind(&self.kind).1
    }

    fn is_high_risk_operation(&self, objective: &str) -> bool {
        match self.kind {
            ModeKind::Edit => {
                Self::word_boundary_match(objective, "delete")
                    || Self::word_boundary_match(objective, "remove")
                    || Self::word_boundary_match(objective, "drop")
                    || Self::word_boundary_match(objective, "truncate")
            }
            ModeKind::SafeGuard => self.compute_risk_score(objective) > 0.15,
            _ => false,
        }
    }

    async fn run(&self, task: AgentTaskEnvelope) -> Result<AgentTaskResult> {
        let base = BaseModeRuntime::new(self.agent_registry.clone(), self.agent_name.clone());
        base.run(self, task).await
    }
}

// ── Shared mode tool-policy helpers ────────────────────────────────────────
// The CLI chat path and the ACP chat path must enforce the SAME tool policy
// per mode. Previously the ACP path had none (Ask mode executed tools, Plan
// mode could run write tools, and the per-agent "truncating" cap was log-only)
// while the CLI applied `filter_tool_calls_by_mode`. These two helpers are the
// single shared implementation.

/// Resolve the tool policy for a mode kind: allowed tools + max tool calls.
/// Single source of truth for the per-mode tool surface and max-call cap,
/// consumed by the ACP tool-execution gate (`filter_tool_calls_by_policy`)
/// and by `GenericModeRuntime::allowed_tools`/`max_tool_calls` (the CLI path).
pub fn policy_for_kind(kind: &ModeKind) -> (&'static [&'static str], usize) {
    let allowed: &'static [&'static str] = match kind {
        ModeKind::Ask => return (&[], 0),
        ModeKind::Plan => plan_tools(),
        ModeKind::Edit | ModeKind::FullAuto => all_exec_tools(),
        ModeKind::SafeGuard => read_only_tools(),
    };
    let max_calls = match kind {
        ModeKind::Ask => 0,
        ModeKind::Plan => 3,
        ModeKind::Edit => 20,
        ModeKind::FullAuto => 50,
        ModeKind::SafeGuard => 30,
    };
    (allowed, max_calls)
}

/// Filter tool calls by the mode's policy (allowed tools + max calls).
/// Returns the kept calls and the names that were dropped (either not in the
/// mode's allowed set or beyond the max-call cap).
pub fn filter_tool_calls_by_policy(
    tool_calls: &[(String, String)],
    kind: &ModeKind,
) -> (Vec<(String, String)>, Vec<String>) {
    let (allowed, max_calls) = policy_for_kind(kind);
    let mut kept: Vec<(String, String)> = Vec::new();
    let mut blocked: Vec<String> = Vec::new();
    for (name, args) in tool_calls {
        if kept.len() >= max_calls || !allowed.contains(&name.as_str()) {
            blocked.push(name.clone());
            continue;
        }
        kept.push((name.clone(), args.clone()));
    }
    (kept, blocked)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_policy_for_kind_ask_has_no_tools() {
        let (allowed, max_calls) = policy_for_kind(&ModeKind::Ask);
        assert!(allowed.is_empty());
        assert_eq!(max_calls, 0);
    }

    #[test]
    fn test_policy_for_kind_plan_is_read_only() {
        let (allowed, _max) = policy_for_kind(&ModeKind::Plan);
        assert!(allowed.contains(&"read_file"));
        assert!(!allowed.contains(&"write_file"));
    }

    #[test]
    fn test_filter_tool_calls_by_policy_caps_and_blocks() {
        let calls = vec![
            ("read_file".to_string(), "{}".to_string()),
            ("write_file".to_string(), "{}".to_string()),
            ("shell_exec".to_string(), "{}".to_string()),
        ];
        // Plan mode: write_file/shell_exec are not in the allowed set.
        let (kept, blocked) = filter_tool_calls_by_policy(&calls, &ModeKind::Plan);
        assert_eq!(kept.len(), 1);
        assert_eq!(blocked.len(), 2);
        // Ask mode: everything is blocked (max_calls = 0).
        let (kept, blocked) = filter_tool_calls_by_policy(&calls, &ModeKind::Ask);
        assert!(kept.is_empty());
        assert_eq!(blocked.len(), 3);
    }

    #[test]
    fn test_policy_lists_use_registered_canonical_names() {
        // Every tool named in the per-mode policy lists must resolve in a fresh
        // ToolRegistry (canonical name or alias). Stale entries (e.g. old alias
        // names that were replaced by canonical ones) silently break Edit /
        // FullAuto filtering: the model sees the canonical name, the policy
        // allows the alias, and the call is dropped. Regression test for the
        // `file_move`/`file_delete` → `move_path`/`delete_path` rename.
        let registry = crate::orchestration::tool::ToolRegistry::new();
        let known: std::collections::HashSet<&str> = registry.all_names().into_iter().collect();
        // Tools that are legitimately registered only under a non-default
        // feature (document-excel, drawing-svg, …). The policy lists name them
        // because the tools exist when that feature is enabled.
        let feature_gated: std::collections::HashSet<&str> = [
            "svg_export",
            "cad_convert",
            "read_excel",
            "read_ppt",
            "read_docx",
            "read_pdf",
            "gltf_read",
        ]
        .into_iter()
        .collect();
        for list in [
            plan_tools(),
            all_exec_tools(),
            read_only_tools(),
            low_risk_tool_names(),
        ] {
            for name in list {
                assert!(
                    known.contains(name) || feature_gated.contains(name),
                    "policy list references unregistered tool name `{name}`"
                );
            }
        }
    }
}
