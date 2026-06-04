//! High-risk vote execution & escalation logic for chat processing.
//!
//! Extracted from `process_chat_request` in `impl/chat.rs` to reduce the
//! size of that function.  This module owns the multi-agent strong-model
//! vote pipeline including the escalation (multi-model) round.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::acp::helpers::autonomy_metrics::{record_vote_reputation_tiebreak, record_vote_winner};
use crate::acp::r#impl::chat::{
    normalize_vote_key, run_high_risk_vote_attempt, select_top_models, AgentStrongVoteOutcome,
    AgentVoteSource,
};
use crate::acp::server::AcpServer;
use crate::agent::Message;

/// Result of executing the high-risk vote pipeline (ballot + optional escalation).
#[allow(dead_code)] // F-GAP-49 — reserved for high-risk vote pipeline integration
pub(crate) struct HighRiskVoteExecutionResult {
    pub response_text: String,
    pub reasoning_text: String,
    pub selected_agent: String,
    pub last_err: Option<anyhow::Error>,
    pub vote_winner: Option<String>,
    pub vote_report: Option<Value>,
    pub used_multi_agent_vote: bool,
    pub used_multi_model_vote: bool,
    pub review_required: bool,
    pub emit_final_vote_response: bool,
    pub agent_vote_candidates: Vec<AgentStrongVoteOutcome>,
    pub agent_vote_failures: Vec<Value>,
    pub agent_vote_sources: Vec<AgentVoteSource>,
    /// Attempt logs from the vote-phase agent calls.
    pub agent_attempts: Vec<Value>,
}

/// Execute the high-risk vote pipeline.
///
/// 1. Runs `join_all` over `high_risk_vote_jobs` → `run_high_risk_vote_attempt()`
/// 2. Collects vote candidates, failures, sources
/// 3. Multi-agent strong model vote: counts votes, selects winner
/// 4. Records tiebreaks
/// 5. If `review_required && escalation_enabled`: runs escalation with `select_top_models()`
/// 6. Escalation winner selection
#[allow(clippy::too_many_arguments)]
#[allow(clippy::type_complexity)]
pub(crate) async fn execute_high_risk_vote(
    server: &AcpServer,
    phase_name: &str,
    trace_id: &str,
    high_risk_vote_jobs: Vec<(
        String,
        Arc<dyn crate::agent::Agent>,
        HashMap<String, Value>,
        Option<String>,
    )>,
    agent_messages: &[Message],
    phase_principles: Option<Vec<String>>,
    vote_timeout: Option<Duration>,
    cache_hit: bool,
    enable_high_risk_multi_agent_vote: bool,
    min_vote_agents: usize,
    max_vote_agents: usize,
    escalation_enabled: bool,
    escalation_models_per_agent: usize,
    escalation_max_agents: usize,
    reputation_scores: &HashMap<String, f64>,
    routing_provenance: &mut Vec<String>,
) -> HighRiskVoteExecutionResult {
    let mut response_text = String::new();
    let mut reasoning_text = String::new();
    let mut selected_agent = String::new();
    let mut last_err: Option<anyhow::Error> = None;
    let mut used_multi_agent_vote = false;
    let mut used_multi_model_vote = false;
    let mut review_required = false;
    let mut vote_report: Option<Value> = None;
    let mut agent_vote_candidates: Vec<AgentStrongVoteOutcome> = Vec::new();
    let mut agent_vote_failures: Vec<Value> = Vec::new();
    let mut agent_vote_sources: Vec<AgentVoteSource> = Vec::new();
    let mut emit_final_vote_response = false;
    let mut vote_winner: Option<String> = None;
    let mut agent_attempts: Vec<Value> = Vec::new();

    // ── Step 1 & 2: Run all vote attempts, collect results ──────────────
    if !high_risk_vote_jobs.is_empty() {
        let vote_results = join_all(high_risk_vote_jobs.into_iter().map(
            |(agent_name, agent, vote_options, strong_model)| {
                let server_ref = server;
                let agent_messages = agent_messages.to_vec();
                let phase_principles = phase_principles.clone();
                async move {
                    run_high_risk_vote_attempt(
                        server_ref,
                        phase_name,
                        trace_id,
                        agent_name,
                        agent,
                        agent_messages,
                        phase_principles,
                        vote_options,
                        vote_timeout,
                        strong_model,
                        "strong_model",
                    )
                    .await
                }
            },
        ))
        .await;

        for result in vote_results {
            agent_attempts.push(result.attempt_log);
            if let Some(candidate) = result.candidate {
                agent_vote_candidates.push(candidate);
            }
            if let Some(source) = result.source {
                agent_vote_sources.push(source);
            }
            if let Some(failure) = result.failure {
                if last_err.is_none() {
                    let default_reason = crate::i18n::runtime::t("error.chat.vote_attempt_failed");
                    let reason = failure
                        .get("reason")
                        .and_then(Value::as_str)
                        .unwrap_or(&default_reason);
                    last_err = Some(anyhow::anyhow!(crate::i18n::runtime::tf(
                        "error.chat.high_risk_vote_failed",
                        &[("reason", reason)]
                    )));
                }
                agent_vote_failures.push(failure);
            }
        }
    }

    // ── Step 3 & 4: Multi-agent strong-model vote counting and winner selection ──
    if !cache_hit
        && enable_high_risk_multi_agent_vote
        && agent_vote_candidates.len() >= min_vote_agents
        && response_text.is_empty()
    {
        let mut vote_counts: HashMap<String, usize> = HashMap::new();
        for candidate in &agent_vote_candidates {
            let key = normalize_vote_key(&candidate.response);
            let entry = vote_counts.entry(key).or_insert(0);
            *entry += 1;
        }

        let mut winner_index = 0usize;
        let mut winner_votes = 0usize;
        let mut winner_rep = 0.0f64;
        let mut winner_len = 0usize;
        for (idx, candidate) in agent_vote_candidates.iter().enumerate() {
            let key = normalize_vote_key(&candidate.response);
            let votes = vote_counts.get(&key).copied().unwrap_or(0);
            let rep = reputation_scores
                .get(&candidate.agent)
                .copied()
                .unwrap_or(0.5);
            let length = candidate.response.chars().count();
            if votes > winner_votes
                || (votes == winner_votes && rep > winner_rep)
                || (votes == winner_votes
                    && (rep - winner_rep).abs() < f64::EPSILON
                    && length > winner_len)
            {
                winner_index = idx;
                winner_votes = votes;
                winner_rep = rep;
                winner_len = length;
            }
        }

        // Record tiebreak if multiple candidates are tied for highest vote count
        let max_vote_count = winner_votes;
        let tied_candidates = agent_vote_candidates
            .iter()
            .filter(|candidate| {
                let key = normalize_vote_key(&candidate.response);
                vote_counts.get(&key).copied().unwrap_or(0) == max_vote_count
            })
            .count();
        if tied_candidates > 1 {
            record_vote_reputation_tiebreak();
            routing_provenance.push("vote_tiebreaked_by_reputation".to_string());
        }

        let winner = agent_vote_candidates[winner_index].clone();
        selected_agent = winner.agent.clone();
        response_text = winner.response.clone();
        reasoning_text = winner.reasoning.clone();
        used_multi_agent_vote = true;
        used_multi_model_vote = false;
        review_required = winner_votes * 2 <= agent_vote_candidates.len();
        last_err = None;
        emit_final_vote_response = true;

        let strong_vote_report = json!({
            "strategy": "multi_agent_strong_model_vote",
            "candidate_agents": agent_vote_candidates
                .iter()
                .map(|candidate| candidate.agent.clone())
                .collect::<Vec<_>>(),
            "candidate_details": agent_vote_candidates
                .iter()
                .map(|candidate| json!({
                    "agent": candidate.agent.clone(),
                    "model": candidate.model.clone(),
                }))
                .collect::<Vec<_>>(),
            "winner_agent": selected_agent.clone(),
            "winner_votes": winner_votes,
            "total_successes": agent_vote_candidates.len(),
            "min_vote_agents": min_vote_agents,
            "max_vote_agents": max_vote_agents,
            "failed_agents": agent_vote_failures,
            "review_required": review_required,
        });

        vote_report = Some(strong_vote_report.clone());
        vote_winner = Some("multi_agent_strong_model_vote".to_string());
        record_vote_winner("multi_agent_strong_model_vote");

        // ── Step 5 & 6: Escalation round (multi-model) ──────────────────
        if review_required && escalation_enabled {
            #[allow(clippy::type_complexity)]
            let mut escalation_jobs: Vec<(
                String,
                Arc<dyn crate::agent::Agent>,
                HashMap<String, Value>,
                Option<String>,
            )> = Vec::new();

            for (agent_name, agent, base_options) in
                agent_vote_sources.iter().take(escalation_max_agents)
            {
                if !agent.supports_model_override() {
                    continue;
                }

                for model_id in select_top_models(agent.as_ref(), escalation_models_per_agent) {
                    let mut model_options = base_options.clone();
                    model_options.insert("model".to_string(), Value::String(model_id.clone()));
                    escalation_jobs.push((
                        agent_name.clone(),
                        Arc::clone(agent),
                        model_options,
                        Some(model_id),
                    ));
                }
            }

            let escalation_results = join_all(escalation_jobs.into_iter().map(
                |(agent_name, agent, model_options, model_id)| {
                    let server_ref = server;
                    let agent_messages = agent_messages.to_vec();
                    let phase_principles = phase_principles.clone();
                    async move {
                        run_high_risk_vote_attempt(
                            server_ref,
                            phase_name,
                            trace_id,
                            agent_name,
                            agent,
                            agent_messages,
                            phase_principles,
                            model_options,
                            vote_timeout,
                            model_id,
                            "escalation",
                        )
                        .await
                    }
                },
            ))
            .await;

            let mut escalation_ballots: Vec<AgentStrongVoteOutcome> = Vec::new();
            let mut escalation_failures: Vec<Value> = Vec::new();

            for result in escalation_results {
                if let Some(ballot) = result.candidate {
                    escalation_ballots.push(ballot);
                }
                if let Some(failure) = result.failure {
                    escalation_failures.push(failure);
                }
            }

            if !escalation_ballots.is_empty() {
                let mut escalation_counts: HashMap<String, usize> = HashMap::new();
                for ballot in &escalation_ballots {
                    let key = normalize_vote_key(&ballot.response);
                    let entry = escalation_counts.entry(key).or_insert(0);
                    *entry += 1;
                }

                let mut escalation_winner_index = 0usize;
                let mut escalation_winner_votes = 0usize;
                let mut escalation_winner_rep = 0.0f64;
                let mut escalation_winner_len = 0usize;
                for (idx, ballot) in escalation_ballots.iter().enumerate() {
                    let key = normalize_vote_key(&ballot.response);
                    let votes = escalation_counts.get(&key).copied().unwrap_or(0);
                    let rep = reputation_scores.get(&ballot.agent).copied().unwrap_or(0.5);
                    let length = ballot.response.chars().count();
                    if votes > escalation_winner_votes
                        || (votes == escalation_winner_votes && rep > escalation_winner_rep)
                        || (votes == escalation_winner_votes
                            && (rep - escalation_winner_rep).abs() < f64::EPSILON
                            && length > escalation_winner_len)
                    {
                        escalation_winner_index = idx;
                        escalation_winner_votes = votes;
                        escalation_winner_rep = rep;
                        escalation_winner_len = length;
                    }
                }
                let escalation_max_vote_count = escalation_winner_votes;
                let escalation_tied_candidates = escalation_ballots
                    .iter()
                    .filter(|ballot| {
                        let key = normalize_vote_key(&ballot.response);
                        escalation_counts.get(&key).copied().unwrap_or(0)
                            == escalation_max_vote_count
                    })
                    .count();
                if escalation_tied_candidates > 1 {
                    record_vote_reputation_tiebreak();
                    routing_provenance.push("escalation_vote_tiebreaked_by_reputation".to_string());
                }

                let escalation_winner = escalation_ballots[escalation_winner_index].clone();
                selected_agent = escalation_winner.agent.clone();
                response_text = escalation_winner.response.clone();
                reasoning_text = escalation_winner.reasoning.clone();
                used_multi_model_vote = true;
                review_required = escalation_winner_votes * 2 <= escalation_ballots.len();
                last_err = None;
                emit_final_vote_response = true;

                vote_report = Some(json!({
                    "strategy": "multi_agent_multi_model_escalation",
                    "strong_round": strong_vote_report,
                    "candidate_details": escalation_ballots
                        .iter()
                        .map(|ballot| json!({
                            "agent": ballot.agent.clone(),
                            "model": ballot.model.clone(),
                        }))
                        .collect::<Vec<_>>(),
                    "winner_agent": selected_agent.clone(),
                    "winner_model": escalation_winner.model,
                    "winner_votes": escalation_winner_votes,
                    "total_successes": escalation_ballots.len(),
                    "max_agents": escalation_max_agents,
                    "models_per_agent": escalation_models_per_agent,
                    "failed_ballots": escalation_failures,
                    "review_required": review_required,
                }));
                vote_winner = Some("multi_agent_multi_model_escalation".to_string());
                record_vote_winner("multi_agent_multi_model_escalation");
            }
        }
    }

    HighRiskVoteExecutionResult {
        response_text,
        reasoning_text,
        selected_agent,
        last_err,
        vote_winner,
        vote_report,
        used_multi_agent_vote,
        used_multi_model_vote,
        review_required,
        emit_final_vote_response,
        agent_vote_candidates,
        agent_vote_failures,
        agent_vote_sources,
        agent_attempts,
    }
}

// ── Upgraded: Weighted & Delphi-Debate Voting ───────────────────────────────

/// Voting mode for the upgraded high-risk vote executor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VoteMode {
    /// Original count-based voting with reputation tiebreaker (default).
    Legacy,
    /// Weighted reputation voting — each candidate's response vote is
    /// weighted by the agent's reputation score from the reputation store.
    Weighted,
    /// Delphi-method debate rounds (requires async re-vote callback).
    /// Falls back to Weighted if the callback is not provided.
    DelphiDebate,
}

impl Default for VoteMode {
    fn default() -> Self {
        Self::Weighted
    }
}

/// Configuration for the upgraded voting executor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoteExecutionConfig {
    /// Which voting mode to use.
    pub mode: VoteMode,
    /// Approval threshold (0.0–1.0) for weighted voting. Default: 0.6.
    pub threshold: f64,
    /// Default reputation weight for agents not found in the reputation store.
    pub default_weight: f64,
    /// Maximum Delphi debate rounds (only used in DelphiDebate mode).
    pub max_delphi_rounds: usize,
    /// Convergence ratio for Delphi debate — fraction of unchanged votes
    /// required to consider the debate converged. Default: 0.8.
    pub convergence_ratio: f64,
}

impl Default for VoteExecutionConfig {
    fn default() -> Self {
        Self {
            mode: VoteMode::Weighted,
            threshold: 0.6,
            default_weight: 0.5,
            max_delphi_rounds: 2,
            convergence_ratio: 0.8,
        }
    }
}

/// A weighted vote entry for computing reputation-weighted outcomes.
#[derive(Debug, Clone)]
struct WeightedBallot {
    /// Index into the original candidates/ballots list.
    index: usize,
    /// Normalized response key (from `normalize_vote_key`).
    response_key: String,
    /// The agent's reputation weight.
    weight: f64,
}

/// Compute the weighted winner from a set of candidates using reputation scores.
///
/// Instead of counting each candidate's response equally, each response's
/// vote weight is the sum of reputation scores of agents who gave that response.
/// The response with the highest total reputation weight wins.
fn compute_weighted_winner(
    candidates: &[AgentStrongVoteOutcome],
    reputation_scores: &HashMap<String, f64>,
    default_weight: f64,
) -> (usize, f64, f64) {
    if candidates.is_empty() {
        return (0, 0.0, 0.0);
    }

    // Build weighted ballots: each candidate vote is weighted by agent reputation
    let mut ballots: Vec<WeightedBallot> = Vec::with_capacity(candidates.len());
    let mut response_weight: HashMap<String, f64> = HashMap::new();

    for (idx, candidate) in candidates.iter().enumerate() {
        let key = normalize_vote_key(&candidate.response);
        let weight = reputation_scores
            .get(&candidate.agent)
            .copied()
            .unwrap_or(default_weight);

        ballots.push(WeightedBallot {
            index: idx,
            response_key: key.clone(),
            weight,
        });
        *response_weight.entry(key).or_insert(0.0) += weight;
    }

    // Find the response_key with the highest total reputation weight
    let (winning_key, max_weight) = response_weight
        .into_iter()
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .unwrap_or_default();

    // Find the first ballot matching the winning key (prefer higher weight if tied)
    let mut best_idx = 0usize;
    let mut best_individual_weight = 0.0f64;
    for ballot in &ballots {
        if ballot.response_key == winning_key && ballot.weight > best_individual_weight {
            best_idx = ballot.index;
            best_individual_weight = ballot.weight;
        }
    }

    // Compute approval ratio: winning weight / total weight
    let total_weight: f64 = ballots.iter().map(|b| b.weight).sum();
    let approval_ratio = if total_weight > 0.0 {
        max_weight / total_weight
    } else {
        0.0
    };

    (best_idx, max_weight, approval_ratio)
}

/// Execute the high-risk vote pipeline using **weighted reputation voting**.
///
/// This is the upgraded version of [`execute_high_risk_vote`]. Instead of
/// simple count-based voting with reputation as tiebreaker, it weights each
/// agent's vote by their reputation score — so high-reputation agents have
/// proportionally greater influence on the outcome.
///
/// # Arguments
///
/// All arguments are the same as [`execute_high_risk_vote`], plus:
/// * `vote_config` – [`VoteExecutionConfig`] controlling mode and threshold.
///
/// Returns the same [`HighRiskVoteExecutionResult`] structure.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::type_complexity)]
pub(crate) async fn execute_high_risk_vote_weighted(
    server: &AcpServer,
    phase_name: &str,
    trace_id: &str,
    high_risk_vote_jobs: Vec<(
        String,
        Arc<dyn crate::agent::Agent>,
        HashMap<String, Value>,
        Option<String>,
    )>,
    agent_messages: &[Message],
    phase_principles: Option<Vec<String>>,
    vote_timeout: Option<Duration>,
    cache_hit: bool,
    enable_high_risk_multi_agent_vote: bool,
    min_vote_agents: usize,
    max_vote_agents: usize,
    escalation_enabled: bool,
    escalation_models_per_agent: usize,
    escalation_max_agents: usize,
    reputation_scores: &HashMap<String, f64>,
    routing_provenance: &mut Vec<String>,
    vote_config: &VoteExecutionConfig,
) -> HighRiskVoteExecutionResult {
    // Delegate to legacy if configured
    if vote_config.mode == VoteMode::Legacy {
        return execute_high_risk_vote(
            server,
            phase_name,
            trace_id,
            high_risk_vote_jobs,
            agent_messages,
            phase_principles,
            vote_timeout,
            cache_hit,
            enable_high_risk_multi_agent_vote,
            min_vote_agents,
            max_vote_agents,
            escalation_enabled,
            escalation_models_per_agent,
            escalation_max_agents,
            reputation_scores,
            routing_provenance,
        )
        .await;
    }

    // ── Step 1 & 2: Run all vote attempts, collect results ──────────────
    // (Same as original execute_high_risk_vote)

    let mut response_text = String::new();
    let mut reasoning_text = String::new();
    let mut selected_agent = String::new();
    let mut last_err: Option<anyhow::Error> = None;
    let mut used_multi_agent_vote = false;
    let mut used_multi_model_vote = false;
    let mut review_required = false;
    let mut vote_report: Option<Value> = None;
    let mut agent_vote_candidates: Vec<AgentStrongVoteOutcome> = Vec::new();
    let mut agent_vote_failures: Vec<Value> = Vec::new();
    let mut agent_vote_sources: Vec<AgentVoteSource> = Vec::new();
    let mut emit_final_vote_response = false;
    let mut vote_winner: Option<String> = None;
    let mut agent_attempts: Vec<Value> = Vec::new();

    if !high_risk_vote_jobs.is_empty() {
        let vote_results = join_all(high_risk_vote_jobs.into_iter().map(
            |(agent_name, agent, vote_options, strong_model)| {
                let server_ref = server;
                let agent_messages = agent_messages.to_vec();
                let phase_principles = phase_principles.clone();
                async move {
                    run_high_risk_vote_attempt(
                        server_ref,
                        phase_name,
                        trace_id,
                        agent_name,
                        agent,
                        agent_messages,
                        phase_principles,
                        vote_options,
                        vote_timeout,
                        strong_model,
                        "strong_model",
                    )
                    .await
                }
            },
        ))
        .await;

        for result in vote_results {
            agent_attempts.push(result.attempt_log);
            if let Some(candidate) = result.candidate {
                agent_vote_candidates.push(candidate);
            }
            if let Some(source) = result.source {
                agent_vote_sources.push(source);
            }
            if let Some(failure) = result.failure {
                if last_err.is_none() {
                    let default_reason = crate::i18n::runtime::t("error.chat.vote_attempt_failed");
                    let reason = failure
                        .get("reason")
                        .and_then(Value::as_str)
                        .unwrap_or(&default_reason);
                    last_err = Some(anyhow::anyhow!(crate::i18n::runtime::tf(
                        "error.chat.high_risk_vote_failed",
                        &[("reason", reason)]
                    )));
                }
                agent_vote_failures.push(failure);
            }
        }
    }

    // ── Step 3: Weighted reputation winner selection ──────────────────────
    if !cache_hit
        && enable_high_risk_multi_agent_vote
        && agent_vote_candidates.len() >= min_vote_agents
        && response_text.is_empty()
    {
        let (winner_index, _winner_weight, approval_ratio) = compute_weighted_winner(
            &agent_vote_candidates,
            reputation_scores,
            vote_config.default_weight,
        );

        let winner = agent_vote_candidates[winner_index].clone();
        selected_agent = winner.agent.clone();
        response_text = winner.response.clone();
        reasoning_text = winner.reasoning.clone();
        used_multi_agent_vote = true;
        used_multi_model_vote = false;
        review_required = approval_ratio < vote_config.threshold;
        last_err = None;
        emit_final_vote_response = true;

        routing_provenance.push(format!(
            "weighted_vote_winner:{}_ratio:{:.3}_threshold:{:.3}",
            selected_agent, approval_ratio, vote_config.threshold
        ));

        let weighted_vote_report = json!({
            "strategy": "weighted_reputation_vote",
            "vote_mode": format!("{:?}", vote_config.mode),
            "candidate_agents": agent_vote_candidates
                .iter()
                .map(|candidate| candidate.agent.clone())
                .collect::<Vec<_>>(),
            "candidate_details": agent_vote_candidates
                .iter()
                .map(|candidate| json!({
                    "agent": candidate.agent.clone(),
                    "model": candidate.model.clone(),
                    "reputation": reputation_scores.get(&candidate.agent).copied().unwrap_or(vote_config.default_weight),
                }))
                .collect::<Vec<_>>(),
            "winner_agent": selected_agent.clone(),
            "approval_ratio": approval_ratio,
            "threshold": vote_config.threshold,
            "total_successes": agent_vote_candidates.len(),
            "min_vote_agents": min_vote_agents,
            "max_vote_agents": max_vote_agents,
            "failed_agents": agent_vote_failures,
            "review_required": review_required,
        });

        vote_report = Some(weighted_vote_report.clone());
        vote_winner = Some("weighted_reputation_vote".to_string());
        record_vote_winner("weighted_reputation_vote");

        // ── Step 5 & 6: Escalation round (multi-model) ──────────────────
        if review_required && escalation_enabled {
            #[allow(clippy::type_complexity)]
            let mut escalation_jobs: Vec<(
                String,
                Arc<dyn crate::agent::Agent>,
                HashMap<String, Value>,
                Option<String>,
            )> = Vec::new();

            for (agent_name, agent, base_options) in
                agent_vote_sources.iter().take(escalation_max_agents)
            {
                if !agent.supports_model_override() {
                    continue;
                }

                for model_id in select_top_models(agent.as_ref(), escalation_models_per_agent) {
                    let mut model_options = base_options.clone();
                    model_options.insert("model".to_string(), Value::String(model_id.clone()));
                    escalation_jobs.push((
                        agent_name.clone(),
                        Arc::clone(agent),
                        model_options,
                        Some(model_id),
                    ));
                }
            }

            let escalation_results = join_all(escalation_jobs.into_iter().map(
                |(agent_name, agent, model_options, model_id)| {
                    let server_ref = server;
                    let agent_messages = agent_messages.to_vec();
                    let phase_principles = phase_principles.clone();
                    async move {
                        run_high_risk_vote_attempt(
                            server_ref,
                            phase_name,
                            trace_id,
                            agent_name,
                            agent,
                            agent_messages,
                            phase_principles,
                            model_options,
                            vote_timeout,
                            model_id,
                            "escalation",
                        )
                        .await
                    }
                },
            ))
            .await;

            let mut escalation_ballots: Vec<AgentStrongVoteOutcome> = Vec::new();
            let mut escalation_failures: Vec<Value> = Vec::new();

            for result in escalation_results {
                if let Some(ballot) = result.candidate {
                    escalation_ballots.push(ballot);
                }
                if let Some(failure) = result.failure {
                    escalation_failures.push(failure);
                }
            }

            if !escalation_ballots.is_empty() {
                let (esc_winner_index, _esc_weight, esc_approval_ratio) = compute_weighted_winner(
                    &escalation_ballots,
                    reputation_scores,
                    vote_config.default_weight,
                );

                let escalation_winner = escalation_ballots[esc_winner_index].clone();
                selected_agent = escalation_winner.agent.clone();
                response_text = escalation_winner.response.clone();
                reasoning_text = escalation_winner.reasoning.clone();
                used_multi_model_vote = true;
                review_required = esc_approval_ratio < vote_config.threshold;
                last_err = None;
                emit_final_vote_response = true;

                routing_provenance.push(format!(
                    "escalation_weighted_vote_winner:{}_ratio:{:.3}",
                    selected_agent, esc_approval_ratio
                ));

                vote_report = Some(json!({
                    "strategy": "weighted_reputation_escalation",
                    "weighted_round": weighted_vote_report,
                    "candidate_details": escalation_ballots
                        .iter()
                        .map(|ballot| json!({
                            "agent": ballot.agent.clone(),
                            "model": ballot.model.clone(),
                            "reputation": reputation_scores.get(&ballot.agent).copied().unwrap_or(vote_config.default_weight),
                        }))
                        .collect::<Vec<_>>(),
                    "winner_agent": selected_agent.clone(),
                    "winner_model": escalation_winner.model,
                    "approval_ratio": esc_approval_ratio,
                    "threshold": vote_config.threshold,
                    "total_successes": escalation_ballots.len(),
                    "max_agents": escalation_max_agents,
                    "models_per_agent": escalation_models_per_agent,
                    "failed_ballots": escalation_failures,
                    "review_required": review_required,
                }));
                vote_winner = Some("weighted_reputation_escalation".to_string());
                record_vote_winner("weighted_reputation_escalation");
            }
        }
    }

    HighRiskVoteExecutionResult {
        response_text,
        reasoning_text,
        selected_agent,
        last_err,
        vote_winner,
        vote_report,
        used_multi_agent_vote,
        used_multi_model_vote,
        review_required,
        emit_final_vote_response,
        agent_vote_candidates,
        agent_vote_failures,
        agent_vote_sources,
        agent_attempts,
    }
}
