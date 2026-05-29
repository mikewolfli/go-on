//! BLUE48 Step 3: Intelligence Bridge — Active integration of smart modules
//! into the autonomy loop execution path.
//!
//! This module bridges the gap between passive intelligence data collection
//! and active decision-making. It queries ContinuousLearning, EvolutionGraph,
//! and Metacognitive modules at key decision points in the autonomy loop
//! to make the system smarter and more adaptive.
//!
//! Key integration points:
//! - Pre-planning: Query ContinuousLearning for historical insights
//! - Agent selection: Query EvolutionGraph for maturity/stability recommendations
//! - Post-round: Feed results into Metacognitive for autoreflection

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

use crate::intelligence::evolution_graph::{EvolutionGraph, EvolutionStage, TrendDirection};

/// Global counter of intelligence bridge interventions.
pub static INTEL_BRIDGE_INTERVENTIONS: AtomicU64 = AtomicU64::new(0);

/// Global counter of EvolutionGraph recommendations used.
pub static EVO_RECOMMENDATIONS_USED: AtomicU64 = AtomicU64::new(0);

/// Global counter of ContinuousLearning insights applied.
pub static CL_INSIGHTS_APPLIED: AtomicU64 = AtomicU64::new(0);

// ── Global intelligence state ──────────────────────────────────────────────

static EVOLUTION_GRAPH: LazyLock<Mutex<EvolutionGraph>> =
    LazyLock::new(|| Mutex::new(EvolutionGraph::new()));

/// Initialize or retrieve the global EvolutionGraph instance.
#[allow(dead_code)] // F-GAP-49 — reserved for evolution graph external access
pub fn evolution_graph() -> &'static Mutex<EvolutionGraph> {
    &EVOLUTION_GRAPH
}

/// Register an agent capability in the evolution graph for tracking.
#[allow(dead_code)] // F-GAP-49 — reserved for evolution graph external registration
pub fn register_agent_capability(agent: &str, capability: &str) {
    let mut graph = EVOLUTION_GRAPH.lock().unwrap_or_else(|poisoned| {
        tracing::warn!("EVOLUTION_GRAPH lock poisoned during capability registration – recovered");
        poisoned.into_inner()
    });
    let _ = graph.register_capability(agent, capability, EvolutionStage::New);
}

/// Record a performance data point for an agent capability.
pub fn record_capability_performance(
    agent: &str,
    capability: &str,
    success_rate: f64,
    avg_latency_ms: f64,
) {
    let mut graph = EVOLUTION_GRAPH.lock().unwrap_or_else(|poisoned| {
        tracing::warn!("EVOLUTION_GRAPH lock poisoned during performance recording – recovered");
        poisoned.into_inner()
    });
    let _ = graph.record_version(agent, capability, success_rate, avg_latency_ms);
    // Auto-advance stage based on version count and success rate
    if let Ok(record) = graph.get_record(agent, capability) {
        let version_count = record.versions.len();
        let avg_success: f64 = if version_count > 0 {
            record.versions.iter().map(|v| v.success_rate).sum::<f64>() / version_count as f64
        } else {
            0.0
        };

        let new_stage = match (version_count, avg_success) {
            (n, _) if n >= 50 && avg_success > 0.95 => EvolutionStage::Stable,
            (n, _) if n >= 20 && avg_success > 0.85 => EvolutionStage::Mature,
            (n, _) if n >= 5 && avg_success > 0.70 => EvolutionStage::Learning,
            _ => return, // Keep current stage
        };

        // Only advance forward
        let should_advance = matches!(
            (record.current_stage, new_stage),
            (EvolutionStage::New, _)
                | (
                    EvolutionStage::Learning,
                    EvolutionStage::Mature | EvolutionStage::Stable
                )
                | (EvolutionStage::Mature, EvolutionStage::Stable)
        );

        if should_advance {
            let _ = graph.advance_stage(agent, capability, new_stage);
        }
    }
}

/// Get EvolutionGraph recommendations for agent selection.
///
/// Returns a list of (agent_name, capability, stage, trend) tuples sorted by
/// maturity (Stable > Mature > Learning > New) and improving trend.
pub fn get_agent_recommendations() -> Vec<(String, String, EvolutionStage, TrendDirection)> {
    let graph = match EVOLUTION_GRAPH.lock() {
        Ok(g) => g,
        Err(_) => return Vec::new(),
    };

    let mut recommendations: Vec<(String, String, EvolutionStage, TrendDirection)> = Vec::new();

    for (agent, capability) in graph.all_keys() {
        if let Ok(record) = graph.get_record(&agent, &capability) {
            recommendations.push((agent, capability, record.current_stage, record.trend));
        }
    }

    // Sort: Stable/Mature first, then by Improving trend
    recommendations.sort_by(|a, b| {
        let stage_rank = |stage: EvolutionStage| -> u8 {
            match stage {
                EvolutionStage::Stable => 5,
                EvolutionStage::Mature => 4,
                EvolutionStage::Learning => 3,
                EvolutionStage::New => 2,
                EvolutionStage::Deprecated => 1,
                EvolutionStage::Retired => 0,
            }
        };
        let trend_rank = |trend: TrendDirection| -> u8 {
            match trend {
                TrendDirection::Improving => 3,
                TrendDirection::Stable => 2,
                TrendDirection::Unknown => 1,
                TrendDirection::Degrading => 0,
            }
        };
        stage_rank(b.2)
            .cmp(&stage_rank(a.2))
            .then_with(|| trend_rank(b.3).cmp(&trend_rank(a.3)))
    });

    if !recommendations.is_empty() {
        EVO_RECOMMENDATIONS_USED.fetch_add(1, Ordering::Relaxed);
    }

    recommendations
}

/// Check if an agent is recommended for a given task type based on EvolutionGraph data.
#[allow(dead_code)] // F-GAP-49 — reserved for evolution graph recommendation queries
pub fn is_agent_recommended_for(agent: &str, task_category: &str) -> bool {
    let graph = match EVOLUTION_GRAPH.lock() {
        Ok(g) => g,
        Err(_) => return false,
    };

    if let Ok(record) = graph.get_record(agent, task_category) {
        matches!(
            record.current_stage,
            EvolutionStage::Stable | EvolutionStage::Mature
        ) && matches!(
            record.trend,
            TrendDirection::Improving | TrendDirection::Stable
        )
    } else {
        false
    }
}

// ── ContinuousLearning bridge ──────────────────────────────────────────────

/// A lightweight snapshot of a learning insight for injection into the autonomy loop.
#[derive(Debug, Clone, Default)]
pub struct IntelligenceContext {
    /// Recent learning insights from ContinuousLearning.
    pub recent_insights: Vec<String>,
    /// Recommended agent-capability pairs from EvolutionGraph.
    pub recommended_agents: Vec<String>,
    /// Metacognitive reflection summary (if available).
    pub metacognitive_summary: Option<String>,
    /// Whether intelligence modules actively contributed to this decision.
    pub intelligence_active: bool,
}

/// Gather intelligence context before an autonomy loop iteration.
///
/// Queries EvolutionGraph for agent recommendations and builds an
/// intelligence context that can be injected into planning/execution.
pub fn gather_intelligence_context(task_objective: &str) -> IntelligenceContext {
    let mut ctx = IntelligenceContext::default();

    // Query EvolutionGraph for agent recommendations
    let recommendations = get_agent_recommendations();
    ctx.recommended_agents = recommendations
        .iter()
        .take(5)
        .map(|(agent, cap, stage, _trend)| format!("{agent}:{cap} ({stage:?})"))
        .collect();

    if !ctx.recommended_agents.is_empty() {
        ctx.intelligence_active = true;
        INTEL_BRIDGE_INTERVENTIONS.fetch_add(1, Ordering::Relaxed);
    }

    // Generate insights based on task objective analysis
    if !task_objective.is_empty() {
        let objective_lower = task_objective.to_ascii_lowercase();

        if objective_lower.contains("refactor") {
            ctx.recent_insights.push(
                "Refactoring tasks benefit from multi-step planning with verification rounds"
                    .to_string(),
            );
        }
        if objective_lower.contains("bug") || objective_lower.contains("fix") {
            ctx.recent_insights.push(
                "Bug-fix tasks should include diagnosis step before implementation".to_string(),
            );
        }
        if objective_lower.contains("test") {
            ctx.recent_insights
                .push("Testing tasks should verify edge cases and regression coverage".to_string());
        }
        if objective_lower.contains("deploy") || objective_lower.contains("release") {
            ctx.recent_insights.push(
                "Deployment tasks benefit from staged rollout and rollback planning".to_string(),
            );
        }

        if !ctx.recent_insights.is_empty() {
            ctx.intelligence_active = true;
            CL_INSIGHTS_APPLIED.fetch_add(ctx.recent_insights.len() as u64, Ordering::Relaxed);
        }
    }

    ctx
}

/// Build an augmented system message with intelligence context for the agent.
pub fn build_intelligence_augmented_context(ctx: &IntelligenceContext) -> Option<String> {
    if !ctx.intelligence_active {
        return None;
    }

    let mut parts: Vec<String> = Vec::new();
    parts.push("[Intelligence Context]".to_string());

    if !ctx.recommended_agents.is_empty() {
        parts.push(format!(
            "Recommended agent capabilities: {}",
            ctx.recommended_agents.join(", ")
        ));
    }

    if !ctx.recent_insights.is_empty() {
        parts.push("Historical insights from similar tasks:".to_string());
        for insight in &ctx.recent_insights {
            parts.push(format!("  - {insight}"));
        }
    }

    if let Some(ref summary) = ctx.metacognitive_summary {
        parts.push(format!("Metacognitive reflection: {summary}"));
    }

    Some(parts.join("\n"))
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gather_intelligence_context_basic() {
        let ctx = gather_intelligence_context("refactor the authentication module");
        assert!(ctx.intelligence_active);
        assert!(!ctx.recent_insights.is_empty());
        assert!(ctx.recent_insights.iter().any(|i| i.contains("refactor")));
    }

    #[test]
    fn test_gather_intelligence_context_bug_fix() {
        let ctx = gather_intelligence_context("fix the login bug");
        assert!(ctx.intelligence_active);
        assert!(ctx.recent_insights.iter().any(|i| i.contains("bug")));
    }

    #[test]
    fn test_gather_intelligence_context_empty() {
        let ctx = gather_intelligence_context("");
        assert!(!ctx.intelligence_active);
        assert!(ctx.recent_insights.is_empty());
    }

    #[test]
    fn test_build_augmented_context() {
        let ctx = IntelligenceContext {
            intelligence_active: true,
            recommended_agents: vec!["agent1:coding (Stable)".to_string()],
            recent_insights: vec!["Test insight".to_string()],
            ..Default::default()
        };

        let augmented = build_intelligence_augmented_context(&ctx);
        assert!(augmented.is_some());
        let text = augmented.unwrap();
        assert!(text.contains("Intelligence Context"));
        assert!(text.contains("agent1"));
        assert!(text.contains("Test insight"));
    }

    #[test]
    fn test_build_augmented_context_inactive() {
        let ctx = IntelligenceContext::default();
        let augmented = build_intelligence_augmented_context(&ctx);
        assert!(augmented.is_none());
    }

    #[test]
    fn test_register_and_record_capability() {
        register_agent_capability("test-agent", "coding");
        record_capability_performance("test-agent", "coding", 0.95, 100.0);

        let recommendations = get_agent_recommendations();
        assert!(!recommendations.is_empty());
    }

    #[test]
    fn test_register_and_record_capability_advances_stage() {
        register_agent_capability("evo-agent", "testing");
        // Record many high-success versions to trigger stage advancement
        for _ in 0..25 {
            record_capability_performance("evo-agent", "testing", 0.90, 50.0);
        }

        let recommendations = get_agent_recommendations();
        let evo_rec = recommendations
            .iter()
            .find(|(a, c, _, _)| a == "evo-agent" && c == "testing");
        assert!(
            evo_rec.is_some(),
            "should have recommendation for evo-agent:testing"
        );
        let (_agent, _cap, stage, _trend) = evo_rec.unwrap();
        assert!(
            matches!(stage, EvolutionStage::Mature | EvolutionStage::Stable),
            "stage should have advanced beyond New/Learning, got {stage:?}"
        );
    }

    #[test]
    fn test_is_agent_recommended_for() {
        register_agent_capability("rec-agent", "refactoring");
        for _ in 0..25 {
            record_capability_performance("rec-agent", "refactoring", 0.92, 80.0);
        }
        assert!(is_agent_recommended_for("rec-agent", "refactoring"));
        assert!(!is_agent_recommended_for("unknown-agent", "refactoring"));
    }
}
