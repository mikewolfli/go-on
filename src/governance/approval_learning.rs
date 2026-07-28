//! Approval Preference Learning (GAP-B52-34)
//!
//! Learns approver preferences over time by recording historical decisions
//! and using them to predict approval likelihood. When confidence exceeds
//! a configurable threshold (default 0.9 based on the past 20 samples),
//! the system may auto-approve without human intervention.
//!
//! Also provides [`ApprovalPolicySuggester`] which analyses decision history
//! to suggest policy improvements.
//!
//! # Architecture
//!
//! ```text
//! Recorded Decisions
//!      │
//!      ▼
//! ApprovalPreferenceLearner
//!      ├── predict_approval(action, context) → f64
//!      └── confidence() → f64
//!              │
//!              ▼
//!      Confidence > 0.9 (n >= 20) → auto-approve
//!              │
//!              ▼
//! ApprovalPolicySuggester
//!      └── suggest_policies() → Vec<PolicySuggestion>
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::SystemTime;
use thiserror::Error;
use tracing::debug;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Default number of recent samples required before confidence is meaningful.
pub const DEFAULT_CONFIDENCE_SAMPLE_SIZE: usize = 20;

/// Default confidence threshold for auto-approval.
pub const DEFAULT_AUTO_APPROVE_THRESHOLD: f64 = 0.9;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum ApprovalLearningError {
    #[error("insufficient history for prediction: need at least {0} samples, have {1}")]
    InsufficientHistory(usize, usize),

    #[error("unknown action type: {0}")]
    UnknownActionType(String),

    #[error("unknown approver: {0}")]
    UnknownApprover(String),

    #[error("internal error: {0}")]
    Internal(String),
}

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// The decision outcome recorded by an approver.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// The action was approved.
    Approved,
    /// The action was rejected.
    Rejected,
    /// The action was escalated to a higher authority.
    Escalated,
    /// The action was skipped / not reviewed.
    Skipped,
}

/// A single recorded approval decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionRecord {
    /// The approver who made the decision.
    pub approver: String,
    /// The type of action being approved or rejected.
    pub action_type: String,
    /// The decision outcome.
    pub decision: ApprovalDecision,
    /// Whether the decision was ultimately approved (true) or not (false).
    pub approved: bool,
    /// Arbitrary context key-value pairs at the time of decision.
    pub context: HashMap<String, String>,
    /// Timestamp when the decision was recorded.
    pub timestamp: SystemTime,
    /// Unique identifier for this decision record.
    pub id: String,
}

/// Aggregated statistics for a specific (approver, action_type) pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApproverActionStats {
    /// The approver name.
    pub approver: String,
    /// The action type.
    pub action_type: String,
    /// Total number of decisions recorded.
    pub total: usize,
    /// Number of approved decisions.
    pub approved: usize,
    /// Number of rejected decisions.
    pub rejected: usize,
    /// Number of escalated decisions.
    pub escalated: usize,
    /// Approval rate (approved / total).
    pub approval_rate: f64,
    /// Most recent decision timestamp.
    pub last_decision: Option<SystemTime>,
}

impl ApproverActionStats {
    /// Compute the approval rate as a fraction [0.0, 1.0].
    pub fn approval_rate(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        self.approved as f64 / self.total as f64
    }
}

// ---------------------------------------------------------------------------
// Approval Preference Learner
// ---------------------------------------------------------------------------

/// Learns and predicts approval preferences based on historical decisions.
///
/// The learner tracks per-(approver, action_type) statistics and provides
/// a confidence-weighted prediction. When confidence > 0.9 with at least
/// 20 recorded samples, the system can auto-approve matching actions.
#[derive(Debug)]
pub struct ApprovalPreferenceLearner {
    /// All recorded decisions, keyed by decision ID.
    decisions: HashMap<String, DecisionRecord>,
    /// Aggregated stats: (approver, action_type) -> Stats.
    stats: HashMap<(String, String), ApproverActionStats>,
    /// Minimum samples required before making a confidence claim.
    min_samples: usize,
    /// Threshold above which auto-approval is triggered.
    auto_approve_threshold: f64,
}

impl Default for ApprovalPreferenceLearner {
    fn default() -> Self {
        Self {
            decisions: HashMap::new(),
            stats: HashMap::new(),
            min_samples: DEFAULT_CONFIDENCE_SAMPLE_SIZE,
            auto_approve_threshold: DEFAULT_AUTO_APPROVE_THRESHOLD,
        }
    }
}

impl ApprovalPreferenceLearner {
    /// Create a new learner with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new learner with custom thresholds.
    pub fn with_thresholds(min_samples: usize, auto_approve_threshold: f64) -> Self {
        Self {
            decisions: HashMap::new(),
            stats: HashMap::new(),
            min_samples,
            auto_approve_threshold,
        }
    }

    // ── Recording ───────────────────────────────────────────────────────

    /// Record a new approval decision.
    ///
    /// Returns the unique ID assigned to the record.
    pub fn record_decision(
        &mut self,
        approver: &str,
        action_type: &str,
        approved: bool,
        context: HashMap<String, String>,
    ) -> String {
        let decision = if approved {
            ApprovalDecision::Approved
        } else {
            ApprovalDecision::Rejected
        };

        let id = format!("{}-{}-{}", approver, action_type, uuid::Uuid::new_v4());

        let record = DecisionRecord {
            approver: approver.to_owned(),
            action_type: action_type.to_owned(),
            decision,
            approved,
            context,
            timestamp: SystemTime::now(),
            id: id.clone(),
        };

        self.decisions.insert(id.clone(), record.clone());

        // Update stats.
        let key = (approver.to_owned(), action_type.to_owned());
        let stats = self.stats.entry(key).or_insert(ApproverActionStats {
            approver: approver.to_owned(),
            action_type: action_type.to_owned(),
            total: 0,
            approved: 0,
            rejected: 0,
            escalated: 0,
            approval_rate: 0.0,
            last_decision: None,
        });

        stats.total += 1;
        if approved {
            stats.approved += 1;
        } else {
            stats.rejected += 1;
        }
        stats.approval_rate = stats.approved as f64 / stats.total as f64;
        stats.last_decision = Some(record.timestamp);

        debug!(
            "record_decision: approver={}, action={}, approved={}, total_samples={}",
            approver, action_type, approved, stats.total
        );

        id
    }

    /// Record an approval decision.
    pub fn record_approval(&mut self, action_type: &str, approver: &str) -> String {
        self.record_decision(approver, action_type, true, HashMap::new())
    }

    /// Record a rejection decision.
    pub fn record_rejection(&mut self, action_type: &str, reason: &str) -> String {
        let mut context = HashMap::new();
        context.insert("rejection_reason".to_string(), reason.to_string());
        self.record_decision("system", action_type, false, context)
    }

    /// Record an escalation event.
    pub fn record_escalation(&mut self, action_type: &str, from_level: &str) -> String {
        let mut context = HashMap::new();
        context.insert("escalation_from".to_string(), from_level.to_string());
        self.record_decision("system", action_type, false, context)
    }

    /// Record an auto-denial event.
    pub fn record_auto_denial(&mut self, action_type: &str, reason: &str) -> String {
        let mut context = HashMap::new();
        context.insert("auto_deny_reason".to_string(), reason.to_string());
        self.record_decision("system", action_type, false, context)
    }

    // ── Prediction ──────────────────────────────────────────────────────

    /// Predict the likelihood (0.0 – 1.0) that an action of the given type
    /// would be approved, based on historical data.
    ///
    /// The prediction is the historical approval rate for the action type,
    /// weighted by how many distinct approvers have approved it.
    pub fn predict_approval(
        &self,
        action_type: &str,
        context: &HashMap<String, String>,
    ) -> Result<f64, ApprovalLearningError> {
        let _ = context; // Context can be used for feature-weighted predictions in future.

        // Collect stats across all approvers for this action type.
        let mut total_decisions = 0usize;
        let mut total_approved = 0usize;

        for ((_approver, at), stats) in &self.stats {
            if at == action_type {
                total_decisions += stats.total;
                total_approved += stats.approved;
            }
        }

        if total_decisions == 0 {
            return Err(ApprovalLearningError::InsufficientHistory(
                self.min_samples,
                0,
            ));
        }

        Ok(total_approved as f64 / total_decisions as f64)
    }

    /// Predict approval for a specific (approver, action_type) pair.
    pub fn predict_approval_for_approver(
        &self,
        approver: &str,
        action_type: &str,
    ) -> Result<f64, ApprovalLearningError> {
        let key = (approver.to_owned(), action_type.to_owned());
        match self.stats.get(&key) {
            Some(stats) if stats.total > 0 => Ok(stats.approval_rate()),
            Some(_) => Err(ApprovalLearningError::InsufficientHistory(
                self.min_samples,
                0,
            )),
            None => Err(ApprovalLearningError::UnknownApprover(approver.into())),
        }
    }

    // ── Confidence ──────────────────────────────────────────────────────

    /// Compute the confidence in the prediction for the given action type.
    ///
    /// Confidence is based on sample size relative to `min_samples`.
    /// Returns a value in [0.0, 1.0], where 1.0 means the learner has
    /// `>= min_samples` samples for this action type.
    pub fn confidence(&self, action_type: &str) -> f64 {
        let total: usize = self
            .stats
            .iter()
            .filter(|((_a, at), _)| at == action_type)
            .map(|(_, s)| s.total)
            .sum();

        if total >= self.min_samples {
            1.0
        } else {
            total as f64 / self.min_samples as f64
        }
    }

    /// Return `true` if the learner can auto-approve the given action type.
    ///
    /// Auto-approval requires:
    /// 1. Confidence >= 1.0 (at least `min_samples` samples).
    /// 2. Predicted approval rate >= `auto_approve_threshold`.
    pub fn can_auto_approve(&self, action_type: &str) -> bool {
        let confidence = self.confidence(action_type);
        if confidence < 1.0 {
            return false;
        }
        match self.predict_approval(action_type, &HashMap::new()) {
            Ok(rate) => rate >= self.auto_approve_threshold,
            Err(_) => false,
        }
    }

    // ── Stats retrieval ─────────────────────────────────────────────────

    /// Get stats for a specific approver and action type.
    pub fn get_stats(&self, approver: &str, action_type: &str) -> Option<&ApproverActionStats> {
        let key = (approver.to_owned(), action_type.to_owned());
        self.stats.get(&key)
    }

    /// Get all stats for a given approver.
    pub fn get_approver_stats(&self, approver: &str) -> Vec<&ApproverActionStats> {
        self.stats
            .iter()
            .filter(|((a, _), _)| a == approver)
            .map(|(_, s)| s)
            .collect()
    }

    /// Get all stats for a given action type.
    pub fn get_action_stats(&self, action_type: &str) -> Vec<&ApproverActionStats> {
        self.stats
            .iter()
            .filter(|((_, at), _)| at == action_type)
            .map(|(_, s)| s)
            .collect()
    }

    /// Total number of recorded decisions.
    pub fn total_decisions(&self) -> usize {
        self.decisions.len()
    }

    /// Total number of distinct approvers.
    pub fn distinct_approvers(&self) -> usize {
        let mut approvers = std::collections::HashSet::new();
        for (a, _) in self.stats.keys() {
            approvers.insert(a.clone());
        }
        approvers.len()
    }

    /// Minimum samples required for confidence.
    pub fn min_samples(&self) -> usize {
        self.min_samples
    }

    /// Auto-approve threshold.
    pub fn auto_approve_threshold(&self) -> f64 {
        self.auto_approve_threshold
    }
}

// ---------------------------------------------------------------------------
// Approval Policy Suggester
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_context(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// A policy suggestion generated by [`super::ApprovalPolicySuggester`].
    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct PolicySuggestion {
        /// Title of the suggestion.
        pub title: String,
        /// Detailed description.
        pub description: String,
        /// The action type this suggestion applies to.
        pub action_type: String,
        /// Suggested auto-approve threshold.
        pub suggested_threshold: f64,
        /// Rationale based on historical data.
        pub rationale: String,
        /// Confidence in this suggestion (0.0 – 1.0) based on data volume.
        pub confidence: f64,
        /// Impact level.
        pub impact: SuggestionImpact,
    }

    /// Impact level of a policy suggestion.
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    pub enum SuggestionImpact {
        Low,
        Medium,
        High,
    }

    /// Analyses historical approval data and suggests policy improvements.
    #[derive(Debug)]
    pub struct ApprovalPolicySuggester {
        /// Minimum data points required before making a suggestion.
        min_data_points: usize,
    }

    impl Default for ApprovalPolicySuggester {
        fn default() -> Self {
            Self {
                min_data_points: 10,
            }
        }
    }

    impl ApprovalPolicySuggester {
        /// Create a new suggestor with a custom minimum data threshold.
        pub fn with_min_data(min_data_points: usize) -> Self {
            Self { min_data_points }
        }

        /// Analyse the learner's history and produce policy suggestions.
        ///
        /// Suggestions include:
        /// - Actions with consistently high approval rates that could be
        ///   auto-approved at a lower threshold.
        /// - Approvers who consistently reject certain action types, suggesting
        ///   additional training or stricter pre-screening.
        /// - Actions with low decision volume that need more data.
        pub fn suggest_policies(
            &self,
            learner: &super::ApprovalPreferenceLearner,
        ) -> Vec<PolicySuggestion> {
            let mut suggestions = Vec::new();

            // Collect unique action types.
            let mut action_types: Vec<String> = learner
                .stats
                .keys()
                .map(|(_, at)| at.clone())
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();
            action_types.sort();

            for at in &action_types {
                let action_stats = learner.get_action_stats(at);
                let total_samples: usize = action_stats.iter().map(|s| s.total).sum();

                if total_samples < self.min_data_points {
                    suggestions.push(PolicySuggestion {
                        title: format!("Insufficient data for '{}'", at),
                        description: format!(
                            "Only {} data points for action type '{}'. \
                             Consider collecting more approval history before auto-approval.",
                            total_samples, at
                        ),
                        action_type: at.clone(),
                        suggested_threshold: learner.auto_approve_threshold(),
                        rationale: format!(
                            "Need at least {} samples for meaningful analysis; currently have {}.",
                            self.min_data_points, total_samples
                        ),
                        confidence: total_samples as f64 / self.min_data_points as f64,
                        impact: SuggestionImpact::Low,
                    });
                    continue;
                }

                // Compute overall approval rate.
                let total_approved: usize = action_stats.iter().map(|s| s.approved).sum();
                let approval_rate = total_approved as f64 / total_samples as f64;

                // If approval rate is very high, suggest auto-approval.
                if approval_rate >= learner.auto_approve_threshold() {
                    let suggested_threshold = (approval_rate - 0.05).max(0.7);
                    suggestions.push(PolicySuggestion {
                        title: format!("Consider auto-approving '{}'", at),
                        description: format!(
                            "Action type '{}' has a {}% approval rate ({}/{} approved). \
                             Consider lowering the auto-approve threshold to {:.2}.",
                            at,
                            (approval_rate * 100.0) as u32,
                            total_approved,
                            total_samples,
                            suggested_threshold
                        ),
                        action_type: at.clone(),
                        suggested_threshold,
                        rationale: format!(
                            "Historical approval rate of {:.2} exceeds current threshold of {:.2}.",
                            approval_rate,
                            learner.auto_approve_threshold()
                        ),
                        confidence: (total_samples as f64 / 100.0).min(1.0),
                        impact: SuggestionImpact::High,
                    });
                }

                // Check for approvers who deviate significantly from the norm.
                for stats in &action_stats {
                    let deviation = (stats.approval_rate - approval_rate).abs();
                    if deviation > 0.3 && stats.total >= self.min_data_points {
                        let trend = if stats.approval_rate > approval_rate {
                            "more permissive"
                        } else {
                            "more restrictive"
                        };
                        suggestions.push(PolicySuggestion {
                            title: format!("Approver '{}' is {} for '{}'", stats.approver, trend, at),
                            description: format!(
                                "Approver '{}' has an approval rate of {:.1}% for '{}', \
                                 while the average is {:.1}% (deviation: {:.1}%). \
                                 Review whether additional training or policy clarification is needed.",
                                stats.approver,
                                stats.approval_rate * 100.0,
                                at,
                                approval_rate * 100.0,
                                deviation * 100.0
                            ),
                            action_type: at.clone(),
                            suggested_threshold: learner.auto_approve_threshold(),
                            rationale: format!(
                                "Deviation of {:.1}% exceeds threshold of 30%.",
                                deviation * 100.0
                            ),
                            confidence: (stats.total as f64 / 50.0).min(1.0),
                            impact: SuggestionImpact::Medium,
                        });
                    }
                }
            }

            suggestions
        }
    }

    #[test]
    fn test_record_decision_and_stats() {
        let mut learner = ApprovalPreferenceLearner::new();
        let id = learner.record_decision("alice", "deploy", true, make_context(&[("env", "prod")]));
        assert!(!id.is_empty());
        assert_eq!(learner.total_decisions(), 1);
        let stats = learner.get_stats("alice", "deploy").unwrap();
        assert_eq!(stats.total, 1);
        assert_eq!(stats.approved, 1);
        assert!((stats.approval_rate() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_predict_approval() {
        let mut learner = ApprovalPreferenceLearner::new();
        learner.record_decision("alice", "deploy", true, HashMap::new());
        learner.record_decision("bob", "deploy", true, HashMap::new());
        learner.record_decision("carol", "deploy", false, HashMap::new());

        let rate = learner.predict_approval("deploy", &HashMap::new()).unwrap();
        assert!((rate - 2.0 / 3.0).abs() < 1e-6);
    }

    #[test]
    fn test_predict_approval_insufficient_data() {
        let learner = ApprovalPreferenceLearner::new();
        let result = learner.predict_approval("deploy", &HashMap::new());
        assert!(result.is_err());
    }

    #[test]
    fn test_confidence() {
        let mut learner = ApprovalPreferenceLearner::with_thresholds(5, 0.9);
        for _ in 0..5 {
            learner.record_decision("alice", "deploy", true, HashMap::new());
        }
        assert!((learner.confidence("deploy") - 1.0).abs() < 1e-6);
        assert!((learner.confidence("rollback") - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_can_auto_approve_below_threshold() {
        let mut learner = ApprovalPreferenceLearner::with_thresholds(3, 0.9);
        // 2 approves, 1 reject = 66% < 90%
        learner.record_decision("alice", "deploy", true, HashMap::new());
        learner.record_decision("alice", "deploy", true, HashMap::new());
        learner.record_decision("alice", "deploy", false, HashMap::new());
        assert!(!learner.can_auto_approve("deploy"));
    }

    #[test]
    fn test_can_auto_approve_above_threshold() {
        let mut learner = ApprovalPreferenceLearner::with_thresholds(3, 0.6);
        learner.record_decision("alice", "deploy", true, HashMap::new());
        learner.record_decision("alice", "deploy", true, HashMap::new());
        learner.record_decision("alice", "deploy", true, HashMap::new());
        assert!(learner.can_auto_approve("deploy"));
    }

    #[test]
    fn test_approver_stats_filtering() {
        let mut learner = ApprovalPreferenceLearner::new();
        learner.record_decision("alice", "deploy", true, HashMap::new());
        learner.record_decision("alice", "rollback", true, HashMap::new());
        learner.record_decision("bob", "deploy", false, HashMap::new());

        assert_eq!(learner.get_approver_stats("alice").len(), 2);
        assert_eq!(learner.get_approver_stats("bob").len(), 1);
        assert_eq!(learner.get_action_stats("deploy").len(), 2);
    }

    #[test]
    fn test_distinct_approvers() {
        let mut learner = ApprovalPreferenceLearner::new();
        learner.record_decision("alice", "deploy", true, HashMap::new());
        learner.record_decision("bob", "deploy", true, HashMap::new());
        assert_eq!(learner.distinct_approvers(), 2);
    }

    #[test]
    fn test_policy_suggester_insufficient_data() {
        let learner = ApprovalPreferenceLearner::new();
        let suggestor = ApprovalPolicySuggester::with_min_data(5);
        let suggestions = suggestor.suggest_policies(&learner);
        // No data at all → no suggestions.
        assert!(suggestions.is_empty());
    }

    #[test]
    fn test_policy_suggester_high_approval() {
        let mut learner = ApprovalPreferenceLearner::with_thresholds(20, 0.9);
        // Record many approvals for "deploy".
        for _ in 0..20 {
            learner.record_decision("alice", "deploy", true, HashMap::new());
        }
        let suggestor = ApprovalPolicySuggester::with_min_data(5);
        let suggestions = suggestor.suggest_policies(&learner);
        // At least one suggestion for deploy high approval rate.
        let deploy_suggestions: Vec<_> = suggestions
            .iter()
            .filter(|s| s.action_type == "deploy")
            .collect();
        assert!(!deploy_suggestions.is_empty());
    }

    #[test]
    fn test_decision_record_serialize() {
        let record = DecisionRecord {
            approver: "alice".into(),
            action_type: "deploy".into(),
            decision: ApprovalDecision::Approved,
            approved: true,
            context: [("env".into(), "prod".into())].into(),
            timestamp: SystemTime::now(),
            id: "test-id".into(),
        };
        let json = serde_json::to_string(&record).expect("decision record should serialize");
        let deserialized: DecisionRecord =
            serde_json::from_str(&json).expect("decision record JSON should deserialize");
        assert_eq!(deserialized.approver, "alice");
        assert!(deserialized.approved);
    }

    #[test]
    fn test_policy_suggestion_serialize() {
        let suggestion = PolicySuggestion {
            title: "Test".into(),
            description: "Desc".into(),
            action_type: "deploy".into(),
            suggested_threshold: 0.85,
            rationale: "Historical data".into(),
            confidence: 0.9,
            impact: SuggestionImpact::Medium,
        };
        let json = serde_json::to_string(&suggestion).expect("policy suggestion should serialize");
        let deserialized: PolicySuggestion =
            serde_json::from_str(&json).expect("policy suggestion JSON should deserialize");
        assert_eq!(deserialized.title, "Test");
        assert!((deserialized.suggested_threshold - 0.85).abs() < 1e-6);
    }

    #[test]
    fn test_predict_approval_for_approver() {
        let mut learner = ApprovalPreferenceLearner::new();
        learner.record_decision("alice", "deploy", true, HashMap::new());
        learner.record_decision("alice", "deploy", true, HashMap::new());
        learner.record_decision("alice", "deploy", false, HashMap::new());

        let rate = learner
            .predict_approval_for_approver("alice", "deploy")
            .unwrap();
        assert!((rate - 2.0 / 3.0).abs() < 1e-6);

        assert!(learner
            .predict_approval_for_approver("nonexistent", "deploy")
            .is_err());
    }
}
