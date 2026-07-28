//! PolicyEvaluator — the core evaluation engine of HarnessBus — F-GAP-13
//!
//! Composites all governance components (PuaRuleEngine, BudgetTracker,
//! SandboxPolicy, IdempotencyCache, OnlineControllerState,
//! SelfRationalizationGuard, SecurityGovernor, RBAC enforcer) into a single
//! evaluate/validate/verify suite.

use crate::governance::approval_engine::ApprovalEngine;
use crate::governance::approval_learning::ApprovalPreferenceLearner;
use crate::governance::drift::drift_protection::DriftProtectionEngine;
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
use crate::governance::reloadable_policy::PolicyReloader;
use crate::governance::review_controls::{
    review_verdict, ReviewGateOutcome, ReviewTimeoutPolicyKind, ReviewVerdict,
};
use crate::governance::runtime_controls::OnlineControllerState;
use crate::governance::security_governor::{
    AuditEntry as SgAuditEntry, ConditionOperator, PolicyAction, PolicyComposition,
    PolicyCondition, PolicySeverity, SecurityGovernor, SecurityGovernorConfig, SecurityPolicy,
};
use crate::i18n::runtime::tf;
use crate::security::content_safety::SafetyChecker;
use crate::security::prompt_injection::InjectionDetector;

use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
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
    /// Approval engine for structured review/approval workflows.
    pub approval_engine: Option<Arc<ApprovalEngine>>,
    /// Drift protection engine for detecting config/metric drift.
    pub drift_protection: Option<Arc<Mutex<DriftProtectionEngine>>>,
    /// Approval preference learner for auto-approval decisions.
    pub approval_learner: Option<Arc<Mutex<ApprovalPreferenceLearner>>>,
    /// Policy reloader for hot-reloadable policy files.
    pub policy_reloader: Option<Arc<Mutex<PolicyReloader>>>,
    /// Thread-safe, runtime-registerable policies keyed by name.
    /// Evaluated after the built-in checks; the first matching policy short-circuits.
    pub policies: Arc<RwLock<HashMap<String, PolicyFn>>>,

    /// Tools that the user has explicitly approved (bypasses require_review).
    pub user_approved_tools: Mutex<std::collections::HashSet<String>>,

    /// Protected invariants — file patterns that block write/destructive operations.
    /// Once set, these are checked before every write tool call and cannot be
    /// bypassed by mode or posture changes (Constitution-level enforcement).
    pub protected_invariants: RwLock<Vec<String>>,

    /// Set to true when the most recent `evaluate()` call was blocked by
    /// the self-rationalization guard. Consumed by `HarnessBus::evaluate()`
    /// to record a rationalization block counter on the governance profile.
    pub(crate) rationalization_block_occurred: AtomicBool,
    /// Set to true when the most recent `evaluate()` call resolved a review
    /// gate outcome that bypasses manual review (i.e., the response indicates
    /// override of the default review requirement).
    pub(crate) review_override_occurred: AtomicBool,

    /// Optional content safety checker for tool arguments and outputs.
    /// When set, tool calls with unsafe content are blocked/flagged.
    pub safety_checker: Option<SafetyChecker>,

    /// Optional prompt injection detector for tool arguments and outputs.
    /// When set, tool calls with injection patterns are blocked/flagged.
    pub injection_detector: Option<InjectionDetector>,
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
        policy_reloader: Option<Arc<Mutex<PolicyReloader>>>,
    ) -> Self {
        // Use default policies only when no policy_reloader is provided.
        let default_policies = if policy_reloader.is_some() {
            vec![]
        } else {
            Self::default_security_policies()
        };

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
            approval_engine: None,
            drift_protection: None,
            approval_learner: None,
            policy_reloader,
            policies: Arc::new(RwLock::new(HashMap::new())),
            security_governor: Arc::new(SecurityGovernor::new(SecurityGovernorConfig {
                default_action: PolicyAction::Deny,
                default_policies,
                ..Default::default()
            })),
            user_approved_tools: Mutex::new(std::collections::HashSet::new()),
            protected_invariants: RwLock::new(Vec::new()),
            rationalization_block_occurred: AtomicBool::new(false),
            review_override_occurred: AtomicBool::new(false),
            safety_checker: None,
            injection_detector: None,
        }
    }

    /// Return the built-in default security policies used when no
    /// [`PolicyReloader`] is configured.
    pub fn default_security_policies() -> Vec<SecurityPolicy> {
        vec![
            // 1. read_allow — allow low-risk, read-only tasks
            SecurityPolicy {
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
            },
            // 2. write_require_approval — require review for tasks that write files
            SecurityPolicy {
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
            },
            // 3. shell_require_code_exec — require review for high-risk task operations
            SecurityPolicy {
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
            },
        ]
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

        // P1-1: Load policies from PolicyReloader (RULES/ directory)
        if let Some(ref reloader) = self.policy_reloader {
            if let Ok(mut guard) = reloader.lock() {
                guard.reload_all();
                // Merge reloaded policies into the runtime policy map
                // Each reloadable policy provides its own evaluator closure via
                // as_evaluator_fn(), so the TOML configuration actually participates
                // in the evaluation pipeline.
                if let Ok(mut policies) = self.policies.write() {
                    let count = guard.policies().len();
                    for (i, policy) in guard.policies().iter().enumerate() {
                        let key = format!("reloadable_{}", i);
                        if !policies.contains_key(&key) {
                            if let Some(evaluator) = policy.as_evaluator_fn() {
                                tracing::debug!(
                                    "Registered reloadable policy '{}' with evaluator",
                                    key
                                );
                                policies.insert(key, evaluator);
                            } else {
                                policies.insert(
                                    key.clone(),
                                    Box::new(|_: &TaskContext| -> Option<PolicyVerdict> { None }),
                                );
                                tracing::debug!(
                                    "Registered reloadable policy '{}' (no evaluator — tracking only)",
                                    key
                                );
                            }
                        }
                    }
                    if count > 0 {
                        tracing::debug!(
                            count = %count,
                            "Merged reloadable policies into runtime policy map"
                        );
                    }
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
        // Detect review override: manual review was required but the gate
        // resolved to Approve (e.g., via a customized review_verdict impl).
        if requires_review && verdict.is_approved() {
            self.review_override_occurred.store(true, Ordering::Release);
        }
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
            self.rationalization_block_occurred
                .store(true, Ordering::Release);
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

    /// Register a protected file pattern. Write/destructive operations that
    /// touch files matching this pattern will be blocked regardless of mode.
    /// Uses simple substring matching on tool arguments (e.g. "Cargo.lock").
    pub fn register_protected_invariant(&self, pattern: &str) {
        if let Ok(mut invariants) = self.protected_invariants.write() {
            if !invariants.iter().any(|p| p == pattern) {
                invariants.push(pattern.to_string());
                tracing::info!(pattern = %pattern, "protected_invariant registered");
            }
        }
    }

    /// Check if any argument value matches a protected invariant pattern.
    fn protected_path_blocked(&self, args: &Value) -> Option<String> {
        let invariants = match self.protected_invariants.read() {
            Ok(g) => g.clone(),
            Err(_) => return None,
        };
        if invariants.is_empty() {
            return None;
        }
        let mut values: Vec<String> = Vec::new();
        collect_string_values(args, &mut values);
        for pattern in &invariants {
            for value in &values {
                if value.contains(pattern.as_str()) {
                    return Some(format!(
                        "protected invariant '{}' blocks operation on '{}'",
                        pattern, value
                    ));
                }
            }
        }
        None
    }

    /// Pre-tool-call validation.
    /// Approve a tool for the current session — bypasses sandbox require_review.
    pub fn approve_tool(&self, tool: &str) {
        if let Ok(mut approved) = self.user_approved_tools.lock() {
            approved.insert(tool.to_string());
        }
    }

    /// Revoke approval for a tool.
    pub fn revoke_tool_approval(&self, tool: &str) {
        if let Ok(mut approved) = self.user_approved_tools.lock() {
            approved.remove(tool);
        }
    }

    pub fn check_tool_call(&self, tool: &str, args: &Value) -> ToolVerdict {
        let level = *self.sandbox_level.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("[harness_bus] lock poisoned, recovering");
            poisoned.into_inner()
        });

        // ── Content safety check on tool arguments ───────────────────────
        if let Some(ref checker) = self.safety_checker {
            let args_text = serde_json::to_string(args).unwrap_or_default();
            let violations = checker.check(&args_text);
            if !violations.is_empty() {
                tracing::warn!(
                    target: "harness_bus",
                    tool = %tool,
                    violations = ?violations,
                    "content safety check blocked tool call"
                );
                return ToolVerdict {
                    allowed: false,
                    require_review: true,
                    idempotent: false,
                    budget_ok: false,
                    permitted: false,
                };
            }
        }

        // ── Prompt injection check on tool arguments ───────────────────
        if let Some(ref detector) = self.injection_detector {
            let args_text = serde_json::to_string(args).unwrap_or_default();
            let result = detector.detect(&args_text);
            if result.detected
                && detector.should_block(
                    &result,
                    crate::security::prompt_injection::InjectionSeverity::Medium,
                )
            {
                tracing::warn!(
                    target: "harness_bus",
                    tool = %tool,
                    violations = ?result.violations,
                    "prompt injection check blocked tool call"
                );
                return ToolVerdict {
                    allowed: false,
                    require_review: true,
                    idempotent: false,
                    budget_ok: false,
                    permitted: false,
                };
            }
        }
        // ── All tools categorized by operation type ────────────────
        //
        // Each tool is classified by its dominant operation class so that
        // the sandbox level check is fine-grained.  Tools not explicitly
        // listed fall through to require user review.
        let mut recognized = true;
        let allowed = match tool {
            // ── Read / Query tools (safe at ALL sandbox levels) ───────
            "read_file"
            | "search_files"
            | "inspect_git_diff"
            | "list_directory"
            | "date_time"
            | "skill_list"
            | "skill-finder"
            // chat.execute is an ACP protocol entry point (no ToolRegistry impl)
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
            | "workflow_execute"
            | "workflow_ask"
            | "workflow_generate"
            | "import_skill"
            | "archive_inspect"
            | "jsonl_read"
            // ── Environment info tool (safe at ALL sandbox levels) ─
            | "environment_info"
            // ── Skill query / echo tools (safe at ALL sandbox levels) ─
            | "echo_skill"
            | "builtin.echo"
            | "goon_skill_version_list"
            // ── Document readers ──────────────────────────────
            | "read_pdf"
            | "pdf_merge"
            | "pdf_split"
            | "read_docx"
            | "read_excel"
            | "read_ppt"
            | "email_parse"
            | "invoice_parse"
            | "web_scrape"
            // ── CAD / 3D readers ──────────────────────────────
            | "dxf_read"
            | "cad_convert"
            | "step_read"
            | "obj_read"
            | "obj_model_read"
            | "stl_read"
            | "gltf_read"
            | "iges_read"
            | "ply_read"
            | "geo_util"
            | "gcode_read"
            | "gpx_read"
            // ── Image / Drawing readers ────────────────────────
            | "image_analyze"
            | "svg_read"
            // ── Data readers ──────────────────────────────────
            | "csv_read"
            | "csv_analyze"
            | "toml_read"
            | "yaml_read"
            // ── Database ───────────────────────────────────────
            | "sqlite_query"
            // ── Game readers ───────────────────────────────────
            | "game_server_query"
            | "game_price_tracker"
            | "game_matchmaking"
            | "game_achievements"
            | "game_mod_list"
            | "game_coaching_assistant"
            // ── Compilation check (read-only, may invoke compiler) ─
            | "cargo_check"
            | "diagnostics"
            // ── Skill execution ──────────────────────────────────
            | "skill_execute" => SandboxPolicy::can_execute_read_file(level),
            // ── Game process / automation (shell) — requires unrestricted ─
            "game_launch"
            | "game_keyboard_input"
            | "game_mouse_input"
            | "game_auto_grind" => SandboxPolicy::can_execute_shell(level),
            // ── Game file operations (write) — restricted at Strict+ ─
            "game_screen_capture"
            | "game_replay_recorder"
            | "game_save_manager"
            | "game_mod_install" => SandboxPolicy::can_execute_write(level),
            // ── Search / Discovery tools ──────────────────────────
            "grep" | "find_path" | "semantic_search" | "code_index_search" | "find_files" => {
                SandboxPolicy::can_execute_search(level)
            }
            // ── Network / Outbound tools ─────────────────────────
            "http_request"
            | "dns_lookup"
            | "ping"
            | "port_scan"
            | "git"
            | "github_search_skills"
            | "rss_read"
            | "game_monitor"
            | "goon_provider_test_connection"
            | "goon_provider_test_completion" => SandboxPolicy::can_execute_network(level),
            // ── Write / Admin tools (restricted in stricter sandbox levels) ─
            "write_file" | "apply_patch" | "create_directory" | "delete_path" | "move_path"
            | "copy_path" | "file_move" | "file_delete" | "compress" | "decompress"
            | "archive_extract" | "jsonl_write"
            // ── Image write tools (includes aliases) ─────────────────
            | "goon_skill_update"
            | "goon_skill_version_rollback"
            | "skill-creator"
            // ── Workflow admin tools (write operations) ────────────────
            | "goon_workflow_run_cancel"
            | "goon_workflow_run_pause"
            | "goon_workflow_run_resume"
            // ── CSV / Data write tools ────────────────────────────────
            | "csv_write"
            | "csv_transform"
            | "toml_write"
            | "yaml_write"
            // ── Image write tools ─────────────────────────────────────
            | "image_convert"
            | "image_resize"
            | "image_generate"
            // ── Drawing / SVG write tools ─────────────────────────────
            | "svg_generate"
            | "svg_export"
            // ── 3D / CAD write tools ──────────────────────────────────
            | "stl_generate"
            // ── Barcode tools ─────────────────────────────────────────
            | "qrcode_generate"
            // ── Document write tools ──────────────────────────────────
            | "write_docx"
            | "write_ppt"
            | "write_excel" => SandboxPolicy::can_execute_write(level),
            // ── Shell / Execution tools (restricted at Basic+) ─────────
            "run_tests" | "execute_command" | "terminal" | "bash" | "cargo_test" | "shell_exec" => {
                SandboxPolicy::can_execute_shell(level)
            }
            // ── Unknown tools — require user review (not auto-allowed) ─
            _ => {
                // Check user-approved tools first (bypasses require_review)
                let is_approved = self.user_approved_tools.lock()
                    .map(|approved| approved.contains(tool))
                    .unwrap_or(false);
                if is_approved {
                    recognized = true;
                    true  // User explicitly approved, bypass sandbox
                } else {
                    recognized = false;
                    false
                }
            }
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

        // Check protected invariants before allowing any operation.
        // This is a mechanical write-hold — it cannot be bypassed by mode/posture.
        if let Some(reason) = self.protected_path_blocked(args) {
            tracing::warn!(
                target: "harness_bus",
                reason = %reason,
                "protected invariant blocked tool call"
            );
            return ToolVerdict {
                allowed: false,
                require_review: false,
                idempotent,
                budget_ok,
                permitted: false,
            };
        }

        let permitted = self.check_permission(tool, args);
        ToolVerdict {
            allowed,
            require_review: !recognized,
            idempotent,
            budget_ok,
            permitted,
        }
    }

    /// Post-execution output verification.
    /// Validates that `output` is a well-formed JSON value, checks for expected
    /// structural fields, runs content safety and prompt injection checks,
    /// and logs the verification outcome.
    pub fn verify_output(&self, output: &Value) -> OutputVerdict {
        let stage = "default";
        let completed: Vec<String> = Vec::new();

        // --- Validate the output value itself ---
        let output_shape = match output {
            Value::Null => {
                tracing::warn!("[harness_bus] verify_output: output is null");
                "null"
            }
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(s) => {
                if s.is_empty() {
                    tracing::warn!("[harness_bus] verify_output: output is an empty string");
                    "string_empty"
                } else {
                    "string"
                }
            }
            Value::Array(arr) => {
                if arr.is_empty() {
                    tracing::warn!("[harness_bus] verify_output: output is an empty array");
                    "array_empty"
                } else {
                    "array"
                }
            }
            Value::Object(obj) => {
                let keys: Vec<&str> = obj.keys().map(|k| k.as_str()).collect();
                tracing::debug!(
                    "[harness_bus] verify_output: output object with keys: {:?}",
                    keys
                );
                "object"
            }
        };

        tracing::debug!("[harness_bus] verify_output: output shape={}", output_shape);

        // ── Content safety check on output ─────────────────────────────
        let mut safety_risk: f64 = 0.0;
        if let Some(ref checker) = self.safety_checker {
            let output_text = serde_json::to_string(output).unwrap_or_default();
            let violations = checker.check(&output_text);
            if !violations.is_empty() {
                tracing::warn!(
                    target: "harness_bus",
                    violations = ?violations,
                    "content safety violation detected in output"
                );
                // Increase risk score based on number/severity of violations
                safety_risk = (violations.len() as f64 * 0.2).min(0.8);
            }
        }

        // ── Prompt injection check on output ──────────────────────────
        if let Some(ref detector) = self.injection_detector {
            let output_text = serde_json::to_string(output).unwrap_or_default();
            let result = detector.detect(&output_text);
            if result.detected {
                let high_sev_count = result
                    .violations
                    .iter()
                    .filter(|v| {
                        v.severity == crate::security::prompt_injection::InjectionSeverity::High
                    })
                    .count();
                if high_sev_count > 0 {
                    safety_risk = safety_risk.max(0.9);
                }
            }
        }

        let engine = self.rule_engine.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("[harness_bus] lock poisoned, recovering");
            poisoned.into_inner()
        });
        let mut evidence = engine.collect_evidence(stage);
        // Record output validation in evidence
        evidence.push(format!("output_shape:{}", output_shape));
        let missing = engine.collect_missing(stage, &completed);
        drop(engine);

        let quality = missing.is_empty()
            && output_shape != "null"
            && output_shape != "string_empty"
            && output_shape != "array_empty";
        let mut risk_score: f64 = if quality { 0.0 } else { 0.5 };

        // P1-11: Call SelfRationalizationGuard to evaluate confidence
        // Fold in safety risk from content/injection checks
        risk_score = risk_score.max(safety_risk);
        {
            let mut guard = self.guard.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("[harness_bus] lock poisoned, recovering");
                poisoned.into_inner()
            });
            let mut annotation = RationalizationAnnotation::default();
            let low_confidence = guard.evaluate(&mut annotation, risk_score as f32, false);
            if low_confidence {
                risk_score = f64::min(risk_score + 0.2, 1.0);
                evidence.push("low_confidence_warning: guard flagged weak evidence".to_string());
                tracing::warn!(
                    adjusted_risk = %risk_score,
                    "verify_output: low confidence detected, adjusted risk_score and added warning"
                );
            }
        }

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
                        format!("output verification failed: output_shape={}", output_shape)
                    }],
                },
                "verify_output".to_string(),
                "harness".to_string(),
                format!(
                    "quality={}, risk_score={}, evidence_count={}, output_shape={}",
                    quality, risk_score, evidence_count, output_shape
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
            "write_file" | "apply_patch" | "create_directory" | "delete_path" | "move_path"
            | "copy_path" | "file_move" | "file_delete" => GovernanceAction::Write,
            "run_tests" | "execute_command" | "terminal" | "bash" | "shell_exec" | "cargo_test" => {
                GovernanceAction::Shell
            }
            "search" | "find" | "grep" | "semantic_search" | "code_index_search" | "find_path"
            | "find_files" => GovernanceAction::Search,
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
                GovernanceAction::Shell | GovernanceAction::Network => Permission::Execute,
                GovernanceAction::Read | GovernanceAction::Search => Permission::Read,
            };
            let tenant_id = rbac.tenant_ids().into_iter().next();
            // P1-4: Extract principal from _args; fall back to "harness"/["user"] when absent
            let user_id = _args
                .get("user_id")
                .and_then(|v| v.as_str())
                .unwrap_or("harness");
            let roles: Vec<&str> = _args
                .get("roles")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect::<Vec<&str>>())
                .unwrap_or_else(|| vec!["user"]);
            let mut principal = Principal::new(user_id, roles, tenant_id.as_deref());
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
    ///
    /// Checks three conditions:
    /// 1. Security governor has denied recent requests
    /// 2. Drift protection has active alerts
    /// 3. Self-rationalization guard has flagged weak evidence
    pub fn needs_reexamine(&self, _ctx: &TaskContext) -> bool {
        // 1. Check security governor for recent denials
        let gov_profile = self.security_governor.profile();
        if gov_profile.total_denials > 0 {
            tracing::info!(
                total_denials = gov_profile.total_denials,
                total_evaluations = gov_profile.total_evaluations,
                "needs_reexamine: security governor has recent denials"
            );
            return true;
        }
        if gov_profile.active_escalations > 0 {
            tracing::info!(
                active_escalations = gov_profile.active_escalations,
                "needs_reexamine: security governor has active escalations"
            );
            return true;
        }

        // 2. Check drift protection for active alerts
        if let Some(ref drift) = self.drift_protection {
            if let Ok(guard) = drift.lock() {
                let active = guard.get_active_alerts();
                if !active.is_empty() {
                    tracing::info!(
                        active_alerts = active.len(),
                        "needs_reexamine: drift protection has active alerts"
                    );
                    return true;
                }
            }
        }

        // 3. Check self-rationalization guard for low-confidence flags
        {
            let guard = self.guard.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("[harness_bus] lock poisoned, recovering");
                poisoned.into_inner()
            });
            if guard.counters.weak_evidence_blocked_count > 0 {
                tracing::info!(
                    blocked = guard.counters.weak_evidence_blocked_count,
                    "needs_reexamine: guard flagged low confidence"
                );
                return true;
            }
        }

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

    /// Returns `true` if the most recent `evaluate()` call was blocked by
    /// the self-rationalization guard, and resets the flag for the next call.
    /// Used by `HarnessBus::evaluate()` to record `record_rationalization_block()`.
    pub fn drain_rationalization_blocked(&self) -> bool {
        self.rationalization_block_occurred
            .swap(false, Ordering::AcqRel)
    }

    /// Returns `true` if the most recent `evaluate()` call detected a review
    /// gate override (manual review was required but the gate resolved to
    /// Approve), and resets the flag.
    pub fn drain_review_override(&self) -> bool {
        self.review_override_occurred.swap(false, Ordering::AcqRel)
    }

    /// Resolve a raw response string into a governance-level review verdict.
    fn resolve_review_policy(response: &str, min_response_chars: usize) -> ReviewVerdict {
        review_verdict(response, min_response_chars)
    }
}

/// Recursively collect all string values from a JSON Value tree.
/// Used by `PolicyEvaluator::protected_path_blocked` to scan tool arguments
/// for protected file patterns.
fn collect_string_values(value: &Value, output: &mut Vec<String>) {
    match value {
        Value::String(s) => output.push(s.clone()),
        Value::Object(map) => {
            for v in map.values() {
                collect_string_values(v, output);
            }
        }
        Value::Array(arr) => {
            for v in arr {
                collect_string_values(v, output);
            }
        }
        _ => {}
    }
}
