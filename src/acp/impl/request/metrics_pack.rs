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
    let status = server.get_status();
    let m = &status.metrics;
    send_result(
        server,
        request_id,
        serde_json::json!({
            "ok": true,
            "total_requests": m.total_requests,
            "active_requests": m.active_requests,
            "failed_requests": m.failed_requests,
            "chat_requests_total": m.chat_requests_total,
        }),
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
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .unwrap_or(20)
        .min(200);
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
        format!("# HELP acp_review_gate_timeout_total ACP review gate timeout total"),
        format!("# TYPE acp_review_gate_timeout_total counter"),
        format!(
            "acp_review_gate_timeout_total {}",
            m.review_gate_timeout_total
        ),
        format!("# HELP acp_review_gate_degraded_total ACP review gate degraded total"),
        format!("# TYPE acp_review_gate_degraded_total counter"),
        format!(
            "acp_review_gate_degraded_total {}",
            m.review_gate_degraded_total
        ),
        format!(
            "# HELP acp_review_gate_invalid_response_total ACP review gate invalid response total"
        ),
        format!("# TYPE acp_review_gate_invalid_response_total counter"),
        format!(
            "acp_review_gate_invalid_response_total {}",
            m.review_gate_invalid_response_total
        ),
        format!("# HELP acp_chat_latency_seconds_count ACP chat latency sample count"),
        format!("# TYPE acp_chat_latency_seconds_count counter"),
        format!(
            "acp_chat_latency_seconds_count {}",
            m.chat_requests_total.max(1)
        ),
        format!("# HELP acp_agent_latency_seconds_count ACP agent latency sample count"),
        format!("# TYPE acp_agent_latency_seconds_count counter"),
        format!(
            "acp_agent_latency_seconds_count {}",
            m.total_requests.max(1)
        ),
        format!("# HELP acp_review_latency_seconds_count ACP review latency sample count"),
        format!("# TYPE acp_review_latency_seconds_count counter"),
        format!(
            "acp_review_latency_seconds_count {}",
            m.review_gate_total.max(1)
        ),
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
