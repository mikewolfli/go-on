//! Session Context Manager — Key concept extraction, intelligent message
//! retention, context window negotiation, and continuity markers.
//!
//! Enhances the existing SessionCompressor with semantic context preservation
//! to maintain conversation quality across long sessions without exceeding
//! token budget limits.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::session_compressor::{CompressedSession, SessionCompressor};

// ---------------------------------------------------------------------------
// ContextWindowBudget
// ---------------------------------------------------------------------------

/// Dynamic context window budget allocation.
#[derive(Debug, Clone)]
pub struct ContextWindowBudget {
    /// Maximum total messages to retain.
    pub max_messages: usize,
    /// Minimum messages to always retain (anchors).
    pub min_retain: usize,
    /// Current task complexity (1-10), affects retention aggressiveness.
    pub task_complexity: u8,
}

impl Default for ContextWindowBudget {
    fn default() -> Self {
        Self {
            max_messages: 1000,
            min_retain: 20,
            task_complexity: 5,
        }
    }
}

impl ContextWindowBudget {
    /// Adjust retention based on task complexity.
    /// Higher complexity → retain more context.
    pub fn effective_retain(&self) -> usize {
        let base = self.max_messages;
        let factor = self.task_complexity as f64 / 10.0;
        let adjusted = (base as f64 * (0.3 + factor * 0.7)) as usize;
        adjusted.max(self.min_retain).min(self.max_messages)
    }
}

// ---------------------------------------------------------------------------
// ContinuityMarker
// ---------------------------------------------------------------------------

/// A lightweight context marker inserted when conversation is trimmed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContinuityMarker {
    /// Summary of what was trimmed.
    pub summary: String,
    /// Key concepts that were extracted from trimmed messages.
    pub key_concepts: Vec<String>,
    /// Files that were referenced.
    pub files_referenced: Vec<String>,
    /// Decisions that were made.
    pub decisions_made: Vec<String>,
    /// How many messages were trimmed.
    pub messages_trimmed: usize,
    /// Error or issues that were encountered.
    pub issues_encountered: Vec<String>,
}

// ---------------------------------------------------------------------------
// SessionContextManager
// ---------------------------------------------------------------------------

/// The central session context manager.
pub struct SessionContextManager {
    /// File paths referenced in this session.
    file_paths: HashSet<String>,
    /// Decisions recorded in this session.
    decisions: Vec<(String, usize)>,
    /// Messages that contain errors.
    error_messages: HashSet<usize>,
    /// Configuration for the context window.
    pub budget: ContextWindowBudget,
    /// Current message count.
    message_count: usize,
    /// Count of messages that contain concepts.
    concept_count: AtomicUsize,
}

impl SessionContextManager {
    pub fn new(budget: ContextWindowBudget) -> Self {
        Self {
            file_paths: HashSet::new(),
            decisions: Vec::new(),
            error_messages: HashSet::new(),
            budget,
            message_count: 0,
            concept_count: AtomicUsize::new(0),
        }
    }

    /// Record a new message and extract its concepts.
    pub fn record_message(&mut self, content: &str) {
        let idx = self.message_count;
        self.message_count += 1;

        // Extract file paths (simple heuristic: /path or .ext patterns)
        let file_path_count = self.extract_file_paths(content);

        // Extract error mentions
        let content_lower = content.to_lowercase();
        let error_keywords = ["error", "fail", "exception", "panic", "timeout", "denied"];
        let has_errors = error_keywords.iter().any(|k| content_lower.contains(k));
        if has_errors {
            self.error_messages.insert(idx);
        }

        // Extract decisions (sentences with decision keywords)
        let decision_keywords = [
            "decided",
            "choose",
            "selected",
            "agreed",
            "confirmed",
            "resolved",
            "finalized",
        ];
        let has_decisions = decision_keywords.iter().any(|k| content_lower.contains(k));
        if has_decisions {
            self.decisions.push((content.to_string(), idx));
        }

        // Track concept-bearing messages (contains decisions, file paths, or errors)
        let has_concepts = has_decisions || file_path_count > 0 || has_errors;
        if has_concepts {
            self.concept_count.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Returns the number of file paths extracted.
    fn extract_file_paths(&mut self, content: &str) -> usize {
        let mut count = 0;
        // Match patterns like /path/to/file.rs, src/main.rs, Cargo.toml
        for word in content.split_whitespace() {
            let word = word.trim_matches(|c: char| {
                !c.is_alphanumeric() && c != '/' && c != '.' && c != '_' && c != '-'
            });
            if word.contains('/')
                || word.ends_with(".rs")
                || word.ends_with(".toml")
                || word.ends_with(".md")
                || word.ends_with(".json")
                || word.ends_with(".yaml")
                || word.ends_with(".py")
                || word.ends_with(".ts")
                || word.ends_with(".js")
            {
                self.file_paths.insert(word.to_string());
                count += 1;
            }
        }
        count
    }

    /// Compute importance score for a message at the given index.
    pub fn score_message(&self, index: usize, content: &str) -> u32 {
        let has_code_block = content.contains("```");
        let has_decision = self.decisions.iter().any(|(_, i)| *i == index);
        let has_file_path = self.file_paths.iter().any(|fp| content.contains(fp));
        let has_error = self.error_messages.contains(&index);
        // Code blocks get the highest single-weight bonus (18); the previous
        // `* 3` + `+= 15` spelling was two steps for one weight and confusing.
        let mut score = if has_code_block { 18 } else { 0 };
        if has_decision {
            score += 20;
        }
        if has_file_path {
            score += 10;
        }
        if has_error {
            score += 15;
        }
        score.min(100)
    }

    /// Select which messages to retain when trimming is needed.
    pub fn select_retained_messages(
        &self,
        messages: &[(String, String)], // (role, content)
        max_retain: usize,
    ) -> Vec<usize> {
        let total = messages.len();
        if total <= max_retain {
            return (0..total).collect();
        }

        // If the number of messages far exceeds the budget, note that
        // semantic compression (via SessionCompressor) could be used.
        if total > max_retain * 2 {
            tracing::debug!(
                "select_retained_messages: {} messages exceeds budget {} by 2× — SessionCompressor could reduce overhead",
                total, max_retain,
            );
        }

        let mut scored: Vec<(usize, u32)> = messages
            .iter()
            .enumerate()
            .map(|(i, (_role, content))| {
                let score = self.score_message(i, content);
                (i, score)
            })
            .collect();

        // Sort by combined score descending
        scored.sort_by_key(|(_, score)| std::cmp::Reverse(*score));

        // Always include first and last
        let mut retained: HashSet<usize> = HashSet::new();
        retained.insert(0);
        retained.insert(total - 1);

        // Fill remaining slots with highest scored messages
        for (idx, _) in scored {
            if retained.len() >= max_retain {
                break;
            }
            retained.insert(idx);
        }

        let mut result: Vec<usize> = retained.into_iter().collect();
        result.sort();
        result
    }

    /// Generate a continuity marker for trimmed messages.
    pub fn generate_continuity_marker(&self, trimmed_indices: &[usize]) -> ContinuityMarker {
        // Extract key concepts from decisions of trimmed messages
        let key_concepts: Vec<String> = self
            .decisions
            .iter()
            .filter(|(_, i)| trimmed_indices.contains(i))
            .flat_map(|(d, _)| {
                d.split_whitespace()
                    .filter(|w| w.len() > 4 && w.chars().all(|c| c.is_alphanumeric()))
                    .map(|w| w.to_string())
            })
            .take(20)
            .collect();

        let files = self.file_paths.iter().cloned().collect();
        let decisions = self
            .decisions
            .iter()
            .filter(|(_, i)| trimmed_indices.contains(i))
            .map(|(d, _)| d.clone())
            .collect();

        let issues = trimmed_indices
            .iter()
            .filter(|i| self.error_messages.contains(i))
            .map(|_| "encountered errors in trimmed context".to_string())
            .collect();

        ContinuityMarker {
            summary: format!(
                "[{} messages trimmed for context window]",
                trimmed_indices.len()
            ),
            key_concepts,
            files_referenced: files,
            decisions_made: decisions,
            messages_trimmed: trimmed_indices.len(),
            issues_encountered: issues,
        }
    }

    /// Compress a list of messages using the given [`SessionCompressor`].
    /// Delegates to `SessionCompressor::compress()` and returns the
    /// [`CompressedSession`] containing summary, kept messages, and metrics.
    ///
    /// Callers should check `compressed.compression_ratio` to determine
    /// whether compression was beneficial.
    pub fn compress_messages(
        &self,
        messages: &[(String, String)],
        compressor: &SessionCompressor,
    ) -> CompressedSession {
        let compressor_msgs: Vec<super::session_compressor::Message> = messages
            .iter()
            .map(|(role, content)| {
                super::session_compressor::Message::new(role.clone(), content.clone())
            })
            .collect();
        compressor.compress(&compressor_msgs)
    }

    /// Get the current concept count — messages that contain decisions, file paths,
    /// or error keywords tracked by `record_message`.
    pub fn concept_count(&self) -> usize {
        self.concept_count.load(Ordering::Relaxed)
    }

    /// Get the current number of tracked decisions.
    pub fn decision_count(&self) -> usize {
        self.decisions.len()
    }
}

impl Default for SessionContextManager {
    fn default() -> Self {
        Self::new(ContextWindowBudget::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_importance_score_message_direct() {
        let mgr = SessionContextManager::default();
        // No concepts, no decisions, no code blocks, no file paths -> score of 0
        let score = mgr.score_message(0, "hello world");
        assert_eq!(score, 0);
        // Contains a code block -> score (3 from initial) + 15 = 18
        let score2 = mgr.score_message(0, "hello ```code``` world");
        assert_eq!(score2, 18);
    }

    #[test]
    fn test_context_window_budget_complexity() {
        let mut budget = ContextWindowBudget {
            task_complexity: 1,
            ..Default::default()
        };
        assert!(budget.effective_retain() < budget.max_messages);
        budget.task_complexity = 10;
        assert_eq!(budget.effective_retain(), budget.max_messages);
    }

    #[test]
    fn test_record_message_extracts_files() {
        let mut mgr = SessionContextManager::default();
        mgr.record_message("Fix src/main.rs and tests/e2e.rs");
        assert!(mgr.file_paths.contains("src/main.rs"));
        assert!(mgr.file_paths.contains("tests/e2e.rs"));
    }

    #[test]
    fn test_record_message_detects_error() {
        let mut mgr = SessionContextManager::default();
        mgr.record_message("Got an error: timeout expired");
        assert_eq!(mgr.error_messages.len(), 1);
    }

    #[test]
    fn test_record_message_detects_decision() {
        let mut mgr = SessionContextManager::default();
        mgr.record_message("I have decided to use the retry strategy");
        assert_eq!(mgr.decision_count(), 1);
    }

    #[test]
    fn test_select_retained_messages_anchors() {
        let mgr = SessionContextManager::default();
        let messages = vec![
            ("user".to_string(), "Hello".to_string()),
            ("assistant".to_string(), "Hi".to_string()),
            ("user".to_string(), "Fix bug".to_string()),
        ];
        let retained = mgr.select_retained_messages(&messages, 2);
        assert!(retained.contains(&0), "first message must be retained");
        assert!(retained.contains(&2), "last message must be retained");
    }

    #[test]
    fn test_continuity_marker_generation() {
        let mut mgr = SessionContextManager::default();
        mgr.record_message("Fix src/lib.rs: error in main loop");
        mgr.record_message("Decided: use async approach");
        let marker = mgr.generate_continuity_marker(&[0]);
        assert_eq!(marker.messages_trimmed, 1);
        assert!(!marker.files_referenced.is_empty());
    }

    #[test]
    fn test_message_scoring_different_content() {
        let mut mgr = SessionContextManager::default();
        mgr.record_message("Fix src/main.rs");
        let score_with_file = mgr.score_message(0, "Fix src/main.rs");
        let score_plain = mgr.score_message(0, "hello");
        assert!(
            score_with_file > score_plain,
            "messages with file paths should score higher"
        );
    }
}
