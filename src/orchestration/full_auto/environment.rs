//! Environment state tracking for [`FullAutoFlow`](super::FullAutoFlow).
//!
//! Provides [`ExecutionEnvironment`] — a snapshot of the execution context —
//! and the [`prepare_environment`](super::FullAutoFlow::prepare_environment)
//! method that builds it from a [`TaskIntent`](super::intent::TaskIntent).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tracing::debug;

use super::{intent::TaskIntent, EnvCacheValue, FullAutoFlow};

// ---------------------------------------------------------------------------
// ExecutionEnvironment
// ---------------------------------------------------------------------------

/// Snapshot of the execution environment at the time of the run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionEnvironment {
    /// Whether all declared prerequisites have been verified.
    pub dependencies_checked: bool,
    /// Whether the required runtime is available.
    pub runtime_ready: bool,
    /// Environment variable / context snapshot.
    pub env_snapshot: HashMap<String, String>,
}

impl ExecutionEnvironment {
    /// Return `true` when the environment is fully ready for execution.
    #[cfg(test)]
    pub fn is_ready(&self) -> bool {
        self.dependencies_checked && self.runtime_ready
    }
}

// ---------------------------------------------------------------------------
// Environment preparation method on FullAutoFlow
// ---------------------------------------------------------------------------

impl FullAutoFlow {
    /// Check whether a single prerequisite (tool/service) is available.
    ///
    /// Uses `command -v` to verify that the named executable is present on
    /// the system PATH. Non-tool prerequisites (e.g. "network access") are
    /// conservatively reported as unavailable so the operator can explicitly
    /// whitelist them.
    fn check_prerequisite_available(prereq: &str) -> bool {
        // Skip empty strings.
        if prereq.is_empty() {
            return true;
        }

        // If the string looks like a simple command name (single word, no
        // path separators) try to resolve it via the system shell.
        if !prereq.contains(char::is_whitespace) && !prereq.contains('/') && !prereq.contains('\\')
        {
            std::process::Command::new("sh")
                .arg("-c")
                .arg(format!("command -v '{}'", prereq.replace('\'', "'\\''")))
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|status| status.success())
                .unwrap_or(false)
        } else {
            // Non-trivial prerequisite string — cannot verify automatically.
            false
        }
    }

    pub fn prepare_environment(&self, intent: &TaskIntent) -> ExecutionEnvironment {
        if !self.config.enable_env_check {
            return ExecutionEnvironment {
                dependencies_checked: true,
                runtime_ready: true,
                env_snapshot: HashMap::new(),
            };
        }

        // Fast-path cache check.
        if let Some(cached) = self.cache.get_env(&intent.prerequisites) {
            debug!("prepare_environment: returning cached environment");
            return ExecutionEnvironment {
                dependencies_checked: cached.dependencies_checked,
                runtime_ready: cached.runtime_ready,
                env_snapshot: HashMap::new(),
            };
        }

        let mut env_snapshot = HashMap::new();
        env_snapshot.insert("mode".to_string(), "full_auto".to_string());
        env_snapshot.insert("task_goals".to_string(), intent.goals.join("; "));
        env_snapshot.insert("constraints".to_string(), intent.constraints.join("; "));

        // Actually check each declared prerequisite.
        let missing: Vec<String> = intent
            .prerequisites
            .iter()
            .filter(|p| !Self::check_prerequisite_available(p))
            .cloned()
            .collect();

        let dependencies_checked = missing.is_empty();

        if !dependencies_checked {
            let error_msg = format!("Missing prerequisites: {}", missing.join(", "));
            tracing::warn!("prepare_environment: {}", error_msg);
            env_snapshot.insert("dependencies_error".to_string(), error_msg);
        }

        // Runtime is ready when there are NO outstanding prerequisites.
        // Empty prerequisites means trivially ready.
        let runtime_ready = intent.prerequisites.is_empty();

        let result = ExecutionEnvironment {
            dependencies_checked,
            runtime_ready,
            env_snapshot,
        };

        // Store in cache for future fast-path lookups.
        self.cache.set_env(
            &intent.prerequisites,
            EnvCacheValue {
                dependencies_checked: result.dependencies_checked,
                runtime_ready: result.runtime_ready,
            },
        );

        result
    }
}
