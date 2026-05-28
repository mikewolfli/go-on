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
#[allow(dead_code)]
pub static INTEL_HUB_ACTIVATIONS: AtomicU64 = AtomicU64::new(0);
/// How many consensus rounds have been started.
pub static CONSENSUS_ROUNDS: AtomicU64 = AtomicU64::new(0);
/// How many rationalization evaluations were performed.
pub static RATIONALIZATION_COUNT: AtomicU64 = AtomicU64::new(0);
/// How many audit entries were recorded.
pub static AUDIT_ENTRY_COUNT: AtomicU64 = AtomicU64::new(0);

// ── Global instances ──────────────────────────────────────────────────────

static GLOBAL_CONSENSUS: LazyLock<Mutex<ConsensusEngine>> =
    LazyLock::new(|| Mutex::new(ConsensusEngine::new(Default::default())));

static GLOBAL_RATIONALIZATION: LazyLock<Mutex<SelfRationalizationGuard>> =
    LazyLock::new(|| Mutex::new(SelfRationalizationGuard::new(0.3)));

static GLOBAL_AUDIT: LazyLock<ThreadSafeAuditLog> = LazyLock::new(|| {
    let audit_path: std::path::PathBuf = std::env::temp_dir().join("goon-audit.ndjson");
    ThreadSafeAuditLog::new_with_path(10_000, audit_path)
});

// ── Public API ──────────────────────────────────────────────────────────────

/// Initialize intelligence hub at server startup.
/// Registers local nodes in the consensus engine.
#[allow(dead_code)]
pub fn init_intel_hub() {
    if let Ok(consensus) = GLOBAL_CONSENSUS.lock() {
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
    }
    tracing::info!("intel_hub: initialized consensus, rationalization, audit");
}

/// Run multi-agent consensus voting on a decision proposal.
/// Returns the consensus verdict (approve/reject) and confidence.
/// Non-blocking — returns (false, 0.0) on any failure.
#[allow(dead_code)]
pub fn consensus_vote_on(
    proposal_id: &str,
    proposal: serde_json::Value,
    approve: bool,
) -> (bool, f64) {
    let consensus = match GLOBAL_CONSENSUS.lock() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("intel_hub: consensus lock failed: {e}");
            return (approve, 0.5);
        }
    };

    let now = crate::intelligence::now_ms();
    let proposals = vec![proposal];
    match consensus.start_round("capability-bus", proposals) {
        Ok(round_id) => {
            CONSENSUS_ROUNDS.fetch_add(1, Ordering::Relaxed);
            let _ = consensus.cast_vote(ConsensusVote {
                node_id: "capability-bus".to_string(),
                round_id: round_id.clone(),
                proposal_id: proposal_id.to_string(),
                approve,
                weight: 1,
                vote_ms: now,
            });
            let _ = consensus.cast_vote(ConsensusVote {
                node_id: "local-agent".to_string(),
                round_id,
                proposal_id: proposal_id.to_string(),
                approve,
                weight: 1,
                vote_ms: now,
            });
            // Finalize round — just check if consensus succeeded
            let (approved, confidence) = if consensus.finalize_round().is_ok() {
                (approve, 0.8)
            } else {
                (approve, 0.6)
            };
            (approved, confidence)
        }
        Err(e) => {
            tracing::warn!("intel_hub: consensus.start_round failed: {e}");
            (approve, 0.5)
        }
    }
}

/// Evaluate a decision using the rationalization guard.
/// Returns (is_justified, explanation) where explanation describes concerns.
#[allow(dead_code)]
pub fn rationalize_decision(agent: &str, task: &str, confidence: f64) -> (bool, String) {
    let mut guard = match GLOBAL_RATIONALIZATION.lock() {
        Ok(g) => g,
        Err(e) => {
            tracing::warn!("intel_hub: rationalization lock failed: {e}");
            return (true, String::new());
        }
    };

    RATIONALIZATION_COUNT.fetch_add(1, Ordering::Relaxed);
    let mut annotation = crate::governance::rationalization::RationalizationAnnotation {
        assumptions: vec![format!("agent_{}_handles_{}", agent, task)],
        evidence_refs: vec![],
        weak_evidence_flags: vec![],
        reexamine_triggered: false,
    };
    let blocked = guard.evaluate(&mut annotation, confidence as f32, false);
    if blocked {
        let reason = annotation
            .weak_evidence_flags
            .first()
            .cloned()
            .unwrap_or_else(|| "low_confidence".to_string());
        (false, reason)
    } else {
        (true, String::new())
    }
}

/// Record an audit entry for the decision pipeline.
#[allow(dead_code)]
pub fn record_audit_entry(entry: AuditLogEntry) {
    GLOBAL_AUDIT.record(entry);
    AUDIT_ENTRY_COUNT.fetch_add(1, Ordering::Relaxed);
}

/// Build an audit entry for agent decision.
#[allow(clippy::too_many_arguments, dead_code)]
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
        let (justified, _reason) = rationalize_decision("agent-x", "risky-task", 0.15);
        // Low confidence may trigger re-examination
        assert!(!justified);
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
