//! OrchestrationContext — explicit dependency container for orchestration services.
//!
//! Removes the global `OnceLock` singletons (`PERFORMANCE_FEED`, `FAILOVER`) from
//! `orchestrator.rs` so that callers supply their own context.  This makes testing
//! order-independent and enables isolated execution.

use crate::observability::live_performance::LivePerformanceFeed;

/// Holds the live-performance feed used by the orchestrator for model
/// selection, cost/latency estimation.
///
/// Create one early in your request lifecycle and pass it to orchestrator
/// functions such as `select_model_for_task` and `select_model_semantic`.
pub struct OrchestrationContext {
    performance_feed: LivePerformanceFeed,
}

impl OrchestrationContext {
    /// Create a new context with default feed settings.
    pub fn new() -> Self {
        Self {
            performance_feed: LivePerformanceFeed::default(),
        }
    }

    /// Reference to the live-performance feed.
    pub fn performance_feed(&self) -> &LivePerformanceFeed {
        &self.performance_feed
    }
}

impl Default for OrchestrationContext {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_new_has_defaults() {
        let ctx = OrchestrationContext::new();
        // Accessors should return valid references without panicking.
        let _feed = ctx.performance_feed();
    }

    #[test]
    fn test_context_default_trait() {
        let ctx = OrchestrationContext::default();
        let _feed = ctx.performance_feed();
    }

    #[test]
    fn performance_feed_records_and_reports_success_rate() {
        let ctx = OrchestrationContext::new();
        // Feed is directly usable for model tracking.
        ctx.performance_feed().record_success("model-a", 100);
        let success_rate = ctx.performance_feed().get_success_rate("model-a");
        assert!(success_rate.is_some());
        assert!(success_rate.unwrap() > 0.0);
        assert_eq!(ctx.performance_feed().get_request_count("model-a"), 1);
    }
}
