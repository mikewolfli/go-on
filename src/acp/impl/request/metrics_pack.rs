//! Metrics-related handlers extracted from runtime_pack.rs.
//!
//! Provides metric snapshot, reset, Prometheus exposition, window query,
//! and error summary handlers.

use super::*;

pub(super) async fn metrics_payload(server: &AcpServer) -> Result<Value> {
    let status = server.get_status();
    Ok(serde_json::json!({ "ok": true, "metrics": status.metrics }))
}

pub(super) async fn metrics_get_payload(server: &AcpServer) -> Result<Value> {
    let status = server.get_status();
    let m = &status.metrics;
    Ok(serde_json::json!({
        "ok": true,
        "total_requests": m.total_requests,
        "active_requests": m.active_requests,
        "failed_requests": m.failed_requests,
        "chat_requests_total": m.chat_requests_total,
    }))
}

pub(super) async fn metrics_reset_payload(server: &AcpServer) -> Result<Value> {
    server.observability.metrics.reset_all();
    Ok(serde_json::json!({ "ok": true, "reset": true }))
}

pub(super) async fn handle_metrics_prometheus(server: &AcpServer) -> Result<DispatchOutput> {
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
        format!("acp_chat_latency_seconds_count {}", m.chat_requests_total),
        format!("# HELP acp_agent_latency_seconds_count ACP agent latency sample count"),
        format!("# TYPE acp_agent_latency_seconds_count counter"),
        format!("acp_agent_latency_seconds_count {}", m.total_requests),
        format!("# HELP acp_review_latency_seconds_count ACP review latency sample count"),
        format!("# TYPE acp_review_latency_seconds_count counter"),
        format!("acp_review_latency_seconds_count {}", m.review_gate_total),
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
    Ok(DispatchOutput::text(lines.join("\n") + "\n"))
}
