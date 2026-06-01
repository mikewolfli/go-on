//! Pua — F-GAP-20
//!
//! PUA enforcement model shared across routing, execution, verification, and review.

use crate::i18n::tf;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};

use crate::roles::AgentRole;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PuaStageRequirement {
    pub stage: String,
    pub required_actions: Vec<String>,
    pub hard_fail_conditions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PuaEnforcementPlan {
    pub escalation_level: String,
    pub mandatory_roles: Vec<AgentRole>,
    pub red_lines: Vec<String>,
    pub quality_compass: Vec<String>,
    pub mandatory_safeguards: Vec<String>,
    pub mandatory_evidence: Vec<String>,
    pub stage_requirements: Vec<PuaStageRequirement>,
}

impl Default for PuaEnforcementPlan {
    fn default() -> Self {
        build_enforcement_plan("Default PUA enforcement plan", 1, false, false, false)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PuaExecutionReport {
    pub stage: String,
    pub status: String,
    pub escalation_level: String,
    pub required_evidence: Vec<String>,
    pub completed_checks: Vec<String>,
    pub missing_checks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum PuaViolationKind {
    RedLine,
    StageFail,
    MissingEvidence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PuaViolation {
    pub kind: PuaViolationKind,
    pub detail: String,
}

impl std::fmt::Display for PuaViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            tf(
                "error.pua.violation",
                &[
                    ("kind", &format!("{:?}", self.kind)),
                    ("detail", &self.detail)
                ]
            )
        )
    }
}

impl std::error::Error for PuaViolation {}

#[derive(Debug)]
pub struct PuaRuleEngine {
    plan: Arc<StdMutex<PuaEnforcementPlan>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TaskType {
    BugFix,
    FeatureAdd,
    Refactor,
    SecurityPatch,
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum QualityCategory {
    Safety,
    Correctness,
    Performance,
    Style,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum VerificationMethod {
    AutoTest,
    ManualReview,
    StaticAnalysis,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskContext {
    pub task_type: TaskType,
    pub file_count: usize,
    pub risk_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QualityCheck {
    pub id: String,
    pub description: String,
    pub category: QualityCategory,
    pub verification: VerificationMethod,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContextRule {
    pub id: String,
    pub task_type: TaskType,
    pub min_risk_score: f64,
    pub min_file_count: usize,
    pub check: QualityCheck,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynamicQualityCompass {
    pub base_checks: Vec<QualityCheck>,
    pub context_rules: Vec<ContextRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PuaLearningRecord {
    pub stage: String,
    pub passed: bool,
    pub missing_checks: Vec<String>,
    pub escalation_level: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum LearningRecord {
    Workflow(serde_json::Value),
    Pua(PuaLearningRecord),
}

pub const LEARNING_RECORDS_FILE: &str = "learning-records.ndjson";

pub fn append_learning_record(storage_dir: &Path, record: &LearningRecord) -> std::io::Result<()> {
    fs::create_dir_all(storage_dir)?;
    let file_path = storage_dir.join(LEARNING_RECORDS_FILE);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(file_path)?;
    let line = serde_json::to_string(record)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string()))?;
    writeln!(file, "{}", line)
}

pub fn load_learning_records(
    storage_dir: &Path,
    limit: usize,
) -> std::io::Result<Vec<LearningRecord>> {
    if limit == 0 {
        return Ok(Vec::new());
    }

    let file_path = storage_dir.join(LEARNING_RECORDS_FILE);
    let content = match fs::read_to_string(&file_path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut records = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let record: LearningRecord = serde_json::from_str(trimmed)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string()))?;
        records.push(record);
    }

    if records.len() > limit {
        Ok(records.split_off(records.len() - limit))
    } else {
        Ok(records)
    }
}

#[derive(Debug, Clone)]
pub struct PuaFeedbackCollector {
    storage_path: PathBuf,
}

impl PuaFeedbackCollector {
    pub fn new(storage_path: PathBuf) -> Self {
        Self { storage_path }
    }

    pub fn collect(&self, report: &PuaExecutionReport) -> std::io::Result<()> {
        let record = PuaLearningRecord {
            stage: report.stage.clone(),
            passed: report.status.eq_ignore_ascii_case("pass")
                || report.status.eq_ignore_ascii_case("enforced"),
            missing_checks: report.missing_checks.clone(),
            escalation_level: parse_escalation_level(&report.escalation_level),
        };
        append_learning_record(&self.storage_path, &LearningRecord::Pua(record))
    }

    pub fn extract_learning_data(&self, limit: usize) -> std::io::Result<Vec<PuaLearningRecord>> {
        let mut records = Vec::new();

        for record in load_learning_records(&self.storage_path, limit.saturating_mul(4))? {
            if let LearningRecord::Pua(pua) = record {
                records.push(pua);
            }
        }

        if records.len() > limit {
            Ok(records.split_off(records.len() - limit))
        } else {
            Ok(records)
        }
    }
}

impl Default for DynamicQualityCompass {
    fn default() -> Self {
        Self {
            base_checks: vec![
                QualityCheck {
                    id: "build-proof".to_string(),
                    description: "Build proof captured".to_string(),
                    category: QualityCategory::Correctness,
                    verification: VerificationMethod::AutoTest,
                    required: true,
                },
                QualityCheck {
                    id: "error-case".to_string(),
                    description: "Error cases tested".to_string(),
                    category: QualityCategory::Correctness,
                    verification: VerificationMethod::AutoTest,
                    required: true,
                },
                QualityCheck {
                    id: "pattern-scan".to_string(),
                    description: "Pattern scan completed".to_string(),
                    category: QualityCategory::Style,
                    verification: VerificationMethod::ManualReview,
                    required: true,
                },
            ],
            context_rules: vec![
                ContextRule {
                    id: "security-threat-model".to_string(),
                    task_type: TaskType::SecurityPatch,
                    min_risk_score: 0.0,
                    min_file_count: 0,
                    check: QualityCheck {
                        id: "security-threat-model".to_string(),
                        description: "Threat model reviewed for security patch".to_string(),
                        category: QualityCategory::Safety,
                        verification: VerificationMethod::StaticAnalysis,
                        required: true,
                    },
                },
                ContextRule {
                    id: "multi-file-regression".to_string(),
                    task_type: TaskType::FeatureAdd,
                    min_risk_score: 0.5,
                    min_file_count: 3,
                    check: QualityCheck {
                        id: "multi-file-regression".to_string(),
                        description: "Multi-file regression checks executed".to_string(),
                        category: QualityCategory::Performance,
                        verification: VerificationMethod::AutoTest,
                        required: true,
                    },
                },
            ],
        }
    }
}

impl DynamicQualityCompass {
    pub fn get_checks(&self, context: &TaskContext) -> Vec<QualityCheck> {
        let mut checks = self.base_checks.clone();

        for rule in &self.context_rules {
            if rule.task_type == context.task_type
                && context.risk_score >= rule.min_risk_score
                && context.file_count >= rule.min_file_count
                && !checks.iter().any(|existing| existing.id == rule.check.id)
            {
                checks.push(rule.check.clone());
            }
        }

        if context.risk_score >= 0.8 && !checks.iter().any(|check| check.id == "high-risk-review") {
            checks.push(QualityCheck {
                id: "high-risk-review".to_string(),
                description: "High-risk changes require reviewer sign-off".to_string(),
                category: QualityCategory::Safety,
                verification: VerificationMethod::ManualReview,
                required: true,
            });
        }

        checks
    }

    pub fn quality_compass_compat(&self) -> Vec<String> {
        quality_compass()
    }
}

impl PuaRuleEngine {
    pub fn new(plan: Arc<StdMutex<PuaEnforcementPlan>>) -> Self {
        Self { plan }
    }

    /// Evaluate approval feedback from the ApprovalEngine.
    /// This allows the PUA rule engine to adjust enforcement plans based on
    /// approval outcomes (e.g., auto-deny patterns, escalation frequency).
    pub fn evaluate_approval_feedback(
        &self,
        request: &super::approval_engine::ApprovalRequest,
    ) -> Result<(), PuaViolation> {
        use super::approval_engine::ApprovalStatus;
        match &request.status {
            ApprovalStatus::AutoDenied { reason, .. } => {
                // Auto-deny is a strong signal — escalate the PUA level
                self.escalate(&format!("auto-deny for {}: {}", request.action, reason));
                Ok(())
            }
            ApprovalStatus::Rejected { reason, .. } => {
                // Rejection triggers a moderate escalation
                self.escalate(&format!("rejected {}: {}", request.action, reason));
                Ok(())
            }
            ApprovalStatus::Approved { .. } => {
                // Approval may allow de-escalation if the action completed safely
                self.de_escalate(&format!("approved {}", request.action));
                Ok(())
            }
            _ => Ok(()),
        }
    }

    pub fn check_red_lines(&self, action: &str) -> Result<(), PuaViolation> {
        let plan = self.plan.lock().map_err(|_| PuaViolation {
            kind: PuaViolationKind::MissingEvidence,
            detail: "failed to lock PUA plan".to_string(),
        })?;
        if plan
            .red_lines
            .iter()
            .any(|line| line.eq_ignore_ascii_case(action))
        {
            return Err(PuaViolation {
                kind: PuaViolationKind::RedLine,
                detail: format!("action '{}' matches a PUA red line", action),
            });
        }
        Ok(())
    }

    pub fn validate_stage(&self, stage: &str, completed: &[String]) -> Result<(), PuaViolation> {
        let plan = self.plan.lock().map_err(|_| PuaViolation {
            kind: PuaViolationKind::MissingEvidence,
            detail: "failed to lock PUA plan".to_string(),
        })?;

        let requirement = match plan
            .stage_requirements
            .iter()
            .find(|req| req.stage.eq_ignore_ascii_case(stage))
        {
            Some(req) => req,
            None => return Ok(()),
        };

        if let Some(triggered) = requirement
            .hard_fail_conditions
            .iter()
            .find(|cond| completed.iter().any(|item| item.eq_ignore_ascii_case(cond)))
        {
            return Err(PuaViolation {
                kind: PuaViolationKind::RedLine,
                detail: format!(
                    "stage '{}' triggered hard-fail condition '{}'",
                    requirement.stage, triggered
                ),
            });
        }

        let missing = requirement
            .required_actions
            .iter()
            .filter(|required| {
                !completed
                    .iter()
                    .any(|item| item.eq_ignore_ascii_case(required))
            })
            .cloned()
            .collect::<Vec<_>>();

        if missing.is_empty() {
            Ok(())
        } else {
            Err(PuaViolation {
                kind: PuaViolationKind::StageFail,
                detail: format!(
                    "stage '{}' missing required actions: {}",
                    requirement.stage,
                    missing.join(", ")
                ),
            })
        }
    }

    pub fn collect_evidence(&self, stage: &str) -> Vec<String> {
        self.plan
            .lock()
            .ok()
            .and_then(|plan| {
                plan.stage_requirements
                    .iter()
                    .find(|req| req.stage.eq_ignore_ascii_case(stage))
                    .map(|req| req.required_actions.clone())
            })
            .unwrap_or_default()
    }

    pub fn collect_missing(&self, stage: &str, completed: &[String]) -> Vec<String> {
        self.collect_evidence(stage)
            .into_iter()
            .filter(|required| {
                !completed
                    .iter()
                    .any(|item| item.eq_ignore_ascii_case(required))
            })
            .collect()
    }

    pub fn generate_report(&self, stage: &str, completed: &[String]) -> PuaExecutionReport {
        let missing_checks = self.collect_missing(stage, completed);
        let escalation_level = self
            .plan
            .lock()
            .ok()
            .map(|plan| plan.escalation_level.clone())
            .unwrap_or_else(|| "L0".to_string());

        PuaExecutionReport {
            stage: stage.to_string(),
            status: if missing_checks.is_empty() {
                "pass".to_string()
            } else {
                "fail".to_string()
            },
            escalation_level,
            required_evidence: self.collect_evidence(stage),
            completed_checks: completed.to_vec(),
            missing_checks,
        }
    }

    pub fn escalate(&self, _reason: &str) -> u8 {
        tracing::debug!("Escalation triggered: {}", _reason);
        let mut plan = self.plan.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("PUA plan lock poisoned: recovering");
            poisoned.into_inner()
        });

        let current = parse_escalation_level(&plan.escalation_level);
        let next = current.saturating_add(1).min(5);
        plan.escalation_level = format!("L{}", next);
        next
    }

    /// De-escalate the security level by one step.
    ///
    /// Decreases the escalation level when threat conditions are resolved
    /// (e.g., after successful recovery from a security incident).
    /// The level is floored at L0 (no escalation).
    pub fn de_escalate(&self, _reason: &str) -> u8 {
        tracing::debug!("De-escalation triggered: {}", _reason);
        let mut plan = self.plan.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("PUA plan lock poisoned: recovering");
            poisoned.into_inner()
        });

        let current = parse_escalation_level(&plan.escalation_level);
        let next = current.saturating_sub(1);
        plan.escalation_level = format!("L{}", next);
        next
    }
}

pub fn quality_compass() -> Vec<String> {
    vec![
        "Build proof captured".to_string(),
        "Error cases tested".to_string(),
        "Pattern scan completed".to_string(),
        "Root cause explained".to_string(),
        "Quality delta stated".to_string(),
    ]
}

pub fn build_enforcement_plan(
    description: &str,
    complexity: u8,
    needs_verification: bool,
    has_safety_concerns: bool,
    involves_multiple_modules: bool,
) -> PuaEnforcementPlan {
    let lower = description.to_lowercase();
    let mut mandatory_roles = vec![AgentRole::Coder];
    if complexity >= 3 || involves_multiple_modules {
        mandatory_roles.push(AgentRole::Planner);
    }
    if needs_verification || lower.contains("test") || lower.contains("verify") {
        mandatory_roles.push(AgentRole::Tester);
    }
    if has_safety_concerns || complexity >= 4 || lower.contains("review") {
        mandatory_roles.push(AgentRole::Reviewer);
    }
    dedupe_roles(&mut mandatory_roles);

    let mut safeguards = vec![
        "Reject placeholder implementations and empty TODO-only branches".to_string(),
        "Require proof-producing verification before completion".to_string(),
        "Block speculative blame without repository evidence".to_string(),
    ];
    if has_safety_concerns {
        safeguards.push("Escalate safety-sensitive tasks to reviewer gate".to_string());
    }
    if involves_multiple_modules {
        safeguards.push("Scan neighboring modules for the same bug pattern".to_string());
    }
    if complexity >= 4 {
        safeguards.push("Force dual review before autonomous approval".to_string());
    }

    let escalation_level = if complexity >= 5 {
        "L3"
    } else if complexity >= 4 {
        "L2"
    } else if needs_verification || has_safety_concerns {
        "L1"
    } else {
        "L0"
    }
    .to_string();

    PuaEnforcementPlan {
        escalation_level,
        mandatory_roles,
        red_lines: vec![
            "Close the loop with executable proof before claiming completion".to_string(),
            "Verify facts before attributing failures to environment or dependencies".to_string(),
            "Exhaust alternative approaches before declaring a blocker".to_string(),
        ],
        quality_compass: quality_compass(),
        mandatory_safeguards: safeguards,
        mandatory_evidence: vec![
            "Observed build, test, or runtime output".to_string(),
            "Root-cause statement tied to concrete code or config".to_string(),
            "Pattern scan summary for similar failure classes".to_string(),
        ],
        stage_requirements: vec![
            PuaStageRequirement {
                stage: "intake".to_string(),
                required_actions: vec![
                    "Classify task risk, complexity, and verification need".to_string(),
                    "Decide the minimum agent roles required".to_string(),
                ],
                hard_fail_conditions: vec![
                    "Ambiguous task accepted without decomposition".to_string(),
                    "High-risk task routed without reviewer coverage".to_string(),
                ],
            },
            PuaStageRequirement {
                stage: "planning".to_string(),
                required_actions: vec![
                    "List proof obligations before implementation".to_string(),
                    "Define what invalidates a success claim".to_string(),
                ],
                hard_fail_conditions: vec![
                    "No verification path defined".to_string(),
                    "No fallback strategy for expected failure modes".to_string(),
                ],
            },
            PuaStageRequirement {
                stage: "execution".to_string(),
                required_actions: vec![
                    "Prefer root-cause fixes over cosmetic patches".to_string(),
                    "Record evidence for each substantive tool action".to_string(),
                ],
                hard_fail_conditions: vec![
                    "Placeholder or empty implementation introduced".to_string(),
                    "Destructive action executed without explicit gate".to_string(),
                ],
            },
            PuaStageRequirement {
                stage: "verification".to_string(),
                required_actions: vec![
                    "Run build or test proof whenever code changes".to_string(),
                    "Validate at least one failure path or edge case".to_string(),
                ],
                hard_fail_conditions: vec![
                    "Completion claimed without proof output".to_string(),
                    "Known verification failure ignored".to_string(),
                ],
            },
            PuaStageRequirement {
                stage: "delivery".to_string(),
                required_actions: vec![
                    "State root cause and prevention delta".to_string(),
                    "Disclose residual risk and missing proof".to_string(),
                ],
                hard_fail_conditions: vec![
                    "Success statement unsupported by evidence".to_string(),
                    "Open questions hidden from the operator".to_string(),
                ],
            },
        ],
    }
}

pub fn merge_phase_principles(
    existing: Option<Vec<String>>,
    phase_name: &str,
) -> Option<Vec<String>> {
    let mut principles = existing.unwrap_or_default();
    principles.extend(vec![
        "PUA red line: close the loop with build/test/runtime proof".to_string(),
        "PUA red line: verify facts before attributing blame".to_string(),
        "PUA red line: exhaust alternative approaches before escalation".to_string(),
    ]);

    match phase_name {
        "coding" => principles.extend(vec![
            "No TODO-only implementations, placeholders, or silent stubs".to_string(),
            "Fix the underlying cause and scan the module for the same pattern".to_string(),
        ]),
        "review" => principles.extend(vec![
            "Findings first; approval requires proof and root cause clarity".to_string(),
            "Reject changes that skip pattern scans or failure-path testing".to_string(),
        ]),
        "planning" => principles.extend(vec![
            "Plan must define verification gates and rollback conditions".to_string(),
        ]),
        _ => principles.push("Delivery must include quality-compass coverage".to_string()),
    }

    dedupe_strings(&mut principles);
    if principles.is_empty() {
        None
    } else {
        Some(principles)
    }
}

pub fn mode_execution_report(mode: &str, high_risk: bool) -> PuaExecutionReport {
    let mut missing_checks = vec![
        "build_proof".to_string(),
        "error_case_validation".to_string(),
        "pattern_scan".to_string(),
        "root_cause_summary".to_string(),
    ];
    let mut completed_checks = vec!["risk_classification".to_string()];
    if high_risk {
        completed_checks.push("high_risk_detected".to_string());
        missing_checks.push("operator_approval".to_string());
    }

    PuaExecutionReport {
        stage: format!("mode:{mode}"),
        status: if high_risk {
            "approval_required".to_string()
        } else {
            "enforced".to_string()
        },
        escalation_level: if high_risk { "L2" } else { "L1" }.to_string(),
        required_evidence: quality_compass(),
        completed_checks,
        missing_checks,
    }
}

pub fn tool_execution_report(tool_name: &str, verification: Option<&str>) -> PuaExecutionReport {
    let mut completed_checks = vec!["tool_audit_recorded".to_string()];
    let mut missing_checks = vec!["proof_linked_to_task".to_string()];
    if let Some(signal) = verification {
        completed_checks.push(format!("verification:{signal}"));
        missing_checks.retain(|item| item != "proof_linked_to_task");
    }

    PuaExecutionReport {
        stage: format!("tool:{tool_name}"),
        status: "enforced".to_string(),
        escalation_level: "L1".to_string(),
        required_evidence: vec![
            "Tool action recorded in audit trail".to_string(),
            "Verification signal emitted when tool changes state".to_string(),
        ],
        completed_checks,
        missing_checks,
    }
}

// Review-gate system prompt used by dual-review execution path.
pub fn review_gate_prompt() -> String {
    "Act as a strict execution approval gate. Reply with APPROVE or REJECT on the first line only. After the first line, evaluate the request against the PUA red lines and quality compass: build/test/runtime proof, fact-based reasoning, exhaustive attempts, pattern scan, root cause clarity, and quality improvement. Reject if any required proof is missing.".to_string()
}

fn dedupe_strings(values: &mut Vec<String>) {
    let mut deduped = Vec::new();
    for value in values.drain(..) {
        if !deduped.contains(&value) {
            deduped.push(value);
        }
    }
    *values = deduped;
}

fn dedupe_roles(values: &mut Vec<AgentRole>) {
    let mut deduped = Vec::new();
    for value in values.drain(..) {
        if !deduped.contains(&value) {
            deduped.push(value);
        }
    }
    *values = deduped;
}

fn parse_escalation_level(level: &str) -> u8 {
    let trimmed = level.trim();
    if let Some(digits) = trimmed.strip_prefix('L') {
        return digits.parse::<u8>().unwrap_or(0);
    }
    trimmed.parse::<u8>().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex as StdMutex};

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir).expect("temporary directory should be creatable");
        dir
    }

    #[test]
    fn builds_high_risk_plan() {
        let plan = build_enforcement_plan("Fix security issue across modules", 5, true, true, true);
        assert!(plan.mandatory_roles.contains(&AgentRole::Reviewer));
        assert!(plan.mandatory_roles.contains(&AgentRole::Tester));
        assert_eq!(plan.escalation_level, "L3");
    }

    #[test]
    fn merges_phase_principles_without_duplicates() {
        let merged = merge_phase_principles(
            Some(vec![
                "PUA red line: close the loop with build/test/runtime proof".to_string(),
            ]),
            "review",
        )
        .unwrap();
        assert_eq!(
            merged
                .iter()
                .filter(|item| item.contains("close the loop"))
                .count(),
            1
        );
    }

    #[test]
    fn pua_rule_engine_blocks_red_line_action() {
        let plan = PuaEnforcementPlan {
            escalation_level: "L1".to_string(),
            mandatory_roles: vec![],
            red_lines: vec!["dangerous.shell".to_string()],
            quality_compass: vec![],
            mandatory_safeguards: vec![],
            mandatory_evidence: vec![],
            stage_requirements: vec![],
        };
        let engine = PuaRuleEngine::new(Arc::new(StdMutex::new(plan)));
        let err = engine
            .check_red_lines("dangerous.shell")
            .expect_err("red line should be blocked");
        assert_eq!(err.kind, PuaViolationKind::RedLine);
    }

    #[test]
    fn pua_rule_engine_fails_stage_with_missing_required_action() {
        let plan = PuaEnforcementPlan {
            escalation_level: "L1".to_string(),
            mandatory_roles: vec![],
            red_lines: vec![],
            quality_compass: vec![],
            mandatory_safeguards: vec![],
            mandatory_evidence: vec![],
            stage_requirements: vec![PuaStageRequirement {
                stage: "execution".to_string(),
                required_actions: vec!["record_evidence".to_string()],
                hard_fail_conditions: vec![],
            }],
        };
        let engine = PuaRuleEngine::new(Arc::new(StdMutex::new(plan)));
        let err = engine
            .validate_stage("execution", &["other_action".to_string()])
            .expect_err("missing required action should fail");
        assert_eq!(err.kind, PuaViolationKind::StageFail);
    }

    #[test]
    fn pua_rule_engine_passes_when_all_conditions_met() {
        let plan = PuaEnforcementPlan {
            escalation_level: "L1".to_string(),
            mandatory_roles: vec![],
            red_lines: vec![],
            quality_compass: vec![],
            mandatory_safeguards: vec![],
            mandatory_evidence: vec![],
            stage_requirements: vec![PuaStageRequirement {
                stage: "execution".to_string(),
                required_actions: vec!["record_evidence".to_string()],
                hard_fail_conditions: vec!["forbidden_action".to_string()],
            }],
        };
        let engine = PuaRuleEngine::new(Arc::new(StdMutex::new(plan)));
        assert!(engine
            .validate_stage("execution", &["record_evidence".to_string()])
            .is_ok());
    }

    #[test]
    fn compass_adds_security_check_for_security_patch_task() {
        let compass = DynamicQualityCompass::default();
        let checks = compass.get_checks(&TaskContext {
            task_type: TaskType::SecurityPatch,
            file_count: 1,
            risk_score: 0.4,
        });

        assert!(checks
            .iter()
            .any(|check| check.category == QualityCategory::Safety));
    }

    #[test]
    fn compass_base_checks_always_present() {
        let compass = DynamicQualityCompass::default();
        let checks = compass.get_checks(&TaskContext {
            task_type: TaskType::Refactor,
            file_count: 1,
            risk_score: 0.1,
        });

        assert!(checks.iter().any(|check| check.id == "build-proof"));
        assert!(checks.iter().any(|check| check.id == "error-case"));
        assert!(checks.iter().any(|check| check.id == "pattern-scan"));
    }

    #[test]
    fn quality_compass_compat_returns_five_items() {
        let compass = DynamicQualityCompass::default();
        let compat = compass.quality_compass_compat();
        assert_eq!(compat.len(), 5);
    }

    #[test]
    fn pua_collector_writes_report_to_ndjson() {
        let dir = unique_temp_dir("goon-pua-collector-write");
        let collector = PuaFeedbackCollector::new(dir.clone());
        let report = PuaExecutionReport {
            stage: "execution".to_string(),
            status: "fail".to_string(),
            escalation_level: "L2".to_string(),
            required_evidence: vec!["proof".to_string()],
            completed_checks: vec!["check-a".to_string()],
            missing_checks: vec!["check-b".to_string()],
        };

        collector.collect(&report).expect("collector should write");
        let output = dir.join("learning-records.ndjson");
        assert!(output.exists(), "feedback ndjson should exist");

        let content = fs::read_to_string(output).expect("feedback file should be readable");
        let first_line = content
            .lines()
            .next()
            .expect("ndjson should contain one line");
        let restored: LearningRecord =
            serde_json::from_str(first_line).expect("line should decode as learning record");
        match restored {
            LearningRecord::Pua(record) => assert_eq!(record.stage, "execution"),
            LearningRecord::Workflow(_) => panic!("expected pua learning record variant"),
        }

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn pua_learning_data_extraction_returns_correct_records() {
        let dir = unique_temp_dir("goon-pua-collector-read");
        let collector = PuaFeedbackCollector::new(dir.clone());

        collector
            .collect(&PuaExecutionReport {
                stage: "planning".to_string(),
                status: "pass".to_string(),
                escalation_level: "L1".to_string(),
                required_evidence: vec![],
                completed_checks: vec!["a".to_string()],
                missing_checks: vec![],
            })
            .expect("first report should be written");

        collector
            .collect(&PuaExecutionReport {
                stage: "execution".to_string(),
                status: "fail".to_string(),
                escalation_level: "L3".to_string(),
                required_evidence: vec![],
                completed_checks: vec!["b".to_string()],
                missing_checks: vec!["c".to_string()],
            })
            .expect("second report should be written");

        let records = collector
            .extract_learning_data(5)
            .expect("learning records should be readable");
        assert_eq!(records.len(), 2);
        assert!(records[0].passed);
        assert!(!records[1].passed);
        assert_eq!(records[1].escalation_level, 3);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn pua_report_status_fail_when_missing_checks_present() {
        let plan = PuaEnforcementPlan {
            escalation_level: "L2".to_string(),
            mandatory_roles: vec![],
            red_lines: vec![],
            quality_compass: vec![],
            mandatory_safeguards: vec![],
            mandatory_evidence: vec![],
            stage_requirements: vec![PuaStageRequirement {
                stage: "verification".to_string(),
                required_actions: vec!["run_tests".to_string(), "validate_edge_case".to_string()],
                hard_fail_conditions: vec![],
            }],
        };
        let engine = PuaRuleEngine::new(Arc::new(StdMutex::new(plan)));
        let report = engine.generate_report("verification", &["run_tests".to_string()]);

        assert_eq!(report.status, "fail");
        assert_eq!(
            report.missing_checks,
            vec!["validate_edge_case".to_string()]
        );
    }

    #[test]
    fn pua_report_status_pass_when_all_checks_complete() {
        let plan = PuaEnforcementPlan {
            escalation_level: "L1".to_string(),
            mandatory_roles: vec![],
            red_lines: vec![],
            quality_compass: vec![],
            mandatory_safeguards: vec![],
            mandatory_evidence: vec![],
            stage_requirements: vec![PuaStageRequirement {
                stage: "verification".to_string(),
                required_actions: vec!["run_tests".to_string()],
                hard_fail_conditions: vec![],
            }],
        };
        let engine = PuaRuleEngine::new(Arc::new(StdMutex::new(plan)));
        let report = engine.generate_report("verification", &["run_tests".to_string()]);

        assert_eq!(report.status, "pass");
        assert!(report.missing_checks.is_empty());
        assert_eq!(report.escalation_level, "L1");
    }

    #[test]
    fn pua_rule_engine_escalate_increases_level_by_one() {
        let plan = PuaEnforcementPlan {
            escalation_level: "L1".to_string(),
            mandatory_roles: vec![],
            red_lines: vec![],
            quality_compass: vec![],
            mandatory_safeguards: vec![],
            mandatory_evidence: vec![],
            stage_requirements: vec![],
        };
        let shared = Arc::new(StdMutex::new(plan));
        let engine = PuaRuleEngine::new(shared.clone());

        let next = engine.escalate("budget overflow");
        assert_eq!(next, 2);
        let level = shared
            .lock()
            .expect("plan lock should succeed")
            .escalation_level
            .clone();
        assert_eq!(level, "L2");
    }

    #[test]
    fn pua_rule_engine_escalate_caps_at_l5() {
        let plan = PuaEnforcementPlan {
            escalation_level: "L5".to_string(),
            mandatory_roles: vec![],
            red_lines: vec![],
            quality_compass: vec![],
            mandatory_safeguards: vec![],
            mandatory_evidence: vec![],
            stage_requirements: vec![],
        };
        let shared = Arc::new(StdMutex::new(plan));
        let engine = PuaRuleEngine::new(shared.clone());

        let next = engine.escalate("budget overflow");
        assert_eq!(next, 5);
        let level = shared
            .lock()
            .expect("plan lock should succeed")
            .escalation_level
            .clone();
        assert_eq!(level, "L5");
    }

    #[test]
    fn pua_rule_engine_de_escalate_decreases_level_by_one() {
        let plan = PuaEnforcementPlan {
            escalation_level: "L3".to_string(),
            mandatory_roles: vec![],
            red_lines: vec![],
            quality_compass: vec![],
            mandatory_safeguards: vec![],
            mandatory_evidence: vec![],
            stage_requirements: vec![],
        };
        let shared = Arc::new(StdMutex::new(plan));
        let engine = PuaRuleEngine::new(shared.clone());

        let next = engine.de_escalate("incident resolved");
        assert_eq!(next, 2);
        let level = shared
            .lock()
            .expect("plan lock should succeed")
            .escalation_level
            .clone();
        assert_eq!(level, "L2");
    }

    #[test]
    fn pua_rule_engine_de_escalate_floors_at_l0() {
        let plan = PuaEnforcementPlan {
            escalation_level: "L0".to_string(),
            mandatory_roles: vec![],
            red_lines: vec![],
            quality_compass: vec![],
            mandatory_safeguards: vec![],
            mandatory_evidence: vec![],
            stage_requirements: vec![],
        };
        let shared = Arc::new(StdMutex::new(plan));
        let engine = PuaRuleEngine::new(shared.clone());

        let next = engine.de_escalate("already baseline");
        assert_eq!(next, 0, "de-escalate should floor at L0");
        let level = shared
            .lock()
            .expect("plan lock should succeed")
            .escalation_level
            .clone();
        assert_eq!(level, "L0", "should stay at L0");
    }

    #[test]
    fn mode_execution_report_marks_high_risk_as_approval_required() {
        let report = mode_execution_report("agent", true);
        assert_eq!(report.stage, "mode:agent");
        assert_eq!(report.status, "approval_required");
        assert_eq!(report.escalation_level, "L2");
        assert!(report
            .completed_checks
            .iter()
            .any(|c| c == "high_risk_detected"));
        assert!(report
            .missing_checks
            .iter()
            .any(|c| c == "operator_approval"));
    }

    #[test]
    fn tool_execution_report_clears_missing_proof_when_verification_present() {
        let report = tool_execution_report("shell", Some("runtime_ok"));
        assert_eq!(report.stage, "tool:shell");
        assert_eq!(report.status, "enforced");
        assert!(report
            .completed_checks
            .iter()
            .any(|c| c == "verification:runtime_ok"));
        assert!(!report
            .missing_checks
            .iter()
            .any(|c| c == "proof_linked_to_task"));
    }

    #[test]
    fn review_gate_prompt_contains_approve_or_reject_instruction() {
        let prompt = review_gate_prompt();
        assert!(prompt.contains("APPROVE or REJECT"));
        assert!(prompt.contains("PUA red lines"));
    }
}
