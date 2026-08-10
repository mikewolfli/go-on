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
//! - `evaluator` — PolicyEvaluator
//! - `mod` — HarnessBus struct + impl + factory functions + re-exports

pub mod audit;
pub mod evaluator;
pub mod types;

// Re-export all public types from submodules for backward compatibility.
pub use audit::PuaGovernanceProfile;
pub use evaluator::PolicyEvaluator;
pub use types::{
    AgentExecutionPolicy, AuditConfig, AuditLevel, CodeExecutionPolicy, Constraint,
    DegradationStrategy, DispatchPolicy, EscalationPolicy, EscalationReason, ExecutionMode,
    ExecutionPolicy, FailureStrategy, FallbackStrategy, FileWritePolicy, GovernancePolicy,
    IdempotencyPolicy, OutputVerdict, PolicyVerdict, PolicyViolation, QualityCompassConfig,
    ReviewLevel, ReviewReason, RoutingStrategy, TimeoutPolicy, ToolUsagePolicy, ToolVerdict,
    VersionCompatPolicy,
};

use crate::fault_tolerance::{FaultToleranceConfig, FaultToleranceEngine, FaultToleranceProfile};
use crate::governance::audit::{AuditLogEntry, ThreadSafeAuditLog};
use crate::governance::drift::drift_protection::{
    DriftPolicy, DriftProfile, DriftProtectionConfig, DriftProtectionEngine, DriftType,
};
use crate::governance::hardening::{BudgetTracker, IdempotencyCache, SandboxLevel, TaskBudget};
use crate::governance::pua::{PuaRuleEngine, TaskContext};
use crate::governance::rationalization::SelfRationalizationGuard;
use crate::governance::rbac::RbacEnforcer;
use crate::governance::runtime_controls::OnlineControllerState;
use crate::i18n::runtime::tf;
use crate::resilience::hyper_resilience::{
    HyperResilienceEngine, ResilienceConfig, ResilienceProfile,
};

use serde_json::Value;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

// ---------------------------------------------------------------------------
// HarnessBus — top-level strategy engine
// ---------------------------------------------------------------------------

/// HarnessBus aggregates every governance component and exposes the
/// PolicyEvaluator as its primary interface.
pub struct HarnessBus {
    pub evaluator: Arc<PolicyEvaluator>,
    pub profile: Arc<Mutex<PuaGovernanceProfile>>,
    pub drift_engine: Arc<DriftProtectionEngine>,
    /// Hyper-resilience engine — circuit breakers, failover, self-healing (F-GAP-27)
    pub resilience_engine: Arc<HyperResilienceEngine>,
    /// Fault tolerance engine — node isolation, heartbeat detection (F-GAP-28)
    pub fault_tolerance: Arc<FaultToleranceEngine>,
    /// Canonical thread-safe audit log with NDJSON persistence (single sink).
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
        audit_log: Arc<ThreadSafeAuditLog>,
        external_resilience_engine: Option<Arc<HyperResilienceEngine>>,
    ) -> Self {
        let bus = HarnessBus {
            evaluator: Arc::new(PolicyEvaluator::new(
                rule_engine,
                sandbox_level,
                budget,
                idempotency,
                runtime_control,
                guard,
            )),
            profile: Arc::new(Mutex::new(PuaGovernanceProfile::default())),
            drift_engine: Arc::new(DriftProtectionEngine::new(DriftProtectionConfig::default())),

            resilience_engine: external_resilience_engine.unwrap_or_else(|| {
                Arc::new(HyperResilienceEngine::new(ResilienceConfig::default()))
            }),
            fault_tolerance: Arc::new(FaultToleranceEngine::new_with_restore(
                FaultToleranceConfig::default(),
            )),
            audit_log,
            consecutive_allows: AtomicU32::new(0),
        };

        // Wire the process-wide tool-execution reporting hook into the bus's
        // engine so orchestration-layer tool circuit-breaker trips (the tool
        // executor cannot structurally reach the engine) surface in
        // health/governance status. `record_outcome` is the sync engine entry
        // point — it drives both the per-service health monitor and the
        // breaker state machine. First installation wins (OnceLock).
        crate::resilience::hyper_resilience::set_tool_execution_report_hook({
            let engine = Arc::clone(&bus.resilience_engine);
            move |breaker_name: String, success: bool| {
                engine.record_outcome(&breaker_name, success, 0);
            }
        });

        // The drift monitor is NOT started here: `HarnessBus::new` is a
        // synchronous constructor that may run outside a tokio runtime (tests,
        // CLI paths), and spawning a perpetual background task per construction
        // would leak one monitor per instance. The server startup path calls
        // `start_drift_monitor` explicitly once.

        // Wire a real drift policy so the monitor has thresholds to evaluate.
        // Metrics are fed from the hot validation paths (see validate_action /
        // verify_output): latency regressions are Performance drift.
        let _ = bus.drift_engine.register_policy(DriftPolicy {
            name: "harness_validation_latency".to_string(),
            drift_types: vec![DriftType::Performance],
            warning_threshold: 0.5,
            critical_threshold: 0.8,
            breach_threshold: 0.95,
            cooldown_ms: 60_000,
            auto_remediate: false,
        });

        bus
    }

    /// Pre-route evaluation — primary entry point called by CapabilityBus.
    ///
    /// The actual PolicyEvaluator::evaluate() call (which acquires up to 6 locks
    /// sequentially) is moved to spawn_blocking to avoid blocking the tokio
    /// runtime thread — BLUE69 principle #23.
    pub async fn evaluate(&self, ctx: &TaskContext) -> PolicyVerdict {
        let start = Instant::now();
        let evaluator = self.evaluator.clone();
        let ctx_for_blocking = ctx.clone();
        let verdict = tokio::task::spawn_blocking(move || evaluator.evaluate(&ctx_for_blocking))
            .await
            .unwrap_or_else(|join_err| {
                tracing::error!(
                    error = %join_err,
                    "PolicyEvaluator::evaluate() panicked in blocking thread"
                );
                PolicyVerdict::Deny(PolicyViolation {
                    kind: "internal_error".to_string(),
                    detail: "Policy evaluation thread panicked".to_string(),
                })
            });
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
            // Record rationalization blocks by draining the flag set in the evaluator.
            if self.evaluator.drain_rationalization_blocked() {
                p.record_rationalization_block();
            }
            match &verdict {
                PolicyVerdict::Allow => p.allow_count = p.allow_count.saturating_add(1),
                PolicyVerdict::Deny(v) => {
                    p.deny_count = p.deny_count.saturating_add(1);
                    match v.kind.as_str() {
                        "red_line" => {
                            p.red_line_blocks = p.red_line_blocks.saturating_add(1);
                            p.record_hardening_event();
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
                            p.record_rbac_denial();
                        }
                        "security_policy" => {
                            p.record_security_block();
                        }
                        _ => p.other_denials = p.other_denials.saturating_add(1),
                    }
                }
                PolicyVerdict::Escalate(_) => {
                    p.escalate_count = p.escalate_count.saturating_add(1);
                    p.record_hardening_event();
                }
                PolicyVerdict::Review(_) => {
                    p.review_count = p.review_count.saturating_add(1);
                    // The review gate is the primary approval workflow in the
                    // current pipeline — record as an approval request.
                    p.record_approval_request();
                }
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
            // Honest active-policy count: the security-governor policy table
            // plus the five fixed policy bundles wired into the evaluator
            // (dispatch / execution / governance / PUA enforcement / quality
            // compass). Previously hardcoded to 12.
            let sg_policies = self.evaluator.security_governor.policies_count();
            p.current_active_policies = sg_policies as u32 + 5;
        }

        // Record execution outcome through the resilience engine (F-GAP-27).
        let success = matches!(
            &verdict,
            PolicyVerdict::Allow
                | PolicyVerdict::AllowWithConstraints(_)
                | PolicyVerdict::Review(_)
        );

        // PUA de-escalation: after 3 consecutive clean evaluations, de-escalate.
        // Uses compare_exchange for atomic reset to prevent double-de-escalation
        // when two concurrent evaluations both pass (principle #15: no races).
        match &verdict {
            PolicyVerdict::Allow | PolicyVerdict::AllowWithConstraints(_) => {
                let prev = self.consecutive_allows.fetch_add(1, Ordering::SeqCst);
                if prev >= 2 {
                    // Atomic reset: only the thread that successfully moves
                    // from (prev + 1) → 0 wins the de-escalation right.
                    // All others see a stale expected value and skip.
                    if self
                        .consecutive_allows
                        .compare_exchange(prev + 1, 0, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                    {
                        let engine = self
                            .evaluator
                            .rule_engine
                            .lock()
                            .unwrap_or_else(|poisoned| {
                                tracing::warn!("[harness_bus] lock poisoned, recovering");
                                poisoned.into_inner()
                            });
                        let level = engine
                            .de_escalate("No violations detected for 3 consecutive evaluations");
                        tracing::info!(
                            new_level = level,
                            "PUA de-escalated after 3 consecutive clean evaluations"
                        );
                        // The PUA rule engine learning from evaluation patterns
                        // constitutes a learning update.
                        if let Ok(mut p) = self.profile.lock() {
                            p.record_learning_update();
                        }
                    }
                }
            }
            _ => {
                self.consecutive_allows.store(0, Ordering::SeqCst);
            }
        }

        // Parallelize resilience recording and audit logging — they are
        // independent, so join them instead of running sequentially.
        let audit_entry = AuditLogEntry {
            timestamp: crate::governance::audit::chrono_now(),
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
        tokio::join!(
            self.resilience_engine
                .record_execution("harness-main", success),
            async {
                self.audit(audit_entry);
            },
        );

        verdict
    }

    /// Pre-tool-call validation against sandbox/budget/RBAC policies.
    ///
    /// Evaluates the tool via `check_tool_call()` and updates profile
    /// metrics. This is the single validation entry point used by
    /// `observe_phase()` and other callers.
    pub async fn validate_action(&self, tool: &str, args: &Value) -> ToolVerdict {
        let start = Instant::now();
        let verdict = self.evaluator.check_tool_call(tool, args);

        // Feed real latency telemetry into the drift engine (Performance drift).
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        let _ = self.drift_engine.record_metric(
            "harness.validate_action.latency_ms",
            elapsed_ms,
            0.0,
            DriftType::Performance,
        );

        // Sync profile update (scope ensures MutexGuard is dropped before any await)
        {
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
        } // MutexGuard dropped here

        verdict
    }

    /// Return the cached result for a repeated (tool, args) call, or `None`
    /// when no valid entry exists (first call / policy TTL expired).
    ///
    /// Tool execution paths use this after `validate_action` reports
    /// `verdict.idempotent` to implement standard idempotency semantics:
    /// a repeat call skips re-execution and returns the cached result.
    pub fn cached_tool_result(&self, tool: &str, args: &Value) -> Option<Value> {
        self.evaluator.cached_idempotent_result(tool, args)
    }

    /// Record a successful tool execution so a repeated (tool, args) call is
    /// deduplicated within the `IdempotencyPolicy` TTL.
    pub fn record_tool_success(&self, tool: &str, args: &Value, response: &Value) {
        self.evaluator.record_tool_success(tool, args, response);
    }

    /// Post-execution output verification with audit recording.
    ///
    /// `stage` is the current PUA execution stage (see
    /// [`PolicyEvaluator::verify_output`]) and drives the PUA evidence chain.
    pub fn verify_output(&self, output: &Value, stage: &str) -> OutputVerdict {
        let start = Instant::now();
        let verdict = self.evaluator.verify_output(output, stage);

        // Feed real latency telemetry into the drift engine (Performance drift).
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        let _ = self.drift_engine.record_metric(
            "harness.verify_output.latency_ms",
            elapsed_ms,
            0.0,
            DriftType::Performance,
        );
        let audit_entry = AuditLogEntry {
            timestamp: crate::governance::audit::chrono_now(),
            task_id: String::new(),
            phase: "harness_audit:verify_output".to_string(),
            agent: None,
            tool: None,
            decision: if verdict.quality { "allow" } else { "deny" }.to_string(),
            inputs: serde_json::json!({
                "violations": if verdict.quality {
                    vec![]
                } else {
                    vec!["output_verification_failed"]
                },
                "context_snapshot": {
                    "risk_score": verdict.risk_score,
                    "evidence_count": verdict.evidence.len(),
                },
            }),
            outputs: None,
            error: None,
            confidence: None,
            data_classification: None,
            compliance_tags: vec![],
            retention_policy: None,
            correlation_id: None,
        };
        self.audit(audit_entry);
        verdict
    }

    /// Record an audit entry.
    ///
    /// Consolidated: writes to the canonical `ThreadSafeAuditLog` only.
    /// The duplicate writes to `audit_trail` and `structured_audit_trail`
    /// were removed — they recorded the same data in different formats.
    /// The profile counter is still updated for metrics reporting, and the
    /// audit log's real degradation signal (entries dropped due to buffer
    /// overflow) is synced into the profile so `GovernanceStatus` can judge
    /// the audit subsystem from a real audit signal instead of unrelated
    /// counters.
    pub fn audit(&self, entry: AuditLogEntry) {
        self.audit_log.record(entry);

        let mut p = self.profile.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("[harness_bus] lock poisoned, recovering");
            poisoned.into_inner()
        });
        p.audit_entries_total = p.audit_entries_total.saturating_add(1);
        p.audit_dropped_entries = self.audit_log.dropped_count();
    }

    /// Build a per-agent execution policy from the three base policies,
    /// adjusted by agent role hints and task type.
    pub fn get_agent_policy(&self, agent: &str, task_type: &str) -> AgentExecutionPolicy {
        let agent_lower = agent.to_ascii_lowercase();
        let task_lower = task_type.to_ascii_lowercase();

        // Derive agent role tier from naming conventions
        let is_admin = agent_lower.contains("admin") || agent_lower.contains("planner");
        let is_reviewer = agent_lower.contains("review") || agent_lower.contains("audit");
        let is_tester = agent_lower.contains("test");

        // Derive task criticality from task type
        let is_security = task_lower.contains("security") || task_lower.contains("patch");
        let is_feature = task_lower.contains("feature") || task_lower.contains("implementation");

        let sandbox_idx = self.evaluator.governance.sandbox_level.level_index() as usize;

        // Admin/planner agents get slightly longer timeouts; everyone else
        // gets the default timeout (security tasks included).
        let timeout = if is_admin {
            self.evaluator.dispatch.timeout_policy.max_timeout
        } else {
            self.evaluator.dispatch.timeout_policy.default_timeout
        };

        // Security tasks, high sandbox idx, and reviewer agents all
        // need higher review level — merge conditions to avoid
        // clippy::if_same_then_else (both branches produce Manual).
        let review_level = if is_security || sandbox_idx >= 2 || is_reviewer {
            ReviewLevel::Manual
        } else {
            ReviewLevel::Auto
        };

        // Reviewers and testers can write files; shell access only for admin-level agents
        let allow_file_write = sandbox_idx <= 1 || is_reviewer || is_tester;
        let allow_shell = sandbox_idx == 0 || is_admin;

        // Feature work may need more tool calls; security tasks use fewer
        let max_tool_calls = if is_feature {
            (self.evaluator.execution.budget.max_tool_calls as u32)
                .saturating_mul(2)
                .min(512)
        } else if is_security {
            (self.evaluator.execution.budget.max_tool_calls as u32)
                .saturating_div(2)
                .max(8)
        } else {
            self.evaluator.execution.budget.max_tool_calls as u32
        };

        AgentExecutionPolicy {
            timeout,
            max_tool_calls,
            allow_file_write,
            allow_shell,
            allow_network: true,
            review_level,
            audit_level: if is_security || sandbox_idx >= 2 {
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

    /// Drift protection profile snapshot (F-GAP-26).
    pub fn drift_profile(&self) -> DriftProfile {
        self.drift_engine.profile()
    }

    // Brain loop profile snapshot removed — the BrainLoop orchestration
    // state machine had zero production callers besides `profile()` (all-zero
    // state); deleted in round 23. The live `plan_construction::Planner` is
    // wired through `OrchestrationServerDeps` and unaffected.

    // Omnipotent mode profile snapshot removed — the OmnipotentMode module
    // had zero production callers besides `profile()` (all-zero data); deleted
    // in round 23. The `omnipotent_mode_readiness` release gate in
    // governance.status is an independent readiness profile and unchanged.

    /// Hyper-resilience profile snapshot (F-GAP-27)
    pub async fn resilience_profile(&self) -> ResilienceProfile {
        self.resilience_engine.profile().await
    }

    /// Fault tolerance profile snapshot (F-GAP-28)
    pub async fn fault_tolerance_profile(&self) -> FaultToleranceProfile {
        self.fault_tolerance.profile().await
    }

    /// Review gate prompt for LLM-based approval (PUA-wired).
    pub fn review_gate_prompt(&self) -> String {
        crate::governance::pua::review_gate_prompt()
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

    /// Start a background tokio task that periodically checks for drift.
    pub fn start_drift_monitor(&self, interval_secs: u64) {
        let interval = std::time::Duration::from_secs(interval_secs);
        let engine = self.drift_engine.clone();
        let profile = self.profile.clone();
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
                        if let Ok(mut p) = profile.lock() {
                            p.record_drift_detection();
                        }
                        // Route drift alerts into the AlertManager pipeline so
                        // they reach recent_alerts / webhook like the other
                        // alert producers (memory, p95, circuit, security).
                        use crate::observability::alert_manager::alert_manager;
                        use crate::shared::alert_severity::AlertSeverity;
                        let alert_severity = match alert.severity {
                            crate::governance::drift::DriftSeverity::Breach
                            | crate::governance::drift::DriftSeverity::Critical => {
                                AlertSeverity::Critical
                            }
                            crate::governance::drift::DriftSeverity::Warning => {
                                AlertSeverity::Warning
                            }
                            crate::governance::drift::DriftSeverity::Notice => AlertSeverity::Info,
                        };
                        if let Ok(mut mgr) = alert_manager().lock() {
                            mgr.report_direct(
                                &format!("drift.{}", metric_name),
                                alert.message.clone(),
                                alert_severity,
                            );
                        }
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
pub fn default_harness_bus() -> HarnessBus {
    let pua_plan = Arc::new(Mutex::new(
        crate::governance::pua::PuaEnforcementPlan::default(),
    ));
    let rule_engine = Arc::new(Mutex::new(PuaRuleEngine::new(pua_plan)));
    let sandbox_level = Arc::new(Mutex::new(SandboxLevel::None));
    let budget = Arc::new(Mutex::new(BudgetTracker::new(TaskBudget::default())));
    let idempotency = Arc::new(Mutex::new(IdempotencyCache::new(Duration::from_secs(3600))));
    let runtime_control = Arc::new(Mutex::new(OnlineControllerState::default()));
    let guard = Arc::new(Mutex::new(SelfRationalizationGuard::new(0.6)));
    // Share the process-wide canonical audit sink.
    let audit_log = Arc::new(crate::governance::audit::global_audit_log().clone());

    HarnessBus::new(
        rule_engine,
        sandbox_level,
        budget,
        idempotency,
        runtime_control,
        guard,
        audit_log,
        None,
    )
}

/// Build a HarnessBus with policies derived from `AppConfig` sections.
pub fn config_aware_harness_bus(config: &crate::config::AppConfig) -> HarnessBus {
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

    // Worker factor: the dual-level scheduler (which scaled the budget) was
    // removed as a production no-op; budgets default to the single-worker
    // factor (1).
    let budget = Arc::new(Mutex::new(BudgetTracker::new(TaskBudget::default())));

    let idempotency = Arc::new(Mutex::new(IdempotencyCache::new(Duration::from_secs(3600))));

    let runtime_control = Arc::new(Mutex::new(OnlineControllerState::default()));
    let guard = Arc::new(Mutex::new(SelfRationalizationGuard::new(0.6)));
    // Share the process-wide canonical audit sink.
    let audit_log = Arc::new(crate::governance::audit::global_audit_log().clone());

    HarnessBus::new(
        rule_engine,
        sandbox_level,
        budget,
        idempotency,
        runtime_control,
        guard,
        audit_log,
        None,
    )
}

// Re-export Duration for factory functions
use std::time::Duration;
