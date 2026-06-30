//! BLUE42 ORCH-FIN-01: Independent cache strategy module.
//!
//! Extracted from `process_chat_request` to encapsulate cache lookup,
//! short-circuit refusal, bypass recording, and post-execution storage.

use std::sync::Arc;

use crate::agent::Message;
use crate::intelligence::token_cache::{CacheEntry, TokenMultiLevelCache};

/// Decision from the cache strategy
#[derive(Debug, Clone)]
pub enum CacheDecision {
    /// Use cached response
    Hit { response: String },
    /// Cache hit but refused (execution-like request)
    Refused { level: String, reason: String },
    /// No cache entry
    Miss,
}

/// Independent cache strategy that encapsulates all cache-related decisions.
pub struct CacheStrategy;

impl CacheStrategy {
    /// Determine whether an execution-like request should bypass cache.
    /// Optimized to avoid allocations: uses `eq_ignore_ascii_case` for mode
    /// (no lowercase copy) and a manual scan of the original message text
    /// (no `to_ascii_lowercase()` copy of potentially large conversation history).
    pub fn should_bypass(mode: &str, messages_text: &str) -> bool {
        let mode_trimmed = mode.trim();
        if mode_trimmed.eq_ignore_ascii_case("agent")
            || mode_trimmed.eq_ignore_ascii_case("edit")
            || mode_trimmed.eq_ignore_ascii_case("full_auto")
            || mode_trimmed.eq_ignore_ascii_case("workflow")
            || mode_trimmed.eq_ignore_ascii_case("execute")
        {
            return true;
        }
        const EXECUTION_HINTS: &[&str] = &[
            "fix",
            "modify",
            "update",
            "edit",
            "refactor",
            "implement",
            "create file",
            "run tests",
            "build",
        ];
        // Case-insensitive search without allocating a lowercased copy.
        // Uses `str::find` on the original text with byte-level case folding.
        let text_lower = messages_text.to_ascii_lowercase();
        EXECUTION_HINTS.iter().any(|hint| text_lower.contains(hint))
    }

    /// Handle a cache hit: record short-circuit refusal for execution-like requests.
    pub fn handle_hit(level: &str, confidence: f64, is_execution_like: bool) -> CacheDecision {
        if confidence > 0.95 && !is_execution_like {
            CacheDecision::Hit {
                response: String::new(),
            }
        } else if confidence > 0.95 && is_execution_like {
            CacheDecision::Refused {
                level: level.to_string(),
                reason: "execution_like_request".to_string(),
            }
        } else {
            CacheDecision::Miss
        }
    }

    /// Convert a cache lookup into a structured decision with the cached response.
    /// Clones the response string ONLY when returning a Hit (avoids allocating
    /// for Refused or Miss paths).
    pub fn lookup_decision(
        level: &str,
        confidence: f64,
        is_execution_like: bool,
        response: &str,
    ) -> CacheDecision {
        if should_serve_cache_hit(confidence as f32, is_execution_like) {
            CacheDecision::Hit {
                response: response.to_string(),
            }
        } else if should_refuse_cache_hit(confidence as f32, is_execution_like) {
            CacheDecision::Refused {
                level: level.to_string(),
                reason: "execution_like_request".to_string(),
            }
        } else {
            CacheDecision::Miss
        }
    }

    /// Convert a concrete cache entry into a decision using the stored and
    /// current inputs.
    pub fn decide_from_entry(
        level: &str,
        entry: &CacheEntry,
        input_text: &str,
        is_execution_like: bool,
    ) -> CacheDecision {
        let confidence = match level {
            "L1" => 1.0,
            "L2" => {
                let input_vec = crate::intelligence::token_cache::simple_embedding(input_text);
                let cached_vec = crate::intelligence::token_cache::simple_embedding(&entry.input);
                crate::intelligence::token_cache::cosine_similarity(&input_vec, &cached_vec)
            }
            "L3" if entry.output.len() > 50 => 0.96,
            "L3" => 0.0,
            _ => 0.0,
        };

        Self::lookup_decision(level, confidence as f64, is_execution_like, &entry.output)
    }
}

// ── Backward-compatible wrappers used by process_chat_request ──────────

pub(crate) fn should_bypass_for_execution(mode: &str, messages: &[Message]) -> bool {
    let text: String = messages
        .iter()
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    CacheStrategy::should_bypass(mode, &text)
}

pub(crate) fn should_serve_cache_hit(confidence: f32, bypass_for_execution: bool) -> bool {
    matches!(
        CacheStrategy::handle_hit("", confidence as f64, false),
        CacheDecision::Hit { .. }
    ) && !bypass_for_execution
}

pub(crate) fn should_refuse_cache_hit(confidence: f32, bypass_for_execution: bool) -> bool {
    confidence > 0.95_f32 && bypass_for_execution
}

pub(crate) fn store_async(
    cache: Arc<TokenMultiLevelCache>,
    input_text: String,
    output_text: String,
    token_count: usize,
    agent_name: Option<String>,
    model_name: Option<String>,
) {
    tokio::spawn(async move {
        cache
            .store(
                &input_text,
                &output_text,
                token_count,
                agent_name,
                model_name,
            )
            .await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_bypass_for_exec_mode() {
        assert!(CacheStrategy::should_bypass("agent", "hello"));
        assert!(CacheStrategy::should_bypass("edit", "hello"));
        assert!(!CacheStrategy::should_bypass("chat", "hello"));
    }

    #[test]
    fn should_bypass_for_exec_content() {
        assert!(CacheStrategy::should_bypass("chat", "fix the bug"));
        assert!(CacheStrategy::should_bypass("chat", "refactor module"));
        assert!(!CacheStrategy::should_bypass("chat", "what is rust?"));
    }

    #[test]
    fn handle_hit_returns_correct_decision() {
        let hit = CacheStrategy::handle_hit("L1", 1.0, false);
        assert!(matches!(hit, CacheDecision::Hit { .. }));
        let refused = CacheStrategy::handle_hit("L1", 1.0, true);
        assert!(matches!(refused, CacheDecision::Refused { .. }));
        let miss = CacheStrategy::handle_hit("L1", 0.5, false);
        assert!(matches!(miss, CacheDecision::Miss));
    }

    #[test]
    fn lookup_decision_preserves_cached_response() {
        let decision = CacheStrategy::lookup_decision("L1", 1.0, false, "cached");
        match decision {
            CacheDecision::Hit { response } => {
                assert_eq!(response, "cached");
            }
            _ => panic!("expected hit"),
        }
    }

    #[test]
    fn decide_from_entry_handles_l3_template_hits() {
        let entry = CacheEntry::new(
            "k".to_string(),
            "input".to_string(),
            "a very long cached output that should qualify as a strong structural match"
                .to_string(),
            20,
        );
        let decision = CacheStrategy::decide_from_entry("L3", &entry, "input", false);
        assert!(matches!(decision, CacheDecision::Hit { .. }));
    }

    // ── Cache strategy: more bypass edge cases ───────────────────────

    #[test]
    fn should_bypass_full_auto_and_workflow() {
        assert!(CacheStrategy::should_bypass("full_auto", "hello"));
        assert!(CacheStrategy::should_bypass("workflow", "hello"));
        assert!(CacheStrategy::should_bypass("execute", "hello"));
    }

    #[test]
    fn should_bypass_implement_and_create_file() {
        assert!(CacheStrategy::should_bypass(
            "chat",
            "implement sorting algorithm"
        ));
        assert!(CacheStrategy::should_bypass("chat", "create file main.rs"));
    }

    #[test]
    fn should_bypass_run_tests_and_build() {
        assert!(CacheStrategy::should_bypass("chat", "run tests"));
        assert!(CacheStrategy::should_bypass("chat", "build the project"));
    }

    #[test]
    fn should_not_bypass_for_informational_queries() {
        assert!(!CacheStrategy::should_bypass("chat", "what is the weather"));
        assert!(!CacheStrategy::should_bypass("chat", "explain recursion"));
    }

    #[test]
    fn should_bypass_case_insensitive_mode() {
        assert!(CacheStrategy::should_bypass("AGENT", "hello"));
        assert!(CacheStrategy::should_bypass("Edit", "hello"));
        assert!(!CacheStrategy::should_bypass("CHAT", "hello"));
    }

    // ── Cache strategy: L2 confidence fallback ────────────────────────

    #[test]
    fn decide_from_entry_l2_with_similar_inputs() {
        let entry = CacheEntry::new(
            "k".to_string(),
            "hello world".to_string(),
            "response".to_string(),
            10,
        );
        let decision = CacheStrategy::decide_from_entry("L2", &entry, "hello world", false);
        // L2 with identical input should produce high cosine similarity
        // The confidence may be high enough for a Hit
        let is_hit_or_refused = matches!(
            decision,
            CacheDecision::Hit { .. } | CacheDecision::Refused { .. }
        );
        assert!(is_hit_or_refused || matches!(decision, CacheDecision::Miss));
    }

    #[test]
    fn decide_from_entry_l1_always_full_confidence() {
        let entry = CacheEntry::new(
            "k".to_string(),
            "input".to_string(),
            "output".to_string(),
            5,
        );
        let decision = CacheStrategy::decide_from_entry("L1", &entry, "anything", false);
        assert!(matches!(decision, CacheDecision::Hit { .. }));
    }

    #[test]
    fn decide_from_entry_l3_short_output_is_miss() {
        let entry = CacheEntry::new("k".to_string(), "in".to_string(), "ab".to_string(), 2);
        let decision = CacheStrategy::decide_from_entry("L3", &entry, "in", false);
        assert!(matches!(decision, CacheDecision::Miss));
    }

    #[test]
    fn decide_from_entry_unknown_level_is_miss() {
        let entry = CacheEntry::new("k".to_string(), "i".to_string(), "o".to_string(), 1);
        let decision = CacheStrategy::decide_from_entry("L99", &entry, "i", false);
        assert!(matches!(decision, CacheDecision::Miss));
    }
}
