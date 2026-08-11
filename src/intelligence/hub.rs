//! BLUE48 — Intelligence Integration Hub
//!
//! Wires orphaned intelligence/governance modules into the hot execution path:
//! - Weighted reputation voting + Delphi debate → decision rationalization
//! - Rationalization → decision explanation in response assembly
//! - Audit → governance audit trail
//!
//! All integrations are non-blocking: failures in any module log a warning
//! but never crash the calling thread.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use std::collections::HashMap;

use std::sync::{Arc, OnceLock, RwLock};

use crate::config::AgentConfig;
use crate::governance::audit::AuditLogEntry;
use crate::governance::rationalization::SelfRationalizationGuard;
use crate::intelligence::capability_bus::core::CapabilityBus;
use crate::intelligence::voter_impls::{
    CapabilityBusVoter, DeepSeekVoter, LocalVoter, RationalizationGuardVoter,
};
use crate::intelligence::weighted_vote::{
    self, delphi_debate, AgentVoter, DelphiConfig, WeightedVoteConfig,
};

// ── Global counters for observability ─────────────────────────────────────

/// How many times the intelligence hub has been activated.
pub static INTEL_HUB_ACTIVATIONS: AtomicU64 = AtomicU64::new(0);
/// How many consensus rounds have been started.
pub static CONSENSUS_ROUNDS: AtomicU64 = AtomicU64::new(0);
/// How many rationalization evaluations were performed.
pub static RATIONALIZATION_COUNT: AtomicU64 = AtomicU64::new(0);

/// Whether Delphi-method debate voting is enabled in rationalize_decision.
/// Must match the `enable_delphi_debate` config default (false) so the hub
/// does not diverge from configuration before `init_intelligence_hub` runs.
static USE_DELPHI_DEBATE: AtomicBool = AtomicBool::new(false);

// ── Global instances ──────────────────────────────────────────────────────

/// Global singleton self-rationalization guard.
///
/// Contention is expected to be negligible because:
/// - It is accessed only from `rationalize_decision()`, once per decision.
/// - The critical section is a single `guard.evaluate()` call that completes
///   in microseconds.
/// - There is no hot-loop or high-frequency polling path through this guard.
///
/// A `std::sync::Mutex` is the correct primitive here: the critical section
/// is synchronous and brief.  `tokio::sync::Mutex` would add unnecessary
/// overhead, and `RwLock` would not improve throughput since there is only
/// one caller and no read-vs-write contention to exploit.
static GLOBAL_RATIONALIZATION: LazyLock<Mutex<SelfRationalizationGuard>> =
    LazyLock::new(|| Mutex::new(SelfRationalizationGuard::new(0.3)));

/// Global voters for the Delphi debate / weighted-vote system.
/// Initialised via [`init_intelligence_hub`] at server startup.
///
/// A `RwLock<Vec<Arc<dyn AgentVoter>>>` (instead of a `OnceLock`) so that
/// callers can snapshot the voter list without holding the lock across await
/// points, and so tests can deterministically replace the voter set (the
/// server-building unit tests register real voters; the hub tests must be
/// able to override them). `Arc` keeps the snapshot clone cheap.
static GLOBAL_VOTERS: RwLock<Vec<Arc<dyn AgentVoter + Send + Sync>>> = RwLock::new(Vec::new());

/// Global capability-bus reference for reputation lookups.
/// Set by [`init_intelligence_hub`] when a bus is available; used by
/// [`rationalize_decision`] to weight the Delphi vote with the real UKB
/// reputation of the agent under evaluation.
static GLOBAL_CAPABILITY_BUS: OnceLock<Arc<CapabilityBus>> = OnceLock::new();

/// Snapshot of all intelligence hub metric counters.
///
/// Used by the governance health endpoint to expose hub activity
/// (I5 — wire the dead-code counters into a read-side).
pub fn hub_metrics() -> serde_json::Value {
    serde_json::json!({
        "intel_hub_activations": INTEL_HUB_ACTIVATIONS.load(Ordering::Relaxed),
        "consensus_rounds": CONSENSUS_ROUNDS.load(Ordering::Relaxed),
        "rationalization_count": RATIONALIZATION_COUNT.load(Ordering::Relaxed),
        "audit_entry_count": crate::governance::audit::global_audit_log().len() as u64,
    })
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Initialize intelligence hub at server startup — single entry point.
///
/// Registers local nodes in the consensus engine and initialises the 4
/// AgentVoter impls (CapabilityBusVoter, LocalVoter,
/// RationalizationGuardVoter, DeepSeekVoter) for the Delphi debate /
/// weighted-vote system.
///
/// `enable_delphi_debate` — when `true`, `rationalize_decision` will
/// use the weighted reputation + Delphi debate voting path instead of
/// the basic rationalization guard.
///
/// `capability_bus` — when `Some`, registers a CapabilityBusVoter;
/// pass `None` to skip it.
///
/// This replaces the previous two-step `init_intel_hub()` +
/// `init_intel_voters()` pattern with a single call.
pub fn init_intelligence_hub(
    enable_delphi_debate: bool,
    capability_bus: Option<Arc<CapabilityBus>>,
) {
    // Phase 1: Store Delphi debate flag
    USE_DELPHI_DEBATE.store(enable_delphi_debate, Ordering::Relaxed);
    if enable_delphi_debate {
        tracing::info!("intel_hub: Delphi debate voting enabled");
    }
    tracing::info!("intel_hub: initialized rationalization, audit");

    // Phase 2: Register voters
    let mut voters: Vec<Box<dyn AgentVoter + Send + Sync>> = Vec::new();

    // Keep the bus for later reputation lookups in `rationalize_decision`.
    // Set it before the CapabilityBusVoter registration below consumes the
    // `Option<Arc<CapabilityBus>>` value.
    if let Some(bus) = capability_bus.as_ref() {
        let _ = GLOBAL_CAPABILITY_BUS.set(bus.clone());
    }

    if let Some(bus) = capability_bus {
        voters.push(Box::new(CapabilityBusVoter::new("capability-bus", bus)));
    }

    voters.push(Box::new(LocalVoter::new(
        "local-agent",
        AgentConfig::default(),
    )));

    voters.push(Box::new(RationalizationGuardVoter::new(
        "rationalization-guard",
        Arc::new(SelfRationalizationGuard::new(0.6)),
    )));

    let deepseek_api_key = std::env::var("DEEPSEEK_API_KEY").unwrap_or_default();
    if deepseek_api_key.trim().is_empty() {
        tracing::warn!(
            "intel_hub: DEEPSEEK_API_KEY not set — DeepSeekVoter not registered (delphi debates use local voters only)"
        );
    } else {
        voters.push(Box::new(DeepSeekVoter::new(
            "deepseek",
            "https://api.deepseek.com",
            "deepseek-v4-flash",
            deepseek_api_key,
        )));
    }

    *GLOBAL_VOTERS
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
        voters.into_iter().map(Arc::from).collect();

    tracing::info!("intel_hub: {} voter(s) registered", {
        GLOBAL_VOTERS
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    });
}

// ── Voting mode configuration ─────────────────────────────────────────────

/// Voting mode for the intelligence hub.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum VoteMode {
    /// Legacy consensus engine (existing simple majority).
    Legacy,
    /// Weighted reputation voting — each vote is weighted by agent reputation.
    Weighted,
    /// Delphi-method debate rounds with weighted reputation voting.
    #[default]
    DelphiDebate,
}

/// Configuration for the upgraded voting system.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VoteConfig {
    /// Which voting mode to use.
    pub mode: VoteMode,
    /// Configuration for weighted voting (used in Weighted and DelphiDebate modes).
    pub weighted: WeightedVoteConfig,
    /// Configuration for Delphi debate (used in DelphiDebate mode).
    pub delphi: DelphiConfig,
}

impl Default for VoteConfig {
    fn default() -> Self {
        Self {
            mode: VoteMode::DelphiDebate,
            weighted: WeightedVoteConfig::default(),
            delphi: DelphiConfig::default(),
        }
    }
}

/// Run multi-agent consensus voting on a decision proposal.
///
/// Registers 3 nodes with different weights and collects REAL votes:
/// - "capability-bus": weight=2, votes based on proposal confidence
/// - "local-agent": weight=1, votes approve (default)
/// - "rationalization-guard": weight=1, votes reject if confidence < 0.4
///
/// Returns the REAL consensus verdict (approve/reject) and confidence.
/// Run multi-agent consensus voting with **weighted reputation** and optional
/// **Delphi-method debate rounds**.
///
/// Instead of simple majority via hardcoded weights, it:
///
/// 1. Collects votes from the 3 internal nodes (capability-bus, local-agent,
///    rationalization-guard).
/// 2. **Weighted mode**: each vote is weighted by the agent's reputation score
///    from the reputation store (passed via `reputations`).
/// 3. **DelphiDebate mode**: runs up to `config.delphi.max_rounds` debate rounds
///    where agents see each other's reasoning before the final weighted vote.
///
/// # Arguments
///
/// * `proposal_id` – Unique proposal identifier.
/// * `proposal` – JSON proposal containing `confidence` and `risk_level`.
/// * `approve` – Default approval intent from the caller.
/// * `reputations` – Map from agent name to reputation score (0.0–1.0).
///   Pass an empty map to use default weights.
/// * `config` – [`VoteConfig`] controlling mode, threshold, debate rounds.
///
/// Returns `(approved, confidence)`.
pub async fn consensus_vote_with_reputation(
    proposal_id: &str,
    proposal: serde_json::Value,
    approve: bool,
    reputations: &HashMap<String, f64>,
    config: &VoteConfig,
) -> (bool, f64) {
    // Collect votes from the registered AgentVoter impls. This is truly
    // async-safe: the voter futures are awaited directly instead of blocking
    // the current thread. (The former hardcoded fallback for an empty voter
    // list was removed: `init_intelligence_hub` always registers voters at
    // server startup, so that branch was dead code in production and tests.)
    // The hub is now actively engaged in a consensus round.
    INTEL_HUB_ACTIVATIONS.fetch_add(1, Ordering::Relaxed);

    let raw_votes = {
        // Snapshot the voter list so no lock is held across the await points
        // below (a std RwLock guard is not Send).
        let voters: Vec<Arc<dyn AgentVoter + Send + Sync>> = GLOBAL_VOTERS
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        // Build the voting context from the proposal
        let context = serde_json::to_string(&proposal).unwrap_or_default();

        let mut votes = HashMap::new();
        // Spawn all voters concurrently and await them directly.
        let voter_futures: Vec<_> = voters
            .iter()
            .cloned()
            .map(|voter| {
                let context = context.clone();
                tokio::spawn(async move {
                    let name = voter.name().to_string();
                    let vote = voter.vote(&context).await;
                    (name, vote)
                })
            })
            .collect();
        let results = futures_util::future::join_all(voter_futures).await;
        for result in results {
            match result {
                Ok((name, vote)) => {
                    votes.insert(name, vote);
                }
                Err(e) => {
                    tracing::warn!("intel_hub: voter task failed: {}", e);
                }
            }
        }
        votes
    };

    // Compute final result based on mode
    let cb_approve = raw_votes
        .get("capability-bus")
        .map(|v| v.approves)
        .unwrap_or(approve);
    let rg_approve = raw_votes
        .get("rationalization-guard")
        .map(|v| v.approves)
        .unwrap_or(approve);
    let final_result = match config.mode {
        VoteMode::DelphiDebate => {
            let debate_context = format!(
                "capability-bus: {}\nlocal-agent: {}\nrationalization-guard: {}",
                if cb_approve { "APPROVE" } else { "REJECT" },
                if approve { "APPROVE" } else { "REJECT" },
                if rg_approve { "APPROVE" } else { "REJECT" },
            );
            tracing::info!(debate_context, "delphi debate context");

            // Use the actual multi-round Delphi debate when global voters are available.
            // Snapshot the voter list (no lock held across the await below).
            let debate_voters: Vec<Arc<dyn AgentVoter + Send + Sync>> = GLOBAL_VOTERS
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            if !debate_voters.is_empty() {
                let agent_refs: Vec<&dyn AgentVoter> = debate_voters
                    .iter()
                    .map(|b| b.as_ref() as &dyn AgentVoter)
                    .collect();
                let debate_question = debate_context.clone();
                let delphi_config = config.delphi.clone();
                // Seed round 0 with the votes already collected above so the
                // voters (including remote LLM voters) are not invoked twice.
                let result = delphi_debate(
                    &agent_refs,
                    &debate_question,
                    reputations,
                    &delphi_config,
                    Some(raw_votes.clone()),
                )
                .await;
                tracing::info!(
                    "delphi_debate: {} rounds, converged={}, approved={}",
                    result.rounds,
                    result.converged,
                    result.final_result.approved
                );
                CONSENSUS_ROUNDS.fetch_add(result.rounds as u64, Ordering::Relaxed);
                result.final_result
            } else {
                // No voters — fall back to simple weighted vote.
                weighted_vote::weighted_vote(
                    &raw_votes,
                    reputations,
                    config.delphi.threshold,
                    config.delphi.default_weight,
                )
            }
        }
        VoteMode::Weighted => weighted_vote::weighted_vote(
            &raw_votes,
            reputations,
            config.weighted.threshold,
            config.weighted.default_weight,
        ),
        // Legacy consensus engine: real simple majority. Each registered
        // voter casts one unweighted vote; approval requires strictly more
        // than half of the votes (no reputation weighting, no threshold).
        VoteMode::Legacy => {
            let participant_count = raw_votes.len();
            let approvals = raw_votes.values().filter(|v| v.approves).count();
            let approved = participant_count > 0 && approvals * 2 > participant_count;
            weighted_vote::VoteResult {
                approved,
                approval_ratio: if participant_count > 0 {
                    approvals as f64 / participant_count as f64
                } else {
                    0.0
                },
                total_weight: participant_count as f64,
                weighted_yes: approvals as f64,
                weighted: false,
                participant_count,
            }
        }
    };

    let final_approve = final_result.approved;
    let approval_ratio = final_result.approval_ratio;
    let confidence = if final_approve {
        0.5 + approval_ratio * 0.4
    } else {
        0.5 - (0.5 - approval_ratio) * 0.4
    };
    let confidence = confidence.clamp(0.1, 0.95);

    // Record audit entry for the consensus vote outcome
    record_audit_entry(
        AuditEntryBuilder::new(
            proposal_id,
            "consensus_vote",
            if final_approve { "allow" } else { "deny" },
        )
        .inputs(proposal.clone())
        .confidence(confidence as f32)
        .build(),
    );

    (final_approve, confidence)
}

/// Evaluate a decision using the rationalization guard with multi-factor risk analysis.
///
/// Multi-factor risk scoring for agent decisions.
///
/// Analyzes:
/// - Task complexity (via token count, keywords)
/// - Agent reputation (via historical success rate, if available)
/// - Confidence level
/// - Risk keywords in task description
///
/// Returns (is_justified, explanation) where explanation describes concerns.
pub async fn rationalize_decision(agent: &str, task: &str, confidence: f64) -> (bool, String) {
    // Multi-factor risk scoring — single keyword table used by both the
    // Delphi-debate proposal and the rationalization threshold below (they
    // previously maintained two separate keyword sets that disagreed).
    //
    // RISK-KEYWORD CROSS-REFERENCE (F8): evaluated — there are six overlapping
    // keyword sources in the codebase and their semantics are mutually
    // incompatible (ratio / additive / boolean / weighted-max), so they are NOT
    // merged; forcing a merge would change behavior:
    //   1. this table (rationalization guard / Delphi-debate risk level) —
    //      ratio: matched-keyword count / table size, plain `contains`;
    //   2. `ModeRuntime::compute_risk_score` (src/orchestration/mode.rs) —
    //      additive scoring (0.10/0.20/0.30) with word-boundary matching;
    //   3. `TaskRouter::analyze_task().has_safety_concerns`
    //      (src/orchestration/task_router.rs) — single boolean flag;
    //   4. `extract_plan_from_response` (src/orchestration/plan_output.rs) —
    //      keyword→weight pairs combined with `max()`;
    //   5. voters (src/intelligence/voter_impls.rs) — per-voter boolean
    //      presence sets (CapabilityBusVoter security/performance,
    //      LocalVoter proposal/risk/positive);
    //   6. `AdversarialVerifier::verify` Security bias
    //      (src/intelligence/verification.rs) — per-finding anti-pattern
    //      detectors, not a scoring table.
    // Unified typed classifier is the consolidation direction, but evaluation
    // verdict: benefit medium, risk medium — the debt is kept. Until then, keep
    // the tables in sync when adding a risk keyword.
    let risk_keywords = [
        "delete", "remove", "exec", "shell", "rm", "sudo", "admin", "override", "bypass", "secret",
        "token", "password", "key", "cert", "database", "drop", "truncate", "alter", "grant",
        "revoke",
    ];
    let task_lower = task.to_lowercase();
    let risk_score = risk_keywords
        .iter()
        .filter(|kw| task_lower.contains(*kw))
        .count() as f64
        / risk_keywords.len() as f64;

    // ── Delphi debate integration ────────────────────────────────────────
    // When enabled, delegate to the weighted reputation + Delphi debate
    // voting path for higher-confidence decision verification.
    if USE_DELPHI_DEBATE.load(Ordering::Relaxed) {
        let proposal = serde_json::json!({
            "confidence": confidence,
            "risk_level": if risk_score > 0.0 { "high" } else { "low" },
        });
        // Reputation-weighted Delphi vote: build the voter-weight map from the
        // real UKB reputation of the agent under evaluation instead of passing
        // an empty map (an empty map degenerated the weighted vote to equal
        // weights). The capability-bus voter represents the evaluated agent's
        // ecosystem, so its vote is weighted by that agent's reputation; the
        // remaining voters (local / rationalization-guard / LLM) are
        // infrastructure and keep the default weight. When no bus is available
        // the map stays empty and the vote uses default weights (unchanged).
        let mut reputations = HashMap::new();
        if let Some(bus) = GLOBAL_CAPABILITY_BUS.get() {
            let ukb = bus.unified_knowledge_bus.read().unwrap_or_else(|poisoned| {
                tracing::warn!("unified_knowledge_bus lock poisoned – recovered");
                poisoned.into_inner()
            });
            let agent_reputation = ukb.get_reputation(agent).unwrap_or(0.5);
            drop(ukb);
            reputations.insert("capability-bus".to_string(), agent_reputation);
        }
        let config = VoteConfig::default(); // defaults to DelphiDebate mode
        let (approved, _confidence) = consensus_vote_with_reputation(
            agent,
            proposal,
            confidence >= 0.5,
            &reputations,
            &config,
        )
        .await;
        if !approved {
            let reason = "delphi_debate_rejected: weighted consensus vote did not approve";
            record_audit_entry(
                AuditEntryBuilder::new(agent, "rationalize", "deny")
                    .agent(agent)
                    .inputs(serde_json::json!({"task": task, "confidence": confidence}))
                    .error(reason)
                    .build(),
            );
            return (false, reason.to_string());
        }
        // Delphi approved — continue to standard rationalization checks
    }

    // Task complexity: longer tasks with more structure are more complex
    let word_count = task.split_whitespace().count().max(1) as f64;
    let complexity_score = (word_count / 200.0).min(1.0);

    // Combine factors: higher risk + higher complexity = higher threshold
    let dynamic_threshold = 0.3 + risk_score * 0.4 + complexity_score * 0.3;
    let adjusted_confidence = confidence * (1.0 - risk_score * 0.3);

    let mut guard = match GLOBAL_RATIONALIZATION.lock() {
        Ok(g) => g,
        Err(e) => {
            tracing::warn!("intel_hub: rationalization lock failed: {e}");
            record_audit_entry(
                AuditEntryBuilder::new(agent, "rationalize", "allow")
                    .agent(agent)
                    .inputs(serde_json::json!({"task": task, "confidence": confidence}))
                    .error("lock_failed")
                    .build(),
            );
            return (true, String::new());
        }
    };

    RATIONALIZATION_COUNT.fetch_add(1, Ordering::Relaxed);
    let mut annotation = crate::governance::rationalization::RationalizationAnnotation {
        assumptions: vec![
            format!("agent_{}_handles_{}", agent, task),
            format!(
                "risk_score={:.2},complexity={:.2},threshold={:.2}",
                risk_score, complexity_score, dynamic_threshold
            ),
        ],
        evidence_refs: vec![],
        weak_evidence_flags: vec![],
        reexamine_triggered: false,
    };

    // adjusted_confidence is a real confidence score (complement-adjusted for
    // risk), so it is passed as-is; the removed trailing `false` was the dead
    // `is_full_auto` parameter of the old evaluate() signature.
    let blocked = guard.evaluate(&mut annotation, adjusted_confidence as f32);

    if blocked || adjusted_confidence < dynamic_threshold {
        let reasons = vec![
            if blocked {
                Some("rationalization_guard_blocked".to_string())
            } else {
                None
            },
            if adjusted_confidence < dynamic_threshold {
                Some(format!(
                    "low_confidence: {:.2} < {:.2}",
                    adjusted_confidence, dynamic_threshold
                ))
            } else {
                None
            },
            if risk_score > 0.3 {
                Some(format!("high_risk_task: score={:.2}", risk_score))
            } else {
                None
            },
        ];
        let reason = reasons
            .into_iter()
            .flatten()
            .next()
            .or_else(|| annotation.weak_evidence_flags.first().cloned())
            .unwrap_or_else(|| "multi_factor_rejection".to_string());
        record_audit_entry(
            AuditEntryBuilder::new(agent, "rationalize", "deny")
                .agent(agent)
                .inputs(serde_json::json!({"task": task, "confidence": confidence, "risk_score": risk_score, "adjusted_confidence": adjusted_confidence}))
                .error(&reason)
                .build()
        );
        (false, reason)
    } else {
        record_audit_entry(
            AuditEntryBuilder::new(agent, "rationalize", "allow")
                .agent(agent)
                .inputs(serde_json::json!({"task": task, "confidence": confidence, "risk_score": risk_score}))
                .build()
        );
        (true, String::new())
    }
}

/// Record an audit entry for the decision pipeline.
///
/// Wired into `rationalize_decision` and `consensus_vote_with_reputation`
/// at key decision points for governance audit trail completeness.
pub fn record_audit_entry(entry: AuditLogEntry) {
    // Single process-wide audit sink (governance::audit::global_audit_log).
    crate::governance::audit::global_audit_log().record(entry);
}

// ── AuditEntryBuilder ──────────────────────────────────────────────────────

/// Builder for [`AuditLogEntry`] that avoids long argument lists.
///
/// # Usage
///
/// ```text
/// use crate::intelligence::hub::AuditEntryBuilder;
///
/// let entry = AuditEntryBuilder::new("task-001", "chat", "allow")
///     .agent("agent-a")
///     .inputs(serde_json::json!({"input": "test"}))
///     .confidence(0.95)
///     .build();
/// ```
pub struct AuditEntryBuilder {
    task_id: String,
    phase: String,
    decision: String,
    agent: Option<String>,
    inputs: serde_json::Value,
    error: Option<String>,
    confidence: Option<f32>,
}

impl AuditEntryBuilder {
    /// Start building an audit entry with the minimum required fields.
    pub fn new(task_id: &str, phase: &str, decision: &str) -> Self {
        Self {
            task_id: task_id.to_string(),
            phase: phase.to_string(),
            decision: decision.to_string(),
            agent: None,
            inputs: serde_json::Value::Null,
            error: None,
            confidence: None,
        }
    }

    /// Set the agent name.
    pub fn agent(mut self, agent: &str) -> Self {
        self.agent = Some(agent.to_string());
        self
    }

    /// Set the input payload.
    pub fn inputs(mut self, inputs: serde_json::Value) -> Self {
        self.inputs = inputs;
        self
    }

    /// Set the error message.
    pub fn error(mut self, error: &str) -> Self {
        self.error = Some(error.to_string());
        self
    }

    /// Set the confidence score.
    pub fn confidence(mut self, confidence: f32) -> Self {
        self.confidence = Some(confidence);
        self
    }

    /// Consume the builder and produce an [`AuditLogEntry`].
    pub fn build(self) -> AuditLogEntry {
        AuditLogEntry {
            timestamp: format!(
                "{:?}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            ),
            task_id: self.task_id,
            phase: self.phase,
            agent: self.agent,
            // tool / outputs / data_classification / compliance_tags /
            // retention_policy / correlation_id are always None/empty here:
            // the builder never had setters for them (dead fields removed), and
            // they are reserved for a future audit-extension pass. `AuditLogEntry`
            // still carries them, so they are emitted as None / empty vec.
            tool: None,
            decision: self.decision,
            inputs: serde_json::to_value(self.inputs).unwrap_or_default(),
            outputs: None,
            error: self.error,
            confidence: self.confidence,
            data_classification: None,
            compliance_tags: vec![],
            retention_policy: None,
            correlation_id: None,
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intelligence::weighted_vote::Vote;

    /// Deterministic voter used to seed `GLOBAL_VOTERS` in tests: votes
    /// `approves` on every call regardless of context.
    struct TestVoter {
        name: &'static str,
        approves: bool,
    }

    #[async_trait::async_trait]
    impl AgentVoter for TestVoter {
        fn name(&self) -> &str {
            self.name
        }

        async fn vote(&self, _context: &str) -> Vote {
            weighted_vote::Vote {
                approves: self.approves,
                reasoning: "test voter".to_string(),
                confidence: 0.8,
            }
        }
    }

    /// Register deterministic voters (2 approve, 1 reject) and enable the
    /// Delphi branch. `GLOBAL_VOTERS` is a replaceable `RwLock` — the
    /// server-building unit tests (e.g. `acp::tests::test_server_builder`)
    /// register real voters via `init_intelligence_hub`, so this helper
    /// **overwrites** the voter set on every call to keep every hub test
    /// deterministic regardless of test ordering or parallelism.
    ///
    /// Directly seeding `GLOBAL_VOTERS` instead of calling
    /// `init_intelligence_hub` keeps tests deterministic: no env-dependent
    /// `DeepSeekVoter` registration and no network access.
    fn ensure_hub_initialized() {
        USE_DELPHI_DEBATE.store(true, Ordering::Relaxed);
        let voters: Vec<Arc<dyn AgentVoter + Send + Sync>> = vec![
            Arc::new(TestVoter {
                name: "voter-a",
                approves: true,
            }),
            Arc::new(TestVoter {
                name: "voter-b",
                approves: true,
            }),
            Arc::new(TestVoter {
                name: "voter-c",
                approves: false,
            }),
        ];
        *GLOBAL_VOTERS
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = voters;
    }

    #[tokio::test]
    async fn test_rationalize_high_confidence() {
        ensure_hub_initialized();
        let rounds_before = CONSENSUS_ROUNDS.load(Ordering::Relaxed);
        let (justified, _reason) = rationalize_decision("agent-x", "simple-task", 0.95).await;
        assert!(justified);
        assert!(RATIONALIZATION_COUNT.load(Ordering::Relaxed) > 0);
        // The registered voters must have been engaged: the Delphi debate runs
        // real rounds (the hardcoded fallback path never increments this).
        assert!(
            CONSENSUS_ROUNDS.load(Ordering::Relaxed) > rounds_before,
            "delphi debate must run real voter rounds"
        );
    }

    #[tokio::test]
    async fn test_rationalize_low_confidence() {
        ensure_hub_initialized();
        let rounds_before = CONSENSUS_ROUNDS.load(Ordering::Relaxed);
        let (justified, reason) =
            rationalize_decision("agent-x", "risky-task with delete and rm", 0.15).await;
        // Low confidence + risk keywords = rejected
        assert!(!justified);
        assert!(!reason.is_empty());
        assert!(CONSENSUS_ROUNDS.load(Ordering::Relaxed) > rounds_before);
    }

    #[tokio::test]
    async fn test_rationalize_safe_high_confidence() {
        ensure_hub_initialized();
        let rounds_before = CONSENSUS_ROUNDS.load(Ordering::Relaxed);
        // Safe task with high confidence should pass
        let (justified, _reason) = rationalize_decision("agent-x", "read file content", 0.95).await;
        assert!(justified);
        assert!(CONSENSUS_ROUNDS.load(Ordering::Relaxed) > rounds_before);
    }

    #[tokio::test]
    async fn test_rationalize_risky_but_confident() {
        ensure_hub_initialized();
        let rounds_before = CONSENSUS_ROUNDS.load(Ordering::Relaxed);
        // Risky task but very high confidence might still pass
        let (justified, _reason) =
            rationalize_decision("agent-x", "delete temporary cache files", 0.98).await;
        assert!(justified);
        assert!(CONSENSUS_ROUNDS.load(Ordering::Relaxed) > rounds_before);
    }

    /// The Delphi branch must be driven by the registered voters, not the
    /// hardcoded fallback: reputation weights flip the weighted outcome while
    /// the voter set stays fixed.
    #[tokio::test]
    async fn test_delphi_branch_uses_registered_voters() {
        ensure_hub_initialized();
        let config = VoteConfig::default(); // DelphiDebate mode
        let proposal = serde_json::json!({ "confidence": 0.95, "risk_level": "low" });

        // Default weights: the 2-approve/1-reject set reaches 2/3 -> approve.
        let (approved, _) = consensus_vote_with_reputation(
            "delphi-1",
            proposal.clone(),
            true,
            &HashMap::new(),
            &config,
        )
        .await;
        assert!(
            approved,
            "delphi weighted vote approves with default weights"
        );

        // Same voters, skewed reputations: the approving voters carry low
        // reputation and the rejecting voter high, so weighted_yes drops below
        // the 0.6 threshold and the same debate now rejects.
        let mut reputations = HashMap::new();
        reputations.insert("voter-a".to_string(), 0.1);
        reputations.insert("voter-b".to_string(), 0.1);
        reputations.insert("voter-c".to_string(), 1.0);
        let (approved_weighted, _) =
            consensus_vote_with_reputation("delphi-2", proposal, true, &reputations, &config).await;
        assert!(
            !approved_weighted,
            "reputation-weighted delphi vote must reject"
        );
    }

    /// `VoteMode::Legacy` must be a real simple majority (>50% of unweighted
    /// votes), distinct from the reputation-weighted path: with the same raw
    /// votes (2 approve, 1 reject), Legacy approves by majority while the
    /// Weighted mode rejects because the approving voters carry low reputation.
    #[tokio::test]
    async fn test_legacy_mode_is_simple_majority() {
        ensure_hub_initialized();
        let proposal = serde_json::json!({ "confidence": 0.9, "risk_level": "low" });
        let mut reputations = HashMap::new();
        reputations.insert("voter-a".to_string(), 0.1);
        reputations.insert("voter-b".to_string(), 0.1);
        reputations.insert("voter-c".to_string(), 1.0);

        let legacy_config = VoteConfig {
            mode: VoteMode::Legacy,
            ..VoteConfig::default()
        };
        let (legacy_approved, _) = consensus_vote_with_reputation(
            "legacy-1",
            proposal.clone(),
            true,
            &reputations,
            &legacy_config,
        )
        .await;
        assert!(
            legacy_approved,
            "Legacy = simple majority: 2 of 3 votes approve (>50%)"
        );

        let weighted_config = VoteConfig {
            mode: VoteMode::Weighted,
            ..VoteConfig::default()
        };
        let (weighted_approved, _) = consensus_vote_with_reputation(
            "weighted-1",
            proposal,
            true,
            &reputations,
            &weighted_config,
        )
        .await;
        assert!(
            !weighted_approved,
            "Weighted = reputation-weighted: low-weight approvers miss the 0.6 threshold"
        );
    }

    #[test]
    fn test_audit_entry() {
        let entry = AuditEntryBuilder::new("task-001", "chat", "allow")
            .agent("agent-a")
            .inputs(serde_json::json!({"input": "test"}))
            .confidence(0.95)
            .build();
        record_audit_entry(entry);
        assert!(
            !crate::governance::audit::global_audit_log().is_empty(),
            "audit entry must reach the canonical sink"
        );
    }
}
