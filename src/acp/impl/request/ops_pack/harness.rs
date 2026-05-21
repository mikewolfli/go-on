//! Harness suite classification and status handler.
//!
//! `classify_harness_suite` categorizes request scenario files by name,
//! and `handle_harness_status` returns a harness status snapshot.

use std::path::Path;

use serde_json::{json, Value};
use tracing::warn;

use super::super::*;

fn classify_harness_suite(name: &str) -> &'static str {
    let lowered = name.to_ascii_lowercase();
    if lowered.contains("adversarial") || lowered.contains("fault") || lowered.contains("chaos") {
        "adversarial"
    } else if lowered.contains("long-chain") || lowered.contains("long_chain") {
        "long_chain"
    } else if lowered.contains("smoke")
        || lowered.contains("runtime-health")
        || lowered.contains("quality-benchmark")
    {
        "smoke"
    } else {
        "regression"
    }
}

pub(super) async fn handle_harness_status(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let fixed_seed = params
        .get("seed")
        .and_then(Value::as_u64)
        .unwrap_or(20260415);

    let mut smoke = Vec::new();
    let mut regression = Vec::new();
    let mut adversarial = Vec::new();
    let mut long_chain = Vec::new();
    let mut warnings = Vec::new();

    let requests_root = Path::new("requests");
    match std::fs::read_dir(requests_root) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let path = entry.path();
                let is_ndjson = path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| ext.eq_ignore_ascii_case("ndjson"))
                    .unwrap_or(false);
                if !is_ndjson {
                    continue;
                }
                let Some(name) = path
                    .file_name()
                    .and_then(|item| item.to_str())
                    .map(|item| item.to_string())
                else {
                    continue;
                };

                match classify_harness_suite(&name) {
                    "smoke" => smoke.push(name),
                    "adversarial" => adversarial.push(name),
                    "long_chain" => long_chain.push(name),
                    _ => regression.push(name),
                }
            }
            smoke.sort();
            regression.sort();
            adversarial.sort();
            long_chain.sort();
        }
        Err(err) => {
            warnings.push(format!("failed to read requests directory: {err}"));
        }
    }

    let scenario_total = smoke.len() + regression.len() + adversarial.len() + long_chain.len();
    let metrics = server.observability.metrics.snapshot();
    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "harness": {
                "fixed_seed": fixed_seed,
                "scenario_total": scenario_total,
                "suites": {
                    "smoke": {
                        "count": smoke.len(),
                        "files": smoke,
                    },
                    "regression": {
                        "count": regression.len(),
                        "files": regression,
                    },
                    "adversarial": {
                        "count": adversarial.len(),
                        "files": adversarial,
                    },
                    "long_chain": {
                        "count": long_chain.len(),
                        "files": long_chain,
                    },
                },
                "scorecard": [
                    {
                        "dimension": "correctness",
                        "target": "all scenarios pass without rpc error",
                        "status": "tracked",
                    },
                    {
                        "dimension": "stability",
                        "target": "runtime.health remains healthy across suites",
                        "status": "tracked",
                    },
                    {
                        "dimension": "latency",
                        "target": "p95 bounded by phase timeout budget",
                        "status": "tracked",
                    },
                    {
                        "dimension": "cost",
                        "target": "timeout spikes remain within baseline",
                        "status": "tracked",
                    },
                    {
                        "dimension": "safety",
                        "target": "security.baseline level stays warn/ok before deploy",
                        "status": "tracked",
                    }
                ],
                "runtime_snapshot": {
                    "total_requests": metrics.total_requests,
                    "failed_requests": metrics.failed_requests,
                    "agent_timeout_failures_total": metrics.agent_timeout_failures_total,
                    "review_gate_timeout_total": metrics.review_gate_timeout_total,
                    "runtime_probe_timeout_total": metrics.runtime_probe_timeout_total,
                },
                "warnings": warnings,
            },
        }),
    )
    .await
}
