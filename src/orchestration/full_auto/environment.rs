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
    /// Prepare the execution environment for the given task intent.
    ///
    /// Builds a snapshot of relevant context (mode, goals, constraints) and
    /// checks whether prerequisites are declared (proxy for runtime
    /// readiness).
    ///
    /// Results are cached keyed by the prerequisites list so that repeated
    /// calls with the same prerequisites avoid recomputation.
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

        // If prerequisites are declared we consider them checkable; in a
        // production setting this would invoke the actual dependency
        // resolver.
        let dependencies_checked = true;
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
