//! ACP Prelude - Type definitions and constants
//!
//! This module contains type definitions, constants, and basic structures
//! used throughout the ACP system. It serves as the foundation for the
//! modular ACP implementation.
//!
//! # Sub-modules
//!
//! | Module | Contents |
//! |---|---|
//! | `constants` | Magic numbers, default values, lock names |
//! | `types` | Shared data types (conversation, review, server status) |
//! | `functions` | Utility functions (timestamps, conversation management) |
//! | `lock_helpers` | Lock acquisition helpers with poison recovery |
//! | `circuit_breaker` | Circuit breaker registry and snapshots |
//! | `lifecycle` | Server lifecycle state |
//! | `maintenance` | Maintenance tracker |
//! | `rate_limiter` | Phase-level token bucket rate limiter |
//! | `inflight` | In-flight request concurrency limiter |
//! | `runtime_metrics` | Server performance metrics |
//! | `re_exports` | Re-exports from other crate modules |

pub mod circuit_breaker;
pub mod constants;
pub mod functions;
pub mod inflight;
pub mod lifecycle;
pub mod lock_helpers;
pub mod maintenance;
pub mod rate_limiter;
pub mod re_exports;
pub mod runtime_metrics;
pub mod types;

// Re-export all public items from sub-modules.
//
// This ensures `use crate::acp::prelude::*` continues to work as before.
pub use circuit_breaker::*;
pub use constants::*;
pub use functions::*;
pub use inflight::*;
pub use lifecycle::*;
pub use lock_helpers::*;
pub use maintenance::*;
pub use rate_limiter::*;
pub use re_exports::*;
pub use runtime_metrics::*;
pub use types::*;

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex as StdMutex};

    use super::{with_acp_lock, PhaseRateLimiter, RuntimeMetrics};

    #[test]
    fn runtime_metrics_records_request_latency_and_outcomes() {
        let metrics = RuntimeMetrics::new();
        metrics.record_request_outcome(true, 12.0);
        metrics.record_request_outcome(false, 24.0);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.total_requests, 2);
        assert_eq!(snapshot.successful_requests, 1);
        assert_eq!(snapshot.failed_requests, 1);
        assert_eq!(snapshot.request_latency_sum_ms, 36.0);
        assert_eq!(snapshot.avg_request_duration_ms, 18.0);
        assert_eq!(snapshot.request_latency_bucket_counts[3], 2); // <= 50ms
    }

    #[test]
    fn runtime_metrics_records_chat_and_review_latency_buckets() {
        let metrics = RuntimeMetrics::new();
        metrics.record_chat_latency(3.0);
        metrics.record_review_latency(5001.0);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.chat_requests_total, 1);
        assert_eq!(snapshot.chat_latency_sum_ms, 3.0);
        assert_eq!(snapshot.chat_latency_bucket_counts[1], 1); // <= 5ms
        assert_eq!(snapshot.review_latency_sum_ms, 5001.0);
        assert_eq!(snapshot.review_latency_bucket_counts[8], 1); // <= 10000ms
    }

    #[test]
    fn runtime_metrics_record_vector_and_summary_counters() {
        let metrics = RuntimeMetrics::new();
        metrics.record_vector_search(2);
        metrics.record_vector_store();
        metrics.record_summary_read(true);
        metrics.record_summary_read(false);
        metrics.record_summary_store();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.vector_search_total, 1);
        assert_eq!(snapshot.vector_hit_total, 2);
        assert_eq!(snapshot.vector_store_total, 1);
        assert_eq!(snapshot.summary_read_total, 2);
        assert_eq!(snapshot.summary_hit_total, 1);
        assert_eq!(snapshot.summary_store_total, 1);
    }

    #[test]
    fn runtime_metrics_tracks_agent_and_probe_timeouts() {
        let metrics = RuntimeMetrics::new();
        metrics.inc_agent_timeout_failure();
        metrics.inc_agent_timeout_failure();
        metrics.inc_runtime_probe_timeout();

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.agent_timeout_failures_total, 2);
        assert_eq!(snapshot.runtime_probe_timeout_total, 1);
    }

    #[test]
    fn with_acp_lock_recovers_poisoned_mutex() {
        let shared: Arc<StdMutex<PhaseRateLimiter>> =
            Arc::new(StdMutex::new(PhaseRateLimiter::default()));

        let poison_target = Arc::clone(&shared);
        let join = std::thread::spawn(move || {
            let _guard = poison_target.lock().expect("lock should be acquired");
            panic!("poison the lock");
        })
        .join();
        assert!(join.is_err(), "poisoning thread should panic");

        let tracked_before = with_acp_lock(
            "test_lock",
            shared.as_ref(),
            |guard: &mut PhaseRateLimiter| {
                let _ = guard.allow("entry:test", 60, Some(5));
                guard.tracked_phases()
            },
        );
        assert_eq!(tracked_before, 1);
    }
}
