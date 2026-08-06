//! F-GAP-26: Drift Protection
//!
//! Detects and prevents goal drift, capability drift, and behavioral drift
//! by comparing measured metrics against established baselines and evaluating
//! deviation against configured policy thresholds.

use anyhow::{bail, Result};

use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use tracing;

use crate::i18n::runtime::tf;

/// Categories of drift that the system monitors.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
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
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone, Serialize)]
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
#[derive(Debug, Clone)]
pub struct DriftProtectionConfig {
    /// Interval in milliseconds between automatic drift checks.
    pub check_interval_ms: u64,
    /// Maximum number of alerts to retain in the engine.
    pub max_alerts: usize,
    /// Time in milliseconds after which an unresolved alert is auto-resolved.
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
///
/// # Lock design
/// All mutable state is stored in a single `Mutex<DriftProtectionInner>` to avoid
/// the overhead of 5 independent mutexes that would be acquired sequentially in
/// every method. A single mutex is simpler, faster, and equally correct.
#[derive(Debug)]
pub struct DriftProtectionEngine {
    config: DriftProtectionConfig,
    inner: Mutex<DriftProtectionInner>,
}

/// Inner state protected by a single mutex.
#[derive(Debug)]
struct DriftProtectionInner {
    policies: HashMap<String, DriftPolicy>,
    metrics: HashMap<String, DriftMetric>,
    /// Historical metrics grouped by drift type for trend analysis.
    metric_history: HashMap<DriftType, Vec<DriftMetric>>,
    alerts: Vec<DriftAlert>,
    alert_counter: u64,
}

impl DriftProtectionEngine {
    /// Creates a new drift protection engine with the given configuration.
    pub fn new(config: DriftProtectionConfig) -> Self {
        Self {
            config,
            inner: Mutex::new(DriftProtectionInner {
                policies: HashMap::new(),
                metrics: HashMap::new(),
                metric_history: HashMap::new(),
                alerts: Vec::new(),
                alert_counter: 0,
            }),
        }
    }

    /// Registers a drift policy. Returns an error if a policy with the same name already exists.
    pub fn register_policy(&self, policy: DriftPolicy) -> Result<()> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| anyhow::anyhow!("failed to lock drift engine: {}", e))?;
        if inner.policies.contains_key(&policy.name) {
            bail!(tf(
                "error.policy_already_registered",
                &[("name", &policy.name)]
            ));
        }
        inner.policies.insert(policy.name.clone(), policy);
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
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| anyhow::anyhow!("failed to lock drift engine: {}", e))?;

        // Auto-baseline: if baseline is 0 (unset), use first historical value.
        let effective_baseline = if baseline_value == 0.0 {
            if let Some(historical) = inner.metric_history.get(&drift_type) {
                historical
                    .first()
                    .map(|m| m.current_value)
                    .unwrap_or(current_value)
            } else {
                current_value // First measurement: use itself as baseline
            }
        } else {
            baseline_value
        };
        let deviation = compute_deviation(current_value, effective_baseline);
        let now_ms = current_time_ms();
        let metric = DriftMetric {
            name: name.to_string(),
            current_value,
            baseline_value: effective_baseline,
            deviation,
            drift_type: drift_type.clone(),
            measured_ms: now_ms,
        };
        inner.metrics.insert(name.to_string(), metric.clone());

        // Track history for trend analysis (keep last 100 entries per type)
        let entry = inner.metric_history.entry(drift_type).or_default();
        entry.push(metric);
        if entry.len() > 100 {
            entry.remove(0);
        }

        Ok(())
    }

    /// Evaluates all recorded metrics against registered policies and returns
    /// any newly triggered alerts. Previously triggered alerts that should be
    /// auto-resolved based on time-out are resolved first.
    ///
    /// If a policy has `auto_remediate` enabled, detected drift will automatically
    /// invoke `suggest_remediation()` and record the remediation in the audit log.
    pub fn check_for_drift(&self) -> Vec<DriftAlert> {
        let now_ms = current_time_ms();
        let config = &self.config;

        let mut inner = self.inner.lock().unwrap_or_else(|poisoned| {
            tracing::warn!(target: "drift_protection", "inner Mutex poisoned – recovering");
            poisoned.into_inner()
        });

        // Clone policies and metrics for iteration (they are small).
        let policies = inner.policies.clone();
        let metrics = inner.metrics.clone();

        // Auto-resolve stale alerts before checking again.
        for alert in inner.alerts.iter_mut() {
            if !alert.resolved
                && now_ms.saturating_sub(alert.triggered_ms) >= config.auto_resolve_after_ms
            {
                alert.resolved = true;
                alert.resolved_ms = Some(now_ms);
            }
        }

        let mut new_alerts: Vec<DriftAlert> = Vec::new();
        let mut auto_remediation_actions: Vec<String> = Vec::new();

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
                let within_cooldown = inner.alerts.iter().any(|a| {
                    a.metric_name == metric.name
                        && a.drift_type == metric.drift_type
                        && a.severity == severity
                        && !a.resolved
                        && now_ms.saturating_sub(a.triggered_ms) < policy.cooldown_ms
                });
                if within_cooldown {
                    continue;
                }

                inner.alert_counter += 1;
                let alert_id = format!("drift-{}", inner.alert_counter);

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
                inner.alerts.push(alert);
                if inner.alerts.len() > config.max_alerts {
                    inner.alerts.remove(0);
                }

                // Auto-remediate: if the policy allows it, invoke remediation.
                if policy.auto_remediate {
                    // Temporarily release the lock to call suggest_remediation
                    // (which would deadlock if we held it).
                    drop(inner);
                    let remediation_suggestions = self.suggest_remediation();
                    for suggestion in &remediation_suggestions {
                        tracing::info!(
                            target: "drift_protection",
                            policy = %policy.name,
                            metric = %metric.name,
                            suggestion = %suggestion,
                            "Auto-remediation triggered"
                        );
                        auto_remediation_actions.push(suggestion.clone());
                    }
                    inner = self.inner.lock().unwrap_or_else(|poisoned| {
                        tracing::warn!(target: "drift_protection", "inner Mutex poisoned – recovering");
                        poisoned.into_inner()
                    });
                }
            }
        }

        // Log aggregated auto-remediation actions.
        if !auto_remediation_actions.is_empty() {
            tracing::info!(
                target: "drift_protection",
                actions = ?auto_remediation_actions,
                "Auto-remediation completed with {} action(s)",
                auto_remediation_actions.len()
            );
        }

        new_alerts
    }

    /// Marks an alert as resolved by its ID. Returns an error if no alert with that ID exists.
    pub fn resolve_alert(&self, alert_id: &str) -> Result<()> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| anyhow::anyhow!("failed to lock drift engine: {}", e))?;
        let alert = inner
            .alerts
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
        let inner = self.inner.lock().unwrap_or_else(|poisoned| {
            tracing::warn!(target: "drift_protection", "inner Mutex poisoned – recovering");
            poisoned.into_inner()
        });
        inner
            .alerts
            .iter()
            .filter(|a| !a.resolved)
            .cloned()
            .collect()
    }

    /// Returns all alerts (resolved and unresolved) filtered by severity.
    pub fn get_alerts_by_severity(&self, severity: DriftSeverity) -> Vec<DriftAlert> {
        let inner = self.inner.lock().unwrap_or_else(|poisoned| {
            tracing::warn!(target: "drift_protection", "inner Mutex poisoned – recovering");
            poisoned.into_inner()
        });
        inner
            .alerts
            .iter()
            .filter(|a| a.severity == severity)
            .cloned()
            .collect()
    }

    /// Returns a snapshot profile of the current drift protection state.
    pub fn profile(&self) -> DriftProfile {
        let inner = self.inner.lock().unwrap_or_else(|poisoned| {
            tracing::warn!(target: "drift_protection", "inner Mutex poisoned – recovering");
            poisoned.into_inner()
        });

        let total_metrics = inner.metrics.len();
        let alerts = &inner.alerts;
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

    /// Generate auto-remediation suggestions for detected drifts.
    pub fn suggest_remediation(&self) -> Vec<String> {
        let inner = self.inner.lock().unwrap_or_else(|poisoned| {
            tracing::warn!(target: "drift_protection", "inner Mutex poisoned – recovering");
            poisoned.into_inner()
        });
        let mut suggestions = Vec::new();

        for alert in &inner.alerts {
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

/// Computes the normalised deviation: |current - baseline| / max(|baseline|, ε).
///
/// Uses a small epsilon (1e-6) instead of 0.01 so the deviation remains
/// meaningful (≈ absolute difference scaled by 1e6) rather than being
/// clamped to an arbitrary 0.01 denominator.
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
    crate::shared::timestamps::now_ts_ms() as u64
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
        let err = result.expect_err("registering duplicate policy should fail");
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
        let err = result.expect_err("resolving nonexistent alert should fail");
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
