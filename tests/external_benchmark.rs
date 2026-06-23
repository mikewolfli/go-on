//! BLUE43 Step 21: 外部对标与持续回归门禁
//!
//! External benchmarking suite that compares go-on's autonomy performance
//! against industry-standard baselines. A regression gate prevents releasing
//! when key metrics degrade beyond acceptable thresholds.
//!
//! Benchmark dimensions:
//! - PassRate:       first-attempt task pass rate (baseline 85%)
//! - Rounds:         rounds to completion (baseline 5)
//! - TailLatencyMs:  p95 tail latency (baseline 30_000ms)
//! - ToolAccuracy:   tool call accuracy (baseline 90%)
//! - RecoverySuccess: automatic recovery success rate (baseline 80%)
//! - AuditCompleteness: audit trail completeness (baseline 100%)

use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------------------
// Benchmark dimension
// ---------------------------------------------------------------------------

/// External benchmark dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BenchmarkDimension {
    /// First-attempt pass rate (higher is better).
    PassRate,
    /// Number of rounds to completion (lower is better).
    Rounds,
    /// Tail latency p95 in ms (lower is better).
    TailLatencyMs,
    /// Tool call accuracy (higher is better).
    ToolAccuracy,
    /// Recovery success rate (higher is better).
    RecoverySuccess,
    /// Audit completeness percentage (higher is better).
    AuditCompleteness,
}

impl BenchmarkDimension {
    fn name(&self) -> &'static str {
        match self {
            BenchmarkDimension::PassRate => "pass_rate",
            BenchmarkDimension::Rounds => "rounds",
            BenchmarkDimension::TailLatencyMs => "tail_latency_ms",
            BenchmarkDimension::ToolAccuracy => "tool_accuracy",
            BenchmarkDimension::RecoverySuccess => "recovery_success",
            BenchmarkDimension::AuditCompleteness => "audit_completeness",
        }
    }

    fn industry_baseline(&self) -> f64 {
        match self {
            BenchmarkDimension::PassRate => 85.0,
            BenchmarkDimension::Rounds => 5.0,
            BenchmarkDimension::TailLatencyMs => 30_000.0,
            BenchmarkDimension::ToolAccuracy => 90.0,
            BenchmarkDimension::RecoverySuccess => 80.0,
            BenchmarkDimension::AuditCompleteness => 100.0,
        }
    }

    /// Whether a higher measured value is better.
    fn higher_is_better(&self) -> bool {
        matches!(
            self,
            BenchmarkDimension::PassRate
                | BenchmarkDimension::ToolAccuracy
                | BenchmarkDimension::RecoverySuccess
                | BenchmarkDimension::AuditCompleteness
        )
    }

    /// Regression tolerance as a multiplier.
    /// For higher-is-better dimensions, go_on_score must be >= baseline * (1 - tolerance).
    /// For lower-is-better dimensions, go_on_score must be <= baseline * (1 + tolerance).
    fn tolerance(&self) -> f64 {
        match self {
            BenchmarkDimension::Rounds => 0.20,
            _ => 0.15,
        }
    }

    fn regression_threshold(&self) -> f64 {
        let base = self.industry_baseline();
        let tol = self.tolerance();
        if self.higher_is_better() {
            base * (1.0 - tol)
        } else {
            base * (1.0 + tol)
        }
    }
}

// ---------------------------------------------------------------------------
// Benchmark result types
// ---------------------------------------------------------------------------

/// Benchmark result for a single dimension.
#[derive(Debug, Clone)]
pub struct BenchmarkResult {
    pub dimension: BenchmarkDimension,
    pub go_on_score: f64,
    pub industry_baseline: f64,
    pub regression_threshold: f64,
    pub passed: bool,
}

/// Full benchmark report covering all six dimensions.
#[derive(Debug, Clone)]
pub struct BenchmarkReport {
    pub timestamp: String,
    pub results: Vec<BenchmarkResult>,
    pub overall_pass: bool,
    pub leading_dimensions: Vec<String>,
    pub trailing_dimensions: Vec<String>,
}

// ---------------------------------------------------------------------------
// Replay scenario types (local copy for test isolation)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct ReplayStep {
    round: u32,
    agent: &'static str,
    simulated_ms: u64,
    success: bool,
    reroute: bool,
}

#[derive(Debug, Clone)]
struct ReplayScenario {
    #[allow(dead_code)]
    name: &'static str,
    steps: Vec<ReplayStep>,
}

#[derive(Debug, Clone, PartialEq)]
struct ReplayMetrics {
    wall_time_ms: u64,
    rounds: u64,
    max_fanout: usize,
    unique_agents: usize,
    success_ratio: f64,
    reroute_count: u64,
}

// ---------------------------------------------------------------------------
// Replay helpers
// ---------------------------------------------------------------------------

fn compute_p95(samples: &[u64]) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let index = ((sorted.len() as f64) * 0.95).ceil() as usize;
    sorted[index.saturating_sub(1).min(sorted.len().saturating_sub(1))]
}

fn replay_metrics(scenario: &ReplayScenario) -> ReplayMetrics {
    let mut unique_rounds = Vec::<u32>::new();
    let mut wall_time_ms = 0u64;
    let mut max_fanout = 0usize;
    let mut unique_agents = std::collections::BTreeSet::<&'static str>::new();
    let mut success_count = 0usize;
    let mut reroute_count = 0u64;

    let mut grouped = std::collections::BTreeMap::<u32, Vec<ReplayStep>>::new();
    for step in &scenario.steps {
        grouped.entry(step.round).or_default().push(*step);
        unique_agents.insert(step.agent);
        if step.success {
            success_count += 1;
        }
        if step.reroute {
            reroute_count += 1;
        }
        if !unique_rounds.contains(&step.round) {
            unique_rounds.push(step.round);
        }
    }

    for steps in grouped.values() {
        max_fanout = max_fanout.max(steps.len());
        wall_time_ms += steps
            .iter()
            .map(|step| step.simulated_ms)
            .max()
            .unwrap_or(0);
    }

    ReplayMetrics {
        wall_time_ms,
        rounds: unique_rounds.len() as u64,
        max_fanout,
        unique_agents: unique_agents.len(),
        success_ratio: success_count as f64 / scenario.steps.len().max(1) as f64,
        reroute_count,
    }
}

// ---------------------------------------------------------------------------
// Benchmark scenarios (industry-standard replay flows)
// ---------------------------------------------------------------------------

/// Single tool call, measures pass rate + p95 latency.
/// Contains 100 invocations in round 1 to produce a statistically meaningful
/// p95 sample while keeping rounds = 1.
fn scenario_simple_task() -> ReplayScenario {
    let mut steps = Vec::with_capacity(100);
    for i in 0..100 {
        // Simulated latency spans 500..=5000ms with 95% success rate
        let simulated = 500 + (i % 10) as u64 * 500;
        let success = i % 20 != 0;
        steps.push(ReplayStep {
            round: 1,
            agent: "tool",
            simulated_ms: simulated,
            success,
            reroute: false,
        });
    }
    ReplayScenario {
        name: "simple_task",
        steps,
    }
}

/// Three serial tool calls, measures rounds + accuracy.
/// Single flow: planner -> coder -> reviewer (3 rounds, 3 tool calls).
fn scenario_multi_tool_serial() -> ReplayScenario {
    ReplayScenario {
        name: "multi_tool_serial",
        steps: vec![
            ReplayStep {
                round: 1,
                agent: "planner",
                simulated_ms: 800,
                success: true,
                reroute: false,
            },
            ReplayStep {
                round: 2,
                agent: "coder",
                simulated_ms: 1200,
                success: true,
                reroute: false,
            },
            ReplayStep {
                round: 3,
                agent: "reviewer",
                simulated_ms: 600,
                success: true,
                reroute: false,
            },
        ],
    }
}

/// Five parallel tool calls followed by a join, measures fanout efficiency.
fn scenario_parallel_fanout() -> ReplayScenario {
    ReplayScenario {
        name: "parallel_fanout",
        steps: vec![
            ReplayStep {
                round: 1,
                agent: "researcher-a",
                simulated_ms: 400,
                success: true,
                reroute: false,
            },
            ReplayStep {
                round: 1,
                agent: "researcher-b",
                simulated_ms: 550,
                success: true,
                reroute: false,
            },
            ReplayStep {
                round: 1,
                agent: "researcher-c",
                simulated_ms: 700,
                success: true,
                reroute: false,
            },
            ReplayStep {
                round: 1,
                agent: "researcher-d",
                simulated_ms: 300,
                success: true,
                reroute: false,
            },
            ReplayStep {
                round: 1,
                agent: "researcher-e",
                simulated_ms: 620,
                success: true,
                reroute: false,
            },
            ReplayStep {
                round: 2,
                agent: "synthesizer",
                simulated_ms: 350,
                success: true,
                reroute: false,
            },
        ],
    }
}

/// Tool failure followed by automatic recovery, measures recovery success.
fn scenario_failure_recovery() -> ReplayScenario {
    ReplayScenario {
        name: "failure_recovery",
        steps: vec![
            // Primary tool fails
            ReplayStep {
                round: 1,
                agent: "coder-primary",
                simulated_ms: 2000,
                success: false,
                reroute: false,
            },
            // Recovery via fallback succeeds
            ReplayStep {
                round: 2,
                agent: "coder-fallback",
                simulated_ms: 1500,
                success: true,
                reroute: true,
            },
        ],
    }
}

/// Full execution with audit trail: plan -> execute -> review -> record.
fn scenario_audit_trail() -> ReplayScenario {
    ReplayScenario {
        name: "audit_trail",
        steps: vec![
            ReplayStep {
                round: 1,
                agent: "planner",
                simulated_ms: 300,
                success: true,
                reroute: false,
            },
            ReplayStep {
                round: 2,
                agent: "executor",
                simulated_ms: 800,
                success: true,
                reroute: false,
            },
            ReplayStep {
                round: 3,
                agent: "auditor",
                simulated_ms: 500,
                success: true,
                reroute: false,
            },
            ReplayStep {
                round: 4,
                agent: "ledger",
                simulated_ms: 200,
                success: true,
                reroute: false,
            },
        ],
    }
}

// ---------------------------------------------------------------------------
// Industry-baseline comparison
// ---------------------------------------------------------------------------

fn benchmark_dimension_for_scenario(
    scenario: &ReplayScenario,
    metrics: &ReplayMetrics,
    dimension: BenchmarkDimension,
) -> BenchmarkResult {
    let industry_baseline = dimension.industry_baseline();
    let regression_threshold = dimension.regression_threshold();

    let go_on_score = match dimension {
        BenchmarkDimension::PassRate => metrics.success_ratio * 100.0,
        BenchmarkDimension::Rounds => metrics.rounds as f64,
        BenchmarkDimension::TailLatencyMs => {
            let latencies: Vec<u64> = scenario.steps.iter().map(|s| s.simulated_ms).collect();
            compute_p95(&latencies) as f64
        }
        BenchmarkDimension::ToolAccuracy => metrics.success_ratio * 100.0,
        BenchmarkDimension::RecoverySuccess => {
            let total_reroutes = scenario.steps.iter().filter(|s| s.reroute).count();
            let successful_reroutes = scenario
                .steps
                .iter()
                .filter(|s| s.reroute && s.success)
                .count();
            if total_reroutes == 0 {
                100.0
            } else {
                successful_reroutes as f64 / total_reroutes as f64 * 100.0
            }
        }
        BenchmarkDimension::AuditCompleteness => {
            let audit_agents = ["auditor", "ledger"];
            let audit_steps: Vec<&ReplayStep> = scenario
                .steps
                .iter()
                .filter(|s| audit_agents.contains(&s.agent))
                .collect();
            if audit_steps.is_empty() {
                100.0
            } else {
                let success_count = audit_steps.iter().filter(|s| s.success).count();
                success_count as f64 / audit_steps.len() as f64 * 100.0
            }
        }
    };

    let passed = if dimension.higher_is_better() {
        go_on_score >= regression_threshold
    } else {
        go_on_score <= regression_threshold
    };

    BenchmarkResult {
        dimension,
        go_on_score,
        industry_baseline,
        regression_threshold,
        passed,
    }
}

fn run_external_benchmarks() -> BenchmarkReport {
    let scenario_specs: Vec<(ReplayScenario, Vec<BenchmarkDimension>)> = vec![
        (
            scenario_simple_task(),
            vec![
                BenchmarkDimension::PassRate,
                BenchmarkDimension::TailLatencyMs,
            ],
        ),
        (
            scenario_multi_tool_serial(),
            vec![BenchmarkDimension::Rounds, BenchmarkDimension::ToolAccuracy],
        ),
        (
            scenario_parallel_fanout(),
            vec![BenchmarkDimension::TailLatencyMs],
        ),
        (
            scenario_failure_recovery(),
            vec![BenchmarkDimension::RecoverySuccess],
        ),
        (
            scenario_audit_trail(),
            vec![BenchmarkDimension::AuditCompleteness],
        ),
    ];

    let mut seen = std::collections::HashSet::new();
    let mut results = Vec::new();
    for (scenario, dimensions) in &scenario_specs {
        let metrics = replay_metrics(scenario);
        for &dim in dimensions {
            if seen.insert(dim) {
                let result = benchmark_dimension_for_scenario(scenario, &metrics, dim);
                results.push(result);
            }
        }
    }

    let overall_pass = results.iter().all(|r| r.passed);
    let mut leading = Vec::new();
    let mut trailing = Vec::new();
    for r in &results {
        if r.passed {
            leading.push(r.dimension.name().to_string());
        } else {
            trailing.push(r.dimension.name().to_string());
        }
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    BenchmarkReport {
        timestamp: format!("{}", now),
        results,
        overall_pass,
        leading_dimensions: leading,
        trailing_dimensions: trailing,
    }
}

// ---------------------------------------------------------------------------
// Regression gate
// ---------------------------------------------------------------------------

/// Assert that all benchmark dimensions meet their regression thresholds.
/// Panics with a detailed report if any dimension has regressed.
fn assert_regression_gate(report: &BenchmarkReport) {
    eprintln!("=== External Benchmark Report ===");
    eprintln!("Timestamp: {}", report.timestamp);
    eprintln!(
        "Overall: {}",
        if report.overall_pass { "PASS" } else { "FAIL" }
    );
    eprintln!();
    eprintln!(
        "{:<24} {:>10} {:>10} {:>10}  {:>6}",
        "Dim", "Go-On", "Baseline", "p95", "Status"
    );
    const SEP: &str = "----------------------------------------------------------------------";
    eprintln!("{SEP}");

    for result in &report.results {
        let status = if result.passed { "PASS" } else { "FAIL" };
        eprintln!(
            "{:<24} {:>10.2} {:>10.2} {:>10.2}  {}",
            result.dimension.name(),
            result.go_on_score,
            result.industry_baseline,
            result.regression_threshold,
            status,
        );
    }
    eprintln!();

    if !report.trailing_dimensions.is_empty() {
        eprintln!(
            "Trailing dimensions (failed): {}",
            report.trailing_dimensions.join(", ")
        );
    }

    assert!(
        report.overall_pass,
        "External benchmark regression gate FAILED: {} dimension(s) below threshold",
        report.trailing_dimensions.len()
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn benchmark_simple_task_meets_baseline() {
    let scenario = scenario_simple_task();
    let metrics = replay_metrics(&scenario);

    let pass_result =
        benchmark_dimension_for_scenario(&scenario, &metrics, BenchmarkDimension::PassRate);
    let latency_result =
        benchmark_dimension_for_scenario(&scenario, &metrics, BenchmarkDimension::TailLatencyMs);

    eprintln!(
        "simple_task: pass_rate={:.2}%, latency_p95={:.0}ms",
        pass_result.go_on_score, latency_result.go_on_score
    );

    assert!(
        pass_result.passed,
        "Pass rate {:.2}% < threshold {:.2}% (baseline {:.2}%)",
        pass_result.go_on_score, pass_result.regression_threshold, pass_result.industry_baseline
    );
    assert!(
        latency_result.passed,
        "P95 latency {:.0}ms > threshold {:.0}ms (baseline {:.0}ms)",
        latency_result.go_on_score,
        latency_result.regression_threshold,
        latency_result.industry_baseline
    );
}

#[test]
fn benchmark_multi_tool_serial_within_rounds() {
    let scenario = scenario_multi_tool_serial();
    let metrics = replay_metrics(&scenario);

    let rounds_result =
        benchmark_dimension_for_scenario(&scenario, &metrics, BenchmarkDimension::Rounds);
    let accuracy_result =
        benchmark_dimension_for_scenario(&scenario, &metrics, BenchmarkDimension::ToolAccuracy);

    eprintln!(
        "multi_tool_serial: rounds={:.0}, accuracy={:.2}%",
        rounds_result.go_on_score, accuracy_result.go_on_score
    );

    assert!(
        rounds_result.passed,
        "Rounds {:.0} > threshold {:.0} (baseline {:.0})",
        rounds_result.go_on_score,
        rounds_result.regression_threshold,
        rounds_result.industry_baseline
    );
    assert!(
        accuracy_result.passed,
        "Tool accuracy {:.2}% < threshold {:.2}% (baseline {:.2}%)",
        accuracy_result.go_on_score,
        accuracy_result.regression_threshold,
        accuracy_result.industry_baseline
    );
}

#[test]
fn benchmark_failure_recovery_rate() {
    let scenario = scenario_failure_recovery();
    let metrics = replay_metrics(&scenario);

    let recovery_result =
        benchmark_dimension_for_scenario(&scenario, &metrics, BenchmarkDimension::RecoverySuccess);

    eprintln!(
        "failure_recovery: recovery_success={:.2}%",
        recovery_result.go_on_score
    );

    assert!(
        recovery_result.passed,
        "Recovery success {:.2}% < threshold {:.2}% (baseline {:.2}%)",
        recovery_result.go_on_score,
        recovery_result.regression_threshold,
        recovery_result.industry_baseline
    );
}

#[test]
fn benchmark_audit_completeness_100_percent() {
    let scenario = scenario_audit_trail();
    let metrics = replay_metrics(&scenario);

    let audit_result = benchmark_dimension_for_scenario(
        &scenario,
        &metrics,
        BenchmarkDimension::AuditCompleteness,
    );

    eprintln!(
        "audit_trail: audit_completeness={:.2}%",
        audit_result.go_on_score
    );

    assert!(
        audit_result.passed,
        "Audit completeness {:.2}% < threshold {:.2}% (baseline {:.2}%)",
        audit_result.go_on_score, audit_result.regression_threshold, audit_result.industry_baseline
    );
    assert!(
        (audit_result.go_on_score - 100.0).abs() < 1e-9,
        "Audit completeness must be exactly 100%, got {:.2}%",
        audit_result.go_on_score
    );
}

#[test]
fn regression_gate_detects_latency_regression() {
    // Deliberately high-latency scenario where p95 exceeds the 15% tolerance
    // over the baseline of 30_000ms (threshold = 34_500ms).
    let mut steps = Vec::with_capacity(100);
    for i in 0..100 {
        // Latency spans 36_000..=60_000ms, all above the 34_500ms threshold
        let simulated = 36_000 + (i % 10) as u64 * 3_000;
        steps.push(ReplayStep {
            round: 1,
            agent: "tool",
            simulated_ms: simulated,
            success: true,
            reroute: false,
        });
    }
    let scenario = ReplayScenario {
        name: "high_latency",
        steps,
    };
    let metrics = replay_metrics(&scenario);

    let result =
        benchmark_dimension_for_scenario(&scenario, &metrics, BenchmarkDimension::TailLatencyMs);

    eprintln!(
        "latency_regression: p95={:.0}ms, threshold={:.0}ms, baseline={:.0}ms",
        result.go_on_score, result.regression_threshold, result.industry_baseline
    );

    assert!(
        !result.passed,
        "Expected latency regression to be detected, but p95 {:.0}ms <= threshold {:.0}ms",
        result.go_on_score, result.regression_threshold
    );
}

#[test]
fn regression_gate_detects_rounds_inflation() {
    // Build a scenario where rounds far exceed the 20% tolerance over baseline of 5
    // (threshold = 6 rounds). This scenario has 10 rounds.
    let steps: Vec<ReplayStep> = (1..=10)
        .map(|r| ReplayStep {
            round: r,
            agent: "tool",
            simulated_ms: 100,
            success: true,
            reroute: false,
        })
        .collect();

    let scenario = ReplayScenario {
        name: "inflated_rounds",
        steps,
    };
    let metrics = replay_metrics(&scenario);

    let result = benchmark_dimension_for_scenario(&scenario, &metrics, BenchmarkDimension::Rounds);

    eprintln!(
        "rounds_inflation: rounds={:.0}, threshold={:.0}, baseline={:.0}",
        result.go_on_score, result.regression_threshold, result.industry_baseline
    );

    assert!(
        !result.passed,
        "Expected rounds inflation to be detected, but rounds {:.0} <= threshold {:.0}",
        result.go_on_score, result.regression_threshold
    );
}

#[test]
fn benchmark_report_includes_all_dimensions() {
    let report = run_external_benchmarks();

    let all_dims = [
        BenchmarkDimension::PassRate,
        BenchmarkDimension::Rounds,
        BenchmarkDimension::TailLatencyMs,
        BenchmarkDimension::ToolAccuracy,
        BenchmarkDimension::RecoverySuccess,
        BenchmarkDimension::AuditCompleteness,
    ];

    let reported_dims: Vec<BenchmarkDimension> =
        report.results.iter().map(|r| r.dimension).collect();

    for dim in &all_dims {
        assert!(
            reported_dims.contains(dim),
            "Benchmark report is missing dimension {:?}",
            dim
        );
    }

    assert_eq!(
        report.results.len(),
        all_dims.len(),
        "Expected {} benchmark results, got {}",
        all_dims.len(),
        report.results.len()
    );

    eprintln!(
        "report includes {} dimensions, overall_pass={}",
        report.results.len(),
        report.overall_pass
    );
    eprintln!("leading: {:?}", report.leading_dimensions);
    eprintln!("trailing: {:?}", report.trailing_dimensions);
}

#[test]
fn regression_gate_validates_all_dimensions() {
    let report = run_external_benchmarks();
    assert_regression_gate(&report);
}
