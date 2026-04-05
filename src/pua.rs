//! PUA enforcement model shared across routing, execution, verification, and review.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PuaExecutionReport {
    pub stage: String,
    pub status: String,
    pub escalation_level: String,
    pub required_evidence: Vec<String>,
    pub completed_checks: Vec<String>,
    pub missing_checks: Vec<String>,
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
            "Verify facts before attributing failures to environment or dependencies"
                .to_string(),
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

pub fn merge_phase_principles(existing: Option<Vec<String>>, phase_name: &str) -> Option<Vec<String>> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
            Some(vec!["PUA red line: close the loop with build/test/runtime proof".to_string()]),
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
}