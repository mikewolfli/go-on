//! Registry management — CRUD operations for workflow presets
//!
//! `WorkflowRegistry` provides full CRUD operations and intelligent
//! conditional matching against a `TaskContext`.

use super::*;

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
