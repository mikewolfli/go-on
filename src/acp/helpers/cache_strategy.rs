//! BLUE42 ORCH-FIN-01: Independent cache strategy module.
//!
//! Extracted from `process_chat_request` to encapsulate cache lookup,
//! short-circuit refusal, bypass recording, and post-execution storage.

use std::sync::Arc;

use serde_json::Value;

use crate::agent::Message;
use crate::intelligence::token_cache::{CacheEntry, TokenMultiLevelCache};

/// Decision from the cache strategy
#[derive(Debug, Clone)]
pub enum CacheDecision {
    /// Use cached response
    Hit { response: String, level: String },
    /// Cache hit but refused (execution-like request)
    Refused { level: String, reason: String },
    /// No cache entry
    Miss,
}

/// Result of a cache store operation
#[cfg(test)]
#[derive(Debug, Clone)]
pub struct CacheStoreResult {
    pub stored: bool,
    pub level: String,
}

/// Independent cache strategy that encapsulates all cache-related decisions.
pub struct CacheStrategy;

impl CacheStrategy {
    /// Determine whether an execution-like request should bypass cache.
    pub fn should_bypass(mode: &str, messages_text: &str) -> bool {
        let mode_lower = mode.trim().to_ascii_lowercase();
        if matches!(
            mode_lower.as_str(),
            "agent" | "edit" | "full_auto" | "workflow" | "execute"
        ) {
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
        let text_lower = messages_text.to_ascii_lowercase();
        EXECUTION_HINTS.iter().any(|hint| text_lower.contains(hint))
    }

    /// Handle a cache hit: record short-circuit refusal for execution-like requests.
    pub fn handle_hit(level: &str, confidence: f64, is_execution_like: bool) -> CacheDecision {
        if confidence > 0.95 && !is_execution_like {
            CacheDecision::Hit {
                response: String::new(),
                level: level.to_string(),
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
    pub fn lookup_decision(
        level: &str,
        confidence: f64,
        is_execution_like: bool,
        response: String,
    ) -> CacheDecision {
        if should_serve_cache_hit(confidence as f32, is_execution_like) {
            CacheDecision::Hit {
                response,
                level: level.to_string(),
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

        Self::lookup_decision(
            level,
            confidence as f64,
            is_execution_like,
            entry.output.clone(),
        )
    }

    /// Build a structured agent_attempt entry for cache outcomes.
    pub fn attempt_entry(decision: &CacheDecision) -> Value {
        match decision {
            CacheDecision::Hit { level, .. } => serde_json::json!({
                "cached": true, "cache_level": level, "duration_ms": 0u64
            }),
            CacheDecision::Refused { level, reason } => serde_json::json!({
                "cached": false, "shortcircuit_refused": true,
                "cache_level": level, "reason": reason, "duration_ms": 0u64
            }),
            CacheDecision::Miss => serde_json::json!({
                "cached": false, "duration_ms": 0u64
            }),
        }
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
    fn cache_store_result_shape_is_constructible() {
        let stored = CacheStoreResult {
            stored: true,
            level: "L2".to_string(),
        };
        assert!(stored.stored);
        assert_eq!(stored.level, "L2");
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
        let decision = CacheStrategy::lookup_decision("L1", 1.0, false, "cached".to_string());
        match decision {
            CacheDecision::Hit { response, level } => {
                assert_eq!(response, "cached");
                assert_eq!(level, "L1");
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
}
