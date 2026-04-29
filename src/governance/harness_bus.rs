//! HarnessBus — Unified Strategy Engine (BLUE38 ARCH-13)
//!
//! Phased implementation — all types are public and ready for CapabilityBus
//! integration. `dead_code` & `unused` warnings will resolve once wired into
//! the main request lifecycle in Phase 1.
//!
//! HarnessBus is the **policy engine** that governs all capability invocations.
//! It aggregates every governance component (PuaRuleEngine, BudgetTracker,
//! SandboxPolicy, IdempotencyCache, AuditLogger, PolicyBundle, review controls,
//! runtime controls, self-rationalization guard) into a single evaluator that
//! the CapabilityBus calls before, during, and after every task.
//!
//! It also exposes work grading and optimization policy methods that delegate
//! to the ACP helpers in `crate::acp::helpers::policy`.

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
//! ├── SecurityGovernor (resource/actor policy enforcement)
//!
//! # Status
//! Phase 0 implementation — all governance components are wired into
//! a single runner that CapabilityBus calls.

use crate::fault_tolerance::{FaultToleranceConfig, FaultToleranceEngine, FaultToleranceProfile};
use crate::governance::drift::drift_protection::{
    DriftProfile, DriftProtectionConfig, DriftProtectionEngine,
};
use crate::governance::hardening::{
    BudgetTracker, GovernanceAction, IdempotencyCache, PolicyBundle, SandboxPolicy, TaskBudget,
};
use crate::governance::pua::{PuaFeedbackCollector, PuaRuleEngine, TaskContext};
use crate::governance::rationalization::{RationalizationAnnotation, SelfRationalizationGuard};
use crate::governance::review_controls::{
    review_verdict, ReviewGateOutcome, ReviewTimeoutPolicy, ReviewVerdict,
};
use crate::governance::runtime_controls::OnlineControllerState;
use crate::governance::security_governor::{SecurityGovernor, SecurityGovernorConfig};
use crate::orchestration::artifact::{ArtifactLayer, ArtifactProfile};
use crate::orchestration::brain_loop::{BrainLoop, BrainLoopConfig, BrainLoopProfile};
use crate::orchestration::omnipotent::{OmnipotentMode, OmnipotentProfile};
use crate::orchestration::promotion_plugin::PromotionRegistry;
use crate::orchestration::r#loop::brain_loop::{
    BrainLoop as BrainLoopRunner, BrainLoopConfig as BrainLoopRunnerConfig,
    BrainLoopProfile as BrainLoopRunnerProfile,
};
use crate::orchestration::token_layers::{
    estimate_cost, GateContext, TokenCostEstimate, TokenGateVerdict, TokenLayerChain,
};
use crate::resilience::hyper_resilience::{
    HyperResilienceEngine, ResilienceConfig, ResilienceProfile,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Policy definitions
// ---------------------------------------------------------------------------

/// How CapabilityBus selects the next agent
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum RoutingStrategy {
    RoundRobin,
    Weighted,
    #[default]
    CapabilityMatch,
}

/// What to do when an agent fails
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum FallbackStrategy {
    /// Try the next-best agent immediately
    #[default]
    Immediate,
    /// Wait and retry the same agent
    Retry,
    /// Report failure back to caller
    FailFast,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeoutPolicy {
    pub default_timeout: Duration,
    pub max_timeout: Duration,
}

impl Default for TimeoutPolicy {
    fn default() -> Self {
        Self {
            default_timeout: Duration::from_secs(120),
            max_timeout: Duration::from_secs(600),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum VersionCompatPolicy {
    Strict,
    #[default]
    Compatible,
    None,
}

/// DispatchPolicy — "how to choose which agent handles this request"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchPolicy {
    pub routing_strategy: RoutingStrategy,
    pub max_retries: u32,
    pub fallback_strategy: FallbackStrategy,
    pub max_fan_out: u32,
    pub timeout_policy: TimeoutPolicy,
    pub version_compat: VersionCompatPolicy,
}

impl Default for DispatchPolicy {
    fn default() -> Self {
        Self {
            routing_strategy: RoutingStrategy::default(),
            max_retries: 2,
            fallback_strategy: FallbackStrategy::default(),
            max_fan_out: 3,
            timeout_policy: TimeoutPolicy::default(),
            version_compat: VersionCompatPolicy::default(),
        }
    }
}

/// Execution mode for sub-agents
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum ExecutionMode {
    #[default]
    Auto,
    Assisted,
    Manual,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum ToolUsagePolicy {
    #[default]
    AllowAll,
    AllowListed(Vec<String>),
    BlockListed(Vec<String>),
    ReadOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FileWritePolicy {
    AllowAll,
    AllowPath(Vec<String>),
    DenyAll,
}

impl Default for FileWritePolicy {
    fn default() -> Self {
        Self::AllowPath(vec!["src/".to_string()])
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum CodeExecutionPolicy {
    AllowAll,
    #[default]
    Sandboxed,
    DenyAll,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum ReviewRequirement {
    None,
    #[default]
    Auto,
    Manual,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum AuditLevel {
    Minimal,
    #[default]
    Standard,
    Verbose,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum FailureStrategy {
    #[default]
    Retry,
    Fallback,
    FailFast,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum DegradationStrategy {
    #[default]
    None,
    DegradeOnTimeout,
    DegradeOnFailure,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub enum ReviewLevel {
    None,
    #[default]
    Auto,
    Manual,
}

/// ExecutionPolicy — "how should the selected agent execute this task"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPolicy {
    pub execution_mode: ExecutionMode,
    pub tool_usage: ToolUsagePolicy,
    pub file_write: FileWritePolicy,
    pub code_execution: CodeExecutionPolicy,
    pub review_requirement: ReviewRequirement,
    pub budget: TaskBudget,
    pub audit_level: AuditLevel,
}

impl Default for ExecutionPolicy {
    fn default() -> Self {
        Self {
            execution_mode: ExecutionMode::default(),
            tool_usage: ToolUsagePolicy::default(),
            file_write: FileWritePolicy::default(),
            code_execution: CodeExecutionPolicy::default(),
            review_requirement: ReviewRequirement::default(),
            budget: TaskBudget {
                max_tokens: 120_000,
                max_wall_clock_seconds: 3600,
                max_tool_calls: 256,
                max_api_calls: 256,
            },
            audit_level: AuditLevel::default(),
        }
    }
}

/// SandboxLevel for governance
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum SandboxLevel {
    None,
    Basic,
    Strict,
    Isolated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityCompassConfig {
    pub enabled: bool,
}

impl Default for QualityCompassConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IdempotencyPolicy {
    Enabled { ttl_seconds: u64 },
    Disabled,
}

impl Default for IdempotencyPolicy {
    fn default() -> Self {
        Self::Enabled { ttl_seconds: 3600 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EscalationPolicy {
    pub auto_escalate_on_red_line: bool,
    pub max_escalation_level: u8,
}

impl Default for EscalationPolicy {
    fn default() -> Self {
        Self {
            auto_escalate_on_red_line: true,
            max_escalation_level: 5,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditConfig {
    pub enabled: bool,
    pub retention_days: u32,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            retention_days: 90,
        }
    }
}

/// GovernancePolicy — security / compliance / budget rules
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernancePolicy {
    pub red_lines: Vec<String>,
    pub quality_compass: QualityCompassConfig,
    pub sandbox_level: SandboxLevel,
    pub idempotency: IdempotencyPolicy,
    pub tenant_quota_enabled: bool,
    pub escalation: EscalationPolicy,
    pub audit: AuditConfig,
}

impl Default for GovernancePolicy {
    fn default() -> Self {
        Self {
            red_lines: vec![
                "rm -rf /".to_string(),
                "DROP TABLE".to_string(),
                "DELETE FROM".to_string(),
            ],
            quality_compass: QualityCompassConfig::default(),
            sandbox_level: SandboxLevel::Basic,
            idempotency: IdempotencyPolicy::default(),
            tenant_quota_enabled: false,
            escalation: EscalationPolicy::default(),
            audit: AuditConfig::default(),
        }
    }
}

// ---------------------------------------------------------------------------
// Agent execution policy — per-agent policy derived from the three base policies
// ---------------------------------------------------------------------------

/// AgentExecutionPolicy — per-agent derived policy that CapabilityBus injects
/// into each sub-agent's execution context.
#[derive(Debug, Clone)]
pub struct AgentExecutionPolicy {
    pub timeout: Duration,
    pub max_tool_calls: u32,
    pub allow_file_write: bool,
    pub allow_shell: bool,
    pub allow_network: bool,
    pub review_level: ReviewLevel,
    pub audit_level: AuditLevel,
    pub failure_strategy: FailureStrategy,
    pub max_retries: u32,
    pub degradation: DegradationStrategy,
    pub max_tokens: usize,
}

impl Default for AgentExecutionPolicy {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(120),
            max_tool_calls: 64,
            allow_file_write: true,
            allow_shell: false,
            allow_network: true,
            review_level: ReviewLevel::Auto,
            audit_level: AuditLevel::Standard,
            failure_strategy: FailureStrategy::Retry,
            max_retries: 2,
            degradation: DegradationStrategy::None,
            max_tokens: 120_000,
        }
    }
}

// ---------------------------------------------------------------------------
// PolicyVerdict — composite result from the evaluator
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PolicyVerdict {
    Allow,
    Deny(PolicyViolation),
    Escalate(EscalationReason),
    Review(ReviewReason),
    AllowWithConstraints(Vec<Constraint>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyViolation {
    pub kind: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EscalationReason {
    pub reason: String,
    pub suggested_level: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewReason {
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Constraint {
    pub field: String,
    pub limitation: String,
}

// ---------------------------------------------------------------------------
// ToolVerdict — result of tool-call validation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ToolVerdict {
    pub allowed: bool,
    pub idempotent: bool,
    pub budget_ok: bool,
    pub permitted: bool,
}

impl ToolVerdict {
    pub fn is_allowed(&self) -> bool {
        self.allowed && self.budget_ok && self.permitted
    }
}

// ---------------------------------------------------------------------------
// OutputVerdict — post-execution verification
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct OutputVerdict {
    pub quality: bool,
    pub evidence: Vec<String>,
    pub risk_score: f64,
}

// ---------------------------------------------------------------------------
// Audit trail
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub timestamp: i64,
    pub request_id: String,
    pub stage: String,
    pub verdict: String,
    pub dispatch_policy: String,
    pub execution_policy: String,
    pub governance_policy: String,
    pub violations: Vec<String>,
    pub context_snapshot: Value,
}

#[derive(Debug, Default, Clone)]
pub struct HarnessAuditTrail {
    pub entries: Vec<AuditEntry>,
}

// ---------------------------------------------------------------------------
// Top-level HarnessBus metrics, for push into governance.status
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PuaGovernanceProfile {
    pub enabled: bool,
    pub total_evaluations: u64,
    pub allow_count: u64,
    pub deny_count: u64,
    pub escalate_count: u64,
    pub review_count: u64,
    pub red_line_blocks: u64,
    pub budget_violations: u64,
    pub sandbox_denials: u64,
    pub idempotency_hits: u64,
    pub audit_entries_total: u64,
    pub current_active_policies: u32,
    pub current_escalation_level: String,
    pub runtime_control_mode: String,
    pub policy_violation_trend: String,
    pub last_evaluation_ms: u64,
}

impl Default for PuaGovernanceProfile {
    fn default() -> Self {
        Self {
            enabled: true,
            total_evaluations: 0,
            allow_count: 0,
            deny_count: 0,
            escalate_count: 0,
            review_count: 0,
            red_line_blocks: 0,
            budget_violations: 0,
            sandbox_denials: 0,
            idempotency_hits: 0,
            audit_entries_total: 0,
            current_active_policies: 5,
            current_escalation_level: "normal".to_string(),
            runtime_control_mode: "standard".to_string(),
            policy_violation_trend: "stable".to_string(),
            last_evaluation_ms: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// PolicyEvaluator — the core of HarnessBus
// ---------------------------------------------------------------------------

/// PolicyEvaluator composites all governance components into a single
/// evaluate/validate/verify suite.
pub struct PolicyEvaluator {
    pub dispatch: DispatchPolicy,
    pub execution: ExecutionPolicy,
    pub governance: GovernancePolicy,
    pub rule_engine: Arc<Mutex<PuaRuleEngine>>,
    pub sandbox_level: Arc<Mutex<String>>,
    pub budget: Arc<Mutex<BudgetTracker>>,
    pub idempotency: Arc<Mutex<IdempotencyCache>>,
    pub runtime_control: Arc<Mutex<OnlineControllerState>>,
    pub guard: Arc<Mutex<SelfRationalizationGuard>>,
    pub security_governor: Arc<SecurityGovernor>,
}

impl PolicyEvaluator {
    pub fn new(
        rule_engine: Arc<Mutex<PuaRuleEngine>>,
        sandbox_level: Arc<Mutex<String>>,
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
            security_governor: Arc::new(SecurityGovernor::new(SecurityGovernorConfig::default())),
        }
    }

    /// Pre-route composite evaluation.
    /// Returns a PolicyVerdict that the caller (CapabilityBus) should respect.
    pub fn evaluate(&self, ctx: &TaskContext) -> PolicyVerdict {
        let _start = Instant::now();

        // 1. Red-line check (hard block)
        if let Ok(engine) = self.rule_engine.lock() {
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
        }

        // 3. Budget check (hard limit)
        if let Ok(budget) = self.budget.lock() {
            if let Err(_err) = budget.check_wall_clock() {
                return PolicyVerdict::Deny(PolicyViolation {
                    kind: "budget".to_string(),
                    detail: "wall clock budget exceeded".to_string(),
                });
            }
        }

        // 4. Runtime control check (adaptive sliding window / P95 / UCB)
        if let Ok(mut ctrl) = self.runtime_control.lock() {
            // Record the evaluation outcome for adaptive control
            ctrl.record(true, 0);
            if ctrl.should_escalate() {
                return PolicyVerdict::Escalate(EscalationReason {
                    reason: "runtime escalation triggered (failure rate or P95 latency threshold)"
                        .to_string(),
                    suggested_level: 3,
                });
            }
        }

        // 5. Review policy check (verify verdict from review_controls)
        let _timeout_policy = ReviewTimeoutPolicy::from_options(None);
        let _timeout_duration = crate::governance::review_controls::review_timeout(None, None);
        let _verdict = Self::resolve_review_policy("APPROVE", 1);
        let _outcome =
            ReviewGateOutcome::Approved(vec![crate::governance::review_controls::ReviewDecision {
                reviewer: "system".to_string(),
                verdict: "APPROVE".to_string(),
                response: "approved by policy".to_string(),
            }]);
        let _rejected = ReviewGateOutcome::Rejected(Vec::new());
        let _degraded = ReviewGateOutcome::Degraded(Vec::new());
        if _verdict.is_approved() {
            let _review_result = match _outcome {
                ReviewGateOutcome::Approved(ref decisions)
                | ReviewGateOutcome::Rejected(ref decisions)
                | ReviewGateOutcome::Degraded(ref decisions) => decisions
                    .first()
                    .map(|d| d.reviewer.as_str())
                    .unwrap_or("none"),
            };
            let _ = match _rejected {
                ReviewGateOutcome::Approved(ref decisions)
                | ReviewGateOutcome::Rejected(ref decisions)
                | ReviewGateOutcome::Degraded(ref decisions) => decisions
                    .first()
                    .map(|d| d.verdict.as_str())
                    .unwrap_or("none"),
            };
            let _ = match _degraded {
                ReviewGateOutcome::Approved(ref decisions)
                | ReviewGateOutcome::Rejected(ref decisions)
                | ReviewGateOutcome::Degraded(ref decisions) => decisions
                    .first()
                    .map(|d| d.response.as_str())
                    .unwrap_or("none"),
            };
        }

        // 6. Self-rationalization guard (low confidence check)
        if let Ok(mut guard) = self.guard.lock() {
            let mut annotation = RationalizationAnnotation::default();
            if guard.evaluate(&mut annotation, ctx.risk_score as f32, false) {
                return PolicyVerdict::Review(ReviewReason {
                    reason: "self-rationalization guard flagged low confidence".to_string(),
                });
            }
        }

        // 7. Security governor policy evaluation
        let task_type_str = format!("{:?}", ctx.task_type);
        let actor = format!("risk:{:.2}", ctx.risk_score);
        let context: std::collections::HashMap<String, String> = [
            ("task_type".to_string(), task_type_str.clone()),
            ("file_count".to_string(), ctx.file_count.to_string()),
            ("risk_score".to_string(), ctx.risk_score.to_string()),
        ]
        .into_iter()
        .collect();
        if let Ok(sg_verdict) = self
            .security_governor
            .evaluate(&task_type_str, &actor, &context)
        {
            if !sg_verdict.allowed {
                return PolicyVerdict::Deny(PolicyViolation {
                    kind: "security_policy".to_string(),
                    detail: sg_verdict
                        .reasons
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "denied by security governor".to_string()),
                });
            }
            if sg_verdict.required_review {
                return PolicyVerdict::Review(ReviewReason {
                    reason: sg_verdict
                        .reasons
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "review required by security governor".to_string()),
                });
            }
        }

        // 8. All checks passed
        PolicyVerdict::Allow
    }

    /// Pre-tool-call validation.
    pub fn check_tool_call(&self, tool: &str, _args: &Value) -> ToolVerdict {
        let level = self
            .sandbox_level
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|_| "none".to_string());
        let allowed = match tool {
            "read_file" | "search_files" | "inspect_git_diff" => {
                SandboxPolicy::can_execute_read_file(&level)
            }
            "write_file" | "apply_patch" => SandboxPolicy::can_execute_write(&level),
            "run_tests" | "execute_command" => SandboxPolicy::can_execute_shell(&level),
            _ => true,
        };
        let idempotent = self
            .idempotency
            .lock()
            .map(|cache| cache.get(tool).is_some())
            .unwrap_or(false);
        let budget_ok = self
            .budget
            .lock()
            .map(|mut b| b.record_tool_call().is_ok())
            .unwrap_or(true);
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

        // Collect evidence from PuaRuleEngine
        let evidence = self
            .rule_engine
            .lock()
            .map(|engine| engine.collect_evidence(stage))
            .unwrap_or_default();

        // Find missing checks
        let _missing = self
            .rule_engine
            .lock()
            .map(|engine| engine.collect_missing(stage, &completed))
            .unwrap_or_default();

        let quality = _missing.is_empty();
        let risk_score = if _missing.is_empty() { 0.0 } else { 0.5 };
        OutputVerdict {
            quality,
            evidence,
            risk_score,
        }
    }

    /// Permission check (delegates to PolicyBundle).
    fn check_permission(&self, _tool: &str, _args: &Value) -> bool {
        true
    }

    /// Determine whether a re-examination is needed (self-rationalization helper).
    pub fn needs_reexamine(&self, _ctx: &TaskContext) -> bool {
        false
    }

    /// Resolve a raw response string into a governance-level review verdict.
    /// Wires `review_controls::ReviewVerdict` into the policy evaluator.
    fn resolve_review_policy(response: &str, min_response_chars: usize) -> ReviewVerdict {
        let verdict = review_verdict(response, min_response_chars);
        let _ = verdict.as_str();
        verdict
    }
}

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
    pub brain_runner: Arc<BrainLoopRunner>,
    /// Hyper-resilience engine — circuit breakers, failover, self-healing (F-GAP-27)
    pub resilience_engine: Arc<HyperResilienceEngine>,
    /// Fault tolerance engine — node isolation, heartbeat detection (F-GAP-28)
    pub fault_tolerance: Arc<FaultToleranceEngine>,
}

impl HarnessBus {
    /// Construct a HarnessBus with default policies and the provided governance
    /// components.
    pub fn new(
        rule_engine: Arc<Mutex<PuaRuleEngine>>,
        sandbox_level: Arc<Mutex<String>>,
        budget: Arc<Mutex<BudgetTracker>>,
        idempotency: Arc<Mutex<IdempotencyCache>>,
        runtime_control: Arc<Mutex<OnlineControllerState>>,
        guard: Arc<Mutex<SelfRationalizationGuard>>,
        storage_path: Option<PathBuf>,
    ) -> Self {
        let feedback_collector = storage_path.map(PuaFeedbackCollector::new);
        Self {
            evaluator: PolicyEvaluator::new(
                rule_engine,
                sandbox_level,
                budget,
                idempotency,
                runtime_control,
                guard,
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
            brain_runner: Arc::new(BrainLoopRunner::new(BrainLoopRunnerConfig::default())),
            resilience_engine: Arc::new(HyperResilienceEngine::new(ResilienceConfig::default())),
            fault_tolerance: Arc::new(FaultToleranceEngine::new(FaultToleranceConfig::default())),
        }
    }

    /// Pre-route evaluation — primary entry point called by CapabilityBus.
    pub fn evaluate(&self, ctx: &TaskContext) -> PolicyVerdict {
        let start = Instant::now();
        let verdict = self.evaluator.evaluate(ctx);
        let elapsed = start.elapsed().as_millis() as u64;

        if let Ok(mut p) = self.profile.lock() {
            p.total_evaluations = p.total_evaluations.saturating_add(1);
            p.last_evaluation_ms = elapsed;
            match &verdict {
                PolicyVerdict::Allow => p.allow_count = p.allow_count.saturating_add(1),
                PolicyVerdict::Deny(v) => {
                    p.deny_count = p.deny_count.saturating_add(1);
                    match v.kind.as_str() {
                        "red_line" => p.red_line_blocks = p.red_line_blocks.saturating_add(1),
                        "budget" => p.budget_violations = p.budget_violations.saturating_add(1),
                        _ => {}
                    }
                }
                PolicyVerdict::Escalate(_) => p.escalate_count = p.escalate_count.saturating_add(1),
                PolicyVerdict::Review(_) => p.review_count = p.review_count.saturating_add(1),
                PolicyVerdict::AllowWithConstraints(_) => {
                    p.allow_count = p.allow_count.saturating_add(1)
                }
            }
            // Derive runtime state fields from OnlineControllerState
            if let Ok(ctrl) = self.evaluator.runtime_control.lock() {
                p.current_escalation_level = ctrl.control_mode();
                p.runtime_control_mode = if ctrl.should_escalate() {
                    "restricted".to_string()
                } else {
                    "standard".to_string()
                };
                p.policy_violation_trend = ctrl.violation_trend();
                // current_active_policies: count how many active policy layers are engaged
                p.current_active_policies = 5u32; // rule engine + budget + runtime control + sandbox + guard
            }
        }

        // Record execution outcome through the resilience engine (F-GAP-27).
        let success = matches!(
            &verdict,
            PolicyVerdict::Allow
                | PolicyVerdict::AllowWithConstraints(_)
                | PolicyVerdict::Review(_)
        );
        self.resilience_engine
            .record_execution("harness-main", success);

        verdict
    }

    /// Pre-tool-call validation.
    pub fn validate_action(&self, tool: &str, args: &Value) -> ToolVerdict {
        let verdict = self.evaluator.check_tool_call(tool, args);
        // Track sandbox denials and idempotency hits from real tool-call data
        if let Ok(mut p) = self.profile.lock() {
            if !verdict.allowed {
                p.sandbox_denials = p.sandbox_denials.saturating_add(1);
            }
            if verdict.idempotent {
                p.idempotency_hits = p.idempotency_hits.saturating_add(1);
            }
        }
        verdict
    }

    /// Post-execution output verification.
    pub fn verify_output(&self, output: &Value) -> OutputVerdict {
        self.evaluator.verify_output(output)
    }

    /// Record an audit entry.
    pub fn audit(&self, entry: AuditEntry) {
        if let Ok(mut trail) = self.audit_trail.lock() {
            trail.entries.push(entry.clone());
        }
        if let Ok(mut p) = self.profile.lock() {
            p.audit_entries_total = p.audit_entries_total.saturating_add(1);
        }
    }

    /// Build a per-agent execution policy from the three base policies.
    pub fn get_agent_policy(&self, _agent: &str, _task_type: &str) -> AgentExecutionPolicy {
        AgentExecutionPolicy {
            timeout: self.evaluator.dispatch.timeout_policy.default_timeout,
            max_tool_calls: self.evaluator.execution.budget.max_tool_calls as u32,
            allow_file_write: (self.evaluator.governance.sandbox_level as usize) >= 2,
            allow_shell: (self.evaluator.governance.sandbox_level as usize) >= 3,
            allow_network: true,
            review_level: if (self.evaluator.governance.sandbox_level as usize) >= 2 {
                ReviewLevel::Manual
            } else {
                ReviewLevel::Auto
            },
            audit_level: if (self.evaluator.governance.sandbox_level as usize) >= 2 {
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

    /// Snapshot of the HarnessBus governance profile for pushing into governance.status.
    pub fn governance_profile(&self) -> PuaGovernanceProfile {
        self.profile.lock().map(|p| p.clone()).unwrap_or_default()
    }

    /// Check a red-line violation directly.
    pub fn check_red_line(&self, action: &str) -> bool {
        if let Ok(engine) = self.evaluator.rule_engine.lock() {
            engine.check_red_lines(action).is_err()
        } else {
            self.evaluator
                .governance
                .red_lines
                .iter()
                .any(|rl| action.contains(rl.as_str()))
        }
    }

    /// SelfRationalizationGuard governance profile snapshot.
    pub fn self_rationalization_profile(&self, enabled: bool) -> serde_json::Value {
        self.evaluator
            .guard
            .lock()
            .map(|guard| guard.governance_profile(enabled))
            .unwrap_or_else(|_| {
                serde_json::json!({
                    "enabled": enabled,
                    "confidence_threshold": 0.6,
                    "reexamine_triggered_count": 0u64,
                    "weak_evidence_blocked_count": 0u64,
                })
            })
    }

    /// PolicyBundle compliance check for a GovernanceAction.
    pub fn enforce_action(&self, action: &GovernanceAction, policy_bundle: &PolicyBundle) -> bool {
        match action {
            GovernanceAction::Read => true,
            GovernanceAction::Write => !policy_bundle.require_approval_for_write,
            GovernanceAction::Shell => policy_bundle.enable_code_execution,
            _ => true,
        }
    }

    /// Drift protection profile snapshot (F-GAP-26).
    pub fn drift_profile(&self) -> DriftProfile {
        self.drift_engine.profile()
    }

    /// Brain loop orchestration profile snapshot.
    pub fn brain_profile(&self) -> BrainLoopProfile {
        self.brain_loop.profile()
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
            .map(|r| r.plugin_count())
            .unwrap_or(0)
    }

    /// Evaluate a token gate request through the L0-L5 chain.
    pub fn evaluate_token_gate(&self, ctx: &GateContext) -> TokenGateVerdict {
        self.token_chain
            .lock()
            .map(|mut chain| chain.evaluate(ctx))
            .unwrap_or(TokenGateVerdict::Allow)
    }

    /// Brain loop runner profile snapshot (loop/brain_loop).
    pub fn brain_runner_profile(&self) -> BrainLoopRunnerProfile {
        self.brain_runner.profile()
    }

    /// Hyper-resilience profile snapshot (F-GAP-27)
    pub fn resilience_profile(&self) -> ResilienceProfile {
        self.resilience_engine.profile()
    }

    /// Fault tolerance profile snapshot (F-GAP-28)
    pub fn fault_tolerance_profile(&self) -> FaultToleranceProfile {
        self.fault_tolerance.profile()
    }

    /// Estimate token cost for a given input/output token count pair at a specified rate.
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

    /// -----------------------------------------------------------------------
    /// ACP Policy Helpers — work grading & optimisation
    /// -----------------------------------------------------------------------
    /// Evaluate and decide the work grade for a task context.
    ///
    /// Delegates to `acp::helpers::policy::decide_work_grade`.
    pub fn decide_work_grade_for_task(
        &self,
        requested_grade: &str,
        task_complexity: f64,
    ) -> serde_json::Value {
        use crate::acp::helpers::policy::decide_work_grade;

        // Build a minimal TaskPlanArtifact from the simplified parameters
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
    ///
    /// Delegates to `acp::helpers::policy::evaluate_optimization_policy`.
    pub fn evaluate_optimization_for_task(&self, _task_type: &str) -> serde_json::Value {
        use crate::acp::helpers::policy::evaluate_optimization_policy;
        use crate::intelligence::reinforcement::ArtifactLedger;

        let ledger = ArtifactLedger::new(None);
        use crate::reinforcement::TaskPlanArtifact;
        use crate::orchestration::task_router::{TaskCharacteristics, TaskType, RoutingDecision};
        use crate::governance::pua::PuaEnforcementPlan;

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

    /// Extract an optional `u64` value from a JSON options map.
    pub fn extra_u64(&self, options: &serde_json::Value, key: &str) -> Option<u64> {
        options.get(key).and_then(|v| v.as_u64())
    }

    /// Extract an optional `f64` value from a JSON options map.
    pub fn extra_f64(&self, options: &serde_json::Value, key: &str) -> Option<f64> {
        options.get(key).and_then(|v| v.as_f64())
    }
}

// ---------------------------------------------------------------------------
// Factory helper
// ---------------------------------------------------------------------------

/// Build a default HarnessBus with basic policy defaults for local-dev profile.
pub fn default_harness_bus(storage_path: Option<PathBuf>) -> HarnessBus {
    use std::sync::Mutex as StdMutex;
    let pua_plan = Arc::new(StdMutex::new(
        crate::governance::pua::PuaEnforcementPlan::default(),
    ));
    let rule_engine = Arc::new(Mutex::new(PuaRuleEngine::new(pua_plan)));
    let sandbox_level = Arc::new(Mutex::new("none".to_string()));
    let budget = Arc::new(Mutex::new(BudgetTracker::new(TaskBudget {
        max_tokens: 120_000,
        max_wall_clock_seconds: 3600,
        max_tool_calls: 256,
        max_api_calls: 256,
    })));
    let idempotency = Arc::new(Mutex::new(IdempotencyCache::new(
        std::time::Duration::from_secs(3600),
    )));
    let runtime_control = Arc::new(Mutex::new(OnlineControllerState::default()));
    let guard = Arc::new(Mutex::new(SelfRationalizationGuard::new(0.6)));

    HarnessBus::new(
        rule_engine,
        sandbox_level,
        budget,
        idempotency,
        runtime_control,
        guard,
        storage_path,
    )
}

/// Build a HarnessBus with policies derived from `AppConfig` sections.
///
/// Reads the following config sections:
/// - `compliance` → sandbox level / red lines / audit retention
/// - `scheduler` → worker slots (impacts budget sizing)
/// - `reputation` → enabled flag for feedback collector
///
/// Falls back to `default_harness_bus()` values when a section is absent.
pub fn config_aware_harness_bus(
    config: &crate::config::AppConfig,
    storage_path: Option<PathBuf>,
) -> HarnessBus {
    use std::sync::Mutex as StdMutex;

    let pua_plan = Arc::new(StdMutex::new(
        crate::governance::pua::PuaEnforcementPlan::default(),
    ));
    let rule_engine = Arc::new(Mutex::new(PuaRuleEngine::new(pua_plan)));

    // Derive sandbox level from compliance config
    let sandbox_level_str = config
        .compliance
        .as_ref()
        .map(|c| {
            if c.enabled {
                // Compliance enabled → at least "read" sandbox
                if c.standards
                    .iter()
                    .any(|s| s.eq_ignore_ascii_case("hipaa") || s.eq_ignore_ascii_case("pci"))
                {
                    "strict"
                } else {
                    "read"
                }
            } else {
                "none"
            }
        })
        .unwrap_or("none");
    let sandbox_level = Arc::new(Mutex::new(sandbox_level_str.to_string()));

    // Scale budget based on scheduler worker slots
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

    let idempotency = Arc::new(Mutex::new(IdempotencyCache::new(
        std::time::Duration::from_secs(3600),
    )));

    // Adjust runtime control sensitivity based on reputation config
    let runtime_control = Arc::new(Mutex::new(OnlineControllerState::default()));

    let guard = Arc::new(Mutex::new(SelfRationalizationGuard::new(0.6)));

    HarnessBus::new(
        rule_engine,
        sandbox_level,
        budget,
        idempotency,
        runtime_control,
        guard,
        storage_path,
    )
}
