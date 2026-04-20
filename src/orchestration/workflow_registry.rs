//! S16: Workflow Registry
//!
//! Defines `WorkflowType` (auto / manual / free / custom) and a registry of
//! named workflow presets.  `WorkflowDetector` infers the type from the current
//! config and runtime context.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::config::WorkflowType;

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
                name: "manual".to_string(),
                workflow_type: WorkflowType::Manual,
                phases: vec!["coding".to_string()],
                description: "Human-driven single-phase execution".to_string(),
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
    /// Infer workflow type from config's `workflow_type` field and optional
    /// per-request override.
    pub fn detect(
        config_wf_type: Option<&WorkflowType>,
        request_mode: Option<&str>,
    ) -> WorkflowType {
        // Request-level override takes precedence
        if let Some(mode) = request_mode {
            match mode.to_ascii_lowercase().as_str() {
                "free" | "ask" => return WorkflowType::Free,
                "manual"       => return WorkflowType::Manual,
                "auto"         => return WorkflowType::Auto,
                "custom"       => return WorkflowType::Custom,
                _ => {}
            }
        }
        config_wf_type.cloned().unwrap_or_default()
    }

    /// True when the workflow type requires phase gating
    pub fn requires_phase_gate(wf: &WorkflowType) -> bool {
        matches!(wf, WorkflowType::Auto | WorkflowType::Custom)
    }

    /// True when review-gate is mandatory
    pub fn requires_review_gate(wf: &WorkflowType) -> bool {
        matches!(wf, WorkflowType::Auto)
    }
}
