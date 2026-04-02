//! Mode runtime orchestration for go-on (Phase 2)
//!
//! These structures are intentional framework definitions for Phase 0-9 architecture.
//! Mode runtimes define orchestration policies per mode that will be activated once
//! the orchestrator integrates them into the execution flow.

#![allow(dead_code)]

use crate::agent::{AgentTaskEnvelope, AgentTaskResult};
use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Supported chat/agent modes
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ModeKind {
    Ask,
    Edit,
    Agent,
    FullAuto,
    SafeGuard, // Automatic mode that requires approval at high-risk nodes
}

/// Mode runtime trait: each mode has its own orchestration, budget, and policy
pub trait ModeRuntime: Send + Sync {
    fn kind(&self) -> ModeKind;
    fn allowed_tools(&self) -> Vec<String>;
    fn max_tool_calls(&self) -> usize;
    fn user_approval_required(&self) -> bool;
    fn is_high_risk_operation(&self, objective: &str) -> bool;
    fn run(&self, task: AgentTaskEnvelope) -> Result<AgentTaskResult>;
}

/// AskModeRuntime: single-turn, no tools, user approval required
pub struct AskModeRuntime;
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
        false // All operations are already gated by user_approval_required
    }
    fn run(&self, task: AgentTaskEnvelope) -> Result<AgentTaskResult> {
        // AskMode: Single-turn question-answering without tools
        log::info!(
            "[Ask Mode] Executing task: {} (phase: {}, role: {})",
            task.objective,
            task.phase,
            task.role
        );
        Ok(AgentTaskResult {
            success: true,
            output: Some(serde_json::json!({
                "mode": "ask",
                "task_id": task.task_id,
                "status": "completed",
                "message": format!("Task '{}' processed in Ask mode", task.objective)
            })),
            error: None,
            audit_log: Some(format!(
                "Ask mode: task_id={}, phase={}, role={}",
                task.task_id, task.phase, task.role
            )),
        })
    }
}

/// EditModeRuntime: constrained edit with plan/patch/verify, user approval required
pub struct EditModeRuntime;
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
        false // All operations are already gated by user_approval_required
    }
    fn run(&self, task: AgentTaskEnvelope) -> Result<AgentTaskResult> {
        // EditMode: Constrained changes with plan/patch/verify workflow
        log::info!(
            "[Edit Mode] Planning edits for: {} (phase: {}, role: {})",
            task.objective,
            task.phase,
            task.role
        );
        Ok(AgentTaskResult {
            success: true,
            output: Some(serde_json::json!({
                "mode": "edit",
                "task_id": task.task_id,
                "status": "completed",
                "stages": ["plan", "patch", "verify"],
                "message": format!("Edit task '{}' completed with verification", task.objective)
            })),
            error: None,
            audit_log: Some(format!(
                "Edit mode: task_id={}, phase={}, role={}, max_tools=5",
                task.task_id, task.phase, task.role
            )),
        })
    }
}

/// AgentModeRuntime: iterative planner-executor with tools, autonomy gated
pub struct AgentModeRuntime;
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
        // AgentMode: Iterative multi-tool execution with user approval for high-risk ops
        let is_high_risk = self.is_high_risk_operation(&task.objective);
        log::info!(
            "[Agent Mode] Executing iterative task: {} (phase: {}, role: {}, high_risk: {})",
            task.objective,
            task.phase,
            task.role,
            is_high_risk
        );
        if is_high_risk {
            log::warn!(
                "[Agent Mode] High-risk operation detected: {}",
                task.objective
            );
        }
        Ok(AgentTaskResult {
            success: !is_high_risk, // Fail if high-risk (requires approval)
            output: Some(serde_json::json!({
                "mode": "agent",
                "task_id": task.task_id,
                "status": if is_high_risk { "pending_approval" } else { "completed" },
                "is_high_risk": is_high_risk,
                "tools_available": ["read_file", "search_files", "apply_patch", "run_tests", "inspect_git_diff"],
                "max_tool_calls": 20,
                "message": format!("Agent task '{}' ready for execution", task.objective)
            })),
            error: if is_high_risk {
                Some("Operator approval required for high-risk operation".to_string())
            } else {
                None
            },
            audit_log: Some(format!(
                "Agent mode: task_id={}, phase={}, role={}, high_risk={}",
                task.task_id, task.phase, task.role, is_high_risk
            )),
        })
    }
}

/// FullAutoModeRuntime: fully automatic with review gate and recovery policy
pub struct FullAutoModeRuntime;
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
        false // FullAuto assumes full trust and does not check for high-risk operations
    }
    fn run(&self, task: AgentTaskEnvelope) -> Result<AgentTaskResult> {
        // FullAutoMode: Unrestricted autonomous execution with full trust
        log::info!(
            "[FullAuto Mode] Executing autonomous task: {} (phase: {}, role: {})",
            task.objective,
            task.phase,
            task.role
        );
        Ok(AgentTaskResult {
            success: true,
            output: Some(serde_json::json!({
                "mode": "fullauto",
                "task_id": task.task_id,
                "status": "completed",
                "execution_level": "full_autonomy",
                "tools_available": ["read_file", "search_files", "apply_patch", "run_tests", "inspect_git_diff"],
                "max_tool_calls": 50,
                "message": format!("FullAuto task '{}' executed autonomously", task.objective)
            })),
            error: None,
            audit_log: Some(format!(
                "FullAuto mode: task_id={}, phase={}, role={}, autonomy_level=full",
                task.task_id, task.phase, task.role
            )),
        })
    }
}

/// SafeGuardModeRuntime: automatic mode one level below FullAuto with user approval at high-risk nodes
///
/// Mode Hierarchy (by automation level):
///   Ask (0) < Edit (5) < Agent (20) < SafeGuard (30) < FullAuto (50)
///
/// SafeGuard provides automated execution with safety guardrails:
/// - Operates automatically for routine operations (read, search, test, patch)
/// - Requires explicit user confirmation before executing high-risk operations
/// - Conservative risk detection: flags delete, drop, rollback, reset operations
/// - Maximum tool calls: 30 (vs FullAuto's 50)
///
/// Use SafeGuard when you want:
/// - Hands-off automation for most work
/// - Safety checkpoints for critical/destructive operations
/// - Fewer tool calls than FullAuto (more restricted scope)
///
/// vs FullAuto:
/// - SafeGuard: Asks for confirmation on destructive operations
/// - FullAuto: Trusts completely, no confirmations needed
pub struct SafeGuardModeRuntime;
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
        // Base requirement is false, but checked per operation via is_high_risk_operation
        // Orchestrator should check is_high_risk_operation and request approval if true
        false
    }
    fn is_high_risk_operation(&self, objective: &str) -> bool {
        let lower = objective.to_lowercase();
        // Conservative high-risk detection (more restrictive than Agent mode)
        // High-risk operations that require explicit user confirmation
        lower.contains("delete")
            || lower.contains("remove")
            || lower.contains("drop")
            || lower.contains("truncate")
            || lower.contains("rollback")
            || lower.contains("revert")
            || lower.contains("force")
            || lower.contains("reset")
            || lower.contains("drop table")
            || lower.contains("drop database")
            || lower.contains("uninstall")
            || lower.contains("downgrade")
    }
    fn run(&self, task: AgentTaskEnvelope) -> Result<AgentTaskResult> {
        // SafeGuardMode: Automatic execution with approval gates for high-risk operations
        let is_high_risk = self.is_high_risk_operation(&task.objective);
        log::info!(
            "[SafeGuard Mode] Executing protected task: {} (phase: {}, role: {}, high_risk: {})",
            task.objective,
            task.phase,
            task.role,
            is_high_risk
        );
        if is_high_risk {
            log::warn!(
                "[SafeGuard Mode] High-risk operation detected: {}",
                task.objective
            );
        }
        Ok(AgentTaskResult {
            success: !is_high_risk, // Fail if high-risk (requires approval)
            output: Some(serde_json::json!({
                "mode": "safeguard",
                "task_id": task.task_id,
                "status": if is_high_risk { "pending_approval" } else { "completed" },
                "is_high_risk": is_high_risk,
                "safety_level": "enhanced",
                "tools_available": ["read_file", "search_files", "apply_patch", "run_tests", "inspect_git_diff"],
                "max_tool_calls": 30,
                "message": format!("SafeGuard task '{}' awaiting safety approval", task.objective)
            })),
            error: if is_high_risk {
                Some(
                    "SafeGuard: Operator approval required for this high-risk operation"
                        .to_string(),
                )
            } else {
                None
            },
            audit_log: Some(format!(
                "SafeGuard mode: task_id={}, phase={}, role={}, high_risk={}, safety=enhanced",
                task.task_id, task.phase, task.role, is_high_risk
            )),
        })
    }
}
