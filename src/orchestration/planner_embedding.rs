//! BLUE48 Step 1: Task classification for the planner.
//!
//! Provides an `EmbeddingTaskClassifier` that classifies task complexity via
//! keyword heuristics.
//!
//! The embedding-based branch (cosine similarity against a `VectorStore`) was
//! removed as unwired dead code: no code path ever wrote `task-classification`
//! documents into the vector store, so the branch always fell through to the
//! keyword heuristic even when a store was injected.

use crate::orchestration::brain_loop::plan_construction::TaskComplexity;

/// Task classifier using heuristic keyword matching (identical to
/// `Planner::analyze_task` complexity logic).
#[derive(Default)]
pub struct EmbeddingTaskClassifier;

impl EmbeddingTaskClassifier {
    /// Classify a task objective into a `TaskComplexity` level.
    ///
    /// Keyword heuristic matching identical to the former
    /// `classify_with_keywords` (which matched `Planner::analyze_task`).
    pub fn classify_task(&self, objective: &str) -> TaskComplexity {
        let lower = objective.to_ascii_lowercase();
        let len = objective.len();

        let has_code = lower.contains("code")
            || lower.contains("file")
            || lower.contains("implement")
            || lower.contains("function")
            || lower.contains("refactor")
            || lower.contains("class")
            || lower.contains("module")
            || lower.contains("build")
            || lower.contains("test");

        let has_research = lower.contains("research")
            || lower.contains("search")
            || lower.contains("find")
            || lower.contains("analyze")
            || lower.contains("explain")
            || lower.contains("compare");

        let has_multiple = lower.contains(" and ")
            || lower.contains(",")
            || lower.contains("first")
            || lower.contains("then")
            || lower.contains("also")
            || lower.contains("both")
            || lower.contains("multiple");

        let has_strong_code = has_code
            && (lower.contains("refactor")
                || lower.contains("build")
                || lower.contains("write tests"))
            && len > 60;

        if (len > 300 && (has_code || has_research))
            || (has_strong_code && has_multiple && has_research)
        {
            TaskComplexity::Complex
        } else if len > 60 || has_code || has_research || has_multiple {
            TaskComplexity::Medium
        } else {
            TaskComplexity::Simple
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classifier_simple_task() {
        let classifier = EmbeddingTaskClassifier;

        assert_eq!(
            classifier.classify_task("Greet the user"),
            TaskComplexity::Simple
        );
        assert_eq!(
            classifier.classify_task("Hello world"),
            TaskComplexity::Simple
        );
        assert_eq!(classifier.classify_task("Hi"), TaskComplexity::Simple);
        assert_eq!(classifier.classify_task(""), TaskComplexity::Simple);
    }

    #[test]
    fn test_keyword_classify_with_code_indicators() {
        let classifier = EmbeddingTaskClassifier;
        assert_eq!(
            classifier.classify_task("write a function to calculate fibonacci"),
            TaskComplexity::Medium
        );
    }

    #[test]
    fn test_keyword_classify_complex() {
        let classifier = EmbeddingTaskClassifier;
        // strong code signals + research + multiple subtasks → Complex
        assert_eq!(
            classifier.classify_task(
                "Research and refactor the codebase, then write tests for the module and build it"
            ),
            TaskComplexity::Complex
        );
        // code signals alone (>60 chars, no research) → Medium
        assert_eq!(
            classifier
                .classify_task("Refactor the codebase and write tests, then also build the module"),
            TaskComplexity::Medium
        );
    }
}
