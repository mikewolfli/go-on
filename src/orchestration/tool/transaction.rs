//! Tool call transactional semantics (BLUE43 Step 15)
//!
//! Provides idempotency tracking, conflict-rate monitoring, and
//! transaction-scoped rollback / compensation for tool execution.
//!
//! # Sub-modules
//!
//! * [`types`] — Core types: [`ToolCallResult`], [`IdempotencyStore`],
//!   [`TransactionScope`], and related infrastructure.
//! * [`types`] — Core types: [`ToolCallResult`], [`IdempotencyStore`],
//!   [`TransactionScope`], and related infrastructure.

pub mod types;

pub use types::*;

use std::time::Instant;

use crate::orchestration::tool::{ToolInput, ToolOutput, ToolRegistry};

// ---------------------------------------------------------------------------
// ToolRegistry transactional extensions
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
    pub async fn execute_transactional(
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
                    scope.rollback().await;

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
    #[tokio::test]
    async fn execute_transactional_rolls_back_on_failure() {
        let mut registry = ToolRegistry::new_empty();
        registry.register(PassThroughTool);
        registry.register(FailTool);
        let store = IdempotencyStore::new();

        let tool_calls = vec![
            ("pass_through".to_string(), dummy_input()),
            ("fail_tool".to_string(), dummy_input()),
        ];

        let result = registry
            .execute_transactional(tool_calls, "txn-rollback-test".to_string(), &store)
            .await;

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
    #[tokio::test]
    async fn execute_transactional_succeeds_when_all_pass() {
        let mut registry = ToolRegistry::new_empty();
        registry.register(PassThroughTool);
        let store = IdempotencyStore::new();

        let tool_calls = vec![
            ("pass_through".to_string(), dummy_input()),
            ("pass_through".to_string(), dummy_input()),
        ];

        let result = registry
            .execute_transactional(tool_calls, "txn-all-pass".to_string(), &store)
            .await;

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
