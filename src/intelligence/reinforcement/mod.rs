//! Sub-modules for the BLUE2 reinforcement utilities.
//!
//! The original monolithic `reinforcement.rs` has been split into focused
//! modules. This `mod.rs` re-exports all public items to preserve backward
//! compatibility for paths like `crate::reinforcement::*`.

pub mod action_check;
// The former federated learning family (federated.rs, federated_discovery.rs,
// federated_privacy.rs, federated_transport.rs, federated_versioning.rs) was
// removed: its only entry points were FederatedRLAdapter and
// CapabilityBus.federated_learning, both of which had zero production callers
// (the CapabilityBus field is always None). The standalone Q-learning
// ReinforcementBus remains the single-node learning path.
pub mod health;
pub mod learning;
pub mod task_plan;

// ── Shared items (originally in the monolithic reinforcement.rs) ──────────

use std::fs;
use std::path::{Path, PathBuf};

/// Maximum number of timestamped archive files to retain per artifact stem.
/// Beyond this limit, the oldest archives are pruned on each write.
const MAX_ARCHIVES_PER_STEM: usize = 10;

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

        // ── Prune old archives ────────────────────────────────────────────────
        // Keep at most MAX_ARCHIVES_PER_STEM archive files per artifact stem.
        // List all files matching `{stem}-*.json`, sort by name (timestamp order),
        // and remove the oldest ones.
        if let Ok(mut entries) = fs::read_dir(&dir) {
            let mut archives: Vec<PathBuf> = Vec::new();
            let prefix = format!("{}-", stem);
            while let Some(Ok(entry)) = entries.next() {
                let path = entry.path();
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with(&prefix) && name != latest_name {
                        archives.push(path);
                    }
                }
            }
            archives.sort();
            while archives.len() > MAX_ARCHIVES_PER_STEM {
                let oldest = archives.remove(0);
                let _ = fs::remove_file(&oldest);
            }
        }

        Ok(latest_path)
    }
}

fn now_ts() -> i64 {
    crate::shared::timestamps::now_ts()
}

// ── Re-exports ────────────────────────────────────────────────────────────

pub use action_check::{
    run_action_check, ActionCheckItem, ActionCheckKind, ActionCheckReport, FinalSummaryArtifact,
};
pub use health::{
    aggregate_status, build_runtime_healthcheck_report, persist_runtime_healthcheck, CheckStatus,
    ComponentReport, RuntimeHealthcheckReport,
};
pub use learning::{
    persist_knowledge_insight_event, persist_workflow_learning_event, KnowledgeBusArtifact,
    KnowledgeInsightArtifact, RewardFunction, RlTaskExecutionMetrics, WorkflowLearningBusArtifact,
    WorkflowLearningEvent,
};

pub use task_plan::{
    build_task_plan, build_workflow_generated_artifact, load_learning_bus,
    persist_clarification_session_artifact, persist_consultation_artifact,
    persist_execution_decision, persist_governance_policy,
    persist_primary_secondary_failover_artifact, persist_primary_secondary_policy_artifact,
    persist_requirement_contract, persist_task_plan, persist_workflow_generated,
    persist_workflow_research, recommend_agent_order_from_execution_history,
    recommend_failure_strategy_from_learning_bus, recommend_parallelism_from_learning_bus,
    recommend_predicted_success_rate_from_learning_bus, recommend_work_grade_from_learning_bus,
    ClarificationSessionArtifact, ConsultationArtifact, ExecutionAssignmentRecord,
    ExecutionDecisionArtifact, ExecutionDecisionCandidate, GovernancePolicyArtifact,
    ParallelPhaseDecisionRecord, PlannedSubtaskRecord, PrimaryFailoverReportItem,
    PrimarySecondaryFailoverArtifact, PrimarySecondaryPolicyArtifact, RequirementContractArtifact,
    TaskExecutionMetrics, TaskExecutionSummary, TaskPlanArtifact, WorkflowEdge,
    WorkflowGeneratedArtifact, WorkflowNode, WorkflowResearchArtifact,
};
