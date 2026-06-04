//! GAP-B53-58: Self-improvement report generation.
//!
//! Aggregates data from the evolution history, metacognitive system,
//! consciousness metrics, and code quality scans into a comprehensive
//! self-improvement report.

use crate::intelligence::code_quality::CodeQualityReport;
use crate::intelligence::consciousness::ConsciousnessProfile;
use crate::intelligence::metacognitive::MetacognitiveProfile;
use crate::orchestration::self_evolution::evolution_history::EvolutionHistory;
use serde::{Deserialize, Serialize};

/// Comprehensive self-improvement report spanning all subsystems.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelfImprovementReport {
    /// When the report was generated.
    pub generated_at_ms: u64,

    // ── Evolution system ────────────────────────────────────────────────
    /// Total evolution cycles executed.
    pub total_evolution_cycles: u64,
    /// Number of successful evolution cycles.
    pub successful_evolutions: u64,
    /// Number of failed evolution cycles.
    pub failed_evolutions: u64,
    /// Number of rolled-back evolutions.
    pub rolled_back_evolutions: u64,

    // ── Metacognitive system ────────────────────────────────────────────
    /// Metacognitive profile snapshot.
    pub metacognitive_profile: Option<MetacognitiveProfile>,

    // ── Consciousness system ────────────────────────────────────────────
    /// Consciousness profile snapshot.
    pub consciousness_profile: Option<ConsciousnessProfile>,

    // ── Code quality ────────────────────────────────────────────────────
    /// Latest code quality report.
    pub code_quality: Option<CodeQualityReport>,

    // ── Summary ─────────────────────────────────────────────────────────
    /// Overall self-improvement health score (0.0–1.0).
    pub overall_health_score: f64,
    /// Human-readable summary text.
    pub summary: String,
    /// Recommended next actions.
    pub recommendations: Vec<String>,
}

impl SelfImprovementReport {
    /// Generate an empty/inert report for bootstrapping.
    pub fn empty() -> Self {
        Self {
            generated_at_ms: crate::intelligence::now_ms(),
            total_evolution_cycles: 0,
            successful_evolutions: 0,
            failed_evolutions: 0,
            rolled_back_evolutions: 0,
            metacognitive_profile: None,
            consciousness_profile: None,
            code_quality: None,
            overall_health_score: 1.0,
            summary: "No self-improvement data available yet.".to_string(),
            recommendations: vec!["Run evolution cycles to gather data.".to_string()],
        }
    }

    /// Generate a report from available subsystem data.
    pub async fn generate(
        history: Option<&EvolutionHistory>,
        metacognitive_profile: Option<MetacognitiveProfile>,
        consciousness_profile: Option<ConsciousnessProfile>,
        code_quality: Option<CodeQualityReport>,
    ) -> Self {
        let mut report = Self::empty();

        // ── Evolution history ───────────────────────────────────────────
        if let Some(h) = history {
            let entries = h.list().await;
            report.total_evolution_cycles = entries.len() as u64;
            report.successful_evolutions =
                entries.iter().filter(|e| e.is_successful()).count() as u64;
            report.failed_evolutions = entries
                .iter()
                .filter(|e| !e.build_result.is_success())
                .count() as u64;
            report.rolled_back_evolutions =
                entries.iter().filter(|e| e.is_rolled_back()).count() as u64;
        }

        report.metacognitive_profile = metacognitive_profile;
        report.consciousness_profile = consciousness_profile;
        report.code_quality = code_quality;

        // ── Compute overall health score ────────────────────────────────
        let mut score = 1.0_f64;
        let mut factors = 0_u32;

        if report.total_evolution_cycles > 0 {
            let success_rate =
                report.successful_evolutions as f64 / report.total_evolution_cycles as f64;
            score *= success_rate;
            factors += 1;
        }

        if let Some(ref mc) = report.metacognitive_profile {
            score *= mc.action_effectiveness_ratio.max(0.1);
            factors += 1;
        }

        if let Some(ref cq) = report.code_quality {
            score *= cq.health_score;
            factors += 1;
        }

        report.overall_health_score = if factors > 0 {
            score.powf(1.0 / factors as f64)
        } else {
            1.0
        };

        // ── Build summary and recommendations ───────────────────────────
        let mut summary_parts = Vec::new();
        let mut recs = Vec::new();

        summary_parts.push(format!(
            "Evolution: {} total, {} successful, {} failed, {} rolled back.",
            report.total_evolution_cycles,
            report.successful_evolutions,
            report.failed_evolutions,
            report.rolled_back_evolutions,
        ));

        if report.total_evolution_cycles == 0 {
            recs.push("Start by running the self-evolution loop.".to_string());
        }

        if let Some(ref mc) = report.metacognitive_profile {
            summary_parts.push(format!(
                "Metacognitive: {} observations, {} actions ({:.0}% effectiveness).",
                mc.total_observations,
                mc.total_actions_taken,
                mc.action_effectiveness_ratio * 100.0,
            ));

            if mc.unresolved_observations > 0 {
                recs.push(format!(
                    "Resolve {} unresolved observations.",
                    mc.unresolved_observations
                ));
            }
        }

        if let Some(ref cs) = report.consciousness_profile {
            summary_parts.push(format!(
                "Consciousness state: {:?}, awareness: {:.2}.",
                cs.state, cs.overall_awareness,
            ));

            if cs.overall_awareness < 0.3 {
                recs.push("Improve data collection to raise consciousness awareness.".to_string());
            }
        }

        if let Some(ref cq) = report.code_quality {
            if !cq.is_clean() {
                summary_parts.push(format!(
                    "Code quality: {} issues found (score: {:.2}).",
                    cq.issues.len(),
                    cq.health_score,
                ));
                recs.push("Address code quality issues detected by the scanner.".to_string());
            } else {
                summary_parts.push("Code quality: clean.".to_string());
            }
        }

        report.summary = summary_parts.join(" ");
        report.recommendations = recs;

        report
    }

    /// Render the report as a formatted markdown string.
    pub fn to_markdown(&self) -> String {
        let mut md = String::new();

        md.push_str("# Self-Improvement Report\n\n");

        md.push_str(&format!(
            "**Generated at:** {} (epoch ms)\n\n",
            self.generated_at_ms
        ));

        md.push_str(&format!(
            "**Overall Health Score:** {:.2}/1.0\n\n",
            self.overall_health_score
        ));

        md.push_str("## Summary\n\n");
        md.push_str(&self.summary);
        md.push_str("\n\n");

        md.push_str("## Evolution\n\n");
        md.push_str(&format!(
            "- Total cycles: {}\n",
            self.total_evolution_cycles
        ));
        md.push_str(&format!("- Successful: {}\n", self.successful_evolutions));
        md.push_str(&format!("- Failed: {}\n", self.failed_evolutions));
        md.push_str(&format!("- Rolled back: {}\n", self.rolled_back_evolutions));

        md.push_str("\n## Recommendations\n\n");
        if self.recommendations.is_empty() {
            md.push_str("No recommendations at this time.\n");
        } else {
            for (i, rec) in self.recommendations.iter().enumerate() {
                md.push_str(&format!("{}. {}\n", i + 1, rec));
            }
        }

        md
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intelligence::code_quality::CodeQualityIssue;
    use crate::intelligence::consciousness::ConsciousnessState;
    use crate::intelligence::metacognitive::MetacognitiveProfile;

    #[test]
    fn test_empty_report() {
        let report = SelfImprovementReport::empty();
        assert_eq!(report.total_evolution_cycles, 0);
        assert_eq!(report.overall_health_score, 1.0);
        assert!(!report.summary.is_empty());
    }

    #[tokio::test]
    async fn test_generate_with_partial_data() {
        let mc_profile = MetacognitiveProfile {
            total_observations: 10,
            unresolved_observations: 2,
            total_actions_taken: 5,
            successful_actions: 3,
            total_reports: 1,
            avg_confidence: 0.7,
            action_effectiveness_ratio: 0.6,
        };

        let cs_profile = ConsciousnessProfile {
            state: ConsciousnessState::Reflexive,
            overall_awareness: 0.5,
            metric_count: 15,
            last_reflexion_ms: 1000,
            reflexion_count: 3,
        };

        let report =
            SelfImprovementReport::generate(None, Some(mc_profile), Some(cs_profile), None).await;

        assert_eq!(report.total_evolution_cycles, 0);
        assert!(report.overall_health_score > 0.0);
        assert!(report.overall_health_score <= 1.0);
    }

    #[test]
    fn test_to_markdown() {
        let report = SelfImprovementReport::empty();
        let md = report.to_markdown();
        assert!(md.contains("Self-Improvement Report"));
        assert!(md.contains("Recommendations"));
    }

    #[tokio::test]
    async fn test_generate_with_code_quality() {
        let cq = crate::intelligence::code_quality::CodeQualityReport {
            issues: vec![CodeQualityIssue::DeadCode {
                module: "src/main.rs".to_string(),
                ratio: 0.15,
            }],
            health_score: 0.85,
            modules_scanned: 5,
            scanned_at_ms: 1000,
        };

        let report = SelfImprovementReport::generate(None, None, None, Some(cq)).await;
        assert!(report.overall_health_score < 1.0);
        assert!(!report.recommendations.is_empty());
    }
}
