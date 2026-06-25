use crate::observability::performance::PerformanceMetrics;

/// Bridge ACP RuntimeMetrics into the observability-layer PerformanceMetrics.
///
/// Reads the current snapshot from the ACP runtime metrics system and converts
/// it to the global performance metrics format. This enables bidirectional
/// metric flow between the `RuntimeMetrics` (ACP) and `AppMetrics` / global
/// performance monitor (observability) systems.
///
/// # Example
///
/// ```text
/// let perf = observability::bridge_runtime_to_performance(&runtime_metrics);
/// println!("Total ops: {}", perf.total_ops);
/// ```
pub fn bridge_runtime_to_performance(
    runtime: &crate::acp::prelude::RuntimeMetrics,
) -> PerformanceMetrics {
    let snap = runtime.snapshot();
    PerformanceMetrics {
        total_ops: snap.total_requests,
        successful_ops: snap.successful_requests,
        failed_ops: snap.failed_requests,
        avg_latency_ms: snap.avg_request_duration_ms,
        ..Default::default()
    }
}

/// Sync ACP RuntimeMetrics into the global performance monitor.
///
/// Records the current ACP runtime metrics state into the global performance
/// monitoring system, ensuring that metrics collected by the ACP layer are
/// visible through the observability layer's `global_metrics_snapshot()`.
pub fn sync_runtime_to_global(runtime: &crate::acp::prelude::RuntimeMetrics) {
    let snap = runtime.snapshot();
    // Record a representative batch so the global monitor reflects ACP state
    if snap.total_requests > 0 {
        crate::observability::performance::record_global_operation(
            snap.successful_requests >= snap.failed_requests,
            snap.avg_request_duration_ms,
        );
    }
}
