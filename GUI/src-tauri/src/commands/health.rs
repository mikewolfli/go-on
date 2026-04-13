use std::time::Instant;

use reqwest::blocking::Client;
use serde::Serialize;

use tauri::State;

use crate::state::AppState;

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct HealthSnapshot {
    pub ok: bool,
    pub endpoint: String,
    pub response_code: Option<u16>,
    pub response_body: Option<String>,
    pub message: Option<String>,
}

#[tauri::command]
pub fn check_health(
    state: State<'_, AppState>,
    endpoint: Option<String>,
) -> Result<HealthSnapshot, String> {
    let endpoint = endpoint.unwrap_or_else(|| "http://127.0.0.1:8090/health".to_string());

    let start = Instant::now();
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(4))
        .build()
        .map_err(|e| e.to_string())?;

    let result = client.get(&endpoint).send();

    let mut inner = state
        .0
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;

    inner.counters.requests_total += 1;
    let now = Instant::now();
    inner.recent_request_instants.push_back(now);
    while let Some(front) = inner.recent_request_instants.front() {
        if now.duration_since(*front).as_secs() > 60 {
            let _ = inner.recent_request_instants.pop_front();
        } else {
            break;
        }
    }

    match result {
        Ok(resp) => {
            let code = resp.status().as_u16();
            let body = resp.text().unwrap_or_default();
            if code >= 200 && code < 300 {
                inner.counters.requests_success += 1;
            } else {
                inner.counters.upstream_failure_count += 1;
            }

            let elapsed = start.elapsed().as_millis() as f64;
            if inner.counters.requests_total == 1 {
                inner.counters.avg_latency_ms = elapsed;
            } else {
                inner.counters.avg_latency_ms =
                    (inner.counters.avg_latency_ms * 0.8) + (elapsed * 0.2);
            }

            let endpoint_counter = inner.endpoint_health.entry(endpoint.clone()).or_default();
            endpoint_counter.total += 1;
            endpoint_counter.success += 1;
            if endpoint_counter.total == 1 {
                endpoint_counter.avg_latency_ms = elapsed;
            } else {
                endpoint_counter.avg_latency_ms =
                    (endpoint_counter.avg_latency_ms * 0.8) + (elapsed * 0.2);
            }

            inner.usage_events.push_back(crate::state::UsageEvent {
                at: now,
                phase: None,
                agent: None,
            });
            while inner.usage_events.len() > 4000 {
                let _ = inner.usage_events.pop_front();
            }

            Ok(HealthSnapshot {
                ok: code >= 200 && code < 300,
                endpoint,
                response_code: Some(code),
                response_body: Some(body),
                message: None,
            })
        }
        Err(err) => {
            inner.counters.upstream_failure_count += 1;

            let endpoint_counter = inner.endpoint_health.entry(endpoint.clone()).or_default();
            endpoint_counter.total += 1;
            endpoint_counter.failure += 1;

            inner.usage_events.push_back(crate::state::UsageEvent {
                at: now,
                phase: None,
                agent: None,
            });
            while inner.usage_events.len() > 4000 {
                let _ = inner.usage_events.pop_front();
            }

            Ok(HealthSnapshot {
                ok: false,
                endpoint,
                response_code: None,
                response_body: None,
                message: Some(err.to_string()),
            })
        }
    }
}
