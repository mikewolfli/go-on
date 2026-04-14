use serde::{Deserialize, Serialize};

use crate::intelligence::reinforcement::{KnowledgeInsightArtifact, WorkflowLearningEvent};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum QualityVerdict {
    Approve,
    ApproveWithCaveats,
    Reject,
    Revise,
    InsufficientEvidence,
    Valid,
    Invalid,
    RequiresRepair,
    Inconclusive,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum QualitySignalType {
    Syntax,
    Tests,
    Lint,
    Policy,
    Logic,
    PuaQualityCompass,
    RuntimeVerification,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualitySignal {
    pub signal_type: QualitySignalType,
    pub passed: bool,
    pub confidence: f32,
    pub details: Option<String>,
}

impl QualitySignal {
    pub fn is_sufficient_for_distillation(&self) -> bool {
        self.passed && self.confidence >= 0.7
    }
}

/// Aggregate quality verdict from multiple signals.
pub fn aggregate_verdict(signals: &[QualitySignal]) -> QualityVerdict {
    if signals.is_empty() {
        return QualityVerdict::InsufficientEvidence;
    }

    let pass_count = signals.iter().filter(|signal| signal.passed).count();
    let pass_rate = pass_count as f64 / signals.len() as f64;
    if pass_rate >= 0.9 {
        QualityVerdict::Approve
    } else if pass_rate >= 0.7 {
        QualityVerdict::ApproveWithCaveats
    } else if pass_rate >= 0.5 {
        QualityVerdict::Revise
    } else {
        QualityVerdict::Reject
    }
}

#[derive(Debug, Clone)]
pub struct KnowledgeDistiller {
    pub min_confidence: f64,
    pub dedup_threshold: f64,
    pub max_insights_per_type: usize,
}

impl Default for KnowledgeDistiller {
    fn default() -> Self {
        Self {
            min_confidence: 0.7,
            dedup_threshold: 0.9,
            max_insights_per_type: 8,
        }
    }
}

impl KnowledgeDistiller {
    pub fn distill(&self, events: &[WorkflowLearningEvent]) -> Vec<KnowledgeInsightArtifact> {
        let mut insights = Vec::new();
        for event in events {
            if event.predicted_success_rate as f64 >= self.min_confidence
                && event.subtasks_failed == 0
            {
                insights.push(KnowledgeInsightArtifact {
                    generated_at: event.generated_at,
                    conversation_id: format!("auto-{}", event.generated_at),
                    branch_id: "main".to_string(),
                    phase: "execution".to_string(),
                    task: event.task.clone(),
                    agent: event.executor.clone(),
                    source: event.source.clone(),
                    request_excerpt: event.task.chars().take(120).collect(),
                    response_excerpt: format!(
                        "success_rate={:.2}, speedup={:.2}",
                        event.predicted_success_rate, event.parallel_speedup
                    ),
                    reusable_insights: vec![format!(
                        "Prefer strategy from successful task '{}'",
                        event.task
                    )],
                    verification_steps: vec!["Replay on similar task".to_string()],
                    confidence: event.predicted_success_rate as f64,
                });
            }
        }
        self.deduplicate(insights)
    }

    pub fn deduplicate(
        &self,
        insights: Vec<KnowledgeInsightArtifact>,
    ) -> Vec<KnowledgeInsightArtifact> {
        let mut kept: Vec<KnowledgeInsightArtifact> = Vec::new();
        for insight in insights {
            if insight.confidence < self.min_confidence {
                continue;
            }

            let prefix = insight.request_excerpt.chars().take(50).collect::<String>();
            let duplicate = kept.iter().any(|existing| {
                existing.phase == insight.phase
                    && similarity(
                        &existing
                            .request_excerpt
                            .chars()
                            .take(50)
                            .collect::<String>(),
                        &prefix,
                    ) >= self.dedup_threshold
            });

            if !duplicate {
                kept.push(insight);
            }
        }

        if kept.len() > self.max_insights_per_type {
            kept.truncate(self.max_insights_per_type);
        }
        kept
    }

    pub fn build_pattern(
        &self,
        success_events: &[WorkflowLearningEvent],
    ) -> Option<KnowledgeInsightArtifact> {
        let best = success_events
            .iter()
            .filter(|event| event.subtasks_failed == 0)
            .max_by(|a, b| {
                a.predicted_success_rate
                    .partial_cmp(&b.predicted_success_rate)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })?;

        Some(KnowledgeInsightArtifact {
            generated_at: best.generated_at,
            conversation_id: format!("pattern-{}", best.generated_at),
            branch_id: "main".to_string(),
            phase: "execution".to_string(),
            task: best.task.clone(),
            agent: best.executor.clone(),
            source: best.source.clone(),
            request_excerpt: best.task.chars().take(120).collect(),
            response_excerpt: "pattern derived from top successful event".to_string(),
            reusable_insights: vec!["Reuse top-performing workflow pattern".to_string()],
            verification_steps: vec!["Validate against next similar task".to_string()],
            confidence: best.predicted_success_rate as f64,
        })
    }
}

fn similarity(left: &str, right: &str) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    if left == right {
        return 1.0;
    }

    let overlap = left.chars().filter(|ch| right.contains(*ch)).count() as f64;
    overlap / left.len().max(right.len()) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_event(confidence: f32, failed: usize, task: &str) -> WorkflowLearningEvent {
        WorkflowLearningEvent {
            generated_at: 1,
            task: task.to_string(),
            complexity: 2,
            predicted_success_rate: confidence,
            subtasks_total: 3,
            subtasks_completed: 3usize.saturating_sub(failed),
            subtasks_failed: failed,
            subtasks_skipped: 0,
            serial_work_ms: 100,
            critical_path_ms: 80,
            parallel_speedup: 1.2,
            parallel_efficiency: 0.8,
            executor: "copilot".to_string(),
            source: "test".to_string(),
            runtime_healthy: true,
            gates_ok: true,
            work_grade: "agent".to_string(),
            risk_score: 0.1,
            clarification_rounds: 0,
            clarification_quality_score: 1.0,
            requirement_change_count: 0,
            review_reject_root_cause: String::new(),
            primary_stability_score: 1.0,
            secondary_utilization_rate: 0.0,
            failover_count: 0,
            failover_root_cause: String::new(),
        }
    }

    #[test]
    fn distiller_filters_low_confidence_insights() {
        let distiller = KnowledgeDistiller::default();
        let insights = distiller.distill(&[
            sample_event(0.95, 0, "high confidence"),
            sample_event(0.5, 0, "low confidence"),
        ]);
        assert_eq!(insights.len(), 1);
        assert!(insights[0].task.contains("high confidence"));
    }

    #[test]
    fn deduplicate_removes_high_similarity_entries() {
        let distiller = KnowledgeDistiller::default();
        let base = KnowledgeInsightArtifact {
            generated_at: 1,
            conversation_id: "c1".to_string(),
            branch_id: "main".to_string(),
            phase: "execution".to_string(),
            task: "t1".to_string(),
            agent: "copilot".to_string(),
            source: "test".to_string(),
            request_excerpt: "same prefix text for deduplication".to_string(),
            response_excerpt: "r1".to_string(),
            reusable_insights: vec![],
            verification_steps: vec![],
            confidence: 0.9,
        };
        let mut alt = base.clone();
        alt.conversation_id = "c2".to_string();

        let result = distiller.deduplicate(vec![base, alt]);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn aggregate_verdict_approve_when_all_signals_pass() {
        let signals = vec![
            QualitySignal {
                signal_type: QualitySignalType::Syntax,
                passed: true,
                confidence: 0.95,
                details: None,
            },
            QualitySignal {
                signal_type: QualitySignalType::Tests,
                passed: true,
                confidence: 0.9,
                details: None,
            },
        ];
        assert_eq!(aggregate_verdict(&signals), QualityVerdict::Approve);
    }
}
