use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};
use serde::Serialize;
use tauri::State;

use crate::state::AppState;
use std::cmp::Reverse;
use std::collections::HashMap;
use std::fs;
use std::time::Instant;

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AiUsageSnapshot {
    pub timestamp: String,
    pub requests_per_minute: f64,
    pub success_rate: f64,
    pub avg_latency_ms: f64,
    pub timeout_count: u64,
    pub rate_limit_count: u64,
    pub breaker_count: u64,
    pub upstream_failure_count: u64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct NameCount {
    pub name: String,
    pub count: u64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TrendPoint {
    pub second_bucket: u64,
    pub count: u64,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UsageHeatmap {
    pub window_seconds: u64,
    pub phase_top: Vec<NameCount>,
    pub agent_top: Vec<NameCount>,
    pub trend: Vec<TrendPoint>,
    pub confidence: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EndpointHealthStat {
    pub endpoint: String,
    pub total: u64,
    pub success: u64,
    pub failure: u64,
    pub success_rate: f64,
    pub avg_latency_ms: f64,
}

fn collect_token(line: &str, key: &str) -> Option<String> {
    let lower = line.to_lowercase();
    let target = format!("{key}=");
    let pos = lower.find(&target)?;
    let start = pos + target.len();
    let token = line[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || *c == '.')
        .collect::<String>();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

fn parse_line_timestamp_utc(line: &str) -> Option<DateTime<Utc>> {
    let token = line.split_whitespace().next()?;

    if let Ok(dt) = DateTime::parse_from_rfc3339(token) {
        return Some(dt.with_timezone(&Utc));
    }

    if let Ok(naive) = NaiveDateTime::parse_from_str(token, "%Y-%m-%dT%H:%M:%S") {
        return Local
            .from_local_datetime(&naive)
            .single()
            .map(|dt| dt.with_timezone(&Utc));
    }

    if let Ok(naive) = NaiveDateTime::parse_from_str(token, "%Y-%m-%d_%H:%M:%S") {
        return Local
            .from_local_datetime(&naive)
            .single()
            .map(|dt| dt.with_timezone(&Utc));
    }

    None
}

fn to_sorted_top(counter: HashMap<String, u64>, top_n: usize) -> Vec<NameCount> {
    let mut entries: Vec<NameCount> = counter
        .into_iter()
        .map(|(name, count)| NameCount { name, count })
        .collect();
    entries.sort_by_key(|x| (Reverse(x.count), x.name.clone()));
    entries.truncate(top_n);
    entries
}

#[tauri::command]
pub fn get_usage_heatmap(
    state: State<'_, AppState>,
    window_seconds: Option<u64>,
) -> Result<UsageHeatmap, String> {
    let mut inner = state
        .0
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;

    let seconds = window_seconds.unwrap_or(300).clamp(60, 900);
    let mut phase_count: HashMap<String, u64> = HashMap::new();
    let mut agent_count: HashMap<String, u64> = HashMap::new();
    let bucket_count = if seconds <= 60 {
        12
    } else if seconds <= 300 {
        15
    } else {
        18
    };
    let bucket_span = std::cmp::max(1, seconds.div_ceil(bucket_count));
    let mut buckets = vec![0u64; bucket_count as usize];
    let now = Instant::now();
    while let Some(front) = inner.usage_events.front() {
        if now.duration_since(front.at).as_secs() > 900 {
            let _ = inner.usage_events.pop_front();
        } else {
            break;
        }
    }

    for event in &inner.usage_events {
        let age = now.duration_since(event.at).as_secs();
        if age > seconds {
            continue;
        }

        let idx_from_now = age / bucket_span;
        let bucket_idx = bucket_count.saturating_sub(1 + idx_from_now);
        if let Some(slot) = buckets.get_mut(bucket_idx as usize) {
            *slot += 1;
        }

        if let Some(phase) = event.phase.as_ref() {
            *phase_count.entry(phase.clone()).or_insert(0) += 1;
        }
        if let Some(agent) = event.agent.as_ref() {
            *agent_count.entry(agent.clone()).or_insert(0) += 1;
        }
    }

    let has_events = !inner.usage_events.is_empty();
    let confidence = if has_events {
        "event-buffer".to_string()
    } else {
        "log-fallback".to_string()
    };

    let mut fallback_lines: Vec<String> = Vec::new();
    let max_fallback = (bucket_count as usize) * 20;

    if !has_events {
        let log_text = fs::read_to_string(&inner.config.log_path).unwrap_or_default();
        let now_utc = Utc::now();

        for line in log_text.lines().rev().take((seconds as usize) * 12) {
            let mut accepted = false;
            if let Some(ts) = parse_line_timestamp_utc(line) {
                let age = now_utc.signed_duration_since(ts).num_seconds();
                if (0..=seconds as i64).contains(&age) {
                    let idx_from_now = (age as u64) / bucket_span;
                    let bucket_idx = bucket_count.saturating_sub(1 + idx_from_now);
                    if let Some(slot) = buckets.get_mut(bucket_idx as usize) {
                        *slot += 1;
                    }
                    accepted = true;
                }
            }

            if accepted {
                if let Some(phase) = collect_token(line, "phase") {
                    *phase_count.entry(phase).or_insert(0) += 1;
                }
                if let Some(agent) = collect_token(line, "agent") {
                    *agent_count.entry(agent).or_insert(0) += 1;
                }
                continue;
            }

            if fallback_lines.len() < max_fallback {
                fallback_lines.push(line.to_string());
            }
        }

        if phase_count.is_empty() && agent_count.is_empty() {
            for line in fallback_lines.iter().rev() {
                if let Some(phase) = collect_token(line, "phase") {
                    *phase_count.entry(phase).or_insert(0) += 1;
                }
                if let Some(agent) = collect_token(line, "agent") {
                    *agent_count.entry(agent).or_insert(0) += 1;
                }
            }
        }
    }

    if buckets.iter().all(|x| *x == 0) && !fallback_lines.is_empty() {
        let total = fallback_lines.len() as u64;
        for (i, _) in fallback_lines.iter().rev().enumerate() {
            let pos = i as u64;
            let idx = (pos * bucket_count) / total;
            let bucket_idx = std::cmp::min(idx, bucket_count - 1) as usize;
            if let Some(slot) = buckets.get_mut(bucket_idx) {
                *slot += 1;
            }
        }
    }

    let trend = buckets
        .into_iter()
        .enumerate()
        .map(|(i, count)| TrendPoint {
            second_bucket: ((i as u64 + 1) * seconds) / bucket_count,
            count,
        })
        .collect::<Vec<_>>();

    Ok(UsageHeatmap {
        window_seconds: seconds,
        phase_top: to_sorted_top(phase_count, 8),
        agent_top: to_sorted_top(agent_count, 8),
        trend,
        confidence,
    })
}

#[tauri::command]
pub fn get_endpoint_health_stats(
    state: State<'_, AppState>,
) -> Result<Vec<EndpointHealthStat>, String> {
    let inner = state
        .0
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;

    let mut stats = inner
        .endpoint_health
        .iter()
        .map(|(endpoint, counter)| {
            let success_rate = if counter.total > 0 {
                (counter.success as f64 / counter.total as f64) * 100.0
            } else {
                0.0
            };
            EndpointHealthStat {
                endpoint: endpoint.clone(),
                total: counter.total,
                success: counter.success,
                failure: counter.failure,
                success_rate,
                avg_latency_ms: counter.avg_latency_ms,
            }
        })
        .collect::<Vec<_>>();

    stats.sort_by(|a, b| b.total.cmp(&a.total).then(a.endpoint.cmp(&b.endpoint)));
    Ok(stats)
}

#[tauri::command]
pub fn get_ai_usage_snapshot(state: State<'_, AppState>) -> Result<AiUsageSnapshot, String> {
    let mut inner = state
        .0
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;

    let now = Instant::now();
    while let Some(front) = inner.recent_request_instants.front() {
        if now.duration_since(*front).as_secs() > 60 {
            let _ = inner.recent_request_instants.pop_front();
        } else {
            break;
        }
    }

    let total = inner.counters.requests_total as f64;
    let success = inner.counters.requests_success as f64;
    let success_rate = if total > 0.0 {
        (success / total) * 100.0
    } else {
        0.0
    };

    Ok(AiUsageSnapshot {
        timestamp: Local::now().to_rfc3339(),
        requests_per_minute: inner.recent_request_instants.len() as f64,
        success_rate,
        avg_latency_ms: inner.counters.avg_latency_ms,
        timeout_count: inner.counters.timeout_count,
        rate_limit_count: inner.counters.rate_limit_count,
        breaker_count: inner.counters.breaker_count,
        upstream_failure_count: inner.counters.upstream_failure_count,
    })
}
