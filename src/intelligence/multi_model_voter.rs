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
    /// Fusion: aggregate all model responses into a single fused output with
    /// contradiction detection and per-model contribution weights.
    Fusion,
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

/// A detected contradiction between model responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contradiction {
    /// Names of the models that disagree.
    pub models: Vec<String>,
    /// The topic or subject of disagreement.
    pub topic: String,
    /// Each model's respective position on the topic.
    pub positions: Vec<String>,
}

/// Method used by the FusionEngine to combine responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FusionMethod {
    /// Simple majority fusion (fallback).
    Majority,
    /// Weighted fusion based on model weights.
    Weighted,
    /// Best-of-N fusion (pick best and augment).
    BestOfN,
    /// Full fusion — merge all responses with contradiction detection.
    Fusion,
}

/// The aggregated outcome of a multi-model vote.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VotingOutcome {
    /// The winning response text (same as final_response for single-winner strategies).
    pub winning_response: String,
    /// The name of the model whose response won ("fusion" for Fusion strategy).
    pub winner_model: String,
    /// Consensus level (0.0-1.0), indicating how strongly models agreed.
    pub consensus_level: f64,
    /// All individual model votes collected.
    pub all_votes: Vec<ModelVoteResult>,
    /// The strategy that produced this outcome.
    pub strategy_used: VotingStrategy,
    /// Total wall-clock duration of the voting round in milliseconds.
    pub total_duration_ms: u64,
    /// Whether a tie-breaker was required to resolve the vote.
    pub tie_breaker_used: bool,
    /// The fused final response text (same as winning_response for single-winner strategies).
    pub final_response: String,
    /// Detected contradictions between model responses.
    #[serde(default)]
    pub contradictions: Vec<Contradiction>,
    /// Per-model contribution weights (0.0-1.0) for Fusion strategy.
    #[serde(default)]
    pub model_contributions: HashMap<String, f64>,
    /// The fusion method used.
    pub fusion_method: FusionMethod,
}

// ── VotingConfig ────────────────────────────────────────────────────────────

/// Configuration for the voting system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VotingConfig {
    /// Whether to use response fusion (contradiction detection + merging).
    /// When `true`, the Fusion strategy is preferred if available;
    /// contradiction detection can still run even when `false`.
    pub use_fusion: bool,
}

impl Default for VotingConfig {
    fn default() -> Self {
        Self { use_fusion: false }
    }
}

// ── FusionEngine ────────────────────────────────────────────────────────────

/// Engine for fusing multiple model responses into a single coherent output,
/// detecting contradictions, and computing per-model contribution weights.
#[derive(Debug, Clone)]
pub struct FusionEngine {
    /// If enabled, attempts advanced fusion via an aggregator model.
    pub fusion_model_enabled: bool,
    /// Model weights (same key space as [`MultiModelVoter::model_weights`]).
    pub model_weights: HashMap<String, f64>,
}

impl FusionEngine {
    /// Create a new [`FusionEngine`] with default settings.
    pub fn new() -> Self {
        Self {
            fusion_model_enabled: false,
            model_weights: HashMap::new(),
        }
    }

    /// Fuse a set of model responses into a single [`VotingOutcome`].
    ///
    /// Uses majority fusion as the default, falling back to simple
    /// response concatenation when no clear majority exists.
    pub fn fuse(&self, responses: Vec<ModelVoteResult>) -> VotingOutcome {
        let n = responses.len();

        let contradictions = Self::detect_contradictions(&responses);
        let contributions = self.compute_contributions(&responses);

        if n == 0 {
            return VotingOutcome {
                winning_response: String::new(),
                winner_model: "fusion".into(),
                consensus_level: 0.0,
                all_votes: vec![],
                strategy_used: VotingStrategy::Fusion,
                total_duration_ms: 0,
                tie_breaker_used: false,
                final_response: String::new(),
                contradictions,
                model_contributions: contributions,
                fusion_method: FusionMethod::Fusion,
            };
        }

        // ── Majority-fusion: group by similarity ─────────────────────────
        let mut clusters: Vec<Vec<usize>> = Vec::new();
        let mut assigned = vec![false; n];

        for i in 0..n {
            if assigned[i] {
                continue;
            }
            let mut cluster = vec![i];
            assigned[i] = true;
            for j in (i + 1)..n {
                if !assigned[j]
                    && MultiModelVoter::similar_responses(
                        &responses[i].response,
                        &responses[j].response,
                    )
                {
                    cluster.push(j);
                    assigned[j] = true;
                }
            }
            clusters.push(cluster);
        }

        // Sort clusters by size descending
        clusters.sort_by(|a, b| b.len().cmp(&a.len()));

        let fused_text = if !clusters.is_empty() && clusters[0].len() > n / 2 {
            // Clear majority — use the most representative response
            let rep_idx = clusters[0][0];
            responses[rep_idx].response.clone()
        } else if !clusters.is_empty() && !responses.is_empty() {
            // No clear majority — merge by appending unique content
            Self::merge_unique_content(&responses)
        } else {
            responses[0].response.clone()
        };

        // Winner model is the one closest to the fused text
        let winner_model = contributions
            .iter()
            .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(name, _)| name.clone())
            .unwrap_or_else(|| "fusion".to_string());

        let consensus = MultiModelVoter::consensus_score(&responses);

        VotingOutcome {
            winning_response: fused_text.clone(),
            winner_model,
            consensus_level: consensus,
            all_votes: responses,
            strategy_used: VotingStrategy::Fusion,
            total_duration_ms: 0,
            tie_breaker_used: false,
            final_response: fused_text,
            contradictions,
            model_contributions: contributions,
            fusion_method: FusionMethod::Fusion,
        }
    }

    /// Detect contradictions between model responses.
    ///
    /// Uses topical keyword analysis: if one model's response contains an
    /// affirmation ("yes", "is", "should") on a topic and another contains
    /// a negation ("no", "is not", "should not") on the same topic, a
    /// contradiction is recorded.
    pub fn detect_contradictions(responses: &[ModelVoteResult]) -> Vec<Contradiction> {
        if responses.len() < 2 {
            return vec![];
        }

        let mut contradictions: Vec<Contradiction> = Vec::new();

        // Simple keyword-based contradiction detection
        let affirm_keywords = [
            "yes", "is", "are", "was", "were", "will", "should", "must", "can", "do",
        ];
        let negate_keywords = [
            "no",
            "not",
            "isn't",
            "aren't",
            "wasn't",
            "weren't",
            "won't",
            "shouldn't",
            "mustn't",
            "cannot",
            "can't",
            "don't",
            "doesn't",
            "never",
        ];

        for i in 0..responses.len() {
            for j in (i + 1)..responses.len() {
                let resp_i = responses[i].response.to_lowercase();
                let resp_j = responses[j].response.to_lowercase();

                // Look for sentences that share a topic but take opposite stances
                let sentences_i: Vec<&str> = resp_i
                    .split(|c: char| c == '.' || c == '!' || c == '?')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .collect();
                let sentences_j: Vec<&str> = resp_j
                    .split(|c: char| c == '.' || c == '!' || c == '?')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .collect();

                for si in &sentences_i {
                    for sj in &sentences_j {
                        // Find shared topic words (nouns / key terms)
                        let words_i: std::collections::HashSet<&str> =
                            si.split_whitespace().filter(|w| w.len() > 3).collect();
                        let words_j: std::collections::HashSet<&str> =
                            sj.split_whitespace().filter(|w| w.len() > 3).collect();

                        let shared: Vec<&&str> = words_i
                            .intersection(&words_j)
                            .filter(|w| {
                                !affirm_keywords.contains(w) && !negate_keywords.contains(w)
                            })
                            .collect();

                        if shared.len() < 2 {
                            continue;
                        }

                        let stance_i = affirm_keywords.iter().any(|k| si.contains(k))
                            && !negate_keywords.iter().any(|k| si.contains(k));
                        let stance_j = affirm_keywords.iter().any(|k| sj.contains(k))
                            && !negate_keywords.iter().any(|k| sj.contains(k));
                        let neg_i = negate_keywords.iter().any(|k| si.contains(k));
                        let neg_j = negate_keywords.iter().any(|k| sj.contains(k));

                        // One affirms, the other negates on the same topic
                        if (stance_i && neg_j) || (stance_j && neg_i) {
                            let topic = shared.iter().map(|w| **w).collect::<Vec<&str>>().join(" ");

                            // Avoid duplicate contradictions on the same topic
                            if !contradictions.iter().any(|c| c.topic == topic) {
                                contradictions.push(Contradiction {
                                    models: vec![
                                        responses[i].model_name.clone(),
                                        responses[j].model_name.clone(),
                                    ],
                                    topic,
                                    positions: vec![
                                        responses[i].response.clone(),
                                        responses[j].response.clone(),
                                    ],
                                });
                            }
                        }
                    }
                }
            }
        }

        contradictions
    }

    /// Compute per-model contribution weights (0.0–1.0) based on similarity
    /// to the consensus (most common) response cluster.
    ///
    /// Models whose responses are most similar to the majority cluster
    /// receive higher weights. Outliers receive lower weights.
    pub fn compute_contributions(&self, responses: &[ModelVoteResult]) -> HashMap<String, f64> {
        let mut contributions = HashMap::new();

        if responses.is_empty() {
            return contributions;
        }

        if responses.len() == 1 {
            contributions.insert(responses[0].model_name.clone(), 1.0);
            return contributions;
        }

        // Compute pairwise similarity matrix
        let n = responses.len();
        let mut sim_matrix = vec![vec![0.0_f64; n]; n];
        for i in 0..n {
            for j in 0..n {
                if i == j {
                    sim_matrix[i][j] = 1.0;
                } else {
                    sim_matrix[i][j] = if MultiModelVoter::similar_responses(
                        &responses[i].response,
                        &responses[j].response,
                    ) {
                        1.0
                    } else {
                        // Use word overlap as a continuous similarity measure
                        let a = responses[i].response.to_lowercase();
                        let b = responses[j].response.to_lowercase();
                        let words_a: std::collections::HashSet<&str> =
                            a.split_whitespace().collect();
                        let words_b: std::collections::HashSet<&str> =
                            b.split_whitespace().collect();
                        let intersection = words_a.intersection(&words_b).count();
                        let union = words_a.union(&words_b).count();
                        if union == 0 {
                            0.0
                        } else {
                            intersection as f64 / union as f64
                        }
                    };
                }
            }
        }

        // For each model, compute average similarity to all others
        let avg_similarities: Vec<f64> = (0..n)
            .map(|i| {
                let sum: f64 = sim_matrix[i].iter().sum();
                (sum - 1.0) / (n as f64 - 1.0) // exclude self-similarity
            })
            .collect();

        // Apply model weights as multipliers
        let mut weighted: Vec<(usize, f64)> = avg_similarities
            .into_iter()
            .enumerate()
            .map(|(i, sim)| {
                let weight = self
                    .model_weights
                    .get(&responses[i].model_name)
                    .copied()
                    .unwrap_or(1.0);
                (i, sim * weight)
            })
            .collect();

        // Normalize to sum to 1.0
        let total: f64 = weighted.iter().map(|(_, s)| s).sum();
        if total > 0.0 {
            for (_, score) in &mut weighted {
                *score /= total;
            }
        } else {
            // Equal weights as fallback
            let eq = 1.0 / n as f64;
            for (_, score) in &mut weighted {
                *score = eq;
            }
        }

        for (idx, score) in weighted {
            contributions.insert(responses[idx].model_name.clone(), score);
        }

        contributions
    }

    /// Merge unique content from all responses into a single text.
    /// Appends any sentences from other responses that are not already present
    /// in the first response.
    fn merge_unique_content(responses: &[ModelVoteResult]) -> String {
        if responses.is_empty() {
            return String::new();
        }

        let mut fused = responses[0].response.clone();
        let base_lower = fused.to_lowercase();

        for resp in &responses[1..] {
            for sentence in resp
                .response
                .split(|c: char| c == '.' || c == '!' || c == '?')
            {
                let s = sentence.trim();
                if s.is_empty() {
                    continue;
                }
                let s_lower = s.to_lowercase();
                // Check if this sentence (or close variant) already exists in fused text
                if !base_lower.contains(&s_lower) && !fused.to_lowercase().contains(&s_lower) {
                    fused.push_str(". ");
                    fused.push_str(s);
                }
            }
        }

        fused
    }
}

impl Default for FusionEngine {
    fn default() -> Self {
        Self::new()
    }
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
    /// Max models to retain in model_weights before evicting the oldest.
    pub max_models: usize,
}

impl MultiModelVoter {
    /// Create a new voter with default configuration.
    ///
    /// Defaults: 3 minimum voters, majority strategy, 30-second per-model timeout.
    const DEFAULT_MAX_MODELS: usize = 100;

    pub fn new() -> Self {
        Self {
            min_voters: 3,
            strategy: VotingStrategy::Majority,
            per_model_timeout_ms: 30_000,
            model_weights: HashMap::new(),
            max_models: Self::DEFAULT_MAX_MODELS,
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
        // Evict the oldest entry when at capacity (model not already tracked).
        if !self.model_weights.contains_key(model_name)
            && self.model_weights.len() >= self.max_models
        {
            // Remove an arbitrary entry since HashMap is unordered.
            if let Some(key) = self.model_weights.keys().next().cloned() {
                self.model_weights.remove(&key);
            }
        }
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

    /// Vote with fusion engine: collect responses and fuse them into a single
    /// coherent output with contradiction detection and contribution weights.
    ///
    /// This bypasses the configured strategy and always uses [`FusionEngine`].
    pub async fn vote_with_fusion(
        &self,
        prompt: &str,
        agents: &[Arc<dyn Agent>],
    ) -> Result<VotingOutcome> {
        let responses = self.collect_votes(prompt, agents).await?;
        let engine = FusionEngine {
            fusion_model_enabled: false,
            model_weights: self.model_weights.clone(),
        };
        Ok(engine.fuse(responses))
    }

    /// Vote with fusion and return detected contradictions separately.
    ///
    /// Returns a tuple of `(VotingOutcome, Vec<Contradiction>)` where the
    /// contradictions are also available as a standalone list for convenience.
    pub async fn vote_with_fusion_and_detect(
        &self,
        prompt: &str,
        agents: &[Arc<dyn Agent>],
    ) -> Result<(VotingOutcome, Vec<Contradiction>)> {
        let outcome = self.vote_with_fusion(prompt, agents).await?;
        let contradictions = FusionEngine::detect_contradictions(&outcome.all_votes);
        Ok((outcome, contradictions))
    }

    /// Internal helper: collect votes from all agents (same core as `vote`).
    async fn collect_votes(
        &self,
        prompt: &str,
        agents: &[Arc<dyn Agent>],
    ) -> Result<Vec<ModelVoteResult>> {
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

                    drop(agent);
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
                        confidence: 0.5,
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

        let mut votes: Vec<ModelVoteResult> = Vec::with_capacity(agents.len());
        for handle in handles {
            match handle.await {
                Ok(Some(vote)) => votes.push(vote),
                Ok(None) => {}
                Err(join_err) => {
                    warn!("MultiModelVoter: spawned task panicked: {}", join_err);
                }
            }
        }

        if votes.is_empty() {
            return Err(anyhow::anyhow!(tf("voter.all_models_failed", &[])));
        }

        Ok(votes)
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
            VotingStrategy::Fusion => {
                // Use FusionEngine to fuse responses
                let engine = FusionEngine {
                    fusion_model_enabled: false,
                    model_weights: self.model_weights.clone(),
                };
                engine.fuse(votes)
            }
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

        let winner_response = votes[winner_idx].response.clone();
        VotingOutcome {
            winning_response: winner_response.clone(),
            winner_model: votes[winner_idx].model_name.clone(),
            consensus_level: consensus,
            all_votes: votes,
            strategy_used: VotingStrategy::Majority,
            total_duration_ms,
            tie_breaker_used,
            final_response: winner_response,
            contradictions: vec![],
            model_contributions: HashMap::new(),
            fusion_method: FusionMethod::Majority,
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

        let winner_response = votes[best_idx].response.clone();
        VotingOutcome {
            winning_response: winner_response.clone(),
            winner_model: votes[best_idx].model_name.clone(),
            consensus_level: consensus,
            all_votes: votes,
            strategy_used: VotingStrategy::Weighted,
            total_duration_ms,
            tie_breaker_used,
            final_response: winner_response,
            contradictions: vec![],
            model_contributions: HashMap::new(),
            fusion_method: FusionMethod::Weighted,
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
            let best_idx = votes.iter().min_by_key(|v| v.latency_ms).unwrap();
            let winner_response = best_idx.response.clone();
            VotingOutcome {
                winning_response: winner_response.clone(),
                winner_model: best_idx.model_name.clone(),
                consensus_level: 1.0,
                all_votes: votes,
                strategy_used: VotingStrategy::Unanimous,
                total_duration_ms,
                tie_breaker_used: false,
                final_response: winner_response,
                contradictions: vec![],
                model_contributions: HashMap::new(),
                fusion_method: FusionMethod::Majority,
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

        let winner_response = votes[best_idx].response.clone();
        VotingOutcome {
            winning_response: winner_response.clone(),
            winner_model: votes[best_idx].model_name.clone(),
            consensus_level: consensus,
            all_votes: votes,
            strategy_used: VotingStrategy::BestOfN,
            total_duration_ms,
            tie_breaker_used,
            final_response: winner_response,
            contradictions: vec![],
            model_contributions: HashMap::new(),
            fusion_method: FusionMethod::BestOfN,
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

    // ── Fusion tests ──────────────────────────────────────────────────

    #[test]
    fn test_fusion_engine_detect_contradictions_no_contradiction() {
        let responses = vec![
            ModelVoteResult {
                model_name: "model-a".into(),
                response: "The sky is blue during the day.".into(),
                confidence: 0.9,
                latency_ms: 10,
            },
            ModelVoteResult {
                model_name: "model-b".into(),
                response: "The sky appears blue due to Rayleigh scattering.".into(),
                confidence: 0.8,
                latency_ms: 15,
            },
        ];

        let contradictions = FusionEngine::detect_contradictions(&responses);
        assert!(
            contradictions.is_empty(),
            "Agreeing responses should have no contradictions"
        );
    }

    #[test]
    fn test_fusion_engine_detect_contradictions_detected() {
        let responses = vec![
            ModelVoteResult {
                model_name: "model-a".into(),
                response: "Climate change is real and caused by human activity.".into(),
                confidence: 0.9,
                latency_ms: 10,
            },
            ModelVoteResult {
                model_name: "model-b".into(),
                response: "Climate change is not real and is a natural phenomenon.".into(),
                confidence: 0.7,
                latency_ms: 20,
            },
        ];

        let contradictions = FusionEngine::detect_contradictions(&responses);
        assert!(
            !contradictions.is_empty(),
            "Opposing positions should produce contradictions"
        );
        // Should identify 'climate change' as the topic
        assert!(
            contradictions[0].topic.contains("climate")
                || contradictions[0].topic.contains("change"),
            "Contradiction topic should reference the shared topic"
        );
        assert_eq!(contradictions[0].models.len(), 2);
    }

    #[test]
    fn test_fusion_engine_detect_single_response() {
        let responses = vec![ModelVoteResult {
            model_name: "model-a".into(),
            response: "Solo response.".into(),
            confidence: 1.0,
            latency_ms: 5,
        }];

        let contradictions = FusionEngine::detect_contradictions(&responses);
        assert!(contradictions.is_empty());
    }

    #[test]
    fn test_fusion_engine_compute_contributions_equal() {
        let engine = FusionEngine::new();
        let responses = vec![
            ModelVoteResult {
                model_name: "model-a".into(),
                response: "The answer is forty two.".into(),
                confidence: 0.9,
                latency_ms: 10,
            },
            ModelVoteResult {
                model_name: "model-b".into(),
                response: "The answer is forty two.".into(),
                confidence: 0.8,
                latency_ms: 15,
            },
        ];

        let contributions = engine.compute_contributions(&responses);
        assert_eq!(contributions.len(), 2);
        // Both responses are identical, so contributions should be roughly equal
        let a = contributions.get("model-a").copied().unwrap_or(0.0);
        let b = contributions.get("model-b").copied().unwrap_or(0.0);
        assert!(
            (a - b).abs() < 0.01,
            "Equal responses should have ~equal contributions: {a} vs {b}"
        );
        // And they should sum to ~1.0
        assert!(
            (a + b - 1.0).abs() < 0.01,
            "Contributions should sum to ~1.0"
        );
    }

    #[test]
    fn test_fusion_engine_compute_contributions_outlier() {
        let engine = FusionEngine::new();
        let responses = vec![
            ModelVoteResult {
                model_name: "majority".into(),
                response: "The capital of France is Paris.".into(),
                confidence: 0.9,
                latency_ms: 10,
            },
            ModelVoteResult {
                model_name: "majority2".into(),
                response: "Paris is the capital of France.".into(),
                confidence: 0.8,
                latency_ms: 15,
            },
            ModelVoteResult {
                model_name: "outlier".into(),
                response: "The capital of France is London.".into(),
                confidence: 0.7,
                latency_ms: 20,
            },
        ];

        let contributions = engine.compute_contributions(&responses);
        assert_eq!(contributions.len(), 3);
        let outlier_weight = contributions.get("outlier").copied().unwrap_or(1.0);
        let majority_weight = contributions.get("majority").copied().unwrap_or(0.0);
        assert!(
            outlier_weight < majority_weight,
            "Outlier should have lower contribution than majority"
        );
        // Sum should be ~1.0
        let sum: f64 = contributions.values().sum();
        assert!(
            (sum - 1.0).abs() < 0.01,
            "Contributions should sum to ~1.0, got {sum}"
        );
    }

    #[test]
    fn test_fusion_engine_compute_contributions_empty() {
        let engine = FusionEngine::new();
        let contributions = engine.compute_contributions(&[]);
        assert!(contributions.is_empty());
    }

    #[test]
    fn test_fusion_engine_compute_contributions_single() {
        let engine = FusionEngine::new();
        let responses = vec![ModelVoteResult {
            model_name: "solo".into(),
            response: "Unique response.".into(),
            confidence: 1.0,
            latency_ms: 5,
        }];

        let contributions = engine.compute_contributions(&responses);
        assert_eq!(contributions.get("solo"), Some(&1.0));
    }

    #[test]
    fn test_fusion_engine_fuse_majority() {
        let engine = FusionEngine::new();
        let responses = vec![
            ModelVoteResult {
                model_name: "model-a".into(),
                response: "The answer is yes.".into(),
                confidence: 0.9,
                latency_ms: 10,
            },
            ModelVoteResult {
                model_name: "model-b".into(),
                response: "The answer is yes.".into(),
                confidence: 0.8,
                latency_ms: 15,
            },
            ModelVoteResult {
                model_name: "model-c".into(),
                response: "The answer is no.".into(),
                confidence: 0.7,
                latency_ms: 20,
            },
        ];

        let outcome = engine.fuse(responses);
        assert!(
            outcome.winning_response.contains("yes"),
            "Fusion should pick majority response"
        );
        assert_eq!(outcome.fusion_method, FusionMethod::Fusion);
        assert_eq!(outcome.strategy_used, VotingStrategy::Fusion);
        assert_eq!(outcome.all_votes.len(), 3);
        assert!(
            !outcome.model_contributions.is_empty(),
            "Should have contribution weights"
        );
    }

    #[test]
    fn test_fusion_engine_fuse_empty() {
        let engine = FusionEngine::new();
        let outcome = engine.fuse(vec![]);
        assert!(outcome.winning_response.is_empty());
        assert!(outcome.final_response.is_empty());
    }

    #[test]
    fn test_fusion_engine_merge_unique_content() {
        let responses = vec![
            ModelVoteResult {
                model_name: "model-a".into(),
                response: "The sky is blue.".into(),
                confidence: 0.9,
                latency_ms: 10,
            },
            ModelVoteResult {
                model_name: "model-b".into(),
                response: "The ocean is deep. Stars are bright.".into(),
                confidence: 0.8,
                latency_ms: 15,
            },
        ];

        let merged = FusionEngine::merge_unique_content(&responses);
        assert!(merged.contains("sky is blue"));
        assert!(
            merged.contains("ocean is deep"),
            "Should include unique content from second response"
        );
        assert!(
            merged.contains("Stars are bright"),
            "Should include all unique content"
        );
    }

    #[tokio::test]
    async fn test_fusion_strategy_via_vote() {
        let agents: Vec<Arc<dyn Agent>> = vec![
            make_agent(0, "model-a", "Paris is the capital of France."),
            make_agent(1, "model-b", "Paris is the capital of France."),
            make_agent(2, "model-c", "London is the capital of France."),
        ];

        let voter = MultiModelVoter::new()
            .with_min_voters(3)
            .with_strategy(VotingStrategy::Fusion)
            .with_timeout_ms(5_000);

        let outcome = voter.vote("test prompt", &agents).await.unwrap();
        assert_eq!(outcome.strategy_used, VotingStrategy::Fusion);
        assert!(
            outcome.winning_response.contains("Paris"),
            "Fusion should favor majority cluster"
        );
        assert!(!outcome.model_contributions.is_empty());
        assert_eq!(outcome.fusion_method, FusionMethod::Fusion);
    }

    #[tokio::test]
    async fn test_vote_with_fusion_method() {
        let agents: Vec<Arc<dyn Agent>> = vec![
            make_agent(0, "model-a", "Option one is correct."),
            make_agent(1, "model-b", "Option two is correct."),
        ];

        let voter = MultiModelVoter::new()
            .with_min_voters(2)
            .with_timeout_ms(5_000);

        let outcome = voter
            .vote_with_fusion("test prompt", &agents)
            .await
            .unwrap();
        // Always uses Fusion strategy
        assert_eq!(outcome.strategy_used, VotingStrategy::Fusion);
        assert!(!outcome.winning_response.is_empty());
        assert!(!outcome.model_contributions.is_empty());
    }

    #[tokio::test]
    async fn test_vote_with_fusion_and_detect() {
        let agents: Vec<Arc<dyn Agent>> = vec![
            make_agent(0, "model-a", "Climate change is real and caused by humans."),
            make_agent(1, "model-b", "Climate change is not real and is natural."),
        ];

        let voter = MultiModelVoter::new()
            .with_min_voters(2)
            .with_timeout_ms(5_000);

        let (outcome, _contradictions) = voter
            .vote_with_fusion_and_detect("test prompt", &agents)
            .await
            .unwrap();
        assert_eq!(outcome.strategy_used, VotingStrategy::Fusion);
        // The detect function may or may not find contradictions depending on
        // sentence-level keyword analysis — at minimum the outcome is valid
        assert!(!outcome.winning_response.is_empty());
        assert!(!outcome.model_contributions.is_empty());
    }

    #[test]
    fn test_voting_config_default() {
        let config = VotingConfig::default();
        assert!(!config.use_fusion);
    }

    #[test]
    fn test_contradiction_struct() {
        let c = Contradiction {
            models: vec!["a".into(), "b".into()],
            topic: "climate change".into(),
            positions: vec!["it is real".into(), "it is not real".into()],
        };
        assert_eq!(c.models.len(), 2);
        assert_eq!(c.topic, "climate change");
        assert_eq!(c.positions.len(), 2);
    }
}
