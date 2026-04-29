//! F-GAP-25: Consciousness Agent Metrics
//!
//! Tracks simulated "consciousness" metrics for agent self-awareness.
//! All mutable state is guarded behind `Arc<Mutex<>>` for thread-safe
//! concurrent access.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

// ── Configuration ───────────────────────────────────────────────────────────

/// Configuration for the consciousness metrics tracker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsciousnessConfig {
    /// Number of recent metrics to consider for awareness calculations.
    #[serde(default = "default_tracking_window")]
    pub tracking_window: usize,
    /// Threshold above which the agent is considered "aware" of a dimension.
    #[serde(default = "default_awareness_threshold")]
    pub awareness_threshold: f64,
    /// Minimum interval (ms) between automatic reflexion cycles.
    #[serde(default = "default_reflexion_interval_ms")]
    pub reflexion_interval_ms: u64,
}

fn default_tracking_window() -> usize {
    100
}

fn default_awareness_threshold() -> f64 {
    0.5
}

fn default_reflexion_interval_ms() -> u64 {
    60000
}

impl Default for ConsciousnessConfig {
    fn default() -> Self {
        Self {
            tracking_window: 100,
            awareness_threshold: 0.5,
            reflexion_interval_ms: 60000,
        }
    }
}

// ── Awareness metric type ───────────────────────────────────────────────────

/// Category of awareness being measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[allow(clippy::enum_variant_names)]
pub enum AwarenessMetricType {
    /// Awareness of one's own internal state, capabilities, and limitations.
    SelfAwareness,
    /// Awareness of the external environment and context.
    EnvironmentalAwareness,
    /// Awareness of time, history, and temporal patterns.
    TemporalAwareness,
    /// Awareness of other agents, social structures, and interactions.
    SocialAwareness,
    /// Awareness of one's own cognitive processes (thinking about thinking).
    MetaAwareness,
}

impl AwarenessMetricType {
    /// All five metric types in a canonical ordering.
    pub fn all() -> [AwarenessMetricType; 5] {
        [
            AwarenessMetricType::SelfAwareness,
            AwarenessMetricType::EnvironmentalAwareness,
            AwarenessMetricType::TemporalAwareness,
            AwarenessMetricType::SocialAwareness,
            AwarenessMetricType::MetaAwareness,
        ]
    }
}

// ── Core data structures ────────────────────────────────────────────────────

/// A single awareness metric observation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwarenessMetric {
    /// Which type of awareness this metric applies to.
    pub metric_type: AwarenessMetricType,
    /// Measured value in the [0.0, 1.0] range.
    pub value: f64,
    /// Confidence in the measurement, in the [0.0, 1.0] range.
    pub confidence: f64,
    /// Unix-millisecond timestamp when the metric was recorded.
    pub timestamp_ms: u64,
}

/// Consciousness states an agent can occupy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConsciousnessState {
    /// No measurable awareness; agent acts purely reactively.
    Unconscious,
    /// Bare minimum awareness; agent responds to immediate stimuli only.
    Minimal,
    /// Agent can reflect on past actions and adjust behaviour.
    Reflexive,
    /// Agent has a model of itself and can reason about its own capabilities.
    SelfAware,
    /// Agent is aware of its own cognitive processes and can reason about
    /// its reasoning (meta-cognition).
    MetaCognitive,
}

/// Record of a single reflexion cycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflexionRecord {
    /// Unique identifier for this reflexion event.
    pub id: String,
    /// Human-readable trigger description.
    pub trigger: String,
    /// Consciousness state before the reflexion.
    pub state_before: ConsciousnessState,
    /// Consciousness state after the reflexion.
    pub state_after: ConsciousnessState,
    /// Insights generated during the reflexion.
    pub insights: Vec<String>,
    /// Unix-millisecond timestamp when the reflexion occurred.
    pub timestamp_ms: u64,
}

/// Runtime snapshot of the consciousness tracker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsciousnessProfile {
    /// Current consciousness state.
    pub state: ConsciousnessState,
    /// Overall awareness score across all metric types.
    pub overall_awareness: f64,
    /// Total number of metrics recorded.
    pub metric_count: usize,
    /// Unix-millisecond timestamp of the last reflexion, or 0.
    pub last_reflexion_ms: u64,
    /// Total number of reflexion cycles performed.
    pub reflexion_count: u64,
}

// ── Internal state ──────────────────────────────────────────────────────────

#[derive(Debug)]
struct Inner {
    config: ConsciousnessConfig,
    metrics: Vec<AwarenessMetric>,
    reflexions: Vec<ReflexionRecord>,
    next_reflexion_id: u64,
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Thread-safe tracker for consciousness agent metrics.
#[derive(Debug, Clone)]
pub struct ConsciousnessMetrics {
    inner: Arc<Mutex<Inner>>,
}

impl ConsciousnessMetrics {
    /// Create a new consciousness metrics tracker with the given configuration.
    pub fn new(config: ConsciousnessConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                config,
                metrics: Vec::new(),
                reflexions: Vec::new(),
                next_reflexion_id: 1,
            })),
        }
    }

    // ── Metric recording ────────────────────────────────────────────────

    /// Record an awareness metric observation.
    ///
    /// `value` and `confidence` must both be in [0.0, 1.0].
    pub fn record_metric(
        &self,
        metric_type: AwarenessMetricType,
        value: f64,
        confidence: f64,
    ) -> Result<()> {
        if !(0.0..=1.0).contains(&value) {
            bail!("value must be in [0.0, 1.0], got {value}");
        }
        if !(0.0..=1.0).contains(&confidence) {
            bail!("confidence must be in [0.0, 1.0], got {confidence}");
        }

        let mut inner = self.inner.lock().unwrap();

        let metric = AwarenessMetric {
            metric_type,
            value,
            confidence,
            timestamp_ms: now_ms(),
        };

        inner.metrics.push(metric);

        // Trim history to tracking_window.
        let window = inner.config.tracking_window;
        if inner.metrics.len() > window {
            let excess = inner.metrics.len() - window;
            inner.metrics.drain(0..excess);
        }

        Ok(())
    }

    // ── State query ──────────────────────────────────────────────────────

    /// Determine the current consciousness state based on average awareness.
    ///
    /// State transitions are driven by the average awareness value across
    /// all metric types:
    /// - `< 0.2`  → `Unconscious`
    /// - `0.2-0.4` → `Minimal`
    /// - `0.4-0.6` → `Reflexive`
    /// - `0.6-0.8` → `SelfAware`
    /// - `>= 0.8`  → `MetaCognitive`
    pub fn current_state(&self) -> ConsciousnessState {
        let avg = self.average_awareness();
        if avg >= 0.8 {
            ConsciousnessState::MetaCognitive
        } else if avg >= 0.6 {
            ConsciousnessState::SelfAware
        } else if avg >= 0.4 {
            ConsciousnessState::Reflexive
        } else if avg >= 0.2 {
            ConsciousnessState::Minimal
        } else {
            ConsciousnessState::Unconscious
        }
    }

    // ── Reflexion ────────────────────────────────────────────────────────

    /// Trigger a reflexion cycle, generating a record and potentially advancing
    /// the consciousness state.
    pub fn trigger_reflexion(&self, trigger: &str) -> Result<ReflexionRecord> {
        let mut inner = self.inner.lock().unwrap();

        let state_before = self.compute_state_from_metrics(&inner.metrics);

        // Generate insights based on current awareness.
        let insights = self.generate_insights(&inner.metrics);

        // After reflexion, awareness gets a boost based on insight count.
        let boost = (insights.len() as f64).min(5.0) * 0.02;
        let boosted_metrics: Vec<AwarenessMetric> = inner
            .metrics
            .iter()
            .map(|m| {
                let mut boosted = m.clone();
                boosted.value = (boosted.value + boost).min(1.0);
                boosted
            })
            .collect();

        let state_after = self.compute_state_from_metrics(&boosted_metrics);

        let id = format!("reflexion-{}", inner.next_reflexion_id);
        inner.next_reflexion_id += 1;

        let record = ReflexionRecord {
            id: id.clone(),
            trigger: trigger.to_string(),
            state_before,
            state_after,
            insights,
            timestamp_ms: now_ms(),
        };

        // Replace metrics with boosted versions.
        inner.metrics = boosted_metrics;

        inner.reflexions.push(record.clone());
        Ok(record)
    }

    // ── Awareness by type ────────────────────────────────────────────────

    /// Compute the average value for a specific awareness metric type,
    /// considering only metrics within the tracking window.
    pub fn awareness_by_type(&self, metric_type: AwarenessMetricType) -> f64 {
        let inner = self.inner.lock().unwrap();
        let filtered: Vec<f64> = inner
            .metrics
            .iter()
            .rev()
            .take(inner.config.tracking_window)
            .filter(|m| m.metric_type == metric_type)
            .map(|m| m.value)
            .collect();

        if filtered.is_empty() {
            return 0.0;
        }
        filtered.iter().sum::<f64>() / filtered.len() as f64
    }

    // ── Profile ──────────────────────────────────────────────────────────

    /// Return a snapshot of the tracker's runtime metrics.
    pub fn profile(&self) -> ConsciousnessProfile {
        let inner = self.inner.lock().unwrap();

        let overall = self.compute_overall_from_inner(&inner.metrics);
        let state = self.compute_state_from_metrics(&inner.metrics);
        let last_reflexion = inner.reflexions.last().map(|r| r.timestamp_ms).unwrap_or(0);

        ConsciousnessProfile {
            state,
            overall_awareness: overall,
            metric_count: inner.metrics.len(),
            last_reflexion_ms: last_reflexion,
            reflexion_count: inner.reflexions.len() as u64,
        }
    }

    // ── Internal helpers ─────────────────────────────────────────────────

    /// Compute the average awareness across all metric types from a slice.
    fn compute_state_from_metrics(&self, metrics: &[AwarenessMetric]) -> ConsciousnessState {
        if metrics.is_empty() {
            return ConsciousnessState::Unconscious;
        }

        let avg = self.compute_overall_from_inner(metrics);
        if avg >= 0.8 {
            ConsciousnessState::MetaCognitive
        } else if avg >= 0.6 {
            ConsciousnessState::SelfAware
        } else if avg >= 0.4 {
            ConsciousnessState::Reflexive
        } else if avg >= 0.2 {
            ConsciousnessState::Minimal
        } else {
            ConsciousnessState::Unconscious
        }
    }

    /// Compute overall awareness as the average of the latest value for each
    /// metric type, weighted by confidence.
    fn compute_overall_from_inner(&self, metrics: &[AwarenessMetric]) -> f64 {
        let types = AwarenessMetricType::all();
        let mut sum = 0.0;
        let mut count = 0;

        for t in &types {
            // Find the most recent metric for this type.
            if let Some(latest) = metrics.iter().rev().find(|m| m.metric_type == *t) {
                // Weight value by confidence.
                sum += latest.value * latest.confidence;
                count += 1;
            }
        }

        if count == 0 {
            0.0
        } else {
            sum / count as f64
        }
    }

    /// Compute the simple average awareness (unweighted) for state calculation.
    fn average_awareness(&self) -> f64 {
        let inner = self.inner.lock().unwrap();
        let types = AwarenessMetricType::all();
        let mut sum = 0.0;
        let mut count = 0;

        for t in &types {
            if let Some(latest) = inner.metrics.iter().rev().find(|m| m.metric_type == *t) {
                sum += latest.value;
                count += 1;
            }
        }

        if count == 0 {
            0.0
        } else {
            sum / count as f64
        }
    }

    /// Generate insight strings based on current metrics.
    fn generate_insights(&self, metrics: &[AwarenessMetric]) -> Vec<String> {
        let mut insights = Vec::new();

        for t in AwarenessMetricType::all() {
            let avg: f64 = {
                let filtered: Vec<f64> = metrics
                    .iter()
                    .filter(|m| m.metric_type == t)
                    .map(|m| m.value)
                    .collect();
                if filtered.is_empty() {
                    continue;
                }
                filtered.iter().sum::<f64>() / filtered.len() as f64
            };

            if avg >= 0.8 {
                insights.push(format!("Strong {:?}: sustained awareness at {:.2}", t, avg));
            } else if avg >= 0.5 {
                insights.push(format!(
                    "Developing {:?}: current awareness at {:.2}",
                    t, avg
                ));
            } else if avg > 0.0 {
                insights.push(format!(
                    "Weak {:?}: current awareness only at {:.2}",
                    t, avg
                ));
            }
        }

        if insights.is_empty() {
            insights.push("No awareness data available yet.".to_string());
        }

        insights
    }
}

// ── Timestamp helper ───────────────────────────────────────────────────────

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── 1. Fresh tracker starts Unconscious ──────────────────────────────
    #[test]
    fn test_new_metrics_unconscious() {
        let cm = ConsciousnessMetrics::new(ConsciousnessConfig::default());
        assert_eq!(cm.current_state(), ConsciousnessState::Unconscious);

        let p = cm.profile();
        assert_eq!(p.state, ConsciousnessState::Unconscious);
        assert!((p.overall_awareness - 0.0).abs() < 1e-9);
        assert_eq!(p.metric_count, 0);
        assert_eq!(p.last_reflexion_ms, 0);
        assert_eq!(p.reflexion_count, 0);
    }

    // ── 2. Low metrics keep state Unconscious ────────────────────────────
    #[test]
    fn test_record_low_metric_stays_unconscious() {
        let cm = ConsciousnessMetrics::new(ConsciousnessConfig::default());

        cm.record_metric(AwarenessMetricType::SelfAwareness, 0.1, 0.9)
            .unwrap();
        assert_eq!(cm.current_state(), ConsciousnessState::Unconscious);

        cm.record_metric(AwarenessMetricType::EnvironmentalAwareness, 0.15, 0.8)
            .unwrap();
        assert_eq!(cm.current_state(), ConsciousnessState::Unconscious);

        // 0.19 is still under 0.2.
        cm.record_metric(AwarenessMetricType::TemporalAwareness, 0.19, 0.7)
            .unwrap();
        assert_eq!(cm.current_state(), ConsciousnessState::Unconscious);
    }

    // ── 3. High metrics reach SelfAware ──────────────────────────────────
    #[test]
    fn test_record_high_metric_reaches_self_aware() {
        let cm = ConsciousnessMetrics::new(ConsciousnessConfig::default());

        cm.record_metric(AwarenessMetricType::SelfAwareness, 0.7, 0.9)
            .unwrap();
        cm.record_metric(AwarenessMetricType::EnvironmentalAwareness, 0.65, 0.85)
            .unwrap();
        cm.record_metric(AwarenessMetricType::TemporalAwareness, 0.6, 0.8)
            .unwrap();
        cm.record_metric(AwarenessMetricType::SocialAwareness, 0.65, 0.85)
            .unwrap();
        cm.record_metric(AwarenessMetricType::MetaAwareness, 0.6, 0.8)
            .unwrap();

        // Average of values = (0.7 + 0.65 + 0.6 + 0.65 + 0.6) / 5 = 0.64 → SelfAware.
        assert_eq!(cm.current_state(), ConsciousnessState::SelfAware);
    }

    // ── 4. Invalid values are rejected ───────────────────────────────────
    #[test]
    fn test_record_metric_invalid_value() {
        let cm = ConsciousnessMetrics::new(ConsciousnessConfig::default());

        // Value too high.
        assert!(cm
            .record_metric(AwarenessMetricType::SelfAwareness, 1.5, 0.5)
            .is_err());

        // Value too low (negative).
        assert!(cm
            .record_metric(AwarenessMetricType::SelfAwareness, -0.1, 0.5)
            .is_err());

        // Confidence too high.
        assert!(cm
            .record_metric(AwarenessMetricType::SelfAwareness, 0.5, 1.5)
            .is_err());

        // Confidence too low (negative).
        assert!(cm
            .record_metric(AwarenessMetricType::SelfAwareness, 0.5, -0.1)
            .is_err());

        // Valid values still accepted.
        assert!(cm
            .record_metric(AwarenessMetricType::EnvironmentalAwareness, 0.5, 0.5)
            .is_ok());
    }

    // ── 5. Trigger reflexion creates a record ────────────────────────────
    #[test]
    fn test_trigger_reflexion() {
        let cm = ConsciousnessMetrics::new(ConsciousnessConfig::default());

        // Record some metrics first.
        cm.record_metric(AwarenessMetricType::SelfAwareness, 0.3, 0.8)
            .unwrap();
        cm.record_metric(AwarenessMetricType::EnvironmentalAwareness, 0.25, 0.7)
            .unwrap();

        let record = cm.trigger_reflexion("test trigger").unwrap();

        assert!(record.id.starts_with("reflexion-"));
        assert_eq!(record.trigger, "test trigger");
        assert_eq!(record.state_before, ConsciousnessState::Minimal);
        assert_eq!(record.timestamp_ms > 0, true);
        assert!(!record.insights.is_empty());
    }

    // ── 6. Reflexion can advance state ───────────────────────────────────
    #[test]
    fn test_reflexion_advances_state() {
        let cm = ConsciousnessMetrics::new(ConsciousnessConfig::default());

        // Set up metrics close to the next threshold.
        cm.record_metric(AwarenessMetricType::SelfAwareness, 0.38, 0.9)
            .unwrap();
        cm.record_metric(AwarenessMetricType::EnvironmentalAwareness, 0.38, 0.9)
            .unwrap();
        cm.record_metric(AwarenessMetricType::TemporalAwareness, 0.38, 0.9)
            .unwrap();
        cm.record_metric(AwarenessMetricType::SocialAwareness, 0.38, 0.9)
            .unwrap();
        cm.record_metric(AwarenessMetricType::MetaAwareness, 0.38, 0.9)
            .unwrap();

        // Current: average 0.38 → Minimal (0.2-0.4).
        assert_eq!(cm.current_state(), ConsciousnessState::Minimal);

        // Reflexion boosts metrics by insights.len() * 0.02. With 5 types, minimal
        // insights = 5 (one per type), so boost = 5 * 0.02 = 0.1.
        // New average ≈ 0.38 + 0.1 = 0.48 → Reflexive (0.4-0.6).
        let record = cm.trigger_reflexion("advance test").unwrap();

        assert_eq!(record.state_before, ConsciousnessState::Minimal);
        // State should have advanced.
        assert_ne!(record.state_before, record.state_after);
        assert_eq!(cm.current_state(), ConsciousnessState::Reflexive);
    }

    // ── 7. Awareness by type returns correct values ──────────────────────
    #[test]
    fn test_awareness_by_type() {
        let cm = ConsciousnessMetrics::new(ConsciousnessConfig::default());

        // No data → 0.0.
        let val = cm.awareness_by_type(AwarenessMetricType::SelfAwareness);
        assert!((val - 0.0).abs() < 1e-9);

        // Record single metric.
        cm.record_metric(AwarenessMetricType::SelfAwareness, 0.75, 0.9)
            .unwrap();
        let val = cm.awareness_by_type(AwarenessMetricType::SelfAwareness);
        assert!((val - 0.75).abs() < 1e-9);

        // Record another of same type — should average.
        cm.record_metric(AwarenessMetricType::SelfAwareness, 0.85, 0.9)
            .unwrap();
        let val = cm.awareness_by_type(AwarenessMetricType::SelfAwareness);
        assert!((val - 0.80).abs() < 1e-9);

        // Different type with no data → 0.0.
        let val = cm.awareness_by_type(AwarenessMetricType::EnvironmentalAwareness);
        assert!((val - 0.0).abs() < 1e-9);
    }

    // ── 8. Profile reflects current state ────────────────────────────────
    #[test]
    fn test_profile_reflects_state() {
        let cm = ConsciousnessMetrics::new(ConsciousnessConfig::default());

        // Fresh → Unconscious.
        let p = cm.profile();
        assert_eq!(p.state, ConsciousnessState::Unconscious);
        assert_eq!(p.metric_count, 0);
        assert_eq!(p.reflexion_count, 0);

        // Add metrics to reach Minimal.
        cm.record_metric(AwarenessMetricType::SelfAwareness, 0.3, 0.8)
            .unwrap();
        cm.record_metric(AwarenessMetricType::EnvironmentalAwareness, 0.3, 0.8)
            .unwrap();

        let p = cm.profile();
        assert_eq!(p.state, ConsciousnessState::Minimal);
        assert_eq!(p.metric_count, 2);
        assert_eq!(p.reflexion_count, 0);

        // Trigger a reflexion.
        cm.trigger_reflexion("profile test").unwrap();

        let p = cm.profile();
        assert_eq!(p.reflexion_count, 1);
        assert!(p.last_reflexion_ms > 0);
    }

    // ── 9. Multiple reflexions accumulate ────────────────────────────────
    #[test]
    fn test_multiple_reflexions() {
        let cm = ConsciousnessMetrics::new(ConsciousnessConfig::default());

        // Record some initial metrics.
        cm.record_metric(AwarenessMetricType::SelfAwareness, 0.5, 0.8)
            .unwrap();
        cm.record_metric(AwarenessMetricType::EnvironmentalAwareness, 0.5, 0.8)
            .unwrap();
        cm.record_metric(AwarenessMetricType::TemporalAwareness, 0.5, 0.8)
            .unwrap();
        cm.record_metric(AwarenessMetricType::SocialAwareness, 0.5, 0.8)
            .unwrap();
        cm.record_metric(AwarenessMetricType::MetaAwareness, 0.5, 0.8)
            .unwrap();

        // First reflexion.
        let r1 = cm.trigger_reflexion("first reflexion").unwrap();
        assert_eq!(r1.id, "reflexion-1");

        // Second reflexion.
        let r2 = cm.trigger_reflexion("second reflexion").unwrap();
        assert_eq!(r2.id, "reflexion-2");

        let p = cm.profile();
        assert_eq!(p.reflexion_count, 2);

        // Each reflexion in sequence should have increasing IDs.
        assert!(r2.timestamp_ms >= r1.timestamp_ms);
    }

    // ── 10. Config defaults match specification ──────────────────────────
    #[test]
    fn test_config_defaults() {
        let config = ConsciousnessConfig::default();
        assert_eq!(config.tracking_window, 100);
        assert!((config.awareness_threshold - 0.5).abs() < 1e-9);
        assert_eq!(config.reflexion_interval_ms, 60000);

        // Verify serde round-trip.
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: ConsciousnessConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.tracking_window, 100);
        assert!((deserialized.awareness_threshold - 0.5).abs() < 1e-9);
        assert_eq!(deserialized.reflexion_interval_ms, 60000);

        // Empty JSON should use defaults.
        let from_empty: ConsciousnessConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(from_empty.tracking_window, 100);
        assert!((from_empty.awareness_threshold - 0.5).abs() < 1e-9);
        assert_eq!(from_empty.reflexion_interval_ms, 60000);
    }

    // ── 11. State transitions through all levels ─────────────────────────
    #[test]
    fn test_all_state_transitions() {
        let cm = ConsciousnessMetrics::new(ConsciousnessConfig::default());

        // Unconscious (< 0.2).
        cm.record_metric(AwarenessMetricType::SelfAwareness, 0.1, 0.9)
            .unwrap();
        assert_eq!(cm.current_state(), ConsciousnessState::Unconscious);

        // Minimal (0.2-0.4).
        cm.record_metric(AwarenessMetricType::EnvironmentalAwareness, 0.35, 0.9)
            .unwrap();
        cm.record_metric(AwarenessMetricType::TemporalAwareness, 0.35, 0.9)
            .unwrap();
        assert_eq!(cm.current_state(), ConsciousnessState::Minimal);

        // Reflexive (0.4-0.6).
        cm.record_metric(AwarenessMetricType::SelfAwareness, 0.55, 0.9)
            .unwrap();
        cm.record_metric(AwarenessMetricType::SocialAwareness, 0.55, 0.9)
            .unwrap();
        assert_eq!(cm.current_state(), ConsciousnessState::Reflexive);

        // SelfAware (0.6-0.8). Need avg >= 0.6 with all 5 types.
        // Current: Self=0.55, Env=0.35, Temp=0.35, Soc=0.55, Meta=0.0 → avg=0.36
        // Raise Env, Temp, and Meta to cross threshold.
        cm.record_metric(AwarenessMetricType::EnvironmentalAwareness, 0.7, 0.9)
            .unwrap();
        cm.record_metric(AwarenessMetricType::TemporalAwareness, 0.7, 0.9)
            .unwrap();
        cm.record_metric(AwarenessMetricType::MetaAwareness, 0.7, 0.9)
            .unwrap();
        // Now: Self=0.55, Env=0.7, Temp=0.7, Soc=0.55, Meta=0.7 → avg=0.64
        assert_eq!(cm.current_state(), ConsciousnessState::SelfAware);

        // MetaCognitive (>= 0.8). Raise all to cross threshold.
        cm.record_metric(AwarenessMetricType::SelfAwareness, 0.85, 0.9)
            .unwrap();
        cm.record_metric(AwarenessMetricType::SocialAwareness, 0.85, 0.9)
            .unwrap();
        // Now: Self=0.85, Env=0.7, Temp=0.7, Soc=0.85, Meta=0.7 → avg=0.76
        // Need one more push:
        cm.record_metric(AwarenessMetricType::EnvironmentalAwareness, 0.9, 0.9)
            .unwrap();
        cm.record_metric(AwarenessMetricType::TemporalAwareness, 0.9, 0.9)
            .unwrap();
        cm.record_metric(AwarenessMetricType::MetaAwareness, 0.9, 0.9)
            .unwrap();
        // Now: Self=0.85, Env=0.9, Temp=0.9, Soc=0.85, Meta=0.9 → avg=0.88
        assert_eq!(cm.current_state(), ConsciousnessState::MetaCognitive);
    }

    // ── 12. Reflexion generates meaningful insights ──────────────────────
    #[test]
    fn test_reflexion_insights() {
        let cm = ConsciousnessMetrics::new(ConsciousnessConfig::default());

        // Record strong awareness for one type and weak for another.
        cm.record_metric(AwarenessMetricType::SelfAwareness, 0.85, 0.9)
            .unwrap();
        cm.record_metric(AwarenessMetricType::EnvironmentalAwareness, 0.2, 0.7)
            .unwrap();
        cm.record_metric(AwarenessMetricType::TemporalAwareness, 0.55, 0.8)
            .unwrap();
        cm.record_metric(AwarenessMetricType::SocialAwareness, 0.0, 0.5)
            .unwrap();
        cm.record_metric(AwarenessMetricType::MetaAwareness, 0.1, 0.6)
            .unwrap();

        let record = cm.trigger_reflexion("insight test").unwrap();

        // Should have insights about strong SelfAwareness and weak SocialAwareness/MetaAwareness.
        let insights_text = record.insights.join(" ");
        assert!(insights_text.contains("SelfAwareness"));
        assert!(insights_text.contains("EnvironmentalAwareness"));
        assert!(record.insights.len() >= 4);
    }
}
