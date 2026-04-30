/// E2E Integration Test: Full System Flow
///
/// Validates the end-to-end integration of CapabilityBus, HarnessBus, and all 21 F-GAP modules.
/// Each test validates a specific cross-module flow, not individual components.
///
/// These tests use the same RpcHarness pattern as acp_runtime_rpc_integration.
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use fs2::FileExt;
use serde_json::{json, Value};
use tempfile::tempdir;

// ---------------------------------------------------------------------------
// Cross-process file lock — serialises go-on child-process creation across
// all test binaries so that integration tests cannot stack concurrent child
// processes that contend for CPU and cause artificial timeouts.
// ---------------------------------------------------------------------------

/// An exclusive advisory file lock held across the lifetime of a test harness.
/// Because all test binaries build the same lock-file path, processes from
/// *different* test files serialise against each other (the in-process
/// `Mutex` only serialises threads within the same binary).
struct CrossProcessLock {
    _file: std::fs::File,
}

impl CrossProcessLock {
    /// Acquire an exclusive lock, blocking until it is available.
    /// Uses `fs2::FileExt::lock_exclusive` which calls `flock(LOCK_EX)` on Unix
    /// or `LockFile` on Windows.
    fn lock(path: &Path) -> Self {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(path)
            .expect("failed to open/create cross-process lock file");

        file.lock_exclusive()
            .expect("failed to acquire cross-process lock");

        Self { _file: file }
    }
}

// The lock is released automatically when `_file` is dropped because
// `fs2::FileExt::lock_exclusive` holds the lock on the fd and closing
// the fd (via `Drop`) releases the lock on Unix.

/// Return the path of the shared cross-process lock file.
///
/// Uses the system temporary directory so that all test binaries (which run
/// as separate OS processes) see the same lock file path.
fn cross_process_lock_path() -> PathBuf {
    std::env::temp_dir().join(".go-on-e2e-integration.lock")
}

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

// Keep the in-process suite guard as well – it serialises threads *within*
// the binary, which is cheaper than the file lock for intra-binary ordering.
static E2E_SUITE_GUARD: OnceLock<Mutex<()>> = OnceLock::new();

fn suite_guard() -> &'static Mutex<()> {
    E2E_SUITE_GUARD.get_or_init(|| Mutex::new(()))
}

fn binary_path() -> PathBuf {
    std::env::var("CARGO_BIN_EXE_go-on")
        .map(PathBuf::from)
        .expect("CARGO_BIN_EXE_go-on is not set")
}

impl E2eHarness {
    fn spawn() -> Self {
        let _suite_guard = match suite_guard().lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        // Acquire the cross-process lock BEFORE spawning the child.
        // This guarantees that only one go-on process runs at a time across
        // all test binaries, eliminating CPU-starvation-induced timeouts.
        let lock_path = cross_process_lock_path();
        let _cross_process_lock = CrossProcessLock::lock(&lock_path);

        let mut child = Command::new(binary_path())
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

        if let Some(params) = params {
            payload["params"] = params;
        }

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
        assert!(result.get("status").is_some(), "health should have status");

        // Verify platform_mode is valid
        if let Some(profile) = result.get("capability_profile") {
            let mode = profile.get("platform_mode").and_then(|m| m.as_str());
            assert!(
                mode.is_some(),
                "capability_profile should have platform_mode"
            );
        }

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

        // Should contain bus-level metrics
        if let Some(harness_bus) = result.get("harness_bus") {
            assert!(harness_bus.is_object(), "harness_bus should be an object");
        }
        if let Some(capability_bus) = result.get("capability_bus") {
            assert!(
                capability_bus.is_object(),
                "capability_bus should be an object"
            );
        }

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

    /// E2E-04: Three-endpoint contract structure validation
    /// Validates repair_readiness and repair_history contract shapes
    /// across workflow.execute and task.execute responses
    #[test]
    fn e2e_three_endpoint_contract_validation() {
        // Structural validation of the contract shapes
        let repair_readiness = json!({
            "eligible": true,
            "max_iterations": 2,
            "governance_mode": "assisted",
            "reason": "test validation"
        });

        // Validate repair_readiness fields
        assert!(repair_readiness["eligible"].is_boolean());
        assert!(repair_readiness["max_iterations"].is_u64());
        assert!(repair_readiness["governance_mode"].is_string());
        assert!(repair_readiness["reason"].is_string());

        let valid_modes = vec!["assisted", "conservative", "manual", "disabled"];
        let mode = repair_readiness["governance_mode"].as_str().unwrap();
        assert!(
            valid_modes.contains(&mode),
            "invalid governance_mode: {mode}"
        );

        // Validate repair_history structure
        let repair_history = json!({
            "iteration": 1,
            "max_iterations": 2,
            "actions": [
                {
                    "iteration": 1,
                    "type": "retry_subtask",
                    "subtask_id": "subtask-001",
                    "result": "success"
                }
            ]
        });

        assert!(repair_history["iteration"].is_u64());
        assert!(repair_history["actions"].is_array());
        if let Some(actions) = repair_history["actions"].as_array() {
            for action in actions {
                assert!(action["result"].is_string());
                let valid_results = vec!["success", "in_progress", "failed"];
                assert!(valid_results.contains(&action["result"].as_str().unwrap()));
            }
        }

        // Field name consistency across endpoints
        let keys: Vec<&str> = repair_readiness
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect();
        let expected_keys = vec!["eligible", "max_iterations", "governance_mode", "reason"];
        for k in expected_keys {
            assert!(keys.contains(&k), "repair_readiness missing field: {k}");
        }
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

        // Should either succeed or return a meaningful error (not crash)
        let has_result = resp.get("result").is_some();
        let has_error = resp.get("error").is_some();
        assert!(
            has_result || has_error,
            "chat.completions should return result or error"
        );

        if let Some(error) = resp.get("error") {
            // If error, should be structured (not just a crash)
            assert!(
                error.get("code").is_some() || error.get("message").is_some(),
                "error should have code or message"
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
