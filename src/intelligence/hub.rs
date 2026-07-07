//! BLUE48 — Intelligence Integration Hub
//!
//! Wires orphaned intelligence/governance modules into the hot execution path:
//! - ConsensusEngine → multi-agent voting in CapabilityBus.decide()
//! - MultiModelVoter → parallel model voting in FullAutoFlow
//! - Rationalization → decision explanation in response assembly
//! - Audit → governance audit trail
//!
//! All integrations are non-blocking: failures in any module log a warning
//! but never crash the calling thread.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use std::collections::HashMap;

use std::sync::{Arc, OnceLock};

use crate::config::AgentConfig;
use crate::governance::audit::{AuditLogEntry, ThreadSafeAuditLog};
use crate::governance::rationalization::SelfRationalizationGuard;
use crate::intelligence::capability_bus::core::CapabilityBus;
use crate::intelligence::consensus::{ConsensusEngine, ConsensusNode, NodeRole};
use crate::intelligence::voter_impls::{
    CapabilityBusVoter, DeepSeekVoter, LocalAgentVoter, LocalVoter, RationalizationGuardVoter,
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
/// How many audit entries were recorded.
pub static AUDIT_ENTRY_COUNT: AtomicU64 = AtomicU64::new(0);

/// Whether Delphi-method debate voting is enabled in rationalize_decision.
static USE_DELPHI_DEBATE: AtomicBool = AtomicBool::new(true);

// ── Global instances ──────────────────────────────────────────────────────

static GLOBAL_CONSENSUS: LazyLock<Mutex<ConsensusEngine>> =
    LazyLock::new(|| Mutex::new(ConsensusEngine::new(Default::default())));

static GLOBAL_RATIONALIZATION: LazyLock<Mutex<SelfRationalizationGuard>> =
    LazyLock::new(|| Mutex::new(SelfRationalizationGuard::new(0.3)));

/// Global voters for the Delphi debate / weighted-vote system.
/// Initialised via [`init_intel_voters`] at server startup.
static GLOBAL_VOTERS: OnceLock<Vec<Box<dyn AgentVoter + Send + Sync>>> = OnceLock::new();

static GLOBAL_AUDIT: LazyLock<ThreadSafeAuditLog> = LazyLock::new(|| {
    let audit_path: std::path::PathBuf = std::env::temp_dir().join("goon-audit.ndjson");
    ThreadSafeAuditLog::new_with_path(10_000, audit_path)
});

/// Snapshot of all intelligence hub metric counters.
///
/// Used by the governance health endpoint to expose hub activity
/// (I5 — wire the dead-code counters into a read-side).
pub fn hub_metrics() -> serde_json::Value {
    serde_json::json!({
        "intel_hub_activations": INTEL_HUB_ACTIVATIONS.load(Ordering::Relaxed),
        "consensus_rounds": CONSENSUS_ROUNDS.load(Ordering::Relaxed),
        "rationalization_count": RATIONALIZATION_COUNT.load(Ordering::Relaxed),
        "audit_entry_count": AUDIT_ENTRY_COUNT.load(Ordering::Relaxed),
    })
}

// ── Default node addresses ───────────────────────────────────────────────

/// Default address for the local agent consensus node.
/// Uses `internal://` scheme because these are in-process logical nodes
/// with no network transport — the consensus engine routes votes entirely
/// within the same memory space. No DNS / TCP resolution is required.
pub const DEFAULT_LOCAL_AGENT_ADDRESS: &str = "internal://local";

/// Default address for the capability bus consensus node.
/// Same rationale as `DEFAULT_LOCAL_AGENT_ADDRESS` — the capability bus
/// is an in-process component, not a remote service, so an `internal://`
/// scheme avoids unnecessary network overhead and keeps the consensus
/// loop zero-allocation for local decisions.
pub const DEFAULT_CAPABILITY_BUS_ADDRESS: &str = "internal://capability_bus";

// ── Public API ──────────────────────────────────────────────────────────────

/// Initialize intelligence hub at server startup.
/// Registers local nodes in the consensus engine.
///
/// `enable_delphi_debate` — when `true`, `rationalize_decision` will
/// use the weighted reputation + Delphi debate voting path instead of
/// the basic rationalization guard.
///
/// Addresses default to `internal://local` and `internal://capability_bus`
/// because both consensus nodes are in-process logical entities with no
/// network transport. Override by passing custom addresses if the consensus
/// engine needs to reference external or multi-process nodes.
pub fn init_intel_hub(enable_delphi_debate: bool) {
    init_intel_hub_with_addrs(
        enable_delphi_debate,
        DEFAULT_LOCAL_AGENT_ADDRESS,
        DEFAULT_CAPABILITY_BUS_ADDRESS,
    )
}

/// Initialize intelligence hub with configurable consensus node addresses.
///
/// `local_agent_address` — address for the local agent consensus node.
/// `capability_bus_address` — address for the capability bus consensus node.
pub fn init_intel_hub_with_addrs(
    enable_delphi_debate: bool,
    local_agent_address: &str,
    capability_bus_address: &str,
) {
    let consensus = match GLOBAL_CONSENSUS.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::warn!("[B48] GLOBAL_CONSENSUS lock poisoned, recovering");
            poisoned.into_inner()
        }
    };
    let _ = consensus.register_node(ConsensusNode {
        id: "local-agent".to_string(),
        address: local_agent_address.to_string(),
        weight: 1,
        role: NodeRole::Leader,
        is_online: true,
        last_heartbeat_ms: crate::intelligence::now_ms(),
    });
    let _ = consensus.register_node(ConsensusNode {
        id: "capability-bus".to_string(),
        address: capability_bus_address.to_string(),
        weight: 1,
        role: NodeRole::Follower,
        is_online: true,
        last_heartbeat_ms: crate::intelligence::now_ms(),
    });
    USE_DELPHI_DEBATE.store(enable_delphi_debate, Ordering::Relaxed);
    if enable_delphi_debate {
        tracing::info!("intel_hub: Delphi debate voting enabled");
    }
    tracing::info!("intel_hub: initialized consensus, rationalization, audit");
}

/// Initialise the 3 internal voters (CapabilityBusVoter, LocalAgentVoter,
/// RationalizationGuardVoter) and store them so that
/// [`consensus_vote_with_reputation`] can delegate to their async
/// `AgentVoter::vote()` implementations.
///
/// Call this once during server startup, *after* `init_intel_hub()`.
/// When `capability_bus` is `None`, only the `LocalAgentVoter` and
/// `RationalizationGuardVoter` are stored.
pub fn init_intel_voters(capability_bus: Option<Arc<CapabilityBus>>) {
    let mut voters: Vec<Box<dyn AgentVoter + Send + Sync>> = Vec::new();

    // CapabilityBusVoter — only when a capability bus is available.
    if let Some(bus) = capability_bus {
        voters.push(Box::new(CapabilityBusVoter::new("capability-bus", bus)));
    }

    // LocalAgentVoter — keyword-heuristic voter.
    voters.push(Box::new(LocalAgentVoter::new("local-agent")));

    // RationalizationGuardVoter — safety-guard voter.
    voters.push(Box::new(RationalizationGuardVoter::new(
        "rationalization-guard",
        Arc::new(SelfRationalizationGuard::new(0.6)),
    )));

    // DeepSeekVoter — LLM-based voter via DeepSeek API.
    let deepseek_api_key = std::env::var("DEEPSEEK_API_KEY").unwrap_or_default();
    voters.push(Box::new(DeepSeekVoter::new(
        "deepseek",
        "https://api.deepseek.com",
        "deepseek-v4-flash",
        deepseek_api_key,
    )));

    // LocalVoter — configurable local model voter.
    voters.push(Box::new(LocalVoter::new("local", AgentConfig::default())));

    let _ = GLOBAL_VOTERS.set(voters).map_err(|_| {
        tracing::warn!("intel_hub: GLOBAL_VOTERS already initialised");
    });

    tracing::info!("intel_hub: {} voter(s) registered", {
        GLOBAL_VOTERS.get().map(|v| v.len()).unwrap_or(0)
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
///    from [`ReputationStore`] (passed via `reputations`).
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
    match config.mode {
        VoteMode::Legacy | VoteMode::Weighted | VoteMode::DelphiDebate => {
            // Continue with weighted / Delphi logic
        }
    }

    let proposal_confidence = proposal
        .get("confidence")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.5);
    let is_risky = proposal
        .get("risk_level")
        .and_then(|v| v.as_str())
        .map(|s| matches!(s, "high" | "critical"))
        .unwrap_or(false);

    // Collect votes — prefer stored AgentVoter impls, fall back to hardcoded.
    // This is now truly async-safe: we directly await the voter futures
    // instead of blocking the current thread.
    let raw_votes = if let Some(voters) = GLOBAL_VOTERS.get() {
        // Build the voting context from the proposal
        let context = serde_json::to_string(&proposal).unwrap_or_default();

        let mut votes = HashMap::new();
        // Spawn all voters concurrently and await them directly.
        let voter_futures: Vec<_> = voters
            .iter()
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
    } else {
        // No voters registered — build hardcoded votes (legacy path)
        let mut votes = std::collections::HashMap::new();

        let cb_approve = if is_risky {
            approve && proposal_confidence > 0.6
        } else {
            approve || proposal_confidence > 0.7
        };
        votes.insert(
            "capability-bus".to_string(),
            weighted_vote::Vote {
                approves: cb_approve,
                reasoning: format!(
                    "proposal_confidence={}, is_risky={}",
                    proposal_confidence, is_risky
                ),
                confidence: proposal_confidence,
            },
        );

        votes.insert(
            "local-agent".to_string(),
            weighted_vote::Vote {
                approves: approve,
                reasoning: "Caller intent".to_string(),
                confidence: 0.7,
            },
        );

        let rg_approve = if is_risky {
            proposal_confidence > 0.5
        } else {
            proposal_confidence > 0.3
        };
        votes.insert(
            "rationalization-guard".to_string(),
            weighted_vote::Vote {
                approves: rg_approve,
                reasoning: format!(
                    "risk_assessment: confidence={}, risky={}",
                    proposal_confidence, is_risky
                ),
                confidence: proposal_confidence.max(0.3),
            },
        );
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
            if let Some(voters) = GLOBAL_VOTERS.get() {
                if !voters.is_empty() {
                    let agent_refs: Vec<&dyn AgentVoter> = voters
                        .iter()
                        .map(|b| b.as_ref() as &dyn AgentVoter)
                        .collect();
                    let debate_question = debate_context.clone();
                    let delphi_config = config.delphi.clone();
                    let result =
                        delphi_debate(&agent_refs, &debate_question, reputations, &delphi_config)
                            .await;
                    tracing::info!(
                        "delphi_debate: {} rounds, converged={}, approved={}",
                        result.rounds,
                        result.converged,
                        result.final_result.approved
                    );
                    result.final_result
                } else {
                    // No voters — fall back to simple weighted vote.
                    weighted_vote::weighted_vote(
                        &raw_votes,
                        reputations,
                        config.delphi.threshold,
                        config.delphi.default_weight,
                        &debate_context,
                    )
                }
            } else {
                // No voters registered — simple weighted vote.
                weighted_vote::weighted_vote(
                    &raw_votes,
                    reputations,
                    config.delphi.threshold,
                    config.delphi.default_weight,
                    &debate_context,
                )
            }
        }
        VoteMode::Weighted | VoteMode::Legacy => weighted_vote::weighted_vote(
            &raw_votes,
            reputations,
            config.weighted.threshold,
            config.weighted.default_weight,
            "",
        ),
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
    // ── Delphi debate integration ────────────────────────────────────────
    // When enabled, delegate to the weighted reputation + Delphi debate
    // voting path for higher-confidence decision verification.
    if USE_DELPHI_DEBATE.load(Ordering::Relaxed) {
        let proposal = serde_json::json!({
            "confidence": confidence,
            "risk_level": if task.to_lowercase().contains("delete")
                || task.to_lowercase().contains("remove")
                || task.to_lowercase().contains("shell")
                || task.to_lowercase().contains("sudo")
            {
                "high"
            } else {
                "low"
            },
        });
        let reputations = HashMap::new();
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

    // Multi-factor risk scoring
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

    let blocked = guard.evaluate(&mut annotation, adjusted_confidence as f32, false);

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
    GLOBAL_AUDIT.record(entry);
    AUDIT_ENTRY_COUNT.fetch_add(1, Ordering::Relaxed);
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
///     .tool("read_file")
///     .inputs(serde_json::json!({"input": "test"}))
///     .confidence(0.95)
///     .build();
/// ```
pub struct AuditEntryBuilder {
    task_id: String,
    phase: String,
    decision: String,
    agent: Option<String>,
    tool: Option<String>,
    inputs: serde_json::Value,
    outputs: Option<serde_json::Value>,
    error: Option<String>,
    confidence: Option<f32>,
    data_classification: Option<String>,
    compliance_tags: Vec<String>,
    retention_policy: Option<String>,
    correlation_id: Option<String>,
}

impl AuditEntryBuilder {
    /// Start building an audit entry with the minimum required fields.
    pub fn new(task_id: &str, phase: &str, decision: &str) -> Self {
        Self {
            task_id: task_id.to_string(),
            phase: phase.to_string(),
            decision: decision.to_string(),
            agent: None,
            tool: None,
            inputs: serde_json::Value::Null,
            outputs: None,
            error: None,
            confidence: None,
            data_classification: None,
            compliance_tags: vec![],
            retention_policy: None,
            correlation_id: None,
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
            tool: self.tool,
            decision: self.decision,
            inputs: serde_json::to_value(self.inputs).unwrap_or_default(),
            outputs: self
                .outputs
                .map(|o| serde_json::to_value(o).unwrap_or_default()),
            error: self.error,
            confidence: self.confidence,
            data_classification: self.data_classification,
            compliance_tags: self.compliance_tags,
            retention_policy: self.retention_policy,
            correlation_id: self.correlation_id,
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rationalize_high_confidence() {
        let (justified, _reason) = rationalize_decision("agent-x", "simple-task", 0.95).await;
        assert!(justified);
        assert!(RATIONALIZATION_COUNT.load(Ordering::Relaxed) > 0);
    }

    #[tokio::test]
    async fn test_rationalize_low_confidence() {
        let (justified, reason) =
            rationalize_decision("agent-x", "risky-task with delete and rm", 0.15).await;
        // Low confidence + risk keywords = rejected
        assert!(!justified);
        assert!(!reason.is_empty());
    }

    #[tokio::test]
    async fn test_rationalize_safe_high_confidence() {
        // Safe task with high confidence should pass
        let (justified, _reason) = rationalize_decision("agent-x", "read file content", 0.95).await;
        assert!(justified);
    }

    #[tokio::test]
    async fn test_rationalize_risky_but_confident() {
        // Risky task but very high confidence might still pass
        let (justified, _reason) =
            rationalize_decision("agent-x", "delete temporary cache files", 0.98).await;
        assert!(justified);
    }

    #[test]
    fn test_audit_entry() {
        let entry = AuditEntryBuilder::new("task-001", "chat", "allow")
            .agent("agent-a")
            .inputs(serde_json::json!({"input": "test"}))
            .confidence(0.95)
            .build();
        record_audit_entry(entry);
        assert!(AUDIT_ENTRY_COUNT.load(Ordering::Relaxed) > 0);
    }
}
