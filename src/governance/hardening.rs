//! Hardening — F-GAP-08
//!
//! Phase 9: Production Hardening and Safety
//! These structures are intentional framework definitions for Phase 0-9 architecture.
//! Budget enforcement, quotas, and policies will be applied by the execution engine
//! once resource tracking and policy enforcement hooks are implemented.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantResourceQuota {
    pub tenant_id: String,
    pub daily_token_limit: usize,
    pub concurrent_tasks_limit: usize,
    pub daily_api_call_limit: usize,
}

/// Tracks per-tenant resource usage and enforces quotas.
/// Used by CapabilityBus to reject tasks when a tenant exceeds its limits.
#[derive(Debug, Default)]
pub struct TenantBudgetEnforcer {
    quotas: HashMap<String, TenantResourceQuota>,
    token_usage: RefCell<HashMap<String, usize>>,
    api_call_usage: RefCell<HashMap<String, usize>>,
    active_tasks: HashMap<String, usize>,
    /// The "day number" (unix_ts / 86400) last observed, used to reset daily counters.
    current_day: Cell<i64>,
}

impl TenantBudgetEnforcer {
    pub fn new() -> Self {
        Self {
            quotas: HashMap::new(),
            token_usage: RefCell::new(HashMap::new()),
            api_call_usage: RefCell::new(HashMap::new()),
            active_tasks: HashMap::new(),
            current_day: Cell::new(Self::today()),
        }
    }

    /// Return the current day number (unix timestamp / 86400).
    fn today() -> i64 {
        crate::acp::prelude::now_ts() / 86400
    }

    /// Reset daily counters if the day has changed.
    fn reset_daily_if_day_changed(&self) {
        let today = Self::today();
        if today != self.current_day.get() {
            self.token_usage.borrow_mut().clear();
            self.api_call_usage.borrow_mut().clear();
            self.current_day.set(today);
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

        let current_tasks = self.active_tasks.get(tenant_id).copied().unwrap_or(0);
        if current_tasks >= quota.concurrent_tasks_limit {
            return Err(format!(
                "tenant '{}' at concurrent task limit ({}/{})",
                tenant_id, current_tasks, quota.concurrent_tasks_limit
            ));
        }

        let tokens = self
            .token_usage
            .borrow()
            .get(tenant_id)
            .copied()
            .unwrap_or(0);
        if tokens >= quota.daily_token_limit {
            return Err(format!(
                "tenant '{}' exceeded daily token limit ({}/{})",
                tenant_id, tokens, quota.daily_token_limit
            ));
        }

        let calls = self
            .api_call_usage
            .borrow()
            .get(tenant_id)
            .copied()
            .unwrap_or(0);
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

    /// Record that a tenant started a task.
    pub fn start_task(&mut self, tenant_id: &str) {
        *self.active_tasks.entry(tenant_id.to_string()).or_insert(0) += 1;
    }

    /// Record resource consumption after a task completes.
    pub fn record_usage(&mut self, tenant_id: &str, tokens: usize, api_calls: usize) {
        *self
            .token_usage
            .borrow_mut()
            .entry(tenant_id.to_string())
            .or_insert(0) += tokens;
        *self
            .api_call_usage
            .borrow_mut()
            .entry(tenant_id.to_string())
            .or_insert(0) += api_calls;
        let tasks = self.active_tasks.entry(tenant_id.to_string()).or_insert(0);
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
    pub sandbox_level: String, // "none", "basic", "strict"
}

impl PolicyBundle {
    pub fn local_dev() -> Self {
        Self {
            name: "local-dev".to_string(),
            deployment_target: "local-dev".to_string(),
            max_autonomy: "edit".to_string(),
            require_approval_for_write: false,
            enable_code_execution: true,
            sandbox_level: "none".to_string(),
        }
    }

    pub fn ci_pipeline() -> Self {
        Self {
            name: "ci-pipeline".to_string(),
            deployment_target: "ci".to_string(),
            max_autonomy: "agent".to_string(),
            require_approval_for_write: true,
            enable_code_execution: true,
            sandbox_level: "basic".to_string(),
        }
    }

    pub fn managed_service() -> Self {
        Self {
            name: "managed-service".to_string(),
            deployment_target: "managed-service".to_string(),
            max_autonomy: "edit".to_string(),
            require_approval_for_write: true,
            enable_code_execution: false,
            sandbox_level: "strict".to_string(),
        }
    }

    pub fn production_hardened() -> Self {
        Self {
            name: "production-hardened".to_string(),
            deployment_target: "production".to_string(),
            max_autonomy: "agent".to_string(),
            require_approval_for_write: true,
            enable_code_execution: false,
            sandbox_level: "isolated".to_string(),
        }
    }
}

pub struct Idempotency;
impl Idempotency {
    /// Generate idempotency key from task parameters
    pub fn key(task_id: &str, phase: &str, objective: &str) -> String {
        // Simple hash-based idempotency key generation
        format!("{}-{}-{:x}", task_id, phase, objective.len())
    }
}

#[derive(Debug, Clone)]
pub struct IdempotentResult {
    pub response: Value,
    pub cached_at: Instant,
}

#[derive(Debug, Clone)]
pub struct IdempotencyCache {
    results: HashMap<String, IdempotentResult>,
    ttl: Duration,
}

#[derive(Debug, Clone)]
pub struct AuditLogger {
    log_dir: PathBuf,
}

impl AuditLogger {
    pub fn new(log_dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&log_dir);
        Self { log_dir }
    }

    /// Append one audit entry in NDJSON format.
    pub fn record(&self, entry: &AutonomousEditAuditEntry) -> std::io::Result<()> {
        fs::create_dir_all(&self.log_dir)?;
        let path = self.log_dir.join("audit.ndjson");
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        let line = serde_json::to_string(entry).map_err(std::io::Error::other)?;
        writeln!(file, "{}", line)?;
        Ok(())
    }

    /// Read the latest `limit` audit entries.
    pub fn recent(&self, limit: usize) -> std::io::Result<Vec<AutonomousEditAuditEntry>> {
        let mut entries = self.read_all_entries()?;
        if entries.len() > limit {
            entries = entries.split_off(entries.len().saturating_sub(limit));
        }
        Ok(entries)
    }

    /// Query audit entries by exact file path match.
    pub fn query_by_path(&self, file_path: &str) -> std::io::Result<Vec<AutonomousEditAuditEntry>> {
        let entries = self.read_all_entries()?;
        Ok(entries
            .into_iter()
            .filter(|entry| entry.file_path == file_path)
            .collect())
    }

    fn read_all_entries(&self) -> std::io::Result<Vec<AutonomousEditAuditEntry>> {
        if !self.log_dir.exists() {
            return Ok(Vec::new());
        }
        let mut files = list_ndjson_files(&self.log_dir)?;
        files.sort();

        let mut entries = Vec::new();
        for file in files {
            let file = File::open(file)?;
            let reader = BufReader::new(file);
            for line in reader.lines() {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                if let Ok(entry) = serde_json::from_str::<AutonomousEditAuditEntry>(&line) {
                    entries.push(entry);
                }
            }
        }
        Ok(entries)
    }
}

fn list_ndjson_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("ndjson"))
            .unwrap_or(false)
        {
            files.push(path);
        }
    }
    Ok(files)
}

impl IdempotencyCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            results: HashMap::new(),
            ttl,
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
        self.results.insert(
            key,
            IdempotentResult {
                response,
                cached_at: Instant::now(),
            },
        );
    }

    pub fn evict_expired(&mut self) {
        let ttl = self.ttl;
        self.results
            .retain(|_, value| value.cached_at.elapsed() <= ttl);
    }
}

pub struct SandboxPolicy;
impl SandboxPolicy {
    /// Check if read_file operations are allowed at this security level
    ///
    /// Security levels: "none" (unrestricted) -> "basic" (limited) -> "strict" (standard) -> "isolated" (production)
    pub fn can_execute_read_file(level: &str) -> bool {
        match level {
            "none" => true,     // Unrestricted: allow all read operations
            "basic" => true,    // Basic: allow read (safe, read-only operation)
            "strict" => true,   // Strict: still allow reads (non-destructive)
            "isolated" => true, // Isolated: allow reads (safe, read-only)
            _ => false,         // Unknown level: deny by default (fail-safe)
        }
    }

    /// Check if file search/pattern matching operations are allowed at this security level
    ///
    /// Search is a read-only operation, safe across all levels
    pub fn can_execute_search(level: &str) -> bool {
        match level {
            "none" => true,     // Unrestricted: allow all searches
            "basic" => true,    // Basic: allow search (read-only, safe operation)
            "strict" => true,   // Strict: allow search (read-only, non-destructive)
            "isolated" => true, // Isolated: allow search (read-only, non-destructive)
            _ => false,         // Unknown level: deny by default
        }
    }

    /// Check if write/modification/file-creation operations are allowed
    ///
    /// Write operations are potentially dangerous and scope-limited by level
    pub fn can_execute_write(level: &str) -> bool {
        match level {
            "none" => true,      // Unrestricted: allow all writes
            "basic" => true,     // Basic: allow writes (but with audit/approval gates)
            "strict" => false,   // Strict: deny writes (read-only enforcement)
            "isolated" => false, // Isolated: deny writes (read-only enforcement, prod hardened)
            _ => false,          // Unknown level: deny by default (fail-safe)
        }
    }

    /// Check if shell/command/code execution is allowed at this security level
    ///
    /// Shell execution is most dangerous and only allowed in unrestricted mode
    pub fn can_execute_shell(level: &str) -> bool {
        match level {
            "none" => true,      // Unrestricted: allow shell/code execution
            "basic" => false,    // Basic: deny shell (too dangerous, use restricted APIs)
            "strict" => false,   // Strict: deny shell execution (locked down)
            "isolated" => false, // Isolated: deny shell execution (locked down, production)
            _ => false,          // Unknown level: deny by default (fail-safe)
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum GovernanceAction {
    Read,
    Search,
    Write,
    Shell,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardeningDecision {
    pub allowed: bool,
    pub reason: String,
    pub policy_name: String,
    pub sandbox_level: String,
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
        GovernanceAction::Read => SandboxPolicy::can_execute_read_file(&policy.sandbox_level),
        GovernanceAction::Search => SandboxPolicy::can_execute_search(&policy.sandbox_level),
        GovernanceAction::Write => SandboxPolicy::can_execute_write(&policy.sandbox_level),
        GovernanceAction::Shell => SandboxPolicy::can_execute_shell(&policy.sandbox_level),
    };

    let action_label = match action {
        GovernanceAction::Read => "read",
        GovernanceAction::Search => "search",
        GovernanceAction::Write => "write",
        GovernanceAction::Shell => "shell",
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
        sandbox_level: policy.sandbox_level.clone(),
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
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn policy_bundle_for_target_maps_ci_and_managed() {
        assert_eq!(policy_bundle_for_target(Some("ci")).sandbox_level, "basic");
        assert_eq!(
            policy_bundle_for_target(Some("managed-service")).sandbox_level,
            "strict"
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
    fn idempotency_cache_evicts_expired_entries() {
        let mut cache = IdempotencyCache::new(Duration::from_millis(0));
        cache.insert("k1".to_string(), serde_json::json!({"ok": true}));
        cache.evict_expired();
        assert!(cache.get("k1").is_none());
    }

    #[test]
    fn idempotency_key_is_deterministic() {
        let k1 = Idempotency::key("task-1", "phase-a", "build the feature");
        let k2 = Idempotency::key("task-1", "phase-a", "build the feature");
        assert_eq!(k1, k2);
    }

    #[test]
    fn audit_logger_writes_and_reads_back_entry() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("goon-audit-test-{unique}"));
        let logger = AuditLogger::new(dir.clone());

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
        let items = logger.recent(1).expect("recent should succeed");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].file_path, "src/main.rs");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn audit_logger_query_by_path_filters_correctly() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("goon-audit-query-test-{unique}"));
        let logger = AuditLogger::new(dir.clone());

        logger
            .record(&AutonomousEditAuditEntry {
                timestamp: "2026-04-14T00:00:00Z".to_string(),
                agent: "mcp.tools.call".to_string(),
                file_path: "src/main.rs".to_string(),
                change_summary: "write_file success".to_string(),
                approval_reason: "ok".to_string(),
                confidence_score: 0.8,
                reversible: true,
            })
            .expect("record #1 should succeed");
        logger
            .record(&AutonomousEditAuditEntry {
                timestamp: "2026-04-14T00:00:01Z".to_string(),
                agent: "mcp.tools.call".to_string(),
                file_path: "src/lib.rs".to_string(),
                change_summary: "read_file success".to_string(),
                approval_reason: "ok".to_string(),
                confidence_score: 1.0,
                reversible: true,
            })
            .expect("record #2 should succeed");

        let filtered = logger
            .query_by_path("src/main.rs")
            .expect("query should succeed");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].file_path, "src/main.rs");

        let _ = fs::remove_dir_all(dir);
    }
}
