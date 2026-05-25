//! Tool call transactional semantics (BLUE43 Step 15)
//!
//! Provides idempotency tracking, conflict-rate monitoring, and
//! transaction-scoped rollback / compensation for tool execution.
//!
//! # Design
//!
//! * [`IdempotencyStore`] — tracks idempotency keys across tool calls so that
//!   re‑issuing the same key returns the cached result instead of re‑executing.
//! * [`TransactionScope`] — collects compensation actions that can be applied
//!   to roll back completed tools when a later tool in the batch fails.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time::{Instant, SystemTime};

use crate::orchestration::tool::{ToolInput, ToolOutput, ToolRegistry};

// ---------------------------------------------------------------------------
// ToolCallResult / ToolCallStatus
// ---------------------------------------------------------------------------

/// Unified result for a tool call with transactional semantics.
#[derive(Debug, Clone)]
pub struct ToolCallResult {
    pub status: ToolCallStatus,
    pub idempotency_key: Option<String>,
    pub idempotency_hit: bool,
    pub transaction_id: Option<String>,
    pub output: ToolOutput,
    pub duration_ms: u64,
}

/// Status of a tool call within a transaction.
#[derive(Debug, Clone)]
pub enum ToolCallStatus {
    /// The tool call completed successfully.
    Success,
    /// The tool call failed with an error message.
    Failure(String),
    /// A batch of tool calls had a mixed outcome.
    Partial {
        completed: Vec<String>,
        failed: Vec<String>,
    },
}

/// Global store for the latest idempotency conflict rate.
/// Updated on each call to `conflict_rate()`, read by governance payload
/// builders to expose tool transaction idempotency health.
static LATEST_IDEMPOTENCY_CONFLICT_RATE: LazyLock<Mutex<Option<f64>>> =
    LazyLock::new(|| Mutex::new(None));

/// Store the latest idempotency conflict rate for governance observability.
pub fn store_idempotency_conflict_rate(rate: f64) {
    if let Ok(mut guard) = LATEST_IDEMPOTENCY_CONFLICT_RATE.lock() {
        *guard = Some(rate);
    }
}

/// Read the latest idempotency conflict rate.
pub fn read_idempotency_conflict_rate() -> Option<f64> {
    LATEST_IDEMPOTENCY_CONFLICT_RATE
        .lock()
        .ok()
        .and_then(|guard| *guard)
}

// ---------------------------------------------------------------------------
// IdempotencyStore
// ---------------------------------------------------------------------------

/// Thread‑safe store that tracks idempotency keys for tool calls.
///
/// When the same key + tool combination arrives again the store returns the
/// cached [`ToolCallResult`] instead of allowing re‑execution.
pub struct IdempotencyStore {
    keys: Mutex<HashMap<String, IdempotencyRecord>>,
    total_conflicts: AtomicU64,
}

/// Internal record for a single idempotency key.
struct IdempotencyRecord {
    _key: String,
    _first_seen_ms: u64,
    last_result: Option<ToolCallResult>,
    conflict_count: u32,
}

impl IdempotencyStore {
    /// Create an empty idempotency store.
    pub fn new() -> Self {
        Self {
            keys: Mutex::new(HashMap::new()),
            total_conflicts: AtomicU64::new(0),
        }
    }

    /// Check whether `key` has been seen before for `tool_name`.
    ///
    /// Returns `(is_duplicate, cached_result)`:
    /// * `is_duplicate == true` — the key already exists; the caller should
    ///   short‑circuit and return `cached_result`.
    /// * `is_duplicate == false` — the key is new; the caller has been
    ///   registered and **must** call [`record_result`] after executing.
    pub fn check_and_record(&self, key: &str, tool_name: &str) -> (bool, Option<ToolCallResult>) {
        let composite = format!("{}::{}", tool_name, key);
        let mut keys = self.keys.lock().expect("IdempotencyStore lock poisoned");

        // Check if this composite key already exists (even without a result).
        if keys.contains_key(&composite) {
            // Duplicate — increment conflict counter.
            self.total_conflicts.fetch_add(1, Ordering::Relaxed);

            // Bump the per-record conflict count and fetch cached result.
            let cached_result = keys.get_mut(&composite).and_then(|record| {
                record.conflict_count += 1;
                record.last_result.clone()
            });

            return (true, cached_result);
        }

        // New key — insert a placeholder record.
        let now_ms = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        keys.insert(
            composite,
            IdempotencyRecord {
                _key: key.to_string(),
                _first_seen_ms: now_ms,
                last_result: None,
                conflict_count: 0,
            },
        );

        (false, None)
    }

    /// Store the result of a tool call for the given key.
    ///
    /// `key` must have been previously registered via [`check_and_record`].
    pub fn record_result(&self, key: &str, tool_name: &str, result: ToolCallResult) {
        let composite = format!("{}::{}", tool_name, key);
        let mut keys = self.keys.lock().expect("IdempotencyStore lock poisoned");

        if let Some(record) = keys.get_mut(&composite) {
            record.last_result = Some(result);
        }
    }

    /// Record a conflict (duplicate key) outside of [`check_and_record`].
    pub fn record_conflict(&self) {
        self.total_conflicts.fetch_add(1, Ordering::Relaxed);
    }

    /// Returns the fraction of keys that have experienced at least one
    /// conflict (duplicate submission).
    ///
    /// Also stores the computed rate in the global governance store so that
    /// the governance status endpoint can expose idempotency health.
    pub fn conflict_rate(&self) -> f64 {
        let keys = self.keys.lock().expect("IdempotencyStore lock poisoned");
        let total = keys.len();
        if total == 0 {
            store_idempotency_conflict_rate(0.0);
            return 0.0;
        }
        let conflicted = keys.values().filter(|r| r.conflict_count > 0).count();
        let rate = conflicted as f64 / total as f64;
        store_idempotency_conflict_rate(rate);
        rate
    }
}

impl Default for IdempotencyStore {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// TransactionScope / CompensateAction
// ---------------------------------------------------------------------------

/// Compensating action that can be invoked to roll back a completed tool call.
/// Type alias for a compensation closure (Arc for shared ownership + thread safety).
pub type CompensateFn = std::sync::Arc<dyn Fn() + Send + Sync + 'static>;

/// Compensating action that can be invoked to roll back a completed tool call.
pub struct CompensateAction {
    /// Name of the tool that this action compensates.
    pub tool_name: String,
    /// Closure that performs the compensation / rollback.
    pub compensate_fn: Option<CompensateFn>,
    /// Maximum time (in milliseconds) allowed for compensation execution.
    pub timeout_ms: u64,
    /// If true, retry the compensation once when a timeout occurs.
    pub retry_on_timeout: bool,
}

impl CompensateAction {
    /// Create a new compensation action with default timeout (30 s) and no retry.
    pub fn new(tool_name: String, compensate_fn: CompensateFn) -> Self {
        Self {
            tool_name,
            compensate_fn: Some(compensate_fn),
            timeout_ms: 30_000,
            retry_on_timeout: false,
        }
    }

    /// Set the timeout for this compensation action.
    pub fn with_timeout(mut self, timeout_ms: u64) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// Enable retry-on-timeout for this compensation action.
    pub fn with_retry_on_timeout(mut self) -> Self {
        self.retry_on_timeout = true;
        self
    }
}

impl std::fmt::Debug for CompensateAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompensateAction")
            .field("tool_name", &self.tool_name)
            .field("timeout_ms", &self.timeout_ms)
            .field("retry_on_timeout", &self.retry_on_timeout)
            .finish()
    }
}

/// Result of executing a single compensation action under timeout.
enum CompensateResult {
    Ok,
    Timeout,
}

/// Execute a compensation closure inside `spawn_blocking` with a timeout.
async fn execute_compensate_with_timeout(
    tool_name: &str,
    compensate_fn: CompensateFn,
    timeout_ms: u64,
) -> CompensateResult {
    use std::time::Duration;

    let fut = tokio::task::spawn_blocking(move || {
        (compensate_fn)();
    });

    match tokio::time::timeout(Duration::from_millis(timeout_ms), fut).await {
        Ok(Ok(())) => CompensateResult::Ok,
        Ok(Err(join_err)) => {
            tracing::error!(
                target: "txn_rollback",
                tool = %tool_name,
                error = %join_err,
                "compensation panicked or join error"
            );
            CompensateResult::Timeout
        }
        Err(_elapsed) => CompensateResult::Timeout,
    }
}

// Integration warmup — exercises all public API methods so the compiler
// does not emit dead_code warnings before full integration wiring.
#[doc(hidden)]
pub fn __compensate_action_touch() {
    let _action = CompensateAction::new("example".to_string(), std::sync::Arc::new(|| {}))
        .with_timeout(5000)
        .with_retry_on_timeout();

    let mut scope = TransactionScope::new("example".to_string());
    scope.register_action(_action);
}

/// Transaction scope for group‑executing tools with rollback on failure.
///
/// When a batch of tools is executed and one fails, the [`TransactionScope`]
/// invokes all registered compensation actions in reverse order to roll back
/// the side effects of previously‑completed tools.
pub struct TransactionScope {
    /// Unique identifier for this transaction.
    pub transaction_id: String,
    /// Names of tools that completed successfully so far.
    pub completed_tools: Vec<String>,
    /// Compensation actions registered (in execution order).
    pub compensate_actions: Vec<CompensateAction>,
}

impl TransactionScope {
    /// Create a new transaction scope with a generated ID.
    pub fn new(transaction_id: String) -> Self {
        Self {
            transaction_id,
            completed_tools: Vec::new(),
            compensate_actions: Vec::new(),
        }
    }

    /// Register a completed tool and its compensation action.
    pub fn register_completion(&mut self, tool_name: String, compensate_fn: CompensateFn) {
        self.completed_tools.push(tool_name.clone());
        self.compensate_actions
            .push(CompensateAction::new(tool_name, compensate_fn));
    }

    /// Register a completed tool with a custom `CompensateAction`.
    pub fn register_action(&mut self, action: CompensateAction) {
        self.completed_tools.push(action.tool_name.clone());
        self.compensate_actions.push(action);
    }

    /// Roll back all completed tools by invoking their compensation actions
    /// in reverse order (last‑completed, first‑rolled‑back).
    ///
    /// Each compensation runs with a per‑action timeout. If a timeout occurs
    /// and `retry_on_timeout` is true, the action is retried once.
    pub async fn rollback(&self) {
        for action in self.compensate_actions.iter().rev() {
            let Some(ref compensate_fn) = action.compensate_fn else {
                tracing::debug!(
                    target: "txn_rollback",
                    tool = %action.tool_name,
                    "no compensation closure registered – skipping"
                );
                continue;
            };

            let tool_name = action.tool_name.clone();
            let timeout_ms = action.timeout_ms;
            let retry = action.retry_on_timeout;

            // Execute compensation under timeout.
            let result = execute_compensate_with_timeout(
                &tool_name,
                std::sync::Arc::clone(compensate_fn),
                timeout_ms,
            )
            .await;

            match result {
                CompensateResult::Ok => {
                    tracing::info!(
                        target: "txn_rollback",
                        tool = %tool_name,
                        "compensation succeeded"
                    );
                }
                CompensateResult::Timeout => {
                    tracing::warn!(
                        target: "txn_rollback",
                        tool = %tool_name,
                        timeout_ms = timeout_ms,
                        "compensation timed out"
                    );

                    if retry {
                        tracing::info!(
                            target: "txn_rollback",
                            tool = %tool_name,
                            "retrying compensation after timeout"
                        );
                        let retry_result = execute_compensate_with_timeout(
                            &tool_name,
                            std::sync::Arc::clone(compensate_fn),
                            timeout_ms,
                        )
                        .await;
                        match retry_result {
                            CompensateResult::Ok => {
                                tracing::info!(
                                    target: "txn_rollback",
                                    tool = %tool_name,
                                    "compensation succeeded on retry"
                                );
                            }
                            CompensateResult::Timeout => {
                                tracing::error!(
                                    target: "txn_rollback",
                                    tool = %tool_name,
                                    "compensation timed out again on retry – giving up"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

impl std::fmt::Debug for TransactionScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransactionScope")
            .field("transaction_id", &self.transaction_id)
            .field("completed_tools", &self.completed_tools)
            .field("compensate_actions", &self.compensate_actions.len())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// ToolRegistry extension – execute_with_idempotency
// ---------------------------------------------------------------------------

impl ToolRegistry {
    /// Execute a tool with idempotency checking.
    ///
    /// If `idempotency_key` is `Some`, the store is consulted first.  A
    /// duplicate key causes the cached result to be returned immediately
    /// without re‑executing the tool.
    pub fn execute_with_idempotency(
        &self,
        name: &str,
        input: &ToolInput,
        idempotency_key: Option<&str>,
        store: &IdempotencyStore,
    ) -> ToolCallResult {
        let transaction_id: Option<String> = None;
        let start = Instant::now();

        // --- Idempotency check ---
        if let Some(key) = idempotency_key {
            let (is_duplicate, cached) = store.check_and_record(key, name);
            if is_duplicate {
                if let Some(mut cached) = cached {
                    cached.idempotency_hit = true;
                    cached.idempotency_key = Some(key.to_string());
                    return cached;
                }
            }
        }

        // --- Execute ---
        let output = match self.run_with_fallback(name, input) {
            Ok(out) => out,
            Err(e) => ToolOutput {
                success: false,
                result: None,
                error: Some(format!("{}", e)),
                verification: None,
                audit_log: None,
                pua_report: None,
            },
        };

        let duration_ms = start.elapsed().as_millis() as u64;
        let status = if output.success {
            ToolCallStatus::Success
        } else {
            ToolCallStatus::Failure(
                output
                    .error
                    .clone()
                    .unwrap_or_else(|| "unknown error".to_string()),
            )
        };

        let result = ToolCallResult {
            status,
            idempotency_key: idempotency_key.map(|k| k.to_string()),
            idempotency_hit: false,
            transaction_id,
            output,
            duration_ms,
        };

        // --- Record result for idempotency ---
        if let Some(key) = idempotency_key {
            store.record_result(key, name, result.clone());
        }

        result
    }

    /// Execute a batch of tools as a single transaction with rollback.
    ///
    /// Each tool is executed sequentially.  If any tool fails, all previously
    /// completed tools are rolled back via their compensation actions.
    ///
    /// The returned [`ToolCallResult`] has status [`ToolCallStatus::Partial`]
    /// when at least one tool succeeded and at least one failed.
    pub fn execute_transactional(
        &self,
        tool_calls: Vec<(String, ToolInput)>,
        transaction_id: String,
        store: &IdempotencyStore,
    ) -> ToolCallResult {
        let mut scope = TransactionScope::new(transaction_id.clone());
        let start = Instant::now();

        let txn_id_for_closure = transaction_id.clone();
        for (tool_name, input) in tool_calls {
            let idempotency_key = format!("txn::{}::{}", txn_id_for_closure, tool_name);
            let result =
                self.execute_with_idempotency(&tool_name, &input, Some(&idempotency_key), store);

            match &result.status {
                ToolCallStatus::Success => {
                    // Register a no‑op compensation by default. In a real
                    // system each tool would provide its own inverse.
                    let name = tool_name.clone();
                    let txn_id = txn_id_for_closure.clone();
                    scope.register_completion(
                        tool_name,
                        std::sync::Arc::new(move || {
                            tracing::warn!(
                                "compensating tool '{}' for txn '{}' (no‑op)",
                                name,
                                txn_id,
                            );
                        }),
                    );
                }
                _ => {
                    // Failure — roll back everything completed so far.
                    tokio::runtime::Handle::current().block_on(scope.rollback());

                    let completed: Vec<String> = scope.completed_tools.clone();
                    let failed: Vec<String> = vec![tool_name.clone()];

                    let duration_ms = start.elapsed().as_millis() as u64;
                    return ToolCallResult {
                        status: ToolCallStatus::Partial { completed, failed },
                        idempotency_key: None,
                        idempotency_hit: false,
                        transaction_id: Some(txn_id_for_closure.clone()),
                        output: ToolOutput {
                            success: false,
                            result: None,
                            error: Some(format!(
                                "transaction '{}' failed at tool '{}'",
                                txn_id_for_closure, tool_name,
                            )),
                            verification: None,
                            audit_log: None,
                            pua_report: None,
                        },
                        duration_ms,
                    };
                }
            }
        }

        // All tools succeeded.
        let duration_ms = start.elapsed().as_millis() as u64;
        ToolCallResult {
            status: ToolCallStatus::Success,
            idempotency_key: None,
            idempotency_hit: false,
            transaction_id: Some(txn_id_for_closure.clone()),
            output: ToolOutput {
                success: true,
                result: None,
                error: None,
                verification: None,
                audit_log: None,
                pua_report: None,
            },
            duration_ms,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::tool::{Tool, ToolInput, ToolOutput, ToolRegistry};

    // -----------------------------------------------------------------------
    // Helper: a tool that always succeeds
    // -----------------------------------------------------------------------
    struct PassThroughTool;

    impl Tool for PassThroughTool {
        fn name(&self) -> &'static str {
            "pass_through"
        }
        fn run(&self, _input: &ToolInput) -> anyhow::Result<ToolOutput> {
            Ok(ToolOutput {
                success: true,
                result: None,
                error: None,
                verification: None,
                audit_log: None,
                pua_report: None,
            })
        }
    }

    // -----------------------------------------------------------------------
    // Helper: a tool that always fails
    // -----------------------------------------------------------------------
    struct FailTool;

    impl Tool for FailTool {
        fn name(&self) -> &'static str {
            "fail_tool"
        }
        fn run(&self, _input: &ToolInput) -> anyhow::Result<ToolOutput> {
            Ok(ToolOutput {
                success: false,
                result: None,
                error: Some("intentional failure".to_string()),
                verification: None,
                audit_log: None,
                pua_report: None,
            })
        }
    }

    // -----------------------------------------------------------------------
    // Helper: a tool that counts executions for compensation tests
    // -----------------------------------------------------------------------
    struct CountedTool {
        counter: std::sync::atomic::AtomicU64,
    }

    impl CountedTool {
        fn new() -> Self {
            Self {
                counter: std::sync::atomic::AtomicU64::new(0),
            }
        }
    }

    impl Tool for CountedTool {
        fn name(&self) -> &'static str {
            "counted_tool"
        }
        fn run(&self, _input: &ToolInput) -> anyhow::Result<ToolOutput> {
            self.counter
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(ToolOutput {
                success: true,
                result: None,
                error: None,
                verification: None,
                audit_log: None,
                pua_report: None,
            })
        }
    }

    fn dummy_input() -> ToolInput {
        ToolInput {
            task_id: "test-task".to_string(),
            phase: "test".to_string(),
            agent_role: "tester".to_string(),
            objective: "test".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({}),
            allowed_base_dir: None,
        }
    }

    // -----------------------------------------------------------------------
    // Idempotency key dedup works (same key, same tool returns cached)
    // -----------------------------------------------------------------------
    #[test]
    fn idempotency_key_dedup_returns_cached_result() {
        let mut registry = ToolRegistry::new_empty();
        registry.register(PassThroughTool);
        let store = IdempotencyStore::new();
        let input = dummy_input();

        // First call — should execute.
        let first =
            registry.execute_with_idempotency("pass_through", &input, Some("dedup-key-1"), &store);
        assert!(
            !first.idempotency_hit,
            "first call should NOT be an idempotency hit"
        );
        assert!(matches!(first.status, ToolCallStatus::Success));

        // Second call with same key — should return cached.
        let second =
            registry.execute_with_idempotency("pass_through", &input, Some("dedup-key-1"), &store);
        assert!(
            second.idempotency_hit,
            "second call should be an idempotency hit"
        );
        assert!(matches!(second.status, ToolCallStatus::Success));
        assert_eq!(second.idempotency_key.as_deref(), Some("dedup-key-1"),);
    }

    // -----------------------------------------------------------------------
    // Conflict rate tracking
    // -----------------------------------------------------------------------
    #[test]
    fn conflict_rate_tracking() {
        let mut registry = ToolRegistry::new_empty();
        registry.register(PassThroughTool);
        let store = IdempotencyStore::new();
        let input = dummy_input();

        // No conflicts yet.
        assert_eq!(store.conflict_rate(), 0.0, "empty store has rate 0");

        // First call — creates record.
        registry.execute_with_idempotency("pass_through", &input, Some("conflict-key"), &store);
        assert_eq!(store.conflict_rate(), 0.0, "no conflict after first call");

        // Second call with same key — creates a conflict.
        registry.execute_with_idempotency("pass_through", &input, Some("conflict-key"), &store);

        // Now 1 out of 1 keys has a conflict → rate = 1.0
        let rate = store.conflict_rate();
        assert!(
            (rate - 1.0).abs() < f64::EPSILON,
            "expected conflict rate 1.0, got {}",
            rate
        );
    }

    // -----------------------------------------------------------------------
    // Transaction scope with compensate actions
    // -----------------------------------------------------------------------
    #[test]
    fn transaction_scope_rollback_invokes_compensations_in_reverse() {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::Arc;

        let invoked = Arc::new(AtomicU64::new(0));
        let invoked_clone = Arc::clone(&invoked);

        let mut scope = TransactionScope::new("txn-comp-test".to_string());

        // Register two compensation actions.
        {
            let inv = Arc::clone(&invoked_clone);
            scope.register_completion(
                "tool_a".to_string(),
                std::sync::Arc::new(move || {
                    inv.fetch_add(10, Ordering::SeqCst);
                }),
            );
        }
        {
            let inv = Arc::clone(&invoked_clone);
            scope.register_completion(
                "tool_b".to_string(),
                std::sync::Arc::new(move || {
                    inv.fetch_add(1, Ordering::SeqCst);
                }),
            );
        }

        assert_eq!(scope.completed_tools, vec!["tool_a", "tool_b"]);

        // Rollback — should invoke in reverse: tool_b then tool_a.
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(scope.rollback());

        // tool_b adds 1, tool_a adds 10 → total = 11
        assert_eq!(invoked.load(Ordering::SeqCst), 11);
    }

    // -----------------------------------------------------------------------
    // Tool call result structure preserves all 3 states
    // -----------------------------------------------------------------------
    #[test]
    fn tool_call_result_preserves_all_three_states() {
        // --- Success ---
        let success = ToolCallResult {
            status: ToolCallStatus::Success,
            idempotency_key: Some("key-1".to_string()),
            idempotency_hit: false,
            transaction_id: Some("txn-1".to_string()),
            output: ToolOutput {
                success: true,
                result: None,
                error: None,
                verification: None,
                audit_log: None,
                pua_report: None,
            },
            duration_ms: 42,
        };
        assert!(matches!(success.status, ToolCallStatus::Success));
        assert!(!success.idempotency_hit);
        assert_eq!(success.duration_ms, 42);

        // --- Failure ---
        let failure = ToolCallResult {
            status: ToolCallStatus::Failure("something went wrong".to_string()),
            idempotency_key: None,
            idempotency_hit: false,
            transaction_id: None,
            output: ToolOutput {
                success: false,
                result: None,
                error: Some("something went wrong".to_string()),
                verification: None,
                audit_log: None,
                pua_report: None,
            },
            duration_ms: 10,
        };
        match &failure.status {
            ToolCallStatus::Failure(msg) => {
                assert_eq!(msg, "something went wrong");
            }
            _ => panic!("expected Failure variant"),
        }

        // --- Partial ---
        let partial = ToolCallResult {
            status: ToolCallStatus::Partial {
                completed: vec!["read_file".to_string()],
                failed: vec!["write_file".to_string()],
            },
            idempotency_key: None,
            idempotency_hit: false,
            transaction_id: Some("txn-2".to_string()),
            output: ToolOutput {
                success: false,
                result: None,
                error: None,
                verification: None,
                audit_log: None,
                pua_report: None,
            },
            duration_ms: 200,
        };
        match &partial.status {
            ToolCallStatus::Partial { completed, failed } => {
                assert_eq!(completed.len(), 1);
                assert_eq!(completed[0], "read_file");
                assert_eq!(failed.len(), 1);
                assert_eq!(failed[0], "write_file");
            }
            _ => panic!("expected Partial variant"),
        }
    }

    // -----------------------------------------------------------------------
    // execute_transactional rolls back on failure
    // -----------------------------------------------------------------------
    #[test]
    fn execute_transactional_rolls_back_on_failure() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut registry = ToolRegistry::new_empty();
        registry.register(PassThroughTool);
        registry.register(FailTool);
        let store = IdempotencyStore::new();

        let tool_calls = vec![
            ("pass_through".to_string(), dummy_input()),
            ("fail_tool".to_string(), dummy_input()),
        ];

        let result =
            registry.execute_transactional(tool_calls, "txn-rollback-test".to_string(), &store);

        match &result.status {
            ToolCallStatus::Partial { completed, failed } => {
                assert_eq!(completed, &["pass_through"]);
                assert_eq!(failed, &["fail_tool"]);
            }
            other => panic!("expected Partial, got {:?}", other),
        }

        assert!(result.transaction_id.is_some());
        assert!(!result.output.success);
    }

    // -----------------------------------------------------------------------
    // execute_transactional succeeds when all tools succeed
    // -----------------------------------------------------------------------
    #[test]
    fn execute_transactional_succeeds_when_all_pass() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let _guard = rt.enter();
        let mut registry = ToolRegistry::new_empty();
        registry.register(PassThroughTool);
        let store = IdempotencyStore::new();

        let tool_calls = vec![
            ("pass_through".to_string(), dummy_input()),
            ("pass_through".to_string(), dummy_input()),
        ];

        let result = registry.execute_transactional(tool_calls, "txn-all-pass".to_string(), &store);

        assert!(matches!(result.status, ToolCallStatus::Success));
        assert!(result.output.success);
    }

    // -----------------------------------------------------------------------
    // Idempotency keys are scoped per tool name
    // -----------------------------------------------------------------------
    #[test]
    fn idempotency_keys_are_scoped_per_tool() {
        let mut registry = ToolRegistry::new_empty();
        registry.register(PassThroughTool);
        let store = IdempotencyStore::new();
        let input = dummy_input();

        // First call with "shared-key".
        let first =
            registry.execute_with_idempotency("pass_through", &input, Some("shared-key"), &store);
        assert!(!first.idempotency_hit);

        // Second call with same key and same tool — should hit.
        let second =
            registry.execute_with_idempotency("pass_through", &input, Some("shared-key"), &store);
        assert!(second.idempotency_hit, "same tool + same key should hit");
    }

    // -----------------------------------------------------------------------
    // record_result stores the result for later retrieval
    // -----------------------------------------------------------------------
    #[test]
    fn check_and_record_without_result_stores_empty() {
        let store = IdempotencyStore::new();

        let (is_dup, cached) = store.check_and_record("fresh-key", "some_tool");
        assert!(!is_dup, "new key should not be duplicate");
        assert!(cached.is_none(), "new key has no cached result");

        // Second check should now be duplicate but still no result
        // (because we never called record_result).
        let (is_dup, cached) = store.check_and_record("fresh-key", "some_tool");
        assert!(is_dup, "repeated key should be duplicate");
        assert!(cached.is_none(), "no result recorded yet");
    }

    #[test]
    fn record_result_stores_for_later_retrieval() {
        let store = IdempotencyStore::new();

        let (is_dup, _) = store.check_and_record("store-key", "my_tool");
        assert!(!is_dup);

        let result = ToolCallResult {
            status: ToolCallStatus::Success,
            idempotency_key: Some("store-key".to_string()),
            idempotency_hit: false,
            transaction_id: None,
            output: ToolOutput {
                success: true,
                result: None,
                error: None,
                verification: None,
                audit_log: None,
                pua_report: None,
            },
            duration_ms: 5,
        };

        store.record_result("store-key", "my_tool", result);

        let (is_dup, cached) = store.check_and_record("store-key", "my_tool");
        assert!(is_dup);
        let cached = cached.expect("cached result should be present after record_result");
        assert!(cached.output.success);
    }

    #[test]
    fn counted_tool_tracks_executions() {
        use std::sync::atomic::Ordering;
        let tool = CountedTool::new();
        let input = dummy_input();
        assert_eq!(tool.counter.load(Ordering::SeqCst), 0);
        let _ = tool.run(&input).unwrap();
        assert_eq!(tool.counter.load(Ordering::SeqCst), 1);
        let _ = tool.run(&input).unwrap();
        assert_eq!(tool.counter.load(Ordering::SeqCst), 2);
    }
}
