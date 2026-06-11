//! Observation phase — trigger source trait and all implementations.
//!
//! Provides the [`TriggerSource`] trait and built-in implementations that
//! detect and emit [`EvolutionTrigger`] values for the evolution loop.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::warn;

use crate::observability::alert_manager::{AlertManager, AlertSeverity};

// ---------------------------------------------------------------------------
// EvolutionTrigger
// ---------------------------------------------------------------------------

/// Describes the reason that triggered an evolution cycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EvolutionTrigger {
    /// A performance metric crossed a threshold in the wrong direction.
    PerformanceRegression {
        /// The metric name (e.g., "latency_p50", "throughput").
        metric: String,
        /// The threshold value that was crossed.
        threshold: f64,
        /// The direction of regression (increasing or decreasing).
        direction: RegressionDirection,
    },
    /// The same error pattern has appeared repeatedly.
    RepeatedError {
        /// The error message pattern.
        pattern: String,
        /// How many times it has been observed.
        count: u64,
    },
    /// Dead code was detected above a certain ratio.
    DeadCodeDetected {
        /// The module where dead code was found.
        module: String,
        /// The ratio of dead code to total code.
        ratio: f64,
    },
    /// A manual evolution request from a user or operator.
    ManualRequest {
        /// Free-form instruction describing what to evolve.
        instruction: String,
    },
    /// Configuration drift detected between expected and actual values.
    ConfigDrift {
        /// The configuration key that drifted.
        key: String,
        /// The expected value.
        expected: String,
        /// The actual value found.
        actual: String,
    },
    /// Capability degradation detected by EvolutionGraph (BLUE56-B10).
    DegradationDetected {
        /// The capability ID that is degrading.
        capability_id: String,
        /// The degradation trend slope (negative = degrading).
        trend_slope: f64,
    },
}

impl EvolutionTrigger {
    /// Returns a human-readable label for this trigger.
    pub fn label(&self) -> &str {
        match self {
            EvolutionTrigger::PerformanceRegression { .. } => "performance_regression",
            EvolutionTrigger::RepeatedError { .. } => "repeated_error",
            EvolutionTrigger::DeadCodeDetected { .. } => "dead_code_detected",
            EvolutionTrigger::ManualRequest { .. } => "manual_request",
            EvolutionTrigger::ConfigDrift { .. } => "config_drift",
            EvolutionTrigger::DegradationDetected { .. } => "degradation_detected",
        }
    }

    /// Returns a short description of the trigger.
    pub fn description(&self) -> String {
        match self {
            EvolutionTrigger::PerformanceRegression {
                metric,
                threshold,
                direction,
            } => {
                format!(
                    "Performance regression: {} {} threshold {}",
                    metric,
                    match direction {
                        RegressionDirection::Increasing => "rose above",
                        RegressionDirection::Decreasing => "fell below",
                    },
                    threshold
                )
            }
            EvolutionTrigger::RepeatedError { pattern, count } => {
                format!("Repeated error ({}x): {}", count, pattern)
            }
            EvolutionTrigger::DeadCodeDetected { module, ratio } => {
                format!("Dead code in {}: {:.1}%", module, ratio * 100.0)
            }
            EvolutionTrigger::ManualRequest { instruction } => {
                format!("Manual: {}", instruction)
            }
            EvolutionTrigger::ConfigDrift {
                key,
                expected,
                actual,
            } => {
                format!(
                    "Config drift: {} expected={} actual={}",
                    key, expected, actual
                )
            }
            EvolutionTrigger::DegradationDetected {
                capability_id,
                trend_slope,
            } => {
                format!(
                    "Capability degradation: {} trend={:.3}",
                    capability_id, trend_slope
                )
            }
        }
    }
}

/// Direction of a regression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RegressionDirection {
    /// The metric is increasing (worse for latency, better for throughput).
    Increasing,
    /// The metric is decreasing (worse for throughput, better for latency).
    Decreasing,
}

// ---------------------------------------------------------------------------
// MetricsSnapshot (re-exported for convenience)
// ---------------------------------------------------------------------------

/// A snapshot of key system metrics at a point in time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    /// Timestamp in milliseconds.
    pub timestamp_ms: u64,
    /// Average request latency in milliseconds.
    pub avg_latency_ms: f64,
    /// Requests per second.
    pub throughput: f64,
    /// Error rate (0.0 – 1.0).
    pub error_rate: f64,
    /// Memory usage in bytes.
    pub memory_bytes: u64,
    /// CPU usage as a fraction (0.0 – 1.0).
    pub cpu_usage: f64,
    /// Number of active goroutines/tasks.
    pub active_tasks: u64,
}

impl MetricsSnapshot {
    /// Create a new metrics snapshot with the current timestamp.
    pub fn new(
        avg_latency_ms: f64,
        throughput: f64,
        error_rate: f64,
        memory_bytes: u64,
        cpu_usage: f64,
        active_tasks: u64,
    ) -> Self {
        Self {
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            avg_latency_ms,
            throughput,
            error_rate,
            memory_bytes,
            cpu_usage,
            active_tasks,
        }
    }

    /// Compute the degradation ratio between this snapshot and another.
    /// Returns a value > 0.2 (20%) if metrics have degraded significantly.
    pub fn degradation_ratio(&self, other: &MetricsSnapshot) -> f64 {
        let mut degradations = Vec::new();

        // Latency: higher is worse
        if other.avg_latency_ms > 0.0 {
            degradations.push((self.avg_latency_ms - other.avg_latency_ms) / other.avg_latency_ms);
        }

        // Throughput: lower is worse
        if other.throughput > 0.0 {
            degradations.push((other.throughput - self.throughput) / other.throughput);
        }

        // Error rate: higher is worse
        if other.error_rate > 0.0 {
            degradations.push((self.error_rate - other.error_rate) / other.error_rate);
        }

        if degradations.is_empty() {
            return 0.0;
        }

        degradations.iter().sum::<f64>() / degradations.len() as f64
    }
}

// ---------------------------------------------------------------------------
// MetricsPoint (for trend analysis)
// ---------------------------------------------------------------------------

/// A single data point for metrics trend analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsPoint {
    /// Timestamp in milliseconds.
    pub timestamp_ms: u64,
    /// Metric value.
    pub value: f64,
    /// Metric label.
    pub label: String,
}

// ---------------------------------------------------------------------------
// TriggerSource trait
// ---------------------------------------------------------------------------

/// A source of evolution triggers that is polled asynchronously.
#[async_trait]
pub trait TriggerSource: Send + Sync + std::fmt::Debug {
    /// Poll for new evolution triggers. Returns a list of triggers that
    /// have been detected since the last poll.
    async fn poll(&self) -> Vec<EvolutionTrigger>;
}

// ---------------------------------------------------------------------------
// MetacognitiveTriggerSource
// ---------------------------------------------------------------------------

/// A trigger source that monitors the system's own cognitive performance
/// (e.g., decision latency, retry rates, planning depth).
#[derive(Debug)]
pub struct MetacognitiveTriggerSource {
    /// Name of this source.
    name: String,
    /// Poll interval.
    interval: Duration,
    /// Thresholds for various metacognitive metrics.
    #[allow(dead_code)]
    // F-GAP-49 — reserved for metacognitive thresholds
    thresholds: HashMap<String, f64>,
}

#[allow(dead_code)]
// F-GAP-49 — reserved for metacognitive trigger source impl
impl MetacognitiveTriggerSource {
    /// Create a new metacognitive trigger source.
    /// TODO-BLUE64: Activate in evolution_loop_builder when metacognitive data is available.
    pub fn new(name: String, interval: Duration) -> Self {
        let mut thresholds = HashMap::new();
        thresholds.insert("decision_latency_ms".to_string(), 5000.0);
        thresholds.insert("retry_rate".to_string(), 0.1);
        thresholds.insert("planning_depth".to_string(), 3.0);
        Self {
            name,
            interval,
            thresholds,
        }
    }

    /// Set a custom threshold for a metric.
    #[allow(dead_code)]
    // F-GAP-49 — reserved for with_threshold
    pub fn with_threshold(mut self, metric: &str, value: f64) -> Self {
        self.thresholds.insert(metric.to_string(), value);
        self
    }
}

#[async_trait]
impl TriggerSource for MetacognitiveTriggerSource {
    async fn poll(&self) -> Vec<EvolutionTrigger> {
        // In a real implementation, this would query the metacognitive
        // monitoring subsystem. For now, return empty — triggers appear
        // only when thresholds are actually crossed.
        let _ = &self.name;
        let _ = &self.interval;
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// AlertManagerTriggerSource
// ---------------------------------------------------------------------------

/// A trigger source that listens to the alert manager for active alerts
/// that should trigger an evolution cycle.
pub struct AlertManagerTriggerSource {
    /// Name of this source.
    name: String,
    /// Reference to the real AlertManager (when connected).
    alert_manager: Option<Arc<StdMutex<AlertManager>>>,
    /// Cached alert fingerprints to avoid re-triggering.
    seen_alerts: tokio::sync::Mutex<Vec<String>>,
}

impl std::fmt::Debug for AlertManagerTriggerSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AlertManagerTriggerSource")
            .field("name", &self.name)
            .field(
                "alert_manager",
                &self.alert_manager.as_ref().map(|_| "<AlertManager>"),
            )
            .field("seen_alerts", &self.seen_alerts)
            .finish()
    }
}

impl AlertManagerTriggerSource {
    /// Create a new alert manager trigger source.
    pub fn new(name: String) -> Self {
        Self {
            name,
            alert_manager: None,
            seen_alerts: tokio::sync::Mutex::new(Vec::new()),
        }
    }

    /// Connect this trigger source to a real AlertManager instance.
    /// When connected, `poll()` will query active alerts and convert
    /// unseen ones into `EvolutionTrigger` values.
    pub fn with_alert_manager(mut self, am: Arc<StdMutex<AlertManager>>) -> Self {
        self.alert_manager = Some(am);
        self
    }
}

#[async_trait]
impl TriggerSource for AlertManagerTriggerSource {
    async fn poll(&self) -> Vec<EvolutionTrigger> {
        let Some(ref am) = self.alert_manager else {
            warn!(
                "AlertManagerTriggerSource[{}]: no AlertManager connected; returning empty",
                self.name
            );
            return Vec::new();
        };

        // Query the real AlertManager for recently fired alerts.
        let recent_alerts = match am.lock() {
            Ok(guard) => guard.get_recent_alerts(),
            Err(poisoned) => {
                warn!(
                    "AlertManagerTriggerSource[{}]: AlertManager lock poisoned",
                    self.name
                );
                poisoned.into_inner().get_recent_alerts()
            }
        };

        if recent_alerts.is_empty() {
            return Vec::new();
        }

        // Fingerprint each alert as "rule:severity:value" to avoid re-triggering.
        let mut seen = self.seen_alerts.lock().await;
        let mut triggers = Vec::new();

        for alert in &recent_alerts {
            let fp = format!(
                "{}:{}:{}",
                alert.rule,
                alert.severity as u8,
                (alert.value * 100.0) as i64
            );
            if seen.contains(&fp) {
                continue;
            }
            seen.push(fp);

            let direction = if alert.value > alert.threshold {
                RegressionDirection::Increasing
            } else {
                RegressionDirection::Decreasing
            };

            match alert.severity {
                AlertSeverity::Critical => {
                    triggers.push(EvolutionTrigger::PerformanceRegression {
                        metric: format!("alert::critical::{}", alert.rule),
                        threshold: alert.threshold,
                        direction,
                    });
                }
                AlertSeverity::Warning => {
                    triggers.push(EvolutionTrigger::PerformanceRegression {
                        metric: format!("alert::warning::{}", alert.rule),
                        threshold: alert.threshold,
                        direction,
                    });
                }
                _ => {
                    // Info-level alerts become manual-request triggers.
                    triggers.push(EvolutionTrigger::ManualRequest {
                        instruction: format!(
                            "Alert '{}': {} (value={}, threshold={})",
                            alert.rule, alert.message, alert.value, alert.threshold
                        ),
                    });
                }
            }
        }

        // Cap seen alerts to prevent unbounded memory growth.
        if seen.len() > 1000 {
            let excess = seen.len() - 500;
            seen.drain(0..excess);
        }

        triggers
    }
}

// ---------------------------------------------------------------------------
// DiagnosticTriggerSource
// ---------------------------------------------------------------------------

/// A trigger source that monitors compiler/LSP diagnostics and test results
/// to detect repeated error patterns.
#[derive(Debug)]
pub struct DiagnosticTriggerSource {
    /// Name of this source.
    name: String,
    /// Map of error patterns to their observed counts.
    error_counts: Arc<tokio::sync::Mutex<HashMap<String, u64>>>,
    /// Minimum count before triggering.
    min_count: u64,
}

#[allow(dead_code)]
// F-GAP-49 — reserved for diagnostic trigger source impl
impl DiagnosticTriggerSource {
    /// Create a new diagnostic trigger source with a shared error-counts map.
    ///
    /// When `external_counts` is `Some`, the source uses that shared map
    /// instead of creating its own, allowing the EvolutionLoop to inject
    /// error patterns from its own pipeline (e.g. verify failures).
    pub fn new(name: String, min_count: u64) -> Self {
        Self {
            name,
            error_counts: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            min_count,
        }
    }

    /// Create a diagnostic trigger source that shares an error-counts map
    /// with an external caller (e.g. the EvolutionLoop).
    pub fn with_shared_counts(
        name: String,
        min_count: u64,
        error_counts: Arc<tokio::sync::Mutex<HashMap<String, u64>>>,
    ) -> Self {
        Self {
            name,
            error_counts,
            min_count,
        }
    }

    /// Returns the inner error-counts map reference for external wiring.
    pub fn inner_counts(&self) -> &Arc<tokio::sync::Mutex<HashMap<String, u64>>> {
        &self.error_counts
    }

    /// Record an observed error pattern.
    ///
    /// Record an observed error pattern so that repeated errors
    /// automatically trigger evolution cycles.
    pub fn record_error(&self, pattern: String) {
        let mut counts = tokio::task::block_in_place(|| self.error_counts.blocking_lock());
        *counts.entry(pattern).or_insert(0) += 1;
    }
}

#[async_trait]
impl TriggerSource for DiagnosticTriggerSource {
    async fn poll(&self) -> Vec<EvolutionTrigger> {
        let mut triggers = Vec::new();
        let mut counts = self.error_counts.lock().await;

        let to_remove: Vec<String> = counts
            .iter()
            .filter(|(_, count)| **count >= self.min_count)
            .map(|(pattern, _)| pattern.clone())
            .collect();

        for pattern in to_remove {
            if let Some(count) = counts.remove(&pattern) {
                triggers.push(EvolutionTrigger::RepeatedError { pattern, count });
            }
        }

        let _ = &self.name;
        triggers
    }
}

// ---------------------------------------------------------------------------
// TickTriggerSource
// ---------------------------------------------------------------------------

/// A simple trigger source that fires at a fixed interval.
///
/// This is the default trigger source that ensures the evolution loop has
/// at least one active source, preventing `NoTriggerSources` errors.
#[allow(dead_code)]
// F-GAP-49 — reserved for tick trigger source
#[derive(Debug)]
pub struct TickTriggerSource {
    /// Name of this source.
    name: String,
    /// Interval between automatic triggers.
    interval: Duration,
    /// Timestamp (ms since epoch) of the last trigger.
    last_trigger_ms: tokio::sync::Mutex<u64>,
}

#[allow(dead_code)]
// F-GAP-49 — reserved for tick trigger source impl
impl TickTriggerSource {
    /// Create a new tick trigger source that fires every `interval`.
    pub fn new(name: String, interval: Duration) -> Self {
        Self {
            name,
            interval,
            last_trigger_ms: tokio::sync::Mutex::new(0),
        }
    }
}

#[async_trait]
impl TriggerSource for TickTriggerSource {
    async fn poll(&self) -> Vec<EvolutionTrigger> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        let mut last = self.last_trigger_ms.lock().await;
        let elapsed_ms = now.saturating_sub(*last);

        if elapsed_ms >= self.interval.as_millis() as u64 {
            *last = now;
            let instruction = format!("Scheduled evolution tick from {}", self.name);
            vec![EvolutionTrigger::ManualRequest { instruction }]
        } else {
            Vec::new()
        }
    }
}

// ---------------------------------------------------------------------------
// ManualTriggerSource
// ---------------------------------------------------------------------------

/// A trigger source that accepts manual evolution requests via a channel.
#[derive(Debug)]
pub struct ManualTriggerSource {
    /// Name of this source.
    name: String,
    /// Receiver for manual trigger requests.
    rx: tokio::sync::Mutex<mpsc::UnboundedReceiver<String>>,
    /// Sender (cloned for external use).
    #[allow(dead_code)]
    // F-GAP-49 — reserved for manual trigger sender
    tx: mpsc::UnboundedSender<String>,
}

#[allow(dead_code)]
// F-GAP-49 — reserved for manual trigger source impl
impl ManualTriggerSource {
    /// Create a new manual trigger source.
    pub fn new(name: String) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            name,
            rx: tokio::sync::Mutex::new(rx),
            tx,
        }
    }

    /// Send a manual evolution request. This is the public API for
    /// submitting manual evolution instructions.
    #[allow(dead_code)]
    pub fn request_evolution(&self, instruction: String) -> Result<(), String> {
        self.tx
            .send(instruction)
            .map_err(|e| format!("Failed to send manual request: {}", e))
    }
}

#[async_trait]
impl TriggerSource for ManualTriggerSource {
    async fn poll(&self) -> Vec<EvolutionTrigger> {
        let mut triggers = Vec::new();
        let mut rx = self.rx.lock().await;

        loop {
            match rx.try_recv() {
                Ok(instruction) => {
                    triggers.push(EvolutionTrigger::ManualRequest { instruction });
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    warn!("ManualTriggerSource channel disconnected");
                    break;
                }
            }
        }

        let _ = &self.name;
        triggers
    }
}

// ---------------------------------------------------------------------------
// PubsubTriggerSource
// ---------------------------------------------------------------------------

/// A trigger source that reads evolution triggers from a `mpsc` channel.
///
/// This bridges the TripleFusion bridge (or any other in-process producer)
/// into the EvolutionLoop without coupling the two subsystems directly.
#[derive(Debug)]
pub struct PubsubTriggerSource {
    /// Name of this source.
    name: String,
    /// Receiver end of the mpsc channel.
    rx: tokio::sync::Mutex<mpsc::UnboundedReceiver<EvolutionTrigger>>,
}

impl PubsubTriggerSource {
    /// Create a new pubsub trigger source.
    pub fn new(name: String, rx: mpsc::UnboundedReceiver<EvolutionTrigger>) -> Self {
        Self {
            name,
            rx: tokio::sync::Mutex::new(rx),
        }
    }
}

#[async_trait]
impl TriggerSource for PubsubTriggerSource {
    async fn poll(&self) -> Vec<EvolutionTrigger> {
        let mut triggers = Vec::new();
        let mut rx = self.rx.lock().await;

        loop {
            match rx.try_recv() {
                Ok(trigger) => {
                    triggers.push(trigger);
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    warn!("PubsubTriggerSource channel disconnected");
                    break;
                }
            }
        }

        let _ = &self.name;
        triggers
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_evolution_trigger_label() {
        let t = EvolutionTrigger::ManualRequest {
            instruction: "fix bug".to_string(),
        };
        assert_eq!(t.label(), "manual_request");
    }

    #[test]
    fn test_evolution_trigger_description() {
        let t = EvolutionTrigger::DeadCodeDetected {
            module: "core".to_string(),
            ratio: 0.15,
        };
        assert!(t.description().contains("15.0%"));
    }

    #[test]
    fn test_metrics_snapshot_degradation() {
        let before = MetricsSnapshot::new(100.0, 1000.0, 0.01, 1_000_000, 0.5, 10);
        let after = MetricsSnapshot::new(500.0, 200.0, 0.10, 2_000_000, 0.8, 20);
        let ratio = after.degradation_ratio(&before);
        assert!(ratio > 0.2);
    }

    #[test]
    fn test_metrics_snapshot_no_degradation() {
        let before = MetricsSnapshot::new(100.0, 1000.0, 0.01, 1_000_000, 0.5, 10);
        let after = MetricsSnapshot::new(90.0, 1100.0, 0.005, 900_000, 0.4, 9);
        let ratio = after.degradation_ratio(&before);
        assert!(ratio < 0.0);
    }

    #[test]
    fn test_manual_trigger_source() {
        let source = ManualTriggerSource::new("test".to_string());
        let result = source.request_evolution("fix lint warnings".to_string());
        assert!(result.is_ok());
    }

    #[test]
    fn test_diagnostic_trigger_source() {
        let source = DiagnosticTriggerSource::new("test".to_string(), 3);
        source.record_error("E0308".to_string());
        source.record_error("E0308".to_string());
        source.record_error("E0308".to_string());
        // After 3 recordings, the next poll should return a trigger
    }

    #[test]
    fn test_regression_direction() {
        assert_eq!(
            format!("{:?}", RegressionDirection::Increasing),
            "Increasing"
        );
        assert_eq!(
            format!("{:?}", RegressionDirection::Decreasing),
            "Decreasing"
        );
    }
}
