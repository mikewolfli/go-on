use crate::observability::performance::PerformanceMetrics;

pub(crate) fn push_metric_header(
    lines: &mut Vec<String>,
    name: &str,
    metric_type: &str,
    help: &str,
) {
    lines.push(format!("# HELP {} {}", name, help));
    lines.push(format!("# TYPE {} {}", name, metric_type));
}

// F-GAP-49: Reserved for future metrics reporting — not yet wired into the hot path.
// `push_scalar_metric` is a convenience wrapper around `push_metric_header` + value line.
// Once a consumer calls it from the Prometheus endpoint builder, remove this annotation.
#[allow(dead_code)] // F-GAP-49 — reserved observability feature
pub(crate) fn push_scalar_metric(
    lines: &mut Vec<String>,
    name: &str,
    metric_type: &str,
    help: &str,
    value: impl std::fmt::Display,
) {
    push_metric_header(lines, name, metric_type, help);
    lines.push(format!("{} {}", name, value));
}

/// Bridge ACP RuntimeMetrics into the observability-layer PerformanceMetrics.
///
/// Reads the current snapshot from the ACP runtime metrics system and converts
/// it to the global performance metrics format. This enables bidirectional
/// metric flow between the `RuntimeMetrics` (ACP) and `AppMetrics` / global
/// performance monitor (observability) systems.
///
/// # Example
///
/// ```ignore
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
