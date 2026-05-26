//! OrchestrationContext — explicit dependency container for orchestration services.
//!
//! Removes the global `OnceLock` singletons (`PERFORMANCE_FEED`, `FAILOVER`) from
//! `orchestrator.rs` so that callers supply their own context.  This makes testing
//! order-independent and enables isolated execution.

use crate::intelligence::hot_failover::{HotFailover, HotFailoverConfig};
use crate::observability::live_performance::LivePerformanceFeed;

/// Holds the live-performance feed and hot-failover instance used by the
/// orchestrator for model selection, cost/latency estimation, and failover.
///
/// Create one early in your request lifecycle and pass it to orchestrator
/// functions such as `select_model_for_task` and `select_model_semantic`.
pub struct OrchestrationContext {
    performance_feed: LivePerformanceFeed,
    failover: HotFailover,
}

impl OrchestrationContext {
    /// Create a new context with default feed and failover settings.
    pub fn new() -> Self {
        Self {
            performance_feed: LivePerformanceFeed::default(),
            failover: HotFailover::new(HotFailoverConfig::default()),
        }
    }

    /// Create a context with explicitly provided instances (useful for tests
    /// or custom configuration).
    pub fn with_feeds(performance_feed: LivePerformanceFeed, failover: HotFailover) -> Self {
        Self {
            performance_feed,
            failover,
        }
    }

    // ------------------------------------------------------------------
    // Accessors
    // ------------------------------------------------------------------

    /// Reference to the live-performance feed.
    pub fn performance_feed(&self) -> &LivePerformanceFeed {
        &self.performance_feed
    }

    /// Reference to the hot-failover instance.
    pub fn failover(&self) -> &HotFailover {
        &self.failover
    }

    // ------------------------------------------------------------------
    // Convenience methods
    // ------------------------------------------------------------------

    /// Record a model execution outcome.
    pub fn record_model_execution(&self, model_id: &str, success: bool, latency_ms: u64) {
        if success {
            self.performance_feed.record_success(model_id, latency_ms);
        } else {
            self.performance_feed.record_failure(model_id, latency_ms);
        }
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
        let _failover = ctx.failover();
    }

    #[test]
    fn test_context_default_trait() {
        let ctx = OrchestrationContext::default();
        let _feed = ctx.performance_feed();
        let _failover = ctx.failover();
    }

    #[test]
    fn test_context_with_feeds() {
        let feed = LivePerformanceFeed::default();
        let failover = HotFailover::new(HotFailoverConfig::default());
        let ctx = OrchestrationContext::with_feeds(feed, failover);
        let _feed = ctx.performance_feed();
        let _failover = ctx.failover();
    }
}
