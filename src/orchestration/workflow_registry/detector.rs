//! Workflow detection — infers workflow type from config and runtime context
//!
//! `WorkflowDetector` infers the type from the current config and runtime
//! context. Also provides gate-check helpers for phase and review gates.

use crate::config::WorkflowType;
use crate::orchestration::roles::{role_registry_industry_for, AgentRole};
use crate::orchestration::startup_context::StartupContext;

/// Detects workflow type from config + request metadata
pub struct WorkflowDetector;

impl WorkflowDetector {
    /// Infer workflow type from config and optional runtime context.
    pub fn detect(
        config_wf_type: Option<&WorkflowType>,
        request_mode: Option<&str>,
        role: Option<&AgentRole>,
        startup_ctx: Option<&StartupContext>,
    ) -> WorkflowType {
        if let Some(mode) = request_mode {
            match mode.to_ascii_lowercase().as_str() {
                "free" | "ask" => return WorkflowType::Free,
                "dev" | "manual" => return WorkflowType::Dev,
                "general" => return WorkflowType::General,
                "auto" => return WorkflowType::Auto,
                "custom" => return WorkflowType::Custom,
                _ => {}
            }
        }

        match config_wf_type.cloned().unwrap_or_default() {
            WorkflowType::Auto => {
                if let Some(AgentRole::Custom(name)) = role {
                    if role_registry_industry_for(name)
                        .map(|industry| !industry.eq_ignore_ascii_case("dev"))
                        .unwrap_or(false)
                    {
                        return WorkflowType::General;
                    }
                }

                if matches!(
                    role,
                    Some(AgentRole::Coder | AgentRole::Tester | AgentRole::Reviewer)
                ) {
                    return WorkflowType::Dev;
                }

                if let Some(ctx) = startup_ctx {
                    if ctx.has_code_repo {
                        WorkflowType::Dev
                    } else {
                        WorkflowType::General
                    }
                } else {
                    WorkflowType::Dev
                }
            }
            other => other,
        }
    }

    /// True when the workflow type requires phase gating
    pub fn requires_phase_gate(wf: &WorkflowType) -> bool {
        matches!(
            wf,
            WorkflowType::Auto | WorkflowType::Dev | WorkflowType::General | WorkflowType::Custom
        )
    }

    /// True when review-gate is mandatory
    pub fn requires_review_gate(wf: &WorkflowType) -> bool {
        matches!(wf, WorkflowType::Auto | WorkflowType::Dev)
    }
}
