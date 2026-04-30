//! S16: Workflow Registry
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
use crate::orchestration::roles::{role_registry_industry_for, AgentRole};
use crate::orchestration::startup_context::StartupContext;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

/// Context describing the task to be matched against a workflow preset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskContext {
    /// High-level task category (e.g. "bug_fix", "feature", "refactor", "q&a")
    pub task_type: String,
    /// A numeric estimate of task complexity (0.0 – 1.0).  Higher = more complex.
    pub complexity_score: f64,
    /// Roles the task requires (e.g. ["Coder", "Reviewer"])
    pub roles_needed: Vec<String>,
}

impl Default for TaskContext {
    fn default() -> Self {
        Self {
            task_type: String::new(),
            complexity_score: 0.0,
            roles_needed: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// WorkflowRegistry
// ---------------------------------------------------------------------------

/// In-memory registry of workflow presets
#[derive(Debug, Default)]
pub struct WorkflowRegistry {
    presets: HashMap<String, WorkflowPreset>,
    /// Optional profile tracking registry activity.
    profile: Option<WorkflowRegistryProfile>,
}

/// Profile that tracks registry activity and state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowRegistryProfile {
    /// Whether the registry is enabled for matching.
    pub enabled: bool,
    /// Number of registered presets.
    pub preset_count: usize,
    /// Timestamp (unix epoch seconds) of the last successful match, or 0.
    pub last_match: i64,
    /// Total number of successful match calls.
    pub match_count: u64,
}

impl Default for WorkflowRegistryProfile {
    fn default() -> Self {
        Self {
            enabled: true,
            preset_count: 0,
            last_match: 0,
            match_count: 0,
        }
    }
}

impl WorkflowRegistry {
    /// Create a new registry with the built-in defaults.
    pub fn new() -> Self {
        let mut r = Self::default();
        r.register_defaults();
        r
    }

    /// Create a new registry with profile tracking enabled.
    pub fn new_with_profile() -> Self {
        let mut r = Self {
            presets: HashMap::new(),
            profile: Some(WorkflowRegistryProfile::default()),
        };
        r.register_defaults();
        if let Some(ref mut p) = r.profile {
            p.preset_count = r.presets.len();
        }
        r
    }

    /// Return a shared reference to the profile, if one exists.
    pub fn profile(&self) -> Option<&WorkflowRegistryProfile> {
        self.profile.as_ref()
    }

    /// Return a mutable reference to the profile, if one exists.
    pub fn profile_mut(&mut self) -> Option<&mut WorkflowRegistryProfile> {
        self.profile.as_mut()
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
                description: "Full autopilot: Planner->Coder->Tester->Reviewer chain".to_string(),
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

    // -----------------------------------------------------------------------
    // 1. register – validate and insert (checks for duplicates)
    // -----------------------------------------------------------------------

    /// Register a new preset.  Returns `Err` with a message when validation
    /// fails or a preset with the same name already exists.
    pub fn register(&mut self, preset: WorkflowPreset) -> Result<(), String> {
        let name = preset.name.trim().to_string();
        if name.is_empty() {
            return Err("Preset name cannot be empty".to_string());
        }
        if self.presets.contains_key(&name) {
            return Err(format!("Preset '{}' already exists", name));
        }
        // WorkflowType::Free presets with no phases are acceptable;
        // other types should have at least one phase.
        if !matches!(preset.workflow_type, WorkflowType::Free) && preset.phases.is_empty() {
            return Err(format!(
                "Preset '{}' of type {:?} must have at least one phase",
                name, preset.workflow_type
            ));
        }
        // Keep the trimmed name in the stored preset.
        let mut preset = preset;
        preset.name = name.clone();
        self.presets.insert(name, preset);
        if let Some(ref mut p) = self.profile {
            p.preset_count = self.presets.len();
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // 2. find – lookup by name
    // -----------------------------------------------------------------------

    /// Look up a preset by name.
    pub fn find(&self, name: &str) -> Option<&WorkflowPreset> {
        self.presets.get(name)
    }

    // -----------------------------------------------------------------------
    // 3. find_by_type – filter by workflow type
    // -----------------------------------------------------------------------

    /// Return all presets matching the given `WorkflowType`, sorted by name.
    pub fn find_by_type(&self, workflow_type: WorkflowType) -> Vec<&WorkflowPreset> {
        let mut result: Vec<_> = self
            .presets
            .values()
            .filter(|p| p.workflow_type == workflow_type)
            .collect();
        result.sort_by_key(|p| p.name.as_str());
        result
    }

    // -----------------------------------------------------------------------
    // 4. match_workflow – intelligent conditional matching
    // -----------------------------------------------------------------------

    /// Select the best workflow preset for the given `TaskContext`.
    ///
    /// Matching rules (first match wins):
    ///
    /// | Condition                                          | Selected Preset |
    /// |----------------------------------------------------|-----------------|
    /// | `task_type` contains "bug" / "fix" / "debug"       | dev             |
    /// | `complexity_score` >= 0.7                          | autopilot       |
    /// | `complexity_score` <= 0.2 AND `task_type` is "q&a" | free            |
    /// | `roles_needed` contains "Coder" or "Reviewer"      | dev             |
    /// | `roles_needed` is empty (Q&A / single-turn)        | free            |
    /// | otherwise                                          | general         |
    pub fn match_workflow(&mut self, task_context: &TaskContext) -> Option<&WorkflowPreset> {
        if let Some(ref p) = self.profile {
            if !p.enabled {
                return None;
            }
        }

        let task_lower = task_context.task_type.to_ascii_lowercase();
        let score = task_context.complexity_score;

        // Helper: check whether any of the given substrings appear in the
        // lowercased task_type.
        let task_type_contains =
            |keywords: &[&str]| -> bool { keywords.iter().any(|kw| task_lower.contains(kw)) };

        let selected_name: &str = {
            // Bug-fix / debug tasks -> dev
            if task_type_contains(&["bug", "fix", "debug"]) {
                "dev"
            }
            // Complex tasks (score >= 0.7) -> autopilot
            else if score >= 0.7 {
                "autopilot"
            }
            // Simple Q&A (score <= 0.2) -> free
            else if score <= 0.2 && task_type_contains(&["q&a", "ask", "question"]) {
                "free"
            }
            // Roles matching: if the task requires coding or review roles -> dev
            else if task_context
                .roles_needed
                .iter()
                .any(|r| r.eq_ignore_ascii_case("Coder") || r.eq_ignore_ascii_case("Reviewer"))
            {
                "dev"
            }
            // No roles specified, or only passive roles -> free (single-turn / Q&A)
            else if task_context.roles_needed.is_empty() {
                "free"
            }
            // Fallback -> general
            else {
                "general"
            }
        };

        let preset = self.presets.get(selected_name)?;

        // Update profile statistics
        if let Some(ref mut p) = self.profile {
            p.last_match = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            p.match_count = p.match_count.wrapping_add(1);
            p.preset_count = self.presets.len();
        }

        Some(preset)
    }

    // -----------------------------------------------------------------------
    // 5. list – all presets sorted by name
    // -----------------------------------------------------------------------

    /// Return references to all registered presets, sorted alphabetically.
    pub fn list(&self) -> Vec<&WorkflowPreset> {
        let mut v: Vec<_> = self.presets.values().collect();
        v.sort_by_key(|p| p.name.as_str());
        v
    }

    // -----------------------------------------------------------------------
    // 6. remove – remove a preset by name
    // -----------------------------------------------------------------------

    /// Remove a preset by name.  Returns `true` if a preset was actually removed.
    pub fn remove(&mut self, name: &str) -> bool {
        let removed = self.presets.remove(name).is_some();
        if removed {
            if let Some(ref mut p) = self.profile {
                p.preset_count = self.presets.len();
            }
        }
        removed
    }

    // -----------------------------------------------------------------------
    // 7. len() and is_empty()
    // -----------------------------------------------------------------------

    /// Number of registered presets.
    pub fn len(&self) -> usize {
        self.presets.len()
    }

    /// Whether the registry has no presets registered.
    pub fn is_empty(&self) -> bool {
        self.presets.is_empty()
    }
}

// ---------------------------------------------------------------------------
// WorkflowDetector (unchanged – kept for backward compatibility)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::roles::AgentRole;

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

    #[test]
    fn test_new_with_profile() {
        let registry = WorkflowRegistry::new_with_profile();
        let p = registry.profile().expect("profile should exist");
        assert!(p.enabled);
        assert_eq!(p.preset_count, 4);
        assert_eq!(p.match_count, 0);
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
    // 4. match_workflow
    // -----------------------------------------------------------------------

    #[test]
    fn test_match_bug_fix_maps_to_dev() {
        let mut registry = WorkflowRegistry::new();
        let ctx = TaskContext {
            task_type: "bug_fix".to_string(),
            complexity_score: 0.3,
            roles_needed: vec![],
        };
        let preset = registry.match_workflow(&ctx);
        assert!(preset.is_some());
        assert_eq!(preset.unwrap().name, "dev");
    }

    #[test]
    fn test_match_debug_maps_to_dev() {
        let mut registry = WorkflowRegistry::new();
        let ctx = TaskContext {
            task_type: "debug".to_string(),
            complexity_score: 0.1,
            roles_needed: vec![],
        };
        let preset = registry.match_workflow(&ctx);
        assert!(preset.is_some());
        assert_eq!(preset.unwrap().name, "dev");
    }

    #[test]
    fn test_match_complex_maps_to_autopilot() {
        let mut registry = WorkflowRegistry::new();
        let ctx = TaskContext {
            task_type: "feature".to_string(),
            complexity_score: 0.85,
            roles_needed: vec![],
        };
        let preset = registry.match_workflow(&ctx);
        assert!(preset.is_some());
        assert_eq!(preset.unwrap().name, "autopilot");
    }

    #[test]
    fn test_match_simple_qa_maps_to_free() {
        let mut registry = WorkflowRegistry::new();
        let ctx = TaskContext {
            task_type: "q&a".to_string(),
            complexity_score: 0.1,
            roles_needed: vec![],
        };
        let preset = registry.match_workflow(&ctx);
        assert!(preset.is_some());
        assert_eq!(preset.unwrap().name, "free");
    }

    #[test]
    fn test_match_with_coder_role_maps_to_dev() {
        let mut registry = WorkflowRegistry::new();
        let ctx = TaskContext {
            task_type: "general".to_string(),
            complexity_score: 0.5,
            roles_needed: vec!["Coder".to_string()],
        };
        let preset = registry.match_workflow(&ctx);
        assert!(preset.is_some());
        assert_eq!(preset.unwrap().name, "dev");
    }

    #[test]
    fn test_match_empty_roles_maps_to_free() {
        let mut registry = WorkflowRegistry::new();
        let ctx = TaskContext {
            task_type: "general".to_string(),
            complexity_score: 0.5,
            roles_needed: vec![],
        };
        let preset = registry.match_workflow(&ctx);
        assert!(preset.is_some());
        assert_eq!(preset.unwrap().name, "free");
    }

    #[test]
    fn test_match_fallback_to_general() {
        let mut registry = WorkflowRegistry::new();
        let ctx = TaskContext {
            task_type: "refactor".to_string(),
            complexity_score: 0.5,
            roles_needed: vec!["Analyst".to_string()],
        };
        let preset = registry.match_workflow(&ctx);
        assert!(preset.is_some());
        assert_eq!(preset.unwrap().name, "general");
    }

    #[test]
    fn test_match_updates_profile() {
        let mut registry = WorkflowRegistry::new_with_profile();
        let ctx = TaskContext {
            task_type: "bug_fix".to_string(),
            complexity_score: 0.3,
            roles_needed: vec![],
        };
        let _ = registry.match_workflow(&ctx);
        let p = registry.profile().unwrap();
        assert_eq!(p.match_count, 1);
        assert!(p.last_match > 0);
    }

    #[test]
    fn test_match_disabled_profile_returns_none() {
        let mut registry = WorkflowRegistry::new_with_profile();
        if let Some(p) = registry.profile_mut() {
            p.enabled = false;
        }
        let ctx = TaskContext {
            task_type: "bug_fix".to_string(),
            complexity_score: 0.3,
            roles_needed: vec![],
        };
        let preset = registry.match_workflow(&ctx);
        assert!(preset.is_none());
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
    fn test_remove_updates_profile() {
        let mut registry = WorkflowRegistry::new_with_profile();
        registry.remove("free");
        assert_eq!(registry.profile().unwrap().preset_count, 3);
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
