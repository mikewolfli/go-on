//! PolicyEvaluator — the core evaluation engine of HarnessBus — F-GAP-13
//!
//! Composites all governance components (PuaRuleEngine, BudgetTracker,
//! SandboxPolicy, IdempotencyCache, OnlineControllerState,
//! SelfRationalizationGuard, SecurityGovernor, RBAC enforcer) into a single
//! evaluate/validate/verify suite.

use crate::acp::r#impl::agent::ReviewGateOutcome;
use crate::governance::hardening::{
    rbac_fallback_allows_action, BudgetTracker, GovernanceAction, IdempotencyCache, SandboxLevel,
    SandboxPolicy,
};
use crate::governance::harness_bus::types::{
    DispatchPolicy, EscalationReason, ExecutionPolicy, GovernancePolicy, IdempotencyPolicy,
    OutputVerdict, PolicyVerdict, PolicyViolation, ReviewReason, ToolVerdict,
};
use crate::governance::pua::{PuaRuleEngine, TaskContext};
use crate::governance::rationalization::{RationalizationAnnotation, SelfRationalizationGuard};
use crate::governance::rbac::{AccessDecision, Permission, Principal, RbacEnforcer};
use crate::governance::review_controls::{review_verdict, verdict_as_str, verdict_is_approved};
use crate::governance::runtime_controls::OnlineControllerState;
use crate::governance::security_governor::{
    AuditEntry as SgAuditEntry, ConditionOperator, PolicyAction, PolicyComposition,
    PolicyCondition, SecurityGovernor, SecurityGovernorConfig, SecurityPolicy,
};
use crate::i18n::runtime::tf;
use crate::intelligence::quality_models::QualityVerdict;
use crate::security::severity::DetectionSeverity;

use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

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

    /// Tools that the user has explicitly approved (bypasses require_review).
    pub user_approved_tools: Mutex<std::collections::HashSet<String>>,

    /// Set to true when the most recent `evaluate()` call was blocked by
    /// the self-rationalization guard. Consumed by `HarnessBus::evaluate()`
    /// to record a rationalization block counter on the governance profile.
    pub(crate) rationalization_block_occurred: AtomicBool,
}

impl PolicyEvaluator {
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
            security_governor: Arc::new(SecurityGovernor::new(SecurityGovernorConfig {
                default_action: PolicyAction::Deny,
                default_policies: Self::default_security_policies(),
                ..Default::default()
            })),
            user_approved_tools: Mutex::new(std::collections::HashSet::new()),
            rationalization_block_occurred: AtomicBool::new(false),
        }
    }

    /// Return the built-in default security policies.
    pub fn default_security_policies() -> Vec<SecurityPolicy> {
        vec![
            // 1. read_allow — allow low-risk, read-only tasks
            SecurityPolicy {
                id: "read_allow".into(),
                name: "Allow read/search operations".into(),
                description:
                    "Permits read and search operations for zero-risk tasks with no file writes"
                        .into(),
                severity: DetectionSeverity::Low,
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
                severity: DetectionSeverity::Medium,
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
                severity: DetectionSeverity::High,
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

    /// Pre-route composite evaluation.
    /// Returns a PolicyVerdict that the caller (CapabilityBus) should respect.
    ///
    /// # Lock acquisition sequence
    ///
    /// This method acquires up to **6 locks sequentially** (worst-case path):
    ///
    /// | # | Lock | Kind | Scope |
    /// |---|------|------|-------|
    /// | 1 | `self.rule_engine` | `Mutex` | Steps 1–2 — red-line check + stage validation |
    /// | 2 | `self.budget` | `Mutex` | Step 3 — wall-clock budget check |
    /// | 3 | `self.runtime_control` | `Mutex` | Step 4 — adaptive sliding window / P95 / UCB escalate check (scoped, re-acquired at step 8) |
    /// | 4 | `self.guard` | `Mutex` | Step 6 — self-rationalization low-confidence guard (scoped) |
    /// | 5–6 | `security_governor` | internal | Step 7 — security policy `evaluate()` + `record_audit()` |
    ///
    /// **Deferred scoping**: Locks 3 and 4 are scoped to the narrowest possible
    /// block so they do not overlap with unrelated work (review gate at step 5,
    /// security governor at step 7). Lock 3 is re-acquired briefly at step 8
    /// to record the success outcome. All other locks are held for exactly one
    /// step and released before the next.
    ///
    /// Each critical section is documented to be brief (sub-millisecond).
    /// Replacing `std::sync::Mutex` with `tokio::sync::Mutex` would add overhead
    /// with no benefit since no critical section is held across an `.await` point.
    pub fn evaluate(&self, ctx: &TaskContext) -> PolicyVerdict {
        let _start = Instant::now();

        // NOTE: The former steps 1-2 (PUA red-line check on the TaskType
        // Debug string and stage validation against a hard-coded "default"
        // stage) were removed: they could never fire. The PUA red lines are
        // natural-language sentences, while this path only had the enum Debug
        // name (e.g. "SecurityPatch") to match against; and "default" is not
        // a stage in the enforcement plan. Real PUA red-line / stage checks
        // run in the ACP request layer (src/acp/impl/request.rs) where the
        // actual method name and inferred stage are available. The tool-call
        // red-line check below uses the configured `GovernancePolicy.red_lines`
        // against the actual tool arguments.

        // 1. Budget check (hard limit)
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
        //    Scope-limited: the MutexGuard is dropped immediately after the
        //    escalate check so it does not serialize subsequent unrelated steps
        //    (review gate at step 5, rationalization at step 6, security governor
        //    at step 7). Re-acquired briefly at step 8.
        let runtime_should_escalate = {
            let mut ctrl = self.runtime_control.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("[harness_bus] lock poisoned, recovering");
                poisoned.into_inner()
            });
            if ctrl.should_escalate() {
                ctrl.record(false, _start.elapsed().as_millis() as u64);
                true
            } else {
                false
            }
        };
        if runtime_should_escalate {
            return PolicyVerdict::Escalate(EscalationReason {
                reason: tf("error.harness_bus.runtime_escalation", &[]),
                suggested_level: 3,
            });
        }

        // 5. Review policy check (verify verdict from review_controls)
        if self.governance.quality_compass.enabled {
            tracing::debug!("review gate evaluating governance-driven review verdict");
        }
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
        let outcome = ReviewGateOutcome {
            passed: matches!(verdict, QualityVerdict::Approve),
            comments: vec![
                format!("governance-policy: {}", review_response),
                verdict_as_str(verdict),
            ],
            reviewer: "governance-policy".to_string(),
            duration_ms: 0,
            verdict,
        };
        let review_result = outcome.reviewer.as_str();
        tracing::debug!(
            reviewer = review_result,
            verdict = verdict_as_str(verdict),
            "review gate evaluated"
        );
        if !verdict_is_approved(verdict) {
            return PolicyVerdict::Review(ReviewReason {
                reason: tf("error.harness_bus.review_gate_manual", &[]),
            });
        }

        // 6. Self-rationalization guard (low confidence check)
        //    Scope-limited: the MutexGuard is dropped immediately after
        //    evaluate() so it does not overlap with the security governor step.
        let guard_blocked = {
            let mut guard = self.guard.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("[harness_bus] lock poisoned, recovering");
                poisoned.into_inner()
            });
            let mut annotation = RationalizationAnnotation::default();
            guard.evaluate(&mut annotation, ctx.risk_score as f32, false)
        };
        if guard_blocked {
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
            self.security_governor.record_audit_counters(&sg_entry);
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
        //    Re-acquire runtime_control briefly (the earlier scope at step 4
        //    already released the guard).
        {
            let mut ctrl = self.runtime_control.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("[harness_bus] lock poisoned, recovering");
                poisoned.into_inner()
            });
            ctrl.record(true, _start.elapsed().as_millis() as u64);
        }
        PolicyVerdict::Allow
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

    /// Whether idempotency dedup is enabled by the governance policy.
    fn idempotency_enabled(&self) -> bool {
        matches!(
            self.governance.idempotency,
            IdempotencyPolicy::Enabled { .. }
        )
    }

    /// Stable idempotency cache key for a (tool, arguments) pair.
    ///
    /// Uses the tenant-prefixed key format of [`IdempotencyCache`]
    /// (`"{tenant}:{operation_key}"`); the tenant is `default` so all tool
    /// entries share one per-tenant LRU quota. The argument JSON is hashed so
    /// two calls with different arguments never dedupe against each other.
    pub fn idempotency_key(tool: &str, args: &Value) -> String {
        use std::hash::{Hash, Hasher};
        let args_json = serde_json::to_string(args).unwrap_or_default();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        args_json.hash(&mut hasher);
        format!("default:{tool}:{:016x}", hasher.finish())
    }

    /// Retrieve the cached result for a repeated (tool, args) call, honoring
    /// both the cache-internal TTL and the `IdempotencyPolicy` TTL. This is
    /// the read point for `GovernancePolicy::idempotency` — previously the
    /// policy had no consumer.
    pub fn cached_idempotent_result(&self, tool: &str, args: &Value) -> Option<Value> {
        let (enabled, policy_ttl) = match self.governance.idempotency {
            IdempotencyPolicy::Enabled { ttl_seconds } => (true, Duration::from_secs(ttl_seconds)),
            IdempotencyPolicy::Disabled => (false, Duration::ZERO),
        };
        if !enabled {
            return None;
        }
        let key = Self::idempotency_key(tool, args);
        let cache = self.idempotency.lock().ok()?;
        let entry = cache.get(&key)?;
        if entry.cached_at.elapsed() > policy_ttl {
            return None;
        }
        Some(entry.response.clone())
    }

    /// Record a successful tool execution so a repeated (tool, args) call is
    /// deduplicated (skip re-execution, return the cached result) within the
    /// `IdempotencyPolicy` TTL.
    pub fn record_tool_success(&self, tool: &str, args: &Value, response: &Value) {
        if !self.idempotency_enabled() {
            return;
        }
        if let Ok(mut cache) = self.idempotency.lock() {
            cache.insert(Self::idempotency_key(tool, args), response.clone());
        }
    }

    pub fn check_tool_call(&self, tool: &str, args: &Value) -> ToolVerdict {
        let level = *self.sandbox_level.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("[harness_bus] lock poisoned, recovering");
            poisoned.into_inner()
        });

        // ── Governance red-line check on tool arguments ───────────────
        // The configured `GovernancePolicy.red_lines` (defaults: "rm -rf /",
        // "DROP TABLE", "DELETE FROM") are matched against the serialized
        // tool arguments. Previously this policy field was write-only — the
        // only red-line path matched the PUA plan's natural-language rules
        // against an enum Debug string that never matched.
        if !self.governance.red_lines.is_empty() {
            let args_text = serde_json::to_string(args).unwrap_or_default();
            let args_lower = args_text.to_ascii_lowercase();
            for line in &self.governance.red_lines {
                if !line.is_empty() && args_lower.contains(&line.to_ascii_lowercase()) {
                    tracing::warn!(
                        target: "harness_bus",
                        tool = %tool,
                        red_line = %line,
                        "governance red-line match in tool arguments; blocked"
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
        }

        // ── All tools categorized by operation type ────────────────
        //
        // Each tool is classified by its dominant operation class so that
        // the sandbox level check is fine-grained.  Tools not explicitly
        // listed fall through to require user review.
        use crate::governance::tool_capability::{ToolCapabilityRegistry, ToolOperation};
        let mut recognized = true;
        let allowed = match ToolCapabilityRegistry::operation(tool) {
            ToolOperation::Read => SandboxPolicy::can_execute_read_file(level),
            ToolOperation::Search => SandboxPolicy::can_execute_search(level),
            ToolOperation::Network => SandboxPolicy::can_execute_network(level),
            ToolOperation::Write => SandboxPolicy::can_execute_write(level),
            ToolOperation::Shell => SandboxPolicy::can_execute_shell(level),
            // ── Unknown tools — require user review (not auto-allowed) ─
            ToolOperation::Unknown => {
                // Check user-approved tools first (bypasses require_review)
                let is_approved = self
                    .user_approved_tools
                    .lock()
                    .map(|approved| approved.contains(tool))
                    .unwrap_or(false);
                if is_approved {
                    recognized = true;
                    true // User explicitly approved, bypass sandbox
                } else {
                    recognized = false;
                    false
                }
            }
        };
        let idempotent = match self.governance.idempotency {
            IdempotencyPolicy::Disabled => false,
            IdempotencyPolicy::Enabled { ttl_seconds } => {
                let policy_ttl = Duration::from_secs(ttl_seconds);
                self.idempotency
                    .lock()
                    .unwrap_or_else(|poisoned| {
                        tracing::warn!("[harness_bus] lock poisoned, recovering");
                        poisoned.into_inner()
                    })
                    .get(&Self::idempotency_key(tool, args))
                    .map(|entry| entry.cached_at.elapsed() <= policy_ttl)
                    .unwrap_or(false)
            }
        };
        let budget_ok = self
            .budget
            .lock()
            .unwrap_or_else(|poisoned| {
                tracing::warn!("[harness_bus] lock poisoned, recovering");
                poisoned.into_inner()
            })
            .record_tool_call()
            .is_ok();

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
    ///
    /// Validates that `output` is a well-formed JSON value, checks for expected
    /// structural fields, runs content safety and prompt injection checks,
    /// and logs the verification outcome.
    ///
    /// `stage` is the current PUA execution stage (e.g. `"verification"` for
    /// post-execute output checks). The stage drives the PUA evidence chain:
    /// `collect_evidence(stage)` / `collect_missing(stage, ...)` are evaluated
    /// against the real stage requirements so the harness genuinely checks the
    /// stage's required evidence instead of the no-op `"default"` stage that
    /// never matched any PUA plan requirement.
    pub fn verify_output(&self, output: &Value, stage: &str) -> OutputVerdict {
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
            self.security_governor.record_audit_counters(&audit_entry);
        }

        verdict
    }

    /// Permission check (delegates to RBAC enforcer when configured, otherwise
    /// applies an explicit fallback policy based on the active sandbox level).
    fn check_permission(&self, tool: &str, _args: &Value) -> bool {
        use crate::governance::tool_capability::ToolCapabilityRegistry;
        let action = ToolCapabilityRegistry::action(tool);

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
            let tenant_id = rbac.default_tenant_id();
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

    /// Resolve a raw response string into a governance-level review verdict.
    fn resolve_review_policy(response: &str, min_response_chars: usize) -> QualityVerdict {
        review_verdict(response, min_response_chars)
    }
}
