//! GAP-B52-37: Self-Evolution End-to-End
//!
//! Validates the self-evolution lifecycle:
//!   trigger → analyze → propose → approve → sandbox → compile → commit → rollback
//!
//! Uses in-memory mock trigger sources and sandbox executors so this test
//! can run without external infrastructure. Real integration would connect
//! to a live go-on server with git and build-toolchain access.
//!
//! Coverage notes:
//!   - The error path (no trigger sources) and the data-structure/constructor
//!     contracts are asserted directly.
//!   - `test_self_evolution_loop_polls_trigger_sources` proves the loop
//!     machinery genuinely starts and polls registered sources.
//!   - The full apply/commit/rollback stages require a real LLM agent (patch
//!     generation), a compile-capable sandbox, and git — not available in
//!     this sandbox, so those stages are covered by unit tests in the crate.
//!
//! # integration-test

use std::path::PathBuf;
use std::time::Duration;

use go_on::orchestration::self_evolution::evolution_loop::{
    Analysis, Approval, ApprovalMode, EvolutionLoop, EvolutionTrigger, RegressionDirection,
};
use go_on::orchestration::self_evolution::sandbox::{BuildResult, SandboxExecutor};

// ── Helpers ────────────────────────────────────────────────────────────────

struct EvolutionE2eContext {
    workdir: PathBuf,
}

impl EvolutionE2eContext {
    fn new(session_id: &str) -> Self {
        let workdir = std::env::temp_dir().join(format!("go-on-e2e-evol-{}", session_id));
        let _ = std::fs::create_dir_all(&workdir);
        Self { workdir }
    }
}

impl Drop for EvolutionE2eContext {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.workdir);
    }
}

/// Create a simple Analysis from a trigger for testing purposes.
fn make_analysis(trigger: EvolutionTrigger) -> Analysis {
    Analysis::new(
        trigger,
        "latency_spike: downstream service timeout".into(),
        "add connection pooling to HTTP client".into(),
        vec!["src/http_client.rs".into()],
        "medium".into(),
        0.85,
    )
}

// ── Tests ──────────────────────────────────────────────────────────────────

/// Validates the ERROR path of the self-evolution lifecycle when no trigger
/// sources are configured. This is NOT a full end-to-end test of the entire
/// trigger→analyze→propose→approval→sandbox→compile→submit→rollback flow.
///
/// To run a genuine full-lifecycle test, you would need:
///   - A live EvolutionLoop with a registered trigger source (e.g. polling interval)
///   - A real or mocked sandbox that compiles and returns BuildResult::Success
///   - Git integration for commit / rollback
///
/// For now this test validates:
///   1. run() errors with "no trigger sources" when poll interval is set but no source is added
///   2. All EvolutionTrigger variants can be constructed and have non-empty labels/descriptions
///   3. Analysis, Approval, CodePatch, and BuildResult data structures are correctly shaped
///
/// NOTE: This test only covers the error path. The happy-path lifecycle must be
/// validated separately (e.g. via integration tests with a mock trigger source).
#[tokio::test]
async fn test_self_evolution_error_path_no_triggers() {
    let ctx = EvolutionE2eContext::new("livecycle-001");
    let sandbox =
        SandboxExecutor::new(ctx.workdir.clone(), 10).with_allowed_targets(vec!["**/*.rs".into()]);

    // ── 1. Setup EvolutionLoop ────────────────────────────────────────
    let mut loop_instance = EvolutionLoop::new(ctx.workdir.clone())
        .with_sandbox(sandbox)
        .with_approval_mode(ApprovalMode::RequireHuman)
        .with_poll_interval(Duration::from_secs(1));

    // Without trigger sources, run() should immediately return an error.
    let run_result = loop_instance.run().await;
    assert!(
        run_result.is_err(),
        "run() must error without trigger sources"
    );
    let err_msg = format!("{}", run_result.unwrap_err());
    assert!(
        err_msg.contains("no trigger sources"),
        "error must mention missing trigger sources, got: {}",
        err_msg
    );

    // Verify EvolutionTrigger can be created for all known types.
    let triggers = vec![
        EvolutionTrigger::PerformanceRegression {
            metric: "latency_p50".into(),
            threshold: 500.0,
            direction: RegressionDirection::Increasing,
        },
        EvolutionTrigger::RepeatedError {
            pattern: "500 Internal Server Error".into(),
            count: 10,
        },
        EvolutionTrigger::DeadCodeDetected {
            module: "src/legacy.rs".into(),
            ratio: 0.3,
        },
        EvolutionTrigger::ManualRequest {
            instruction: "Optimize database queries".into(),
        },
        EvolutionTrigger::ConfigDrift {
            key: "max_connections".into(),
            expected: "100".into(),
            actual: "50".into(),
        },
        EvolutionTrigger::DegradationDetected {
            capability_id: "auth-service".into(),
            trend_slope: -0.05,
        },
    ];
    for t in &triggers {
        assert!(!t.label().is_empty());
        assert!(!t.description().is_empty());
    }

    // ── 2. Trigger (simulated) ─────────────────────────────────────────
    let trigger = EvolutionTrigger::PerformanceRegression {
        metric: "latency_p50".into(),
        threshold: 500.0,
        direction: RegressionDirection::Increasing,
    };
    assert_eq!(trigger.label(), "performance_regression");
    assert!(
        trigger.description().contains("Performance regression"),
        "trigger description must be descriptive"
    );

    // ── 3. Analyze ─────────────────────────────────────────────────────
    let analysis = make_analysis(trigger);
    // The production constructor generates the id/timestamp — not echoed
    // from caller-provided constants — so assert those real side effects.
    assert!(
        !analysis.analysis_id.is_nil(),
        "analysis must carry a generated id"
    );
    assert!(
        analysis.timestamp_ms > 0,
        "analysis must record a real timestamp"
    );

    // ── 4. Propose (simulated approval) ────────────────────────────────
    let approval = Approval::approved("e2e-tester".into(), Some("Approved for sandbox".into()));
    // is_approved() reflects the real status derived from the constructor.
    assert!(approval.is_approved());
    assert!(
        approval.timestamp_ms > 0,
        "approval must record a real timestamp"
    );

    // ── 5. Sandbox (apply a real patch to a real file) ─────────────────
    use go_on::orchestration::self_evolution::sandbox::CodePatch;
    let target = ctx.workdir.join("src/lib.rs");
    std::fs::create_dir_all(ctx.workdir.join("src")).expect("create src dir");
    std::fs::write(&target, "// original\n").expect("write target file");
    let patch = CodePatch::new(
        "src/lib.rs".into(),
        vec![(1, "// original".into())],
        vec![(1, "// patched".into())],
        "e2e test patch".into(),
    );
    // The constructor generates a patch id and derives the diff from the
    // original/patched lines — both are production behavior.
    assert!(
        patch.patch_id.is_some(),
        "constructor must generate a patch id"
    );
    assert!(
        !patch.diff.is_empty(),
        "diff must be derived for a real change"
    );
    assert!(
        patch.diff.contains("-// original"),
        "diff must contain removal"
    );
    assert!(
        patch.diff.contains("+// patched"),
        "diff must contain insertion"
    );

    // Apply the patch through the production API and verify the on-disk
    // content actually changed.
    let changed = patch
        .apply_to_file(&ctx.workdir)
        .await
        .expect("patch must apply");
    assert_eq!(changed, 1, "one line must be changed");
    let after = std::fs::read_to_string(&target).expect("read patched file");
    assert!(
        after.contains("// patched"),
        "patched content must be on disk, got: {after:?}"
    );
    assert!(
        !after.contains("// original"),
        "original line must be replaced, got: {after:?}"
    );

    // ── 6. BuildResult semantics ───────────────────────────────────────
    // The variant constructors and their semantic accessors (is_success /
    // time_ms / summary) are production API; no caller-provided constants
    // are read back here.
    let build_success = BuildResult::Success {
        warnings: 1,
        time_ms: 42,
    };
    assert!(build_success.is_success());
    assert!(
        build_success.summary().contains("SUCCESS"),
        "summary must label success, got: {}",
        build_success.summary()
    );

    let build_fail = BuildResult::CompileError {
        errors: 3,
        lines: vec![
            "error[E0308]: type mismatch".into(),
            "help: try using `convert`".into(),
        ],
    };
    assert!(!build_fail.is_success());
    // time_ms() is documented as 0 for non-Success variants (no timing data).
    assert_eq!(build_fail.time_ms(), 0);
    assert!(build_fail.summary().contains("COMPILE ERROR"));
    assert!(
        build_fail.summary().contains("error[E0308]"),
        "summary must embed the captured error lines"
    );

    let build_test_fail = BuildResult::TestFailure {
        failed: 2,
        passed: 10,
    };
    assert!(!build_test_fail.is_success());
    assert_eq!(build_test_fail.time_ms(), 0);
    assert!(build_test_fail.summary().contains("TEST FAILURE"));

    // ── 7. Submit & Rollback ───────────────────────────────────────────
    // Real submission uses git commit/deploy. Validate the approval
    // types used for rollback decisions.
    let rollback = Approval::approved(
        "rollback-agent".into(),
        Some("auto-rollback on health check failure".into()),
    );
    assert!(rollback.is_approved());

    // Verify rejected approval.
    let rejected = Approval::rejected("tester".into(), Some("changes too risky".into()));
    assert!(!rejected.is_approved());
}

/// Validates the ERROR path of the evolution loop under auto-approval mode.
/// This test does NOT actually trigger a rollback — it only verifies that
/// EvolutionLoop::run() returns an error when no trigger sources are configured.
///
/// A genuine auto-rollback test would require:
///   - A live EvolutionLoop with a trigger source that fires
///   - A sandbox that reports compile success, followed by a health check
///     that returns failure, causing the loop to call Approval::approved("auto-rollback")
///     and execute a git revert
///
/// For now this test validates:
///   1. run() errors with "no trigger sources" under ApprovalMode::AutoApproval
///   2. The Approval::approved structure used for automated rollback decisions
#[tokio::test]
async fn test_self_evolution_error_path_no_triggers_auto_approval() {
    let ctx = EvolutionE2eContext::new("rollback-001");
    let sandbox =
        SandboxExecutor::new(ctx.workdir.clone(), 3).with_allowed_targets(vec!["**/*.rs".into()]);

    let mut loop_instance = EvolutionLoop::new(ctx.workdir.clone())
        .with_sandbox(sandbox)
        .with_approval_mode(ApprovalMode::AutoApproval);

    // Same as above: no trigger sources → run() returns an error.
    let run_result = loop_instance.run().await;
    assert!(
        run_result.is_err(),
        "run() must error without trigger sources"
    );
    let err_msg = format!("{}", run_result.unwrap_err());
    assert!(
        err_msg.contains("no trigger sources"),
        "error must mention missing trigger sources, got: {}",
        err_msg
    );

    // In auto-rollback, health check failure triggers automatic rollback.
    // Validate the approval structure for automated rollback decisions.
    let rollback_approval =
        Approval::approved("auto-rollback".into(), Some("health check failed".into()));
    assert!(rollback_approval.is_approved());
    assert_eq!(rollback_approval.by, "auto-rollback");
}

/// Validates the happy-path loop machinery: `run()` genuinely starts, polls
/// registered trigger sources, and keeps driving evolution cycles instead of
/// returning immediately or spinning on a no-op.
///
/// A counting mock trigger source fires one real trigger on its first poll
/// (exercising the analyze/propose phases — without an agent the cycle is
/// recorded as a skip) and counts subsequent polls. `run()` runs until the
/// test has observed several polls and then cancels the task.
#[tokio::test]
async fn test_self_evolution_loop_polls_trigger_sources() {
    use go_on::orchestration::self_evolution::evolution_loop::observe::TriggerSource;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[derive(Debug)]
    struct CountingSource {
        polls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl TriggerSource for CountingSource {
        fn name(&self) -> &str {
            "counting_source"
        }
        async fn poll(&self) -> Vec<EvolutionTrigger> {
            let n = self.polls.fetch_add(1, Ordering::SeqCst) + 1;
            if n == 1 {
                // Fire one real trigger so the loop executes an evolution
                // cycle (analyze → propose) rather than only polling.
                vec![EvolutionTrigger::ManualRequest {
                    instruction: "probe evolution cycle".into(),
                }]
            } else {
                vec![]
            }
        }
    }

    let ctx = EvolutionE2eContext::new("loop-001");
    let sandbox =
        SandboxExecutor::new(ctx.workdir.clone(), 3).with_allowed_targets(vec!["**/*.rs".into()]);
    let polls = Arc::new(AtomicUsize::new(0));
    let source = Box::new(CountingSource {
        polls: Arc::clone(&polls),
    });

    let mut loop_instance = EvolutionLoop::new(ctx.workdir.clone())
        .with_sandbox(sandbox)
        .with_trigger_source(source)
        .with_poll_interval(Duration::from_millis(50));

    let handle = tokio::spawn(async move {
        // Runs until cancelled; returns Err only if no trigger sources are
        // registered (which is not the case here).
        let _ = loop_instance.run().await;
    });

    // Wait until the loop has polled the source several times (proving the
    // interval loop is alive and drives cycles), with a bounded deadline.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while polls.load(Ordering::SeqCst) < 3 && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    handle.abort();
    let _ = handle.await;

    assert!(
        polls.load(Ordering::SeqCst) >= 3,
        "run() must poll registered trigger sources repeatedly, got {} polls",
        polls.load(Ordering::SeqCst)
    );
}
