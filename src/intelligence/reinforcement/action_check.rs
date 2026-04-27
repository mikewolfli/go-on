//! Action check module — spec/QA/retest/final verification gates.
//!
//! Extracted from the original monolithic `reinforcement.rs`.

use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::ArtifactLedger;
use super::health::CheckStatus;

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

    pub fn as_str(self) -> &'static str {
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

/// Run an action check of the specified kind against the artifact ledger.
///
/// Validates that the required artifacts exist and are well-formed according
/// to expected fields. Generates retest and final summary artifacts as needed.
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
                "final evidence chain is complete enough for controlled promotion".to_string()
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
    expected_fields: &[&str],
) -> ActionCheckItem {
    let path = ledger.latest_path(category, latest_name);
    if !path.exists() {
        return ActionCheckItem {
            name: label.to_string(),
            status: CheckStatus::Error,
            message: format!("{category}/{latest_name} not found"),
            artifact_path: None,
        };
    }

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            return ActionCheckItem {
                name: label.to_string(),
                status: CheckStatus::Error,
                message: format!("cannot read {category}/{latest_name}: {e}"),
                artifact_path: Some(path.display().to_string()),
            };
        }
    };

    let parsed: Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            return ActionCheckItem {
                name: label.to_string(),
                status: CheckStatus::Error,
                message: format!("invalid JSON in {category}/{latest_name}: {e}"),
                artifact_path: Some(path.display().to_string()),
            };
        }
    };

    let missing: Vec<&str> = expected_fields
        .iter()
        .filter(|field| !parsed.get(field.to_string()).map_or(false, |v| !v.is_null()))
        .copied()
        .collect();

    if missing.is_empty() {
        ActionCheckItem {
            name: label.to_string(),
            status: CheckStatus::Healthy,
            message: format!("{category}/{latest_name} valid"),
            artifact_path: Some(path.display().to_string()),
        }
    } else {
        ActionCheckItem {
            name: label.to_string(),
            status: CheckStatus::Warn,
            message: format!(
                "{category}/{latest_name} missing fields: {}",
                missing.join(", ")
            ),
            artifact_path: Some(path.display().to_string()),
        }
    }
}

fn now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
```Now let's create `task_plan.rs`:
