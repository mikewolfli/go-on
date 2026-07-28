//! F-GAP-25: Consciousness Agent Metrics
//!
//! Tracks simulated "consciousness" metrics for agent self-awareness.
//! All mutable state is guarded behind `Arc<Mutex<>>` for thread-safe
//! concurrent access.
//!
//! ## Relationship with [`super::metacognitive`]
//!
//! - `consciousness` tracks **numerical awareness metrics** across 7 dimensions and
//!   maintains a state machine (Unconscious → MetaCognitive). It is purely
//!   metric/statistical — no task tracking, no agent association.
//! - [`super::metacognitive`] tracks **concrete execution observations**, manages
//!   a corrective action lifecycle, and generates structured reflection reports
//!   with severity-weighted confidence scores.
//! - The two subsystems are bridged by [`super::triple_fusion`], which pushes
//!   metacognitive observations into consciousness EnvironmentalAwareness metrics
//!   and converts consciousness insights into evolution triggers.
//!
//! In short: `consciousness` = *how aware* the system is numerically;
//! `metacognitive` = *what it observes and does about it*.

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
    /// Error rate awareness — how often operations fail.
    ErrorRate,
    /// Task success rate awareness — how often tasks complete successfully.
    TaskSuccessRate,
}

impl AwarenessMetricType {
    /// All metric types in a canonical ordering.
    pub fn all() -> [AwarenessMetricType; 7] {
        [
            AwarenessMetricType::SelfAwareness,
            AwarenessMetricType::EnvironmentalAwareness,
            AwarenessMetricType::TemporalAwareness,
            AwarenessMetricType::SocialAwareness,
            AwarenessMetricType::MetaAwareness,
            AwarenessMetricType::ErrorRate,
            AwarenessMetricType::TaskSuccessRate,
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

/// Trend direction derived from a moving-average comparison of recent metrics.
///
/// Used by `ConsciousnessMetrics::current_state()` to adjust the conscious
/// state based on whether awareness is improving or declining.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum TrendDirection {
    StrongUp,
    WeakUp,
    Stable,
    WeakDown,
    StrongDown,
}

// ── Internal state ──────────────────────────────────────────────────────────

#[derive(Debug)]
struct Inner {
    config: ConsciousnessConfig,
    metrics: Vec<AwarenessMetric>,
    reflexions: Vec<ReflexionRecord>,
    total_reflexions: u64,
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
                total_reflexions: 0,
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

        let mut inner = crate::lock_or_recover!(&self.inner, "intelligence");

        let metric = AwarenessMetric {
            metric_type,
            value,
            confidence,
            timestamp_ms: crate::shared::timestamps::now_ts_ms() as u64,
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

    /// Determine the current consciousness state based on average awareness
    /// with trend analysis.
    ///
    /// State transitions are driven by the average awareness value across
    /// all metric types and the short-term trend direction:
    /// - `< 0.2`  → `Unconscious`
    /// - `0.2-0.4` → `Minimal`
    /// - `0.4-0.6` → `Reflexive`
    /// - `0.6-0.8` → `SelfAware`
    /// - `>= 0.8`  → `MetaCognitive`
    ///
    /// When a strong upward trend is detected, the state is promoted by
    /// one level (early recognition of improvement). When a strong downward
    /// trend is detected, the state is demoted by one level (early warning
    /// of degradation).
    pub fn current_state(&self) -> ConsciousnessState {
        let avg = self.average_awareness();
        // Determine the base state from the average awareness value.
        let base = if avg >= 0.8 {
            ConsciousnessState::MetaCognitive
        } else if avg >= 0.6 {
            ConsciousnessState::SelfAware
        } else if avg >= 0.4 {
            ConsciousnessState::Reflexive
        } else if avg >= 0.2 {
            ConsciousnessState::Minimal
        } else {
            ConsciousnessState::Unconscious
        };

        // Apply trend adjustment using simple linear regression over recent
        // metrics. A strong upward trend promotes the state by one level;
        // a strong downward trend demotes it by one level.
        let trend = self.compute_trend();
        match trend {
            TrendDirection::StrongUp => Self::promote_state(base),
            TrendDirection::StrongDown => Self::demote_state(base),
            TrendDirection::Stable | TrendDirection::WeakUp | TrendDirection::WeakDown => base,
        }
    }

    // ── Reflexion ────────────────────────────────────────────────────────

    /// Trigger a reflexion cycle, generating a record and potentially advancing
    /// the consciousness state.
    pub fn trigger_reflexion(&self, trigger: &str) -> Result<ReflexionRecord> {
        // Clone metrics and other data under the lock, then process outside
        let (metrics_clone, next_id) = {
            let inner = crate::lock_or_recover!(&self.inner, "intelligence");
            (inner.metrics.clone(), inner.next_reflexion_id)
        };

        let state_before = self.compute_state_from_metrics(&metrics_clone);

        // Generate insights based on current awareness.
        let insights = self.generate_insights(&metrics_clone);

        // After reflexion, awareness gets a boost based on insight count.
        let boost = (insights.len() as f64).min(5.0) * 0.02;
        let boosted_metrics: Vec<AwarenessMetric> = metrics_clone
            .iter()
            .map(|m| {
                let mut boosted = m.clone();
                boosted.value = (boosted.value + boost).min(1.0);
                boosted
            })
            .collect();

        let state_after = self.compute_state_from_metrics(&boosted_metrics);

        let id = format!("reflexion-{}", next_id);

        let record = ReflexionRecord {
            id: id.clone(),
            trigger: trigger.to_string(),
            state_before,
            state_after,
            insights,
            timestamp_ms: crate::shared::timestamps::now_ts_ms() as u64,
        };

        // Re-acquire lock to write back
        let mut inner = crate::lock_or_recover!(&self.inner, "intelligence");
        inner.next_reflexion_id += 1;
        // Replace metrics with boosted versions.
        inner.metrics = boosted_metrics;

        const MAX_REFLEXIONS: usize = 1000;
        if inner.reflexions.len() >= MAX_REFLEXIONS {
            inner.reflexions.remove(0);
        }

        inner.reflexions.push(record.clone());
        inner.total_reflexions = inner.total_reflexions.saturating_add(1);
        Ok(record)
    }

    // ── Awareness by type ────────────────────────────────────────────────

    /// Compute the average value for a specific awareness metric type,
    /// considering only metrics within the tracking window.
    pub fn awareness_by_type(&self, metric_type: AwarenessMetricType) -> f64 {
        let inner = crate::lock_or_recover!(&self.inner, "intelligence");
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
        let inner = crate::lock_or_recover!(&self.inner, "intelligence");

        let overall = self.compute_overall_from_inner(&inner.metrics);
        let state = self.compute_state_from_metrics(&inner.metrics);
        let last_reflexion = inner.reflexions.last().map(|r| r.timestamp_ms).unwrap_or(0);

        ConsciousnessProfile {
            state,
            overall_awareness: overall,
            metric_count: inner.metrics.len(),
            last_reflexion_ms: last_reflexion,
            reflexion_count: inner.total_reflexions,
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
        let inner = crate::lock_or_recover!(&self.inner, "intelligence");
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

    /// Compute trend direction using a moving-average comparison over the
    /// N most recent metrics.
    ///
    /// Splits the recent metric window into two halves (older vs newer)
    /// and compares their averages. A minimum of 10 data points is required
    /// before any trend adjustment is applied, preventing noise from small
    /// sample sizes.
    fn compute_trend(&self) -> TrendDirection {
        let inner = crate::lock_or_recover!(&self.inner, "intelligence");
        let window = inner.config.tracking_window;
        let metrics: Vec<&AwarenessMetric> =
            inner.metrics.iter().rev().take(window).collect::<Vec<_>>();

        // Require at least 10 data points before attempting trend analysis
        // to avoid overreacting to noise in small samples.
        if metrics.len() < 10 {
            return TrendDirection::Stable;
        }

        // Also collect the earlier half of the window for moving average
        let half = metrics.len() / 2;
        if half < 2 {
            return TrendDirection::Stable;
        }

        // Split into two halves: early (newer) and late (older).
        // Since metrics are reversed (newest first), early = newer half.
        let (early, late) = metrics.split_at(half);

        let early_avg: f64 = early.iter().map(|m| m.value).sum::<f64>() / early.len() as f64;
        let late_avg: f64 = late.iter().map(|m| m.value).sum::<f64>() / late.len() as f64;

        let diff = early_avg - late_avg;
        let threshold = 0.05; // 5% absolute change threshold

        if diff > threshold {
            TrendDirection::StrongUp
        } else if diff > threshold * 0.5 {
            TrendDirection::WeakUp
        } else if diff < -threshold {
            TrendDirection::StrongDown
        } else if diff < -threshold * 0.5 {
            TrendDirection::WeakDown
        } else {
            TrendDirection::Stable
        }
    }

    /// Promote a consciousness state by one level.
    fn promote_state(state: ConsciousnessState) -> ConsciousnessState {
        match state {
            ConsciousnessState::Unconscious => ConsciousnessState::Minimal,
            ConsciousnessState::Minimal => ConsciousnessState::Reflexive,
            ConsciousnessState::Reflexive => ConsciousnessState::SelfAware,
            ConsciousnessState::SelfAware => ConsciousnessState::MetaCognitive,
            ConsciousnessState::MetaCognitive => ConsciousnessState::MetaCognitive,
        }
    }

    /// Demote a consciousness state by one level.
    fn demote_state(state: ConsciousnessState) -> ConsciousnessState {
        match state {
            ConsciousnessState::Unconscious => ConsciousnessState::Unconscious,
            ConsciousnessState::Minimal => ConsciousnessState::Unconscious,
            ConsciousnessState::Reflexive => ConsciousnessState::Minimal,
            ConsciousnessState::SelfAware => ConsciousnessState::Reflexive,
            ConsciousnessState::MetaCognitive => ConsciousnessState::SelfAware,
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
            .expect("should record SelfAwareness metric at 0.1");
        assert_eq!(cm.current_state(), ConsciousnessState::Unconscious);

        cm.record_metric(AwarenessMetricType::EnvironmentalAwareness, 0.15, 0.8)
            .expect("should record EnvironmentalAwareness metric at 0.15");
        assert_eq!(cm.current_state(), ConsciousnessState::Unconscious);

        // 0.19 is still under 0.2.
        cm.record_metric(AwarenessMetricType::TemporalAwareness, 0.19, 0.7)
            .expect("should record TemporalAwareness metric at 0.19");
        assert_eq!(cm.current_state(), ConsciousnessState::Unconscious);
    }

    // ── 3. High metrics reach SelfAware ──────────────────────────────────
    #[test]
    fn test_record_high_metric_reaches_self_aware() {
        let cm = ConsciousnessMetrics::new(ConsciousnessConfig::default());

        cm.record_metric(AwarenessMetricType::SelfAwareness, 0.7, 0.9)
            .expect("should record high SelfAwareness metric");
        cm.record_metric(AwarenessMetricType::EnvironmentalAwareness, 0.65, 0.85)
            .expect("should record high EnvironmentalAwareness metric");
        cm.record_metric(AwarenessMetricType::TemporalAwareness, 0.6, 0.8)
            .expect("should record high TemporalAwareness metric");
        cm.record_metric(AwarenessMetricType::SocialAwareness, 0.65, 0.85)
            .expect("should record high SocialAwareness metric");
        cm.record_metric(AwarenessMetricType::MetaAwareness, 0.6, 0.8)
            .expect("should record high MetaAwareness metric");

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
            .expect("should record initial SelfAwareness metric");
        cm.record_metric(AwarenessMetricType::EnvironmentalAwareness, 0.25, 0.7)
            .expect("should record initial EnvironmentalAwareness metric");

        let record = cm
            .trigger_reflexion("test trigger")
            .expect("should trigger first reflexion");

        assert!(record.id.starts_with("reflexion-"));
        assert_eq!(record.trigger, "test trigger");
        assert_eq!(record.state_before, ConsciousnessState::Minimal);
        assert!(record.timestamp_ms > 0);
        assert!(!record.insights.is_empty());
    }

    // ── 6. Reflexion can advance state ───────────────────────────────────
    #[test]
    fn test_reflexion_advances_state() {
        let cm = ConsciousnessMetrics::new(ConsciousnessConfig::default());

        // Set up metrics close to the next threshold.
        cm.record_metric(AwarenessMetricType::SelfAwareness, 0.38, 0.9)
            .expect("should record initial SelfAwareness at 0.38");
        cm.record_metric(AwarenessMetricType::EnvironmentalAwareness, 0.38, 0.9)
            .expect("should record initial EnvironmentalAwareness at 0.38");
        cm.record_metric(AwarenessMetricType::TemporalAwareness, 0.38, 0.9)
            .expect("should record initial TemporalAwareness at 0.38");
        cm.record_metric(AwarenessMetricType::SocialAwareness, 0.38, 0.9)
            .expect("should record initial SocialAwareness at 0.38");
        cm.record_metric(AwarenessMetricType::MetaAwareness, 0.38, 0.9)
            .expect("should record initial MetaAwareness at 0.38");

        // Current: average 0.38 → Minimal (0.2-0.4).
        assert_eq!(cm.current_state(), ConsciousnessState::Minimal);

        // Reflexion boosts metrics by insights.len() * 0.02. With 5 types, minimal
        // insights = 5 (one per type), so boost = 5 * 0.02 = 0.1.
        // New average ≈ 0.38 + 0.1 = 0.48 → Reflexive (0.4-0.6).
        let record = cm
            .trigger_reflexion("advance test")
            .expect("should trigger reflexion to advance state");

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
            .expect("should record first SelfAwareness metric");
        let val = cm.awareness_by_type(AwarenessMetricType::SelfAwareness);
        assert!((val - 0.75).abs() < 1e-9);

        // Record another of same type — should average.
        cm.record_metric(AwarenessMetricType::SelfAwareness, 0.85, 0.9)
            .expect("should record second SelfAwareness metric");
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
            .expect("should record SelfAwareness for Minimal state");
        cm.record_metric(AwarenessMetricType::EnvironmentalAwareness, 0.3, 0.8)
            .expect("should record EnvironmentalAwareness for Minimal state");

        let p = cm.profile();
        assert_eq!(p.state, ConsciousnessState::Minimal);
        assert_eq!(p.metric_count, 2);
        assert_eq!(p.reflexion_count, 0);

        // Trigger a reflexion.
        cm.trigger_reflexion("profile test")
            .expect("should trigger reflexion in profile test");

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
            .expect("should record SelfAwareness at 0.5");
        cm.record_metric(AwarenessMetricType::EnvironmentalAwareness, 0.5, 0.8)
            .expect("should record EnvironmentalAwareness at 0.5");
        cm.record_metric(AwarenessMetricType::TemporalAwareness, 0.5, 0.8)
            .expect("should record TemporalAwareness at 0.5");
        cm.record_metric(AwarenessMetricType::SocialAwareness, 0.5, 0.8)
            .expect("should record SocialAwareness at 0.5");
        cm.record_metric(AwarenessMetricType::MetaAwareness, 0.5, 0.8)
            .expect("should record MetaAwareness at 0.5");

        // First reflexion.
        let r1 = cm
            .trigger_reflexion("first reflexion")
            .expect("should trigger first reflexion");
        assert_eq!(r1.id, "reflexion-1");

        // Second reflexion.
        let r2 = cm
            .trigger_reflexion("second reflexion")
            .expect("should trigger second reflexion");
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
        let json = serde_json::to_string(&config).expect("should serialize config to JSON");
        let deserialized: ConsciousnessConfig =
            serde_json::from_str(&json).expect("should deserialize config from JSON");
        assert_eq!(deserialized.tracking_window, 100);
        assert!((deserialized.awareness_threshold - 0.5).abs() < 1e-9);
        assert_eq!(deserialized.reflexion_interval_ms, 60000);

        // Empty JSON should use defaults.
        let from_empty: ConsciousnessConfig =
            serde_json::from_str("{}").expect("should deserialize empty JSON with defaults");
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
            .expect("should record low SelfAwareness for Unconscious state");
        assert_eq!(cm.current_state(), ConsciousnessState::Unconscious);

        // Minimal (0.2-0.4).
        cm.record_metric(AwarenessMetricType::EnvironmentalAwareness, 0.35, 0.9)
            .expect("should record EnvironmentalAwareness at 0.35");
        cm.record_metric(AwarenessMetricType::TemporalAwareness, 0.35, 0.9)
            .expect("should record TemporalAwareness at 0.35");
        assert_eq!(cm.current_state(), ConsciousnessState::Minimal);

        // Reflexive (0.4-0.6).
        cm.record_metric(AwarenessMetricType::SelfAwareness, 0.55, 0.9)
            .expect("should record SelfAwareness at 0.55");
        cm.record_metric(AwarenessMetricType::SocialAwareness, 0.55, 0.9)
            .expect("should record SocialAwareness at 0.55");
        assert_eq!(cm.current_state(), ConsciousnessState::Reflexive);

        // SelfAware (0.6-0.8). Need avg >= 0.6 with all 5 types.
        // Current: Self=0.55, Env=0.35, Temp=0.35, Soc=0.55, Meta=0.0 → avg=0.36
        // Raise Env, Temp, and Meta to cross threshold.
        cm.record_metric(AwarenessMetricType::EnvironmentalAwareness, 0.7, 0.9)
            .expect("should record EnvironmentalAwareness at 0.7");
        cm.record_metric(AwarenessMetricType::TemporalAwareness, 0.7, 0.9)
            .expect("should record TemporalAwareness at 0.7");
        cm.record_metric(AwarenessMetricType::MetaAwareness, 0.7, 0.9)
            .expect("should record MetaAwareness at 0.7");
        // Now: Self=0.55, Env=0.7, Temp=0.7, Soc=0.55, Meta=0.7 → avg=0.64
        assert_eq!(cm.current_state(), ConsciousnessState::SelfAware);

        // MetaCognitive (>= 0.8). Raise all to cross threshold.
        cm.record_metric(AwarenessMetricType::SelfAwareness, 0.85, 0.9)
            .expect("should record SelfAwareness at 0.85");
        cm.record_metric(AwarenessMetricType::SocialAwareness, 0.85, 0.9)
            .expect("should record SocialAwareness at 0.85");
        // Now: Self=0.85, Env=0.7, Temp=0.7, Soc=0.85, Meta=0.7 → avg=0.76
        // Need one more push:
        cm.record_metric(AwarenessMetricType::EnvironmentalAwareness, 0.9, 0.9)
            .expect("should record EnvironmentalAwareness at 0.9");
        cm.record_metric(AwarenessMetricType::TemporalAwareness, 0.9, 0.9)
            .expect("should record TemporalAwareness at 0.9");
        cm.record_metric(AwarenessMetricType::MetaAwareness, 0.9, 0.9)
            .expect("should record MetaAwareness at 0.9");
        // Now: Self=0.85, Env=0.9, Temp=0.9, Soc=0.85, Meta=0.9 → avg=0.88
        assert_eq!(cm.current_state(), ConsciousnessState::MetaCognitive);
    }

    // ── 12. Reflexion generates meaningful insights ──────────────────────
    #[test]
    fn test_reflexion_insights() {
        let cm = ConsciousnessMetrics::new(ConsciousnessConfig::default());

        // Record strong awareness for one type and weak for another.
        cm.record_metric(AwarenessMetricType::SelfAwareness, 0.85, 0.9)
            .expect("should record strong SelfAwareness metric");
        cm.record_metric(AwarenessMetricType::EnvironmentalAwareness, 0.2, 0.7)
            .expect("should record weak EnvironmentalAwareness metric");
        cm.record_metric(AwarenessMetricType::TemporalAwareness, 0.55, 0.8)
            .expect("should record moderate TemporalAwareness metric");
        cm.record_metric(AwarenessMetricType::SocialAwareness, 0.0, 0.5)
            .expect("should record zero SocialAwareness metric");
        cm.record_metric(AwarenessMetricType::MetaAwareness, 0.1, 0.6)
            .expect("should record low MetaAwareness metric");

        let record = cm
            .trigger_reflexion("insight test")
            .expect("should trigger reflexion for insight test");

        // Should have insights about strong SelfAwareness and weak SocialAwareness/MetaAwareness.
        let insights_text = record.insights.join(" ");
        assert!(insights_text.contains("SelfAwareness"));
        assert!(insights_text.contains("EnvironmentalAwareness"));
        assert!(record.insights.len() >= 4);
    }
}
