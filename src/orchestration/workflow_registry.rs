//! S16: Workflow Registry
//!
//! Defines `WorkflowType` (auto / dev / general / free / custom) and a registry of
//! named workflow presets.  `WorkflowDetector` infers the type from the current
//! config and runtime context.
//!
//! NOTE: This is an intentional architecture framework (S16, Phase 0-9).
//! Kept as a stable extension point for future workflow preset management.

use crate::config::WorkflowType;
use crate::orchestration::roles::{role_registry_industry_for, AgentRole};
use crate::orchestration::startup_context::StartupContext;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
                phases: vec![
                    "planning".to_string(),
                    "coding".to_string(),
                    "review".to_string(),
                    "delivery".to_string(),
                ],
                description: "Full autopilot: Planner→Coder→Tester→Reviewer chain".to_string(),
            },
            WorkflowPreset {
                name: "dev".to_string(),
                workflow_type: WorkflowType::Dev,
                phases: vec![
                    "planning".to_string(),
                    "coding".to_string(),
                    "review".to_string(),
                    "delivery".to_string(),
                ],
                description: "Development workflow preset".to_string(),
            },
            WorkflowPreset {
                name: "general".to_string(),
                workflow_type: WorkflowType::General,
                phases: vec![
                    "gathering".to_string(),
                    "thinking".to_string(),
                    "executing".to_string(),
                    "validating".to_string(),
                    "closing".to_string(),
                ],
                description: "Cross-industry general workflow preset".to_string(),
            },
            WorkflowPreset {
                name: "free".to_string(),
                workflow_type: WorkflowType::Free,
                phases: Vec::new(),
                description: "Freestyle single-turn (mode=ask, no gating)".to_string(),
            },
        ];
        for p in defaults {
            self.presets.insert(p.name.clone(), p);
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::roles::AgentRole;

    #[test]
    fn test_new_registry_empty() {
        // WorkflowRegistry::new() registers defaults, but we can inspect the list.
        let registry = WorkflowRegistry::new();
        let presets = registry.list();
        // The default registry has 4 presets: autopilot, dev, general, free
        assert_eq!(presets.len(), 4);
        let names: Vec<&str> = presets.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"autopilot"));
        assert!(names.contains(&"dev"));
        assert!(names.contains(&"general"));
        assert!(names.contains(&"free"));
    }

    #[test]
    fn test_register_workflow() {
        let mut registry = WorkflowRegistry::new();
        let custom = WorkflowPreset {
            name: "my-custom".to_string(),
            workflow_type: WorkflowType::Custom,
            phases: vec!["alpha".to_string(), "beta".to_string()],
            description: "Custom preset".to_string(),
        };
        registry.register(custom);

        let found = registry.get("my-custom");
        assert!(found.is_some());
        let preset = found.unwrap();
        assert_eq!(preset.workflow_type, WorkflowType::Custom);
        assert_eq!(preset.phases, vec!["alpha", "beta"]);
        assert_eq!(preset.description, "Custom preset");
    }

    #[test]
    fn test_find_dev_workflow() {
        let registry = WorkflowRegistry::new();
        let dev = registry.get("dev");
        assert!(dev.is_some());
        let preset = dev.unwrap();
        assert_eq!(preset.workflow_type, WorkflowType::Dev);
        assert_eq!(preset.name, "dev");

        let general = registry.get("general");
        assert!(general.is_some());
        assert_eq!(general.unwrap().workflow_type, WorkflowType::General);
    }

    #[test]
    fn test_match_workflow_by_type() {
        // Test that WorkflowDetector::detect returns expected types
        // given explicit mode matches.
        let result = WorkflowDetector::detect(
            Some(&WorkflowType::Auto),
            Some("free"),
            None,
            None,
        );
        assert_eq!(result, WorkflowType::Free);

        let result = WorkflowDetector::detect(
            Some(&WorkflowType::Auto),
            Some("dev"),
            None,
            None,
        );
        assert_eq!(result, WorkflowType::Dev);

        let result = WorkflowDetector::detect(
            Some(&WorkflowType::Auto),
            Some("auto"),
            None,
            None,
        );
        assert_eq!(result, WorkflowType::Auto);
    }

    #[test]
    fn test_find_nonexistent_workflow() {
        let registry = WorkflowRegistry::new();
        let result = registry.get("nonexistent");
        assert!(result.is_none());

        let result = registry.get("");
        assert!(result.is_none());
    }

    #[test]
    fn test_workflow_detection_auto_mode() {
        // Auto mode, no explicit request mode, role is Coder → Dev
        let result = WorkflowDetector::detect(
            Some(&WorkflowType::Auto),
            None,
            Some(&AgentRole::Coder),
            None,
        );
        assert_eq!(result, WorkflowType::Dev);

        // Auto mode, no request mode, no role, with code repo → Dev
        let ctx = StartupContext {
            has_code_repo: true,
            ..Default::default()
        };
        let result = WorkflowDetector::detect(
            Some(&WorkflowType::Auto),
            None,
            None,
            Some(&ctx),
        );
        assert_eq!(result, WorkflowType::Dev);

        // Auto mode, no request mode, no role, no code repo → General
        let ctx = StartupContext {
            has_code_repo: false,
            ..Default::default()
        };
        let result = WorkflowDetector::detect(
            Some(&WorkflowType::Auto),
            None,
            None,
            Some(&ctx),
        );
        assert_eq!(result, WorkflowType::General);
    }

    #[test]
    fn test_workflow_detector_explicit_type() {
        // Explicit non-Auto config type should pass through
        let result = WorkflowDetector::detect(
            Some(&WorkflowType::General),
            None,
            None,
            None,
        );
        assert_eq!(result, WorkflowType::General);

        let result = WorkflowDetector::detect(
            Some(&WorkflowType::Free),
            None,
            None,
            None,
        );
        assert_eq!(result, WorkflowType::Free);

        let result = WorkflowDetector::detect(
            Some(&WorkflowType::Custom),
            None,
            None,
            None,
        );
        assert_eq!(result, WorkflowType::Custom);

        // Request mode overrides config type
        let result = WorkflowDetector::detect(
            Some(&WorkflowType::Custom),
            Some("auto"),
            None,
            None,
        );
        assert_eq!(result, WorkflowType::Auto);
    }

    #[test]
    fn test_requires_phase_gate() {
        assert!(WorkflowDetector::requires_phase_gate(&WorkflowType::Auto));
        assert!(WorkflowDetector::requires_phase_gate(&WorkflowType::Dev));
        assert!(WorkflowDetector::requires_phase_gate(&WorkflowType::General));
        assert!(WorkflowDetector::requires_phase_gate(&WorkflowType::Custom));
        assert!(!WorkflowDetector::requires_phase_gate(&WorkflowType::Free));
    }

    #[test]
    fn test_requires_review_gate() {
        assert!(WorkflowDetector::requires_review_gate(&WorkflowType::Auto));
        assert!(WorkflowDetector::requires_review_gate(&WorkflowType::Dev));
        assert!(!WorkflowDetector::requires_review_gate(&WorkflowType::General));
        assert!(!WorkflowDetector::requires_review_gate(&WorkflowType::Free));
        assert!(!WorkflowDetector::requires_review_gate(&WorkflowType::Custom));
    }
}
