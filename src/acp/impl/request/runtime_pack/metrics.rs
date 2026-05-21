use super::super::*;
use crate::i18n::runtime::t;
use std::sync::{Mutex as StdMutex, OnceLock};

#[derive(Clone, Debug, Serialize)]
struct MetricWindowPoint {
    ts: i64,
    qps: f64,
    p95: f64,
    error_rate: f64,
    success_rate: f64,
}

static METRIC_WINDOW_HISTORY: OnceLock<StdMutex<Vec<MetricWindowPoint>>> = OnceLock::new();

fn metric_window_history() -> &'static StdMutex<Vec<MetricWindowPoint>> {
    METRIC_WINDOW_HISTORY.get_or_init(|| StdMutex::new(Vec::new()))
}

fn append_metric_window_sample(server: &AcpServer) -> MetricWindowPoint {
    let snapshot = server.observability.metrics.snapshot();
    let total = snapshot.total_requests.max(1) as f64;
    let point = MetricWindowPoint {
        ts: crate::acp::prelude::now_ts(),
        qps: (snapshot.total_requests as f64 / 60.0),
        p95: snapshot.avg_request_duration_ms,
        error_rate: (snapshot.failed_requests as f64 / total).clamp(0.0, 1.0),
        success_rate: ((snapshot
            .total_requests
            .saturating_sub(snapshot.failed_requests)) as f64
            / total)
            .clamp(0.0, 1.0),
    };

    if let Ok(mut history) = metric_window_history().lock() {
        history.push(point.clone());
        let cutoff = point.ts - 3600;
        history.retain(|item| item.ts >= cutoff);
    }

    point
}

fn percentile_value(mut values: Vec<f64>, percentile: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let clamped = percentile.clamp(0.0, 1.0);
    let index = ((values.len() - 1) as f64 * clamped).round() as usize;
    values[index.min(values.len() - 1)]
}

fn classify_error_group(event: &TraceEvent) -> String {
    let error_text = event.error.as_deref().unwrap_or_default().trim();
    if !error_text.is_empty() {
        let lowered = error_text.to_ascii_lowercase();
        for (needle, label) in [
            ("timeout", "timeout"),
            ("rate limit", "rate_limited"),
            ("unauthorized", "auth_error"),
            ("permission denied", "auth_error"),
            ("not found", "not_found"),
            ("invalid", "validation_error"),
            ("parse", "parse_error"),
            ("network", "network_error"),
        ] {
            if lowered.contains(needle) {
                return label.to_string();
            }
        }

        if let Some((code, _)) = error_text.split_once(':') {
            let code = code.trim();
            if !code.is_empty() && code.len() <= 48 && !code.contains(' ') {
                return code.to_ascii_lowercase();
            }
        }

        return lowered
            .split_whitespace()
            .take(4)
            .collect::<Vec<_>>()
            .join("_");
    }

    if !event.tool.as_deref().unwrap_or_default().trim().is_empty() {
        return format!("tool:{}", event.tool.as_deref().unwrap_or_default());
    }

    if event.event_type.starts_with("phase.") {
        return event.event_type.clone();
    }

    if !event.phase.trim().is_empty() {
        return event.phase.clone();
    }

    event.status.clone()
}

pub(super) async fn handle_metrics(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    let status = server.get_status();
    send_result(
        server,
        request_id,
        json!({
            "metrics": status.metrics,
            "timestamp": status.timestamp,
        }),
    )
    .await
}

pub(super) async fn handle_metrics_get(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    let snapshot = serde_json::to_value(server.observability.metrics.snapshot())?;
    let mut result = snapshot.clone();
    if let Value::Object(ref mut map) = result {
        map.insert("ok".to_string(), json!(true));
        map.insert("metrics".to_string(), snapshot);
    }
    send_result(server, request_id, result).await
}

pub(super) async fn handle_metrics_prometheus(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    let metrics = server.observability.metrics.snapshot();
    let gauges = build_runtime_gauge_snapshot(server);
    let breaker_snapshot = server
        .circuit_breakers
        .lock()
        .map(|guard| {
            guard
                .snapshots()
                .into_iter()
                .map(|item| {
                    (
                        item.name,
                        PrometheusCircuitBreakerSnapshot {
                            state: item.state,
                            consecutive_failures: item.failure_count as u64,
                        },
                    )
                })
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    let phase_limiter_snapshot = server
        .phase_rate_limiter
        .lock()
        .map(|guard| guard.snapshot())
        .unwrap_or_default();
    let inflight_snapshot = server
        .inflight_limiter
        .lock()
        .map(|guard| guard.snapshot())
        .unwrap_or_default();
    let lifecycle_snapshot = server
        .lifecycle_state
        .lock()
        .map(|guard| PrometheusLifecycleSnapshot {
            shutting_down: guard.shutdown_requested(),
        })
        .unwrap_or(PrometheusLifecycleSnapshot {
            shutting_down: false,
        });
    let maintenance_snapshot = server
        .maintenance_tracker
        .lock()
        .map(|guard| {
            let snapshot = guard.snapshot();
            PrometheusMaintenanceSnapshot {
                cycles_total: snapshot.cycles_total,
                running: snapshot.running,
            }
        })
        .unwrap_or(PrometheusMaintenanceSnapshot {
            cycles_total: 0,
            running: false,
        });
    let text = build_prometheus_metrics(
        &PrometheusMetricsSnapshot {
            chat_requests_total: metrics.chat_requests_total,
            cache_lookup_total: 0,
            cache_hit_total: 0,
            cache_store_total: 0,
            vector_search_total: metrics.vector_search_total,
            vector_hit_total: metrics.vector_hit_total,
            vector_store_total: metrics.vector_store_total,
            summary_read_total: metrics.summary_read_total,
            summary_hit_total: metrics.summary_hit_total,
            summary_store_total: metrics.summary_store_total,
            agent_failures_total: metrics.failed_requests,
            agent_timeout_failures_total: metrics.agent_timeout_failures_total,
            runtime_probe_timeout_total: metrics.runtime_probe_timeout_total,
            agent_panic_failures_total: 0,
            agent_other_failures_total: 0,
            review_gate_total: metrics.review_gate_total,
            review_gate_approved_total: metrics.review_gate_approved_total,
            review_gate_rejected_total: metrics.review_gate_rejected_total,
            review_gate_timeout_total: metrics.review_gate_timeout_total,
            review_gate_degraded_total: metrics.review_gate_degraded_total,
            review_gate_invalid_response_total: metrics.review_gate_invalid_response_total,
            lazy_blue5_doc_lookup_total: 0,
            lazy_blue5_doc_hit_total: 0,
            lazy_blue5_doc_reload_total: 0,
            lazy_app_config_lookup_total: 0,
            lazy_app_config_hit_total: 0,
            lazy_app_config_reload_total: 0,
            lazy_clarification_lookup_total: 0,
            lazy_clarification_hit_total: 0,
            lazy_clarification_reload_total: 0,
            chat_latency_count: metrics.chat_requests_total,
            chat_latency_sum_seconds: metrics.chat_latency_sum_ms / 1000.0,
            chat_latency_bucket_counts: metrics.chat_latency_bucket_counts,
            agent_latency_count: metrics.total_requests,
            agent_latency_sum_seconds: metrics.request_latency_sum_ms / 1000.0,
            agent_latency_bucket_counts: metrics.request_latency_bucket_counts,
            review_latency_count: metrics.review_gate_total,
            review_latency_sum_seconds: metrics.review_latency_sum_ms / 1000.0,
            review_latency_bucket_counts: metrics.review_latency_bucket_counts,
        },
        &gauges,
        &breaker_snapshot,
        &phase_limiter_snapshot,
        &inflight_snapshot,
        &lifecycle_snapshot,
        &maintenance_snapshot,
    );

    send_result(server, request_id, json!({ "text": text })).await
}

pub(super) async fn handle_metrics_reset(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    server.observability.metrics.reset_all();
    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "reset": true,
            "timestamp": crate::acp::prelude::now_ts(),
        }),
    )
    .await
}

pub(super) fn metrics_window_query_payload(server: &AcpServer, params: &Value) -> Value {
    let window = params.get("window").and_then(Value::as_str).unwrap_or("5m");
    let seconds = match window {
        "1m" => 60,
        "5m" => 300,
        "1h" => 3600,
        _ => 300,
    };

    let latest = append_metric_window_sample(server);
    let cutoff = latest.ts - seconds;
    let series = metric_window_history()
        .lock()
        .map(|history| {
            history
                .iter()
                .filter(|point| point.ts >= cutoff)
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let latency_samples = series.iter().map(|point| point.p95).collect::<Vec<_>>();
    let window_p95 = percentile_value(latency_samples, 0.95);

    json!({
        "ok": true,
        "window": window,
        "summary": {
            "samples": series.len(),
            "p95": window_p95,
            "window_seconds": seconds,
        },
        "series": series,
    })
}

pub(super) fn metrics_errors_summary_payload(server: &AcpServer, params: &Value) -> Value {
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(20)
        .min(200);

    let snapshot = server.observability.metrics.snapshot();
    let mut error_groups: HashMap<String, usize> = HashMap::new();
    let mut failed_events_count = 0usize;
    let mut sample_failures_ring = std::collections::VecDeque::with_capacity(limit);

    if let Ok(guard) = trace_events().lock() {
        for event in guard.iter() {
            if !event.status.eq_ignore_ascii_case("error") {
                continue;
            }
            failed_events_count += 1;
            let key = classify_error_group(event);
            *error_groups.entry(key.clone()).or_insert(0) += 1;

            if limit > 0 {
                sample_failures_ring.push_back(json!({
                    "timestamp": event.timestamp,
                    "event_type": event.event_type,
                    "task_id": event.task_id,
                    "phase": event.phase,
                    "status": event.status,
                    "duration_ms": event.duration_ms,
                    "error_code": key,
                    "error": event.error,
                }));
                if sample_failures_ring.len() > limit {
                    sample_failures_ring.pop_front();
                }
            }
        }
    }

    let mut grouped = error_groups.into_iter().collect::<Vec<_>>();
    grouped.sort_by_key(|right| std::cmp::Reverse(right.1));
    let grouped = grouped
        .into_iter()
        .map(|(error_type, count)| json!({"error_type": error_type, "count": count}))
        .collect::<Vec<_>>();

    let sample_failures = sample_failures_ring.into_iter().rev().collect::<Vec<_>>();

    json!({
        "ok": true,
        "window": params.get("window").and_then(Value::as_str).unwrap_or("5m"),
        "summary": {
            "failed_events": failed_events_count,
            "error_groups": grouped.len(),
        },
        "series": {
            "qps": snapshot.total_requests as f64 / 60.0,
            "p95": snapshot.avg_request_duration_ms,
            "error_rate": if snapshot.total_requests > 0 {
                snapshot.failed_requests as f64 / snapshot.total_requests as f64
            } else {
                0.0
            },
            "success_rate": if snapshot.total_requests > 0 {
                (snapshot.total_requests.saturating_sub(snapshot.failed_requests)) as f64
                    / snapshot.total_requests as f64
            } else {
                1.0
            },
        },
        "error_groups": grouped,
        "sample_failures": sample_failures,
    })
}

pub(super) async fn handle_metrics_window_query(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    send_result(
        server,
        request_id,
        metrics_window_query_payload(server, &params),
    )
    .await
}

pub(super) async fn handle_metrics_errors_summary(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    send_result(
        server,
        request_id,
        metrics_errors_summary_payload(server, &params),
    )
    .await
}

pub(super) async fn handle_debug_panel_get(
    server: &AcpServer,
    _params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    send_result(server, request_id, build_debug_panel_payload(server).await).await
}

pub(super) async fn build_debug_panel_payload(server: &AcpServer) -> Value {
    let state = server.conversation_state.lock().await;
    let conversation_count = state
        .checkpoints
        .iter()
        .map(|cp| cp.conversation_id.as_str())
        .collect::<std::collections::HashSet<_>>()
        .len();
    let checkpoint_count = state.checkpoints.len();

    json!({
        "ok": true,
        "panel": {
            "trace": {"stage_transitions": []},
            "selected_agents": [],
            "review_outcomes": [],
            "runtime_health": {"ok": true},
            "review_gate": {
                "total": server.observability.metrics.snapshot().review_gate_total,
            },
            "conversations": {
                "count": conversation_count,
                "checkpoints": checkpoint_count,
            }
        }
    })
}

pub(super) async fn handle_trace_get(
    server: &AcpServer,
    params: Value,
    request_id: Option<Value>,
) -> Result<()> {
    send_result(server, request_id, build_trace_payload(&params)).await
}

pub(super) fn build_trace_payload(params: &Value) -> Value {
    let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(100) as usize;

    let (trace_events_len, limited_trace_events) = match trace_events().lock() {
        Ok(guard) => {
            let total = guard.len();
            let start = total.saturating_sub(limit);
            let events = guard.iter().skip(start).cloned().collect::<Vec<_>>();
            (total, events)
        }
        Err(_) => (0, Vec::new()),
    };

    json!({
        "events": limited_trace_events,
        "total": trace_events_len,
        "limit": limit,
    })
}

pub(super) async fn handle_trace_metrics(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    send_result(server, request_id, trace_metrics_snapshot(server)).await
}

pub(super) async fn handle_shutdown(server: &AcpServer, request_id: Option<Value>) -> Result<()> {
    info!("{}", t("info.shutdown_requested"));
    server.begin_shutdown();
    server.shutdown_notify.notify_waiters();

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "shutdown": "initiated"
        }),
    )
    .await
}

pub(super) async fn handle_runtime_restart(
    server: &AcpServer,
    request_id: Option<Value>,
) -> Result<()> {
    info!("{}", t("info.restart_requested"));

    server.begin_shutdown();
    server.shutdown_notify.notify_waiters();

    send_result(
        server,
        request_id,
        json!({
            "ok": true,
            "restart": "initiated",
        }),
    )
    .await
}
