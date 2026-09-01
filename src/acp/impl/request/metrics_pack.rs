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
        "# HELP acp_review_gate_timeout_total ACP review gate timeout total".to_string(),
        "# TYPE acp_review_gate_timeout_total counter".to_string(),
        format!(
            "acp_review_gate_timeout_total {}",
            m.review_gate_timeout_total
        ),
        "# HELP acp_review_gate_degraded_total ACP review gate degraded total".to_string(),
        "# TYPE acp_review_gate_degraded_total counter".to_string(),
        format!(
            "acp_review_gate_degraded_total {}",
            m.review_gate_degraded_total
        ),
        "# HELP acp_review_gate_invalid_response_total ACP review gate invalid response total"
            .to_string(),
        "# TYPE acp_review_gate_invalid_response_total counter".to_string(),
        format!(
            "acp_review_gate_invalid_response_total {}",
            m.review_gate_invalid_response_total
        ),
        "# HELP acp_chat_latency_seconds_count ACP chat latency sample count".to_string(),
        "# TYPE acp_chat_latency_seconds_count counter".to_string(),
        format!("acp_chat_latency_seconds_count {}", m.chat_requests_total),
        "# HELP acp_agent_latency_seconds_count ACP agent latency sample count".to_string(),
        "# TYPE acp_agent_latency_seconds_count counter".to_string(),
        format!("acp_agent_latency_seconds_count {}", m.total_requests),
        "# HELP acp_review_latency_seconds_count ACP review latency sample count".to_string(),
        "# TYPE acp_review_latency_seconds_count counter".to_string(),
        format!("acp_review_latency_seconds_count {}", m.review_gate_total),
        "# HELP go_on_chat_requests_total Total chat requests".to_string(),
        "# TYPE go_on_chat_requests_total counter".to_string(),
        format!("go_on_chat_requests_total {}", m.chat_requests_total),
        "# HELP go_on_agent_failures_total Total agent failures".to_string(),
        "# TYPE go_on_agent_failures_total counter".to_string(),
        format!("go_on_agent_failures_total {}", m.failed_requests),
        "# HELP go_on_review_gate_total Total review gates".to_string(),
        "# TYPE go_on_review_gate_total counter".to_string(),
        format!("go_on_review_gate_total {}", m.review_gate_total),
    ];
    Ok(DispatchOutput::text(lines.join("\n") + "\n"))
}
