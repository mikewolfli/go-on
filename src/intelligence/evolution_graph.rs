//! F-GAP-18: Evolution Graph
//!
//! Extends the capability graph with evolution tracking: lifecycle states,
//! version history, and performance trends. This module enables monitoring
//! of agent capability maturity and automatic promotion/deprecation decisions.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ─── Evolution data types ────────────────────────────────────────────────────

/// The lifecycle stage of a capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EvolutionStage {
    /// Capability has been registered but not yet proven.
    New,
    /// Capability is actively being trained/improved.
    Learning,
    /// Capability has proven effective and is nearing stability.
    Mature,
    /// Capability is stable and reliable.
    Stable,
    /// Capability is still functional but being phased out.
    Deprecated,
    /// Capability is no longer available.
    Retired,
}

/// A single version snapshot of a capability's performance.
#[derive(Debug, Clone)]
pub struct CapabilityVersion {
    /// Semantic version string (e.g. "1.2.3").
    pub version: String,
    /// The evolution stage at which this version was recorded.
    pub stage: EvolutionStage,
    /// Unix timestamp (milliseconds) when this version was created.
    pub created_ms: u64,
    /// Success rate in range [0.0, 1.0].
    pub success_rate: f64,
    /// Average latency in milliseconds.
    pub avg_latency_ms: f64,
}

/// Direction of performance trend over recent versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrendDirection {
    /// Success rate is trending upward.
    Improving,
    /// Success rate is steady within tolerance.
    Stable,
    /// Success rate is trending downward.
    Degrading,
    /// Insufficient data to determine a trend.
    Unknown,
}

/// Full evolution record for a specific (agent, capability) pair.
#[derive(Debug, Clone)]
pub struct EvolutionRecord {
    /// The capability name.
    pub capability: String,
    /// The agent name.
    pub agent: String,
    /// Ordered list of version snapshots (oldest first).
    pub versions: Vec<CapabilityVersion>,
    /// The current lifecycle stage.
    pub current_stage: EvolutionStage,
    /// The calculated trend direction.
    pub trend: TrendDirection,
}

/// Aggregate profile of the entire evolution graph.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EvolutionProfile {
    /// Total number of registered (agent, capability) pairs.
    pub total_capabilities: usize,
    /// Count of capabilities at Mature or Stable stage.
    pub mature_count: usize,
    /// Count of capabilities with Degrading trend.
    pub degrading_count: usize,
    /// Count of capabilities at Deprecated or Retired stage.
    pub deprecated_count: usize,
}

// ─── Evolution Graph ─────────────────────────────────────────────────────────

/// In-memory evolution tracker for agent capabilities.
///
/// Maintains version history, lifecycle stages, and performance trends
/// for every (agent, capability) pair registered in the system.
#[derive(Debug)]
pub struct EvolutionGraph {
    /// Keyed by "(agent, capability)".
    records: HashMap<(String, String), EvolutionRecord>,
    /// Monotonically increasing version counter.
    version_counter: u64,
    /// Max total records before evicting the oldest.
    max_records: usize,
    /// Max versions to retain per record before evicting the oldest.
    max_versions_per_record: usize,
    /// Slope tolerance threshold below which the trend is considered Stable.
    trend_tolerance: f64,
}

impl EvolutionGraph {
    /// Create a new empty evolution graph.
    ///
    /// The graph is owned by [`crate::intelligence::capability_bus::core::CapabilityBus`]
    /// (created once in `CapabilityBus::new`); `CapabilityBus::evolve_evolution_graph`
    /// records real capability trajectories on it and `decide()` reads degrading
    /// capabilities from it. The self-evolution `EvolutionLoop` does not write
    /// to this graph (its former integration recorded hardcoded metrics and was
    /// removed as unwired dead code).
    pub fn new() -> Self {
        Self {
            records: HashMap::new(),
            version_counter: 0,
            max_records: 5000,
            max_versions_per_record: 100,
            trend_tolerance: 0.02,
        }
    }

    /// Returns the current trend tolerance.
    pub fn trend_tolerance(&self) -> f64 {
        self.trend_tolerance
    }

    /// Sets the trend tolerance threshold.
    pub fn set_trend_tolerance(&mut self, tolerance: f64) {
        self.trend_tolerance = tolerance;
    }

    /// Register a new capability for an agent at the given initial stage.
    pub fn register_capability(
        &mut self,
        agent: &str,
        capability: &str,
        initial_stage: EvolutionStage,
    ) -> Result<()> {
        let key = (agent.to_string(), capability.to_string());
        if self.records.contains_key(&key) {
            return Err(anyhow!(
                "Capability '{}' already registered for agent '{}'",
                capability,
                agent
            ));
        }
        // Evict the oldest record when at capacity.
        if self.records.len() >= self.max_records {
            if let Some(oldest_key) = self
                .records
                .iter()
                .min_by_key(|(_, rec)| rec.versions.last().map(|v| v.created_ms).unwrap_or(0))
                .map(|(k, _)| k.clone())
            {
                self.records.remove(&oldest_key);
            }
        }

        self.records.insert(
            key,
            EvolutionRecord {
                capability: capability.to_string(),
                agent: agent.to_string(),
                versions: Vec::new(),
                current_stage: initial_stage,
                trend: TrendDirection::Unknown,
            },
        );
        Ok(())
    }

    /// Record a new version snapshot for the capability and return it.
    ///
    /// `success_rate` should be in range [0.0, 1.0]. The new version's stage is
    /// set to the capability's current stage, and the trend is recalculated.
    pub fn record_version(
        &mut self,
        agent: &str,
        capability: &str,
        success_rate: f64,
        avg_latency_ms: f64,
    ) -> Result<CapabilityVersion> {
        let record = self
            .records
            .get_mut(&(agent.to_string(), capability.to_string()))
            .ok_or_else(|| {
                anyhow!(
                    "Capability '{}' not found for agent '{}'",
                    capability,
                    agent
                )
            })?;

        self.version_counter += 1;
        let version = CapabilityVersion {
            version: format!("v{}", self.version_counter),
            stage: record.current_stage,
            created_ms: crate::shared::timestamps::now_ts_ms() as u64,
            success_rate,
            avg_latency_ms,
        };

        // Evict oldest version when per-record limit is reached.
        if record.versions.len() >= self.max_versions_per_record {
            record.versions.remove(0);
        }

        record.versions.push(version.clone());
        record.trend = calculate_trend(&record.versions, self.trend_tolerance);
        Ok(version)
    }

    /// Advance the capability to a new lifecycle stage.
    ///
    /// Returns an error if the transition is invalid (e.g. Retired → Mature).
    pub fn advance_stage(
        &mut self,
        agent: &str,
        capability: &str,
        new_stage: EvolutionStage,
    ) -> Result<()> {
        let record = self
            .records
            .get_mut(&(agent.to_string(), capability.to_string()))
            .ok_or_else(|| {
                anyhow!(
                    "Capability '{}' not found for agent '{}'",
                    capability,
                    agent
                )
            })?;

        if !is_valid_transition(record.current_stage, new_stage) {
            return Err(anyhow!(
                "Invalid stage transition: {:?} → {:?} for capability '{}' of agent '{}'",
                record.current_stage,
                new_stage,
                capability,
                agent
            ));
        }

        record.current_stage = new_stage;
        Ok(())
    }

    /// Get the evolution history for a given (agent, capability) pair.
    pub fn get_history(&self, agent: &str, capability: &str) -> Option<&EvolutionRecord> {
        self.records
            .get(&(agent.to_string(), capability.to_string()))
    }

    /// Get a reference to the evolution record (BLUE48 Step 3).
    pub fn get_record(&self, agent: &str, capability: &str) -> Result<&EvolutionRecord> {
        self.records
            .get(&(agent.to_string(), capability.to_string()))
            .ok_or_else(|| {
                anyhow!(
                    "Capability '{}' not found for agent '{}'",
                    capability,
                    agent
                )
            })
    }

    /// Return all (agent, capability) keys currently tracked (BLUE48 Step 3).
    pub fn all_keys(&self) -> Vec<(String, String)> {
        self.records.keys().cloned().collect()
    }

    /// Find all capabilities with a Degrading trend.
    ///
    /// Returns a vector of `(agent, capability, trend_slope)` where trend_slope
    /// is the linear regression slope (negative means degrading).
    pub fn find_degrading_capabilities(&self) -> Vec<(String, String, f64)> {
        self.records
            .iter()
            .filter(|(_, rec)| rec.trend == TrendDirection::Degrading)
            .map(|((agent, capability), rec)| {
                let slope = linear_regression_slope(&rec.versions);
                (agent.clone(), capability.clone(), slope)
            })
            .collect()
    }

    /// Find capabilities that are candidates for promotion (from Learning → Mature).
    ///
    /// A capability is considered promotable when:
    /// - Its current stage is `Learning`
    /// - It has at least 3 versions recorded
    /// - Its trend is `Improving`
    pub fn find_candidates_for_promotion(&self) -> Vec<(String, String)> {
        self.records
            .iter()
            .filter(|(_, rec)| {
                rec.current_stage == EvolutionStage::Learning
                    && rec.versions.len() >= 3
                    && rec.trend == TrendDirection::Improving
            })
            .map(|((agent, capability), _)| (agent.clone(), capability.clone()))
            .collect()
    }

    /// Build an aggregate profile of the entire evolution graph.
    pub fn profile(&self) -> EvolutionProfile {
        let total = self.records.len();
        let mut mature = 0;
        let mut degrading = 0;
        let mut deprecated = 0;

        for rec in self.records.values() {
            match rec.current_stage {
                EvolutionStage::Mature | EvolutionStage::Stable => mature += 1,
                _ => {}
            }
            if rec.trend == TrendDirection::Degrading {
                degrading += 1;
            }
            match rec.current_stage {
                EvolutionStage::Deprecated | EvolutionStage::Retired => deprecated += 1,
                _ => {}
            }
        }

        EvolutionProfile {
            total_capabilities: total,
            mature_count: mature,
            degrading_count: degrading,
            deprecated_count: deprecated,
        }
    }
}

impl Default for EvolutionGraph {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Transition rules ────────────────────────────────────────────────────────

/// Validate a stage transition.
///
/// Allowed transitions follow a forward-only lifecycle:
/// New → Learning → Mature → Stable → Deprecated → Retired
/// Once Retired, no further transitions are allowed.
fn is_valid_transition(from: EvolutionStage, to: EvolutionStage) -> bool {
    match (from, to) {
        (EvolutionStage::New, EvolutionStage::Learning) => true,
        (EvolutionStage::Learning, EvolutionStage::Mature) => true,
        (EvolutionStage::Mature, EvolutionStage::Stable) => true,
        (EvolutionStage::Stable, EvolutionStage::Deprecated) => true,
        (EvolutionStage::Deprecated, EvolutionStage::Retired) => true,
        // No transitions from Retired.
        (EvolutionStage::Retired, _) => false,
        // Also allow staying in the same stage (re-registration / no-op).
        (a, b) if a == b => true,
        _ => false,
    }
}

// ─── Trend calculation ──────────────────────────────────────────────────────

/// Calculate the trend direction from a list of versions.
///
/// Uses simple linear regression on the success rate over time.
/// Returns `Unknown` when there are fewer than 2 data points.
fn calculate_trend(versions: &[CapabilityVersion], tolerance: f64) -> TrendDirection {
    if versions.len() < 2 {
        return TrendDirection::Unknown;
    }
    let slope = linear_regression_slope(versions);

    if slope > tolerance {
        TrendDirection::Improving
    } else if slope < -tolerance {
        TrendDirection::Degrading
    } else {
        TrendDirection::Stable
    }
}

/// Compute the linear regression slope of success rate vs. `created_ms` timestamps.
///
/// Uses real timestamps for the x-axis. If all timestamps are identical
/// (e.g. in tests), falls back to using version index (0..n).
/// Returns the slope coefficient. A positive value means improving,
/// negative means degrading.
fn linear_regression_slope(versions: &[CapabilityVersion]) -> f64 {
    let n = versions.len() as f64;
    if n < 2.0 {
        return 0.0;
    }

    // Use timestamps; if they are all identical, fall back to index.
    let use_timestamps = {
        let min_ts = versions.iter().map(|v| v.created_ms).min().unwrap_or(0);
        let max_ts = versions.iter().map(|v| v.created_ms).max().unwrap_or(0);
        min_ts != max_ts
    };

    let x_vals: Vec<f64> = if use_timestamps {
        versions.iter().map(|v| v.created_ms as f64).collect()
    } else {
        (0..versions.len()).map(|i| i as f64).collect()
    };
    let rates: Vec<f64> = versions.iter().map(|v| v.success_rate).collect();

    let mean_x = x_vals.iter().sum::<f64>() / n;
    let mean_y = rates.iter().sum::<f64>() / n;

    let mut num = 0.0;
    let mut den = 0.0;
    for (x, y) in x_vals.iter().zip(rates.iter()) {
        let dx = x - mean_x;
        num += dx * (y - mean_y);
        den += dx * dx;
    }

    if den == 0.0 {
        0.0
    } else {
        num / den
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── 1 ────────────────────────────────────────────────────────────────────

    // ── 1 ────────────────────────────────────────────────────────────────────

    #[test]
    fn test_register_capability() {
        let mut graph = EvolutionGraph::new();
        graph
            .register_capability("agent_a", "code_review", EvolutionStage::New)
            .expect("register_capability for agent_a/code_review should succeed");
        let record = graph.get_history("agent_a", "code_review");
        assert!(record.is_some());
        let r = record.expect("get_history should return a record for agent_a/code_review");
        assert_eq!(r.agent, "agent_a");
        assert_eq!(r.capability, "code_review");
        assert_eq!(r.current_stage, EvolutionStage::New);
        assert!(r.versions.is_empty());
    }

    // ── 3 ────────────────────────────────────────────────────────────────────

    #[test]
    fn test_record_version_creates_history() {
        let mut graph = EvolutionGraph::new();
        graph
            .register_capability("agent_a", "code_review", EvolutionStage::Learning)
            .expect("register_capability for agent_a/code_review should succeed");

        let v1 = graph
            .record_version("agent_a", "code_review", 0.85, 120.0)
            .expect("first record_version for agent_a/code_review should succeed");
        assert_eq!(v1.version, "v1");
        assert_eq!(v1.success_rate, 0.85);
        assert_eq!(v1.stage, EvolutionStage::Learning);

        let v2 = graph
            .record_version("agent_a", "code_review", 0.90, 110.0)
            .expect("second record_version for agent_a/code_review should succeed");
        assert_eq!(v2.version, "v2");

        let record = graph.get_history("agent_a", "code_review").expect(
            "get_history for agent_a/code_review should return a record after recording versions",
        );
        assert_eq!(record.versions.len(), 2);
    }

    // ── 4 ────────────────────────────────────────────────────────────────────

    #[test]
    fn test_advance_stage() {
        let mut graph = EvolutionGraph::new();
        graph
            .register_capability("agent_b", "translation", EvolutionStage::New)
            .expect("register_capability for agent_b/translation should succeed");

        graph
            .advance_stage("agent_b", "translation", EvolutionStage::Learning)
            .expect("advance_stage from New to Learning should succeed");
        let record = graph
            .get_history("agent_b", "translation")
            .expect("get_history for agent_b/translation should return a record");
        assert_eq!(record.current_stage, EvolutionStage::Learning);

        graph
            .advance_stage("agent_b", "translation", EvolutionStage::Mature)
            .expect("advance_stage from Learning to Mature should succeed");
        let record = graph.get_history("agent_b", "translation").expect(
            "get_history for agent_b/translation should return a record after second advance",
        );
        assert_eq!(record.current_stage, EvolutionStage::Mature);
    }

    // ── 5 ────────────────────────────────────────────────────────────────────

    #[test]
    fn test_advance_stage_invalid_transition() {
        let mut graph = EvolutionGraph::new();
        graph
            .register_capability("agent_c", "qa_testing", EvolutionStage::Retired)
            .expect("register_capability for agent_c/qa_testing should succeed");

        let result = graph.advance_stage("agent_c", "qa_testing", EvolutionStage::Mature);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Invalid stage transition"));
        assert!(err.contains("Retired"));

        // Verify stage did not change.
        let record = graph
            .get_history("agent_c", "qa_testing")
            .expect("get_history for agent_c/qa_testing should return a record");
        assert_eq!(record.current_stage, EvolutionStage::Retired);
    }

    // ── 6 ────────────────────────────────────────────────────────────────────

    #[test]
    fn test_duplicate_registration_fails() {
        let mut graph = EvolutionGraph::new();
        graph
            .register_capability("agent_d", "sentiment", EvolutionStage::New)
            .expect("register_capability for agent_d/sentiment should succeed on first call");

        let result = graph.register_capability("agent_d", "sentiment", EvolutionStage::New);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("already registered"));
    }

    // ── 7 ────────────────────────────────────────────────────────────────────

    #[test]
    fn test_find_degrading_capabilities() {
        let mut graph = EvolutionGraph::new();

        // Register two capabilities: one degrading, one improving.
        graph
            .register_capability("agent_e", "summarizer", EvolutionStage::Learning)
            .expect("register_capability for agent_e/summarizer should succeed");
        graph
            .register_capability("agent_e", "extractor", EvolutionStage::Learning)
            .expect("register_capability for agent_e/extractor should succeed");

        // Degrading: success rates go down.
        graph
            .record_version("agent_e", "summarizer", 0.95, 100.0)
            .expect("first record_version for agent_e/summarizer (degrading) should succeed");
        graph
            .record_version("agent_e", "summarizer", 0.85, 105.0)
            .expect("second record_version for agent_e/summarizer (degrading) should succeed");
        graph
            .record_version("agent_e", "summarizer", 0.75, 110.0)
            .expect("third record_version for agent_e/summarizer (degrading) should succeed");

        // Improving: success rates go up.
        graph
            .record_version("agent_e", "extractor", 0.70, 200.0)
            .expect("first record_version for agent_e/extractor (improving) should succeed");
        graph
            .record_version("agent_e", "extractor", 0.80, 190.0)
            .expect("second record_version for agent_e/extractor (improving) should succeed");
        graph
            .record_version("agent_e", "extractor", 0.90, 180.0)
            .expect("third record_version for agent_e/extractor (improving) should succeed");

        let degrading = graph.find_degrading_capabilities();
        assert_eq!(degrading.len(), 1);
        assert_eq!(degrading[0].0, "agent_e");
        assert_eq!(degrading[0].1, "summarizer");
        assert!(degrading[0].2 < 0.0); // negative slope
    }

    // ── 8 ────────────────────────────────────────────────────────────────────

    #[test]
    fn test_find_candidates_for_promotion() {
        let mut graph = EvolutionGraph::new();

        // Register two capabilities: one promotable, one not.
        graph
            .register_capability("agent_f", "classifier", EvolutionStage::Learning)
            .expect("register_capability for agent_f/classifier should succeed");
        graph
            .register_capability("agent_f", "clusterer", EvolutionStage::Learning)
            .expect("register_capability for agent_f/clusterer should succeed");

        // promotable: 3 improving versions.
        for rate in [0.70, 0.80, 0.90] {
            graph
                .record_version("agent_f", "classifier", rate, 50.0)
                .expect("record_version for agent_f/classifier (promotable) should succeed");
        }

        // not promotable: only 2 versions (minimum is 3).
        graph
            .record_version("agent_f", "clusterer", 0.80, 60.0)
            .expect("first record_version for agent_f/clusterer (not promotable) should succeed");
        graph
            .record_version("agent_f", "clusterer", 0.85, 55.0)
            .expect("second record_version for agent_f/clusterer (not promotable) should succeed");

        let candidates = graph.find_candidates_for_promotion();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].0, "agent_f");
        assert_eq!(candidates[0].1, "classifier");
    }

    // ── 9 ────────────────────────────────────────────────────────────────────

    #[test]
    fn test_profile_reflects_state() {
        let mut graph = EvolutionGraph::new();

        // Mature capability.
        graph
            .register_capability("agent_g", "matcher", EvolutionStage::Mature)
            .expect("register_capability for agent_g/matcher should succeed");
        graph
            .record_version("agent_g", "matcher", 0.95, 10.0)
            .expect("record_version for agent_g/matcher should succeed");

        // Deprecated capability.
        graph
            .register_capability("agent_g", "legacy", EvolutionStage::Deprecated)
            .expect("register_capability for agent_g/legacy should succeed");

        // Learning + Degrading capability.
        graph
            .register_capability("agent_h", "failing", EvolutionStage::Learning)
            .expect("register_capability for agent_h/failing should succeed");
        graph
            .record_version("agent_h", "failing", 0.90, 30.0)
            .expect("first record_version for agent_h/failing should succeed");
        graph
            .record_version("agent_h", "failing", 0.80, 35.0)
            .expect("second record_version for agent_h/failing should succeed");
        graph
            .record_version("agent_h", "failing", 0.70, 40.0)
            .expect("third record_version for agent_h/failing should succeed");

        let p = graph.profile();
        assert_eq!(p.total_capabilities, 3);
        assert_eq!(p.mature_count, 1);
        assert_eq!(p.degrading_count, 1);
        assert_eq!(p.deprecated_count, 1);
    }

    // ── 10 ───────────────────────────────────────────────────────────────────

    #[test]
    fn test_trend_calculation_improving() {
        let mut graph = EvolutionGraph::new();
        graph
            .register_capability("agent_i", "improving_skill", EvolutionStage::Learning)
            .expect("register_capability for agent_i/improving_skill should succeed");

        for rate in [0.60, 0.70, 0.80, 0.90, 0.95] {
            graph
                .record_version("agent_i", "improving_skill", rate, 100.0)
                .expect(
                    "record_version for agent_i/improving_skill (improving trend) should succeed",
                );
        }

        let record = graph
            .get_history("agent_i", "improving_skill")
            .expect("get_history for agent_i/improving_skill should return a record");
        assert_eq!(record.trend, TrendDirection::Improving);
    }

    // ── 11 ───────────────────────────────────────────────────────────────────

    #[test]
    fn test_trend_calculation_degrading() {
        let mut graph = EvolutionGraph::new();
        graph
            .register_capability("agent_j", "degrading_skill", EvolutionStage::Learning)
            .expect("register_capability for agent_j/degrading_skill should succeed");

        for rate in [0.95, 0.85, 0.75, 0.65] {
            graph
                .record_version("agent_j", "degrading_skill", rate, 100.0)
                .expect(
                    "record_version for agent_j/degrading_skill (degrading trend) should succeed",
                );
        }

        let record = graph
            .get_history("agent_j", "degrading_skill")
            .expect("get_history for agent_j/degrading_skill should return a record");
        assert_eq!(record.trend, TrendDirection::Degrading);
    }

    // ── 12 ───────────────────────────────────────────────────────────────────

    #[test]
    fn test_get_history_nonexistent_returns_none() {
        let graph = EvolutionGraph::new();
        assert!(graph.get_history("nonexistent", "missing_cap").is_none());
    }
}
