//! Hardening — F-GAP-08
//!
//! Phase 9: Production Hardening and Safety
//!
//! All types in this module (`TaskBudget`, `TenantResourceQuota`, `BudgetTracker`,
//! `AccessPolicy`, `ResourceBudget`, etc.) are **actively wired** and imported by
//! multiple call sites. Budget enforcement is applied at governance checkpoints
//! throughout the execution pipeline, and policy enforcement hooks (rate limiting,
//! quota gates, admission control) are operational. This is not forward-looking
//! scaffolding — the hardening layer is live.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{Duration, Instant};

use crate::core::config::RuntimeConfig;
use crate::i18n::runtime::tf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskBudget {
    pub max_tokens: usize,
    pub max_wall_clock_seconds: u64,
    pub max_tool_calls: usize,
    pub max_api_calls: usize,
}

impl Default for TaskBudget {
    fn default() -> Self {
        Self {
            max_tokens: 120_000,
            max_wall_clock_seconds: 3600,
            max_tool_calls: 256,
            max_api_calls: 256,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TenantResourceQuota {
    pub tenant_id: String,
    pub daily_token_limit: usize,
    pub concurrent_tasks_limit: usize,
    pub daily_api_call_limit: usize,
}

/// Shared budget state behind a single mutex to eliminate lock-ordering deadlock risk.
#[derive(Debug, Default)]
struct BudgetState {
    token_usage: HashMap<String, usize>,
    api_call_usage: HashMap<String, usize>,
    active_tasks: HashMap<String, usize>,
}

/// Tracks per-tenant resource usage and enforces quotas.
/// Used by CapabilityBus to reject tasks when a tenant exceeds its limits.
#[derive(Debug, Default)]
pub struct TenantBudgetEnforcer {
    quotas: HashMap<String, TenantResourceQuota>,
    state: std::sync::Mutex<BudgetState>,
    /// The "day number" (unix_ts / 86400) last observed, used to reset daily counters.
    current_day: AtomicI64,
}

impl TenantBudgetEnforcer {
    pub fn new() -> Self {
        Self {
            quotas: HashMap::new(),
            state: std::sync::Mutex::new(BudgetState {
                token_usage: HashMap::new(),
                api_call_usage: HashMap::new(),
                active_tasks: HashMap::new(),
            }),
            current_day: AtomicI64::new(Self::today()),
        }
    }

    /// Return the current day number (unix timestamp / 86400).
    fn today() -> i64 {
        crate::shared::timestamps::now_ts_ms() / 86_400_000
    }

    /// Reset daily counters if the day has changed.
    fn reset_daily_if_day_changed(&self) {
        let today = Self::today();
        if today != self.current_day.load(Ordering::Relaxed) {
            match self.state.lock() {
                Ok(mut state) => {
                    state.token_usage.clear();
                    state.api_call_usage.clear();
                }
                Err(poisoned) => {
                    tracing::warn!("budget state lock poisoned, recovering");
                    let mut state = poisoned.into_inner();
                    state.token_usage.clear();
                    state.api_call_usage.clear();
                }
            }
            self.current_day.store(today, Ordering::Relaxed);
        }
    }

    /// Register or update a tenant's quota.
    pub fn set_quota(&mut self, quota: TenantResourceQuota) {
        self.quotas.insert(quota.tenant_id.clone(), quota);
    }

    /// Check whether a tenant is allowed to start a new task.
    pub fn check_can_start(&self, tenant_id: &str) -> Result<(), String> {
        self.reset_daily_if_day_changed();
        let quota = self
            .quotas
            .get(tenant_id)
            .ok_or_else(|| format!("no quota configured for tenant '{}'", tenant_id))?;

        let guard = self.state.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("budget state lock poisoned: recovering");
            poisoned.into_inner()
        });
        let current_tasks = guard.active_tasks.get(tenant_id).copied().unwrap_or(0);
        if current_tasks >= quota.concurrent_tasks_limit {
            return Err(format!(
                "tenant '{}' at concurrent task limit ({}/{})",
                tenant_id, current_tasks, quota.concurrent_tasks_limit
            ));
        }

        let tokens = guard.token_usage.get(tenant_id).copied().unwrap_or(0);
        if tokens >= quota.daily_token_limit {
            return Err(format!(
                "tenant '{}' exceeded daily token limit ({}/{})",
                tenant_id, tokens, quota.daily_token_limit
            ));
        }

        let calls = guard.api_call_usage.get(tenant_id).copied().unwrap_or(0);
        if calls >= quota.daily_api_call_limit {
            return Err(tf(
                "error.tenant_limit_exceeded",
                &[
                    ("tenant_id", tenant_id),
                    ("calls", &calls.to_string()),
                    ("limit", &quota.daily_api_call_limit.to_string()),
                ],
            ));
        }

        Ok(())
    }

    /// Atomically check whether a tenant can start a new task and, if so,
    /// record that the task has started. This eliminates the TOCTOU race
    /// present when callers invoke check_can_start() and start_task() separately.
    pub fn check_and_start_task(&mut self, tenant_id: &str) -> Result<(), String> {
        self.reset_daily_if_day_changed();
        let quota = self
            .quotas
            .get(tenant_id)
            .ok_or_else(|| format!("no quota configured for tenant '{}'", tenant_id))?;

        let mut guard = self.state.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("budget state lock poisoned: recovering");
            poisoned.into_inner()
        });
        let current_tasks = guard.active_tasks.get(tenant_id).copied().unwrap_or(0);
        if current_tasks >= quota.concurrent_tasks_limit {
            return Err(format!(
                "tenant '{}' at concurrent task limit ({}/{})",
                tenant_id, current_tasks, quota.concurrent_tasks_limit
            ));
        }

        let tokens = guard.token_usage.get(tenant_id).copied().unwrap_or(0);
        if tokens >= quota.daily_token_limit {
            return Err(format!(
                "tenant '{}' exceeded daily token limit ({}/{})",
                tenant_id, tokens, quota.daily_token_limit
            ));
        }

        let calls = guard.api_call_usage.get(tenant_id).copied().unwrap_or(0);
        if calls >= quota.daily_api_call_limit {
            return Err(tf(
                "error.tenant_limit_exceeded",
                &[
                    ("tenant_id", tenant_id),
                    ("calls", &calls.to_string()),
                    ("limit", &quota.daily_api_call_limit.to_string()),
                ],
            ));
        }

        // All checks passed — atomically consume the slot.
        *guard.active_tasks.entry(tenant_id.to_string()).or_insert(0) += 1;
        Ok(())
    }

    /// Record that a tenant started a task.
    ///
    /// Prefer `check_and_start_task` over calling this separately after
    /// `check_can_start` to avoid TOCTOU races.
    pub fn start_task(&mut self, tenant_id: &str) {
        let mut guard = self.state.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("budget state lock poisoned: recovering");
            poisoned.into_inner()
        });
        *guard.active_tasks.entry(tenant_id.to_string()).or_insert(0) += 1;
    }

    /// Record resource consumption after a task completes.
    pub fn record_usage(&mut self, tenant_id: &str, tokens: usize, api_calls: usize) {
        let mut guard = self.state.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("budget state lock poisoned, recovering");
            poisoned.into_inner()
        });
        *guard.token_usage.entry(tenant_id.to_string()).or_insert(0) += tokens;
        *guard
            .api_call_usage
            .entry(tenant_id.to_string())
            .or_insert(0) += api_calls;
        let tasks = guard.active_tasks.entry(tenant_id.to_string()).or_insert(0);
        *tasks = tasks.saturating_sub(1);
    }

    pub fn quotas(&self) -> &HashMap<String, TenantResourceQuota> {
        &self.quotas
    }

    /// Auto-provision default quotas from runtime config for the "default-tenant"
    /// if no quota is already configured.  Called at server startup when user auth
    /// is enabled so that the budget enforcer does not reject every request.
    pub fn auto_provision_default(&mut self, config: &RuntimeConfig) {
        if !self.quotas.contains_key("default-tenant") {
            self.set_quota(TenantResourceQuota {
                tenant_id: "default-tenant".to_string(),
                daily_token_limit: config.tenant_default_daily_token_limit as usize,
                concurrent_tasks_limit: config.tenant_default_concurrent_tasks,
                daily_api_call_limit: config.tenant_default_daily_api_calls,
            });
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetExceededError {
    pub limit_type: &'static str,
    pub limit: usize,
    pub used: usize,
}

impl fmt::Display for BudgetExceededError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "budget exceeded: {} limit={}, used={}",
            self.limit_type, self.limit, self.used
        )
    }
}

impl std::error::Error for BudgetExceededError {}

#[derive(Debug, Clone)]
pub struct BudgetTracker {
    task_budget: TaskBudget,
    tokens_used: usize,
    tool_calls_made: usize,
    started_at: Instant,
}

impl BudgetTracker {
    pub fn new(task_budget: TaskBudget) -> Self {
        Self {
            task_budget,
            tokens_used: 0,
            tool_calls_made: 0,
            started_at: Instant::now(),
        }
    }

    pub fn record_tokens(&mut self, tokens: usize) -> Result<(), BudgetExceededError> {
        self.tokens_used = self.tokens_used.saturating_add(tokens);
        if self.tokens_used > self.task_budget.max_tokens {
            return Err(BudgetExceededError {
                limit_type: "tokens",
                limit: self.task_budget.max_tokens,
                used: self.tokens_used,
            });
        }
        Ok(())
    }

    pub fn record_tool_call(&mut self) -> Result<(), BudgetExceededError> {
        self.tool_calls_made = self.tool_calls_made.saturating_add(1);
        if self.tool_calls_made > self.task_budget.max_tool_calls {
            return Err(BudgetExceededError {
                limit_type: "tool_calls",
                limit: self.task_budget.max_tool_calls,
                used: self.tool_calls_made,
            });
        }
        Ok(())
    }

    /// Reset all budget counters for a fresh request budget.
    /// Prevents long-running backends from hitting accumulated limits.
    pub fn reset(&mut self) {
        self.started_at = Instant::now();
        self.tokens_used = 0;
        self.tool_calls_made = 0;
    }

    pub fn check_wall_clock(&self) -> Result<(), BudgetExceededError> {
        let elapsed = self.started_at.elapsed().as_secs() as usize;
        let limit = self.task_budget.max_wall_clock_seconds as usize;
        if elapsed > limit {
            return Err(BudgetExceededError {
                limit_type: "wall_clock_seconds",
                limit,
                used: elapsed,
            });
        }
        Ok(())
    }

    pub fn remaining_tokens(&self) -> usize {
        self.task_budget.max_tokens.saturating_sub(self.tokens_used)
    }

    pub fn consume_with_pua(
        &mut self,
        tokens: usize,
        pua: &crate::pua::PuaRuleEngine,
    ) -> Result<(), BudgetExceededError> {
        match self.record_tokens(tokens) {
            Ok(()) => Ok(()),
            Err(err) => {
                let _ = pua.escalate(&format!("BudgetExceeded: {}", err.limit_type));
                Err(err)
            }
        }
    }
}

pub fn task_budget_for_target(target: Option<&str>) -> TaskBudget {
    match target.unwrap_or("local-dev").to_ascii_lowercase().as_str() {
        "ci" | "ci-pipeline" => TaskBudget {
            max_tokens: 20_000,
            max_wall_clock_seconds: 600,
            max_tool_calls: 64,
            max_api_calls: 64,
        },
        "managed-service" | "managed" => TaskBudget {
            max_tokens: 8_000,
            max_wall_clock_seconds: 300,
            max_tool_calls: 24,
            max_api_calls: 24,
        },
        _ => TaskBudget {
            max_tokens: 120_000,
            max_wall_clock_seconds: 3_600,
            max_tool_calls: 256,
            max_api_calls: 256,
        },
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutonomousEditAuditEntry {
    pub timestamp: String,
    pub agent: String,
    pub file_path: String,
    pub change_summary: String,
    pub approval_reason: String,
    pub confidence_score: f32,
    pub reversible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyBundle {
    pub name: String,
    pub deployment_target: String, // "local-dev", "ci", "managed-service"
    pub max_autonomy: String,      // "ask", "edit", "agent", "full_auto"
    pub require_approval_for_write: bool,
    pub enable_code_execution: bool,
    pub sandbox_level: SandboxLevel,
}

impl PolicyBundle {
    pub fn local_dev() -> Self {
        Self {
            name: "local-dev".to_string(),
            deployment_target: "local-dev".to_string(),
            max_autonomy: "edit".to_string(),
            require_approval_for_write: false,
            enable_code_execution: true,
            sandbox_level: SandboxLevel::None,
        }
    }

    pub fn ci_pipeline() -> Self {
        Self {
            name: "ci-pipeline".to_string(),
            deployment_target: "ci".to_string(),
            max_autonomy: "agent".to_string(),
            require_approval_for_write: true,
            enable_code_execution: true,
            sandbox_level: SandboxLevel::Basic,
        }
    }

    pub fn managed_service() -> Self {
        Self {
            name: "managed-service".to_string(),
            deployment_target: "managed-service".to_string(),
            max_autonomy: "edit".to_string(),
            require_approval_for_write: true,
            enable_code_execution: false,
            sandbox_level: SandboxLevel::Strict,
        }
    }

    pub fn production_hardened() -> Self {
        Self {
            name: "production-hardened".to_string(),
            deployment_target: "production".to_string(),
            max_autonomy: "agent".to_string(),
            require_approval_for_write: true,
            enable_code_execution: false,
            sandbox_level: SandboxLevel::Isolated,
        }
    }
}

#[derive(Debug, Clone)]
pub struct IdempotentResult {
    pub response: Value,
    pub cached_at: Instant,
}

/// Per-tenant LRU-limited idempotency cache.
///
/// Each tenant gets an LRU cap (`max_entries_per_tenant`) so that one tenant
/// cannot evict another's entries. Within a tenant, when the cap is reached,
/// the oldest entry (by insertion order) is evicted.
///
/// Key format: `"{tenant_id}:{operation_key}"` — callers embed the tenant
/// in the key so that `insert` and `get` are single-lookup operations.
#[derive(Debug, Clone)]
pub struct IdempotencyCache {
    results: HashMap<String, IdempotentResult>,
    /// Per-tenant insertion order queue for LRU eviction (VecDeque for O(1) front removal).
    tenant_keys: HashMap<String, VecDeque<String>>,
    ttl: Duration,
    /// Maximum entries per tenant before LRU eviction kicks in.
    /// Defaults to `MAX_ENTRIES_PER_TENANT` (1000).
    max_entries_per_tenant: usize,
}

/// Default maximum entries per tenant for [`IdempotencyCache`].
const MAX_ENTRIES_PER_TENANT: usize = 1000;

/// Extract the tenant prefix from a cache key.
/// Returns `"_default"` if no colon separator is found.
fn tenant_from_key(key: &str) -> &str {
    key.split(':').next().unwrap_or("_default")
}

impl From<AutonomousEditAuditEntry> for crate::governance::audit::AuditLogEntry {
    fn from(e: AutonomousEditAuditEntry) -> Self {
        crate::governance::audit::AuditLogEntry {
            timestamp: e.timestamp,
            task_id: e.file_path.clone(),
            phase: "autonomous_edit".to_string(),
            agent: Some(e.agent),
            tool: None,
            decision: format!("approval={}", e.approval_reason),
            inputs: serde_json::json!({
                "file_path": e.file_path,
                "change_summary": e.change_summary,
                "reversible": e.reversible,
            }),
            outputs: None,
            error: None,
            confidence: Some(e.confidence_score),
            data_classification: None,
            compliance_tags: Vec::new(),
            retention_policy: None,
            correlation_id: None,
        }
    }
}

/// Records autonomous-edit audit events into the process-wide canonical sink
/// ([`crate::governance::audit::global_audit_log`]).
///
/// The former second NDJSON writer (`.goon/audit/audit.ndjson`) was removed:
/// all audit writers now share the single `ThreadSafeAuditLog` persistence
/// layer (`~/.goon/audit.ndjson`).
#[derive(Debug, Clone)]
pub struct AuditLogger;

impl AuditLogger {
    /// Create a logger. The argument is kept for API compatibility with the
    /// previous per-directory logger; persistence now goes to the global sink.
    pub fn new(_log_dir: PathBuf) -> Self {
        Self
    }

    /// Append one audit entry to the canonical global audit sink.
    pub fn record(&self, entry: &AutonomousEditAuditEntry) -> std::io::Result<()> {
        crate::governance::audit::global_audit_log().record(entry.clone().into());
        Ok(())
    }
}

impl IdempotencyCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            results: HashMap::new(),
            tenant_keys: HashMap::new(),
            ttl,
            max_entries_per_tenant: MAX_ENTRIES_PER_TENANT,
        }
    }

    pub fn get(&self, key: &str) -> Option<&IdempotentResult> {
        let entry = self.results.get(key)?;
        if entry.cached_at.elapsed() > self.ttl {
            return None;
        }
        Some(entry)
    }

    pub fn insert(&mut self, key: String, response: Value) {
        let tenant = tenant_from_key(&key).to_string();

        // Enforce per-tenant LRU cap: evict oldest entries for this tenant
        // until we're under the limit (plus one for the new entry).
        // VecDeque::pop_front() is O(1), unlike Vec::remove(0).
        let keys_for_tenant = self.tenant_keys.entry(tenant.clone()).or_default();
        if keys_for_tenant.len() >= self.max_entries_per_tenant {
            let to_evict = keys_for_tenant
                .len()
                .saturating_sub(self.max_entries_per_tenant)
                + 1;
            for _ in 0..to_evict {
                if let Some(oldest) = keys_for_tenant.pop_front() {
                    self.results.remove(&oldest);
                }
            }
        }

        // Record the insertion order for LRU eviction.
        keys_for_tenant.push_back(key.clone());

        self.results.insert(
            key,
            IdempotentResult {
                response,
                cached_at: Instant::now(),
            },
        );
    }
}

/// Sandbox level for governance policy enforcement.
///
/// Higher levels represent stricter isolation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SandboxLevel {
    /// No sandbox — all operations allowed.
    None,
    /// Basic sandbox — advisory warnings, some restrictions.
    Basic,
    /// Strict sandbox — enforced restrictions, read-only for dangerous ops.
    Strict,
    /// Isolated sandbox — maximum security, data exfiltration prevented.
    Isolated,
}

impl std::fmt::Display for SandboxLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SandboxLevel::None => write!(f, "none"),
            SandboxLevel::Basic => write!(f, "basic"),
            SandboxLevel::Strict => write!(f, "strict"),
            SandboxLevel::Isolated => write!(f, "isolated"),
        }
    }
}

impl SandboxLevel {
    /// Returns the numeric index of the sandbox level.
    /// Higher values represent stricter isolation.
    pub fn level_index(&self) -> u8 {
        match self {
            SandboxLevel::None => 0,
            SandboxLevel::Basic => 1,
            SandboxLevel::Strict => 2,
            SandboxLevel::Isolated => 3,
        }
    }
}

pub struct SandboxPolicy;

impl SandboxPolicy {
    /// Check if read_file operations are allowed at this security level
    pub fn can_execute_read_file(level: SandboxLevel) -> bool {
        match level {
            SandboxLevel::None => true,
            SandboxLevel::Basic => true,
            SandboxLevel::Strict => true,
            SandboxLevel::Isolated => true,
        }
    }

    /// Check if file search/pattern matching operations are allowed at this security level
    ///
    /// Search is a read-only operation, safe across most levels
    pub fn can_execute_search(level: SandboxLevel) -> bool {
        match level {
            SandboxLevel::None => true,
            SandboxLevel::Basic => true,
            SandboxLevel::Strict => true,
            SandboxLevel::Isolated => true,
        }
    }

    /// Check if write/modification/file-creation operations are allowed
    ///
    /// Write operations are potentially dangerous and scope-limited by level
    pub fn can_execute_write(level: SandboxLevel) -> bool {
        match level {
            SandboxLevel::None => true,
            SandboxLevel::Basic => true,
            SandboxLevel::Strict => false,
            SandboxLevel::Isolated => false,
        }
    }

    /// Check if shell/command/code execution is allowed at this security level
    ///
    /// Shell execution is most dangerous and only allowed in unrestricted mode
    pub fn can_execute_shell(level: SandboxLevel) -> bool {
        match level {
            SandboxLevel::None => true,
            SandboxLevel::Basic => false,
            SandboxLevel::Strict => false,
            SandboxLevel::Isolated => false,
        }
    }

    /// Check if outbound network operations (HTTP requests, DNS lookups, ping) are allowed.
    ///
    /// Network access is restricted at Strict/Isolated levels to prevent data exfiltration.
    pub fn can_execute_network(level: SandboxLevel) -> bool {
        match level {
            SandboxLevel::None => true,
            SandboxLevel::Basic => true,
            SandboxLevel::Strict => false,
            SandboxLevel::Isolated => false,
        }
    }

    /// Check whether a given operation is allowed at the given sandbox level,
    /// delegating to the specific `can_execute_*` method based on the operation name.
    ///
    /// For unknown operations, a warning is logged and `false` is returned.
    pub fn check(level: SandboxLevel, operation: &str) -> bool {
        match operation {
            "read" => Self::can_execute_read_file(level),
            "search" => Self::can_execute_search(level),
            "write" => Self::can_execute_write(level),
            "shell" => Self::can_execute_shell(level),
            "network" => Self::can_execute_network(level),
            _ => {
                tracing::warn!(
                    "SandboxPolicy: unknown operation '{}' at level {:?} — denying by default. Allowed operations: read, search, write, shell, network",
                    operation,
                    level,
                );
                false
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum GovernanceAction {
    Read,
    Search,
    Write,
    Shell,
    Network,
}

impl GovernanceAction {
    /// Return the action as a static string for sandbox policy checks.
    pub fn as_str(&self) -> &'static str {
        match self {
            GovernanceAction::Read => "read",
            GovernanceAction::Search => "search",
            GovernanceAction::Write => "write",
            GovernanceAction::Shell => "shell",
            GovernanceAction::Network => "network",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardeningDecision {
    pub allowed: bool,
    pub reason: String,
    pub policy_name: String,
    pub sandbox_level: SandboxLevel,
}

/// Resolve policy bundle from deployment target.
pub fn policy_bundle_for_target(target: Option<&str>) -> PolicyBundle {
    match target.unwrap_or("local-dev").to_ascii_lowercase().as_str() {
        "ci" | "ci-pipeline" => PolicyBundle::ci_pipeline(),
        "managed-service" | "managed" => PolicyBundle::managed_service(),
        "production" | "prod" => PolicyBundle::production_hardened(),
        _ => PolicyBundle::local_dev(),
    }
}

/// Enforce sandbox policy on a concrete action.
pub fn enforce_action(policy: &PolicyBundle, action: GovernanceAction) -> HardeningDecision {
    let allowed = match action {
        GovernanceAction::Read => SandboxPolicy::can_execute_read_file(policy.sandbox_level),
        GovernanceAction::Search => SandboxPolicy::can_execute_search(policy.sandbox_level),
        GovernanceAction::Write => SandboxPolicy::can_execute_write(policy.sandbox_level),
        GovernanceAction::Shell => SandboxPolicy::can_execute_shell(policy.sandbox_level),
        GovernanceAction::Network => SandboxPolicy::can_execute_network(policy.sandbox_level),
    };

    let action_label = match action {
        GovernanceAction::Read => "read",
        GovernanceAction::Search => "search",
        GovernanceAction::Write => "write",
        GovernanceAction::Shell => "shell",
        GovernanceAction::Network => "network",
    };

    HardeningDecision {
        allowed,
        reason: if allowed {
            format!("policy '{}' allows {} action", policy.name, action_label)
        } else {
            format!(
                "policy '{}' denied {} action at sandbox level '{}'",
                policy.name, action_label, policy.sandbox_level
            )
        },
        policy_name: policy.name.clone(),
        sandbox_level: policy.sandbox_level,
    }
}

/// Fallback authorization when RBAC enforcer is unavailable.
///
/// This is deployment-policy driven to avoid implicit allow-all behavior.
pub fn rbac_fallback_allows_action(
    deployment_target: Option<&str>,
    action: GovernanceAction,
) -> HardeningDecision {
    let policy = policy_bundle_for_target(deployment_target);
    let mut decision = enforce_action(&policy, action);
    decision.reason = format!(
        "RBAC unavailable; applying deployment fallback policy '{}': {}",
        decision.policy_name, decision.reason
    );
    decision
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pua::{PuaEnforcementPlan, PuaRuleEngine};
    use std::sync::{Arc, Mutex as StdMutex};

    #[test]
    fn policy_bundle_for_target_maps_ci_and_managed() {
        assert_eq!(
            policy_bundle_for_target(Some("ci")).sandbox_level,
            SandboxLevel::Basic
        );
        assert_eq!(
            policy_bundle_for_target(Some("managed-service")).sandbox_level,
            SandboxLevel::Strict
        );
    }

    #[test]
    fn strict_policy_denies_write_and_shell() {
        let policy = policy_bundle_for_target(Some("managed-service"));
        assert!(!enforce_action(&policy, GovernanceAction::Write).allowed);
        assert!(!enforce_action(&policy, GovernanceAction::Shell).allowed);
    }

    #[test]
    fn rbac_fallback_respects_deployment_policy() {
        let local_write = rbac_fallback_allows_action(Some("local-dev"), GovernanceAction::Write);
        assert!(local_write.allowed);

        let managed_write =
            rbac_fallback_allows_action(Some("managed-service"), GovernanceAction::Write);
        assert!(!managed_write.allowed);

        let managed_read =
            rbac_fallback_allows_action(Some("managed-service"), GovernanceAction::Read);
        assert!(managed_read.allowed);
    }

    #[test]
    fn budget_tracker_rejects_on_token_overflow() {
        let budget = TaskBudget {
            max_tokens: 100,
            max_wall_clock_seconds: 60,
            max_tool_calls: 10,
            max_api_calls: 10,
        };
        let mut tracker = BudgetTracker::new(budget);
        assert!(tracker.record_tokens(101).is_err());
    }

    #[test]
    fn budget_tracker_allows_within_limit_and_reports_remaining() {
        let budget = TaskBudget {
            max_tokens: 100,
            max_wall_clock_seconds: 60,
            max_tool_calls: 2,
            max_api_calls: 10,
        };
        let mut tracker = BudgetTracker::new(budget);
        assert!(tracker.record_tokens(40).is_ok());
        assert_eq!(tracker.remaining_tokens(), 60);
        assert!(tracker.record_tool_call().is_ok());
        assert!(tracker.record_tool_call().is_ok());
        assert!(tracker.record_tool_call().is_err());
    }

    #[test]
    fn budget_tracker_token_overflow_escalates_pua_level() {
        let budget = TaskBudget {
            max_tokens: 100,
            max_wall_clock_seconds: 60,
            max_tool_calls: 10,
            max_api_calls: 10,
        };
        let mut tracker = BudgetTracker::new(budget);
        let plan = Arc::new(StdMutex::new(PuaEnforcementPlan {
            escalation_level: "L1".to_string(),
            mandatory_roles: vec![],
            red_lines: vec![],
            quality_compass: vec![],
            mandatory_safeguards: vec![],
            mandatory_evidence: vec![],
            stage_requirements: vec![],
        }));
        let engine = PuaRuleEngine::new(plan.clone());

        assert!(tracker.consume_with_pua(101, &engine).is_err());
        let escalation_level = plan
            .lock()
            .expect("plan lock should succeed")
            .escalation_level
            .clone();
        assert_eq!(escalation_level, "L2");
    }

    #[test]
    fn budget_tracker_token_overflow_escalation_capped_at_l5() {
        let budget = TaskBudget {
            max_tokens: 100,
            max_wall_clock_seconds: 60,
            max_tool_calls: 10,
            max_api_calls: 10,
        };
        let mut tracker = BudgetTracker::new(budget);
        let plan = Arc::new(StdMutex::new(PuaEnforcementPlan {
            escalation_level: "L5".to_string(),
            mandatory_roles: vec![],
            red_lines: vec![],
            quality_compass: vec![],
            mandatory_safeguards: vec![],
            mandatory_evidence: vec![],
            stage_requirements: vec![],
        }));
        let engine = PuaRuleEngine::new(plan.clone());

        assert!(tracker.consume_with_pua(101, &engine).is_err());
        let escalation_level = plan
            .lock()
            .expect("plan lock should succeed")
            .escalation_level
            .clone();
        assert_eq!(escalation_level, "L5");
    }

    #[test]
    fn idempotency_cache_returns_cached_result_within_ttl() {
        let mut cache = IdempotencyCache::new(Duration::from_secs(300));
        cache.insert("k1".to_string(), serde_json::json!({"ok": true}));
        let value = cache
            .get("k1")
            .expect("idempotency cache should hit within ttl");
        assert_eq!(
            value.response.get("ok").and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn idempotency_cache_expires_entry_after_ttl() {
        // Expiry is enforced lazily in `get` (the TTL check on read path);
        // there is no separate eviction pass.
        let mut cache = IdempotencyCache::new(Duration::from_millis(0));
        cache.insert("k1".to_string(), serde_json::json!({"ok": true}));
        assert!(cache.get("k1").is_none());
    }

    #[test]
    fn audit_logger_writes_to_global_sink_via_from_conversion() {
        let logger = AuditLogger::new(PathBuf::from(".goon/audit"));

        let entry = AutonomousEditAuditEntry {
            timestamp: "2026-04-14T00:00:00Z".to_string(),
            agent: "mcp.tools.call".to_string(),
            file_path: "src/main.rs".to_string(),
            change_summary: "write_file success".to_string(),
            approval_reason: "policy local-dev allows write".to_string(),
            confidence_score: 0.9,
            reversible: true,
        };

        logger.record(&entry).expect("record should succeed");

        // The entry lands in the canonical global sink with the file path
        // preserved in `task_id` (see the From conversion).
        let entries = crate::governance::audit::global_audit_log().entries();
        let last = entries.last().expect("global sink should have the entry");
        assert_eq!(last.task_id, "src/main.rs");
        assert_eq!(last.phase, "autonomous_edit");
        assert_eq!(last.agent.as_deref(), Some("mcp.tools.call"));
        assert_eq!(last.inputs["change_summary"], "write_file success");
    }

    #[test]
    fn audit_logger_query_by_path_filters_correctly() {
        // The old query_by_path reader was removed together with the second
        // NDJSON writer; the From conversion must preserve the file path in
        // the canonical AuditLogEntry so the info survives in the single sink.
        let source = AutonomousEditAuditEntry {
            timestamp: "2026-04-14T00:00:01Z".to_string(),
            agent: "mcp.tools.call".to_string(),
            file_path: "src/lib.rs".to_string(),
            change_summary: "read_file success".to_string(),
            approval_reason: "ok".to_string(),
            confidence_score: 1.0,
            reversible: true,
        };
        let entry: crate::governance::audit::AuditLogEntry = source.into();
        assert_eq!(entry.task_id, "src/lib.rs");
        assert_eq!(entry.phase, "autonomous_edit");
        assert!(entry.inputs["change_summary"].as_str().is_some());
    }
}
