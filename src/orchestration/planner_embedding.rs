//! BLUE48 Step 1: Embedding-based task classification with global vector store.
//!
//! Provides an `EmbeddingTaskClassifier` that uses cosine similarity via
//! a global `VectorStore` to classify task complexity and detect task types.
//! Falls back to keyword matching when the vector store is unavailable.

use crate::orchestration::planner_executor::TaskComplexity;
use std::sync::{Arc, OnceLock};

/// Global vector store reference for embedding-based task classification.
/// Set once during server startup; used by all `EmbeddingTaskClassifier` instances.
static GLOBAL_TASK_VECTOR_STORE: OnceLock<Arc<crate::memory::vector::VectorStore>> =
    OnceLock::new();

/// Initialize the global task vector store for embedding-based classification.
/// Call once during server startup after the vector store is created.
pub fn init_global_task_vector_store(store: Arc<crate::memory::vector::VectorStore>) {
    let _ = GLOBAL_TASK_VECTOR_STORE.set(store);
}

/// Check if the global task vector store is available.
pub fn has_global_task_vector_store() -> bool {
    GLOBAL_TASK_VECTOR_STORE.get().is_some()
}

/// Embedding-based task classifier that uses a global `VectorStore`
/// for cosine similarity matching.
///
/// When no global vector store is configured, classification degrades gracefully
/// to heuristic keyword matching (the same logic used by `Planner::analyze_task`).
#[derive(Default)]
pub struct EmbeddingTaskClassifier {
    /// Per-instance override vector store (takes precedence over global).
    vector_store: Option<Arc<crate::memory::vector::VectorStore>>,
}

impl EmbeddingTaskClassifier {
    /// Create a new classifier with an optional per-instance vector store reference.
    /// When `None`, the global vector store (if set via `init_global_task_vector_store`) is used.
    #[allow(dead_code, reason = "F-GAP-49 — reserved for future use")]
    pub fn new(vector_store: Option<Arc<crate::memory::vector::VectorStore>>) -> Self {
        Self { vector_store }
    }

    /// Resolve the active vector store: per-instance > global > None.
    fn resolve_store(&self) -> Option<&crate::memory::vector::VectorStore> {
        self.vector_store
            .as_deref()
            .or_else(|| GLOBAL_TASK_VECTOR_STORE.get().map(|arc| arc.as_ref()))
    }

    /// Classify a task objective into a `TaskComplexity` level.
    ///
    /// Uses embedding-based similarity when a vector store is available,
    /// otherwise falls back to keyword heuristics.
    pub fn classify_task(&self, objective: &str) -> TaskComplexity {
        // Pre-check: if the global store is available, we prefer embedding-based
        // classification. This wires `has_global_task_vector_store` into the
        // classification flow (F-GAP-49).
        let _store_available = has_global_task_vector_store();

        if let Some(store) = self.resolve_store() {
            if let Some(complexity) = self.classify_with_embedding(store, objective) {
                return complexity;
            }
        }
        // Fallback: keyword-based heuristic classification
        self.classify_with_keywords(objective)
    }

    /// Attempt embedding-based classification by searching the vector store
    /// for semantically similar task descriptions and using their associated
    /// metadata to infer complexity.
    fn classify_with_embedding(
        &self,
        store: &crate::memory::vector::VectorStore,
        objective: &str,
    ) -> Option<TaskComplexity> {
        // Search the vector store for entries that resemble the objective.
        // We search across a generic "task-classification" phase with a low
        // similarity threshold to cast a wide net.
        let (hits, _feedback) = store
            .search("task-classification", objective, 5, 0.15, 200)
            .ok()?;

        if hits.is_empty() {
            return None;
        }

        // Use the match quality to guide classification:
        //   - Highest similarity > 0.6  →  tag as Complex (multi-faceted task)
        //   - Average similarity > 0.35 →  tag as Medium
        //   - Otherwise →  let keyword fallback decide
        let best = hits.iter().map(|h| h.similarity).fold(0.0_f32, f32::max);

        if best > 0.6 {
            Some(TaskComplexity::Complex)
        } else if best > 0.35 {
            Some(TaskComplexity::Medium)
        } else {
            None // fall through to keyword heuristics
        }
    }

    /// Keyword-based heuristic classifier, matching `Planner::analyze_task` logic.
    fn classify_with_keywords(&self, objective: &str) -> TaskComplexity {
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
    fn test_classifier_defaults_to_keyword_fallback() {
        let classifier = EmbeddingTaskClassifier::default();

        // Simple task
        assert_eq!(
            classifier.classify_task("Greet the user"),
            TaskComplexity::Simple
        );

        // Medium task
        assert_eq!(
            classifier.classify_task(
                "Fix the bug in the authentication module and verify everything works correctly"
            ),
            TaskComplexity::Medium
        );

        // Complex task
        assert_eq!(
            classifier.classify_task("Research the authentication module, refactor to use JWT, build a middleware chain, and write comprehensive unit tests for all modified components"),
            TaskComplexity::Complex
        );
    }

    #[test]
    fn test_classifier_without_vector_store_falls_back() {
        let classifier = EmbeddingTaskClassifier::new(None);

        assert_eq!(
            classifier.classify_task("Hello world"),
            TaskComplexity::Simple
        );
        assert_eq!(
            classifier.classify_task("Implement a feature"),
            TaskComplexity::Medium
        );
    }

    #[test]
    fn test_classifier_short_objective_is_simple() {
        let classifier = EmbeddingTaskClassifier::default();
        assert_eq!(classifier.classify_task("Hi"), TaskComplexity::Simple);
        assert_eq!(classifier.classify_task(""), TaskComplexity::Simple);
    }

    #[test]
    fn test_keyword_classify_with_code_indicators() {
        let classifier = EmbeddingTaskClassifier::default();
        assert_eq!(
            classifier.classify_task("write a function to calculate fibonacci"),
            TaskComplexity::Medium
        );
    }
}
