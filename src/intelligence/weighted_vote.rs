//! F-GAP-49-2: Weighted reputation voting with Delphi-method debate rounds.
//!
//! Upgrades the simple-majority voting system to:
//! 1. **Weighted reputation voting** – votes are weighted by each agent's
//!    reputation score from the reputation store, so high-reputation agents
//!    have proportionally greater influence on the outcome.
//! 2. **Delphi-method debate rounds** – agents participate in up to `N`
//!    rounds where they see each other's reasoning and may update their
//!    votes, converging toward consensus without a central aggregator.

use std::collections::HashMap;

use futures_util::future::join_all;
use serde::{Deserialize, Serialize};

// ── Types ───────────────────────────────────────────────────────────────────

/// A single agent's vote on a proposal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vote {
    /// Whether the agent approves the proposal.
    pub approves: bool,
    /// Free-text rationale for this vote.
    pub reasoning: String,
    /// Confidence in this vote (0.0–1.0).
    pub confidence: f64,
}

/// Result of a single weighted voting round.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoteResult {
    /// Whether the proposal was approved given the threshold.
    pub approved: bool,
    /// Ratio of weighted "yes" votes to total weight (0.0–1.0).
    pub approval_ratio: f64,
    /// Total weight that participated.
    pub total_weight: f64,
    /// Weighted "yes" sum.
    pub weighted_yes: f64,
    /// Whether this result was computed with reputation weights.
    pub weighted: bool,
    /// Number of agents that participated.
    pub participant_count: usize,
}

/// Configuration for weighted voting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeightedVoteConfig {
    /// Approval threshold (0.0–1.0). Default 0.6 (60%).
    pub threshold: f64,
    /// Default weight for agents not found in the reputation store.
    pub default_weight: f64,
}

impl Default for WeightedVoteConfig {
    fn default() -> Self {
        Self {
            threshold: 0.6,
            default_weight: 0.5,
        }
    }
}

/// A single round's worth of debate history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelphiRound {
    /// Round number (0-based).
    pub round: usize,
    /// Votes cast in this round, keyed by agent name.
    pub votes: HashMap<String, Vote>,
    /// The approval ratio computed at the end of this round.
    pub approval_ratio: f64,
}

/// Result of a full Delphi-method debate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelphiResult {
    /// Final votes, keyed by agent name.
    pub votes: HashMap<String, Vote>,
    /// Number of debate rounds that actually ran.
    pub rounds: usize,
    /// Whether the debate converged before `max_rounds` was reached.
    pub converged: bool,
    /// History of all debate rounds.
    pub history: Vec<DelphiRound>,
    /// The final vote result after the last round.
    pub final_result: VoteResult,
    /// Configuration used for this debate.
    pub config: DelphiConfig,
}

/// Configuration for Delphi-method debate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelphiConfig {
    /// Maximum number of debate rounds (default: 2).
    pub max_rounds: usize,
    /// Approval threshold for the final weighted vote.
    pub threshold: f64,
    /// Default weight for agents without a reputation score.
    pub default_weight: f64,
    /// Minimum convergence ratio — if at least this fraction of agents
    /// didn't change their vote since the previous round, consider converged.
    pub convergence_ratio: f64,
}

impl Default for DelphiConfig {
    fn default() -> Self {
        Self {
            max_rounds: 2,
            threshold: 0.6,
            default_weight: 0.5,
            convergence_ratio: 0.8,
        }
    }
}

// ── Core Functions ──────────────────────────────────────────────────────────

/// Perform a weighted reputation vote.
///
/// Each agent's vote is weighted by its reputation score. Returns the
/// computed [`VoteResult`] including the approval ratio and final decision.
///
/// # Arguments
///
/// * `votes` – Map from agent name to their [`Vote`].
/// * `reputations` – Map from agent name to reputation score (0.0–1.0).
/// * `threshold` – Fraction of weighted "yes" required for approval (0.0–1.0).
/// * `default_weight` – Default weight for agents not in `reputations`.
pub fn weighted_vote(
    votes: &HashMap<String, Vote>,
    reputations: &HashMap<String, f64>,
    threshold: f64,
    default_weight: f64,
) -> VoteResult {
    let mut weighted_yes = 0.0_f64;
    let mut total_weight = 0.0_f64;

    for (agent, vote) in votes {
        let weight = reputations.get(agent).copied().unwrap_or(default_weight);
        weighted_yes += weight * if vote.approves { 1.0 } else { 0.0 };
        total_weight += weight;
    }

    if total_weight == 0.0 {
        return VoteResult {
            approved: false,
            approval_ratio: 0.0,
            total_weight: 0.0,
            weighted_yes: 0.0,
            weighted: true,
            participant_count: votes.len(),
        };
    }

    let approval_ratio = weighted_yes / total_weight;
    VoteResult {
        approved: approval_ratio >= threshold,
        approval_ratio,
        total_weight,
        weighted_yes,
        weighted: true,
        participant_count: votes.len(),
    }
}

/// Check whether the set of votes has converged.
///
/// Convergence is defined as at least `convergence_ratio` fraction of agents
/// casting the same vote (same `approves` value) as in the previous round.
///
/// Returns `true` if the number of changed votes is below the threshold,
/// or if there are fewer than 2 participants (trivially converged).
pub fn is_converged(
    current: &HashMap<String, Vote>,
    previous: &HashMap<String, Vote>,
    convergence_ratio: f64,
) -> bool {
    if current.len() < 2 || previous.is_empty() {
        return true;
    }

    let mut unchanged = 0_usize;
    let total = current.len();

    for (agent, vote) in current {
        if let Some(prev) = previous.get(agent) {
            if vote.approves == prev.approves {
                unchanged += 1;
            }
        }
    }

    (unchanged as f64 / total as f64) >= convergence_ratio
}

/// Format other agents' reasoning into a context prompt for the next round.
pub fn format_debate_history(round_votes: &HashMap<String, Vote>) -> String {
    let mut lines: Vec<String> = Vec::new();
    for (agent, vote) in round_votes {
        let stance = if vote.approves { "APPROVE" } else { "REJECT" };
        lines.push(format!(
            "[{}] {} (confidence: {:.2}): {}",
            stance, agent, vote.confidence, vote.reasoning
        ));
    }
    lines.join("\n---\n")
}

/// Run a full Delphi-method debate across multiple rounds.
///
/// In round 0, each agent votes independently based on the question.
/// In subsequent rounds, agents see the attributed reasoning of all other
/// agents from the previous round (`format_debate_history` tags each entry
/// with the voting agent's name — the debate is deliberately NOT anonymized),
/// and may update their vote.
///
/// After each round, convergence is checked. If converged, the debate ends
/// early. At the end, a final weighted vote is computed.
///
/// # Arguments
///
/// * `agents` – Slice of agent voter implementations.
/// * `question` – The proposal or question being voted on.
/// * `reputations` – Map from agent name to reputation score.
/// * `config` – [`DelphiConfig`] controlling rounds, threshold, etc.
/// * `initial_votes` – Round-0 votes already collected by the caller. When
///   provided, the voters are not invoked again for round 0 — this avoids
///   paying a full extra voter round (including remote LLM voters) when the
///   caller has already gathered the first-round votes.
pub async fn delphi_debate(
    agents: &[&dyn AgentVoter],
    question: &str,
    reputations: &HashMap<String, f64>,
    config: &DelphiConfig,
    initial_votes: Option<HashMap<String, Vote>>,
) -> DelphiResult {
    let mut history: Vec<DelphiRound> = Vec::new();
    let mut current_votes: HashMap<String, Vote> = HashMap::new();
    let mut previous_votes: HashMap<String, Vote> = HashMap::new();

    // Round 0 may already have been cast by the caller; seed the debate with
    // it so convergence is checked from round 1 onward without re-invoking
    // the voters (which would duplicate remote LLM calls).
    let mut first_round = 0usize;
    if let Some(initial) = initial_votes {
        if !initial.is_empty() {
            let round_result = weighted_vote(
                &initial,
                reputations,
                config.threshold,
                config.default_weight,
            );
            history.push(DelphiRound {
                round: 0,
                votes: initial.clone(),
                approval_ratio: round_result.approval_ratio,
            });
            previous_votes = initial.clone();
            current_votes = initial;
            first_round = 1;
        }
    }

    for round in first_round..config.max_rounds {
        // Build context for this round
        let context = {
            let debate_history = history
                .last()
                .map(|r| format_debate_history(&r.votes))
                .unwrap_or_default();
            format!(
                "Question: {question}\n\nOther agents' reasoning from previous round:\n{}",
                debate_history
            )
        };

        // Collect votes from all agents — voters are independent, so run them
        // concurrently (a slow remote voter no longer serializes local voters).
        let mut round_votes: HashMap<String, Vote> = HashMap::new();
        let votes: Vec<(String, Vote)> = join_all(agents.iter().map(|agent| async {
            let vote = agent.vote(&context).await;
            (agent.name().to_string(), vote)
        }))
        .await;
        for (name, vote) in votes {
            round_votes.insert(name, vote);
        }

        // Compute weighted approval ratio for this round
        let round_result = weighted_vote(
            &round_votes,
            reputations,
            config.threshold,
            config.default_weight,
        );

        let round_record = DelphiRound {
            round,
            votes: round_votes.clone(),
            approval_ratio: round_result.approval_ratio,
        };
        history.push(round_record);

        // Check convergence against previous round
        if round > 0 && is_converged(&round_votes, &previous_votes, config.convergence_ratio) {
            current_votes = round_votes;
            break;
        }

        previous_votes = round_votes.clone();
        current_votes = round_votes;
    }

    // Final weighted vote result
    let final_result = weighted_vote(
        &current_votes,
        reputations,
        config.threshold,
        config.default_weight,
    );

    let rounds_elapsed = history.len();
    DelphiResult {
        votes: current_votes,
        rounds: rounds_elapsed,
        converged: rounds_elapsed < config.max_rounds,
        history,
        final_result,
        config: config.clone(),
    }
}

// ── Trait ───────────────────────────────────────────────────────────────────

/// An agent that can participate in voting and Delphi-method debate.
///
/// Implementors provide their name and an async `vote()` method that
/// returns a [`Vote`] given a context string describing the proposal
/// and (in later debate rounds) the reasoning of other agents.
#[async_trait::async_trait]
pub trait AgentVoter: Send + Sync {
    /// Display name of this agent, used as the key in vote maps.
    fn name(&self) -> &str;

    /// Cast a vote based on the provided context.
    ///
    /// In round 0, `context` contains only the question.
    /// In subsequent rounds, `context` includes other agents' reasoning.
    async fn vote(&self, context: &str) -> Vote;
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── weighted_vote tests ────────────────────────────────────────────────

    #[test]
    fn test_weighted_vote_unanimous_approval() {
        let mut votes = HashMap::new();
        votes.insert(
            "alice".to_string(),
            Vote {
                approves: true,
                reasoning: "Good proposal".into(),
                confidence: 0.9,
            },
        );
        votes.insert(
            "bob".to_string(),
            Vote {
                approves: true,
                reasoning: "Agreed".into(),
                confidence: 0.8,
            },
        );
        let mut reps = HashMap::new();
        reps.insert("alice".to_string(), 0.9);
        reps.insert("bob".to_string(), 0.7);

        let result = weighted_vote(&votes, &reps, 0.6, 0.5);
        assert!(result.approved);
        assert!((result.approval_ratio - 1.0).abs() < f64::EPSILON);
        assert!(result.weighted);
        assert_eq!(result.participant_count, 2);
    }

    #[test]
    fn test_weighted_vote_no_consensus() {
        let mut votes = HashMap::new();
        votes.insert(
            "alice".to_string(),
            Vote {
                approves: true,
                reasoning: "Yes".into(),
                confidence: 0.9,
            },
        );
        votes.insert(
            "bob".to_string(),
            Vote {
                approves: false,
                reasoning: "No".into(),
                confidence: 0.8,
            },
        );
        let mut reps = HashMap::new();
        reps.insert("alice".to_string(), 0.9);
        reps.insert("bob".to_string(), 0.9);

        let result = weighted_vote(&votes, &reps, 0.6, 0.5);
        // alice has 0.9 weight, bob has 0.9, so 0.9 / 1.8 = 0.5 < 0.6
        assert!(!result.approved);
        assert!((result.approval_ratio - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_weighted_vote_reputation_matters() {
        // Alice has high reputation, Bob has low reputation.
        // Alice approves, Bob disapproves.
        let mut votes = HashMap::new();
        votes.insert(
            "alice".to_string(),
            Vote {
                approves: true,
                reasoning: "Yes".into(),
                confidence: 0.9,
            },
        );
        votes.insert(
            "bob".to_string(),
            Vote {
                approves: false,
                reasoning: "No".into(),
                confidence: 0.8,
            },
        );
        let mut reps = HashMap::new();
        reps.insert("alice".to_string(), 0.95); // high reputation
        reps.insert("bob".to_string(), 0.15); // low reputation (excluded range)

        let result = weighted_vote(&votes, &reps, 0.6, 0.5);
        // Alice has 0.95 weight, Bob has 0.15 weight
        // weighted_yes = 0.95, total_weight = 1.10
        // approval_ratio = 0.95 / 1.10 ≈ 0.864 >= 0.6 → approved
        assert!(result.approved);
        let expected_ratio = 0.95 / 1.10;
        assert!((result.approval_ratio - expected_ratio).abs() < 1e-10);
    }

    #[test]
    fn test_weighted_vote_default_weight_for_unknown() {
        let mut votes = HashMap::new();
        votes.insert(
            "unknown".to_string(),
            Vote {
                approves: true,
                reasoning: "Yes".into(),
                confidence: 0.5,
            },
        );
        // No reputation entry for "unknown" → uses default_weight = 0.5
        let reps = HashMap::new();
        let result = weighted_vote(&votes, &reps, 0.6, 0.5);
        assert!(result.approved);
        assert!((result.weighted_yes - 0.5).abs() < f64::EPSILON);
        assert_eq!(result.participant_count, 1);
    }

    #[test]
    fn test_weighted_vote_custom_threshold() {
        let mut votes = HashMap::new();
        votes.insert(
            "alice".to_string(),
            Vote {
                approves: true,
                reasoning: "Yes".into(),
                confidence: 0.5,
            },
        );
        votes.insert(
            "bob".to_string(),
            Vote {
                approves: false,
                reasoning: "No".into(),
                confidence: 0.5,
            },
        );
        let mut reps = HashMap::new();
        reps.insert("alice".to_string(), 0.8);
        reps.insert("bob".to_string(), 0.2);

        // With threshold 0.5, alice's 0.8 / 1.0 = 0.8 >= 0.5 → approved
        let result = weighted_vote(&votes, &reps, 0.5, 0.5);
        assert!(result.approved);
        assert!((result.approval_ratio - 0.8).abs() < f64::EPSILON);

        // With threshold 0.9, 0.8 < 0.9 → not approved
        let result = weighted_vote(&votes, &reps, 0.9, 0.5);
        assert!(!result.approved);
    }

    // ── is_converged tests ─────────────────────────────────────────────────

    #[test]
    fn test_is_converged_all_same() {
        let mut current = HashMap::new();
        current.insert(
            "a".to_string(),
            Vote {
                approves: true,
                reasoning: "".into(),
                confidence: 0.5,
            },
        );
        current.insert(
            "b".to_string(),
            Vote {
                approves: true,
                reasoning: "".into(),
                confidence: 0.5,
            },
        );
        let previous = current.clone();
        assert!(is_converged(&current, &previous, 0.8));
    }

    #[test]
    fn test_is_converged_partial_change() {
        let mut previous = HashMap::new();
        previous.insert(
            "a".to_string(),
            Vote {
                approves: true,
                reasoning: "".into(),
                confidence: 0.5,
            },
        );
        previous.insert(
            "b".to_string(),
            Vote {
                approves: false,
                reasoning: "".into(),
                confidence: 0.5,
            },
        );
        previous.insert(
            "c".to_string(),
            Vote {
                approves: true,
                reasoning: "".into(),
                confidence: 0.5,
            },
        );

        let mut current = previous.clone();
        // "b" changes vote
        current.insert(
            "b".to_string(),
            Vote {
                approves: true,
                reasoning: "changed mind".into(),
                confidence: 0.6,
            },
        );

        // 2 out of 3 unchanged = 0.667; threshold 0.8 → not converged
        assert!(!is_converged(&current, &previous, 0.8));
        // threshold 0.6 → converged (0.667 >= 0.6)
        assert!(is_converged(&current, &previous, 0.6));
    }

    #[test]
    fn test_is_converged_few_agents() {
        let mut current = HashMap::new();
        current.insert(
            "a".to_string(),
            Vote {
                approves: true,
                reasoning: "".into(),
                confidence: 0.5,
            },
        );
        let previous = HashMap::new();
        // Fewer than 2 agents → trivially converged
        assert!(is_converged(&current, &previous, 0.8));
    }

    #[test]
    fn test_is_converged_empty_previous() {
        let mut current = HashMap::new();
        current.insert(
            "a".to_string(),
            Vote {
                approves: true,
                reasoning: "".into(),
                confidence: 0.5,
            },
        );
        current.insert(
            "b".to_string(),
            Vote {
                approves: false,
                reasoning: "".into(),
                confidence: 0.5,
            },
        );
        let previous = HashMap::new();
        // Empty previous → trivially converged (first round)
        assert!(is_converged(&current, &previous, 0.8));
    }

    // ── format_debate_history tests ────────────────────────────────────────

    #[test]
    fn test_format_debate_history_single() {
        let mut votes = HashMap::new();
        votes.insert(
            "alice".to_string(),
            Vote {
                approves: true,
                reasoning: "Solid plan".into(),
                confidence: 0.85,
            },
        );
        let formatted = format_debate_history(&votes);
        assert!(formatted.contains("[APPROVE]"));
        assert!(formatted.contains("alice"));
        assert!(formatted.contains("Solid plan"));
        assert!(formatted.contains("0.85"));
    }

    #[test]
    fn test_format_debate_history_multiple() {
        let mut votes = HashMap::new();
        votes.insert(
            "alice".to_string(),
            Vote {
                approves: true,
                reasoning: "Looks good".into(),
                confidence: 0.9,
            },
        );
        votes.insert(
            "bob".to_string(),
            Vote {
                approves: false,
                reasoning: "Too risky".into(),
                confidence: 0.7,
            },
        );
        let formatted = format_debate_history(&votes);
        assert!(formatted.contains("[APPROVE]"));
        assert!(formatted.contains("[REJECT]"));
        assert!(formatted.contains("---"));
    }

    // ── DelphiConfig defaults ──────────────────────────────────────────────

    // ── VoteResult sanity ──────────────────────────────────────────────────

    #[test]
    fn test_vote_result_tracks_weighted_flag() {
        let vote = Vote {
            approves: true,
            reasoning: "test".into(),
            confidence: 0.5,
        };
        let mut votes = HashMap::new();
        votes.insert("a".to_string(), vote);
        let mut reps = HashMap::new();
        reps.insert("a".to_string(), 1.0);

        let result = weighted_vote(&votes, &reps, 0.6, 0.5);
        assert!(result.weighted);
        assert_eq!(result.participant_count, 1);
    }
}
