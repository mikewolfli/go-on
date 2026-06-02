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
    /// Updates both LivePerformanceFeed and HotFailover for automatic
    /// model switching on repeated failures.
    pub fn record_model_execution(&self, model_id: &str, success: bool, latency_ms: u64) {
        if success {
            self.performance_feed.record_success(model_id, latency_ms);
        } else {
            self.performance_feed.record_failure(model_id, latency_ms);
            // Record failure in HotFailover to enable automatic model switching
            self.failover.record_failure(model_id);
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

    // ── record_model_execution: LivePerformanceFeed updates ───────────

    #[test]
    fn record_model_execution_success_updates_feed() {
        let ctx = OrchestrationContext::new();
        ctx.record_model_execution("model-a", true, 100);

        // After recording a success, the feed should reflect it
        let feed = ctx.performance_feed();
        let success_rate = feed.get_success_rate("model-a");
        assert!(
            success_rate.is_some(),
            "success rate should be available after recording"
        );
        assert!(success_rate.unwrap() > 0.0);
        assert_eq!(feed.get_request_count("model-a"), 1);
    }

    #[test]
    fn record_model_execution_failure_updates_feed() {
        let ctx = OrchestrationContext::new();
        ctx.record_model_execution("model-b", false, 50);

        let feed = ctx.performance_feed();
        let success_rate = feed.get_success_rate("model-b");
        assert!(success_rate.is_some());
        assert!((success_rate.unwrap() - 0.0).abs() < 0.01);
    }

    #[test]
    fn record_model_execution_mixed_outcomes_no_panic() {
        let ctx = OrchestrationContext::new();
        for i in 0..10 {
            let success = i % 2 == 0;
            ctx.record_model_execution("model-c", success, (i as u64) * 10);
        }
        // Multiple records of mixed outcomes should produce intermediate rates
        let rate = ctx.performance_feed().get_success_rate("model-c").unwrap();
        assert!(rate > 0.0 && rate < 1.0);
        assert_eq!(ctx.performance_feed().get_request_count("model-c"), 10);
    }

    // ── HotFailover integration: model failure → auto-switch ──────────

    #[test]
    fn failover_tracks_model_failures() {
        let ctx = OrchestrationContext::new();
        let failover = ctx.failover();

        // Record failures via the orchestration context
        ctx.record_model_execution("model-d", false, 200);

        // The failover should mark the model as blacklisted
        assert!(
            failover.is_blacklisted("model-d"),
            "failed model should be blacklisted"
        );
    }

    #[test]
    fn failover_integration_success_records() {
        let ctx = OrchestrationContext::new();
        let failover = ctx.failover();

        // Record successes — failover should NOT blacklist
        for _ in 0..5 {
            ctx.record_model_execution("model-e", true, 30);
        }

        assert!(
            !failover.is_blacklisted("model-e"),
            "successful model should not be blacklisted"
        );
    }

    // ── Concurrent record_model_execution — no data race ──────────────

    #[test]
    fn concurrent_record_model_execution_no_race() {
        use std::sync::Arc;
        use std::thread;

        let ctx = Arc::new(OrchestrationContext::new());
        let mut handles = Vec::new();

        for i in 0..4 {
            let ctx_clone = Arc::clone(&ctx);
            handles.push(thread::spawn(move || {
                for j in 0..25 {
                    let model = format!("concurrent-model-{}-{}", i, j);
                    ctx_clone.record_model_execution(&model, j % 2 == 0, (j as u64) * 5);
                }
            }));
        }

        for handle in handles {
            handle.join().expect("thread should not panic");
        }

        // After 100 concurrent calls, the feed should still be queryable
        let _rate = ctx
            .performance_feed()
            .get_success_rate("concurrent-model-0-0");
    }

    #[test]
    fn concurrent_record_model_execution_same_model_no_race() {
        use std::sync::Arc;
        use std::thread;

        let ctx = Arc::new(OrchestrationContext::new());
        let mut handles = Vec::new();

        // All threads write to the same model — stress-test internal locking
        for _ in 0..8 {
            let ctx_clone = Arc::clone(&ctx);
            handles.push(thread::spawn(move || {
                for j in 0..50 {
                    ctx_clone.record_model_execution("hot-model", j % 2 == 0, (j as u64) % 100);
                }
            }));
        }

        for handle in handles {
            handle.join().expect("thread should not panic");
        }

        // After concurrent writes, the feed should have a success rate
        let rate = ctx.performance_feed().get_success_rate("hot-model");
        assert!(rate.is_some());
        assert_eq!(ctx.performance_feed().get_request_count("hot-model"), 400);
    }
}
