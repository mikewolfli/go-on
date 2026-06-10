//! PolicyEvaluator — the core evaluation engine of HarnessBus — F-GAP-13
//!
//! Composites all governance components (PuaRuleEngine, BudgetTracker,
//! SandboxPolicy, IdempotencyCache, OnlineControllerState,
//! SelfRationalizationGuard, SecurityGovernor, RBAC enforcer) into a single
//! evaluate/validate/verify suite.

use crate::governance::hardening::{
    rbac_fallback_allows_action, BudgetTracker, GovernanceAction, IdempotencyCache, SandboxLevel,
    SandboxPolicy,
};
use crate::governance::harness_bus::types::{
    DispatchPolicy, EscalationReason, ExecutionPolicy, GovernancePolicy, OutputVerdict,
    PolicyVerdict, PolicyViolation, ReviewReason, ToolVerdict,
};
use crate::governance::pua::{PuaRuleEngine, TaskContext};
use crate::governance::rationalization::{RationalizationAnnotation, SelfRationalizationGuard};
use crate::governance::rbac::{AccessDecision, Permission, Principal, RbacEnforcer};
use crate::governance::review_controls::{
    review_verdict, ReviewGateOutcome, ReviewTimeoutPolicyKind, ReviewVerdict,
};
use crate::governance::runtime_controls::OnlineControllerState;
use crate::governance::security_governor::{
    AuditEntry as SgAuditEntry, ConditionOperator, PolicyAction, PolicyComposition,
    PolicyCondition, PolicySeverity, SecurityGovernor, SecurityGovernorConfig, SecurityPolicy,
};
use crate::i18n::runtime::tf;

use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

/// Runtime-registerable policy: takes a TaskContext and returns None if no opinion,
/// or Some(PolicyVerdict) to short-circuit the normal evaluation flow.
pub type PolicyFn = Box<dyn Fn(&TaskContext) -> Option<PolicyVerdict> + Send + Sync>;

/// PolicyEvaluator composites all governance components into a single
/// evaluate/validate/verify suite.
pub struct PolicyEvaluator {
    pub dispatch: DispatchPolicy,
    pub execution: ExecutionPolicy,
    pub governance: GovernancePolicy,
    pub rule_engine: Arc<Mutex<PuaRuleEngine>>,
    pub sandbox_level: Arc<Mutex<SandboxLevel>>,
    pub budget: Arc<Mutex<BudgetTracker>>,
    pub idempotency: Arc<Mutex<IdempotencyCache>>,
    pub runtime_control: Arc<Mutex<OnlineControllerState>>,
    pub guard: Arc<Mutex<SelfRationalizationGuard>>,
    pub security_governor: Arc<SecurityGovernor>,
    pub rbac_enforcer: RwLock<Option<Arc<RwLock<RbacEnforcer>>>>,
    /// Thread-safe, runtime-registerable policies keyed by name.
    /// Evaluated after the built-in checks; the first matching policy short-circuits.
    pub policies: Arc<RwLock<HashMap<String, PolicyFn>>>,
}

impl PolicyEvaluator {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        rule_engine: Arc<Mutex<PuaRuleEngine>>,
        sandbox_level: Arc<Mutex<SandboxLevel>>,
        budget: Arc<Mutex<BudgetTracker>>,
        idempotency: Arc<Mutex<IdempotencyCache>>,
        runtime_control: Arc<Mutex<OnlineControllerState>>,
        guard: Arc<Mutex<SelfRationalizationGuard>>,
    ) -> Self {
        Self {
            dispatch: DispatchPolicy::default(),
            execution: ExecutionPolicy::default(),
            governance: GovernancePolicy::default(),
            rule_engine,
            sandbox_level,
            budget,
            idempotency,
            runtime_control,
            guard,
            rbac_enforcer: RwLock::new(None),
            policies: Arc::new(RwLock::new(HashMap::new())),
            security_governor: Arc::new({
                let gov = SecurityGovernor::new(SecurityGovernorConfig {
                    default_action: PolicyAction::Deny,
                    ..Default::default()
                });

                // 1. read_allow — allow low-risk, read-only tasks
                gov.register_policy(SecurityPolicy {
                    id: "read_allow".into(),
                    name: "Allow read/search operations".into(),
                    description:
                        "Permits read and search operations for zero-risk tasks with no file writes"
                            .into(),
                    severity: PolicySeverity::Low,
                    action: PolicyAction::Allow,
                    conditions: vec![
                        PolicyCondition {
                            field: "risk_score".into(),
                            operator: ConditionOperator::Equals,
                            value: "0".into(),
                        },
                        PolicyCondition {
                            field: "file_count".into(),
                            operator: ConditionOperator::Equals,
                            value: "0".into(),
                        },
                    ],
                    composition: PolicyComposition::And,
                    escalation_level: None,
                });

                // 2. write_require_approval — require review for tasks that write files
                gov.register_policy(SecurityPolicy {
                    id: "write_require_approval".into(),
                    name: "Write operations require approval".into(),
                    description: "Tasks that modify files require manual review approval".into(),
                    severity: PolicySeverity::Medium,
                    action: PolicyAction::RequireReview,
                    conditions: vec![PolicyCondition {
                        field: "file_count".into(),
                        operator: ConditionOperator::NotEquals,
                        value: "0".into(),
                    }],
                    composition: PolicyComposition::And,
                    escalation_level: None,
                });

                // 3. shell_require_code_exec — require review for high-risk task operations
                gov.register_policy(SecurityPolicy {
                    id: "shell_require_code_exec".into(),
                    name: "Shell operations require code execution review".into(),
                    description: "Shell and terminal operations require additional review approval"
                        .into(),
                    severity: PolicySeverity::High,
                    action: PolicyAction::RequireReview,
                    conditions: vec![PolicyCondition {
                        field: "risk_score".into(),
                        operator: ConditionOperator::NotEquals,
                        value: "0".into(),
                    }],
                    composition: PolicyComposition::And,
                    escalation_level: None,
                });

                gov
            }),
        }
    }

    /// Register a runtime policy. The closure is invoked during evaluate();
    /// if it returns Some(verdict) the evaluation short-circuits.
    pub fn register_policy(&self, name: &str, policy: PolicyFn) {
        if let Ok(mut guard) = self.policies.write() {
            guard.insert(name.to_string(), policy);
            tracing::debug!(policy = %name, "Runtime policy registered");
        }
    }

    /// Deregister a previously registered runtime policy.
    pub fn deregister_policy(&self, name: &str) {
        if let Ok(mut guard) = self.policies.write() {
            guard.remove(name);
            tracing::debug!(policy = %name, "Runtime policy deregistered");
        }
    }

    /// Pre-route composite evaluation.
    /// Returns a PolicyVerdict that the caller (CapabilityBus) should respect.
    pub fn evaluate(&self, ctx: &TaskContext) -> PolicyVerdict {
        let _start = Instant::now();

        // 0. Check runtime-registerable policies first (short-circuit on match).
        if let Ok(guard) = self.policies.read() {
            for (name, policy) in guard.iter() {
                if let Some(verdict) = policy(ctx) {
                    tracing::debug!(policy = %name, verdict = ?verdict, "Runtime policy matched");
                    return verdict;
                }
            }
        }

        // 1. Red-line check (hard block)
        let engine = self.rule_engine.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("[harness_bus] lock poisoned, recovering");
            poisoned.into_inner()
        });
        if let Err(violation) = engine.check_red_lines(&format!("{:?}", ctx.task_type)) {
            return PolicyVerdict::Deny(PolicyViolation {
                kind: "red_line".to_string(),
                detail: violation.detail.clone(),
            });
        }
        // 2. Stage validation
        if let Err(fail) = engine.validate_stage("default", &[]) {
            return PolicyVerdict::Escalate(EscalationReason {
                reason: fail.detail.clone(),
                suggested_level: 2,
            });
        }

        // 3. Budget check (hard limit)
        let budget = self.budget.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("[harness_bus] lock poisoned, recovering");
            poisoned.into_inner()
        });
        if let Err(_err) = budget.check_wall_clock() {
            return PolicyVerdict::Deny(PolicyViolation {
                kind: "budget".to_string(),
                detail: tf("error.harness_bus.budget_exceeded", &[]),
            });
        }

        // 4. Runtime control check (adaptive sliding window / P95 / UCB)
        let mut runtime_ctrl = Some(self.runtime_control.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("[harness_bus] lock poisoned, recovering");
            poisoned.into_inner()
        }));
        if let Some(ref mut ctrl) = runtime_ctrl {
            if ctrl.should_escalate() {
                ctrl.record(false, _start.elapsed().as_millis() as u64);
                return PolicyVerdict::Escalate(EscalationReason {
                    reason: tf("error.harness_bus.runtime_escalation", &[]),
                    suggested_level: 3,
                });
            }
        }

        // 5. Review policy check (verify verdict from review_controls)
        if self.governance.quality_compass.enabled {
            tracing::debug!("review gate evaluating governance-driven review verdict");
        }
        let timeout_policy = ReviewTimeoutPolicyKind::from_options(None);
        let timeout_duration = crate::governance::review_controls::review_timeout(None, None);
        let requires_manual_review = ctx.risk_score >= 0.70
            || ctx.file_count >= 8
            || matches!(
                ctx.task_type,
                crate::governance::pua::TaskType::SecurityPatch
            );
        let requires_review = requires_manual_review;
        let review_response = if requires_review {
            tf("status.harness_bus.review_rejected", &[])
        } else {
            tf("status.harness_bus.review_approved", &[])
        };
        let verdict = Self::resolve_review_policy(&review_response, 8);
        let outcome = match verdict {
            ReviewVerdict::Approve => ReviewGateOutcome::Approved(vec![
                crate::governance::review_controls::ReviewDecision {
                    reviewer: "governance-policy".to_string(),
                    verdict: verdict.as_str().to_string(),
                    response: review_response.to_string(),
                },
            ]),
            ReviewVerdict::Reject => ReviewGateOutcome::Rejected(vec![
                crate::governance::review_controls::ReviewDecision {
                    reviewer: "governance-policy".to_string(),
                    verdict: verdict.as_str().to_string(),
                    response: review_response.to_string(),
                },
            ]),
            ReviewVerdict::Invalid => ReviewGateOutcome::Degraded(vec![
                crate::governance::review_controls::ReviewDecision {
                    reviewer: "governance-policy".to_string(),
                    verdict: verdict.as_str().to_string(),
                    response: review_response.to_string(),
                },
            ]),
        };
        let review_result = match &outcome {
            ReviewGateOutcome::Approved(decisions)
            | ReviewGateOutcome::Rejected(decisions)
            | ReviewGateOutcome::Degraded(decisions) => decisions
                .first()
                .map(|d| d.reviewer.as_str())
                .unwrap_or("none"),
        };
        tracing::debug!(
            reviewer = review_result,
            verdict = verdict.as_str(),
            timeout_policy = ?timeout_policy,
            timeout_duration = ?timeout_duration,
            "review gate evaluated"
        );
        if !verdict.is_approved() {
            return PolicyVerdict::Review(ReviewReason {
                reason: tf("error.harness_bus.review_gate_manual", &[]),
            });
        }

        // 6. Self-rationalization guard (low confidence check)
        let mut guard = self.guard.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("[harness_bus] lock poisoned, recovering");
            poisoned.into_inner()
        });
        let mut annotation = RationalizationAnnotation::default();
        if guard.evaluate(&mut annotation, ctx.risk_score as f32, false) {
            return PolicyVerdict::Review(ReviewReason {
                reason: tf("error.harness_bus.low_confidence", &[]),
            });
        }

        // 7. Security governor policy evaluation
        let task_type_str = format!("{:?}", ctx.task_type);
        let actor = format!("risk:{:.2}", ctx.risk_score);
        let context: HashMap<String, String> = [
            ("task_type".to_string(), task_type_str.clone()),
            ("file_count".to_string(), ctx.file_count.to_string()),
            ("risk_score".to_string(), ctx.risk_score.to_string()),
        ]
        .into_iter()
        .collect();
        let sg_result = self
            .security_governor
            .evaluate(&task_type_str, &actor, &context);

        // B51-33: Record security governor audit entry after evaluation.
        {
            use crate::governance::security_governor::PolicyVerdict as SgPolicyVerdict;
            let sg_entry = SgAuditEntry::new(
                sg_result
                    .as_ref()
                    .ok()
                    .and_then(|v| v.matched_policy.clone())
                    .unwrap_or_else(|| "none".to_string()),
                SgPolicyVerdict {
                    allowed: sg_result.as_ref().ok().map(|v| v.allowed).unwrap_or(false),
                    required_review: sg_result
                        .as_ref()
                        .ok()
                        .map(|v| v.required_review)
                        .unwrap_or(false),
                    escalation_level: sg_result
                        .as_ref()
                        .ok()
                        .map(|v| v.escalation_level.clone())
                        .unwrap_or_else(|| "normal".to_string()),
                    matched_policy: sg_result
                        .as_ref()
                        .ok()
                        .and_then(|v| v.matched_policy.clone()),
                    reasons: sg_result
                        .as_ref()
                        .ok()
                        .map(|v| v.reasons.clone())
                        .unwrap_or_default(),
                },
                task_type_str.clone(),
                actor.clone(),
                sg_result
                    .as_ref()
                    .ok()
                    .map(|v| v.reasons.join("; "))
                    .unwrap_or_else(|| "evaluation_error".to_string()),
            );
            self.security_governor.record_audit(sg_entry);
        }

        match sg_result {
            Ok(sg_verdict) => {
                if !sg_verdict.allowed {
                    return PolicyVerdict::Deny(PolicyViolation {
                        kind: "security_policy".to_string(),
                        detail: sg_verdict.reasons.first().cloned().unwrap_or_else(|| {
                            tf("error.harness_bus.security_governor_denied", &[])
                        }),
                    });
                }
                if sg_verdict.required_review {
                    return PolicyVerdict::Review(ReviewReason {
                        reason: sg_verdict.reasons.first().cloned().unwrap_or_else(|| {
                            tf("error.harness_bus.security_governor_review", &[])
                        }),
                    });
                }
            }
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "SecurityGovernor evaluate failed — denying operation as safety default"
                );
                return PolicyVerdict::Deny(PolicyViolation {
                    kind: "security_policy".to_string(),
                    detail: tf("error.harness_bus.security_governor_denied", &[]),
                });
            }
        }

        // 8. All checks passed — record success for adaptive control
        if let Some(mut ctrl) = runtime_ctrl {
            ctrl.record(true, _start.elapsed().as_millis() as u64);
        }
        PolicyVerdict::Allow
    }

    /// Pre-tool-call validation.
    pub fn check_tool_call(&self, tool: &str, _args: &Value) -> ToolVerdict {
        let level = *self.sandbox_level.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("[harness_bus] lock poisoned, recovering");
            poisoned.into_inner()
        });
        let allowed = match tool {
            "read_file"
            | "search_files"
            | "inspect_git_diff"
            | "chat.execute"
            | "acp_trace_get"
            | "acp_debug_panel_get"
            | "goon_workflow_run_list"
            | "goon_workflow_run_get"
            | "goon_metrics_window_query"
            | "goon_metrics_errors_summary"
            | "goon_provider_capabilities"
            | "prompts_list"
            | "prompts_get"
            | "skill-finder" => SandboxPolicy::can_execute_read_file(level),
            "grep" | "find_path" | "semantic_search" => SandboxPolicy::can_execute_search(level),
            "write_file" | "apply_patch" | "create_directory" | "delete_path" | "move_path"
            | "copy_path" => SandboxPolicy::can_execute_write(level),
            "run_tests" | "execute_command" | "terminal" | "bash" => {
                SandboxPolicy::can_execute_shell(level)
            }
            _ => false,
        };
        let idempotent = self
            .idempotency
            .lock()
            .unwrap_or_else(|poisoned| {
                tracing::warn!("[harness_bus] lock poisoned, recovering");
                poisoned.into_inner()
            })
            .get(tool)
            .is_some();
        let budget_ok = self
            .budget
            .lock()
            .unwrap_or_else(|poisoned| {
                tracing::warn!("[harness_bus] lock poisoned, recovering");
                poisoned.into_inner()
            })
            .record_tool_call()
            .is_ok();
        let permitted = self.check_permission(tool, _args);
        ToolVerdict {
            allowed,
            idempotent,
            budget_ok,
            permitted,
        }
    }

    /// Post-execution output verification.
    pub fn verify_output(&self, _output: &Value) -> OutputVerdict {
        let stage = "default";
        let completed: Vec<String> = Vec::new();

        let engine = self.rule_engine.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("[harness_bus] lock poisoned, recovering");
            poisoned.into_inner()
        });
        let evidence = engine.collect_evidence(stage);
        let missing = engine.collect_missing(stage, &completed);
        drop(engine);

        let quality = missing.is_empty();
        let risk_score = if missing.is_empty() { 0.0 } else { 0.5 };
        let evidence_count = evidence.len();
        let verdict = OutputVerdict {
            quality,
            evidence,
            risk_score,
        };

        if self.governance.audit.enabled {
            let audit_entry = SgAuditEntry::new(
                "verify_output".to_string(),
                crate::governance::security_governor::PolicyVerdict {
                    allowed: quality,
                    required_review: !quality,
                    escalation_level: "normal".to_string(),
                    matched_policy: None,
                    reasons: vec![if quality {
                        "output verification passed".to_string()
                    } else {
                        "output verification found missing evidence".to_string()
                    }],
                },
                "verify_output".to_string(),
                "harness".to_string(),
                format!(
                    "quality={}, risk_score={}, evidence_count={}",
                    quality, risk_score, evidence_count
                ),
            );
            self.security_governor.record_audit(audit_entry);
        }

        verdict
    }

    /// Permission check (delegates to RBAC enforcer when configured, otherwise
    /// applies an explicit fallback policy based on the active sandbox level).
    fn check_permission(&self, tool: &str, _args: &Value) -> bool {
        let action = match tool {
            "write_file" | "apply_patch" | "create_directory" | "delete_path" => {
                GovernanceAction::Write
            }
            "run_tests" | "execute_command" | "terminal" => GovernanceAction::Shell,
            "search" | "find" | "grep" | "semantic_search" => GovernanceAction::Search,
            _ => GovernanceAction::Read,
        };

        let shared_arc = self
            .rbac_enforcer
            .read()
            .ok()
            .and_then(|guard| guard.clone());
        let inner_guard = shared_arc.as_ref().and_then(|arc| arc.read().ok());
        if let Some(ref rbac) = inner_guard {
            let required_perm = match action {
                GovernanceAction::Write => Permission::Write,
                GovernanceAction::Shell => Permission::Execute,
                GovernanceAction::Read | GovernanceAction::Search => Permission::Read,
            };
            let tenant_id = rbac.tenant_ids().into_iter().next();
            let mut principal = Principal::new("harness", vec!["user"], tenant_id.as_deref());
            rbac.resolve_permissions(&mut principal);
            match rbac.check_access(&principal, &required_perm) {
                AccessDecision::Allow => {
                    tracing::debug!(
                        tool = %tool,
                        required = ?required_perm,
                        "RBAC: access granted"
                    );
                    true
                }
                decision => {
                    tracing::warn!(
                        tool = %tool,
                        required = ?required_perm,
                        decision = ?decision,
                        "RBAC: access denied"
                    );
                    false
                }
            }
        } else {
            let deployment_hint = {
                let level = self.sandbox_level.lock().unwrap_or_else(|poisoned| {
                    tracing::warn!("[harness_bus] lock poisoned, recovering");
                    poisoned.into_inner()
                });
                match *level {
                    SandboxLevel::None => "local-dev",
                    SandboxLevel::Basic => "ci",
                    SandboxLevel::Strict => "managed-service",
                    SandboxLevel::Isolated => "production",
                }
            };
            let decision = rbac_fallback_allows_action(Some(deployment_hint), action);
            tracing::info!(
                tool = %tool,
                policy = %decision.policy_name,
                sandbox = %decision.sandbox_level,
                allowed = decision.allowed,
                reason = %decision.reason,
                "RBAC enforcer unavailable, applying fallback policy"
            );
            decision.allowed
        }
    }

    /// Determine whether a re-examination is needed (self-rationalization helper).
    pub fn needs_reexamine(&self, _ctx: &TaskContext) -> bool {
        false
    }

    /// Inject a shared Arc RBAC enforcer for multi-tenant permission checks.
    pub fn set_rbac_enforcer(&self, enforcer: Arc<RwLock<RbacEnforcer>>) {
        match self.rbac_enforcer.write() {
            Ok(mut guard) => {
                *guard = Some(enforcer);
            }
            Err(poisoned) => {
                tracing::error!(target: "harness_bus", "rbac_enforcer RwLock poisoned – cannot set enforcer");
                let mut guard = poisoned.into_inner();
                *guard = Some(enforcer);
            }
        }
    }

    /// Resolve a raw response string into a governance-level review verdict.
    fn resolve_review_policy(response: &str, min_response_chars: usize) -> ReviewVerdict {
        let verdict = review_verdict(response, min_response_chars);
        let _ = verdict.as_str();
        verdict
    }
}
