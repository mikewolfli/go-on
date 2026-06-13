//! HarnessBus — F-GAP-13
//!
//! Unified Strategy Engine (BLUE38 ARCH-13)
//!
//! HarnessBus is the **policy engine** that governs all capability invocations.
//! It aggregates every governance component (PuaRuleEngine, BudgetTracker,
//! SandboxPolicy, IdempotencyCache, AuditLogger, PolicyBundle, review controls,
//! runtime controls, self-rationalization guard) into a single evaluator that
//! the CapabilityBus calls before, during, and after every task.
//!
//! # Architecture
//!
//! ```text
//! HarnessBus (strategy engine)
//! ├── Policy Layer   — DispatchPolicy / ExecutionPolicy / GovernancePolicy
//! ├── Enforcement    — PolicyEvaluator (composite verdict: Allow/Deny/Escalate/Review/…)
//! │   ├── PuaRuleEngine (red lines, stage validation)
//! │   ├── BudgetTracker (token / clock / tool budgets)
//! │   ├── SandboxPolicy (file / shell / network permissions)
//! │   ├── IdempotencyCache (dedup)
//! │   ├── OnlineControllerState (adaptive runtime control)
//! │   └── SelfRationalizationGuard (low-confidence re-examine)
//! ├── Audit Layer    — AuditLogger + ProvenanceLedger
//! └── Feedback Layer — PuaFeedbackCollector + EscalationEngine
//! ```
//!
//! This file was split from a 2096-line GOD into 4 submodules:
//! - `types` — all policy type definitions (enums, structs, Default impls)
//! - `audit` — HarnessAuditTrail, PuaGovernanceProfile
//! - `evaluator` — PolicyEvaluator, PolicyFn
//! - `mod` — HarnessBus struct + impl + factory functions + re-exports

pub mod audit;
pub mod evaluator;
pub mod types;

// Re-export all public types from submodules for backward compatibility.
pub use audit::{HarnessAuditTrail, PuaGovernanceProfile};
pub use evaluator::{PolicyEvaluator, PolicyFn};
pub use types::{
    AgentExecutionPolicy, AuditConfig, AuditEntry, AuditLevel, CodeExecutionPolicy, Constraint,
    Decision, DegradationStrategy, DispatchPolicy, EscalationPolicy, EscalationReason,
    ExecutionMode, ExecutionPolicy, FailureStrategy, FallbackStrategy, FileWritePolicy,
    GovernancePolicy, IdempotencyPolicy, OutputVerdict, PolicyVerdict, PolicyViolation,
    QualityCompassConfig, ReviewLevel, ReviewReason, ReviewRequirement, RoutingStrategy,
    TimeoutPolicy, ToolUsagePolicy, ToolVerdict, VersionCompatPolicy,
};

use crate::fault_tolerance::{FaultToleranceConfig, FaultToleranceEngine, FaultToleranceProfile};
use crate::governance::audit::{AuditLogEntry, ThreadSafeAuditLog};
use crate::governance::drift::drift_protection::{
    DriftProfile, DriftProtectionConfig, DriftProtectionEngine,
};
use crate::governance::hardening::{
    BudgetTracker, GovernanceAction, IdempotencyCache, PolicyBundle, SandboxLevel, TaskBudget,
};
use crate::governance::pua::{PuaFeedbackCollector, PuaRuleEngine, TaskContext};
use crate::governance::rationalization::SelfRationalizationGuard;
use crate::governance::rbac::RbacEnforcer;
use crate::governance::reloadable_policy::PolicyReloader;
use crate::governance::runtime_controls::OnlineControllerState;
use crate::i18n::runtime::tf;
use crate::orchestration::artifact::{ArtifactLayer, ArtifactProfile};
use crate::orchestration::brain_loop::{BrainLoop, BrainLoopConfig, BrainLoopProfile};
use crate::orchestration::omnipotent::{OmnipotentMode, OmnipotentProfile};
use crate::orchestration::promotion_plugin::PromotionRegistry;
use crate::orchestration::token_layers::{
    estimate_cost, GateContext, TokenCostEstimate, TokenGateVerdict, TokenLayerChain,
};
use crate::resilience::hyper_resilience::{
    HyperResilienceEngine, ResilienceConfig, ResilienceProfile,
};

use serde_json::Value;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

// ---------------------------------------------------------------------------
// HarnessBus — top-level strategy engine
// ---------------------------------------------------------------------------

/// HarnessBus aggregates every governance component and exposes the
/// PolicyEvaluator as its primary interface.
pub struct HarnessBus {
    pub evaluator: PolicyEvaluator,
    pub audit_trail: Arc<Mutex<HarnessAuditTrail>>,
    pub feedback_collector: Option<PuaFeedbackCollector>,
    pub profile: Arc<Mutex<PuaGovernanceProfile>>,
    pub drift_engine: Arc<DriftProtectionEngine>,
    pub brain_loop: Arc<BrainLoop>,
    pub artifact_layer: Arc<ArtifactLayer>,
    pub omnipotent_mode: Arc<OmnipotentMode>,
    pub promotion_registry: Arc<Mutex<PromotionRegistry>>,
    pub token_chain: Arc<Mutex<TokenLayerChain>>,
    /// Second brain loop instance for runner profile snapshots (consolidated with flat version).
    pub brain_runner: Arc<BrainLoop>,
    /// Hyper-resilience engine — circuit breakers, failover, self-healing (F-GAP-27)
    pub resilience_engine: Arc<HyperResilienceEngine>,
    /// Fault tolerance engine — node isolation, heartbeat detection (F-GAP-28)
    pub fault_tolerance: Arc<FaultToleranceEngine>,
    /// Structured audit trail for replay and evidence export (dual system integration).
    pub structured_audit_trail: Arc<Mutex<crate::orchestration::audit::AuditTrail>>,
    /// Canonical thread-safe audit log with NDJSON persistence (canonical sink).
    pub audit_log: Arc<ThreadSafeAuditLog>,
    /// Consecutive allow-count for PUA de-escalation.
    consecutive_allows: AtomicU32,
}

impl HarnessBus {
    /// Construct a HarnessBus with default policies and the provided governance components.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        rule_engine: Arc<Mutex<PuaRuleEngine>>,
        sandbox_level: Arc<Mutex<SandboxLevel>>,
        budget: Arc<Mutex<BudgetTracker>>,
        idempotency: Arc<Mutex<IdempotencyCache>>,
        runtime_control: Arc<Mutex<OnlineControllerState>>,
        guard: Arc<Mutex<SelfRationalizationGuard>>,
        storage_path: Option<PathBuf>,
        audit_log: Arc<ThreadSafeAuditLog>,
        external_resilience_engine: Option<Arc<HyperResilienceEngine>>,
        policy_reloader: Option<Arc<Mutex<PolicyReloader>>>,
    ) -> Self {
        let feedback_collector = storage_path.map(PuaFeedbackCollector::new);
        let bus = HarnessBus {
            evaluator: PolicyEvaluator::new(
                rule_engine,
                sandbox_level,
                budget,
                idempotency,
                runtime_control,
                guard,
                policy_reloader,
            ),
            audit_trail: Arc::new(Mutex::new(HarnessAuditTrail::default())),
            feedback_collector,
            profile: Arc::new(Mutex::new(PuaGovernanceProfile::default())),
            drift_engine: Arc::new(DriftProtectionEngine::new(DriftProtectionConfig::default())),
            brain_loop: Arc::new(BrainLoop::new(BrainLoopConfig::default())),
            artifact_layer: Arc::new(ArtifactLayer::new()),
            omnipotent_mode: Arc::new(OmnipotentMode::new()),
            promotion_registry: Arc::new(Mutex::new(PromotionRegistry::new())),
            token_chain: Arc::new(Mutex::new(TokenLayerChain::new())),
            brain_runner: Arc::new(BrainLoop::new(BrainLoopConfig::default())),
            resilience_engine: external_resilience_engine.unwrap_or_else(|| {
                Arc::new(HyperResilienceEngine::new(ResilienceConfig::default()))
            }),
            fault_tolerance: Arc::new(FaultToleranceEngine::new(FaultToleranceConfig::default())),
            structured_audit_trail: Arc::new(Mutex::new(
                crate::orchestration::audit::AuditTrail::new("harness-bus", 1000),
            )),
            audit_log,
            consecutive_allows: AtomicU32::new(0),
        };

        // Start background health checks for the resilience engine.
        {
            let engine = Arc::clone(&bus.resilience_engine);
            tokio::spawn(async move {
                engine.start_health_checks().await;
            });
        }

        // GAP-B58-C16: Start drift monitor (checks for metric drift every 60 seconds).
        bus.start_drift_monitor(60);

        bus
    }

    /// Pre-route evaluation — primary entry point called by CapabilityBus.
    pub async fn evaluate(&self, ctx: &TaskContext) -> PolicyVerdict {
        let start = Instant::now();
        let verdict = self.evaluator.evaluate(ctx);
        let elapsed = start.elapsed().as_millis() as u64;

        // Profile and runtime-control updates (scope ensures guards are dropped
        // before the async call below).
        {
            let mut p = self.profile.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("[harness_bus] lock poisoned, recovering");
                poisoned.into_inner()
            });
            p.total_evaluations = p.total_evaluations.saturating_add(1);
            p.last_evaluation_ms = elapsed;
            match &verdict {
                PolicyVerdict::Allow => p.allow_count = p.allow_count.saturating_add(1),
                PolicyVerdict::Deny(v) => {
                    p.deny_count = p.deny_count.saturating_add(1);
                    match v.kind.as_str() {
                        "red_line" => {
                            p.red_line_blocks = p.red_line_blocks.saturating_add(1);
                            let engine =
                                self.evaluator
                                    .rule_engine
                                    .lock()
                                    .unwrap_or_else(|poisoned| {
                                        tracing::warn!("[harness_bus] lock poisoned, recovering");
                                        poisoned.into_inner()
                                    });
                            let level = engine
                                .escalate(&format!("Red-line violation detected: {}", v.detail));
                            tracing::info!(
                                new_level = level,
                                detail = %v.detail,
                                "PUA auto-escalated due to red-line violation"
                            );
                        }
                        "budget" => p.budget_violations = p.budget_violations.saturating_add(1),
                        "rbac" | "permission" => {
                            p.rbac_denials = p.rbac_denials.saturating_add(1);
                        }
                        "security_policy" => {
                            p.security_blocks = p.security_blocks.saturating_add(1);
                        }
                        _ => p.other_denials = p.other_denials.saturating_add(1),
                    }
                }
                PolicyVerdict::Escalate(_) => p.escalate_count = p.escalate_count.saturating_add(1),
                PolicyVerdict::Review(_) => p.review_count = p.review_count.saturating_add(1),
                PolicyVerdict::AllowWithConstraints(_) => {
                    p.allow_count = p.allow_count.saturating_add(1)
                }
            }
            let ctrl = self
                .evaluator
                .runtime_control
                .lock()
                .unwrap_or_else(|poisoned| {
                    tracing::warn!("[harness_bus] lock poisoned, recovering");
                    poisoned.into_inner()
                });
            p.current_escalation_level = ctrl.control_mode();
            p.runtime_control_mode = if ctrl.should_escalate() {
                tf("status.harness_bus.mode_restricted", &[])
            } else {
                tf("status.harness_bus.mode_standard", &[])
            };
            p.policy_violation_trend = ctrl.violation_trend();
            p.current_active_policies = 12u32;
        }

        // Record execution outcome through the resilience engine (F-GAP-27).
        let success = matches!(
            &verdict,
            PolicyVerdict::Allow
                | PolicyVerdict::AllowWithConstraints(_)
                | PolicyVerdict::Review(_)
        );
        self.resilience_engine
            .record_execution("harness-main", success)
            .await;

        // PUA de-escalation: after 3 consecutive clean evaluations, de-escalate.
        match &verdict {
            PolicyVerdict::Allow | PolicyVerdict::AllowWithConstraints(_) => {
                let prev = self.consecutive_allows.fetch_add(1, Ordering::SeqCst);
                if prev >= 2 {
                    self.consecutive_allows.store(0, Ordering::SeqCst);
                    let engine = self
                        .evaluator
                        .rule_engine
                        .lock()
                        .unwrap_or_else(|poisoned| {
                            tracing::warn!("[harness_bus] lock poisoned, recovering");
                            poisoned.into_inner()
                        });
                    let level =
                        engine.de_escalate("No violations detected for 3 consecutive evaluations");
                    tracing::info!(
                        new_level = level,
                        "PUA de-escalated after 3 consecutive clean evaluations"
                    );
                }
            }
            _ => {
                self.consecutive_allows.store(0, Ordering::SeqCst);
            }
        }

        // B51-32: Record evaluation outcome to the canonical ThreadSafeAuditLog.
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let audit_entry = AuditLogEntry {
            timestamp: format!("{}", now_ms),
            task_id: format!("{:?}", ctx.task_type),
            phase: "pre_route".to_string(),
            agent: None,
            tool: None,
            decision: format!("{:?}", verdict),
            inputs: serde_json::json!({
                "task_type": format!("{:?}", ctx.task_type),
                "file_count": ctx.file_count,
                "risk_score": ctx.risk_score,
            }),
            outputs: None,
            error: None,
            confidence: None,
            data_classification: None,
            compliance_tags: vec![],
            retention_policy: None,
            correlation_id: None,
        };
        self.audit_log.record(audit_entry);

        verdict
    }

    /// Pre-tool-call validation.
    pub fn validate_action(&self, tool: &str, args: &Value) -> ToolVerdict {
        let verdict = self.evaluator.check_tool_call(tool, args);
        let mut p = self.profile.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("[harness_bus] lock poisoned, recovering");
            poisoned.into_inner()
        });
        if !verdict.allowed {
            p.sandbox_denials = p.sandbox_denials.saturating_add(1);
        }
        if verdict.idempotent {
            p.idempotency_hits = p.idempotency_hits.saturating_add(1);
        }
        verdict
    }

    /// Post-execution output verification with audit recording.
    pub fn verify_output(&self, output: &Value) -> OutputVerdict {
        let verdict = self.evaluator.verify_output(output);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let audit_entry = AuditEntry {
            timestamp: now_ms,
            request_id: String::new(),
            stage: "verify_output".to_string(),
            verdict: if verdict.quality { "allow" } else { "deny" }.to_string(),
            dispatch_policy: String::new(),
            execution_policy: String::new(),
            governance_policy: String::new(),
            violations: if verdict.quality {
                vec![]
            } else {
                vec!["output_verification_failed".to_string()]
            },
            context_snapshot: serde_json::json!({
                "risk_score": verdict.risk_score,
                "evidence_count": verdict.evidence.len(),
            }),
        };
        self.audit(audit_entry);
        verdict
    }

    /// Record an audit entry.
    pub fn audit(&self, entry: AuditEntry) {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        self.audit_log.record(AuditLogEntry {
            timestamp: format!("{}", now_ms),
            task_id: entry.request_id.clone(),
            phase: format!("harness_audit:{}", entry.stage),
            agent: None,
            tool: None,
            decision: entry.verdict.clone(),
            inputs: serde_json::json!({
                "dispatch_policy": &entry.dispatch_policy,
                "execution_policy": &entry.execution_policy,
                "governance_policy": &entry.governance_policy,
                "violations": &entry.violations,
                "context_snapshot": &entry.context_snapshot,
            }),
            outputs: None,
            error: None,
            confidence: None,
            data_classification: None,
            compliance_tags: vec![],
            retention_policy: None,
            correlation_id: None,
        });

        self.audit_trail
            .lock()
            .unwrap_or_else(|poisoned| {
                tracing::warn!("[harness_bus] lock poisoned, recovering");
                poisoned.into_inner()
            })
            .entries
            .push(entry.clone());

        let mut structured = self
            .structured_audit_trail
            .lock()
            .unwrap_or_else(|poisoned| {
                tracing::warn!("[harness_bus] lock poisoned, recovering");
                poisoned.into_inner()
            });
        {
            use crate::orchestration::audit::AuditEntry as OrchestrationAuditEntry;
            structured.append_entry(OrchestrationAuditEntry::new(
                "harness_audit",
                &entry.request_id,
                &entry.stage,
                serde_json::json!({
                    "verdict": &entry.verdict,
                    "dispatch_policy": &entry.dispatch_policy,
                    "execution_policy": &entry.execution_policy,
                    "governance_policy": &entry.governance_policy,
                    "violations": &entry.violations,
                }),
                serde_json::json!({
                    "context_snapshot": &entry.context_snapshot,
                }),
            ));
        }

        let mut p = self.profile.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("[harness_bus] lock poisoned, recovering");
            poisoned.into_inner()
        });
        p.audit_entries_total = p.audit_entries_total.saturating_add(1);
    }

    /// Build a per-agent execution policy from the three base policies.
    pub fn get_agent_policy(&self, _agent: &str, _task_type: &str) -> AgentExecutionPolicy {
        AgentExecutionPolicy {
            timeout: self.evaluator.dispatch.timeout_policy.default_timeout,
            max_tool_calls: self.evaluator.execution.budget.max_tool_calls as u32,
            allow_file_write: self.evaluator.governance.sandbox_level.level_index() as usize <= 1,
            allow_shell: self.evaluator.governance.sandbox_level.level_index() as usize == 0,
            allow_network: true,
            review_level: if self.evaluator.governance.sandbox_level.level_index() as usize >= 2 {
                ReviewLevel::Manual
            } else {
                ReviewLevel::Auto
            },
            audit_level: if self.evaluator.governance.sandbox_level.level_index() as usize >= 2 {
                AuditLevel::Verbose
            } else {
                AuditLevel::Standard
            },
            failure_strategy: FailureStrategy::Retry,
            max_retries: self.evaluator.dispatch.max_retries,
            degradation: DegradationStrategy::None,
            max_tokens: self.evaluator.execution.budget.max_tokens,
        }
    }

    /// Snapshot of the HarnessBus governance profile.
    pub fn governance_profile(&self) -> PuaGovernanceProfile {
        self.profile
            .lock()
            .unwrap_or_else(|poisoned| {
                tracing::warn!("[harness_bus] lock poisoned, recovering");
                poisoned.into_inner()
            })
            .clone()
    }

    /// Check a red-line violation directly.
    pub fn check_red_line(&self, action: &str) -> bool {
        let engine = self
            .evaluator
            .rule_engine
            .lock()
            .unwrap_or_else(|poisoned| {
                tracing::warn!("[harness_bus] lock poisoned, recovering");
                poisoned.into_inner()
            });
        engine.check_red_lines(action).is_err()
    }

    /// SelfRationalizationGuard governance profile snapshot.
    pub fn self_rationalization_profile(&self, enabled: bool) -> serde_json::Value {
        self.evaluator
            .guard
            .lock()
            .unwrap_or_else(|poisoned| {
                tracing::warn!("[harness_bus] lock poisoned, recovering");
                poisoned.into_inner()
            })
            .governance_profile(enabled)
    }

    /// PolicyBundle compliance check for a GovernanceAction.
    pub fn enforce_action(&self, action: &GovernanceAction, policy_bundle: &PolicyBundle) -> bool {
        match action {
            GovernanceAction::Read => {
                let level = self
                    .evaluator
                    .sandbox_level
                    .lock()
                    .unwrap_or_else(|poisoned| {
                        tracing::warn!("[harness_bus] lock poisoned, recovering");
                        poisoned.into_inner()
                    });
                if *level == SandboxLevel::Isolated {
                    return false;
                }
                true
            }
            GovernanceAction::Search => {
                let level = self
                    .evaluator
                    .sandbox_level
                    .lock()
                    .unwrap_or_else(|poisoned| {
                        tracing::warn!("[harness_bus] lock poisoned, recovering");
                        poisoned.into_inner()
                    });
                if *level == SandboxLevel::Strict || *level == SandboxLevel::Isolated {
                    return false;
                }
                true
            }
            GovernanceAction::Write => !policy_bundle.require_approval_for_write,
            GovernanceAction::Shell => policy_bundle.enable_code_execution,
        }
    }

    /// Drift protection profile snapshot (F-GAP-26).
    pub fn drift_profile(&self) -> DriftProfile {
        self.drift_engine.profile()
    }

    /// Brain loop orchestration profile snapshot.
    pub async fn brain_profile(&self) -> BrainLoopProfile {
        self.brain_loop.profile().await
    }

    /// Artifact layer profile snapshot.
    pub fn artifact_profile(&self) -> ArtifactProfile {
        self.artifact_layer.profile()
    }

    /// Omnipotent mode profile snapshot.
    pub fn omnipotent_profile(&self) -> OmnipotentProfile {
        self.omnipotent_mode.profile()
    }

    /// Number of registered promotion plugins.
    pub fn promotion_plugin_count(&self) -> usize {
        self.promotion_registry
            .lock()
            .unwrap_or_else(|poisoned| {
                tracing::warn!("[harness_bus] lock poisoned, recovering");
                poisoned.into_inner()
            })
            .plugin_count()
    }

    /// Evaluate a token gate request through the L0-L5 chain.
    pub fn evaluate_token_gate(&self, ctx: &GateContext) -> TokenGateVerdict {
        self.token_chain
            .lock()
            .unwrap_or_else(|poisoned| {
                tracing::warn!("[harness_bus] lock poisoned, recovering");
                poisoned.into_inner()
            })
            .evaluate(ctx)
    }

    /// Brain loop runner profile snapshot (consolidated flat version).
    pub async fn brain_runner_profile(&self) -> BrainLoopProfile {
        self.brain_runner.profile().await
    }

    /// Hyper-resilience profile snapshot (F-GAP-27)
    pub async fn resilience_profile(&self) -> ResilienceProfile {
        self.resilience_engine.profile().await
    }

    /// Fault tolerance profile snapshot (F-GAP-28)
    pub async fn fault_tolerance_profile(&self) -> FaultToleranceProfile {
        self.fault_tolerance.profile().await
    }

    /// Estimate token cost for a given input/output token count pair.
    pub fn token_cost_estimate(
        &self,
        input: u64,
        output: u64,
        cost_per_1k: f64,
    ) -> TokenCostEstimate {
        estimate_cost(input, output, cost_per_1k)
    }

    /// Review gate prompt for LLM-based approval (PUA-wired).
    pub fn review_gate_prompt(&self) -> String {
        crate::governance::pua::review_gate_prompt()
    }

    /// Evaluate and decide the work grade for a task context.
    pub fn decide_work_grade_for_task(
        &self,
        requested_grade: &str,
        task_complexity: f64,
    ) -> serde_json::Value {
        use crate::acp::helpers::policy::decide_work_grade;

        let plan = crate::reinforcement::TaskPlanArtifact {
            generated_at: 0,
            task: String::new(),
            characteristics: crate::orchestration::task_router::TaskCharacteristics {
                description: String::new(),
                task_type: crate::orchestration::task_router::TaskType::Unknown,
                complexity: task_complexity.min(5.0) as u8,
                required_capabilities: vec![],
                involves_multiple_modules: false,
                is_time_critical: false,
                needs_verification: false,
                has_safety_concerns: false,
            },
            routing: crate::orchestration::task_router::RoutingDecision {
                roles: vec![],
                requirements: vec![],
                predicted_success_rate: 0.95,
                estimated_duration_seconds: 0,
                can_parallelize: vec![],
                risk_factors: vec![],
                recommended_safeguards: vec![],
                pua_enforcement: crate::governance::pua::PuaEnforcementPlan::default(),
            },
            decomposition: None,
            planned_subtasks: vec![],
            sub_agent_recommended: false,
            activation_reasons: vec![],
            action_checks_required: vec![],
        };

        let decision = decide_work_grade(Some(requested_grade), &plan, true, false, false);

        serde_json::json!({
            "requested": decision.requested.as_str(),
            "decided": decision.decided.as_str(),
            "decision_action": decision.decision_action,
            "reasons": decision.reasons,
            "risk_score": decision.risk_score,
        })
    }

    /// Evaluate optimization policy for a task.
    pub fn evaluate_optimization_for_task(&self, _task_type: &str) -> serde_json::Value {
        use crate::acp::helpers::policy::evaluate_optimization_policy;
        use crate::governance::pua::PuaEnforcementPlan;
        use crate::intelligence::reinforcement::ArtifactLedger;
        use crate::orchestration::task_router::{RoutingDecision, TaskCharacteristics, TaskType};
        use crate::reinforcement::TaskPlanArtifact;

        let ledger = ArtifactLedger::new(None);
        let plan = TaskPlanArtifact {
            generated_at: 0,
            task: String::new(),
            characteristics: TaskCharacteristics {
                description: String::new(),
                task_type: TaskType::Unknown,
                complexity: 0,
                required_capabilities: vec![],
                involves_multiple_modules: false,
                is_time_critical: false,
                needs_verification: false,
                has_safety_concerns: false,
            },
            routing: RoutingDecision {
                roles: vec![],
                requirements: vec![],
                predicted_success_rate: 0.95,
                estimated_duration_seconds: 0,
                can_parallelize: vec![],
                risk_factors: vec![],
                recommended_safeguards: vec![],
                pua_enforcement: PuaEnforcementPlan::default(),
            },
            decomposition: None,
            planned_subtasks: vec![],
            sub_agent_recommended: false,
            activation_reasons: vec![],
            action_checks_required: vec![],
        };
        let outcome = evaluate_optimization_policy(&ledger, "", &plan, None, true, true);

        serde_json::json!({
            "auto_attach": outcome.report.auto_attach,
            "auto_detach": outcome.report.auto_detach,
            "runtime_healthy": outcome.report.runtime_healthy,
            "anomaly_detected": outcome.report.anomaly_detected,
            "phase_parallelism_cap": outcome.phase_parallelism_cap,
            "force_fail_fast": outcome.force_fail_fast,
        })
    }

    /// Update the sandbox level at runtime.
    pub fn set_sandbox_level(&self, level: SandboxLevel) {
        *self
            .evaluator
            .sandbox_level
            .lock()
            .unwrap_or_else(|poisoned| {
                tracing::warn!("[harness_bus] lock poisoned, recovering");
                poisoned.into_inner()
            }) = level;
    }

    /// Inject a shared Arc RBAC enforcer into the policy evaluator.
    pub fn set_rbac_enforcer(&self, enforcer: Arc<RwLock<RbacEnforcer>>) {
        self.evaluator.set_rbac_enforcer(enforcer);
    }

    /// Extract an optional `u64` value from a JSON options map.
    pub fn extra_u64(&self, options: &serde_json::Value, key: &str) -> Option<u64> {
        options.get(key).and_then(|v| v.as_u64())
    }

    /// Extract an optional `f64` value from a JSON options map.
    pub fn extra_f64(&self, options: &serde_json::Value, key: &str) -> Option<f64> {
        options.get(key).and_then(|v| v.as_f64())
    }

    /// Start a background tokio task that periodically checks for drift.
    pub fn start_drift_monitor(&self, interval_secs: u64) {
        let interval = std::time::Duration::from_secs(interval_secs);
        let engine = self.drift_engine.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                let alerts = engine.check_for_drift();
                for alert in alerts {
                    if !alert.resolved {
                        let alert_id = &alert.id;
                        let metric_name = &alert.metric_name;
                        let drift_type = format!("{:?}", alert.drift_type);
                        let severity = format!("{:?}", alert.severity);
                        tracing::warn!(
                            target: "drift_monitor",
                            alert_id = %alert_id,
                            metric = %metric_name,
                            drift_type = %drift_type,
                            "Drift alert triggered: {} (severity: {})",
                            alert.message,
                            severity,
                        );
                    }
                }
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Factory helpers
// ---------------------------------------------------------------------------

/// Build a default HarnessBus with basic policy defaults for local-dev profile.
pub fn default_harness_bus(storage_path: Option<PathBuf>) -> HarnessBus {
    let pua_plan = Arc::new(Mutex::new(
        crate::governance::pua::PuaEnforcementPlan::default(),
    ));
    let rule_engine = Arc::new(Mutex::new(PuaRuleEngine::new(pua_plan)));
    let sandbox_level = Arc::new(Mutex::new(SandboxLevel::None));
    let budget = Arc::new(Mutex::new(BudgetTracker::new(TaskBudget {
        max_tokens: 120_000,
        max_wall_clock_seconds: 3600,
        max_tool_calls: 256,
        max_api_calls: 256,
    })));
    let idempotency = Arc::new(Mutex::new(IdempotencyCache::new(Duration::from_secs(3600))));
    let runtime_control = Arc::new(Mutex::new(OnlineControllerState::default()));
    let guard = Arc::new(Mutex::new(SelfRationalizationGuard::new(0.6)));
    let audit_log = Arc::new(ThreadSafeAuditLog::new_with_default_path(10_000));

    HarnessBus::new(
        rule_engine,
        sandbox_level,
        budget,
        idempotency,
        runtime_control,
        guard,
        storage_path,
        audit_log,
        None,
        None,
    )
}

/// Build a HarnessBus with policies derived from `AppConfig` sections.
pub fn config_aware_harness_bus(
    config: &crate::config::AppConfig,
    storage_path: Option<PathBuf>,
) -> HarnessBus {
    let pua_plan = Arc::new(Mutex::new(
        crate::governance::pua::PuaEnforcementPlan::default(),
    ));
    let rule_engine = Arc::new(Mutex::new(PuaRuleEngine::new(pua_plan)));

    let sandbox_level = Arc::new(Mutex::new(
        config
            .compliance
            .as_ref()
            .map(|c| {
                if c.enabled {
                    if c.standards
                        .iter()
                        .any(|s| s.eq_ignore_ascii_case("hipaa") || s.eq_ignore_ascii_case("pci"))
                    {
                        SandboxLevel::Strict
                    } else {
                        SandboxLevel::Basic
                    }
                } else {
                    SandboxLevel::None
                }
            })
            .unwrap_or(SandboxLevel::None),
    ));

    let worker_factor = config
        .scheduler
        .as_ref()
        .map(|s| {
            if s.enabled {
                (s.worker_slots.max(1) as u64).min(16)
            } else {
                1u64
            }
        })
        .unwrap_or(1u64);

    let budget = Arc::new(Mutex::new(BudgetTracker::new(TaskBudget {
        max_tokens: (120_000 * worker_factor) as usize,
        max_wall_clock_seconds: 3600 * worker_factor,
        max_tool_calls: (256 * worker_factor) as usize,
        max_api_calls: (256 * worker_factor) as usize,
    })));

    let idempotency = Arc::new(Mutex::new(IdempotencyCache::new(Duration::from_secs(3600))));

    let runtime_control = Arc::new(Mutex::new(OnlineControllerState::default()));
    let guard = Arc::new(Mutex::new(SelfRationalizationGuard::new(0.6)));
    let audit_log = Arc::new(ThreadSafeAuditLog::new_with_default_path(10_000));

    HarnessBus::new(
        rule_engine,
        sandbox_level,
        budget,
        idempotency,
        runtime_control,
        guard,
        storage_path,
        audit_log,
        None,
        None,
    )
}

// Re-export Duration for factory functions
use std::time::Duration;
