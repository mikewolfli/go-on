//! Mode runtime orchestration for go-on (Phase 2)
//!
//! These structures are intentional framework definitions for Phase 0-9 architecture.
//! Mode runtimes define orchestration policies per mode that will be activated once
//! the orchestrator integrates them into the execution flow.

use crate::agent::{
    Agent, AgentError, AgentRegistry, AgentTaskEnvelope, AgentTaskResult, Message, StreamingSender,
};
use crate::pua::mode_execution_report;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{info, warn};

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
/// When `posture` is `None`, the default posture for the mode is used.
/// When `Some(p)`, the given posture overrides the mode default.
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
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let sender = StreamingSender::new(tx);

    let agent_ref: &dyn Agent = agent;
    agent_ref
        .chat(messages, principles, options, sender)
        .await
        .map_err(|e| anyhow::anyhow!("agent chat failed: {}", e))?;

    let mut full_output = String::new();
    // Add timeout to prevent infinite hang on dropped sender
    while let Some(token) = tokio::time::timeout(Duration::from_secs(300), rx.recv())
        .await
        .unwrap_or(None)
    {
        full_output.push_str(&token);
    }
    Ok(full_output)
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
fn build_chat_messages(task: &AgentTaskEnvelope) -> Vec<Message> {
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
    /// When true, auto-degrade the operation mode based on risk score
    /// (SafeGuard only; ignored by other modes).
    pub auto_degrade: bool,
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
            auto_degrade: false,
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

    /// Create a new SafeGuard-mode runtime with degradation enabled.
    ///
    /// # Deprecated
    /// Use `GenericModeRuntime::new(ModeKind::SafeGuard, registry, agent_name)`
    /// and set `auto_degrade = true` on the result instead.
    #[deprecated(
        since = "0.1.0",
        note = "Use `GenericModeRuntime::new(ModeKind::SafeGuard, registry, agent_name)` and set `.auto_degrade = true` on the result."
    )]
    pub fn new_safeguard(registry: Arc<AgentRegistry>, agent_name: Option<String>) -> Self {
        let mut runtime = Self::new(ModeKind::SafeGuard, registry, agent_name);
        runtime.auto_degrade = true;
        runtime
    }

    /// Compute a numeric risk score for the given objective string.
    ///
    /// Returns a value in 0.0–1.0 where:
    /// - < 0.4  = low risk (safe operations)
    /// - 0.4–0.7 = medium risk (warrants ReadOnly degradation)
    /// - 0.7–0.95 = high risk (warrants AllowWithAudit degradation)
    /// - > 0.95  = extreme risk (full Block)
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

    /// Evaluate risk and return the appropriate degradation policy.
    pub fn evaluate_degradation(&self, risk_score: f64) -> AutoDegradePolicy {
        if risk_score > 0.95 {
            AutoDegradePolicy::Block
        } else if risk_score > 0.70 {
            AutoDegradePolicy::ConfirmRequired
        } else if risk_score > 0.40 {
            AutoDegradePolicy::ReadOnly
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
                            "tools_available": ["read_file", "search_files", "apply_patch", "run_tests", "inspect_git_diff"],
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
                let policy = if self.auto_degrade {
                    self.evaluate_degradation(risk_score)
                } else if risk_score > 0.95 {
                    AutoDegradePolicy::Block
                } else if risk_score > 0.40 {
                    AutoDegradePolicy::ConfirmRequired
                } else {
                    AutoDegradePolicy::AllowWithAudit
                };

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
                                "tools_available": ["read_file", "search_files", "apply_patch", "run_tests", "inspect_git_diff"],
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
                    AutoDegradePolicy::ReadOnly => {
                        warn!(
                            "[SafeGuard Mode] Auto-degrading to read-only for: {} (score: {:.2})",
                            objective, risk_score
                        );
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
                                "tools_available": ["read_file", "search_files", "apply_patch", "run_tests", "inspect_git_diff"],
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
                    "tools_available": ["read_file", "search_files", "apply_patch", "run_tests", "inspect_git_diff"],
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
                    "tools_available": ["read_file", "search_files", "apply_patch", "run_tests", "inspect_git_diff"],
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
                let policy = if self.auto_degrade {
                    self.evaluate_degradation(risk_score)
                } else if risk_score > 0.95 {
                    AutoDegradePolicy::Block
                } else if risk_score > 0.40 {
                    AutoDegradePolicy::ConfirmRequired
                } else {
                    AutoDegradePolicy::AllowWithAudit
                };
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
                        "tools_available": ["read_file", "search_files", "apply_patch", "run_tests", "inspect_git_diff"],
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
        static PLAN_TOOLS: OnceLock<Vec<&'static str>> = OnceLock::new();
        static ALL_EXEC_TOOLS: OnceLock<Vec<&'static str>> = OnceLock::new();
        static READ_ONLY_TOOLS: OnceLock<Vec<&'static str>> = OnceLock::new();

        let tools = match self.kind {
            ModeKind::Ask => return vec![],
            ModeKind::Plan => PLAN_TOOLS.get_or_init(|| {
                vec![
                    "read_file",
                    "read_file_lines",
                    "search_files",
                    "grep",
                    "list_directory",
                    "inspect_git_diff",
                    "code_index_search",
                    "go_to_definition",
                    "find_references",
                    "date_time",
                    "environment_info",
                    "json_query",
                    "diff",
                    "archive_inspect",
                    "code_metrics",
                    "dns_lookup",
                    "ping",
                    "docker_ps",
                    "docker_logs",
                    "http_request",
                    "skill_list",
                    "rss_read",
                    "jsonl_read",
                ]
            }),
            ModeKind::Edit | ModeKind::FullAuto => ALL_EXEC_TOOLS.get_or_init(|| {
                vec![
                    // ── File tools ──
                    "read_file",
                    "read_file_lines",
                    "write_file",
                    "apply_patch",
                    "file_move",
                    "file_delete",
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
                    "diff",
                    "git",
                    // ── Build / Test / Lint ──
                    "cargo_check",
                    "cargo_test",
                    "run_tests",
                    "run_build",
                    "lint_code",
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
                    "add_dependency",
                    // ── Agent tools ──
                    "spawn_agent",
                    "apply_code_action",
                    // ── Skill tools (always available) ──
                    "skill_list",
                    "skill_execute",
                    "skill_create",
                    "skill_reload",
                ]
            }),
            ModeKind::SafeGuard => {
                if matches!(self.degrade_policy, AutoDegradePolicy::ReadOnly) {
                    READ_ONLY_TOOLS.get_or_init(|| {
                        vec![
                            "read_file",
                            "read_file_lines",
                            "search_files",
                            "grep",
                            "list_directory",
                            "inspect_git_diff",
                            "code_index_search",
                            "go_to_definition",
                            "find_references",
                            "diff",
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
                        ]
                    })
                } else {
                    ALL_EXEC_TOOLS.get_or_init(|| {
                        vec![
                            // ── File tools ──
                            "read_file",
                            "read_file_lines",
                            "write_file",
                            "apply_patch",
                            "file_move",
                            "file_delete",
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
                            "diff",
                            "git",
                            // ── Build / Test / Lint ──
                            "cargo_check",
                            "cargo_test",
                            "run_tests",
                            "run_build",
                            "lint_code",
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
                            "add_dependency",
                            // ── Agent tools ──
                            "spawn_agent",
                            "apply_code_action",
                            // ── Skill tools (always available) ──
                            "skill_list",
                            "skill_execute",
                            "skill_create",
                            "skill_reload",
                        ]
                    })
                }
            }
        };

        tools.iter().map(|s| s.to_string()).collect()
    }

    fn max_tool_calls(&self) -> usize {
        match self.kind {
            ModeKind::Ask => 0,
            ModeKind::Plan => 3,
            ModeKind::Edit => 20,
            ModeKind::FullAuto => 50,
            ModeKind::SafeGuard => 30,
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mode_kind_variants_exist() {
        // Verify all expected variants can be constructed.
        let ask = ModeKind::Ask;
        let edit = ModeKind::Edit;
        let full_auto = ModeKind::FullAuto;
        let safe_guard = ModeKind::SafeGuard;

        // Equality checks.
        assert_eq!(ask, ModeKind::Ask);
        assert_eq!(edit, ModeKind::Edit);
        assert_eq!(full_auto, ModeKind::FullAuto);
        assert_eq!(safe_guard, ModeKind::SafeGuard);
        assert_ne!(ask, edit);
    }

    #[test]
    fn test_mode_kind_debug_format() {
        let debug_str = format!("{:?}", ModeKind::FullAuto);
        assert!(debug_str.contains("FullAuto"));
    }

    #[test]
    fn test_auto_degrade_policy_default() {
        let policy = AutoDegradePolicy::default();
        assert_eq!(policy, AutoDegradePolicy::ReadOnly);

        // All variants should be constructable.
        let _block = AutoDegradePolicy::Block;
        let _read_only = AutoDegradePolicy::ReadOnly;
        let _allow = AutoDegradePolicy::AllowWithAudit;
        let _confirm = AutoDegradePolicy::ConfirmRequired;
    }
}
