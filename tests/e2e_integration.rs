/// E2E Integration Test: Full System Flow
///
/// Validates the end-to-end integration of CapabilityBus, HarnessBus, and all 21 F-GAP modules.
/// Each test validates a specific cross-module flow, not individual components.
///
/// These tests use the same RpcHarness pattern as acp_runtime_rpc_integration.
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

pub mod common;
use common::binary_path;
use common::suite_mutex;
use common::CrossProcessLock;

const LOCK_NAME: &str = "e2e-integration";

struct E2eHarness {
    child: Child,
    // Option so we can explicitly drop (close) stdin before wait_for_exit to
    // prevent the write-side pipe race: the child blocks on its stdin reader
    // until EOF, which only arrives once the write end is closed.
    stdin: Option<ChildStdin>,
    stdout_rx: Receiver<Value>,
    stderr_lines: Arc<Mutex<Vec<String>>>,
    // Serialize this integration suite to avoid flaky child-process pipe races.
    _suite_guard: MutexGuard<'static, ()>,
    // Cross-process file lock that serialises go-on child-process creation
    // across *all* test binaries (crates in tests/), preventing CPU contention
    // that would otherwise cause artificial timeouts in timing-sensitive tests.
    _cross_process_lock: CrossProcessLock,
}

/// Convenience wrapper around the shared suite mutex.
fn suite_guard() -> &'static Mutex<()> {
    suite_mutex()
}

impl E2eHarness {
    fn spawn() -> Self {
        // Minimal test config with synthetic providers (no network deps).
        let test_config = r#"
default_phase = "coding"

[flow]
name = "E2E Test Flow"
phases = ["coding"]

[runtime]
maintenance_interval_seconds = 60
health_interval_seconds = 120
shutdown_drain_seconds = 5
governance_enabled = true

[phases.coding]
description = "Coding phase for e2e tests"
agents = []
fallback = true
"#;
        Self::spawn_with_config(test_config)
    }

    /// Spawn the go-on binary with a caller-provided config file. Used by
    /// tests that need a specific agent/phase layout (e.g. workflow.execute).
    fn spawn_with_config(test_config: &str) -> Self {
        let _suite_guard = match suite_guard().lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        // Acquire the cross-process lock BEFORE spawning the child.
        // This guarantees that only one go-on process runs at a time across
        // all test binaries, eliminating CPU-starvation-induced timeouts.
        let _cross_process_lock = CrossProcessLock::new(LOCK_NAME, 60);

        // Determine project root by walking up from the binary path until
        // we find the Cargo.toml that belongs to this workspace.
        let mut project_root = binary_path();
        project_root.pop();
        loop {
            if project_root.join("Cargo.toml").exists() {
                break;
            }
            if !project_root.pop() {
                project_root = std::env::current_dir().unwrap_or_default();
                break;
            }
        }

        // Write the config to a temp dir (no network deps needed).
        let tmp_dir = std::env::temp_dir().join(format!("go-on-e2e-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&tmp_dir);
        let config_path = tmp_dir.join("config.toml");
        std::fs::write(&config_path, test_config).expect("failed to write e2e test config");

        let mut child = Command::new(binary_path())
            .current_dir(&project_root)
            .arg("--config")
            .arg(config_path.to_str().expect("config path is valid UTF-8"))
            .arg("--protocol-mode")
            .arg("acp_stdio")
            .env("GO_ON_ENABLE_LOCAL_TEST_AGENTS", "1")
            .env("GO_ON_LOG", "error")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn go-on");

        let stdin = child.stdin.take().expect("failed to capture child stdin");
        let stdout = child.stdout.take().expect("failed to capture child stdout");
        let stderr = child.stderr.take().expect("failed to capture child stderr");
        let stderr_lines = Arc::new(Mutex::new(Vec::new()));

        let (stdout_tx, stdout_rx) = mpsc::channel();

        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else {
                    break;
                };
                if line.trim().is_empty() {
                    continue;
                }
                if let Ok(value) = serde_json::from_str::<Value>(&line) {
                    let _ = stdout_tx.send(value);
                }
            }
        });

        // Drain stderr so a verbose process cannot block on a full stderr buffer.
        let stderr_lines_clone = Arc::clone(&stderr_lines);
        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                let Ok(line) = line else {
                    break;
                };
                if let Ok(mut guard) = stderr_lines_clone.lock() {
                    guard.push(line);
                    if guard.len() > 200 {
                        let overflow = guard.len() - 200;
                        guard.drain(0..overflow);
                    }
                }
            }
        });

        Self {
            child,
            stdin: Some(stdin),
            stdout_rx,
            stderr_lines,
            _suite_guard,
            _cross_process_lock,
        }
    }

    fn request(&mut self, id: u64, method: &str, params: Option<Value>) -> Value {
        let mut payload = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
        });

        // Inject admin role into params so RBAC doesn't reject requests.
        // The test harness needs full access to all methods.
        let merged_params = match params {
            Some(mut p) => {
                if !p.as_object().is_some_and(|o| o.contains_key("roles")) {
                    p["roles"] = json!(["admin"]);
                    p["user_id"] = json!("test-admin");
                }
                p
            }
            None => json!({
                "roles": ["admin"],
                "user_id": "test-admin"
            }),
        };
        payload["params"] = merged_params;

        let body = serde_json::to_string(&payload).expect("failed to encode request");
        let stdin = self.stdin.as_mut().expect("stdin already closed");
        writeln!(stdin, "{body}").expect("failed to write request to stdin");
        stdin.flush().expect("failed to flush request");

        self.read_response_for_id(id, Duration::from_secs(10))
    }

    /// Close the write end of the child's stdin pipe so the child sees EOF.
    /// Must be called after sending the final request (e.g. "shutdown") and
    /// before `wait_for_exit` to avoid a pipe-write race that causes flaky hangs.
    fn close_stdin(&mut self) {
        drop(self.stdin.take());
    }

    fn read_response_for_id(&mut self, id: u64, timeout: Duration) -> Value {
        let deadline = Instant::now() + timeout;
        loop {
            let now = Instant::now();
            if now >= deadline {
                panic!("timed out waiting for response id {id}");
            }
            let remaining = deadline.saturating_duration_since(now);
            let msg = match self.stdout_rx.recv_timeout(remaining) {
                Ok(msg) => msg,
                Err(err) => {
                    let status = self.child.try_wait().ok().flatten();
                    let stderr_tail = self.stderr_tail(30);
                    panic!(
                        "stdout closed while waiting for response id {id}: {err}; child status: {:?}; stderr tail:\n{}",
                        status,
                        stderr_tail
                    );
                }
            };
            if msg.get("id") == Some(&json!(id)) {
                return msg;
            }
        }
    }

    fn wait_for_exit(&mut self, timeout: Duration) {
        // Close the write end of stdin so the child sees EOF and can exit cleanly.
        // Without this, the child's stdin-reader blocks and the process never terminates,
        // causing a timing race in the multi-process pipe harness.
        self.close_stdin();
        let deadline = Instant::now() + timeout;
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    assert!(status.success(), "child exited with status: {status}");
                    return;
                }
                Ok(None) => {
                    if Instant::now() >= deadline {
                        panic!("timed out waiting for child exit");
                    }
                    thread::sleep(Duration::from_millis(20));
                }
                Err(err) => panic!(
                    "failed to wait for child: {err}; stderr tail:\n{}",
                    self.stderr_tail(30)
                ),
            }
        }
    }

    fn stderr_tail(&self, limit: usize) -> String {
        if let Ok(guard) = self.stderr_lines.lock() {
            let start = guard.len().saturating_sub(limit);
            let lines = guard[start..].to_vec();
            if lines.is_empty() {
                "<empty>".to_string()
            } else {
                lines.join("\n")
            }
        } else {
            "<stderr lock poisoned>".to_string()
        }
    }
}

impl Drop for E2eHarness {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

// ── E2E Tests: CapabilityBus ↔ HarnessBus Integration ────────────────────

#[cfg(test)]
mod e2e_tests {
    use super::*;

    /// E2E-01: Full health check returns all required system profiles
    #[test]
    fn e2e_health_check_returns_full_profiles() {
        let mut harness = E2eHarness::spawn();
        let resp = harness.request(1, "health", None);

        let result = resp.get("result").expect("health should return result");

        // Health response uses lifecycle.is_healthy instead of a top-level status field
        if let Some(lifecycle) = result.get("lifecycle") {
            assert!(
                lifecycle.get("is_healthy").is_some(),
                "health.lifecycle should have is_healthy"
            );
        } else {
            panic!("health response should have lifecycle");
        }

        harness.wait_for_exit(Duration::from_secs(5));
    }

    /// E2E-02b: governance.audit.verify round-trips the hash-chain endpoint
    #[test]
    fn e2e_governance_audit_verify_returns_chain_shape() {
        let mut harness = E2eHarness::spawn();
        let resp = harness.request(1, "governance.audit.verify", None);

        let result = resp
            .get("result")
            .expect("governance.audit.verify should return result");
        assert_eq!(
            result.get("ok").and_then(Value::as_bool),
            Some(true),
            "verify should succeed"
        );
        // The chain file may not exist yet on a fresh machine — entry_count
        // must still be a number and the integrity fields must be present.
        assert!(
            result.get("entry_count").is_some(),
            "verify should report entry_count"
        );
        assert!(
            result.get("is_chain_intact").is_some(),
            "verify should report is_chain_intact"
        );
        assert!(
            result.get("violations").is_some(),
            "verify should report violations list"
        );

        harness.wait_for_exit(Duration::from_secs(5));
    }

    /// E2E-02: Governance status reports all bus metrics
    #[test]
    fn e2e_governance_status_returns_bus_metrics() {
        let mut harness = E2eHarness::spawn();
        let resp = harness.request(1, "governance.status", None);

        let result = resp
            .get("result")
            .expect("governance.status should return result");

        // Should contain bus-level metrics (hard assertions — previously the
        // `if let Some(...)` guards let a missing key pass silently).
        assert!(
            result.get("harness_bus").is_some(),
            "governance.status must report harness_bus metrics"
        );
        assert!(
            result.get("capability_bus").is_some(),
            "governance.status must report capability_bus metrics"
        );
        assert!(
            result.get("harness_bus").unwrap().is_object(),
            "harness_bus should be an object"
        );
        assert!(
            result.get("capability_bus").unwrap().is_object(),
            "capability_bus should be an object"
        );

        harness.wait_for_exit(Duration::from_secs(5));
    }

    /// E2E-03: Initialize returns protocol and version info
    #[test]
    fn e2e_initialize_returns_protocol_info() {
        let mut harness = E2eHarness::spawn();
        let resp = harness.request(
            1,
            "initialize",
            Some(json!({
                "protocol": "acp",
                "version": "1.0"
            })),
        );

        let result = resp.get("result");
        assert!(result.is_some(), "initialize should return result");
        if let Some(r) = result {
            assert!(
                r.get("protocol").is_some()
                    || r.get("serverInfo").is_some()
                    || r.get("version").is_some(),
                "initialize result should contain protocol/server info"
            );
        }

        harness.wait_for_exit(Duration::from_secs(5));
    }

    /// E2E-04: workflow.execute returns the real repair contract
    /// (repair_readiness + repair_history) with the backend's actual shapes.
    ///
    /// Sends a real `workflow.execute` RPC against a spawned go-on process
    /// (local_echo agents, no LLM/network required) and validates the
    /// contract fields the backend actually produces. `task.execute` does not
    /// carry the repair contract, so this test covers the workflow.execute
    /// endpoint only.
    #[test]
    fn e2e_workflow_execute_returns_repair_contract() {
        let workflow_config = r#"
default_phase = "coding"

[flow]
name = "E2E Workflow Flow"
phases = ["coding"]

[runtime]
maintenance_interval_seconds = 60
health_interval_seconds = 120
shutdown_drain_seconds = 5
governance_enabled = true

[agents.local_echo]
type = "local_echo"

[phases.coding]
description = "Coding"
agents = ["local_echo"]
fallback = true

[phases.coding.options.extra]
review_min_level = "standard"
review_required_reviews = 1
review_timeout_policy = "reject"
review_required_checks = []
"#;
        let mut harness = E2eHarness::spawn_with_config(workflow_config);
        let resp = harness.request(
            1,
            "workflow.execute",
            Some(json!({
                "task": "Write a hello-world Rust CLI subcommand",
                "requirement_confirmed": true,
                "auto_gates": false,
            })),
        );

        // The backend must answer with a result object, not an error.
        let result = resp
            .get("result")
            .expect("workflow.execute should return a result object");
        assert!(
            resp.get("error").is_none() || resp.get("error").map(Value::is_null).unwrap_or(true),
            "workflow.execute should not error, got: {resp}"
        );

        // ── repair_readiness contract (real backend shape) ────────────────
        let readiness = result
            .get("repair_readiness")
            .expect("workflow.execute result must carry repair_readiness");
        assert!(
            readiness["eligible"].is_boolean(),
            "repair_readiness.eligible must be a boolean"
        );
        assert!(
            readiness["max_iterations"].is_u64(),
            "repair_readiness.max_iterations must be an integer"
        );
        let mode = readiness["governance_mode"]
            .as_str()
            .expect("repair_readiness.governance_mode must be a string");
        assert!(
            !mode.is_empty(),
            "repair_readiness.governance_mode must not be empty"
        );
        assert!(
            readiness["reason"].is_string(),
            "repair_readiness.reason must be a string"
        );

        // ── repair_history contract (real backend shape) ─────────────────
        // With local_echo agents all subtasks succeed, so the backend returns
        // the empty form `{ "actions": [] }`; the actions array must still be
        // present and every action must carry the documented fields.
        let history = result
            .get("repair_history")
            .expect("workflow.execute result must carry repair_history");
        let actions = history["actions"]
            .as_array()
            .expect("repair_history.actions must be an array");
        for action in actions {
            assert!(action["iteration"].is_u64());
            assert!(action["type"].is_string());
            assert!(
                action["subtask_id"].is_string(),
                "repair action must carry subtask_id"
            );
            assert!(
                action["result"].is_string(),
                "repair action must carry a result string"
            );
        }
        // Iteration bookkeeping appears when a repair loop actually ran;
        // assert on the fields the full form documents without hard-coding
        // the no-repair case.
        if let Some(iteration) = history.get("iteration") {
            assert!(iteration.is_u64(), "repair_history.iteration must be u64");
        }

        harness.wait_for_exit(Duration::from_secs(5));
    }

    /// E2E-05: Capability listing returns structured result
    #[test]
    fn e2e_capability_listing() {
        let mut harness = E2eHarness::spawn();
        let resp = harness.request(1, "capabilities.list", Some(json!({})));

        let result = resp.get("result");
        assert!(result.is_some(), "capabilities.list should return result");

        harness.wait_for_exit(Duration::from_secs(5));
    }

    /// E2E-06: Chat completion request flows through governance
    #[test]
    fn e2e_chat_completion_governance_flow() {
        let mut harness = E2eHarness::spawn();
        let resp = harness.request(
            1,
            "chat.completions",
            Some(json!({
                "model": "test-model",
                "messages": [{"role": "user", "content": "test"}],
                "max_tokens": 10
            })),
        );

        // Must return exactly one of result/error (previously the `has_result
        // || has_error` OR was tautologically true for any response).
        let has_result = resp.get("result").is_some();
        let has_error = resp.get("error").is_some();
        assert!(
            has_result != has_error,
            "chat.completions must return exactly one of result/error"
        );

        if let Some(error) = resp.get("error") {
            // If error, should be structured (not just a crash)
            assert!(
                error.get("code").is_some() && error.get("message").is_some(),
                "error should have code and message"
            );
        }

        harness.wait_for_exit(Duration::from_secs(5));
    }

    /// E2E-07: Multi-step lifecycle across protocols
    #[test]
    fn e2e_multi_step_lifecycle() {
        let mut harness = E2eHarness::spawn();

        // Step 1: Initialize
        let init = harness.request(
            1,
            "initialize",
            Some(json!({
                "protocol": "acp",
                "version": "1.0"
            })),
        );
        assert!(
            init.get("result").is_some() || init.get("error").is_some(),
            "initialize should respond"
        );

        // Step 2: Health check
        let health = harness.request(2, "health", None);
        assert!(health.get("result").is_some(), "health should respond");

        // Step 3: Governance status
        let gov = harness.request(3, "governance.status", None);
        assert!(
            gov.get("result").is_some(),
            "governance.status should respond"
        );

        // Step 4: Capabilities
        let caps = harness.request(4, "capabilities.list", Some(json!({})));
        assert!(
            caps.get("result").is_some(),
            "capabilities.list should respond"
        );

        harness.wait_for_exit(Duration::from_secs(5));
    }
}
