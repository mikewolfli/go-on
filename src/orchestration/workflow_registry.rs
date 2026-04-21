//! S16: Workflow Registry
//!
//! Defines `WorkflowType` (auto / dev / general / free / custom) and a registry of
//! named workflow presets.  `WorkflowDetector` infers the type from the current
//! config and runtime context.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::config::WorkflowType;
use crate::orchestration::roles::{role_registry_industry_for, AgentRole};
use crate::orchestration::startup_context::StartupContext;

/// A named preset that maps to a WorkflowType and default phase list
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowPreset {
    pub name: String,
    pub workflow_type: WorkflowType,
    pub phases: Vec<String>,
    pub description: String,
}

/// In-memory registry of workflow presets
#[derive(Debug, Default)]
pub struct WorkflowRegistry {
    presets: HashMap<String, WorkflowPreset>,
}

impl WorkflowRegistry {
    pub fn new() -> Self {
        let mut r = Self::default();
        r.register_defaults();
        r
    }

    fn register_defaults(&mut self) {
        let defaults = vec![
            WorkflowPreset {
                name: "autopilot".to_string(),
                workflow_type: WorkflowType::Auto,
                phases: vec!["planning".to_string(), "coding".to_string(), "review".to_string(), "delivery".to_string()],
                description: "Full autopilot: Planner→Coder→Tester→Reviewer chain".to_string(),
            },
            WorkflowPreset {
                name: "dev".to_string(),
                workflow_type: WorkflowType::Dev,
                phases: vec!["planning".to_string(), "coding".to_string(), "review".to_string(), "delivery".to_string()],
                description: "Development workflow preset".to_string(),
            },
            WorkflowPreset {
                name: "general".to_string(),
                workflow_type: WorkflowType::General,
                phases: vec!["gathering".to_string(), "thinking".to_string(), "executing".to_string(), "validating".to_string(), "closing".to_string()],
                description: "Cross-industry general workflow preset".to_string(),
            },
            WorkflowPreset {
                name: "free".to_string(),
                workflow_type: WorkflowType::Free,
                phases: Vec::new(),
                description: "Freestyle single-turn (mode=ask, no gating)".to_string(),
            },
        ];
        for p in defaults { self.presets.insert(p.name.clone(), p); }
    }

    pub fn register(&mut self, preset: WorkflowPreset) {
        self.presets.insert(preset.name.clone(), preset);
    }

    pub fn get(&self, name: &str) -> Option<&WorkflowPreset> {
        self.presets.get(name)
    }

    pub fn list(&self) -> Vec<&WorkflowPreset> {
        let mut v: Vec<_> = self.presets.values().collect();
        v.sort_by_key(|p| p.name.as_str());
        v
    }
}

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
                "auto"         => return WorkflowType::Auto,
                "custom"       => return WorkflowType::Custom,
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

                if matches!(role, Some(AgentRole::Coder | AgentRole::Tester | AgentRole::Reviewer)) {
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
        matches!(wf, WorkflowType::Auto | WorkflowType::Dev | WorkflowType::General | WorkflowType::Custom)
    }

    /// True when review-gate is mandatory
    pub fn requires_review_gate(wf: &WorkflowType) -> bool {
        matches!(wf, WorkflowType::Auto | WorkflowType::Dev)
    }
}
