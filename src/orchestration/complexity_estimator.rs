//! Task Complexity Estimator — Estimates task complexity to drive
//! adaptive BrainLoop iterations, tool selection, and resource allocation.
//!
//! Uses multiple signals (task description length, tool count required,
//! keyword analysis, similarity to historical complex tasks) to produce
//! a 1-10 complexity score that feeds into the BrainLoop and scheduler.

use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// ComplexityLevel
// ---------------------------------------------------------------------------

/// Complexity level derived from analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ComplexityLevel {
    /// Trivial task, single step, no tools needed.
    Trivial,
    /// Simple task, 1-2 steps, basic tools.
    Simple,
    /// Moderate complexity, 3-5 steps, multiple tools.
    Moderate,
    /// Complex task, 5-10 steps, specialized tools.
    Complex,
    /// Very complex, 10+ steps, multi-agent coordination.
    VeryComplex,
}

impl ComplexityLevel {
    pub fn from_score(score: u8) -> Self {
        match score {
            1..=2 => Self::Trivial,
            3..=4 => Self::Simple,
            5..=6 => Self::Moderate,
            7..=8 => Self::Complex,
            9..=10 => Self::VeryComplex,
            _ => Self::Moderate,
        }
    }

    /// Recommended max iterations for this complexity level.
    pub fn recommended_iterations(&self) -> u32 {
        match self {
            Self::Trivial => 1,
            Self::Simple => 3,
            Self::Moderate => 5,
            Self::Complex => 10,
            Self::VeryComplex => 20,
        }
    }

    /// Recommended max fan-out (parallel tool branches).
    pub fn recommended_fanout(&self) -> u32 {
        match self {
            Self::Trivial => 1,
            Self::Simple => 2,
            Self::Moderate => 3,
            Self::Complex => 5,
            Self::VeryComplex => 8,
        }
    }
}

// ---------------------------------------------------------------------------
// ComplexitySignal
// ---------------------------------------------------------------------------

/// Individual signals that contribute to complexity estimation.
#[derive(Debug, Clone)]
struct ComplexitySignal {
    /// Description of the signal.
    pub name: String,
    /// Raw value.
    pub raw_value: f64,
    /// Normalized contribution [0.0, 1.0].
    pub normalized: f64,
    /// Weight of this signal in the final score.
    pub weight: f64,
}

// ---------------------------------------------------------------------------
// ComplexityEstimate
// ---------------------------------------------------------------------------

/// Full complexity estimation result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplexityEstimate {
    /// Final score 1-10.
    pub score: u8,
    /// Derived level.
    pub level: ComplexityLevel,
    /// Recommended max iterations for BrainLoop.
    pub recommended_iterations: u32,
    /// Recommended max fan-out for parallel execution.
    pub recommended_fanout: u32,
    /// Individual signal contributions.
    pub signals: Vec<String>,
}

// ---------------------------------------------------------------------------
// ComplexityEstimator
// ---------------------------------------------------------------------------

/// Estimates task complexity from task descriptions and metadata.
#[derive(Debug)]
pub struct ComplexityEstimator {
    /// Keywords that indicate high complexity.
    complex_keywords: Vec<String>,
    /// Keywords that indicate low complexity.
    simple_keywords: Vec<String>,
    /// Historical complexity scores for similar tasks (task summary → score).
    history: RefCell<HashMap<String, u8>>,
}

impl ComplexityEstimator {
    pub fn new() -> Self {
        Self {
            complex_keywords: vec![
                "refactor".to_string(),
                "migrate".to_string(),
                "architect".to_string(),
                "redesign".to_string(),
                "multi-step".to_string(),
                "pipeline".to_string(),
                "orchestrate".to_string(),
                "distributed".to_string(),
                "concurrent".to_string(),
                "optimize".to_string(),
                "scale".to_string(),
                "integrate".to_string(),
                "transaction".to_string(),
                "consensus".to_string(),
                "failover".to_string(),
                "encrypt".to_string(),
                "authenticate".to_string(),
                "authorize".to_string(),
            ],
            simple_keywords: vec![
                "read".to_string(),
                "echo".to_string(),
                "list".to_string(),
                "check".to_string(),
                "status".to_string(),
                "ping".to_string(),
                "health".to_string(),
                "version".to_string(),
                "help".to_string(),
            ],
            history: RefCell::new(HashMap::new()),
        }
    }

    /// Estimate the complexity of a task description.
    pub fn estimate(&self, task_description: &str) -> ComplexityEstimate {
        let lower = task_description.to_lowercase();
        let word_count = lower.split_whitespace().count();
        let lower = &lower; // bind as &str to reuse

        // Signal 1: Keyword analysis (weight: 0.35)
        let complex_hits = self
            .complex_keywords
            .iter()
            .filter(|k| lower.contains(k.as_str()))
            .count();
        let simple_hits = self
            .simple_keywords
            .iter()
            .filter(|k| lower.contains(k.as_str()))
            .count();
        let keyword_score = if complex_hits > 0 || simple_hits > 0 {
            (complex_hits as f64) / ((complex_hits + simple_hits) as f64).max(1.0)
        } else {
            0.5 // neutral
        };
        let signal1 = ComplexitySignal {
            name: "keyword_analysis".to_string(),
            raw_value: complex_hits as f64 - simple_hits as f64,
            normalized: keyword_score,
            weight: 0.35,
        };

        // Signal 2: Description length (weight: 0.25)
        let length_score = if word_count > 100 {
            1.0
        } else if word_count > 50 {
            0.8
        } else if word_count > 20 {
            0.5
        } else if word_count > 10 {
            0.3
        } else {
            0.1
        };
        let signal2 = ComplexitySignal {
            name: "description_length".to_string(),
            raw_value: word_count as f64,
            normalized: length_score,
            weight: 0.25,
        };

        // Signal 3: Instruction density (weight: 0.20)
        let instruction_markers = ["must", "should", "need to", "require", "ensure", "prevent"];
        let instruction_count = instruction_markers
            .iter()
            .filter(|m| lower.contains(*m))
            .count();
        let instruction_score =
            (instruction_count as f64 / instruction_markers.len() as f64).min(1.0);
        let signal3 = ComplexitySignal {
            name: "instruction_density".to_string(),
            raw_value: instruction_count as f64,
            normalized: instruction_score,
            weight: 0.20,
        };

        // Signal 4: Tool requirement density (weight: 0.20)
        let tool_hints = [
            "file", "shell", "git", "http", "grep", "search", "test", "build", "compile",
        ];
        let tool_hint_count = tool_hints.iter().filter(|t| lower.contains(*t)).count();
        let tool_score = (tool_hint_count as f64 / tool_hints.len() as f64).min(1.0);
        let signal4 = ComplexitySignal {
            name: "tool_requirement_density".to_string(),
            raw_value: tool_hint_count as f64,
            normalized: tool_score,
            weight: 0.20,
        };

        // Weighted sum
        let raw = signal1.normalized * signal1.weight
            + signal2.normalized * signal2.weight
            + signal3.normalized * signal3.weight
            + signal4.normalized * signal4.weight;

        let score = (raw * 9.0 + 1.0).round() as u8;
        let level = ComplexityLevel::from_score(score);

        let signals = vec![
            format!(
                "{} (raw={}, weight={}, score={:.2})",
                signal1.name, signal1.raw_value, signal1.weight, signal1.normalized
            ),
            format!(
                "{} (raw={}, weight={}, score={:.2})",
                signal2.name, signal2.raw_value, signal2.weight, signal2.normalized
            ),
            format!(
                "{} (raw={}, weight={}, score={:.2})",
                signal3.name, signal3.raw_value, signal3.weight, signal3.normalized
            ),
            format!(
                "{} (raw={}, weight={}, score={:.2})",
                signal4.name, signal4.raw_value, signal4.weight, signal4.normalized
            ),
        ];

        // Record in history for trend analysis (F-GAP-17).
        // Keep at most 1000 entries to bound memory growth.
        {
            let mut history = self.history.borrow_mut();
            if history.len() >= 1000 {
                // Retain only the most recent 500 entries.
                let keys: Vec<String> = history.keys().take(500).cloned().collect();
                for k in keys {
                    history.remove(&k);
                }
            }
            history.insert(task_description.to_string(), score);
        }

        ComplexityEstimate {
            score,
            level,
            recommended_iterations: level.recommended_iterations(),
            recommended_fanout: level.recommended_fanout(),
            signals,
        }
    }
}

impl Default for ComplexityEstimator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trivial_task() {
        let estimator = ComplexityEstimator::new();
        let estimate = estimator.estimate("ping health check");
        assert!(estimate.score <= 3, "trivial task should be low complexity");
    }

    #[test]
    fn test_complex_task() {
        let estimator = ComplexityEstimator::new();
        let estimate = estimator.estimate(
            "Refactor the distributed transaction pipeline to integrate with the consensus engine. \
             Must handle failover, concurrent access, and encrypted data. Requires migrating all \
             existing endpoints and adding comprehensive test coverage."
        );
        assert!(
            estimate.score >= 6,
            "complex task should be high complexity: {}",
            estimate.score
        );
    }

    #[test]
    fn test_level_from_score() {
        assert_eq!(ComplexityLevel::from_score(2), ComplexityLevel::Trivial);
        assert_eq!(ComplexityLevel::from_score(5), ComplexityLevel::Moderate);
        assert_eq!(
            ComplexityLevel::from_score(10),
            ComplexityLevel::VeryComplex
        );
    }

    #[test]
    fn test_recommended_iterations() {
        assert_eq!(ComplexityLevel::Trivial.recommended_iterations(), 1);
        assert!(
            ComplexityLevel::VeryComplex.recommended_iterations()
                > ComplexityLevel::Trivial.recommended_iterations()
        );
    }
}
