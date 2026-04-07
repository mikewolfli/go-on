//! BLUE2 reinforcement utilities.
//!
//! This module provides a focused implementation of the BLUE2 plan without
//! imposing whole-repo overreach. It adds:
//! - durable `.goon/` artifacts
//! - runtime healthchecks
//! - executable action checks
//! - controlled task planning artifacts for sub-agent style decomposition

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::cache::ResponseCache;
use crate::config::{validate_runtime_readiness, AppConfig};
use crate::task_decomposer::{TaskDecomposer, TaskDecomposition};
use crate::task_router::{RoutingDecision, TaskCharacteristics, TaskRouter};
use crate::vector::VectorStore;

const GOON_DIR: &str = ".goon";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Healthy,
    Warn,
    Error,
    Skipped,
}

impl CheckStatus {
    fn severity(self) -> u8 {
        match self {
            Self::Healthy => 0,
            Self::Skipped => 0,
            Self::Warn => 1,
            Self::Error => 2,
        }
    }

    fn merge(self, other: Self) -> Self {
        if self.severity() >= other.severity() {
            self
        } else {
            other
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentReport {
    pub name: String,
    pub status: CheckStatus,
    pub message: String,
    pub details: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeHealthcheckReport {
    pub generated_at: i64,
    pub overall_status: CheckStatus,
    pub components: Vec<ComponentReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointSummaryArtifact {
    pub checkpoint_id: String,
    pub conversation_id: String,
    pub branch_id: String,
    pub parent_checkpoint_id: Option<String>,
    pub created_at: i64,
    pub note: Option<String>,
    pub message_count: usize,
    pub message_chars: usize,
    pub assistant_excerpt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedSubtaskRecord {
    pub id: String,
    pub description: String,
    pub status: String,
    pub phase_index: usize,
    pub retry_count: u32,
    /// Unix timestamp when subtask execution started (None = not yet executed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_ts: Option<i64>,
    /// Unix timestamp when subtask execution stopped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_ts: Option<i64>,
    /// Wall-clock execution time in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// e.g. "completed", "failed", "skipped"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// Agent/role name that executed this subtask.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executor: Option<String>,
}

impl PlannedSubtaskRecord {
    /// Mark this record as executed with full lifecycle telemetry (Section 5).
    pub fn mark_executed(
        &mut self,
        start_ts: i64,
        stop_ts: i64,
        duration_ms: u64,
        outcome: impl Into<String>,
        executor: impl Into<String>,
    ) {
        self.start_ts = Some(start_ts);
        self.stop_ts = Some(stop_ts);
        self.duration_ms = Some(duration_ms);
        let outcome = outcome.into();
        self.status = outcome.clone();
        self.outcome = Some(outcome);
        self.executor = Some(executor.into());
    }
}


/// Aggregate result of executing a `TaskPlanArtifact` via `task.execute`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskExecutionSummary {
    pub generated_at: i64,
    pub task: String,
    pub subtasks_total: usize,
    pub subtasks_completed: usize,
    pub subtasks_failed: usize,
    pub subtasks_skipped: usize,
    pub executor: String,
    pub records: Vec<PlannedSubtaskRecord>,
    pub artifact_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPlanArtifact {
    pub generated_at: i64,
    pub task: String,
    pub characteristics: TaskCharacteristics,
    pub routing: RoutingDecision,
    pub decomposition: Option<TaskDecomposition>,
    pub planned_subtasks: Vec<PlannedSubtaskRecord>,
    pub sub_agent_recommended: bool,
    pub activation_reasons: Vec<String>,
    pub action_checks_required: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionCheckKind {
    All,
    Spec,
    Qa,
    Retest,
    Final,
}

impl ActionCheckKind {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "all" => Some(Self::All),
            "spec" => Some(Self::Spec),
            "qa" => Some(Self::Qa),
            "retest" => Some(Self::Retest),
            "final" => Some(Self::Final),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Spec => "spec",
            Self::Qa => "qa",
            Self::Retest => "retest",
            Self::Final => "final",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionCheckItem {
    pub name: String,
    pub status: CheckStatus,
    pub message: String,
    pub artifact_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionCheckReport {
    pub generated_at: i64,
    pub kind: String,
    pub overall_status: CheckStatus,
    pub ok: bool,
    pub checks_run: Vec<ActionCheckItem>,
    pub evidence_refs: Vec<String>,
    pub retest_report_path: Option<String>,
    pub final_summary_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinalSummaryArtifact {
    pub generated_at: i64,
    pub overall_status: CheckStatus,
    pub evidence_refs: Vec<String>,
    pub action_check_path: String,
    pub conclusion: String,
}

#[derive(Debug, Clone)]
pub struct ArtifactLedger {
    root: PathBuf,
}

impl ArtifactLedger {
    pub fn new(config_path: Option<&Path>) -> Self {
        let root = config_path
            .and_then(|path| path.parent().map(|parent| parent.join(GOON_DIR)))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(GOON_DIR));
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

        fs::write(&archive_path, &encoded)
            .with_context(|| format!("failed to write ledger artifact {}", archive_path.display()))?;
        fs::write(&latest_path, &encoded)
            .with_context(|| format!("failed to write latest ledger artifact {}", latest_path.display()))?;

        Ok(latest_path)
    }
}

pub fn build_task_plan(task: &str) -> TaskPlanArtifact {
    let characteristics = TaskRouter::analyze_task(task);
    let routing = TaskRouter::route_task(&characteristics);
    let should_decompose = characteristics.complexity >= 3
        || routing.roles.len() >= 2
        || characteristics.involves_multiple_modules;
    let decomposition = should_decompose.then(|| TaskDecomposer::decompose(&characteristics));

    let planned_subtasks = decomposition
        .as_ref()
        .map(planned_subtask_records)
        .unwrap_or_default();

    let mut activation_reasons = Vec::new();
    if characteristics.complexity >= 4 {
        activation_reasons.push("complexity>=4".to_string());
    }
    if routing.roles.len() >= 3 {
        activation_reasons.push("multi_role_execution".to_string());
    }
    if !routing.can_parallelize.is_empty() {
        activation_reasons.push("parallelizable_roles_detected".to_string());
    }
    if characteristics.involves_multiple_modules {
        activation_reasons.push("cross_module_task".to_string());
    }

    TaskPlanArtifact {
        generated_at: now_ts(),
        task: task.to_string(),
        characteristics,
        routing,
        decomposition,
        planned_subtasks,
        sub_agent_recommended: !activation_reasons.is_empty(),
        activation_reasons,
        action_checks_required: vec![
            "spec".to_string(),
            "qa".to_string(),
            "retest".to_string(),
            "final".to_string(),
        ],
    }
}

pub fn persist_task_plan(ledger: &ArtifactLedger, plan: &TaskPlanArtifact) -> Result<PathBuf> {
    ledger.write_json("spec", "latest-plan.json", plan)
}

/// Persist the execution summary produced by `task.execute` to the durable ledger.
pub fn persist_task_execution_summary(
    ledger: &ArtifactLedger,
    summary: &TaskExecutionSummary,
) -> Result<PathBuf> {
    ledger.write_json("spec", "latest-execution.json", summary)
}

pub fn build_runtime_healthcheck_report(
    config_path: Option<&Path>,
    cache: Option<&ResponseCache>,
    vector_store: Option<&VectorStore>,
) -> Result<RuntimeHealthcheckReport> {
    let ledger = ArtifactLedger::new(config_path);
    let mut components = Vec::new();

    match ledger.ensure_ready() {
        Ok(()) => components.push(ComponentReport {
            name: "ledger".to_string(),
            status: CheckStatus::Healthy,
            message: "durable ledger is writable".to_string(),
            details: json!({ "root": ledger.root().display().to_string() }),
        }),
        Err(err) => components.push(ComponentReport {
            name: "ledger".to_string(),
            status: CheckStatus::Error,
            message: err.to_string(),
            details: json!({ "root": ledger.root().display().to_string() }),
        }),
    }

    if let Some(path) = config_path {
        match AppConfig::load(path).and_then(|config| validate_runtime_readiness(path, &config)) {
            Ok(report) => {
                let status = if report.critical_count > 0 {
                    CheckStatus::Error
                } else if report.warn_count > 0 || report.info_count > 0 {
                    CheckStatus::Warn
                } else {
                    CheckStatus::Healthy
                };
                components.push(ComponentReport {
                    name: "config".to_string(),
                    status,
                    message: format!(
                        "config score {}/100, profile {}",
                        report.score, report.profile_recommendation
                    ),
                    details: serde_json::to_value(&report).unwrap_or_else(|_| json!({})),
                });
            }
            Err(err) => components.push(ComponentReport {
                name: "config".to_string(),
                status: CheckStatus::Error,
                message: err.to_string(),
                details: json!({ "config_path": path.display().to_string() }),
            }),
        }
    } else {
        components.push(ComponentReport {
            name: "config".to_string(),
            status: CheckStatus::Skipped,
            message: "config path unavailable".to_string(),
            details: json!({}),
        });
    }

    if let Some(cache) = cache {
        match cache.entry_count() {
            Ok(entries) => components.push(ComponentReport {
                name: "cache".to_string(),
                status: CheckStatus::Healthy,
                message: format!("sqlite cache reachable with {} entries", entries),
                details: json!({ "entries": entries }),
            }),
            Err(err) => components.push(ComponentReport {
                name: "cache".to_string(),
                status: CheckStatus::Error,
                message: err.to_string(),
                details: json!({}),
            }),
        }
    } else {
        components.push(ComponentReport {
            name: "cache".to_string(),
            status: CheckStatus::Skipped,
            message: "cache disabled".to_string(),
            details: json!({}),
        });
    }

    if let Some(vector_store) = vector_store {
        match (vector_store.memory_entry_count(), vector_store.summary_entry_count()) {
            (Ok(memory_entries), Ok(summary_entries)) => components.push(ComponentReport {
                name: "vector".to_string(),
                status: CheckStatus::Healthy,
                message: format!(
                    "vector store reachable with {} memory entries and {} summaries",
                    memory_entries, summary_entries
                ),
                details: json!({
                    "memory_entries": memory_entries,
                    "summary_entries": summary_entries,
                }),
            }),
            (Err(err), _) | (_, Err(err)) => components.push(ComponentReport {
                name: "vector".to_string(),
                status: CheckStatus::Error,
                message: err.to_string(),
                details: json!({}),
            }),
        }
    } else {
        components.push(ComponentReport {
            name: "vector".to_string(),
            status: CheckStatus::Skipped,
            message: "vector store disabled".to_string(),
            details: json!({}),
        });
    }

    let overall_status = aggregate_status(components.iter().map(|component| component.status));
    Ok(RuntimeHealthcheckReport {
        generated_at: now_ts(),
        overall_status,
        components,
    })
}

pub fn persist_runtime_healthcheck(
    ledger: &ArtifactLedger,
    report: &RuntimeHealthcheckReport,
) -> Result<PathBuf> {
    ledger.write_json("qa", "latest-healthcheck.json", report)
}

pub fn run_action_check(
    ledger: &ArtifactLedger,
    kind: ActionCheckKind,
) -> Result<ActionCheckReport> {
    ledger.ensure_ready()?;
    let generated_at = now_ts();
    let mut checks_run = Vec::new();
    let mut evidence_refs = Vec::new();
    let mut overall_status = CheckStatus::Healthy;
    let mut retest_report_path = None;
    let mut final_summary_path = None;

    let include_spec = matches!(
        kind,
        ActionCheckKind::All | ActionCheckKind::Spec | ActionCheckKind::Retest
    );
    let include_qa = matches!(
        kind,
        ActionCheckKind::All | ActionCheckKind::Qa | ActionCheckKind::Retest
    );
    let include_checkpoint = matches!(kind, ActionCheckKind::All | ActionCheckKind::Retest);

    if include_spec {
        let item = check_json_artifact(
            ledger,
            "spec",
            "latest-plan.json",
            "spec_plan",
            &["task", "characteristics", "routing", "planned_subtasks"],
        );
        maybe_capture_evidence(&item, &mut evidence_refs);
        overall_status = overall_status.merge(item.status);
        checks_run.push(item);
    }

    if include_qa {
        let item = check_json_artifact(
            ledger,
            "qa",
            "latest-healthcheck.json",
            "qa_healthcheck",
            &["generated_at", "overall_status", "components"],
        );
        maybe_capture_evidence(&item, &mut evidence_refs);
        overall_status = overall_status.merge(item.status);
        checks_run.push(item);
    }

    if include_checkpoint {
        let item = check_json_artifact(
            ledger,
            "checkpoints",
            "latest.json",
            "recovery_checkpoint",
            &[
                "checkpoint_id",
                "conversation_id",
                "branch_id",
                "message_count",
            ],
        );
        maybe_capture_evidence(&item, &mut evidence_refs);
        overall_status = overall_status.merge(item.status);
        checks_run.push(item);
    }

    if matches!(kind, ActionCheckKind::All | ActionCheckKind::Retest) {
        let retest_report = ActionCheckReport {
            generated_at,
            kind: "retest".to_string(),
            overall_status,
            ok: overall_status != CheckStatus::Error,
            checks_run: checks_run.clone(),
            evidence_refs: evidence_refs.clone(),
            retest_report_path: None,
            final_summary_path: None,
        };
        let path = ledger.write_json("retest", "latest-action-check.json", &retest_report)?;
        retest_report_path = Some(path.display().to_string());
        evidence_refs.push(path.display().to_string());
    }

    if matches!(kind, ActionCheckKind::All | ActionCheckKind::Final) {
        let retest_item = check_json_artifact(
            ledger,
            "retest",
            "latest-action-check.json",
            "retest_report",
            &["generated_at", "checks_run", "evidence_refs", "ok"],
        );
        if kind == ActionCheckKind::Final {
            maybe_capture_evidence(&retest_item, &mut evidence_refs);
            overall_status = overall_status.merge(retest_item.status);
            checks_run.push(retest_item);
        }

        let summary = FinalSummaryArtifact {
            generated_at,
            overall_status,
            evidence_refs: evidence_refs.clone(),
            action_check_path: ledger
                .latest_path("retest", "latest-action-check.json")
                .display()
                .to_string(),
            conclusion: if overall_status == CheckStatus::Error {
                "final evidence chain is incomplete; fix failing checks before promotion"
                    .to_string()
            } else {
                "final evidence chain is complete enough for controlled promotion"
                    .to_string()
            },
        };
        let path = ledger.write_json("final", "latest-summary.json", &summary)?;
        final_summary_path = Some(path.display().to_string());
        evidence_refs.push(path.display().to_string());
    }

    let report = ActionCheckReport {
        generated_at,
        kind: kind.as_str().to_string(),
        overall_status,
        ok: overall_status != CheckStatus::Error,
        checks_run,
        evidence_refs,
        retest_report_path,
        final_summary_path,
    };

    let latest_name = format!("latest-{}.json", kind.as_str());
    let _ = ledger.write_json("action-checks", latest_name.as_str(), &report)?;
    Ok(report)
}

fn maybe_capture_evidence(item: &ActionCheckItem, evidence_refs: &mut Vec<String>) {
    if item.status != CheckStatus::Error {
        if let Some(path) = &item.artifact_path {
            evidence_refs.push(path.clone());
        }
    }
}

fn check_json_artifact(
    ledger: &ArtifactLedger,
    category: &str,
    latest_name: &str,
    label: &str,
    required_keys: &[&str],
) -> ActionCheckItem {
    let path = ledger.latest_path(category, latest_name);
    let artifact_path = path.display().to_string();

    let value = match fs::read_to_string(&path) {
        Ok(content) => match serde_json::from_str::<Value>(&content) {
            Ok(value) => value,
            Err(err) => {
                return ActionCheckItem {
                    name: label.to_string(),
                    status: CheckStatus::Error,
                    message: format!("artifact is not valid JSON: {}", err),
                    artifact_path: Some(artifact_path),
                }
            }
        },
        Err(err) => {
            return ActionCheckItem {
                name: label.to_string(),
                status: CheckStatus::Error,
                message: format!("artifact missing: {}", err),
                artifact_path: Some(artifact_path),
            }
        }
    };

    let missing = required_keys
        .iter()
        .filter(|key| value.get(**key).is_none())
        .map(|key| (*key).to_string())
        .collect::<Vec<_>>();

    if missing.is_empty() {
        ActionCheckItem {
            name: label.to_string(),
            status: CheckStatus::Healthy,
            message: "artifact structure verified".to_string(),
            artifact_path: Some(artifact_path),
        }
    } else {
        ActionCheckItem {
            name: label.to_string(),
            status: CheckStatus::Error,
            message: format!("artifact missing keys: {}", missing.join(", ")),
            artifact_path: Some(artifact_path),
        }
    }
}

fn planned_subtask_records(decomposition: &TaskDecomposition) -> Vec<PlannedSubtaskRecord> {
    decomposition
        .execution_phases
        .iter()
        .enumerate()
        .flat_map(|(phase_index, subtask_ids)| {
            subtask_ids.iter().filter_map(move |subtask_id| {
                decomposition
                    .subtasks
                    .iter()
                    .find(|subtask| subtask.id == *subtask_id)
                    .map(|subtask| PlannedSubtaskRecord {
                        id: subtask.id.clone(),
                        description: subtask.description.clone(),
                        status: "planned".to_string(),
                        phase_index,
                        retry_count: 0,
                        start_ts: None,
                        stop_ts: None,
                        duration_ms: None,
                        outcome: None,
                        executor: None,
                    })
            })
        })
        .collect()
}

pub fn aggregate_status<I>(statuses: I) -> CheckStatus
where
    I: IntoIterator<Item = CheckStatus>,
{
    statuses
        .into_iter()
        .fold(CheckStatus::Healthy, |acc, status| acc.merge(status))
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn assistant_excerpt(messages: &[crate::agent::Message]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|message| message.role == "assistant")
        .map(|message| trim_chars(&message.content, 240))
}

pub fn total_message_chars(messages: &[crate::agent::Message]) -> usize {
    messages.iter().map(|message| message.content.chars().count()).sum()
}

fn trim_chars(text: &str, max_chars: usize) -> String {
    let mut result = text.chars().take(max_chars).collect::<String>();
    if text.chars().count() > max_chars {
        result.push_str("...");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::tempdir;

    #[test]
    fn task_plan_recommends_sub_agents_for_complex_work() {
        let plan = build_task_plan(
            "design a complex multi-module feature with verification and review",
        );
        assert!(plan.sub_agent_recommended);
        assert!(!plan.planned_subtasks.is_empty());
    }

    #[test]
    fn healthcheck_persists_to_ledger() {
        let temp = tempdir().expect("tempdir should be created");
        let config_path = temp.path().join("config.toml");
        fs::write(
            &config_path,
            r#"
default_phase = "coding"
model_selection_mode = "manual"

[flow]
phases = ["coding"]

[agents.copilot]
provider = "copilot"
api_key = "env://COPILOT_API_KEY"
model = "gpt-4"

[phases.coding]
description = "coding phase"
agents = ["copilot"]
fallback = true
"#,
        )
        .expect("config file should be written");

        let ledger = ArtifactLedger::new(Some(&config_path));
        let report = build_runtime_healthcheck_report(Some(&config_path), None, None)
            .expect("healthcheck should build");
        let path = persist_runtime_healthcheck(&ledger, &report)
            .expect("healthcheck should persist");
        assert!(path.exists());
    }

    #[test]
    fn action_check_generates_retest_and_final_artifacts() {
        let temp = tempdir().expect("tempdir should be created");
        let config_path = temp.path().join("config.toml");
        let ledger = ArtifactLedger::new(Some(&config_path));

        let plan = build_task_plan("fix complex bug across multiple modules with tests");
        persist_task_plan(&ledger, &plan).expect("task plan should persist");

        let health = RuntimeHealthcheckReport {
            generated_at: now_ts(),
            overall_status: CheckStatus::Healthy,
            components: vec![],
        };
        persist_runtime_healthcheck(&ledger, &health)
            .expect("healthcheck should persist");

        let checkpoint = CheckpointSummaryArtifact {
            checkpoint_id: "cp-1".to_string(),
            conversation_id: "conv-1".to_string(),
            branch_id: "main".to_string(),
            parent_checkpoint_id: None,
            created_at: now_ts(),
            note: Some("coding/copilot".to_string()),
            message_count: 2,
            message_chars: 12,
            assistant_excerpt: Some("hello".to_string()),
        };
        ledger
            .write_json("checkpoints", "latest.json", &checkpoint)
            .expect("checkpoint should persist");

        let report = run_action_check(&ledger, ActionCheckKind::All)
            .expect("action check should succeed");
        assert!(report.ok);
        assert!(ledger.latest_path("retest", "latest-action-check.json").exists());
        assert!(ledger.latest_path("final", "latest-summary.json").exists());
    }
}