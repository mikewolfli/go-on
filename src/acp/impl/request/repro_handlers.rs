//! Reproducibility and optimization handlers extracted from runtime_pack.rs.
//!
//! Provides the `handle_optimization_peak` handler for runtime optimization
//! analysis. The `handle_build_repro` function (reproducible build tracking)
//! is already in `repro_pack.rs`.

use super::*;

pub(super) async fn handle_optimization_peak(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let status = server.get_status();
    let total_requests = status.metrics.total_requests;
    let failed_requests = status.metrics.failed_requests;
    let success_requests = total_requests.saturating_sub(failed_requests);
    let task_success_rate = if total_requests == 0 {
        1.0
    } else {
        success_requests as f64 / total_requests as f64
    };
    let gates = vec![
        serde_json::json!({"name": "reliability", "ready": task_success_rate >= 0.80}),
        serde_json::json!({"name": "stability", "ready": status.lifecycle.is_healthy}),
        serde_json::json!({"name": "observability", "ready": status.metrics.circuit_breaker_open_count == 0}),
    ];
    let overall_pass = gates
        .iter()
        .all(|gate| gate.get("ready").and_then(Value::as_bool).unwrap_or(false));

    send_result(
        server,
        request_id,
        serde_json::json!({
            "ok": true,
            "peak": {
                "total_requests": total_requests,
                "failed_requests": failed_requests,
                "gates": gates,
                "overall_pass": overall_pass,
                "indicators": {
                    "task_success_rate": task_success_rate,
                    "failure_ratio": 1.0 - task_success_rate,
                },
                "scorecard": {
                    "dimensions": {
                        "knowledge_refinement_score": (task_success_rate * 100.0),
                        "reliability_score": (task_success_rate * 100.0),
                    }
                }
            },
            "recommendations": [],
        }),
    )
    .await
}
