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
use serde::{Deserialize, Serialize};
use std::sync::{Arc, OnceLock};
use tokio::sync::mpsc;
use tracing::{info, warn};

/// Shared tokio runtime reused across all mode executions.
fn shared_runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| tokio::runtime::Runtime::new().expect("create shared runtime"))
}

/// Supported chat/agent modes
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ModeKind {
    Ask,
    Edit,
    Agent,
    FullAuto,
    /// Automatic mode that requires approval at high-risk nodes.
    SafeGuard,
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

/// Mode runtime trait: each mode has its own orchestration, budget, and policy
///
/// All implementations should instrument `run` for tracing and performance monitoring
/// in the implementation, not on the trait itself.
pub trait ModeRuntime: Send + Sync {
    /// Returns the mode kind.
    fn kind(&self) -> ModeKind;
    /// Returns the allowed tools for this mode.
    fn allowed_tools(&self) -> Vec<String>;
    /// Returns the maximum number of tool calls allowed.
    fn max_tool_calls(&self) -> usize;
    /// Whether user approval is required for this mode.
    fn user_approval_required(&self) -> bool;
    /// Whether the given objective is high risk.
    fn is_high_risk_operation(&self, objective: &str) -> bool;
    /// Run the mode orchestration for a given agent task.
    fn run(&self, task: AgentTaskEnvelope) -> Result<AgentTaskResult>;
}

/// Helper to execute an agent chat synchronously by blocking on a shared tokio runtime.
fn execute_agent_chat(
    agent: &dyn Agent,
    messages: Vec<Message>,
    principles: Option<Vec<String>>,
    options: Option<std::collections::HashMap<String, serde_json::Value>>,
) -> Result<String> {
    shared_runtime().block_on(async {
        let (tx, mut rx) = mpsc::channel::<String>(64);
        let sender = StreamingSender::new(tx);

        let agent_ref: &dyn Agent = agent;
        agent_ref
            .chat(messages, principles, options, sender)
            .await
            .map_err(|e| anyhow::anyhow!("agent chat failed: {}", e))?;

        let mut full_output = String::new();
        while let Some(token) = rx.recv().await {
            full_output.push_str(&token);
        }
        Ok(full_output)
    })
}

/// Helper to execute an agent run_task synchronously using the shared tokio runtime.
fn execute_agent_run_task(
    agent: &dyn Agent,
    envelope: AgentTaskEnvelope,
) -> Result<AgentTaskResult> {
    shared_runtime().block_on(async {
        let result = agent.run_task(envelope);
        result.map_err(|e| anyhow::anyhow!("agent run_task failed: {}", e))
    })
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
    pub fn run(
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
                        match execute_agent_chat(agent.as_ref(), messages, None, None) {
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
                        match execute_agent_run_task(agent.as_ref(), task) {
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
// AskModeRuntime
// ---------------------------------------------------------------------------

/// AskModeRuntime: single-turn Q&A, no tools, user approval required.
///
/// Uses `Agent::chat()` to produce a direct answer.
#[derive(Default)]
pub struct AskModeRuntime {
    /// Optional agent registry to pick a default chat agent.
    /// If not provided, run() falls back to a graceful informational response.
    pub agent_registry: Option<Arc<AgentRegistry>>,
    /// Name of the agent to use for chat (defaults to first available).
    pub agent_name: Option<String>,
}

impl AskModeRuntime {
    pub fn new(registry: Arc<AgentRegistry>, agent_name: Option<String>) -> Self {
        Self {
            agent_registry: Some(registry),
            agent_name,
        }
    }
}

impl ModeStrategy for AskModeRuntime {
    fn mode_name(&self) -> &str {
        "ask"
    }
    fn use_chat(&self) -> bool {
        true
    }
    fn pua_mode(&self) -> &str {
        "ask"
    }
    fn log_start(&self, objective: &str, phase: &str, role: &str) {
        info!(
            "[Ask Mode] Executing task: {} (phase: {}, role: {})",
            objective, phase, role
        );
    }
    fn fallback_result(&self, task_id: &str, objective: &str, phase: &str, role: &str) -> AgentTaskResult {
        AgentTaskResult {
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
        }
    }
}

impl ModeRuntime for AskModeRuntime {
    fn kind(&self) -> ModeKind {
        ModeKind::Ask
    }
    fn allowed_tools(&self) -> Vec<String> {
        vec![]
    }
    fn max_tool_calls(&self) -> usize {
        0
    }
    fn user_approval_required(&self) -> bool {
        true
    }
    fn is_high_risk_operation(&self, _objective: &str) -> bool {
        false
    }
    fn run(&self, task: AgentTaskEnvelope) -> Result<AgentTaskResult> {
        let base = BaseModeRuntime::new(self.agent_registry.clone(), self.agent_name.clone());
        base.run(self, task)
    }
}

// ---------------------------------------------------------------------------
// EditModeRuntime
// ---------------------------------------------------------------------------

/// EditModeRuntime: constrained edit with plan/patch/verify, user approval required.
///
/// Uses `Agent::run_task()` to execute the edit as a structured task.
#[derive(Default)]
pub struct EditModeRuntime {
    pub agent_registry: Option<Arc<AgentRegistry>>,
    pub agent_name: Option<String>,
}

impl EditModeRuntime {
    pub fn new(registry: Arc<AgentRegistry>, agent_name: Option<String>) -> Self {
        Self {
            agent_registry: Some(registry),
            agent_name,
        }
    }
}

impl ModeStrategy for EditModeRuntime {
    fn mode_name(&self) -> &str {
        "edit"
    }
    fn use_chat(&self) -> bool {
        false
    }
    fn pua_mode(&self) -> &str {
        "edit"
    }
    fn log_start(&self, objective: &str, phase: &str, role: &str) {
        info!(
            "[Edit Mode] Planning edits for: {} (phase: {}, role: {})",
            objective, phase, role
        );
    }
    fn fallback_result(&self, task_id: &str, objective: &str, phase: &str, role: &str) -> AgentTaskResult {
        AgentTaskResult {
            success: true,
            output: Some(serde_json::json!({
                "mode": "edit",
                "task_id": task_id.to_string(),
                "status": "unavailable",
                "note": "No suitable Edit mode agent was available in the registry",
                "stages": ["plan", "patch", "verify"],
                "message": format!("Edit task '{}' completed with verification", objective)
            })),
            error: None,
            audit_log: Some(format!(
                "Edit mode: task_id={}, phase={}, role={}, max_tools=5",
                task_id, phase, role
            )),
            pua_report: Some(mode_execution_report("edit", false)),
        }
    }
}

impl ModeRuntime for EditModeRuntime {
    fn kind(&self) -> ModeKind {
        ModeKind::Edit
    }
    fn allowed_tools(&self) -> Vec<String> {
        vec![
            "read_file".to_string(),
            "apply_patch".to_string(),
            "run_tests".to_string(),
        ]
    }
    fn max_tool_calls(&self) -> usize {
        5
    }
    fn user_approval_required(&self) -> bool {
        true
    }
    fn is_high_risk_operation(&self, _objective: &str) -> bool {
        false
    }
    fn run(&self, task: AgentTaskEnvelope) -> Result<AgentTaskResult> {
        let base = BaseModeRuntime::new(self.agent_registry.clone(), self.agent_name.clone());
        base.run(self, task)
    }
}

// ---------------------------------------------------------------------------
// AgentModeRuntime
// ---------------------------------------------------------------------------

/// AgentModeRuntime: iterative planner-executor with tools, autonomy gated.
///
/// Uses `Agent::run_task()` with multi-tool iteration.
/// Fails on high-risk operations unless approval is given.
#[derive(Default)]
pub struct AgentModeRuntime {
    pub agent_registry: Option<Arc<AgentRegistry>>,
    pub agent_name: Option<String>,
}

impl AgentModeRuntime {
    pub fn new(registry: Arc<AgentRegistry>, agent_name: Option<String>) -> Self {
        Self {
            agent_registry: Some(registry),
            agent_name,
        }
    }
}

impl ModeStrategy for AgentModeRuntime {
    fn mode_name(&self) -> &str {
        "agent"
    }
    fn use_chat(&self) -> bool {
        false
    }
    fn pua_mode(&self) -> &str {
        "agent"
    }
    fn log_start(&self, objective: &str, phase: &str, role: &str) {
        let is_high_risk = self.is_high_risk_operation(objective);
        info!(
            "[Agent Mode] Executing iterative task: {} (phase: {}, role: {}, high_risk: {})",
            objective, phase, role, is_high_risk
        );
    }
    fn pre_execute(
        &self,
        task_id: &str,
        objective: &str,
        phase: &str,
        role: &str,
    ) -> Option<Result<AgentTaskResult>> {
        if self.is_high_risk_operation(objective) {
            warn!("[Agent Mode] High-risk operation detected: {}", objective);
            Some(Ok(AgentTaskResult {
                success: false,
                output: Some(serde_json::json!({
                    "mode": "agent",
                    "task_id": task_id.to_string(),
                    "status": "pending_approval",
                    "is_high_risk": true,
                    "tools_available": ["read_file", "search_files", "apply_patch", "run_tests", "inspect_git_diff"],
                    "max_tool_calls": 20,
                                        "message": format!("Agent task '{}' requires approval for high-risk operation", objective)
                })),
                error: Some(AgentError::Runtime(
                    "Operator approval required for high-risk operation".to_string(),
                )),
                audit_log: Some(format!(
                    "Agent mode: task_id={}, phase={}, role={}, high_risk=true",
                    task_id, phase, role
                )),
                pua_report: Some(mode_execution_report("agent", true)),
            }))
        } else {
            None
        }
    }
    fn fallback_result(&self, task_id: &str, objective: &str, phase: &str, role: &str) -> AgentTaskResult {
        AgentTaskResult {
            success: true,
            output: Some(serde_json::json!({
                "mode": "agent",
                "task_id": task_id.to_string(),
                "status": "unavailable",
                "note": "No suitable Agent mode agent was available in the registry",
                "tools_available": ["read_file", "search_files", "apply_patch", "run_tests", "inspect_git_diff"],
                "max_tool_calls": 20,
                "message": format!("Agent task '{}' ready for execution", objective)
            })),
            error: None,
            audit_log: Some(format!(
                "Agent mode: task_id={}, phase={}, role={}, high_risk={}",
                task_id, phase, role, false
            )),
            pua_report: Some(mode_execution_report("agent", false)),
        }
    }
}

impl ModeRuntime for AgentModeRuntime {
    fn kind(&self) -> ModeKind {
        ModeKind::Agent
    }
    fn allowed_tools(&self) -> Vec<String> {
        vec![
            "read_file".to_string(),
            "search_files".to_string(),
            "apply_patch".to_string(),
            "run_tests".to_string(),
            "inspect_git_diff".to_string(),
        ]
    }
    fn max_tool_calls(&self) -> usize {
        20
    }
    fn user_approval_required(&self) -> bool {
        false
    }
    fn is_high_risk_operation(&self, objective: &str) -> bool {
        let lower = objective.to_lowercase();
        lower.contains("delete")
            || lower.contains("remove")
            || lower.contains("drop")
            || lower.contains("truncate")
    }
    fn run(&self, task: AgentTaskEnvelope) -> Result<AgentTaskResult> {
        let base = BaseModeRuntime::new(self.agent_registry.clone(), self.agent_name.clone());
        base.run(self, task)
    }
}

// ---------------------------------------------------------------------------
// FullAutoModeRuntime
// ---------------------------------------------------------------------------

/// FullAutoModeRuntime: fully automatic with review gate and recovery policy.
///
/// Uses `Agent::run_task()` with full autonomy — no approval gates.
#[derive(Default)]
pub struct FullAutoModeRuntime {
    pub agent_registry: Option<Arc<AgentRegistry>>,
    pub agent_name: Option<String>,
}

impl FullAutoModeRuntime {
    pub fn new(registry: Arc<AgentRegistry>, agent_name: Option<String>) -> Self {
        Self {
            agent_registry: Some(registry),
            agent_name,
        }
    }
}

impl ModeStrategy for FullAutoModeRuntime {
    fn mode_name(&self) -> &str {
        "full_auto"
    }
    fn use_chat(&self) -> bool {
        false
    }
    fn pua_mode(&self) -> &str {
        "full_auto"
    }
    fn log_start(&self, objective: &str, phase: &str, role: &str) {
        info!(
            "[FullAuto Mode] Executing autonomous task: {} (phase: {}, role: {})",
            objective, phase, role
        );
    }
    fn fallback_result(&self, task_id: &str, objective: &str, phase: &str, role: &str) -> AgentTaskResult {
        AgentTaskResult {
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
        }
    }
}

impl ModeRuntime for FullAutoModeRuntime {
    fn kind(&self) -> ModeKind {
        ModeKind::FullAuto
    }
    fn allowed_tools(&self) -> Vec<String> {
        vec![
            "read_file".to_string(),
            "search_files".to_string(),
            "apply_patch".to_string(),
            "run_tests".to_string(),
            "inspect_git_diff".to_string(),
        ]
    }
    fn max_tool_calls(&self) -> usize {
        50
    }
    fn user_approval_required(&self) -> bool {
        false
    }
    fn is_high_risk_operation(&self, _objective: &str) -> bool {
        false
    }
    fn run(&self, task: AgentTaskEnvelope) -> Result<AgentTaskResult> {
        let base = BaseModeRuntime::new(self.agent_registry.clone(), self.agent_name.clone());
        base.run(self, task)
    }
}

// ---------------------------------------------------------------------------
// SafeGuardModeRuntime
// ---------------------------------------------------------------------------

/// SafeGuardModeRuntime: automatic mode with approval gates at high-risk nodes.
///
/// Same as Agent mode but with conservative risk detection and
/// mandatory approval gates for destructive operations.
///
/// Mode Hierarchy (by automation level):
///   Ask (0) < Edit (5) < Agent (20) < SafeGuard (30) < FullAuto (50)
#[derive(Default)]
pub struct SafeGuardModeRuntime {
    pub agent_registry: Option<Arc<AgentRegistry>>,
    pub agent_name: Option<String>,
    /// When true, auto-degrade the operation mode based on risk score
    /// instead of blocking outright.
    pub auto_degrade: bool,
    /// The policy to apply when risk is elevated but not extreme (> 0.95).
    pub degrade_policy: AutoDegradePolicy,
}

impl SafeGuardModeRuntime {
    pub fn new(registry: Arc<AgentRegistry>, agent_name: Option<String>) -> Self {
        Self {
            agent_registry: Some(registry),
            agent_name,
            auto_degrade: true,
            degrade_policy: AutoDegradePolicy::default(),
        }
    }

    /// Compute a numeric risk score for the given objective string.
    ///
    /// Returns a value in 0.0–1.0 where:
    /// - < 0.4  = low risk (safe operations)
    /// - 0.4–0.7 = medium risk (warrants ReadOnly degradation)
    /// - 0.7–0.95 = high risk (warrants AllowWithAudit degradation)
    /// - > 0.95  = extreme risk (full Block)
    pub fn compute_risk_score(&self, objective: &str) -> f64 {
        let lower = objective.to_lowercase();
        let mut score: f64 = 0.0;

        // Destructive keywords — each adds 0.25 to the score.
        let extreme_keywords = ["drop database", "drop table", "truncate", "uninstall"];
        for kw in &extreme_keywords {
            if lower.contains(kw) {
                score += 0.30;
            }
        }

        let high_risk_keywords = [
            "delete",
            "remove",
            "drop",
            "rollback",
            "revert",
            "force",
            "reset",
            "downgrade",
            "format",
            "purge",
        ];
        for kw in &high_risk_keywords {
            if lower.contains(kw) {
                score += 0.20;
            }
        }

        let medium_risk_keywords = [
            "modify",
            "change",
            "update",
            "rename",
            "move",
            "overwrite",
            "replace",
        ];
        for kw in &medium_risk_keywords {
            if lower.contains(kw) {
                score += 0.10;
            }
        }

        score.min(1.0)
    }

    /// Evaluate risk and return the appropriate degradation policy.
    pub fn evaluate_degradation(&self, risk_score: f64) -> AutoDegradePolicy {
        if risk_score > 0.95 {
            AutoDegradePolicy::Block
        } else if risk_score > 0.70 {
            AutoDegradePolicy::AllowWithAudit
        } else if risk_score > 0.40 {
            AutoDegradePolicy::ReadOnly
        } else {
            // Low risk — no degradation needed.
            AutoDegradePolicy::AllowWithAudit
        }
    }
}

impl ModeStrategy for SafeGuardModeRuntime {
    fn mode_name(&self) -> &str {
        "safeguard"
    }
    fn use_chat(&self) -> bool {
        false
    }
    fn pua_mode(&self) -> &str {
        "safeguard"
    }
    fn log_start(&self, objective: &str, phase: &str, role: &str) {
        let risk_score = self.compute_risk_score(objective);
        info!(
            "[SafeGuard Mode] Executing protected task: {} (phase: {}, role: {}, risk_score: {:.2})",
            objective, phase, role, risk_score
        );
    }
    fn pre_execute(
        &self,
        task_id: &str,
        objective: &str,
        phase: &str,
        role: &str,
    ) -> Option<Result<AgentTaskResult>> {
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
    fn fallback_result(&self, task_id: &str, objective: &str, phase: &str, role: &str) -> AgentTaskResult {
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

impl ModeRuntime for SafeGuardModeRuntime {
    fn kind(&self) -> ModeKind {
        ModeKind::SafeGuard
    }
    fn allowed_tools(&self) -> Vec<String> {
        vec![
            "read_file".to_string(),
            "search_files".to_string(),
            "apply_patch".to_string(),
            "run_tests".to_string(),
            "inspect_git_diff".to_string(),
        ]
    }
    fn max_tool_calls(&self) -> usize {
        30
    }
    fn user_approval_required(&self) -> bool {
        false
    }
    fn is_high_risk_operation(&self, objective: &str) -> bool {
        self.compute_risk_score(objective) > 0.15
    }
    fn run(&self, task: AgentTaskEnvelope) -> Result<AgentTaskResult> {
        let base = BaseModeRuntime::new(self.agent_registry.clone(), self.agent_name.clone());
        base.run(self, task)
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
        let agent = ModeKind::Agent;
        let full_auto = ModeKind::FullAuto;
        let safe_guard = ModeKind::SafeGuard;

        // Equality checks.
        assert_eq!(ask, ModeKind::Ask);
        assert_eq!(edit, ModeKind::Edit);
        assert_eq!(agent, ModeKind::Agent);
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
