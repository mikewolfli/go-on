//! S6: Layered Prompt Builder
//!
//! Assembles agent prompts from three layers, each size-capped independently:
//!   1. Identity Layer  — who is this agent and what are its invariants
//!   2. Context Layer   — task snapshot, conversation history digest
//!   3. Task Layer      — concrete instruction for this turn
//!
//! NOTE: This is an intentional architecture framework (S6, Phase 0-9).
//! Kept as a stable extension point for future prompt layering integration.
//! Main chain prompt assembly uses direct formatting in the Agent trait.

use serde::{Deserialize, Serialize};

/// Configuration for prompt layer budgets
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptLayerConfig {
    /// Max chars allocated to the identity layer (default 400)
    #[serde(default = "default_identity_max_chars")]
    pub identity_max_chars: usize,
    /// Max chars allocated to the context layer (default 800)
    #[serde(default = "default_context_max_chars")]
    pub context_max_chars: usize,
    /// Max chars for the task instruction layer (default 4000)
    #[serde(default = "default_task_max_chars")]
    pub task_max_chars: usize,
    /// Whether prompt layering is enabled
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_identity_max_chars() -> usize {
    400
}
fn default_context_max_chars() -> usize {
    800
}
fn default_task_max_chars() -> usize {
    4000
}
fn default_enabled() -> bool {
    true
}

impl Default for PromptLayerConfig {
    fn default() -> Self {
        Self {
            identity_max_chars: 400,
            context_max_chars: 800,
            task_max_chars: 4000,
            enabled: true,
        }
    }
}

/// The three layers of a composed prompt
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PromptLayers {
    pub identity: String,
    pub context: String,
    pub task: String,
}

/// Stats produced by the builder (visible in traces)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PromptBuildStats {
    pub identity_chars: usize,
    pub context_chars: usize,
    pub task_chars: usize,
    pub total_chars: usize,
    pub identity_truncated: bool,
    pub context_truncated: bool,
    pub task_truncated: bool,
}

/// LayeredPromptBuilder assembles layers respecting individual size caps
pub struct LayeredPromptBuilder {
    pub config: PromptLayerConfig,
}

impl LayeredPromptBuilder {
    pub fn new(config: PromptLayerConfig) -> Self {
        Self { config }
    }

    /// Build a single prompt string and return stats
    pub fn build(&self, layers: PromptLayers) -> (String, PromptBuildStats) {
        if !self.config.enabled {
            let task = layers.task.clone();
            let chars = task.len();
            return (
                task,
                PromptBuildStats {
                    task_chars: chars,
                    total_chars: chars,
                    ..Default::default()
                },
            );
        }

        let identity = truncate_str(&layers.identity, self.config.identity_max_chars);
        let identity_truncated = identity.len() < layers.identity.len();
        let context = truncate_str(&layers.context, self.config.context_max_chars);
        let context_truncated = context.len() < layers.context.len();
        let task = truncate_str(&layers.task, self.config.task_max_chars);
        let task_truncated = task.len() < layers.task.len();

        let mut parts = Vec::new();
        if !identity.is_empty() {
            parts.push(identity.to_string());
        }
        if !context.is_empty() {
            parts.push(context.to_string());
        }
        if !task.is_empty() {
            parts.push(task.to_string());
        }
        let composed = parts.join("\n\n");

        let stats = PromptBuildStats {
            identity_chars: identity.len(),
            context_chars: context.len(),
            task_chars: task.len(),
            total_chars: composed.len(),
            identity_truncated,
            context_truncated,
            task_truncated,
        };
        (composed, stats)
    }
}

fn truncate_str(s: &str, max_chars: usize) -> &str {
    if s.len() <= max_chars {
        return s;
    }
    // Truncate at char boundary
    let mut idx = 0;
    for (count, c) in s.chars().enumerate() {
        if count >= max_chars {
            break;
        }
        idx += c.len_utf8();
    }
    &s[..idx]
}
