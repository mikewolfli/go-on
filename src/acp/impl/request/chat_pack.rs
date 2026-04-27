use super::*;

/// Handle chat request
pub(super) async fn handle_chat(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
    trace: &RequestTraceContext,
) -> Result<()> {
    use crate::acp::r#impl::chat::handle_chat as chat_handler;

    match chat_handler(
        server,
        request_id.clone(),
        Some(params),
        None,
        Some(trace.clone()),
    )
    .await
    {
        Ok(()) => Ok(()),
        Err(err) => {
            let message = err.to_string();
            if message.to_ascii_lowercase().contains("rate limited") {
                send_error(server, request_id, -32029, message, None).await
            } else {
                send_error(server, request_id, -32603, message, None).await
            }
        }
    }
}

/// Handle phase request
pub(super) async fn handle_phase(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
    _trace: &RequestTraceContext,
) -> Result<()> {
    let rate_limiter = server
        .phase_rate_limiter
        .lock()
        .map(|guard| {
            json!({
                "tracked": guard.tracked_phases(),
                "buckets": guard.snapshot(),
            })
        })
        .unwrap_or_else(|_| json!({"tracked": 0, "buckets": {}}));

    let inflight = server
        .inflight_limiter
        .lock()
        .map(|guard| {
            let (global, phase) = guard.snapshot();
            json!({"global": global, "phase": phase})
        })
        .unwrap_or_else(|_| json!({"global": 0, "phase": {}}));

    send_result(
        server,
        request_id,
        json!({
            "rate_limiter": rate_limiter,
            "inflight": inflight,
        }),
    )
    .await
}

/// Handle primary/secondary summary
pub(super) async fn handle_primary_secondary_summary(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    let ledger = clone_artifact_ledger(server);
    let window = params
        .get("limit")
        .or_else(|| params.get("window"))
        .and_then(Value::as_u64)
        .map(|v| v as usize)
        .unwrap_or(20)
        .max(1);
    let bus = read_latest_artifact::<WorkflowLearningBusArtifact>(
        &ledger,
        "spec",
        "latest-learning.json",
    );
    let policy = read_latest_artifact::<PrimarySecondaryPolicyArtifact>(
        &ledger,
        "spec",
        "latest-primary-secondary-policy.json",
    );
    let failover = read_latest_artifact::<PrimarySecondaryFailoverArtifact>(
        &ledger,
        "spec",
        "latest-primary-secondary-failover.json",
    );

    let events = bus
        .as_ref()
        .map(|bus| {
            bus.events
                .iter()
                .rev()
                .take(window)
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let count = events.len().max(1);
    let avg_primary_stability = events
        .iter()
        .map(|item| item.primary_stability_score)
        .sum::<f64>()
        / count as f64;
    let avg_secondary_utilization = events
        .iter()
        .map(|item| item.secondary_utilization_rate)
        .sum::<f64>()
        / count as f64;
    let total_failovers = events
        .iter()
        .map(|item| item.failover_count as u64)
        .sum::<u64>();
    let mut root_causes = HashMap::new();
    for event in &events {
        if !event.failover_root_cause.is_empty() {
            *root_causes
                .entry(event.failover_root_cause.clone())
                .or_insert(0_u64) += 1;
        }
    }

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "summary": {
                "total_events": events.len(),
                "averages": {
                    "primary_stability_score": avg_primary_stability,
                    "secondary_utilization_rate": avg_secondary_utilization,
                },
                "totals": {
                    "failover_count": total_failovers,
                },
                "failover_root_causes": root_causes,
                "latest_policy": policy,
                "latest_failover": failover,
            }
        }),
    )
    .await
}

pub(super) fn parse_messages(params: &Value) -> Option<Vec<Message>> {
    if let Some(messages) = params.get("messages") {
        return serde_json::from_value(messages.clone()).ok();
    }
    if let Some(message) = params.get("message") {
        return serde_json::from_value(message.clone())
            .ok()
            .map(|message| vec![message]);
    }

    params
        .get("content")
        .and_then(Value::as_str)
        .map(|content| {
            vec![Message {
                role: params
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or("user")
                    .to_string(),
                content: content.to_string(),
            }]
        })
}

/// Send error response
pub(super) async fn send_error(
    server: &AcpServer,
    id: Option<Value>,
    code: i64,
    message: String,
    data: Option<Value>,
) -> Result<()> {
    mark_error_response(id.as_ref());
    let error_data =
        inject_platform_profiles_if_absent(data.unwrap_or_else(|| json!({})), "acp.error");
    let data = Some(error_data);
    let data = match take_pua_report(id.as_ref()) {
        Some(encoded) => Some(inject_pua_report_into_error_data(data, encoded)),
        None => data,
    };
    let data = with_error_contract_data(code, &message, data);
    crate::acp::r#impl::io::send_error(server, id, code, message, data).await
}

/// Send result response
pub(super) async fn send_result(server: &AcpServer, id: Option<Value>, result: Value) -> Result<()> {
    let method = DISPATCH_REQUEST_METHOD
        .try_with(|m| m.clone())
        .unwrap_or_default();
    let result = inject_platform_profiles_if_absent(result, &method);
    let result = match take_pua_report(id.as_ref()) {
        Some(encoded) => inject_pua_report_into_result(result, encoded),
        None => result,
    };
    crate::acp::r#impl::io::send_result(server, id, result).await
}
