//! Sub-modules for the BLUE2 reinforcement utilities.
//!
//! The original monolithic `reinforcement.rs` has been split into focused
//! modules. This `mod.rs` re-exports all public items to preserve backward
//! compatibility for paths like `crate::reinforcement::*`.

pub mod action_check;
pub mod federated;
// Standalone P2P discovery module. Implements full peer discovery and heartbeat
// logic but has zero CapabilityBus integration. To wire it in, add CapabilityBus
// calls (e.g. registering discovered peers as capability route targets).
//
// NOTE: federated_transport is NOT gated here — it provides the foundational
// transport abstraction (PeerInfo, FederatedTransport trait) used by federated.rs.
#[cfg(feature = "sub-bus-distributed-memory")]
pub mod federated_discovery;
pub mod federated_privacy;
pub mod federated_transport;
pub mod federated_versioning;
pub mod health;
pub mod learning;
pub mod task_plan;

// ── Shared items (originally in the monolithic reinforcement.rs) ──────────

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const GOON_DIR: &str = ".goon";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactLedger {
    root: PathBuf,
}

impl ArtifactLedger {
    pub fn new(config_path: Option<&Path>) -> Self {
        let root = config_path
            .and_then(|path| path.parent().map(|parent| parent.join(GOON_DIR)))
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join(GOON_DIR)
            });
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn ensure_ready(&self) -> Result<()> {
        fs::create_dir_all(&self.root)
            .with_context(|| format!("failed to create ledger root {}", self.root.display()))
    }

    pub fn latest_path(&self, category: &str, latest_name: &str) -> PathBuf {
        self.root.join(category).join(latest_name)
    }

    pub fn write_json<T: Serialize>(
        &self,
        category: &str,
        latest_name: &str,
        value: &T,
    ) -> Result<PathBuf> {
        self.ensure_ready()?;

        let dir = self.root.join(category);
        fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create ledger category {}", dir.display()))?;

        let latest_path = dir.join(latest_name);
        let stem = latest_name.strip_suffix(".json").unwrap_or(latest_name);
        let archive_path = dir.join(format!("{}-{}.json", stem, now_ts()));
        let encoded = serde_json::to_vec_pretty(value)?;

        fs::write(&archive_path, &encoded).with_context(|| {
            format!("failed to write ledger artifact {}", archive_path.display())
        })?;
        fs::write(&latest_path, &encoded).with_context(|| {
            format!(
                "failed to write latest ledger artifact {}",
                latest_path.display()
            )
        })?;

        Ok(latest_path)
    }
}

fn now_ts() -> i64 {
    crate::acp::prelude::now_ts()
}

// ── Re-exports ────────────────────────────────────────────────────────────

pub use crate::orchestration::core_dag::TaskGraphCheckpointArtifact;
pub use action_check::{
    run_action_check, ActionCheckItem, ActionCheckKind, ActionCheckReport, FinalSummaryArtifact,
};
pub use federated::{
    AggregationMethod, ContributionWeight, DistillationRound, DistillationStatus,
    FederatedClientState, FederatedConfig, FederatedError, FederatedLearning, FederatedProfile,
    FederatedRL, FederatedRLConfig, FederatedRLProfile, FederatedResult, FederatedRound,
    ModelWeights, PolicyEntry, SharedFederatedLearning,
};
pub use federated_privacy::{DifferentialPrivacyConfig, PrivacyBudget};
pub use federated_versioning::{migrate_weights, ModelVersion, VERSION_INITIAL};
pub use health::{
    aggregate_status, build_runtime_healthcheck_report, persist_runtime_healthcheck, CheckStatus,
    ComponentReport, RuntimeHealthcheckReport,
};
pub use learning::{
    persist_knowledge_insight_event, persist_workflow_learning_event, ExperienceKnowledgeBase,
    FailurePattern, KnowledgeBusArtifact, KnowledgeInsightArtifact, LearningFeedbackSystem,
    LearningPattern, QLearningAgent, RewardFunction, RlTaskExecutionMetrics, SuccessCase,
    WorkflowLearningBusArtifact, WorkflowLearningEvent,
};

pub use task_plan::{
    build_task_plan, build_workflow_generated_artifact, load_task_graph_checkpoint,
    persist_clarification_session_artifact, persist_consultation_artifact,
    persist_execution_decision, persist_governance_policy, persist_pipeline_unified_metrics,
    persist_primary_secondary_failover_artifact, persist_primary_secondary_policy_artifact,
    persist_requirement_contract, persist_task_execution_summary, persist_task_graph_checkpoint,
    persist_task_plan, persist_workflow_generated, persist_workflow_optimization_policy,
    persist_workflow_research, persist_workflow_work_grade,
    recommend_agent_order_from_execution_history, recommend_failure_strategy_from_learning,
    recommend_parallelism_from_learning, recommend_predicted_success_rate_from_learning,
    recommend_reattach_modules_from_policy_history, recommend_work_grade_from_learning,
    CheckpointSummaryArtifact, ClarificationSessionArtifact, ConsultationArtifact,
    ExecutionAssignmentRecord, ExecutionDecisionArtifact, ExecutionDecisionCandidate,
    GovernancePolicyArtifact, ParallelPhaseDecisionRecord, PipelineUnifiedMetricsArtifact,
    PlannedSubtaskRecord, PrimaryFailoverReportItem, PrimarySecondaryFailoverArtifact,
    PrimarySecondaryPolicyArtifact, RequirementContractArtifact, TaskExecutionMetrics,
    TaskExecutionSummary, TaskPlanArtifact, WorkflowEdge, WorkflowGeneratedArtifact, WorkflowNode,
    WorkflowOptimizationPolicyArtifact, WorkflowResearchArtifact, WorkflowWorkGradeArtifact,
};

use std::sync::{Arc, Mutex};
use tracing::info;

// ── FederatedRLAdapter ────────────────────────────────────────────────────

/// Bridge between the main ACP chain and the federated learning module.
///
/// `FederatedRLAdapter` wraps `FederatedLearning` and pre-configures it with
/// differential privacy and model versioning. It serves as the initialization
/// point for federated learning in the ACP runtime.
#[derive(Debug, Clone)]
pub struct FederatedRLAdapter {
    inner: Arc<Mutex<FederatedLearning>>,
    /// Whether differential privacy is active
    pub privacy_enabled: bool,
    /// Whether model versioning is active
    pub versioning_enabled: bool,
}

impl FederatedRLAdapter {
    /// Create a new adapter with default federated config, optionally
    /// enabling privacy and versioning.
    pub fn new(enable_privacy: bool, enable_versioning: bool) -> Self {
        let mut fl = FederatedLearning::new(FederatedConfig::default());

        // Enable differential privacy with sensible defaults.
        if enable_privacy {
            let dp_config =
                DifferentialPrivacyConfig::new(4.0, 1e-5, 1.0).expect("valid default DP config");
            let budget = PrivacyBudget::new(
                4.0 * 100.0, // enough for ~100 rounds at ε=4.0/round
                100,
                dp_config,
            );
            fl = fl.with_privacy(dp_config, Some(budget));
            info!("FederatedRLAdapter: differential privacy enabled");
        }

        // Enable model versioning with the initial version.
        if enable_versioning {
            fl = fl.with_versioning(VERSION_INITIAL);
            info!("FederatedRLAdapter: model versioning enabled (v{VERSION_INITIAL})");
        }

        Self {
            inner: Arc::new(Mutex::new(fl)),
            privacy_enabled: enable_privacy,
            versioning_enabled: enable_versioning,
        }
    }

    /// Return a reference to the inner `FederatedLearning` handle.
    pub fn inner(&self) -> Arc<Mutex<FederatedLearning>> {
        Arc::clone(&self.inner)
    }

    /// Return a profile that includes privacy and versioning status.
    pub fn profile(&self) -> FederatedProfile {
        self.inner
            .lock()
            .unwrap_or_else(|e| {
                tracing::warn!("FederatedRLAdapter: profile lock poisoned");
                e.into_inner()
            })
            .profile()
    }

    /// Register a client for federated learning.
    pub fn register_client(&self, client_id: &str, weight: f64) -> anyhow::Result<()> {
        self.inner
            .lock()
            .unwrap_or_else(|e| {
                tracing::warn!("FederatedRLAdapter: register_client lock poisoned");
                e.into_inner()
            })
            .register_client(client_id, weight)
    }

    /// Submit local weights and trigger an aggregation round if enough
    /// clients have contributed.
    pub fn submit_and_aggregate(
        &self,
        client_id: &str,
        weights: ModelWeights,
        improvement: f64,
    ) -> anyhow::Result<Option<FederatedRound>> {
        let mut fl = self.inner.lock().unwrap_or_else(|e| {
            tracing::warn!("FederatedRLAdapter: submit_and_aggregate lock poisoned");
            e.into_inner()
        });
        fl.submit_local_weights(client_id, weights, improvement)?;
        if fl.pending_weights_count() >= fl.min_clients_required() {
            let round = fl.aggregate_round()?;
            Ok(Some(round))
        } else {
            Ok(None)
        }
    }
}
