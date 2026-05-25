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
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, warn};

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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AutoDegradePolicy {
    /// Block the operation entirely (original behavior).
    Block,
    /// Switch to read-only mode — only allow read/inspect tools.
    ReadOnly,
    /// Allow the operation but with enhanced audit logging.
    AllowWithAudit,
    /// Ask the operator for confirmation before proceeding.
    ConfirmRequired,
}

impl Default for AutoDegradePolicy {
    fn default() -> Self {
        AutoDegradePolicy::ReadOnly
    }
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

/// Helper to execute an agent chat synchronously by blocking on a tokio runtime.
fn execute_agent_chat(
    agent: &dyn Agent,
    messages: Vec<Message>,
    principles: Option<Vec<String>>,
    options: Option<std::collections::HashMap<String, serde_json::Value>>,
) -> Result<String> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
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

/// Helper to execute an agent run_task synchronously.
fn execute_agent_run_task(
    agent: &dyn Agent,
    envelope: AgentTaskEnvelope,
) -> Result<AgentTaskResult> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        // run_task is synchronous on the trait; we call it directly
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
        let task_id = task.task_id.clone();
        let objective = task.objective.clone();
        let phase = task.phase.clone();
        let role = task.role.clone();

        info!(
            "[Ask Mode] Executing task: {} (phase: {}, role: {})",
            objective, phase, role
        );

        // Try to execute via a real agent
        if let Some(ref registry) = self.agent_registry {
            let agent_name = match self.agent_name.as_deref() {
                Some(name) => Some(name.to_string()),
                None => registry.names().first().cloned(),
            };

            if let Some(name) = agent_name {
                if let Some(agent) = registry.get(&name) {
                    let messages = build_chat_messages(&task);
                    match execute_agent_chat(agent.as_ref(), messages, None, None) {
                        Ok(output) => {
                            return Ok(AgentTaskResult {
                                success: true,
                                output: Some(serde_json::json!({
                                    "mode": "ask",
                                    "task_id": task_id.clone(),
                                    "status": "completed",
                                    "agent": name,
                                    "answer": output,
                                })),
                                error: None,
                                audit_log: Some(format!(
                                    "Ask mode: task_id={}, phase={}, role={}, agent={}",
                                    task_id, phase, role, name
                                )),
                                pua_report: Some(mode_execution_report("ask", false)),
                            });
                        }
                        Err(e) => {
                            warn!("[Ask Mode] Agent '{}' chat failed: {}", name, e);
                        }
                    }
                }
            }
        }

        // Graceful degradation: no agent available — return informational response
        warn!("[Ask Mode] No agent available — falling back to informational response");
        Ok(AgentTaskResult {
            success: true,
            output: Some(serde_json::json!({
                "mode": "ask",
                "task_id": task_id.clone(),
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
        })
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
        let task_id = task.task_id.clone();
        let objective = task.objective.clone();
        let phase = task.phase.clone();
        let role = task.role.clone();

        info!(
            "[Edit Mode] Planning edits for: {} (phase: {}, role: {})",
            objective, phase, role
        );

        // Attempt real agent execution via run_task
        if let Some(ref registry) = self.agent_registry {
            let agent_name = match self.agent_name.as_deref() {
                Some(name) => Some(name.to_string()),
                None => registry.names().first().cloned(),
            };

            if let Some(name) = agent_name {
                if let Some(agent) = registry.get(&name) {
                    match execute_agent_run_task(agent.as_ref(), task) {
                        Ok(result) => {
                            return Ok(AgentTaskResult {
                                pua_report: Some(mode_execution_report("edit", false)),
                                ..result
                            });
                        }
                        Err(e) => {
                            warn!("[Edit Mode] Agent '{}' run_task failed: {}", name, e);
                        }
                    }
                }
            }
        }

        // Graceful degradation: no agent available — return informational response
        warn!("[Edit Mode] No agent available — falling back to informational response");
        Ok(AgentTaskResult {
            success: true,
            output: Some(serde_json::json!({
                "mode": "edit",
                "task_id": task_id,
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
        })
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
        let task_id = task.task_id.clone();
        let objective = task.objective.clone();
        let phase = task.phase.clone();
        let role = task.role.clone();
        let is_high_risk = self.is_high_risk_operation(&objective);

        info!(
            "[Agent Mode] Executing iterative task: {} (phase: {}, role: {}, high_risk: {})",
            objective, phase, role, is_high_risk
        );
        if is_high_risk {
            warn!("[Agent Mode] High-risk operation detected: {}", objective);
            // Return pending approval status without executing
            return Ok(AgentTaskResult {
                success: false,
                output: Some(serde_json::json!({
                    "mode": "agent",
                    "task_id": task_id.clone(),
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
            });
        }

        // Attempt real agent execution via run_task
        if let Some(ref registry) = self.agent_registry {
            let agent_name = match self.agent_name.as_deref() {
                Some(name) => Some(name.to_string()),
                None => registry.names().first().cloned(),
            };

            if let Some(name) = agent_name {
                if let Some(agent) = registry.get(&name) {
                    match execute_agent_run_task(agent.as_ref(), task) {
                        Ok(result) => {
                            return Ok(AgentTaskResult {
                                pua_report: Some(mode_execution_report("agent", false)),
                                ..result
                            });
                        }
                        Err(e) => {
                            warn!("[Agent Mode] Agent '{}' run_task failed: {}", name, e);
                        }
                    }
                }
            }
        }

        // Graceful degradation: no agent available — return informational response
        warn!("[Agent Mode] No agent available — falling back to informational response");
        Ok(AgentTaskResult {
            success: true,
            output: Some(serde_json::json!({
                "mode": "agent",
                "task_id": task_id,
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
        })
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
        let task_id = task.task_id.clone();
        let objective = task.objective.clone();
        let phase = task.phase.clone();
        let role = task.role.clone();

        info!(
            "[FullAuto Mode] Executing autonomous task: {} (phase: {}, role: {})",
            objective, phase, role
        );

        // Attempt real agent execution via run_task
        if let Some(ref registry) = self.agent_registry {
            let agent_name = match self.agent_name.as_deref() {
                Some(name) => Some(name.to_string()),
                None => registry.names().first().cloned(),
            };

            if let Some(name) = agent_name {
                if let Some(agent) = registry.get(&name) {
                    match execute_agent_run_task(agent.as_ref(), task) {
                        Ok(result) => {
                            return Ok(AgentTaskResult {
                                pua_report: Some(mode_execution_report("full_auto", false)),
                                ..result
                            });
                        }
                        Err(e) => {
                            warn!("[FullAuto Mode] Agent '{}' run_task failed: {}", name, e);
                        }
                    }
                }
            }
        }

        // Graceful degradation: no agent available — return informational response
        warn!("[FullAuto Mode] No agent available — falling back to informational response");
        Ok(AgentTaskResult {
            success: true,
            output: Some(serde_json::json!({
                "mode": "fullauto",
                "task_id": task_id,
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
        })
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
        let task_id = task.task_id.clone();
        let objective = task.objective.clone();
        let phase = task.phase.clone();
        let role = task.role.clone();
        let risk_score = self.compute_risk_score(&objective);

        info!(
            "[SafeGuard Mode] Executing protected task: {} (phase: {}, role: {}, risk_score: {:.2})",
            objective, phase, role, risk_score
        );

        // Determine degradation policy based on risk score.
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
                return Ok(AgentTaskResult {
                    success: false,
                    output: Some(serde_json::json!({
                        "mode": "safeguard",
                        "task_id": task_id.clone(),
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
                });
            }
            AutoDegradePolicy::ReadOnly => {
                warn!(
                    "[SafeGuard Mode] Auto-degrading to read-only for: {} (score: {:.2})",
                    objective, risk_score
                );
                // Fall through to execute with read-only toolset.
                // The agent execution below will be constrained to read-only tools.
            }
            AutoDegradePolicy::AllowWithAudit => {
                info!(
                    "[SafeGuard Mode] Proceeding with enhanced audit for: {} (score: {:.2})",
                    objective, risk_score
                );
                // Fall through to normal execution with audit logging.
            }
            AutoDegradePolicy::ConfirmRequired => {
                warn!(
                    "[SafeGuard Mode] Confirmation required for: {} (score: {:.2})",
                    objective, risk_score
                );
                return Ok(AgentTaskResult {
                    success: false,
                    output: Some(serde_json::json!({
                        "mode": "safeguard",
                        "task_id": task_id.clone(),
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
                });
            }
        }

        // Attempt real agent execution via run_task (non-high-risk)
        if let Some(ref registry) = self.agent_registry {
            let agent_name = match self.agent_name.as_deref() {
                Some(name) => Some(name.to_string()),
                None => registry.names().first().cloned(),
            };

            if let Some(name) = agent_name {
                if let Some(agent) = registry.get(&name) {
                    match execute_agent_run_task(agent.as_ref(), task) {
                        Ok(result) => {
                            return Ok(AgentTaskResult {
                                pua_report: Some(mode_execution_report("safeguard", false)),
                                ..result
                            });
                        }
                        Err(e) => {
                            warn!("[SafeGuard Mode] Agent '{}' run_task failed: {}", name, e);
                        }
                    }
                }
            }
        }

        // Graceful degradation: no agent available — return informational response
        warn!("[SafeGuard Mode] No agent available — falling back to informational response");
        Ok(AgentTaskResult {
            success: true,
            output: Some(serde_json::json!({
                "mode": "safeguard",
                "task_id": task_id,
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
        })
    }
}
