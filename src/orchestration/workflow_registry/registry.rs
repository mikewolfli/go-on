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
    // 4. list – all presets sorted by name
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
        self.presets.remove(name).is_some()
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
