//! F-GAP-06: Workflow Registry
//!
//! Defines `WorkflowType` (auto / dev / general / free / custom) and a registry of
//! named workflow presets.  `WorkflowDetector` infers the type from the current
//! config and runtime context.
//!
//! `WorkflowRegistry` provides full CRUD operations and intelligent conditional
//! matching against a `TaskContext`.
//!
//! NOTE: This is an intentional architecture framework (S16, Phase 0-9).
//! Kept as a stable extension point for future workflow preset management.

use crate::config::WorkflowType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub mod detector;
pub mod registry;

pub use detector::WorkflowDetector;

// ---------------------------------------------------------------------------
// Preset & TaskContext
// ---------------------------------------------------------------------------

/// A named preset that maps to a WorkflowType and default phase list
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowPreset {
    pub name: String,
    pub workflow_type: WorkflowType,
    pub phases: Vec<String>,
    pub description: String,
}

// ---------------------------------------------------------------------------
// WorkflowRegistry
// ---------------------------------------------------------------------------

/// In-memory registry of workflow presets
#[derive(Debug, Default)]
pub struct WorkflowRegistry {
    presets: HashMap<String, WorkflowPreset>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::roles::AgentRole;
    use crate::orchestration::startup_context::StartupContext;

    // -----------------------------------------------------------------------
    // WorkflowRegistry construction
    // -----------------------------------------------------------------------

    #[test]
    fn test_new_registry_empty() {
        let registry = WorkflowRegistry::new();
        let presets = registry.list();
        assert_eq!(presets.len(), 4);
        let names: Vec<&str> = presets.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"autopilot"));
        assert!(names.contains(&"dev"));
        assert!(names.contains(&"general"));
        assert!(names.contains(&"free"));
    }

    // -----------------------------------------------------------------------
    // 1. register – validation
    // -----------------------------------------------------------------------

    #[test]
    fn test_register_workflow() {
        let mut registry = WorkflowRegistry::new();
        let custom = WorkflowPreset {
            name: "my-custom".to_string(),
            workflow_type: WorkflowType::Custom,
            phases: vec!["alpha".to_string(), "beta".to_string()],
            description: "Custom preset".to_string(),
        };
        registry.register(custom).unwrap();

        let found = registry.find("my-custom");
        assert!(found.is_some());
        let preset = found.unwrap();
        assert_eq!(preset.workflow_type, WorkflowType::Custom);
        assert_eq!(preset.phases, vec!["alpha", "beta"]);
        assert_eq!(preset.description, "Custom preset");
    }

    #[test]
    fn test_register_rejects_empty_name() {
        let mut registry = WorkflowRegistry::new();
        let result = registry.register(WorkflowPreset {
            name: "   ".to_string(),
            workflow_type: WorkflowType::Free,
            phases: vec![],
            description: "no name".to_string(),
        });
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("empty"));
    }

    #[test]
    fn test_register_rejects_duplicate() {
        let mut registry = WorkflowRegistry::new();
        let result = registry.register(WorkflowPreset {
            name: "dev".to_string(),
            workflow_type: WorkflowType::Dev,
            phases: vec!["a".to_string()],
            description: "dupe".to_string(),
        });
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already exists"));
    }

    #[test]
    fn test_register_rejects_non_free_without_phases() {
        let mut registry = WorkflowRegistry::new();
        let result = registry.register(WorkflowPreset {
            name: "empty-dev".to_string(),
            workflow_type: WorkflowType::Dev,
            phases: vec![],
            description: "no phases".to_string(),
        });
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must have at least one phase"));
    }

    // -----------------------------------------------------------------------
    // 2. find
    // -----------------------------------------------------------------------

    #[test]
    fn test_find_dev_workflow() {
        let registry = WorkflowRegistry::new();
        let dev = registry.find("dev");
        assert!(dev.is_some());
        let preset = dev.unwrap();
        assert_eq!(preset.workflow_type, WorkflowType::Dev);
        assert_eq!(preset.name, "dev");

        let general = registry.find("general");
        assert!(general.is_some());
        assert_eq!(general.unwrap().workflow_type, WorkflowType::General);
    }

    #[test]
    fn test_find_nonexistent_workflow() {
        let registry = WorkflowRegistry::new();
        let result = registry.find("nonexistent");
        assert!(result.is_none());

        let result = registry.find("");
        assert!(result.is_none());
    }

    // -----------------------------------------------------------------------
    // 3. find_by_type
    // -----------------------------------------------------------------------

    #[test]
    fn test_find_by_type() {
        let registry = WorkflowRegistry::new();
        // Dev type: should find "dev"
        let dev_presets = registry.find_by_type(WorkflowType::Dev);
        assert_eq!(dev_presets.len(), 1);
        assert_eq!(dev_presets[0].name, "dev");

        // Free type: should find "free"
        let free_presets = registry.find_by_type(WorkflowType::Free);
        assert_eq!(free_presets.len(), 1);
        assert_eq!(free_presets[0].name, "free");

        // Custom type: none registered by default
        let custom_presets = registry.find_by_type(WorkflowType::Custom);
        assert!(custom_presets.is_empty());
    }

    #[test]
    fn test_find_by_type_returns_sorted() {
        let mut registry = WorkflowRegistry::new();
        registry
            .register(WorkflowPreset {
                name: "b-dev".to_string(),
                workflow_type: WorkflowType::Dev,
                phases: vec!["x".to_string()],
                description: "".to_string(),
            })
            .unwrap();
        registry
            .register(WorkflowPreset {
                name: "a-dev".to_string(),
                workflow_type: WorkflowType::Dev,
                phases: vec!["y".to_string()],
                description: "".to_string(),
            })
            .unwrap();

        let devs = registry.find_by_type(WorkflowType::Dev);
        assert_eq!(devs.len(), 3);
        // "a-dev", "b-dev", "dev" in alphabetical order
        assert_eq!(devs[0].name, "a-dev");
        assert_eq!(devs[1].name, "b-dev");
        assert_eq!(devs[2].name, "dev");
    }

    // -----------------------------------------------------------------------
    // 5. list
    // -----------------------------------------------------------------------

    #[test]
    fn test_list_returns_sorted_presets() {
        let registry = WorkflowRegistry::new();
        let names: Vec<&str> = registry.list().iter().map(|p| p.name.as_str()).collect();
        // Default: autopilot, dev, free, general
        assert_eq!(names, vec!["autopilot", "dev", "free", "general"]);
    }

    // -----------------------------------------------------------------------
    // 6. remove
    // -----------------------------------------------------------------------

    #[test]
    fn test_remove_existing() {
        let mut registry = WorkflowRegistry::new();
        assert!(registry.find("free").is_some());
        assert!(registry.remove("free"));
        assert!(registry.find("free").is_none());
    }

    #[test]
    fn test_remove_nonexistent() {
        let mut registry = WorkflowRegistry::new();
        assert!(!registry.remove("nonexistent"));
    }

    #[test]
    fn test_remove_preset() {
        let mut registry = WorkflowRegistry::new();
        assert!(registry.remove("free"));
        assert!(registry.find("free").is_none());
    }

    // -----------------------------------------------------------------------
    // 7. len / is_empty
    // -----------------------------------------------------------------------

    #[test]
    fn test_len() {
        let registry = WorkflowRegistry::new();
        assert_eq!(registry.len(), 4);
    }

    #[test]
    fn test_is_empty() {
        let registry = WorkflowRegistry::new();
        assert!(!registry.is_empty());

        let empty: WorkflowRegistry = WorkflowRegistry::default();
        assert!(empty.is_empty());
    }

    // -----------------------------------------------------------------------
    // WorkflowDetector (existing)
    // -----------------------------------------------------------------------

    #[test]
    fn test_match_workflow_by_type() {
        let result = WorkflowDetector::detect(Some(&WorkflowType::Auto), Some("free"), None, None);
        assert_eq!(result, WorkflowType::Free);

        let result = WorkflowDetector::detect(Some(&WorkflowType::Auto), Some("dev"), None, None);
        assert_eq!(result, WorkflowType::Dev);

        let result = WorkflowDetector::detect(Some(&WorkflowType::Auto), Some("auto"), None, None);
        assert_eq!(result, WorkflowType::Auto);
    }

    #[test]
    fn test_workflow_detection_auto_mode() {
        let result = WorkflowDetector::detect(
            Some(&WorkflowType::Auto),
            None,
            Some(&AgentRole::Coder),
            None,
        );
        assert_eq!(result, WorkflowType::Dev);

        let ctx = StartupContext {
            has_code_repo: true,
            ..Default::default()
        };
        let result = WorkflowDetector::detect(Some(&WorkflowType::Auto), None, None, Some(&ctx));
        assert_eq!(result, WorkflowType::Dev);

        let ctx = StartupContext {
            has_code_repo: false,
            ..Default::default()
        };
        let result = WorkflowDetector::detect(Some(&WorkflowType::Auto), None, None, Some(&ctx));
        assert_eq!(result, WorkflowType::General);
    }

    #[test]
    fn test_workflow_detector_explicit_type() {
        let result = WorkflowDetector::detect(Some(&WorkflowType::General), None, None, None);
        assert_eq!(result, WorkflowType::General);

        let result = WorkflowDetector::detect(Some(&WorkflowType::Free), None, None, None);
        assert_eq!(result, WorkflowType::Free);

        let result = WorkflowDetector::detect(Some(&WorkflowType::Custom), None, None, None);
        assert_eq!(result, WorkflowType::Custom);

        let result =
            WorkflowDetector::detect(Some(&WorkflowType::Custom), Some("auto"), None, None);
        assert_eq!(result, WorkflowType::Auto);
    }

    #[test]
    fn test_requires_phase_gate() {
        assert!(WorkflowDetector::requires_phase_gate(&WorkflowType::Auto));
        assert!(WorkflowDetector::requires_phase_gate(&WorkflowType::Dev));
        assert!(WorkflowDetector::requires_phase_gate(
            &WorkflowType::General
        ));
        assert!(WorkflowDetector::requires_phase_gate(&WorkflowType::Custom));
        assert!(!WorkflowDetector::requires_phase_gate(&WorkflowType::Free));
    }

    #[test]
    fn test_requires_review_gate() {
        assert!(WorkflowDetector::requires_review_gate(&WorkflowType::Auto));
        assert!(WorkflowDetector::requires_review_gate(&WorkflowType::Dev));
        assert!(!WorkflowDetector::requires_review_gate(
            &WorkflowType::General
        ));
        assert!(!WorkflowDetector::requires_review_gate(&WorkflowType::Free));
        assert!(!WorkflowDetector::requires_review_gate(
            &WorkflowType::Custom
        ));
    }
}
