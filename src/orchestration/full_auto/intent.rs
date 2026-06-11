//! Intent detection and goal management for [`FullAutoFlow`](super::FullAutoFlow).
//!
//! Provides [`TaskIntent`] — a structured representation of a parsed task —
//! and the [`parse_task`](super::FullAutoFlow::parse_task) method that
//! extracts goals, constraints, prerequisites, and deliverables from a
//! free‑form description.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use tracing::debug;

use super::{FullAutoFlow, IntentCacheValue};

// ---------------------------------------------------------------------------
// TaskIntent
// ---------------------------------------------------------------------------

/// Structured representation of a parsed task.
///
/// Each field captures a distinct dimension extracted from the raw task
/// description via lightweight heuristics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskIntent {
    /// What the task aims to achieve.
    pub goals: Vec<String>,
    /// Boundaries and limitations that must be respected.
    pub constraints: Vec<String>,
    /// Required skills, tools, or runtime capabilities.
    pub prerequisites: Vec<String>,
    /// Expected outputs or artifacts.
    pub deliverables: Vec<String>,
}

impl TaskIntent {
    /// Build a combined text string from all goals for matching purposes.
    pub fn goal_text(&self) -> String {
        self.goals.join(" ")
    }

    /// Check whether non‑zero goals exist.
    #[cfg(test)]
    pub fn has_goals(&self) -> bool {
        !self.goals.is_empty()
    }

    /// Number of known constraints.
    #[cfg(test)]
    pub fn constraint_count(&self) -> usize {
        self.constraints.len()
    }
}

// ---------------------------------------------------------------------------
// Parsing methods on FullAutoFlow
// ---------------------------------------------------------------------------

impl FullAutoFlow {
    /// Parse a free‑form task description into a structured `TaskIntent`.
    ///
    /// Uses lightweight heuristics to identify goals, constraints,
    /// prerequisites, and deliverables from the raw text. Lines prefixed
    /// with `-` or `*` are classified by keyword (`goal:`, `constraint:`,
    /// `require:`, `deliverable:`, `output:`). Unclassified bullet lines
    /// and multi‑word standalone lines default to goals.
    ///
    /// Results are cached via the fast-path cache so that repeated calls
    /// with the same task text avoid re-parsing.
    pub fn parse_task(&self, task: &str) -> TaskIntent {
        // Fast-path cache check.
        if let Some(cached) = self.cache.get_intent(task) {
            debug!("parse_task: returning cached intent");
            return cached.into_task_intent();
        }

        let mut goals: Vec<String> = Vec::new();
        let mut constraints: Vec<String> = Vec::new();
        let mut prerequisites: Vec<String> = Vec::new();
        let mut deliverables: Vec<String> = Vec::new();

        for line in task.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let (is_bullet, content) = if let Some(rest) = trimmed
                .strip_prefix('-')
                .or_else(|| trimmed.strip_prefix('*'))
            {
                (true, rest.trim())
            } else {
                (false, trimmed)
            };

            if content.is_empty() {
                if is_bullet {
                    // An empty bullet is meaningless; skip.
                    continue;
                }
                // Non‑empty trimmed line with no bullet → treat as goal.
                goals.push(trimmed.to_string());
                continue;
            }

            let lower = content.to_lowercase();

            if lower.starts_with("goal:") || lower.starts_with("goal ") {
                goals.push(Self::strip_label(content, &["goal:", "goal "]));
            } else if lower.starts_with("constraint:") || lower.starts_with("constraint ") {
                constraints.push(Self::strip_label(content, &["constraint:", "constraint "]));
            } else if lower.starts_with("require:")
                || lower.starts_with("require ")
                || lower.starts_with("prerequisite:")
                || lower.starts_with("prerequisite ")
            {
                prerequisites.push(Self::strip_label(
                    content,
                    &["require:", "require ", "prerequisite:", "prerequisite "],
                ));
            } else if lower.starts_with("deliverable:")
                || lower.starts_with("deliverable ")
                || lower.starts_with("output:")
                || lower.starts_with("output ")
            {
                deliverables.push(Self::strip_label(
                    content,
                    &["deliverable:", "deliverable ", "output:", "output "],
                ));
            } else if is_bullet {
                // Unclassified bullet → default to goal.
                goals.push(content.to_string());
            } else if trimmed.len() > 10 {
                // Longer non‑bullet line → heuristic for an implicit goal.
                goals.push(trimmed.to_string());
            }
        }

        // Fallback: if nothing useful was parsed, treat the entire task as
        // a single goal so the flow has something to work with.
        if goals.is_empty() && task.len() > 5 {
            goals.push(task.to_string());
        }

        let intent = TaskIntent {
            goals,
            constraints,
            prerequisites,
            deliverables,
        };

        // Store in cache for future fast-path lookups.
        self.cache
            .set_intent(task, IntentCacheValue::from(intent.clone()));

        intent
    }

    /// Strip one of the recognised labels from the front of `content`.
    fn strip_label(content: &str, labels: &[&str]) -> String {
        let lower = content.to_lowercase();
        for label in labels {
            if lower.starts_with(label) {
                let remainder = &content[label.len()..];
                return remainder.trim().to_string();
            }
        }
        content.to_string()
    }
}

/// Tokenize a string into a set of lowercased alphanumeric tokens of length
/// ≥ 3.
pub(crate) fn tokenize(text: &str) -> BTreeSet<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| t.len() >= 3)
        .map(|t| t.to_ascii_lowercase())
        .collect()
}
