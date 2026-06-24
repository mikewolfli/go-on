//! Transaction types for tool call transactional semantics (BLUE43 Step 15)
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
use std::time::SystemTime;

use tracing::warn;

use crate::orchestration::tool::ToolOutput;

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
    let mut guard = LATEST_IDEMPOTENCY_CONFLICT_RATE
        .lock()
        .unwrap_or_else(|poisoned| {
            warn!("LATEST_IDEMPOTENCY_CONFLICT_RATE lock poisoned – recovered");
            poisoned.into_inner()
        });
    *guard = Some(rate);
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
        let mut keys = self.keys.lock().unwrap_or_else(|poisoned| {
            warn!("IdempotencyStore lock poisoned – recovered data");
            poisoned.into_inner()
        });

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
        let mut keys = self.keys.lock().unwrap_or_else(|poisoned| {
            warn!("IdempotencyStore lock poisoned – recovered data");
            poisoned.into_inner()
        });

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
        let keys = self.keys.lock().unwrap_or_else(|poisoned| {
            warn!("IdempotencyStore lock poisoned – recovered data");
            poisoned.into_inner()
        });
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
pub(super) enum CompensateResult {
    Ok,
    Timeout,
}

/// Execute a compensation closure inside `spawn_blocking` with a timeout.
pub(super) async fn execute_compensate_with_timeout(
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
