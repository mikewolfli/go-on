//! Policy type definitions for HarnessBus — F-GAP-13
//!
//! All enums, structs, and their default implementations used by the
//! HarnessBus governance policy engine.

use crate::governance::hardening::{SandboxLevel, TaskBudget};
use serde::{Deserialize, Serialize};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Routing & dispatch
// ---------------------------------------------------------------------------

/// How CapabilityBus selects the next agent
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[non_exhaustive]
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

// ---------------------------------------------------------------------------
// Execution mode & tool policies
// ---------------------------------------------------------------------------

/// Execution mode for sub-agents
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ExecutionMode {
    #[default]
    Auto,
    Assisted,
    Manual,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ToolUsagePolicy {
    #[default]
    AllowAll,
    AllowListed(Vec<String>),
    BlockListed(Vec<String>),
    ReadOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
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
#[non_exhaustive]
pub enum CodeExecutionPolicy {
    AllowAll,
    #[default]
    Sandboxed,
    DenyAll,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub enum AuditLevel {
    Minimal,
    #[default]
    Standard,
    Verbose,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub enum FailureStrategy {
    #[default]
    Retry,
    Fallback,
    FailFast,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DegradationStrategy {
    #[default]
    None,
    DegradeOnTimeout,
    DegradeOnFailure,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ReviewLevel {
    None,
    #[default]
    Auto,
    Manual,
}

// ---------------------------------------------------------------------------
// Execution & governance policy structs
// ---------------------------------------------------------------------------

/// ExecutionPolicy — "how should the selected agent execute this task"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPolicy {
    pub execution_mode: ExecutionMode,
    pub tool_usage: ToolUsagePolicy,
    pub file_write: FileWritePolicy,
    pub code_execution: CodeExecutionPolicy,
    pub review_requirement: ReviewLevel,
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
            review_requirement: ReviewLevel::default(),
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
// Verdict types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum PolicyVerdict {
    Allow,
    Deny(PolicyViolation),
    Escalate(EscalationReason),
    Review(ReviewReason),
    AllowWithConstraints(Vec<Constraint>),
}

/// Slimmed-down decision enum for HTTP response mapping.
/// Deny → 403, RequireReview → 449, Escalate → 402, Allow → 200.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny(String),
    RequireReview(String),
    Escalate(String, u8),
}

impl From<PolicyVerdict> for Decision {
    fn from(v: PolicyVerdict) -> Self {
        match v {
            PolicyVerdict::Allow | PolicyVerdict::AllowWithConstraints(_) => Decision::Allow,
            PolicyVerdict::Deny(v) => Decision::Deny(format!("{}: {}", v.kind, v.detail)),
            PolicyVerdict::Review(r) => Decision::RequireReview(r.reason),
            PolicyVerdict::Escalate(e) => Decision::Escalate(e.reason, e.suggested_level),
        }
    }
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
    pub require_review: bool,
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
// Audit entry
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
    pub context_snapshot: serde_json::Value,
}
