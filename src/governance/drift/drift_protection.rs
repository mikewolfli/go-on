//! F-GAP-26: Drift Protection
//!
//! Detects and prevents goal drift, capability drift, and behavioral drift
//! by comparing measured metrics against established baselines and evaluating
//! deviation against configured policy thresholds.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::i18n::runtime::tf;

/// Categories of drift that the system monitors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub enum DriftType {
    /// Deviation from strategic goals or objectives.
    Goal,
    /// Deviation in system capabilities or feature set.
    Capability,
    /// Deviation in agent or user behaviour patterns.
    Behavioral,
    /// Deviation in performance metrics (latency, throughput, etc.).
    Performance,
    /// Deviation in operational context or environment.
    Context,
}

/// Severity level assigned to a detected drift alert.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
pub enum DriftSeverity {
    /// Informational notice – drift is present but within acceptable bounds.
    Notice,
    /// Warning – drift exceeds the warning threshold and should be reviewed.
    Warning,
    /// Critical – drift exceeds the critical threshold and requires attention.
    Critical,
    /// Breach – drift exceeds the breach threshold; policy violation.
    Breach,
}

/// A single measured metric with its baseline and computed deviation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriftMetric {
    /// Human-readable name identifying this metric.
    pub name: String,
    /// The value measured most recently.
    pub current_value: f64,
    /// The expected or reference value.
    pub baseline_value: f64,
    /// Computed deviation from baseline (normalised).
    pub deviation: f64,
    /// The category of drift this metric belongs to.
    pub drift_type: DriftType,
    /// Timestamp (milliseconds since epoch) when the measurement was taken.
    pub measured_ms: u64,
}

/// An alert raised when a drift metric exceeds one of the policy thresholds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriftAlert {
    /// Unique identifier for this alert.
    pub id: String,
    /// Name of the metric that triggered the alert.
    pub metric_name: String,
    /// Category of drift.
    pub drift_type: DriftType,
    /// Severity assigned based on the threshold that was exceeded.
    pub severity: DriftSeverity,
    /// The deviation value that caused the alert.
    pub deviation: f64,
    /// Human-readable description of the alert.
    pub message: String,
    /// Timestamp (milliseconds since epoch) when the alert was triggered.
    pub triggered_ms: u64,
    /// Whether the alert has been resolved.
    pub resolved: bool,
    /// Timestamp (milliseconds since epoch) when the alert was resolved, if applicable.
    pub resolved_ms: Option<u64>,
}

/// A policy that defines acceptable drift thresholds for one or more drift types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriftPolicy {
    /// Name of this policy.
    pub name: String,
    /// The drift types this policy applies to.
    pub drift_types: Vec<DriftType>,
    /// Deviation threshold for issuing a warning (0.0 – 1.0).
    pub warning_threshold: f64,
    /// Deviation threshold for issuing a critical alert (0.0 – 1.0).
    pub critical_threshold: f64,
    /// Deviation threshold for declaring a breach (0.0 – 1.0).
    pub breach_threshold: f64,
    /// Minimum time in milliseconds between repeated alerts for the same metric.
    pub cooldown_ms: u64,
    /// Whether the system should attempt automatic remediation when this policy is breached.
    pub auto_remediate: bool,
}

/// A snapshot summary of the drift protection engine's current state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriftProfile {
    /// Total number of metrics being tracked.
    pub total_metrics: usize,
    /// Number of currently active (unresolved) alerts.
    pub active_alerts: usize,
    /// Number of active alerts at Critical severity or above.
    pub critical_alerts: usize,
    /// The highest severity among all active alerts, if any.
    pub highest_severity: Option<DriftSeverity>,
    /// Timestamp (milliseconds since epoch) of the last drift check.
    pub last_check_ms: u64,
}

/// Configuration for the drift protection engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriftProtectionConfig {
    /// Interval in milliseconds between automatic drift checks.
    #[serde(default = "default_check_interval")]
    pub check_interval_ms: u64,
    /// Maximum number of alerts to retain in the engine.
    #[serde(default = "default_max_alerts")]
    pub max_alerts: usize,
    /// Time in milliseconds after which an unresolved alert is auto-resolved.
    #[serde(default = "default_auto_resolve")]
    pub auto_resolve_after_ms: u64,
}

fn default_check_interval() -> u64 {
    60_000
}

fn default_max_alerts() -> usize {
    100
}

fn default_auto_resolve() -> u64 {
    3_600_000
}

impl Default for DriftProtectionConfig {
    fn default() -> Self {
        Self {
            check_interval_ms: default_check_interval(),
            max_alerts: default_max_alerts(),
            auto_resolve_after_ms: default_auto_resolve(),
        }
    }
}

/// Thread-safe engine that monitors metrics, evaluates drift against policies,
/// and produces alerts when deviation thresholds are exceeded.
#[derive(Debug)]
pub struct DriftProtectionEngine {
    config: DriftProtectionConfig,
    policies: Mutex<HashMap<String, DriftPolicy>>,
    metrics: Mutex<HashMap<String, DriftMetric>>,
    /// Historical metrics grouped by drift type for trend analysis.
    metric_history: Mutex<HashMap<DriftType, Vec<DriftMetric>>>,
    alerts: Mutex<Vec<DriftAlert>>,
    alert_counter: Mutex<u64>,
}

impl DriftProtectionEngine {
    /// Creates a new drift protection engine with the given configuration.
    pub fn new(config: DriftProtectionConfig) -> Self {
        Self {
            config,
            policies: Mutex::new(HashMap::new()),
            metrics: Mutex::new(HashMap::new()),
            metric_history: Mutex::new(HashMap::new()),
            alerts: Mutex::new(Vec::new()),
            alert_counter: Mutex::new(0),
        }
    }

    /// Registers a drift policy. Returns an error if a policy with the same name already exists.
    pub fn register_policy(&self, policy: DriftPolicy) -> Result<()> {
        let mut policies = self
            .policies
            .lock()
            .map_err(|e| anyhow::anyhow!("failed to lock policies: {}", e))?;
        if policies.contains_key(&policy.name) {
            bail!(tf(
                "error.policy_already_registered",
                &[("name", &policy.name)]
            ));
        }
        policies.insert(policy.name.clone(), policy);
        Ok(())
    }

    /// Records a metric measurement. If a metric with the same name already exists
    /// for the given drift type, it is updated with the new value; otherwise a new entry
    /// is created.
    pub fn record_metric(
        &self,
        name: &str,
        current_value: f64,
        baseline_value: f64,
        drift_type: DriftType,
    ) -> Result<()> {
        let deviation = compute_deviation(current_value, baseline_value);
        let now_ms = current_time_ms();
        let drift_type_for_history = drift_type.clone();
        let metric = DriftMetric {
            name: name.to_string(),
            current_value,
            baseline_value,
            deviation,
            drift_type,
            measured_ms: now_ms,
        };
        let mut metrics = self
            .metrics
            .lock()
            .map_err(|e| anyhow::anyhow!("failed to lock metrics: {}", e))?;
        metrics.insert(name.to_string(), metric.clone());

        // Track history for trend analysis (keep last 100 entries per type)
        if let Ok(mut history) = self.metric_history.lock() {
            let entry = history.entry(drift_type_for_history).or_default();
            entry.push(metric);
            if entry.len() > 100 {
                entry.remove(0);
            }
        }

        Ok(())
    }

    /// Evaluates all recorded metrics against registered policies and returns
    /// any newly triggered alerts. Previously triggered alerts that should be
    /// auto-resolved based on time-out are resolved first.
    pub fn check_for_drift(&self) -> Vec<DriftAlert> {
        let now_ms = current_time_ms();
        let config = &self.config;

        // Auto-resolve stale alerts before checking again.
        if let Ok(mut alerts) = self.alerts.lock() {
            for alert in alerts.iter_mut() {
                if !alert.resolved
                    && now_ms.saturating_sub(alert.triggered_ms) >= config.auto_resolve_after_ms
                {
                    alert.resolved = true;
                    alert.resolved_ms = Some(now_ms);
                }
            }
        }

        let policies = match self.policies.lock() {
            Ok(p) => p.clone(),
            Err(_) => return Vec::new(),
        };
        let metrics = match self.metrics.lock() {
            Ok(m) => m.clone(),
            Err(_) => return Vec::new(),
        };
        let mut alerts = match self.alerts.lock() {
            Ok(a) => a,
            Err(_) => return Vec::new(),
        };

        let mut new_alerts: Vec<DriftAlert> = Vec::new();

        for metric in metrics.values() {
            for policy in policies.values() {
                if !policy.drift_types.contains(&metric.drift_type) {
                    continue;
                }

                // Determine severity based on thresholds.
                let severity = if metric.deviation >= policy.breach_threshold {
                    DriftSeverity::Breach
                } else if metric.deviation >= policy.critical_threshold {
                    DriftSeverity::Critical
                } else if metric.deviation >= policy.warning_threshold {
                    DriftSeverity::Warning
                } else {
                    continue; // within acceptable bounds, no alert
                };

                // Check cooldown: avoid duplicate alerts for the same metric within cooldown period.
                let within_cooldown = alerts.iter().any(|a| {
                    a.metric_name == metric.name
                        && a.drift_type == metric.drift_type
                        && a.severity == severity
                        && !a.resolved
                        && now_ms.saturating_sub(a.triggered_ms) < policy.cooldown_ms
                });
                if within_cooldown {
                    continue;
                }

                let alert_id = format!("drift-{}", {
                    let mut ctr = match self.alert_counter.lock() {
                        Ok(c) => c,
                        Err(_) => continue,
                    };
                    *ctr += 1;
                    *ctr
                });

                let message = format!(
                    "{} drift detected in '{}': deviation {:.4} exceeds {} threshold ({:.2})",
                    match severity {
                        DriftSeverity::Breach => "Breach",
                        DriftSeverity::Critical => "Critical",
                        DriftSeverity::Warning => "Warning",
                        DriftSeverity::Notice => "Notice",
                    },
                    metric.name,
                    metric.deviation,
                    match severity {
                        DriftSeverity::Breach => "breach",
                        DriftSeverity::Critical => "critical",
                        DriftSeverity::Warning => "warning",
                        DriftSeverity::Notice => "notice",
                    },
                    match severity {
                        DriftSeverity::Breach => policy.breach_threshold,
                        DriftSeverity::Critical => policy.critical_threshold,
                        DriftSeverity::Warning => policy.warning_threshold,
                        DriftSeverity::Notice => 0.0,
                    },
                );

                let alert = DriftAlert {
                    id: alert_id.clone(),
                    metric_name: metric.name.clone(),
                    drift_type: metric.drift_type.clone(),
                    severity: severity.clone(),
                    deviation: metric.deviation,
                    message,
                    triggered_ms: now_ms,
                    resolved: false,
                    resolved_ms: None,
                };

                new_alerts.push(alert.clone());

                // Enforce alert capacity.
                alerts.push(alert);
                if alerts.len() > config.max_alerts {
                    alerts.remove(0);
                }
            }
        }

        new_alerts
    }

    /// Marks an alert as resolved by its ID. Returns an error if no alert with that ID exists.
    pub fn resolve_alert(&self, alert_id: &str) -> Result<()> {
        let mut alerts = self
            .alerts
            .lock()
            .map_err(|e| anyhow::anyhow!("failed to lock alerts: {}", e))?;
        let alert = alerts
            .iter_mut()
            .find(|a| a.id == alert_id)
            .ok_or_else(|| {
                anyhow::anyhow!(tf("error.alert_not_found", &[("alert_id", alert_id)]))
            })?;
        alert.resolved = true;
        alert.resolved_ms = Some(current_time_ms());
        Ok(())
    }

    /// Returns a list of all alerts that are currently unresolved.
    pub fn get_active_alerts(&self) -> Vec<DriftAlert> {
        match self.alerts.lock() {
            Ok(alerts) => alerts.iter().filter(|a| !a.resolved).cloned().collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Returns all alerts (resolved and unresolved) filtered by severity.
    pub fn get_alerts_by_severity(&self, severity: DriftSeverity) -> Vec<DriftAlert> {
        match self.alerts.lock() {
            Ok(alerts) => alerts
                .iter()
                .filter(|a| a.severity == severity)
                .cloned()
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Returns a snapshot profile of the current drift protection state.
    pub fn profile(&self) -> DriftProfile {
        let total_metrics = match self.metrics.lock() {
            Ok(m) => m.len(),
            Err(_) => 0,
        };
        let alerts = match self.alerts.lock() {
            Ok(a) => a.clone(),
            Err(_) => Vec::new(),
        };

        let active_alerts = alerts.iter().filter(|a| !a.resolved).count();
        let critical_alerts = alerts
            .iter()
            .filter(|a| !a.resolved && a.severity >= DriftSeverity::Critical)
            .count();
        let highest_severity = alerts
            .iter()
            .filter(|a| !a.resolved)
            .map(|a| a.severity.clone())
            .max();

        DriftProfile {
            total_metrics,
            active_alerts,
            critical_alerts,
            highest_severity,
            last_check_ms: current_time_ms(),
        }
    }

    /// Analyze drift metrics over time to detect rising trends.
    /// Returns a list of drift types that show a statistically significant upward trend.
    pub fn detect_trends(&self) -> Vec<(DriftType, f64, String)> {
        let history = match self.metric_history.lock() {
            Ok(h) => h.clone(),
            Err(_) => return Vec::new(),
        };
        let mut trends = Vec::new();

        for (drift_type, metrics) in &history {
            if metrics.len() >= 5 {
                // Simple linear regression slope
                let n = metrics.len() as f64;
                let indices: Vec<f64> = (0..metrics.len()).map(|i| i as f64).collect();
                let values: Vec<f64> = metrics.iter().map(|m| m.deviation).collect();

                let sum_x: f64 = indices.iter().sum();
                let sum_y: f64 = values.iter().sum();
                let sum_xy: f64 = indices.iter().zip(values.iter()).map(|(x, y)| x * y).sum();
                let sum_xx: f64 = indices.iter().map(|x| x * x).sum();

                let denominator = n * sum_xx - sum_x * sum_x;
                if denominator.abs() < f64::EPSILON {
                    continue;
                }
                let slope = (n * sum_xy - sum_x * sum_y) / denominator;

                if slope > 0.05 {
                    let severity = if slope > 0.2 {
                        "critical"
                    } else if slope > 0.1 {
                        "warning"
                    } else {
                        "notice"
                    };
                    trends.push((
                        drift_type.clone(),
                        slope,
                        format!(
                            "Rising trend detected in {:?} (slope: {:.4}, severity: {})",
                            drift_type, slope, severity
                        ),
                    ));
                }
            }
        }

        trends
    }

    /// Generate auto-remediation suggestions for detected drifts.
    pub fn suggest_remediation(&self) -> Vec<String> {
        let alerts = match self.alerts.lock() {
            Ok(a) => a.clone(),
            Err(_) => return Vec::new(),
        };
        let mut suggestions = Vec::new();

        for alert in &alerts {
            if alert.resolved {
                continue;
            }
            match alert.drift_type {
                DriftType::Goal => {
                    suggestions.push(format!(
                        "Realign goal '{}': current deviation {:.2} exceeds threshold",
                        alert.metric_name, alert.deviation
                    ));
                }
                DriftType::Performance => {
                    suggestions.push(format!(
                        "Performance drift detected in '{}': consider scaling resources or optimizing",
                        alert.metric_name
                    ));
                }
                DriftType::Behavioral => {
                    suggestions.push(format!(
                        "Behavioral drift in '{}': review agent configuration or retrain",
                        alert.metric_name
                    ));
                }
                _ => {
                    suggestions.push(format!(
                        "Drift detected in {:?} metric '{}': recommend investigation",
                        alert.drift_type, alert.metric_name
                    ));
                }
            }
        }

        suggestions
    }
}

/// Computes the normalised deviation: |current - baseline| / max(baseline, 0.01).
fn compute_deviation(current: f64, baseline: f64) -> f64 {
    let denominator = if baseline.abs() < 0.01 {
        0.01
    } else {
        baseline.abs()
    };
    (current - baseline).abs() / denominator
}

/// Returns the current time in milliseconds since the Unix epoch.
fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_policy() -> DriftPolicy {
        DriftPolicy {
            name: "default".to_string(),
            drift_types: vec![
                DriftType::Goal,
                DriftType::Capability,
                DriftType::Behavioral,
                DriftType::Performance,
                DriftType::Context,
            ],
            warning_threshold: 0.10,
            critical_threshold: 0.25,
            breach_threshold: 0.50,
            cooldown_ms: 10_000,
            auto_remediate: false,
        }
    }

    fn make_engine() -> DriftProtectionEngine {
        DriftProtectionEngine::new(DriftProtectionConfig::default())
    }

    // ------------------------------------------------------------------
    // 1. New engine is empty
    // ------------------------------------------------------------------
    #[test]
    fn test_new_engine_empty() {
        let engine = make_engine();
        let profile = engine.profile();
        assert_eq!(profile.total_metrics, 0);
        assert_eq!(profile.active_alerts, 0);
        assert!(profile.highest_severity.is_none());
        assert!(engine.get_active_alerts().is_empty());
    }

    // ------------------------------------------------------------------
    // 2. Register a policy
    // ------------------------------------------------------------------
    #[test]
    fn test_register_policy() {
        let engine = make_engine();
        let policy = default_policy();
        assert!(engine.register_policy(policy).is_ok());
    }

    // ------------------------------------------------------------------
    // 3. Registering a duplicate policy fails
    // ------------------------------------------------------------------
    #[test]
    fn test_register_duplicate_policy_fails() {
        let engine = make_engine();
        let policy = default_policy();
        assert!(engine.register_policy(policy.clone()).is_ok());
        let result = engine.register_policy(policy);
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(
            err.to_string().contains("error.policy_already_registered")
                || err.to_string().contains("already registered")
        );
    }

    // ------------------------------------------------------------------
    // 4. Record a metric
    // ------------------------------------------------------------------
    #[test]
    fn test_record_metric() {
        let engine = make_engine();
        assert!(engine
            .record_metric("test_metric", 0.5, 0.4, DriftType::Performance)
            .is_ok());
        let profile = engine.profile();
        assert_eq!(profile.total_metrics, 1);
    }

    // ------------------------------------------------------------------
    // 5. No drift when values are close to baseline
    // ------------------------------------------------------------------
    #[test]
    fn test_check_for_drift_no_drift() {
        let engine = make_engine();
        engine
            .register_policy(default_policy())
            .expect("register policy should succeed");
        engine
            .record_metric("latency", 0.105, 0.10, DriftType::Performance)
            .expect("record metric should succeed");
        // deviation = |0.105 - 0.10| / 0.10 = 0.05 — below warning threshold of 0.10
        let alerts = engine.check_for_drift();
        assert!(alerts.is_empty());
    }

    // ------------------------------------------------------------------
    // 6. Warning threshold is exceeded
    // ------------------------------------------------------------------
    #[test]
    fn test_check_for_drift_triggers_warning() {
        let engine = make_engine();
        engine
            .register_policy(default_policy())
            .expect("register policy should succeed");
        engine
            .record_metric("latency", 0.115, 0.10, DriftType::Performance)
            .expect("record metric should succeed");
        // deviation = |0.115 - 0.10| / 0.10 = 0.15 — above warning (0.10), below critical (0.25)
        let alerts = engine.check_for_drift();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].severity, DriftSeverity::Warning);
        assert_eq!(alerts[0].metric_name, "latency");
    }

    // ------------------------------------------------------------------
    // 7. Critical threshold is exceeded
    // ------------------------------------------------------------------
    #[test]
    fn test_check_for_drift_triggers_critical() {
        let engine = make_engine();
        engine
            .register_policy(default_policy())
            .expect("register policy should succeed");
        engine
            .record_metric("throughput", 0.65, 0.50, DriftType::Performance)
            .expect("record metric should succeed");
        // deviation = |0.65 - 0.50| / 0.50 = 0.30 — above critical (0.25), below breach (0.50)
        let alerts = engine.check_for_drift();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].severity, DriftSeverity::Critical);
    }

    // ------------------------------------------------------------------
    // 8. Breach threshold is exceeded
    // ------------------------------------------------------------------
    #[test]
    fn test_check_for_drift_triggers_breach() {
        let engine = make_engine();
        engine
            .register_policy(default_policy())
            .expect("register policy should succeed");
        engine
            .record_metric("completion_rate", 0.20, 0.10, DriftType::Goal)
            .expect("record metric should succeed");
        // deviation = |0.20 - 0.10| / 0.10 = 1.00 — above breach (0.50)
        let alerts = engine.check_for_drift();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].severity, DriftSeverity::Breach);
    }

    // ------------------------------------------------------------------
    // 9. Resolve an alert
    // ------------------------------------------------------------------
    #[test]
    fn test_resolve_alert() {
        let engine = make_engine();
        engine
            .register_policy(default_policy())
            .expect("register policy should succeed");
        engine
            .record_metric("accuracy", 1.00, 0.50, DriftType::Goal)
            .expect("record metric should succeed");
        let alerts = engine.check_for_drift();
        assert_eq!(alerts.len(), 1);
        let alert_id = alerts[0].id.clone();

        assert!(engine.resolve_alert(&alert_id).is_ok());

        // Verify it no longer appears in active alerts.
        let active = engine.get_active_alerts();
        assert!(active.iter().all(|a| a.id != alert_id));
    }

    // ------------------------------------------------------------------
    // 10. Resolving a nonexistent alert fails
    // ------------------------------------------------------------------
    #[test]
    fn test_resolve_nonexistent_alert_fails() {
        let engine = make_engine();
        let result = engine.resolve_alert("nonexistent-alert-id");
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(
            err.to_string().contains("error.alert_not_found")
                || err.to_string().contains("not found")
        );
    }

    // ------------------------------------------------------------------
    // 11. get_active_alerts returns only unresolved alerts
    // ------------------------------------------------------------------
    #[test]
    fn test_get_active_alerts() {
        let engine = make_engine();
        engine
            .register_policy(default_policy())
            .expect("register policy should succeed");
        engine
            .record_metric("metric_a", 0.30, 0.10, DriftType::Goal)
            .expect("record metric a should succeed");
        engine
            .record_metric("metric_b", 0.30, 0.10, DriftType::Capability)
            .expect("record metric b should succeed");
        let alerts = engine.check_for_drift();
        assert_eq!(alerts.len(), 2);

        // Resolve one alert.
        engine
            .resolve_alert(&alerts[0].id)
            .expect("resolve first alert should succeed");

        let active = engine.get_active_alerts();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, alerts[1].id);
    }

    // ------------------------------------------------------------------
    // 12. Profile reflects the engine state accurately
    // ------------------------------------------------------------------
    #[test]
    fn test_profile_reflects_state() {
        let engine = make_engine();
        engine
            .register_policy(default_policy())
            .expect("register policy should succeed");

        // Record two metrics; one will trigger a critical alert, the other a warning.
        engine
            .record_metric("critical_metric", 0.60, 0.20, DriftType::Goal)
            .expect("record critical metric should succeed");
        // deviation = |0.60 - 0.20| / 0.20 = 2.00 → breach / critical
        engine
            .record_metric("warning_metric", 0.060, 0.05, DriftType::Context)
            .expect("record warning metric should succeed");
        // deviation = |0.060 - 0.05| / 0.05 = 0.20 → above warning threshold (0.10), below critical (0.25) => warning

        let _alerts = engine.check_for_drift();

        let profile = engine.profile();
        assert_eq!(profile.total_metrics, 2);
        assert_eq!(profile.active_alerts, 2);
        // The breach/critical count should include the first alert since its severity >= Critical.
        assert_eq!(profile.critical_alerts, 1);
        assert_eq!(profile.highest_severity, Some(DriftSeverity::Breach));
    }
}
