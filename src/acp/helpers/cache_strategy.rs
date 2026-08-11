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
        // Case-insensitive scan of the original text without allocating a
        // lowercased copy of the (potentially large) conversation history.
        // Byte-level scanning is safe for ASCII needles: UTF-8 continuation
        // bytes are in 0x80..=0xBF, which never match an ASCII needle byte.
        EXECUTION_HINTS
            .iter()
            .any(|hint| contains_ascii_case_insensitive(messages_text, hint))
    }
}

/// Case-insensitive substring scan over a UTF-8 haystack for an ASCII needle.
///
/// Never allocates a lowercased copy; folds both sides to lowercase per byte.
fn contains_ascii_case_insensitive(haystack: &str, needle: &str) -> bool {
    let needle_bytes = needle.as_bytes();
    if needle_bytes.is_empty() {
        return true;
    }
    let hay_bytes = haystack.as_bytes();
    if needle_bytes.len() > hay_bytes.len() {
        return false;
    }
    'outer: for i in 0..=(hay_bytes.len() - needle_bytes.len()) {
        for (j, &nb) in needle_bytes.iter().enumerate() {
            if !hay_bytes[i + j].eq_ignore_ascii_case(&nb) {
                continue 'outer;
            }
        }
        return true;
    }
    false
}

impl CacheStrategy {
    /// Convert a concrete cache entry into a decision using the match
    /// confidence computed by the cache lookup (callers pass the score from
    /// [`TokenMultiLevelCache::lookup`] — 1.0 for exact/durable hits, the L2
    /// cosine score otherwise — so the input is embedded exactly once).
    /// Single decision path: confidence > 0.95 serves a Hit unless the request
    /// is execution-like (Refused) — otherwise Miss.
    pub fn decide_from_entry(
        level: &str,
        entry: &CacheEntry,
        confidence: f32,
        is_execution_like: bool,
    ) -> CacheDecision {
        if confidence > 0.95 && !is_execution_like {
            // Clone the response string ONLY on the Hit path (avoids
            // allocating for Refused or Miss).
            CacheDecision::Hit {
                response: entry.output.clone(),
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
}

// ── Backward-compatible wrappers used by process_chat_request ──────────

pub(crate) fn should_bypass_for_execution(mode: &str, messages: &[Message]) -> bool {
    // Scope the hint scan to USER messages only. Injected system/metadata
    // content — startup context (whose summary contains a literal
    // "**Build:** <commands>" line), skill instructions, vector/memory recall
    // — is context, not user intent. Scanning it made the execution hints
    // match on virtually every request in a detected project (e.g. the word
    // "build" in the startup summary), which permanently bypassed the token
    // & semantic caches in production. User messages carry the intent, so
    // scoping the scan to them is strictly more accurate: cache hits that
    // were previously suppressed by metadata now serve, while genuine
    // execution requests (mode in the exec set, or execution hints in the
    // user text) still bypass.
    let user_text: String = messages
        .iter()
        .filter(|m| m.role == "user")
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    CacheStrategy::should_bypass(mode, &user_text)
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
    fn decide_from_entry_hit_refused_miss_matrix() {
        let entry = CacheEntry::new(
            "k".to_string(),
            "input".to_string(),
            "cached-output".to_string(),
            10,
        );
        // High confidence, non-execution-like → Hit carrying the cached response.
        let hit = CacheStrategy::decide_from_entry("L1", &entry, 1.0, false);
        match hit {
            CacheDecision::Hit { response } => assert_eq!(response, "cached-output"),
            _ => panic!("expected hit"),
        }
        // High confidence, execution-like → Refused.
        let refused = CacheStrategy::decide_from_entry("L1", &entry, 1.0, true);
        assert!(matches!(refused, CacheDecision::Refused { .. }));
        // Zero confidence (unknown/miss level) → Miss.
        let miss = CacheStrategy::decide_from_entry("L3", &entry, 0.0, false);
        assert!(matches!(miss, CacheDecision::Miss));
    }

    #[test]
    fn decide_from_entry_handles_unknown_level_as_miss() {
        // L3 template tier was removed (fake implementation); any unknown
        // level string falls back to the default Miss path.
        let entry = CacheEntry::new(
            "k".to_string(),
            "input".to_string(),
            "a very long cached output".to_string(),
            20,
        );
        let decision = CacheStrategy::decide_from_entry("L3", &entry, 0.0, false);
        assert!(matches!(decision, CacheDecision::Miss));
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
    fn should_bypass_for_execution_scans_user_messages_only() {
        // Injected system/metadata (e.g. the startup-context "**Build:**"
        // line) must NOT trigger the execution-like bypass — it is context,
        // not intent. Previously this permanently disabled the caches in
        // detected projects.
        let with_metadata = vec![
            crate::agent::Message {
                role: "system".to_string(),
                content: "[startup context]\n**Build:** cargo build, cargo test".to_string(),
            },
            crate::agent::Message {
                role: "user".to_string(),
                content: "what does this README say?".to_string(),
            },
        ];
        assert!(
            !should_bypass_for_execution("chat", &with_metadata),
            "system metadata must not trigger the execution bypass"
        );

        // …but a user message carrying execution intent still bypasses.
        let exec_intent = vec![
            crate::agent::Message {
                role: "system".to_string(),
                content: "[startup context]\n**Build:** cargo build".to_string(),
            },
            crate::agent::Message {
                role: "user".to_string(),
                content: "implement a login form".to_string(),
            },
        ];
        assert!(
            should_bypass_for_execution("chat", &exec_intent),
            "user execution intent must still bypass"
        );
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
        // The confidence is supplied by the cache lookup (here: a near-identical
        // L2 match above the 0.95 Hit threshold).
        let decision = CacheStrategy::decide_from_entry("L2", &entry, 0.98, false);
        assert!(matches!(decision, CacheDecision::Hit { .. }));
        // A weak match below the threshold is a Miss regardless of level.
        let weak = CacheStrategy::decide_from_entry("L2", &entry, 0.9, false);
        assert!(matches!(weak, CacheDecision::Miss));
    }

    #[test]
    fn decide_from_entry_l1_always_full_confidence() {
        let entry = CacheEntry::new(
            "k".to_string(),
            "input".to_string(),
            "output".to_string(),
            5,
        );
        let decision = CacheStrategy::decide_from_entry("L1", &entry, 1.0, false);
        assert!(matches!(decision, CacheDecision::Hit { .. }));
    }

    #[test]
    fn decide_from_entry_unknown_level_short_output_is_miss() {
        let entry = CacheEntry::new("k".to_string(), "in".to_string(), "ab".to_string(), 2);
        let decision = CacheStrategy::decide_from_entry("L3", &entry, 0.0, false);
        assert!(matches!(decision, CacheDecision::Miss));
    }

    #[test]
    fn decide_from_entry_unknown_level_is_miss() {
        let entry = CacheEntry::new("k".to_string(), "i".to_string(), "o".to_string(), 1);
        let decision = CacheStrategy::decide_from_entry("L99", &entry, 0.0, false);
        assert!(matches!(decision, CacheDecision::Miss));
    }
}
