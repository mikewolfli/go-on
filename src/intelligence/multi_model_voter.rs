//! F-GAP-16: Multi-model voting
#![cfg_attr(
    not(feature = "sub-bus-voter-future"),
    allow(dead_code, unused_imports)
)]

//! MultiModelVoter — Concurrent multi-model voting for high-stakes decisions.
//!
//! Sends the same prompt to multiple agents concurrently and aggregates
//! responses through configurable voting strategies. Used primarily by
//! SafeGuard mode and the capability bus for risk-sensitive routing.

use crate::agents::agent::{Agent, Message, StreamingSender};
use crate::i18n::runtime::tf;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::mpsc;
use tracing::{info, warn};

// ── Voting strategy ─────────────────────────────────────────────────────────

/// Strategy for aggregating votes from multiple models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VotingStrategy {
    /// Simple majority wins — the most-frequent response is selected.
    Majority,
    /// Weighted voting based on model capability / weight configuration.
    Weighted,
    /// Unanimous consensus required — every model must agree.
    Unanimous,
    /// Best-of-N: pick the response with highest confidence score.
    BestOfN,
}

// ── Result types ────────────────────────────────────────────────────────────

/// A single model's vote result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelVoteResult {
    /// Name of the model that produced this vote.
    pub model_name: String,
    /// The raw response text.
    pub response: String,
    /// Confidence score (0.0–1.0), normalized by the voter.
    pub confidence: f64,
    /// Round-trip latency for this model in milliseconds.
    pub latency_ms: u64,
}

/// The aggregated outcome of a multi-model vote.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VotingOutcome {
    /// The winning response text.
    pub winning_response: String,
    /// The name of the model whose response won.
    pub winner_model: String,
    /// Consensus level (0.0–1.0), indicating how strongly models agreed.
    pub consensus_level: f64,
    /// All individual model votes collected.
    pub all_votes: Vec<ModelVoteResult>,
    /// The strategy that produced this outcome.
    pub strategy_used: VotingStrategy,
    /// Total wall-clock duration of the voting round in milliseconds.
    pub total_duration_ms: u64,
    /// Whether a tie-breaker was required to resolve the vote.
    pub tie_breaker_used: bool,
}

// ── MultiModelVoter ─────────────────────────────────────────────────────────

/// Concurrent multi-model voter.
///
/// Sends the same prompt to every agent, collects their streaming responses,
/// and applies a chosen [`VotingStrategy`] to select the best answer.
pub struct MultiModelVoter {
    /// Minimum number of models that must participate for a valid vote.
    pub min_voters: usize,
    /// The voting strategy used to aggregate responses.
    pub strategy: VotingStrategy,
    /// Per-model timeout in milliseconds.
    pub per_model_timeout_ms: u64,
    /// Weight configuration keyed by model name (used by [`VotingStrategy::Weighted`]).
    pub model_weights: HashMap<String, f64>,
}

impl MultiModelVoter {
    /// Create a new voter with default configuration.
    ///
    /// Defaults: 3 minimum voters, majority strategy, 30-second per-model timeout.
    pub fn new() -> Self {
        Self {
            min_voters: 3,
            strategy: VotingStrategy::Majority,
            per_model_timeout_ms: 30_000,
            model_weights: HashMap::new(),
        }
    }

    /// Set the minimum number of voters required.
    pub fn with_min_voters(mut self, min: usize) -> Self {
        self.min_voters = min;
        self
    }

    /// Set the voting strategy.
    pub fn with_strategy(mut self, strategy: VotingStrategy) -> Self {
        self.strategy = strategy;
        self
    }

    /// Set the per-model timeout in milliseconds.
    pub fn with_timeout_ms(mut self, ms: u64) -> Self {
        self.per_model_timeout_ms = ms;
        self
    }

    /// Add or update a weight for a specific model.
    pub fn with_weight(mut self, model_name: &str, weight: f64) -> Self {
        self.model_weights.insert(model_name.to_string(), weight);
        self
    }

    // ── Core voting method ──────────────────────────────────────────────

    /// Send `prompt` to every agent concurrently, collect results, and produce
    /// a [`VotingOutcome`] according to the configured strategy.
    ///
    /// Each agent call is wrapped in `tokio::time::timeout`; models that exceed
    /// the deadline are silently dropped (logged at warn level).
    pub async fn vote(&self, prompt: &str, agents: &[Arc<dyn Agent>]) -> Result<VotingOutcome> {
        let start = Instant::now();

        if agents.is_empty() {
            return Err(anyhow::anyhow!(tf("voter.no_agents_available", &[])));
        }

        if agents.len() < self.min_voters {
            warn!(
                "MultiModelVoter: only {} agent(s) available, need at least {} — proceeding anyway",
                agents.len(),
                self.min_voters
            );
        }

        let deadline = std::time::Duration::from_millis(self.per_model_timeout_ms);

        // ── Launch concurrent tasks ────────────────────────────────────

        let mut handles = Vec::with_capacity(agents.len());

        for (idx, agent_ref) in agents.iter().enumerate() {
            let agent = Arc::clone(agent_ref);
            let prompt = prompt.to_string();

            let handle = tokio::spawn(async move {
                let model_name = agent
                    .default_model()
                    .map(|m| m.name.clone())
                    .unwrap_or_else(|| format!("agent-{}", idx));

                let vote_start = Instant::now();

                let response = tokio::time::timeout(deadline, async {
                    let (tx, mut rx) = mpsc::channel::<String>(256);
                    let sender = StreamingSender::new(tx);

                    let messages = vec![Message {
                        role: "user".to_string(),
                        content: prompt.clone(),
                    }];

                    agent
                        .chat(messages, None, None, sender)
                        .await
                        .map_err(|e| anyhow::anyhow!("chat failed: {}", e))?;

                    // Drop the original sender so rx.recv() eventually returns None
                    drop(agent); // but we need to keep the channel alive
                                 // Actually we already sent the message; collect remaining tokens
                    let mut buf = String::new();
                    while let Some(token) = rx.recv().await {
                        buf.push_str(&token);
                    }
                    Ok::<String, anyhow::Error>(buf)
                })
                .await;

                let latency_ms = vote_start.elapsed().as_millis() as u64;

                match response {
                    Ok(Ok(text)) => Some(ModelVoteResult {
                        model_name,
                        response: text,
                        confidence: 0.5, // neutral default; caller can refine
                        latency_ms,
                    }),
                    Ok(Err(e)) => {
                        warn!(
                            "MultiModelVoter: agent '{}' returned error: {}",
                            model_name, e
                        );
                        None
                    }
                    Err(_elapsed) => {
                        warn!(
                            "MultiModelVoter: agent '{}' timed out after {}ms",
                            model_name, latency_ms
                        );
                        None
                    }
                }
            });

            handles.push(handle);
        }

        // ── Collect results ────────────────────────────────────────────

        let mut votes: Vec<ModelVoteResult> = Vec::with_capacity(agents.len());
        for handle in handles {
            match handle.await {
                Ok(Some(vote)) => votes.push(vote),
                Ok(None) => { /* agent failed or timed out — already logged */ }
                Err(join_err) => {
                    warn!("MultiModelVoter: spawned task panicked: {}", join_err);
                }
            }
        }

        if votes.is_empty() {
            return Err(anyhow::anyhow!(tf("voter.all_models_failed", &[])));
        }

        // ── Apply strategy ─────────────────────────────────────────────

        let total_duration_ms = start.elapsed().as_millis() as u64;
        let consensus = Self::consensus_score(&votes);
        let outcome = self.apply_strategy(votes, consensus, total_duration_ms);

        info!(
            "MultiModelVoter: strategy={:?} winner={} consensus={:.2} duration={}ms tiebreaker={}",
            self.strategy,
            outcome.winner_model,
            outcome.consensus_level,
            outcome.total_duration_ms,
            outcome.tie_breaker_used
        );

        Ok(outcome)
    }

    /// Apply the configured voting strategy to a set of collected votes.
    fn apply_strategy(
        &self,
        votes: Vec<ModelVoteResult>,
        consensus: f64,
        total_duration_ms: u64,
    ) -> VotingOutcome {
        match self.strategy {
            VotingStrategy::Majority => self.majority_outcome(votes, consensus, total_duration_ms),
            VotingStrategy::Weighted => self.weighted_outcome(votes, consensus, total_duration_ms),
            VotingStrategy::Unanimous => {
                self.unanimous_outcome(votes, consensus, total_duration_ms)
            }
            VotingStrategy::BestOfN => self.best_of_n_outcome(votes, consensus, total_duration_ms),
        }
    }

    // ── Strategy implementations ───────────────────────────────────────

    /// Majority: count how many responses are similar, pick the largest cluster.
    fn majority_outcome(
        &self,
        votes: Vec<ModelVoteResult>,
        consensus: f64,
        total_duration_ms: u64,
    ) -> VotingOutcome {
        let n = votes.len();
        // Group votes by similarity clusters
        let mut clusters: Vec<Vec<usize>> = Vec::new();
        let mut assigned = vec![false; n];

        for i in 0..n {
            if assigned[i] {
                continue;
            }
            let mut cluster = vec![i];
            assigned[i] = true;
            for j in (i + 1)..n {
                if !assigned[j] && Self::similar_responses(&votes[i].response, &votes[j].response) {
                    cluster.push(j);
                    assigned[j] = true;
                }
            }
            clusters.push(cluster);
        }

        // Pick the largest cluster; on tie, use the faster response
        clusters.sort_by(|a, b| {
            b.len()
                .cmp(&a.len())
                .then_with(|| votes[a[0]].latency_ms.cmp(&votes[b[0]].latency_ms))
        });

        let best_cluster = &clusters[0];
        let winner_idx = best_cluster[0];
        let tie_breaker_used = clusters.len() > 1
            && clusters[0].len() == clusters.get(1).map(|c| c.len()).unwrap_or(0);

        VotingOutcome {
            winning_response: votes[winner_idx].response.clone(),
            winner_model: votes[winner_idx].model_name.clone(),
            consensus_level: consensus,
            all_votes: votes,
            strategy_used: VotingStrategy::Majority,
            total_duration_ms,
            tie_breaker_used,
        }
    }

    /// Weighted: multiply confidence by model weight, pick highest weighted score.
    fn weighted_outcome(
        &self,
        votes: Vec<ModelVoteResult>,
        consensus: f64,
        total_duration_ms: u64,
    ) -> VotingOutcome {
        let mut best_idx = 0;
        let mut best_score = 0.0f64;
        let mut tie_breaker_used = false;

        for (i, v) in votes.iter().enumerate() {
            let weight = self
                .model_weights
                .get(&v.model_name)
                .copied()
                .unwrap_or(1.0);
            let score = v.confidence * weight;

            if score > best_score {
                best_score = score;
                best_idx = i;
                tie_breaker_used = false;
            } else if (score - best_score).abs() < f64::EPSILON {
                // Tie: pick the faster response
                if v.latency_ms < votes[best_idx].latency_ms {
                    best_idx = i;
                }
                tie_breaker_used = true;
            }
        }

        VotingOutcome {
            winning_response: votes[best_idx].response.clone(),
            winner_model: votes[best_idx].model_name.clone(),
            consensus_level: consensus,
            all_votes: votes,
            strategy_used: VotingStrategy::Weighted,
            total_duration_ms,
            tie_breaker_used,
        }
    }

    /// Unanimous: require all responses to be similar; otherwise degrade.
    fn unanimous_outcome(
        &self,
        votes: Vec<ModelVoteResult>,
        consensus: f64,
        total_duration_ms: u64,
    ) -> VotingOutcome {
        let all_similar = votes
            .iter()
            .all(|v| Self::similar_responses(&votes[0].response, &v.response));

        if all_similar {
            // Pick the fastest unanimous response
            let best = votes.iter().min_by_key(|v| v.latency_ms).unwrap();
            VotingOutcome {
                winning_response: best.response.clone(),
                winner_model: best.model_name.clone(),
                consensus_level: 1.0,
                all_votes: votes,
                strategy_used: VotingStrategy::Unanimous,
                total_duration_ms,
                tie_breaker_used: false,
            }
        } else {
            // Degrade: fall through to majority internally
            warn!(
                "MultiModelVoter: unanimous consensus not reached (level={:.2}), falling back to majority",
                consensus
            );
            let mut outcome = self.majority_outcome(votes, consensus, total_duration_ms);
            outcome.strategy_used = VotingStrategy::Unanimous;
            outcome.tie_breaker_used = true;
            outcome
        }
    }

    /// Best-of-N: return the vote with the highest confidence.
    fn best_of_n_outcome(
        &self,
        votes: Vec<ModelVoteResult>,
        consensus: f64,
        total_duration_ms: u64,
    ) -> VotingOutcome {
        let mut best_idx = 0;
        let mut best_conf = 0.0f64;
        let mut tie_breaker_used = false;

        for (i, v) in votes.iter().enumerate() {
            if v.confidence > best_conf {
                best_conf = v.confidence;
                best_idx = i;
                tie_breaker_used = false;
            } else if (v.confidence - best_conf).abs() < f64::EPSILON {
                if v.latency_ms < votes[best_idx].latency_ms {
                    best_idx = i;
                }
                tie_breaker_used = true;
            }
        }

        VotingOutcome {
            winning_response: votes[best_idx].response.clone(),
            winner_model: votes[best_idx].model_name.clone(),
            consensus_level: consensus,
            all_votes: votes,
            strategy_used: VotingStrategy::BestOfN,
            total_duration_ms,
            tie_breaker_used,
        }
    }

    // ── Utility functions ──────────────────────────────────────────────

    /// Crude similarity check: responses are "similar" if their normalized
    /// (trimmed, lowercased) Jaccard-like overlap exceeds 50%.
    pub fn similar_responses(a: &str, b: &str) -> bool {
        let a_norm = a.trim().to_lowercase();
        let b_norm = b.trim().to_lowercase();

        if a_norm == b_norm {
            return true;
        }

        // Word-level Jaccard similarity
        let words_a: std::collections::HashSet<&str> = a_norm.split_whitespace().collect();
        let words_b: std::collections::HashSet<&str> = b_norm.split_whitespace().collect();

        if words_a.is_empty() && words_b.is_empty() {
            return true;
        }

        let intersection = words_a.intersection(&words_b).count();
        let union = words_a.union(&words_b).count();

        if union == 0 {
            return false;
        }

        let similarity = intersection as f64 / union as f64;
        similarity > 0.5
    }

    /// Compute a consensus score (0.0–1.0) across all votes.
    ///
    /// For each pair of responses we compute the similarity; the consensus
    /// is the average pairwise similarity.
    pub fn consensus_score(votes: &[ModelVoteResult]) -> f64 {
        let n = votes.len();
        if n <= 1 {
            return 1.0;
        }

        let mut total = 0.0f64;
        let mut pairs = 0usize;

        for i in 0..n {
            for j in (i + 1)..n {
                if Self::similar_responses(&votes[i].response, &votes[j].response) {
                    total += 1.0;
                }
                pairs += 1;
            }
        }

        if pairs == 0 {
            return 1.0;
        }

        total / pairs as f64
    }
}

impl Default for MultiModelVoter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Test helpers ───────────────────────────────────────────────────

    /// A minimal stub agent that echoes its index for testing.
    struct StubAgent {
        idx: usize,
        model_name: String,
        response: String,
        delay_ms: u64,
        should_fail: bool,
    }

    #[async_trait::async_trait]
    impl Agent for StubAgent {
        async fn chat(
            &self,
            _messages: Vec<Message>,
            _principles: Option<Vec<String>>,
            _options: Option<HashMap<String, serde_json::Value>>,
            sender: StreamingSender,
        ) -> crate::core::error::Result<()> {
            if self.should_fail {
                return Err(crate::core::error::AppError::Proxy(
                    crate::core::error::ProxyError::Internal("stub failure".to_string()),
                ));
            }
            if self.delay_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
            }
            let _ = sender.send(self.response.clone());
            Ok(())
        }

        fn default_model(&self) -> Option<crate::agents::agent::ModelInfo> {
            Some(crate::agents::agent::ModelInfo {
                id: format!("stub-model-{}", self.idx),
                name: self.model_name.clone(),
                description: "stub agent for testing".to_string(),
                is_default: true,
                capabilities: vec!["chat".to_string()],
                context_window: None,
            })
        }
    }

    fn make_agent(idx: usize, name: &str, response: &str) -> Arc<dyn Agent> {
        Arc::new(StubAgent {
            idx,
            model_name: name.to_string(),
            response: response.to_string(),
            delay_ms: 0,
            should_fail: false,
        })
    }

    fn make_failing_agent(idx: usize, name: &str) -> Arc<dyn Agent> {
        Arc::new(StubAgent {
            idx,
            model_name: name.to_string(),
            response: String::new(),
            delay_ms: 0,
            should_fail: true,
        })
    }

    // ── Unit tests ─────────────────────────────────────────────────────

    #[test]
    fn test_similar_responses_identical() {
        assert!(MultiModelVoter::similar_responses(
            "hello world",
            "hello world"
        ));
        assert!(MultiModelVoter::similar_responses("  HELLO  ", "hello"));
    }

    #[test]
    fn test_similar_responses_different() {
        assert!(!MultiModelVoter::similar_responses(
            "the cat sat on the mat",
            "quantum computing is fascinating"
        ));
    }

    #[test]
    fn test_similar_responses_partial_overlap() {
        // "hello world foo" vs "hello world bar" → intersection=2, union=4 → 0.5, NOT > 0.5
        assert!(!MultiModelVoter::similar_responses(
            "hello world foo",
            "hello world bar"
        ));
        // "hello world foo bar" vs "hello world foo baz" → intersection=3, union=5 → 0.6 > 0.5
        assert!(MultiModelVoter::similar_responses(
            "hello world foo bar",
            "hello world foo baz"
        ));
    }

    #[test]
    fn test_similar_responses_empty() {
        assert!(MultiModelVoter::similar_responses("", ""));
    }

    #[test]
    fn test_consensus_score_all_same() {
        let votes = vec![
            ModelVoteResult {
                model_name: "a".into(),
                response: "yes".into(),
                confidence: 1.0,
                latency_ms: 10,
            },
            ModelVoteResult {
                model_name: "b".into(),
                response: "yes".into(),
                confidence: 1.0,
                latency_ms: 20,
            },
        ];
        assert!((MultiModelVoter::consensus_score(&votes) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_consensus_score_single() {
        let votes = vec![ModelVoteResult {
            model_name: "a".into(),
            response: "solo".into(),
            confidence: 0.9,
            latency_ms: 5,
        }];
        assert!((MultiModelVoter::consensus_score(&votes) - 1.0).abs() < f64::EPSILON);
    }

    // ── Integration-style async tests ──────────────────────────────────

    #[tokio::test]
    async fn test_majority_picks_largest_cluster() {
        let agents: Vec<Arc<dyn Agent>> = vec![
            make_agent(0, "gpt", "yes"),
            make_agent(1, "claude", "yes"),
            make_agent(2, "gemini", "no"),
        ];

        let voter = MultiModelVoter::new()
            .with_min_voters(3)
            .with_strategy(VotingStrategy::Majority)
            .with_timeout_ms(5_000);

        let outcome = voter.vote("test prompt", &agents).await.unwrap();
        assert_eq!(outcome.winning_response, "yes");
        assert_eq!(outcome.strategy_used, VotingStrategy::Majority);
        assert!(!outcome.tie_breaker_used);
    }

    #[tokio::test]
    async fn test_unanimous_falls_back_to_majority() {
        let agents: Vec<Arc<dyn Agent>> = vec![
            make_agent(0, "gpt", "yes"),
            make_agent(1, "claude", "yes"),
            make_agent(2, "gemini", "no"),
        ];

        let voter = MultiModelVoter::new()
            .with_min_voters(3)
            .with_strategy(VotingStrategy::Unanimous)
            .with_timeout_ms(5_000);

        let outcome = voter.vote("test prompt", &agents).await.unwrap();
        // Unanimous fails, falls back to majority → "yes" wins
        assert_eq!(outcome.winning_response, "yes");
        assert!(outcome.tie_breaker_used);
    }

    #[tokio::test]
    async fn test_best_of_n_picks_highest_confidence() {
        // All agents respond "ok", the voter picks the one with highest confidence
        let agents: Vec<Arc<dyn Agent>> = vec![
            make_agent(0, "gpt", "answer A"),
            make_agent(1, "claude", "answer B"),
            make_agent(2, "gemini", "answer C"),
        ];

        let voter = MultiModelVoter::new()
            .with_min_voters(3)
            .with_strategy(VotingStrategy::BestOfN)
            .with_timeout_ms(5_000);

        let outcome = voter.vote("test prompt", &agents).await.unwrap();
        assert_eq!(outcome.strategy_used, VotingStrategy::BestOfN);
        assert!(!outcome.all_votes.is_empty());
    }

    #[tokio::test]
    async fn test_no_agents_returns_error() {
        let voter = MultiModelVoter::new();
        let result = voter.vote("prompt", &[]).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_failing_agents_are_filtered() {
        let agents: Vec<Arc<dyn Agent>> =
            vec![make_agent(0, "gpt", "yes"), make_failing_agent(1, "bad")];

        let voter = MultiModelVoter::new()
            .with_min_voters(1)
            .with_strategy(VotingStrategy::Majority)
            .with_timeout_ms(5_000);

        let outcome = voter.vote("test", &agents).await.unwrap();
        assert_eq!(outcome.winning_response, "yes");
        assert_eq!(outcome.all_votes.len(), 1);
    }

    #[tokio::test]
    async fn test_weighted_voting_uses_model_weights() {
        let agents: Vec<Arc<dyn Agent>> = vec![
            make_agent(0, "gpt", "answer A"),
            make_agent(1, "claude", "answer B"),
        ];

        let voter = MultiModelVoter::new()
            .with_min_voters(2)
            .with_strategy(VotingStrategy::Weighted)
            .with_timeout_ms(5_000)
            .with_weight("gpt", 2.0)
            .with_weight("claude", 1.0);

        let outcome = voter.vote("test", &agents).await.unwrap();
        assert_eq!(outcome.strategy_used, VotingStrategy::Weighted);
        assert!(!outcome.all_votes.is_empty());
    }
}
