//! BLUE41: CapabilityBus signal bridge for the autonomy loop.
//!
//! Provides structured decision data from the CapabilityBus to replace
//! keyword-based heuristic tool/agent preferences in the autonomy loop.

use serde::{Deserialize, Serialize};

/// Structured capability signals passed from the request handler into
/// the autonomy loop, replacing keyword-based heuristic preferences.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CapabilitySignals {
    /// Agent recommended by the capability bus (if any)
    pub preferred_agent: Option<String>,
    /// Mode recommended by the capability bus
    pub recommended_mode: String,
    /// Tools preferred based on task type and history (ordered by score)
    pub preferred_tools: Vec<String>,
    /// BLUE42 Step 3: Alternative agents available for reroute
    pub agent_alternatives: Vec<String>,
    /// Whether the capability bus was available for this request
    pub capability_bus_available: bool,
    /// Task complexity calculated by the router
    pub task_complexity: u32,
    /// Task type as classified by the router
    pub task_type: String,
}

impl CapabilitySignals {
    /// Build a tool preference list from capability signals, falling back
    /// to task-type defaults when no capability-bus data is available.
    pub fn resolve_tool_preferences(&self, max_tools: usize) -> Vec<String> {
        if !self.preferred_tools.is_empty() {
            let mut tools = self.preferred_tools.clone();
            tools.truncate(max_tools);
            return tools;
        }

        // Fallback: derive tool preferences from task type
        let task_lower = self.task_type.to_ascii_lowercase();
        let mode_lower = self.recommended_mode.to_ascii_lowercase();
        let mut tools: Vec<String> = Vec::new();

        if task_lower.contains("fix")
            || task_lower.contains("modify")
            || task_lower.contains("edit")
            || task_lower.contains("refactor")
            || task_lower.contains("implement")
        {
            tools.push("read_file".to_string());
            tools.push("search_files".to_string());
            tools.push("write_file".to_string());
            tools.push("apply_patch".to_string());
            if task_lower.contains("test") || task_lower.contains("build") {
                tools.push("run_tests".to_string());
            }
        } else if task_lower.contains("search")
            || task_lower.contains("find")
            || task_lower.contains("inspect")
        {
            tools.push("search_files".to_string());
            tools.push("read_file".to_string());
            tools.push("inspect_git_diff".to_string());
        } else if task_lower.contains("test")
            || task_lower.contains("build")
            || task_lower.contains("verify")
        {
            tools.push("run_tests".to_string());
            tools.push("read_file".to_string());
        } else {
            tools.push("read_file".to_string());
            tools.push("search_files".to_string());
        }

        if mode_lower == "execute" || mode_lower == "full_auto" {
            tools.push("bash".to_string());
        }

        tools.truncate(max_tools);
        tools
    }

    /// Returns true if the capability bus explicitly recommended this agent.
    #[cfg(test)]
    pub fn is_agent_preferred(&self, agent_name: &str) -> bool {
        self.preferred_agent
            .as_ref()
            .map(|pa| pa == agent_name)
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::CapabilitySignals;

    #[test]
    fn preferred_agent_helper_reflects_preference() {
        let signals = CapabilitySignals {
            preferred_agent: Some("alpha".to_string()),
            ..CapabilitySignals::default()
        };
        assert!(signals.is_agent_preferred("alpha"));
        assert!(!signals.is_agent_preferred("beta"));
    }
}
