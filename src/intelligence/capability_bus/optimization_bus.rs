//! OptimizationBus — Unified optimization sub-bus (BLUE38 §1, ARCH-13 multi-bus architecture)
//!
//! OptimizationBus wraps the CostOptimizer, SpeedOptimizer, the unified
//! hyper-resilience engine (circuit breakers + health + degradation, formerly
//! `FailurePrevention`), ReliabilityOptimizer and WorkflowOptimizer into a
//! single, unified sub-bus that exposes optimization recommendations,
//! circuit-breaker queries, and execution feedback to the CapabilityBus
//! coordinator.
//!
//! # Architecture
//!
//! ```text
//!                  CapabilityBus (scheduling coordinator)
//!                              │
//!                    OptimizationBus (this module)
//!          ┌───────────┬───────┼───────┬───────────┐
//!          │           │       │       │           │
//!      CostOptimizer  Speed  Failure  Reliability Workflow
//!                    Opt.   Prevent.   Opt.       Opt.
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::resilience::hyper_resilience::{HyperResilienceEngine, ResilienceConfig};

// ---------------------------------------------------------------------------
// Supporting types
// ---------------------------------------------------------------------------

/// Profile / diagnostic snapshot of the optimisation bus state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizationBusProfile {
    /// Whether the bus is currently enabled.
    pub enabled: bool,
    /// Total number of optimisation decisions made since creation.
    pub total_optimizations: u64,
    /// Rough estimate of cumulative cost savings (arbitrary units).
    pub cost_savings_estimate: f64,
    /// Number of times a speed improvement was recommended.
    pub speed_improvements: u64,
    /// Number of times a circuit breaker was tripped.
    pub circuit_breaker_trips: u64,
    /// Number of reliability flags raised.
    pub reliability_flags: u64,
}

impl Default for OptimizationBusProfile {
    fn default() -> Self {
        Self {
            enabled: true,
            total_optimizations: 0,
            cost_savings_estimate: 0.0,
            speed_improvements: 0,
            circuit_breaker_trips: 0,
            reliability_flags: 0,
        }
    }
}

/// A single optimisation recommendation produced by the bus.
#[derive(Debug, Clone)]
pub struct OptimizationRecommendation {
    /// Suggested agent identifier (or `None` if no reroute is needed).
    pub suggested_agent: Option<String>,
    /// Estimated monetary / token cost for the recommended action.
    pub estimated_cost: f64,
    /// Estimated duration in milliseconds.
    pub estimated_duration_ms: u64,
    /// Reliability score in [0.0, 1.0] (1.0 = most reliable).
    pub reliability_score: f64,
    /// Confidence in this recommendation in [0.0, 1.0].
    pub confidence: f64,
}

impl OptimizationRecommendation {
    /// A neutral / no-change recommendation.
    pub fn no_op() -> Self {
        Self {
            suggested_agent: None,
            estimated_cost: 0.0,
            estimated_duration_ms: 0,
            reliability_score: 0.5,
            confidence: 0.0,
        }
    }
}

// ---------------------------------------------------------------------------
// Lightweight delegating wrappers that forward to the core optimization
// primitives (workflow CostOptimizer, TokenOptimizer, etc.).
// ---------------------------------------------------------------------------

/// Simple cost estimator with per-agent base costs.
struct CostEstimator {
    base_costs: HashMap<String, f64>,
}

impl CostEstimator {
    fn new() -> Self {
        let mut base_costs = HashMap::new();
        Self::insert_bounded(&mut base_costs, "claude-sonnet-4".to_string(), 3.00, 500);
        Self::insert_bounded(&mut base_costs, "claude-haiku".to_string(), 0.80, 500);
        Self::insert_bounded(&mut base_costs, "gpt-4o".to_string(), 2.50, 500);
        Self::insert_bounded(&mut base_costs, "gpt-4o-mini".to_string(), 0.60, 500);

        Self { base_costs }
    }

    #[inline]
    fn insert_bounded(map: &mut HashMap<String, f64>, key: String, value: f64, max: usize) {
        if map.len() >= max {
            if let Some(oldest) = map.keys().next().cloned() {
                map.remove(&oldest);
            }
        }
        map.insert(key, value);
    }

    #[inline]
    fn insert_bounded_u64(map: &mut HashMap<String, u64>, key: String, value: u64, max: usize) {
        if map.len() >= max {
            if let Some(oldest) = map.keys().next().cloned() {
                map.remove(&oldest);
            }
        }
        map.insert(key, value);
    }

    /// Estimate the cost (in arbitrary units) for a given agent and token count.
    fn estimate_cost(&self, agent: &str, token_count: u64) -> f64 {
        let cost_per_1k = self.base_costs.get(agent).copied().unwrap_or(1.0);
        (token_count as f64 / 1000.0) * cost_per_1k
    }

    /// Suggest cost-optimised agent (delegates to workflow CostOptimizer logic).
    fn suggest_cheaper_agent(&self, _task_type: &str, token_count: u64) -> Option<String> {
        // Honest semantics: this function always builds an empty
        // OptimizationContext (no real execution history is wired in), which
        // makes CostOptimizer report success_rate=1.0 and always answer
        // "downgrade_model_tier". Returning the hard-coded "claude-haiku"
        // from that degenerate input would be a fabricated recommendation, so
        // a no-op is returned and the caller falls back to its default agent.
        // Real cost recommendations require actual run history, fed via
        // `persist_workflow_learning_event`.
        let _ = token_count;
        None
    }
}

/// Simple speed estimator with base latencies per agent.
struct SpeedEstimator {
    base_latencies: HashMap<String, u64>,
}

impl SpeedEstimator {
    fn new() -> Self {
        let mut base_latencies = HashMap::new();
        CostEstimator::insert_bounded_u64(
            &mut base_latencies,
            "claude-sonnet-4".to_string(),
            1200,
            500,
        );
        CostEstimator::insert_bounded_u64(
            &mut base_latencies,
            "claude-haiku".to_string(),
            400,
            500,
        );
        CostEstimator::insert_bounded_u64(&mut base_latencies, "gpt-4o".to_string(), 800, 500);
        CostEstimator::insert_bounded_u64(&mut base_latencies, "gpt-4o-mini".to_string(), 350, 500);

        Self { base_latencies }
    }

    /// Estimate the latency (ms) for a given agent and token count.
    fn estimate_latency(&self, agent: &str, token_count: u64) -> u64 {
        let base = self.base_latencies.get(agent).copied().unwrap_or(1000);
        let overhead = (token_count / 100).saturating_mul(10);
        base + overhead
    }

    /// Suggest the fastest agent for a task.
    fn suggest_fastest_agent(&self) -> Option<String> {
        self.base_latencies
            .iter()
            .min_by_key(|(_, &lat)| lat)
            .map(|(agent, _)| agent.clone())
    }
}

// ---------------------------------------------------------------------------
// Lightweight delegating wrappers for the bus's five sub-optimisers.
// ---------------------------------------------------------------------------

/// Wraps the unified `HyperResilienceEngine` for the optimization bus.
///
/// The engine is the single resilience authority (breakers + health +
/// degradation); the bus is a thin read/write facade over it.
struct BusResilience {
    inner: Arc<HyperResilienceEngine>,
}

impl BusResilience {
    fn new() -> Self {
        Self {
            inner: Arc::new(HyperResilienceEngine::new(ResilienceConfig::default())),
        }
    }

    fn is_circuit_broken(&self, agent: &str) -> bool {
        matches!(
            self.inner.breaker_state(agent),
            crate::resilience::hyper_resilience::CircuitState::Open
        )
    }

    fn record_outcome(&self, agent: &str, duration_ms: u64, success: bool) {
        self.inner.record_outcome(agent, success, duration_ms);
    }

    fn circuit_breaker_trips(&self) -> u64 {
        // Count how many agents currently have an open circuit breaker.
        self.inner
            .health_report()
            .iter()
            .filter(|h| {
                matches!(
                    self.inner.breaker_state(&h.service_name),
                    crate::resilience::hyper_resilience::CircuitState::Open
                )
            })
            .count() as u64
    }
}

/// Lightweight reliability scorer.
struct ReliabilityOptimizer {
    reliability_scores: HashMap<String, f64>,
}

impl ReliabilityOptimizer {
    fn new() -> Self {
        let mut scores = HashMap::new();
        CostEstimator::insert_bounded(&mut scores, "claude-sonnet-4".to_string(), 0.98, 500);
        CostEstimator::insert_bounded(&mut scores, "claude-haiku".to_string(), 0.92, 500);
        CostEstimator::insert_bounded(&mut scores, "gpt-4o".to_string(), 0.95, 500);
        CostEstimator::insert_bounded(&mut scores, "gpt-4o-mini".to_string(), 0.88, 500);

        Self {
            reliability_scores: scores,
        }
    }

    fn score(&self, agent: &str) -> f64 {
        self.reliability_scores.get(agent).copied().unwrap_or(0.5)
    }

    fn suggest_most_reliable(&self) -> Option<String> {
        self.reliability_scores
            .iter()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(agent, _)| agent.clone())
    }
}

/// Lightweight workflow speed-score calculator.
struct SpeedOptimizer {
    estimator: SpeedEstimator,
}

impl SpeedOptimizer {
    fn new() -> Self {
        Self {
            estimator: SpeedEstimator::new(),
        }
    }

    fn estimate_duration(&self, agent: &str, token_count: u64) -> u64 {
        self.estimator.estimate_latency(agent, token_count)
    }

    fn fastest_agent(&self) -> Option<String> {
        self.estimator.suggest_fastest_agent()
    }
}

// ---------------------------------------------------------------------------
// OptimizationBus – public API
// ---------------------------------------------------------------------------

/// Unified optimization sub-bus that exposes cost, speed, failure-prevention,
/// reliability, and workflow optimisation through a single interface.
pub struct OptimizationBus {
    /// Cost optimizer reference
    cost_optimizer: Arc<Mutex<CostEstimator>>,
    /// Speed optimizer reference
    speed_optimizer: Arc<Mutex<SpeedOptimizer>>,
    /// Unified resilience facade (circuit breakers + health + degradation)
    resilience: BusResilience,
    /// Reliability optimizer reference
    reliability_optimizer: Arc<Mutex<ReliabilityOptimizer>>,
    /// Profile metrics
    profile: Arc<Mutex<OptimizationBusProfile>>,
}

impl OptimizationBus {
    /// Create a new `OptimizationBus` with default optimizers.
    pub fn new() -> Self {
        Self {
            cost_optimizer: Arc::new(Mutex::new(CostEstimator::new())),
            speed_optimizer: Arc::new(Mutex::new(SpeedOptimizer::new())),
            resilience: BusResilience::new(),
            reliability_optimizer: Arc::new(Mutex::new(ReliabilityOptimizer::new())),
            profile: Arc::new(Mutex::new(OptimizationBusProfile::default())),
        }
    }

    /// Get optimization recommendations for a task.
    ///
    /// Examines the task type, estimated token count, and priority to produce
    /// a single `OptimizationRecommendation` that balances cost, speed, and
    /// reliability.
    pub fn recommend(
        &self,
        task_type: &str,
        token_count: u64,
        priority: &str,
    ) -> OptimizationRecommendation {
        let cost = self
            .cost_optimizer
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let speed = self
            .speed_optimizer
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let reliability = self
            .reliability_optimizer
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut profile = self.profile.lock().unwrap_or_else(|e| e.into_inner());

        profile.total_optimizations = profile.total_optimizations.wrapping_add(1);

        // Determine candidate agent based on priority.
        // Production callers pass "high" (full-auto mode) and "balanced"
        // (default). Previously those fell through to `_ => None`, so every
        // recommendation returned the hard-coded claude-sonnet-4 fallback and
        // the whole hot-path call produced no decision input. Map them to the
        // real optimizers: "high" prefers the most reliable known agent,
        // "balanced" prefers the lowest-latency known agent.
        let suggested_agent = match priority {
            "cost" => cost.suggest_cheaper_agent(task_type, token_count),
            "speed" => speed.fastest_agent(),
            "reliability" => reliability.suggest_most_reliable(),
            "high" => reliability.suggest_most_reliable(),
            "balanced" => speed.fastest_agent(),
            _ => None, // unknown priority — let the caller decide.
        };

        let agent = suggested_agent.as_deref().unwrap_or("claude-sonnet-4");

        let estimated_cost = cost.estimate_cost(agent, token_count);
        let estimated_duration_ms = speed.estimate_duration(agent, token_count);
        let reliability_score = reliability.score(agent);

        let confidence = match priority {
            "cost" => 0.75,
            "speed" => 0.80,
            "reliability" => 0.85,
            "high" => 0.85,
            "balanced" => 0.70,
            _ => 0.60,
        };

        // Track speed improvements.
        if priority == "speed" && suggested_agent.is_some() {
            profile.speed_improvements = profile.speed_improvements.wrapping_add(1);
        }

        OptimizationRecommendation {
            suggested_agent,
            estimated_cost,
            estimated_duration_ms,
            reliability_score,
            confidence,
        }
    }

    /// Check if an agent is currently circuit-broken (open circuit).
    pub fn is_circuit_broken(&self, agent: &str) -> bool {
        let broken = self.resilience.is_circuit_broken(agent);
        if broken {
            let mut profile = self.profile.lock().unwrap_or_else(|e| e.into_inner());
            profile.circuit_breaker_trips = profile.circuit_breaker_trips.wrapping_add(1);
        }
        broken
    }

    /// Record execution results for optimizer feedback.
    ///
    /// Feeds the outcome back into the failure-prevention subsystem so that
    /// future circuit-breaker and recommendation decisions are informed by
    /// real execution data.
    pub fn record_execution(&self, agent: &str, duration_ms: u64, _token_cost: u64, success: bool) {
        self.resilience.record_outcome(agent, duration_ms, success);

        // If this was a failure, log a reliability flag.
        if !success {
            let mut profile = self.profile.lock().unwrap_or_else(|e| e.into_inner());
            profile.reliability_flags = profile.reliability_flags.wrapping_add(1);
        }
    }

    /// Return a snapshot of the current bus profile.
    pub fn profile(&self) -> OptimizationBusProfile {
        let profile = self.profile.lock().unwrap_or_else(|e| e.into_inner());
        let trips = self.resilience.circuit_breaker_trips();
        OptimizationBusProfile {
            circuit_breaker_trips: trips,
            ..profile.clone()
        }
    }
}

impl Default for OptimizationBus {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recommend_cost_priority() {
        let bus = OptimizationBus::new();
        let rec = bus.recommend("code_gen", 10_000, "cost");
        // Cost optimisation should suggest a cheaper agent.
        assert!(rec.estimated_cost > 0.0);
        // Confidence for cost priority.
        assert!((rec.confidence - 0.75).abs() < 1e-9);
    }

    #[test]
    fn test_recommend_speed_priority() {
        let bus = OptimizationBus::new();
        let rec = bus.recommend("code_gen", 10_000, "speed");
        // Speed optimisation should pick the fastest agent (gpt-4o-mini at 350ms base).
        assert!(rec.estimated_duration_ms > 0);
        // Confidence for speed priority.
        assert!((rec.confidence - 0.80).abs() < 1e-9);
    }

    #[test]
    fn test_recommend_reliability_priority() {
        let bus = OptimizationBus::new();
        let rec = bus.recommend("code_gen", 10_000, "reliability");
        // Reliability optimisation should produce a 0.98 score (claude-sonnet-4).
        assert!((rec.reliability_score - 0.98).abs() < 1e-9);
        // Confidence for reliability priority.
        assert!((rec.confidence - 0.85).abs() < 1e-9);
    }

    #[test]
    fn test_recommend_balanced_priority() {
        let bus = OptimizationBus::new();
        let rec = bus.recommend("code_gen", 5_000, "balanced");
        // Balanced maps to the lowest-latency known agent (real suggestion,
        // not None): costs/duration are computed for the suggested agent.
        assert!(rec.suggested_agent.is_some());
        assert!(rec.estimated_cost > 0.0);
        assert!(rec.estimated_duration_ms > 0);
        assert!((rec.confidence - 0.70).abs() < 1e-9);
    }

    #[test]
    fn test_profile_increments() {
        let bus = OptimizationBus::new();

        let _rec = bus.recommend("chat", 1_000, "cost");
        let p = bus.profile();
        assert_eq!(p.total_optimizations, 1);

        let _rec = bus.recommend("chat", 1_000, "speed");
        let p = bus.profile();
        assert_eq!(p.total_optimizations, 2);
        // Speed priority also increments speed_improvements.
        assert!(p.speed_improvements >= 1);
    }

    #[test]
    fn test_circuit_breaker_trips_after_failures() {
        let bus = OptimizationBus::new();
        // Record multiple failures to trip the circuit breaker.
        for _ in 0..6 {
            bus.record_execution("fragile-agent", 2000, 500, false);
        }
        assert!(bus.is_circuit_broken("fragile-agent"));

        let p = bus.profile();
        assert!(p.circuit_breaker_trips > 0);
        assert!(p.reliability_flags >= 6);
    }

    #[test]
    fn test_default_impl() {
        let bus = OptimizationBus::default();
        assert!(bus.profile().enabled);
    }
}
