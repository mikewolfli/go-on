//! BLUE38 F-GAP-25: Agency Consciousness Metrics (M10 "意识代理指标")
//!
//! Metrics that measure system self-awareness, adaptability, and autonomous
//! behavior.  All mutable state is guarded behind `Arc<Mutex<>>` for thread-safe
//! concurrent access.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

// ── Consciousness dimension ──────────────────────────────────────────────────

/// The six dimensions of agency consciousness tracked by this module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ConsciousnessDimension {
    /// Awareness of own identity, capabilities, and limitations.
    SelfAwareness,
    /// Capacity to adapt to changing environments and requirements.
    Adaptability,
    /// Ability to act independently without external direction.
    Autonomy,
    /// Clarity and persistence of goal-directed behaviour.
    GoalDirectedness,
    /// Capacity for self-reflection and recursive self-evaluation.
    Reflexivity,
    /// Ability to acquire new knowledge and improve over time.
    LearningCapacity,
}

impl ConsciousnessDimension {
    /// All six dimensions in a canonical ordering.
    pub fn all() -> [ConsciousnessDimension; 6] {
        [
            ConsciousnessDimension::SelfAwareness,
            ConsciousnessDimension::Adaptability,
            ConsciousnessDimension::Autonomy,
            ConsciousnessDimension::GoalDirectedness,
            ConsciousnessDimension::Reflexivity,
            ConsciousnessDimension::LearningCapacity,
        ]
    }
}

// ── Core data structures ────────────────────────────────────────────────────

/// A single metric observation for a consciousness dimension.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConsciousnessMetric {
    /// Which dimension this metric applies to.
    pub dimension: ConsciousnessDimension,
    /// Score in the [0.0, 1.0] range.
    pub score: f64,
    /// Confidence in the measurement, in the [0.0, 1.0] range.
    pub confidence: f64,
    /// Unix-millisecond timestamp when the metric was recorded.
    pub timestamp_ms: u64,
    /// Human-readable description of what was measured.
    pub description: String,
}

/// Self-awareness sub-metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfAwarenessMetrics {
    /// Clarity of self-identity — does the system know what it is?
    pub identity_clarity: f64,
    /// Accuracy of self-assessed capabilities vs. actual capabilities.
    pub capability_accuracy: f64,
    /// Awareness of limitations and boundaries.
    pub limitation_awareness: f64,
    /// Insight into own performance characteristics.
    pub performance_insight: f64,
}

impl SelfAwarenessMetrics {
    /// Compute the average of all four sub-metrics.
    pub fn average(&self) -> f64 {
        (self.identity_clarity
            + self.capability_accuracy
            + self.limitation_awareness
            + self.performance_insight)
            / 4.0
    }
}

/// Adaptability sub-metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdaptabilityMetrics {
    /// Speed and quality of response to changes in the environment.
    pub response_to_change: f64,
    /// Rate at which the system learns from new information.
    pub learning_speed: f64,
    /// Ability to switch strategies when current approach is failing.
    pub strategy_switching: f64,
    /// Effectiveness of recovering from errors gracefully.
    pub error_recovery: f64,
}

impl AdaptabilityMetrics {
    /// Compute the average of all four sub-metrics.
    pub fn average(&self) -> f64 {
        (self.response_to_change
            + self.learning_speed
            + self.strategy_switching
            + self.error_recovery)
            / 4.0
    }
}

/// Autonomy sub-metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutonomyMetrics {
    /// Frequency/quality of decisions made without external input.
    pub independent_decisions: f64,
    /// Actions initiated by the system on its own volition.
    pub self_initiated_actions: f64,
    /// Ability to select and prioritise its own goals.
    pub goal_self_selection: f64,
    /// Self-management of computational and memory resources.
    pub resource_self_management: f64,
}

impl AutonomyMetrics {
    /// Compute the average of all four sub-metrics.
    pub fn average(&self) -> f64 {
        (self.independent_decisions
            + self.self_initiated_actions
            + self.goal_self_selection
            + self.resource_self_management)
            / 4.0
    }
}

/// A full consciousness report generated from raw metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsciousnessReport {
    /// Unique report identifier.
    pub id: String,
    /// Unix-millisecond timestamp when the report was generated.
    pub timestamp_ms: u64,
    /// Self-awareness assessment.
    pub awareness: SelfAwarenessMetrics,
    /// Adaptability assessment.
    pub adaptability: AdaptabilityMetrics,
    /// Autonomy assessment.
    pub autonomy: AutonomyMetrics,
    /// Overall consciousness score (weighted average of all dimensions).
    pub overall_score: f64,
    /// Narrative reflection depth describing the system's state.
    pub reflection_depth: String,
    /// Actionable recommendations for improving consciousness.
    pub recommendations: Vec<String>,
}

/// Configuration for the agency consciousness tracker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsciousnessConfig {
    /// Whether metric tracking is enabled.
    #[serde(default = "default_enable_tracking")]
    pub enable_tracking: bool,
    /// Interval in milliseconds between automatic report generation (0 = manual only).
    #[serde(default)]
    pub report_interval_ms: u64,
    /// Minimum number of recorded metrics required before a report can be generated.
    #[serde(default = "default_min_data_for_report")]
    pub min_data_for_report: u32,
    /// Maximum number of historical metrics retained.
    #[serde(default = "default_max_history")]
    pub max_history: usize,
}

fn default_enable_tracking() -> bool {
    true
}
fn default_min_data_for_report() -> u32 {
    6
}
fn default_max_history() -> usize {
    500
}

impl Default for ConsciousnessConfig {
    fn default() -> Self {
        Self {
            enable_tracking: true,
            report_interval_ms: 0,
            min_data_for_report: 6,
            max_history: 500,
        }
    }
}

/// Runtime metrics snapshot of the agency consciousness tracker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsciousnessProfile {
    /// Whether the tracker is enabled.
    pub enabled: bool,
    /// Unix-millisecond timestamp of the last report, or 0.
    pub last_report_ms: u64,
    /// Current overall consciousness score.
    pub overall_score: f64,
    /// Total raw metrics recorded.
    pub total_metrics: usize,
    /// Number of distinct dimensions that have metrics.
    pub dimensions_count: usize,
    /// Total number of reports generated.
    pub reports_generated: u64,
}

// ── Internal state ──────────────────────────────────────────────────────────

#[derive(Debug)]
struct Inner {
    config: ConsciousnessConfig,
    metrics: Vec<ConsciousnessMetric>,
    reports: Vec<ConsciousnessReport>,
    next_id: u64,
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Thread-safe tracker that records and analyses agency consciousness metrics.
#[derive(Debug, Clone)]
pub struct AgencyConsciousness {
    inner: Arc<Mutex<Inner>>,
}

impl AgencyConsciousness {
    /// Create a new agency consciousness tracker with the given configuration.
    pub fn new(config: ConsciousnessConfig) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                config,
                metrics: Vec::new(),
                reports: Vec::new(),
                next_id: 1,
            })),
        }
    }

    // ── Metric recording ────────────────────────────────────────────────

    /// Record a consciousness metric for a given dimension.
    ///
    /// Returns the number of metrics currently stored after insertion (useful
    /// for callers that want to know if history has been trimmed).
    pub fn record_metric(
        &self,
        dimension: ConsciousnessDimension,
        score: f64,
        confidence: f64,
        description: &str,
    ) -> Result<usize> {
        if !(0.0..=1.0).contains(&score) {
            bail!("score must be in [0.0, 1.0], got {score}");
        }
        if !(0.0..=1.0).contains(&confidence) {
            bail!("confidence must be in [0.0, 1.0], got {confidence}");
        }

        let mut inner = self.inner.lock().unwrap();

        if !inner.config.enable_tracking {
            bail!("consciousness tracking is disabled");
        }

        let metric = ConsciousnessMetric {
            dimension,
            score,
            confidence,
            timestamp_ms: now_ms(),
            description: description.to_string(),
        };

        inner.metrics.push(metric);

        // Trim history if we exceed the maximum.
        let max = inner.config.max_history;
        if inner.metrics.len() > max {
            let excess = inner.metrics.len() - max;
            inner.metrics.drain(0..excess);
        }

        Ok(inner.metrics.len())
    }

    // ── Metric retrieval ─────────────────────────────────────────────────

    /// Get recent metrics, optionally filtered by dimension.
    ///
    /// `limit` controls how many of the most recent matching metrics to return
    /// (0 means all matching metrics).
    pub fn get_metrics(
        &self,
        dimension_filter: Option<ConsciousnessDimension>,
        limit: usize,
    ) -> Vec<ConsciousnessMetric> {
        let inner = self.inner.lock().unwrap();

        let iter: Box<dyn Iterator<Item = &ConsciousnessMetric>> = match dimension_filter {
            Some(dim) => Box::new(inner.metrics.iter().filter(move |m| m.dimension == dim)),
            None => Box::new(inner.metrics.iter()),
        };

        let mut result: Vec<ConsciousnessMetric> = iter.cloned().collect();

        // Return the *most recent* up to `limit`.
        if limit > 0 && result.len() > limit {
            result = result.split_off(result.len() - limit);
        }

        result
    }

    /// Get the latest metric for a dimension, if one exists.
    pub fn latest_metric(&self, dimension: ConsciousnessDimension) -> Option<ConsciousnessMetric> {
        let inner = self.inner.lock().unwrap();
        inner
            .metrics
            .iter()
            .rev()
            .find(|m| m.dimension == dimension)
            .cloned()
    }

    // ── Score computation ────────────────────────────────────────────────

    /// Compute a self-awareness score from the latest dimension metrics.
    ///
    /// This aggregates metrics for `SelfAwareness`, `GoalDirectedness`, and
    /// `Reflexivity` — the three dimensions most closely tied to self-awareness.
    pub fn compute_self_awareness(&self) -> f64 {
        compute_dimension_set_average(
            self,
            &[
                ConsciousnessDimension::SelfAwareness,
                ConsciousnessDimension::GoalDirectedness,
                ConsciousnessDimension::Reflexivity,
            ],
        )
    }

    /// Compute an adaptability score from the latest dimension metrics.
    ///
    /// This aggregates metrics for `Adaptability` and `LearningCapacity`.
    pub fn compute_adaptability(&self) -> f64 {
        compute_dimension_set_average(
            self,
            &[
                ConsciousnessDimension::Adaptability,
                ConsciousnessDimension::LearningCapacity,
            ],
        )
    }

    /// Compute an autonomy score from the latest dimension metrics.
    ///
    /// This uses the `Autonomy` dimension metric.
    pub fn compute_autonomy(&self) -> f64 {
        self.latest_metric(ConsciousnessDimension::Autonomy)
            .map(|m| m.score)
            .unwrap_or(0.0)
    }

    /// Compute the weighted average of all six dimensions.
    ///
    /// If no metrics exist for a dimension it contributes 0.0.
    pub fn overall_consciousness_score(&self) -> f64 {
        let dims = ConsciousnessDimension::all();
        let count = dims.len() as f64;
        let sum: f64 = dims
            .iter()
            .map(|d| self.latest_metric(*d).map(|m| m.score).unwrap_or(0.0))
            .sum();
        if count == 0.0 {
            0.0
        } else {
            sum / count
        }
    }

    /// Return a map of dimension → latest score for all six dimensions.
    pub fn dimension_breakdown(&self) -> HashMap<ConsciousnessDimension, f64> {
        let mut map = HashMap::new();
        for dim in ConsciousnessDimension::all() {
            let score = self.latest_metric(dim).map(|m| m.score).unwrap_or(0.0);
            map.insert(dim, score);
        }
        map
    }

    // ── Report generation ────────────────────────────────────────────────

    /// Generate a full consciousness report from the currently recorded metrics.
    ///
    /// Returns the report ID on success.
    pub fn generate_report(&self) -> Result<String> {
        let mut inner = self.inner.lock().unwrap();

        let total_metrics = inner.metrics.len() as u32;
        if total_metrics < inner.config.min_data_for_report {
            bail!(
                "insufficient data: have {} metrics, need {}",
                total_metrics,
                inner.config.min_data_for_report
            );
        }

        // Build sub-metrics from existing dimension data.
        let awareness = SelfAwarenessMetrics {
            identity_clarity: inner
                .metrics
                .iter()
                .rev()
                .find(|m| m.dimension == ConsciousnessDimension::SelfAwareness)
                .map(|m| m.score)
                .unwrap_or(0.0),
            capability_accuracy: inner
                .metrics
                .iter()
                .rev()
                .find(|m| m.dimension == ConsciousnessDimension::GoalDirectedness)
                .map(|m| m.score)
                .unwrap_or(0.0),
            limitation_awareness: inner
                .metrics
                .iter()
                .rev()
                .find(|m| m.dimension == ConsciousnessDimension::Reflexivity)
                .map(|m| m.score)
                .unwrap_or(0.0),
            performance_insight: inner
                .metrics
                .iter()
                .rev()
                .find(|m| m.dimension == ConsciousnessDimension::SelfAwareness)
                .map(|m| m.confidence)
                .unwrap_or(0.0),
        };

        let adaptability = AdaptabilityMetrics {
            response_to_change: inner
                .metrics
                .iter()
                .rev()
                .find(|m| m.dimension == ConsciousnessDimension::Adaptability)
                .map(|m| m.score)
                .unwrap_or(0.0),
            learning_speed: inner
                .metrics
                .iter()
                .rev()
                .find(|m| m.dimension == ConsciousnessDimension::LearningCapacity)
                .map(|m| m.score)
                .unwrap_or(0.0),
            strategy_switching: inner
                .metrics
                .iter()
                .rev()
                .find(|m| m.dimension == ConsciousnessDimension::Adaptability)
                .map(|m| m.confidence)
                .unwrap_or(0.0),
            error_recovery: inner
                .metrics
                .iter()
                .rev()
                .find(|m| m.dimension == ConsciousnessDimension::LearningCapacity)
                .map(|m| m.confidence)
                .unwrap_or(0.0),
        };

        let autonomy = AutonomyMetrics {
            independent_decisions: inner
                .metrics
                .iter()
                .rev()
                .find(|m| m.dimension == ConsciousnessDimension::Autonomy)
                .map(|m| m.score)
                .unwrap_or(0.0),
            self_initiated_actions: inner
                .metrics
                .iter()
                .rev()
                .find(|m| m.dimension == ConsciousnessDimension::Autonomy)
                .map(|m| m.confidence)
                .unwrap_or(0.0),
            goal_self_selection: inner
                .metrics
                .iter()
                .rev()
                .find(|m| m.dimension == ConsciousnessDimension::GoalDirectedness)
                .map(|m| m.score)
                .unwrap_or(0.0),
            resource_self_management: inner
                .metrics
                .iter()
                .rev()
                .find(|m| m.dimension == ConsciousnessDimension::GoalDirectedness)
                .map(|m| m.confidence)
                .unwrap_or(0.0),
        };

        // Weighted overall score: all six dimensions equally.
        let overall_score = self.compute_overall_from_inner(&inner);
        let reflection_depth = generate_reflection_text(overall_score);
        let recommendations = generate_recommendations(overall_score);

        let id = format!("consciousness-report-{}", inner.next_id);
        inner.next_id += 1;

        let report = ConsciousnessReport {
            id: id.clone(),
            timestamp_ms: now_ms(),
            awareness,
            adaptability,
            autonomy,
            overall_score,
            reflection_depth,
            recommendations,
        };

        inner.reports.push(report);
        Ok(id)
    }

    /// Get a previously generated report by ID.
    pub fn get_report(&self, id: &str) -> Result<ConsciousnessReport> {
        let inner = self.inner.lock().unwrap();
        inner
            .reports
            .iter()
            .find(|r| r.id == id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("report not found: {id}"))
    }

    /// List the most recent reports, up to the given limit.
    pub fn list_reports(&self, limit: usize) -> Vec<ConsciousnessReport> {
        let inner = self.inner.lock().unwrap();
        if limit == 0 {
            return inner.reports.clone();
        }
        let start = if inner.reports.len() > limit {
            inner.reports.len() - limit
        } else {
            0
        };
        inner.reports[start..].to_vec()
    }

    // ── Profile ─────────────────────────────────────────────────────────

    /// Return a snapshot of the tracker's runtime metrics.
    pub fn profile(&self) -> ConsciousnessProfile {
        let inner = self.inner.lock().unwrap();

        let dims_with_data = ConsciousnessDimension::all()
            .iter()
            .filter(|d| inner.metrics.iter().any(|m| m.dimension == **d))
            .count();

        ConsciousnessProfile {
            enabled: inner.config.enable_tracking,
            last_report_ms: inner.reports.last().map(|r| r.timestamp_ms).unwrap_or(0),
            overall_score: self.compute_overall_from_inner(&inner),
            total_metrics: inner.metrics.len(),
            dimensions_count: dims_with_data,
            reports_generated: inner.reports.len() as u64,
        }
    }

    // ── Internal helpers ─────────────────────────────────────────────────

    /// Compute overall consciousness score from already-locked inner state.
    fn compute_overall_from_inner(&self, inner: &Inner) -> f64 {
        let dims = ConsciousnessDimension::all();
        let count = dims.len() as f64;
        let sum: f64 = dims
            .iter()
            .map(|d| {
                inner
                    .metrics
                    .iter()
                    .rev()
                    .find(|m| m.dimension == *d)
                    .map(|m| m.score)
                    .unwrap_or(0.0)
            })
            .sum();
        if count == 0.0 {
            0.0
        } else {
            sum / count
        }
    }
}

// ── Private helpers ─────────────────────────────────────────────────────────

/// Compute the average of the latest score for each dimension in the set.
fn compute_dimension_set_average(
    consciousness: &AgencyConsciousness,
    dims: &[ConsciousnessDimension],
) -> f64 {
    if dims.is_empty() {
        return 0.0;
    }
    let sum: f64 = dims
        .iter()
        .map(|d| {
            consciousness
                .latest_metric(*d)
                .map(|m| m.score)
                .unwrap_or(0.0)
        })
        .sum();
    sum / dims.len() as f64
}

/// Generate a narrative reflection depth string based on the overall score.
fn generate_reflection_text(score: f64) -> String {
    if score >= 0.9 {
        "高度自我意识 - 系统在几乎所有的意识维度上都表现出卓越的自我认知和适应能力。".to_string()
    } else if score >= 0.75 {
        "强自我意识 - 系统具有良好的自我意识，能够有效地适应变化并自主行动。".to_string()
    } else if score >= 0.5 {
        "中等自我意识 - 系统表现出基本的自我认知，但在某些维度上仍有改进空间。".to_string()
    } else if score >= 0.25 {
        "弱自我意识 - 系统自我意识有限，需要更多的反射和适应能力提升。".to_string()
    } else {
        "极低自我意识 - 系统缺乏基本的自我认知，需要大幅提升各维度的意识水平。".to_string()
    }
}

/// Generate actionable recommendations based on the overall score.
fn generate_recommendations(score: f64) -> Vec<String> {
    let mut recs = Vec::new();
    if score < 0.5 {
        recs.push("Increase self-awareness data collection across all dimensions.".to_string());
    }
    if score < 0.7 {
        recs.push("Implement periodic reflection cycles to improve Reflexivity.".to_string());
        recs.push("Enhance learning capacity by incorporating feedback loops.".to_string());
    }
    if score < 0.85 {
        recs.push("Strengthen autonomy by allowing more independent decision-making.".to_string());
        recs.push("Improve adaptability with dynamic strategy switching mechanisms.".to_string());
    }
    if recs.is_empty() {
        recs.push(
            "All dimensions performing well; continue monitoring for regression.".to_string(),
        );
    }
    recs
}

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

    fn base_config() -> ConsciousnessConfig {
        ConsciousnessConfig {
            enable_tracking: true,
            report_interval_ms: 0,
            min_data_for_report: 3,
            max_history: 500,
        }
    }

    /// Helper to record a metric for a dimension with a given score.
    fn record(c: &AgencyConsciousness, dim: ConsciousnessDimension, score: f64) {
        c.record_metric(dim, score, 0.8, &format!("test metric for {dim:?}"))
            .unwrap();
    }

    // ── 1. Fresh consciousness tracker is empty ─────────────────────────
    #[test]
    fn test_new_consciousness_empty() {
        let c = AgencyConsciousness::new(base_config());
        assert!(c.get_metrics(None, 0).is_empty());
        assert_eq!(c.latest_metric(ConsciousnessDimension::SelfAwareness), None);
        assert!(c.list_reports(0).is_empty());

        // Overall score with no data is 0.0.
        assert!((c.overall_consciousness_score() - 0.0).abs() < 1e-9);

        let p = c.profile();
        assert!(p.enabled);
        assert_eq!(p.last_report_ms, 0);
        assert!((p.overall_score - 0.0).abs() < 1e-9);
        assert_eq!(p.total_metrics, 0);
        assert_eq!(p.dimensions_count, 0);
        assert_eq!(p.reports_generated, 0);
    }

    // ── 2. Record a metric and verify fields ────────────────────────────
    #[test]
    fn test_record_metric() {
        let c = AgencyConsciousness::new(base_config());
        let count = c
            .record_metric(
                ConsciousnessDimension::SelfAwareness,
                0.85,
                0.9,
                "Identity clarity assessment",
            )
            .unwrap();
        assert_eq!(count, 1);

        let metrics = c.get_metrics(None, 0);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].dimension, ConsciousnessDimension::SelfAwareness);
        assert!((metrics[0].score - 0.85).abs() < 1e-9);
        assert!((metrics[0].confidence - 0.9).abs() < 1e-9);
        assert_eq!(metrics[0].description, "Identity clarity assessment");
        assert!(metrics[0].timestamp_ms > 0);
    }

    // ── 3. Record metric with invalid score/confidence ──────────────────
    #[test]
    fn test_record_metric_out_of_range() {
        let c = AgencyConsciousness::new(base_config());

        // Score out of range.
        assert!(c
            .record_metric(ConsciousnessDimension::SelfAwareness, 1.5, 0.5, "bad")
            .is_err());

        // Confidence out of range.
        assert!(c
            .record_metric(ConsciousnessDimension::SelfAwareness, 0.5, -0.1, "bad")
            .is_err());

        // Tracking disabled.
        let config = ConsciousnessConfig {
            enable_tracking: false,
            ..base_config()
        };
        let c2 = AgencyConsciousness::new(config);
        assert!(c2
            .record_metric(ConsciousnessDimension::SelfAwareness, 0.5, 0.5, "disabled")
            .is_err());
    }

    // ── 4. Get metrics filtered by dimension ────────────────────────────
    #[test]
    fn test_get_metrics_by_dimension() {
        let c = AgencyConsciousness::new(base_config());

        record(&c, ConsciousnessDimension::SelfAwareness, 0.9);
        record(&c, ConsciousnessDimension::Adaptability, 0.7);
        record(&c, ConsciousnessDimension::SelfAwareness, 0.95);

        let all = c.get_metrics(None, 0);
        assert_eq!(all.len(), 3);

        let sa = c.get_metrics(Some(ConsciousnessDimension::SelfAwareness), 0);
        assert_eq!(sa.len(), 2);
        assert!(sa
            .iter()
            .all(|m| m.dimension == ConsciousnessDimension::SelfAwareness));

        let ad = c.get_metrics(Some(ConsciousnessDimension::Adaptability), 0);
        assert_eq!(ad.len(), 1);

        // Limit to 1 most recent.
        let limited = c.get_metrics(None, 1);
        assert_eq!(limited.len(), 1);
        // Most recent is the last recorded.
        assert!((limited[0].score - 0.95).abs() < 1e-9);
    }

    // ── 5. Latest metric per dimension ──────────────────────────────────
    #[test]
    fn test_latest_metric() {
        let c = AgencyConsciousness::new(base_config());

        // Record multiple metrics for the same dimension.
        record(&c, ConsciousnessDimension::Autonomy, 0.5);
        record(&c, ConsciousnessDimension::Autonomy, 0.6);
        record(&c, ConsciousnessDimension::Autonomy, 0.75);

        let latest = c.latest_metric(ConsciousnessDimension::Autonomy).unwrap();
        assert!((latest.score - 0.75).abs() < 1e-9);

        // Dimension with no data returns None.
        assert!(c
            .latest_metric(ConsciousnessDimension::Reflexivity)
            .is_none());
    }

    // ── 6. Compute self-awareness ───────────────────────────────────────
    #[test]
    fn test_compute_self_awareness() {
        let c = AgencyConsciousness::new(base_config());

        // No data → 0.0.
        assert!((c.compute_self_awareness() - 0.0).abs() < 1e-9);

        // Add SelfAwareness, GoalDirectedness, Reflexivity.
        record(&c, ConsciousnessDimension::SelfAwareness, 0.8);
        record(&c, ConsciousnessDimension::GoalDirectedness, 0.7);
        record(&c, ConsciousnessDimension::Reflexivity, 0.9);

        // Average of the three = (0.8 + 0.7 + 0.9) / 3 = 0.8.
        let sa = c.compute_self_awareness();
        assert!((sa - 0.8).abs() < 1e-9);

        // Adaptability and Autonomy should NOT affect self-awareness.
        record(&c, ConsciousnessDimension::Adaptability, 1.0);
        record(&c, ConsciousnessDimension::Autonomy, 1.0);
        let sa2 = c.compute_self_awareness();
        assert!((sa2 - 0.8).abs() < 1e-9);
    }

    // ── 7. Compute adaptability ────────────────────────────────────────
    #[test]
    fn test_compute_adaptability() {
        let c = AgencyConsciousness::new(base_config());

        // No data → 0.0.
        assert!((c.compute_adaptability() - 0.0).abs() < 1e-9);

        // Add Adaptability and LearningCapacity.
        record(&c, ConsciousnessDimension::Adaptability, 0.6);
        record(&c, ConsciousnessDimension::LearningCapacity, 0.8);

        // Average = (0.6 + 0.8) / 2 = 0.7.
        let ad = c.compute_adaptability();
        assert!((ad - 0.7).abs() < 1e-9);
    }

    // ── 8. Compute autonomy ────────────────────────────────────────────
    #[test]
    fn test_compute_autonomy() {
        let c = AgencyConsciousness::new(base_config());

        // No data → 0.0.
        assert!((c.compute_autonomy() - 0.0).abs() < 1e-9);

        record(&c, ConsciousnessDimension::Autonomy, 0.65);
        let au = c.compute_autonomy();
        assert!((au - 0.65).abs() < 1e-9);

        // Record another — uses latest.
        record(&c, ConsciousnessDimension::Autonomy, 0.9);
        let au2 = c.compute_autonomy();
        assert!((au2 - 0.9).abs() < 1e-9);
    }

    // ── 9. Generate a consciousness report ──────────────────────────────
    #[test]
    fn test_generate_report() {
        let c = AgencyConsciousness::new(base_config());

        // Not enough data (need 3, have 0).
        assert!(c.generate_report().is_err());

        // Record metrics for all needed dimensions.
        record(&c, ConsciousnessDimension::SelfAwareness, 0.8);
        record(&c, ConsciousnessDimension::Adaptability, 0.75);
        record(&c, ConsciousnessDimension::Autonomy, 0.7);

        // Still need 3, we have 3 — should succeed.
        let report_id = c.generate_report().unwrap();
        assert!(report_id.starts_with("consciousness-report-"));

        let report = c.get_report(&report_id).unwrap();
        assert_eq!(report.id, report_id);
        assert!(report.timestamp_ms > 0);
        assert!(report.overall_score > 0.0);
        assert!(!report.reflection_depth.is_empty());
        assert!(!report.recommendations.is_empty());
    }

    // ── 10. Get report ──────────────────────────────────────────────────
    #[test]
    fn test_get_report() {
        let c = AgencyConsciousness::new(base_config());

        // Generate a report.
        record(&c, ConsciousnessDimension::SelfAwareness, 0.8);
        record(&c, ConsciousnessDimension::Adaptability, 0.75);
        record(&c, ConsciousnessDimension::Autonomy, 0.7);

        let id = c.generate_report().unwrap();
        let report = c.get_report(&id).unwrap();
        assert_eq!(report.id, id);

        // Non-existent report.
        assert!(c.get_report("consciousness-report-9999").is_err());
    }

    // ── 11. Overall consciousness score ─────────────────────────────────
    #[test]
    fn test_overall_consciousness_score() {
        let c = AgencyConsciousness::new(base_config());

        // With no data, score is 0.0.
        assert!((c.overall_consciousness_score() - 0.0).abs() < 1e-9);

        // Add metrics for all six dimensions.
        record(&c, ConsciousnessDimension::SelfAwareness, 1.0);
        record(&c, ConsciousnessDimension::Adaptability, 0.8);
        record(&c, ConsciousnessDimension::Autonomy, 0.6);
        record(&c, ConsciousnessDimension::GoalDirectedness, 0.9);
        record(&c, ConsciousnessDimension::Reflexivity, 0.7);
        record(&c, ConsciousnessDimension::LearningCapacity, 0.5);

        // Average = (1.0 + 0.8 + 0.6 + 0.9 + 0.7 + 0.5) / 6 = 4.5 / 6 = 0.75.
        let overall = c.overall_consciousness_score();
        assert!((overall - 0.75).abs() < 1e-9);
    }

    // ── 12. Dimension breakdown ─────────────────────────────────────────
    #[test]
    fn test_dimension_breakdown() {
        let c = AgencyConsciousness::new(base_config());

        // Empty → all zeros.
        let breakdown = c.dimension_breakdown();
        assert_eq!(breakdown.len(), 6);
        for (_dim, score) in &breakdown {
            assert!((*score - 0.0).abs() < 1e-9);
        }

        // Add some data.
        record(&c, ConsciousnessDimension::SelfAwareness, 0.9);
        record(&c, ConsciousnessDimension::Adaptability, 0.7);
        record(&c, ConsciousnessDimension::Autonomy, 0.5);

        let breakdown = c.dimension_breakdown();
        assert_eq!(breakdown.len(), 6);
        assert!((breakdown[&ConsciousnessDimension::SelfAwareness] - 0.9).abs() < 1e-9);
        assert!((breakdown[&ConsciousnessDimension::Adaptability] - 0.7).abs() < 1e-9);
        assert!((breakdown[&ConsciousnessDimension::Autonomy] - 0.5).abs() < 1e-9);
        // Dimensions with no data are 0.0.
        assert!((breakdown[&ConsciousnessDimension::GoalDirectedness] - 0.0).abs() < 1e-9);
        assert!((breakdown[&ConsciousnessDimension::Reflexivity] - 0.0).abs() < 1e-9);
        assert!((breakdown[&ConsciousnessDimension::LearningCapacity] - 0.0).abs() < 1e-9);
    }

    // ── 13. Profile reflects state accurately ───────────────────────────
    #[test]
    fn test_profile_reflects_state() {
        let c = AgencyConsciousness::new(base_config());

        // Fresh profile.
        let p = c.profile();
        assert!(p.enabled);
        assert_eq!(p.last_report_ms, 0);
        assert_eq!(p.total_metrics, 0);
        assert_eq!(p.dimensions_count, 0);
        assert_eq!(p.reports_generated, 0);

        // Record metrics for two dimensions.
        record(&c, ConsciousnessDimension::SelfAwareness, 0.9);
        record(&c, ConsciousnessDimension::Adaptability, 0.7);

        let p = c.profile();
        assert!(p.enabled);
        assert_eq!(p.total_metrics, 2);
        assert_eq!(p.dimensions_count, 2);
        assert_eq!(p.reports_generated, 0);

        // Generate a report and check profile updates.
        record(&c, ConsciousnessDimension::Autonomy, 0.5);
        assert_eq!(c.generate_report().is_ok(), true);

        let p = c.profile();
        assert_eq!(p.total_metrics, 3);
        assert_eq!(p.dimensions_count, 3);
        assert_eq!(p.reports_generated, 1);
        assert!(p.last_report_ms > 0);
        assert!(p.overall_score > 0.0);
    }
}
