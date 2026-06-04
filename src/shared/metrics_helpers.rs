//! Shared metrics formatting helpers.
//!
//! Extracted from `observability::observability` to break the circular
//! dependency: acp → observability → intelligence → acp.
//!
//! These helpers are standalone functions that do not depend on any
//! observability-specific types.

/// Push a Prometheus-style header comment for a metric.
pub fn push_metric_header(lines: &mut Vec<String>, name: &str, metric_type: &str, help: &str) {
    lines.push(format!("# HELP {} {}", name, help));
    lines.push(format!("# TYPE {} {}", name, metric_type));
}

/// Push a Prometheus-style header followed by a single value line.
#[allow(dead_code)]
pub fn push_scalar_metric(
    lines: &mut Vec<String>,
    name: &str,
    metric_type: &str,
    help: &str,
    value: impl std::fmt::Display,
) {
    push_metric_header(lines, name, metric_type, help);
    lines.push(format!("{} {}", name, value));
}
