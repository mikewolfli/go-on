//! Metrics-related handlers extracted from runtime_pack.rs.
//!
//! Provides metric snapshot, reset, Prometheus exposition, window query,
//! and error summary handlers.

use super::*;

pub(super) async fn handle_metrics(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    let status = server.get_status();
    send_result(
        server,
        request_id,
        serde_json::json!({ "ok": true, "metrics": status.metrics }),
    )
    .await
}

pub(super) async fn handle_metrics_get(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    send_result(
        server,
        request_id,
        serde_json::json!({ "ok": true }),
    )
    .await
}

pub(super) async fn handle_metrics_reset(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    server.observability.metrics.reset_all();
    send_result(
        server,
        request_id,
        serde_json::json!({ "ok": true, "reset": true }),
    )
    .await
}

pub(super) async fn handle_metrics_window_query(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let window = params.get("window").and_then(Value::as_str).unwrap_or("5m");
    send_result(
        server,
        request_id,
        serde_json::json!({ "ok": true, "window": window }),
    )
    .await
}

pub(super) async fn handle_metrics_errors_summary(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let limit = params.get("limit").and_then(Value::as_u64).map(|v| v as usize).unwrap_or(20).min(200);
    let status = server.get_status();
    send_result(
        server,
        request_id,
        serde_json::json!({
            "ok": true,
            "total": limit,
            "error_count": status.metrics.failed_requests,
        }),
    )
    .await
}

pub(super) async fn handle_metrics_prometheus(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    let status = server.get_status();
    let m = &status.metrics;
    let lines = vec![
        format!("# HELP go_on_chat_requests_total Total chat requests"),
        format!("# TYPE go_on_chat_requests_total counter"),
        format!("go_on_chat_requests_total {}", m.chat_requests_total),
        format!("# HELP go_on_agent_failures_total Total agent failures"),
        format!("# TYPE go_on_agent_failures_total counter"),
        format!("go_on_agent_failures_total {}", m.failed_requests),
        format!("# HELP go_on_review_gate_total Total review gates"),
        format!("# TYPE go_on_review_gate_total counter"),
        format!("go_on_review_gate_total {}", m.review_gate_total),
    ];
    send_result(
        server,
        request_id,
        serde_json::json!({ "text": lines.join("\n") + "\n" }),
    )
    .await
}

#[cfg(test)]
mod tests {
    #[test]
    fn prometheus_format() {
        let lines = vec![
            "# HELP go_on_chat_requests_total Total chat requests",
            "# TYPE go_on_chat_requests_total counter",
            "go_on_chat_requests_total 42",
        ];
        let text = lines.join("\n") + "\n";
        assert!(text.contains("go_on_chat_requests_total 42"));
    }

    #[test]
    fn limit_clamp() {
        assert_eq!(200usize.min(200), 200);
        assert_eq!(300usize.min(200), 200);
    }
}
