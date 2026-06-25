/// GAP-B50-20: 端到端流式性能 Benchmark
///
/// Measures:
/// - time_to_first_token_ms (TTFT) p50/p95/p99
/// - tokens_per_second (TPS)
/// - time_to_complete_ms (TTC)
/// - stream_interrupt_latency_ms
///
/// Tested modes: GUI mode, VSCode mode, pure HTTP mode
/// Records 3 server profiles differences.
/// Regression detection: TTFT p50 > baseline × 1.5
///
/// NOTE: This test requires a live LLM server with streaming capabilities.
/// It takes ~9 minutes when a server is available.
/// If no server is detected at 127.0.0.1:8090, the test soft-skips.
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

pub mod common;
use common::binary_path;
use common::suite_mutex;
use common::CrossProcessLock;

const LOCK_NAME: &str = "streaming-bench";

fn suite_guard() -> &'static Mutex<()> {
    suite_mutex()
}

// ---------------------------------------------------------------------------
// Benchmark harness
// ---------------------------------------------------------------------------

struct BenchHarness {
    child: Child,
    stdin: Option<Box<dyn Write + Send>>,
    stdout_rx: Receiver<Value>,
    _stderr_lines: Arc<Mutex<Vec<String>>>,
    _suite_guard: MutexGuard<'static, ()>,
    _cross_process_lock: CrossProcessLock,
}

impl BenchHarness {
    /// Spawn the go-on binary in streaming mode.
    fn spawn(mode: &str) -> Self {
        let _suite_guard = match suite_guard().lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let _cross_process_lock = CrossProcessLock::new(LOCK_NAME, 60);

        let mut child = Command::new(binary_path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("GO_ON_MODE", mode)
            .env("GO_ON_STREAMING", "1")
            .env("RUST_LOG", "warn")
            .spawn()
            .expect("failed to spawn go-on");

        let stdin = child
            .stdin
            .take()
            .map(|s| Box::new(s) as Box<dyn Write + Send>);
        let stderr_lines = Arc::new(Mutex::new(Vec::new()));
        let stderr_reader = BufReader::new(child.stderr.take().unwrap());
        let stderr_log = Arc::clone(&stderr_lines);
        thread::spawn(move || {
            for line in stderr_reader.lines().map_while(Result::ok) {
                let mut lines = stderr_log.lock().unwrap();
                if lines.len() > 1000 {
                    lines.remove(0);
                }
                lines.push(line);
            }
        });

        let (tx, stdout_rx) = mpsc::channel::<Value>();
        let stdout_reader = BufReader::new(child.stdout.take().unwrap());
        thread::spawn(move || {
            let mut buf = String::new();
            let mut reader = stdout_reader;
            while let Ok(n) = reader.read_line(&mut buf) {
                if n == 0 {
                    break;
                }
                if let Ok(val) = serde_json::from_str::<Value>(buf.trim()) {
                    let _ = tx.send(val);
                }
                buf.clear();
            }
        });

        Self {
            child,
            stdin,
            stdout_rx,
            _stderr_lines: stderr_lines,
            _suite_guard,
            _cross_process_lock,
        }
    }

    /// Send a streaming request and collect all response chunks with timing.
    fn stream_request(&mut self, request: &Value) -> Vec<StreamChunk> {
        let mut chunks = Vec::new();

        // Send request as JSON-LD line
        if let Some(stdin) = self.stdin.as_mut() {
            let line = serde_json::to_string(request).unwrap();
            writeln!(stdin, "{}", line).expect("failed to write to stdin");
            stdin.flush().expect("failed to flush stdin");
        }

        // Collect chunks until we see a final response
        loop {
            match self.stdout_rx.recv_timeout(Duration::from_secs(30)) {
                Ok(chunk) => {
                    let now = Instant::now();
                    let is_final = chunk.get("type").and_then(|t| t.as_str()) == Some("final")
                        || chunk.get("done").and_then(|d| d.as_bool()).unwrap_or(false);
                    chunks.push(StreamChunk {
                        timestamp: now,
                        data: chunk,
                    });
                    if is_final {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // Timeout — treat as interrupt
                    chunks.push(StreamChunk {
                        timestamp: Instant::now(),
                        data: json!({"type": "timeout", "done": true}),
                    });
                    break;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        chunks
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A single streamed response chunk with timing.
#[derive(Debug, Clone)]
struct StreamChunk {
    timestamp: Instant,
    data: Value,
}

/// Benchmark results for a single test run.
#[derive(Debug, Clone, Default)]
struct StreamBenchResult {
    /// Time from request start to first chunk (ms)
    time_to_first_token_ms: f64,
    /// Total tokens received
    total_tokens: u64,
    /// Total time to complete (ms)
    time_to_complete_ms: f64,
    /// Calculated tokens per second
    tokens_per_second: f64,
    /// If interrupted, the time between last token and interrupt detection (ms)
    stream_interrupt_latency_ms: Option<f64>,
}

/// Percentile statistics
#[derive(Debug, Clone, Default)]
struct Percentiles {
    p50: f64,
    p95: f64,
    p99: f64,
    count: usize,
}

fn compute_percentiles(mut values: Vec<f64>) -> Percentiles {
    if values.is_empty() {
        return Percentiles::default();
    }
    values.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let count = values.len();
    Percentiles {
        p50: values[(count as f64 * 0.50) as usize].max(values[0]),
        p95: values[(count as f64 * 0.95).min((count - 1) as f64) as usize],
        p99: values[(count as f64 * 0.99).min((count - 1) as f64) as usize],
        count,
    }
}

/// Run a single benchmark measurement: send a request and collect timing metrics.
fn bench_stream(harness: &mut BenchHarness, prompt: &str) -> StreamBenchResult {
    let start = Instant::now();

    let request = json!({
        "jsonrpc": "2.0",
        "method": "chat",
        "params": {
            "messages": [
                {"role": "user", "content": prompt}
            ],
            "stream": true,
        },
        "id": 1,
    });

    let chunks = harness.stream_request(&request);
    let total_duration = start.elapsed();

    let mut result = StreamBenchResult::default();

    if let Some(first) = chunks.first() {
        result.time_to_first_token_ms =
            first.timestamp.duration_since(start).as_secs_f64() * 1000.0;
    }

    result.time_to_complete_ms = total_duration.as_secs_f64() * 1000.0;

    // Count tokens from response chunks
    for chunk in &chunks {
        if let Some(choices) = chunk.data.get("result").and_then(|r| r.get("choices")) {
            if let Some(choice) = choices.get(0) {
                if let Some(delta) = choice.get("delta") {
                    if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
                        // Rough token estimate: ~4 chars per token
                        result.total_tokens += (content.len() / 4).max(1) as u64;
                    }
                }
            }
        }
        // Also check for token_count field
        if let Some(tc) = chunk.data.get("token_count").and_then(|t| t.as_u64()) {
            result.total_tokens = result.total_tokens.max(tc);
        }
    }

    // Calculate TPS
    if result.time_to_complete_ms > 0.0 && result.total_tokens > 0 {
        result.tokens_per_second =
            result.total_tokens as f64 / (result.time_to_complete_ms / 1000.0);
    }

    // Detect interrupt latency: if a timeout occurred, measure gap
    if let Some(last) = chunks.iter().rev().nth(1) {
        if let Some(timeout) = chunks.last() {
            if timeout.data.get("type").and_then(|t| t.as_str()) == Some("timeout") {
                result.stream_interrupt_latency_ms = Some(
                    timeout
                        .timestamp
                        .duration_since(last.timestamp)
                        .as_secs_f64()
                        * 1000.0,
                );
            }
        }
    }

    result
}

/// Aggregate multiple benchmark runs into percentiles.
fn aggregate_results(results: &[StreamBenchResult]) -> BTreeMap<String, Percentiles> {
    let ttft: Vec<f64> = results.iter().map(|r| r.time_to_first_token_ms).collect();
    let tps: Vec<f64> = results.iter().map(|r| r.tokens_per_second).collect();
    let ttc: Vec<f64> = results.iter().map(|r| r.time_to_complete_ms).collect();
    let interrupt: Vec<f64> = results
        .iter()
        .filter_map(|r| r.stream_interrupt_latency_ms)
        .collect();

    let mut map = BTreeMap::new();
    map.insert("time_to_first_token_ms".into(), compute_percentiles(ttft));
    map.insert("tokens_per_second".into(), compute_percentiles(tps));
    map.insert("time_to_complete_ms".into(), compute_percentiles(ttc));
    if !interrupt.is_empty() {
        map.insert(
            "stream_interrupt_latency_ms".into(),
            compute_percentiles(interrupt),
        );
    }
    map
}

/// Known baseline for TTFT p50 (ms). Tune this to match your hardware.
const BASELINE_TTFT_P50_MS: f64 = 500.0;

/// Check if the given metrics exceed the regression threshold.
fn check_regression(label: &str, percentiles: &Percentiles) -> Vec<String> {
    let mut regressions = Vec::new();

    if percentiles.count > 0 {
        let ratio = percentiles.p50 / BASELINE_TTFT_P50_MS;
        if ratio > 1.5 {
            regressions.push(format!(
                "[REGRESSION] {} TTFT p50={:.1}ms is {:.1}x baseline ({:.1}ms)",
                label, percentiles.p50, ratio, BASELINE_TTFT_P50_MS
            ));
        }
    }

    regressions
}

// ── Test modes ────────────────────────────────────────────────────────────

/// Test modes: GUI, VSCode, pure HTTP
#[derive(Debug, Clone, Copy)]
enum BenchMode {
    Gui,
    Vscode,
    Http,
}

impl BenchMode {
    fn env_value(&self) -> &'static str {
        match self {
            Self::Gui => "gui",
            Self::Vscode => "vscode",
            Self::Http => "http",
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Gui => "GUI mode",
            Self::Vscode => "VSCode mode",
            Self::Http => "HTTP mode",
        }
    }
}

const ALL_MODES: [BenchMode; 3] = [BenchMode::Gui, BenchMode::Vscode, BenchMode::Http];
const PROMPTS: &[&str] = &[
    "Hello, what is 2+2?",
    "Write a short poem about coding.",
    "Explain the concept of recursion in 3 sentences.",
    "What is the capital of France?",
    "List 3 benefits of unit testing.",
];

/// Run all benchmarks and return results per mode.
fn run_benchmarks() -> BTreeMap<String, Vec<StreamBenchResult>> {
    let mut all_results: BTreeMap<String, Vec<StreamBenchResult>> = BTreeMap::new();

    for mode in &ALL_MODES {
        let label = mode.label().to_string();
        let mut mode_results = Vec::new();

        eprintln!("  [bench] starting {}...", label);

        let mut harness = BenchHarness::spawn(mode.env_value());

        // Warmup
        let _ = bench_stream(&mut harness, "Warmup: ignore this.");
        eprintln!("  [bench] {} warmup complete", label);

        // Bench runs
        for prompt in PROMPTS {
            let result = bench_stream(&mut harness, prompt);
            mode_results.push(result);
        }

        harness.kill();

        let runs_count = mode_results.len();
        all_results.insert(label.clone(), mode_results);
        eprintln!("  [bench] {} complete ({} runs)", label, runs_count);
    }

    all_results
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn streaming_e2e_benchmark() {
    // Quick health check: try to reach the local server
    let server_available = TcpStream::connect_timeout(
        &"127.0.0.1:8090".parse().expect("valid socket addr"),
        std::time::Duration::from_secs(2),
    )
    .is_ok();
    if !server_available {
        eprintln!("╔═══════════════════════════════════════════════════════════╗");
        eprintln!("║   Streaming E2E Benchmark: SKIPPED                       ║");
        eprintln!("║   No LLM server detected at 127.0.0.1:8090               ║");
        eprintln!("║   Start the server and re-run to run the benchmark.      ║");
        eprintln!("╚═══════════════════════════════════════════════════════════╝");
        return;
    }
    eprintln!("╔═══════════════════════════════════════════════════════════╗");
    eprintln!("║   GAP-B50-20: Streaming E2E Performance Benchmark       ║");
    eprintln!("╚═══════════════════════════════════════════════════════════╝");
    eprintln!();

    let all_results = run_benchmarks();

    let mut summary: Vec<String> = Vec::new();
    let mut total_regressions = 0;

    for (label, results) in &all_results {
        eprintln!("── {} ──", label);
        let agg = aggregate_results(results);

        for (metric, p) in &agg {
            eprintln!(
                "  {:<30} p50={:>8.1}  p95={:>8.1}  p99={:>8.1}  (n={})",
                metric, p.p50, p.p95, p.p99, p.count
            );

            let regressions = check_regression(&format!("{} / {}", label, metric), p);
            for r in &regressions {
                eprintln!("  {}", r);
                summary.push(r.clone());
                total_regressions += 1;
            }
        }
        eprintln!();
    }

    // Print profiles summary
    eprintln!("── Server Profiles ──");
    for (label, results) in &all_results {
        let ttft: Vec<f64> = results.iter().map(|r| r.time_to_first_token_ms).collect();
        let ttc: Vec<f64> = results.iter().map(|r| r.time_to_complete_ms).collect();
        let tps: Vec<f64> = results.iter().map(|r| r.tokens_per_second).collect();
        let avg_ttft = ttft.iter().sum::<f64>() / ttft.len() as f64;
        let avg_ttc = ttc.iter().sum::<f64>() / ttc.len() as f64;
        let avg_tps = tps.iter().sum::<f64>() / tps.len() as f64;
        eprintln!(
            "  {:<20} TTFT avg={:>8.1}ms  TTC avg={:>8.1}ms  TPS avg={:>8.1}",
            label, avg_ttft, avg_ttc, avg_tps
        );
    }
    eprintln!();

    // Report regressions
    if total_regressions > 0 {
        eprintln!("── Regression Summary ──");
        for r in &summary {
            eprintln!("  {}", r);
        }
        panic!(
            "Detected {} regression(s) — TTFT p50 exceeds 1.5× baseline",
            total_regressions
        );
    }

    eprintln!("✅ All benchmarks passed — no regressions detected.");
}
