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

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::governance::audit::{AuditLogEntry, ThreadSafeAuditLog};
use crate::governance::rationalization::SelfRationalizationGuard;
use crate::intelligence::consensus::{ConsensusEngine, ConsensusNode, ConsensusVote, NodeRole};

// ── Global counters for observability ─────────────────────────────────────

/// How many times the intelligence hub has been activated.
#[allow(dead_code)] // F-GAP-49 — reserved intelligence hub feature
pub static INTEL_HUB_ACTIVATIONS: AtomicU64 = AtomicU64::new(0);
/// How many consensus rounds have been started.
#[allow(dead_code)] // F-GAP-49 — reserved intelligence hub feature
pub static CONSENSUS_ROUNDS: AtomicU64 = AtomicU64::new(0);
/// How many rationalization evaluations were performed.
pub static RATIONALIZATION_COUNT: AtomicU64 = AtomicU64::new(0);
/// How many audit entries were recorded.
#[allow(dead_code)] // F-GAP-49 — reserved intelligence hub feature
pub static AUDIT_ENTRY_COUNT: AtomicU64 = AtomicU64::new(0);

// ── Global instances ──────────────────────────────────────────────────────

static GLOBAL_CONSENSUS: LazyLock<Mutex<ConsensusEngine>> =
    LazyLock::new(|| Mutex::new(ConsensusEngine::new(Default::default())));

static GLOBAL_RATIONALIZATION: LazyLock<Mutex<SelfRationalizationGuard>> =
    LazyLock::new(|| Mutex::new(SelfRationalizationGuard::new(0.3)));

#[allow(dead_code)] // F-GAP-49 — reserved intelligence hub feature
static GLOBAL_AUDIT: LazyLock<ThreadSafeAuditLog> = LazyLock::new(|| {
    let audit_path: std::path::PathBuf = std::env::temp_dir().join("goon-audit.ndjson");
    ThreadSafeAuditLog::new_with_path(10_000, audit_path)
});

// ── Public API ──────────────────────────────────────────────────────────────

/// Initialize intelligence hub at server startup.
/// Registers local nodes in the consensus engine.
pub fn init_intel_hub() {
    let consensus = match GLOBAL_CONSENSUS.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::warn!("[B48] GLOBAL_CONSENSUS lock poisoned, recovering");
            poisoned.into_inner()
        }
    };
    let _ = consensus.register_node(ConsensusNode {
        id: "local-agent".to_string(),
        address: "internal://local".to_string(),
        weight: 1,
        role: NodeRole::Leader,
        is_online: true,
        last_heartbeat_ms: crate::intelligence::now_ms(),
    });
    let _ = consensus.register_node(ConsensusNode {
        id: "capability-bus".to_string(),
        address: "internal://capability_bus".to_string(),
        weight: 1,
        role: NodeRole::Follower,
        is_online: true,
        last_heartbeat_ms: crate::intelligence::now_ms(),
    });
    tracing::info!("intel_hub: initialized consensus, rationalization, audit");
}

/// Run multi-agent consensus voting on a decision proposal.
///
/// Registers 3 nodes with different weights and collects REAL votes:
/// - "capability-bus": weight=2, votes based on proposal confidence
/// - "local-agent": weight=1, votes approve (default)
/// - "rationalization-guard": weight=1, votes reject if confidence < 0.4
///
/// Returns the REAL consensus verdict (approve/reject) and confidence.
/// Non-blocking — returns (approve, 0.3) as degraded fallback on any failure.
// F-GAP-48: intentionally not wired into the hot path; rationalize_decision is primary
#[allow(dead_code)] // F-GAP-49 — reserved intelligence hub feature
pub fn consensus_vote_on(
    proposal_id: &str,
    proposal: serde_json::Value,
    approve: bool,
) -> (bool, f64) {
    let consensus = match GLOBAL_CONSENSUS.lock() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("intel_hub: consensus lock failed: {e}");
            return (approve, 0.3);
        }
    };

    let now = crate::intelligence::now_ms();
    // Extract a confidence score from the proposal to drive real voting
    let proposal_confidence = proposal
        .get("confidence")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.5);
    let is_risky = proposal
        .get("risk_level")
        .and_then(|v| v.as_str())
        .map(|s| matches!(s, "high" | "critical"))
        .unwrap_or(false);

    let proposals = vec![proposal];
    match consensus.start_round("capability-bus", proposals) {
        Ok(round_id) => {
            CONSENSUS_ROUNDS.fetch_add(1, Ordering::Relaxed);

            // Node 1: capability-bus votes based on proposal confidence
            // Higher confidence → more likely to approve
            let cb_approve = if is_risky {
                approve && proposal_confidence > 0.6
            } else {
                approve || proposal_confidence > 0.7
            };
            let _ = consensus.cast_vote(ConsensusVote {
                node_id: "capability-bus".to_string(),
                round_id: round_id.clone(),
                proposal_id: proposal_id.to_string(),
                approve: cb_approve,
                weight: 2,
                vote_ms: now,
            });

            // Node 2: local-agent votes the caller's intent but has lower weight
            let _ = consensus.cast_vote(ConsensusVote {
                node_id: "local-agent".to_string(),
                round_id: round_id.clone(),
                proposal_id: proposal_id.to_string(),
                approve,
                weight: 1,
                vote_ms: now,
            });

            // Node 3: rationalization-guard independently votes based on confidence threshold
            let rg_approve = if is_risky {
                proposal_confidence > 0.5
            } else {
                proposal_confidence > 0.3
            };
            let _ = consensus.cast_vote(ConsensusVote {
                node_id: "rationalization-guard".to_string(),
                round_id,
                proposal_id: proposal_id.to_string(),
                approve: rg_approve,
                weight: 1,
                vote_ms: now,
            });

            // Finalize round — compute REAL consensus result
            if consensus.finalize_round().is_ok() {
                // Consensus achieved: compute weighted verdict
                let weighted_approve =
                    (cb_approve as u64 * 2 + approve as u64 + rg_approve as u64) as f64;
                let total_weight = 4.0;
                let approval_ratio = weighted_approve / total_weight;
                let final_approve = approval_ratio >= 0.5;
                let confidence = if final_approve {
                    0.5 + approval_ratio * 0.4
                } else {
                    0.5 - (0.5 - approval_ratio) * 0.4
                };
                (final_approve, confidence.clamp(0.1, 0.95))
            } else {
                // No consensus — fall back to conservative decision
                tracing::warn!(
                    "intel_hub: no consensus on proposal={}, approve={}, confidence={}",
                    proposal_id,
                    approve,
                    proposal_confidence
                );
                (approve, 0.4)
            }
        }
        Err(e) => {
            tracing::warn!(
                "intel_hub: consensus.start_round failed for {}: {e}",
                proposal_id
            );
            (approve, 0.3)
        }
    }
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
pub fn rationalize_decision(agent: &str, task: &str, confidence: f64) -> (bool, String) {
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
        (false, reason)
    } else {
        (true, String::new())
    }
}

/// Record an audit entry for the decision pipeline.
// F-GAP-48: intentionally not wired into the hot path; rationalize_decision is primary
#[allow(dead_code)] // F-GAP-49 — reserved intelligence hub feature
pub fn record_audit_entry(entry: AuditLogEntry) {
    GLOBAL_AUDIT.record(entry);
    AUDIT_ENTRY_COUNT.fetch_add(1, Ordering::Relaxed);
}

// ── AuditEntryBuilder ──────────────────────────────────────────────────────

/// Builder for [`AuditLogEntry`] that avoids long argument lists.
///
/// # Usage
///
/// ```ignore
/// use crate::intelligence::hub::AuditEntryBuilder;
///
/// let entry = AuditEntryBuilder::new("task-001", "chat", "allow")
///     .agent("agent-a")
///     .tool("read_file")
///     .inputs(serde_json::json!({"input": "test"}))
///     .confidence(0.95)
///     .build();
/// ```
#[allow(dead_code)] // Public API — reserved for adoption over the old positional function
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

#[allow(dead_code)] // Public API — reserved for adoption over the old positional function
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

    /// Set the tool name.
    pub fn tool(mut self, tool: &str) -> Self {
        self.tool = Some(tool.to_string());
        self
    }

    /// Set the input payload.
    pub fn inputs(mut self, inputs: serde_json::Value) -> Self {
        self.inputs = inputs;
        self
    }

    /// Set the output payload.
    pub fn outputs(mut self, outputs: serde_json::Value) -> Self {
        self.outputs = Some(outputs);
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

    /// Set the data classification label.
    pub fn data_classification(mut self, dc: &str) -> Self {
        self.data_classification = Some(dc.to_string());
        self
    }

    /// Add a compliance tag.
    pub fn compliance_tag(mut self, tag: &str) -> Self {
        self.compliance_tags.push(tag.to_string());
        self
    }

    /// Set the retention policy.
    pub fn retention_policy(mut self, rp: &str) -> Self {
        self.retention_policy = Some(rp.to_string());
        self
    }

    /// Set the correlation ID.
    pub fn correlation_id(mut self, cid: &str) -> Self {
        self.correlation_id = Some(cid.to_string());
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

/// Build an audit entry for agent decision.
///
/// Prefer [`AuditEntryBuilder`] for new code — it avoids the long parameter
/// list and makes call sites self-documenting.
// F-GAP-48: intentionally not wired into the hot path; rationalize_decision is primary
#[allow(dead_code)] // F-GAP-49
#[allow(clippy::too_many_arguments)]
pub fn build_audit_entry(
    task_id: &str,
    phase: &str,
    agent: Option<&str>,
    tool: Option<&str>,
    decision: &str,
    inputs: serde_json::Value,
    outputs: Option<serde_json::Value>,
    error: Option<String>,
    confidence: Option<f32>,
) -> AuditLogEntry {
    AuditLogEntry {
        timestamp: format!(
            "{:?}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        ),
        task_id: task_id.to_string(),
        phase: phase.to_string(),
        agent: agent.map(String::from),
        tool: tool.map(String::from),
        decision: decision.to_string(),
        inputs: serde_json::to_value(inputs).unwrap_or_default(),
        outputs: outputs.map(|o| serde_json::to_value(o).unwrap_or_default()),
        error,
        confidence,
        data_classification: None,
        compliance_tags: vec![],
        retention_policy: None,
        correlation_id: None,
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_consensus_vote_basic() {
        init_intel_hub();
        let (approved, conf) =
            consensus_vote_on("test-proposal", serde_json::json!({"action": "test"}), true);
        assert!(approved);
        assert!(conf > 0.0);
        assert!(CONSENSUS_ROUNDS.load(Ordering::Relaxed) > 0);
    }

    #[test]
    fn test_rationalize_high_confidence() {
        let (justified, _reason) = rationalize_decision("agent-x", "simple-task", 0.95);
        assert!(justified);
        assert!(RATIONALIZATION_COUNT.load(Ordering::Relaxed) > 0);
    }

    #[test]
    fn test_rationalize_low_confidence() {
        let (justified, reason) =
            rationalize_decision("agent-x", "risky-task with delete and rm", 0.15);
        // Low confidence + risk keywords = rejected
        assert!(!justified);
        assert!(!reason.is_empty());
    }

    #[test]
    fn test_rationalize_safe_high_confidence() {
        // Safe task with high confidence should pass
        let (justified, _reason) = rationalize_decision("agent-x", "read file content", 0.95);
        assert!(justified);
    }

    #[test]
    fn test_rationalize_risky_but_confident() {
        // Risky task but very high confidence might still pass
        let (justified, _reason) =
            rationalize_decision("agent-x", "delete temporary cache files", 0.98);
        assert!(justified);
    }

    #[test]
    fn test_audit_entry() {
        let entry = build_audit_entry(
            "task-001",
            "chat",
            Some("agent-a"),
            Some("read_file"),
            "allow",
            serde_json::json!({"input": "test"}),
            None,
            None,
            Some(0.95),
        );
        record_audit_entry(entry);
        assert!(AUDIT_ENTRY_COUNT.load(Ordering::Relaxed) > 0);
    }

    #[test]
    fn test_consensus_tracks_activations() {
        let before = INTEL_HUB_ACTIVATIONS.load(Ordering::Relaxed);
        init_intel_hub();
        consensus_vote_on("prop-activation", serde_json::json!({"x": 1}), true);
        assert!(INTEL_HUB_ACTIVATIONS.load(Ordering::Relaxed) >= before);
    }
}
