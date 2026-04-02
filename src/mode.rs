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
    fn run(&self, _task: AgentTaskEnvelope) -> Result<AgentTaskResult> {
        Err(anyhow::anyhow!("Not implemented: wire to agent"))
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
    fn run(&self, _task: AgentTaskEnvelope) -> Result<AgentTaskResult> {
        Err(anyhow::anyhow!("Not implemented: wire to agent"))
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
    fn run(&self, _task: AgentTaskEnvelope) -> Result<AgentTaskResult> {
        Err(anyhow::anyhow!("Not implemented: wire to agent"))
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
    fn run(&self, _task: AgentTaskEnvelope) -> Result<AgentTaskResult> {
        Err(anyhow::anyhow!("Not implemented: wire to agent"))
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
    fn run(&self, _task: AgentTaskEnvelope) -> Result<AgentTaskResult> {
        Err(anyhow::anyhow!("Not implemented: wire to agent"))
    }
}
