use std::sync::OnceLock;

use super::*;
use crate::i18n::runtime::{t, tf};
use crate::shared::secret_override::set_secret_override;

type CopilotModelsCacheEntry = Option<(u64, Vec<String>)>;
type CopilotModelsCache = tokio::sync::Mutex<CopilotModelsCacheEntry>;
static COPILOT_MODELS_CACHE: std::sync::OnceLock<CopilotModelsCache> = std::sync::OnceLock::new();

const COPILOT_TOKEN_URL: &str = "https://api.github.com/copilot_internal/v2/token";
const COPILOT_MODELS_URL: &str = "https://api.githubcopilot.com/models";
const COPILOT_MODELS_CACHE_TTL_SECS: u64 = 300;

/// Snapshot of the proxy environment variables that determine which
/// [`reqwest::Client`] configuration [`build_github_client`] produces. Used to
/// decide whether a cached client can be reused.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProxyEnvSnapshot {
    https_proxy: Option<String>,
    https_proxy_lower: Option<String>,
    all_proxy: Option<String>,
    all_proxy_lower: Option<String>,
}

impl ProxyEnvSnapshot {
    fn capture() -> Self {
        Self {
            https_proxy: std::env::var("HTTPS_PROXY").ok(),
            https_proxy_lower: std::env::var("https_proxy").ok(),
            all_proxy: std::env::var("ALL_PROXY").ok(),
            all_proxy_lower: std::env::var("all_proxy").ok(),
        }
    }
}

/// Last successfully built Copilot HTTP client, keyed on the proxy env
/// snapshot it was built from. Rebuilt only when the env vars change.
type GithubClientCacheEntry = (ProxyEnvSnapshot, reqwest::Client);
static GITHUB_CLIENT_CACHE: std::sync::OnceLock<std::sync::Mutex<Option<GithubClientCacheEntry>>> =
    std::sync::OnceLock::new();

/// Try to build a [`reqwest::Client`] with proxy autodetection.
///
/// Checks `HTTPS_PROXY` / `https_proxy` / `ALL_PROXY` / `all_proxy` environment
/// variables first.  If none are set, probes a list of well-known local proxy ports.
/// Falls back to a plain (direct) client if nothing works.
///
/// The auto-detect probe performs up to 7 serial TCP connects with a 100 ms
/// timeout each (~700 ms worst case) — far too expensive to repeat for every
/// request. The built client is cached keyed on the proxy env snapshot and
/// reused until the env vars change.
async fn build_github_client() -> reqwest::Client {
    let env_snapshot = ProxyEnvSnapshot::capture();
    let cache = GITHUB_CLIENT_CACHE.get_or_init(|| std::sync::Mutex::new(None));

    // Fast path: reuse the cached client when the proxy env is unchanged.
    if let Ok(guard) = cache.lock() {
        if let Some((cached_env, client)) = guard.as_ref() {
            if *cached_env == env_snapshot {
                return client.clone();
            }
        }
    }

    // Slow path: (re)build the client from scratch. The auto-detect probe runs
    // up to 7 serial TCP connects with a 100 ms timeout each (~700 ms worst
    // case), so it must not execute directly on a tokio worker — offload the
    // whole probe/selection to the blocking pool (logic unchanged).
    let client = tokio::task::spawn_blocking(build_github_client_uncached)
        .await
        .unwrap_or_else(|_| build_github_client_uncached());
    if let Ok(mut guard) = cache.lock() {
        // A concurrent caller may have built with the same env while we were
        // probing — prefer its result so the cache is not thrashed.
        if let Some((cached_env, cached_client)) = guard.as_ref() {
            if *cached_env == env_snapshot {
                return cached_client.clone();
            }
        }
        *guard = Some((env_snapshot, client.clone()));
    }
    client
}

/// The uncached probe/selection logic. See [`build_github_client`].
fn build_github_client_uncached() -> reqwest::Client {
    // 1. Check explicitly-configured env vars
    let proxy_env = std::env::var("HTTPS_PROXY")
        .or_else(|_| std::env::var("https_proxy"))
        .or_else(|_| std::env::var("ALL_PROXY"))
        .or_else(|_| std::env::var("all_proxy"));

    if let Ok(proxy_url) = proxy_env {
        if !proxy_url.is_empty() {
            if let Ok(proxy) = reqwest::Proxy::https(&proxy_url) {
                tracing::debug!("Using HTTPS_PROXY proxy: {proxy_url}");
                if let Ok(client) = reqwest::Client::builder().proxy(proxy).build() {
                    return client;
                }
            }
            // If the user set a proxy but it failed to parse, fall through to probing
            tracing::warn!("Failed to build proxy from env var {proxy_url}, trying auto-detect");
        }
    }

    // 2. Common local proxy ports (same list as gui/src/main.rs auto_detect_proxy)
    let common_proxies: &[&str] = &[
        "http://127.0.0.1:15732",
        "http://127.0.0.1:7890",
        "socks5://127.0.0.1:7890",
        "http://127.0.0.1:10809",
        "http://127.0.0.1:10808",
        "http://127.0.0.1:1080",
        "http://127.0.0.1:33210",
    ];

    for proxy_url in common_proxies {
        // Try a quick TCP connect first to see if anything is listening
        let addr = proxy_url
            .trim_start_matches("http://")
            .trim_start_matches("https://")
            .trim_start_matches("socks5://")
            .trim_start_matches("socks4://");
        if let Some(port_str) = addr.split(':').nth(1) {
            if let Ok(port) = port_str.parse::<u16>() {
                let socket_addr = match format!("127.0.0.1:{port}").parse() {
                    Ok(addr) => addr,
                    Err(_) => continue,
                };
                if std::net::TcpStream::connect_timeout(
                    &socket_addr,
                    std::time::Duration::from_millis(100),
                )
                .is_err()
                {
                    continue;
                }
                // Port open – try to build a reqwest client with this proxy
                // Build proxy – `proxy_url` is the `&str` pointer directly
                let proxy_url_str = *proxy_url;
                let proxy_result = if proxy_url_str.starts_with("socks5://") {
                    // For socks proxies, try using Proxy::all (requires "socks" feature)
                    reqwest::Proxy::all(proxy_url_str)
                } else {
                    reqwest::Proxy::https(proxy_url_str)
                };

                match proxy_result {
                    Ok(proxy) => match reqwest::Client::builder().proxy(proxy).build() {
                        Ok(client) => {
                            tracing::debug!("Using auto-detected proxy: {proxy_url}");
                            return client;
                        }
                        Err(e) => {
                            tracing::debug!(
                                "Proxy {proxy_url} port open but client build failed: {e}"
                            );
                        }
                    },
                    Err(e) => {
                        tracing::debug!("Proxy {proxy_url} port open but proxy parse failed: {e}");
                    }
                }
            }
        }
    }

    // 3. Fallback: plain direct client via shared singleton
    tracing::debug!("No proxy detected, using direct connection");
    crate::shared::http_client::http_client()
        .cloned()
        .unwrap_or_else(|_| {
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("reqwest Client build failed (TLS backend?)")
        })
}

fn copilot_models_cache() -> &'static CopilotModelsCache {
    COPILOT_MODELS_CACHE.get_or_init(|| tokio::sync::Mutex::new(None))
}

fn read_copilot_models_cache() -> Option<Vec<String>> {
    let guard = copilot_models_cache().try_lock().ok()?;
    let (fetched_at, models) = guard.as_ref()?.clone();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if now.saturating_sub(fetched_at) <= COPILOT_MODELS_CACHE_TTL_SECS {
        Some(models)
    } else {
        None
    }
}

async fn read_stale_copilot_models_cache() -> Option<Vec<String>> {
    let guard = copilot_models_cache().lock().await;
    guard.as_ref().map(|(_, models)| models.clone())
}

async fn store_copilot_models_cache(models: Vec<String>) {
    let mut guard = copilot_models_cache().lock().await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    *guard = Some((now, models));
}

async fn resolve_copilot_github_token() -> Option<String> {
    // Only check keyring, not environment variables.
    // The copilot.rs agent handles env vars via resolve_secret() with full secret
    // pooling and rotation. This function is for the ACP provider.list_models path
    // where we want consistent behavior: all API keys come from the system keyring.
    // Keyring I/O blocks on a D-Bus pipe — must run on the blocking pool.
    for account in ["github_copilot_token", "copilot_api_key"] {
        if let Some(value) =
            crate::shared::secret_override::get_keyring_cached_async("go-on", account).await
        {
            return Some(value);
        }
    }
    None
}

async fn resolve_copilot_models_dynamic() -> Vec<String> {
    if let Some(models) = read_copilot_models_cache() {
        return models;
    }

    let fallback = crate::agents::copilot::COPILOT_FALLBACK_MODEL_PRIORITY
        .iter()
        .map(|model| (*model).to_string())
        .collect::<Vec<_>>();

    let Some(github_token) = resolve_copilot_github_token().await else {
        return read_stale_copilot_models_cache().await.unwrap_or(fallback);
    };

    let client = build_github_client().await;
    let token_resp = match client
        .get(COPILOT_TOKEN_URL)
        .header("Authorization", format!("token {}", github_token))
        .header("Accept", "application/json")
        .header("User-Agent", "go-on/1.0")
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(_) => return read_stale_copilot_models_cache().await.unwrap_or(fallback),
    };

    if !token_resp.status().is_success() {
        return read_stale_copilot_models_cache().await.unwrap_or(fallback);
    }

    let token_body: Value = match token_resp.json().await {
        Ok(body) => body,
        Err(_) => return read_stale_copilot_models_cache().await.unwrap_or(fallback),
    };

    let Some(copilot_token) = token_body.get("token").and_then(Value::as_str) else {
        return read_stale_copilot_models_cache().await.unwrap_or(fallback);
    };

    let models_resp = match client
        .get(COPILOT_MODELS_URL)
        .header("Authorization", format!("Bearer {}", copilot_token))
        .header("Accept", "application/json")
        .header("User-Agent", "go-on/1.0")
        .header("Editor-Version", "vscode/1.90.0")
        .header("Editor-Plugin-Version", "copilot-chat/0.17.0")
        .header("Copilot-Integration-Id", "copilot-chat")
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(_) => return read_stale_copilot_models_cache().await.unwrap_or(fallback),
    };

    if !models_resp.status().is_success() {
        return read_stale_copilot_models_cache().await.unwrap_or(fallback);
    }

    let payload: Value = match models_resp.json().await {
        Ok(body) => body,
        Err(_) => return read_stale_copilot_models_cache().await.unwrap_or(fallback),
    };

    let ranked = crate::agents::copilot::CopilotAgent::extract_ranked_model_ids(&payload);
    if ranked.is_empty() {
        return read_stale_copilot_models_cache().await.unwrap_or(fallback);
    }

    store_copilot_models_cache(ranked.clone()).await;
    ranked
}

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
    // Estimate real p95 from latency histogram bucket counts
    let real_p95 = estimate_p95_from_buckets(&snapshot.request_latency_bucket_counts);
    let point = MetricWindowPoint {
        ts: crate::acp::prelude::now_ts(),
        qps: (snapshot.total_requests as f64 / 60.0),
        p95: real_p95,
        error_rate: (snapshot.failed_requests as f64 / total).clamp(0.0, 1.0),
        success_rate: ((snapshot
            .total_requests
            .saturating_sub(snapshot.failed_requests)) as f64
            / total)
            .clamp(0.0, 1.0),
    };

    let mut history = metric_window_history().lock().unwrap_or_else(|poisoned| {
        tracing::warn!("lock poisoned, recovering");
        poisoned.into_inner()
    });
    history.push(point.clone());
    let cutoff = point.ts - 3600;
    history.retain(|item| item.ts >= cutoff);

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

/// Estimate p95 latency from histogram bucket counts.
///
/// Canonical implementation lives in `observability::metrics_exporter`;
/// this is a thin alias so the request-pack call sites stay unchanged.
pub(super) fn estimate_p95_from_buckets(bucket_counts: &[u64; 10]) -> f64 {
    crate::observability::metrics_exporter::estimate_p95_from_buckets(bucket_counts)
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

    let guard = trace_events().lock().unwrap_or_else(|poisoned| {
        tracing::warn!("trace_events lock poisoned during error analysis – recovered");
        poisoned.into_inner()
    });
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
            "p95": estimate_p95_from_buckets(&snapshot.request_latency_bucket_counts),
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

/// Build the debug panel payload — single implementation owned by
/// `config_handlers`; this is a thin alias so the ACP tool and the
/// `debug.panel.get` RPC method share one code path.
pub(super) async fn build_debug_panel_payload(server: &AcpServer) -> Value {
    super::config_handlers::build_debug_panel_payload_impl(server).await
}

pub(super) fn action_check_payload(server: &AcpServer, params: Value) -> Result<Value> {
    let kind = params
        .get("kind")
        .and_then(Value::as_str)
        .and_then(ActionCheckKind::parse)
        .unwrap_or(ActionCheckKind::All);
    let report = run_action_check(&clone_artifact_ledger(server), kind)?;
    Ok(json!({"ok": report.ok, "report": report}))
}

pub(super) async fn handle_conversation_checkpoint_create(
    server: &AcpServer,
    params: Value,
) -> Result<DispatchOutput> {
    let Some(conversation_id) = params.get("conversation_id").and_then(Value::as_str) else {
        anyhow::bail!(t("error.conversation_id_required"));
    };

    if conversation_id.trim().is_empty() {
        anyhow::bail!(t("error.conversation_id_required"));
    }
    let branch_id = params
        .get("branch_id")
        .or_else(|| params.get("branch"))
        .and_then(Value::as_str)
        .unwrap_or("main");
    if branch_id.trim().is_empty() || branch_id.chars().any(char::is_whitespace) {
        anyhow::bail!(t("error.branch_id_invalid"));
    }
    let messages = match parse_messages(&params) {
        Some(messages) if !messages.is_empty() => messages,
        _ => {
            anyhow::bail!(t("error.messages_required"));
        }
    };

    let note = params
        .get("note")
        .and_then(Value::as_str)
        .map(str::to_string);
    let checkpoint =
        create_checkpoint_record(server, conversation_id, branch_id, messages, note, None).await;
    let ck_value = serde_json::to_value(&checkpoint).unwrap_or_default();

    Ok(DispatchOutput::created(ck_value))
}

pub(super) async fn handle_conversation_checkpoint_list(
    server: &AcpServer,
    params: Value,
) -> Result<DispatchOutput> {
    let Some(conversation_id) = params.get("conversation_id").and_then(Value::as_str) else {
        anyhow::bail!(t("error.conversation_id_required"));
    };
    let branch_id = params
        .get("branch_id")
        .or_else(|| params.get("branch"))
        .and_then(Value::as_str);
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .map(|v| v as usize);
    let checkpoints = list_checkpoint_records(server, conversation_id, branch_id, limit).await;

    Ok(DispatchOutput::ok(json!({
        "ok": true,
        "conversation_id": conversation_id,
        "count": checkpoints.len(),
        "checkpoints": checkpoints,
    })))
}

pub(super) async fn handle_conversation_rollback(
    server: &AcpServer,
    params: Value,
) -> Result<DispatchOutput> {
    let Some(conversation_id) = params.get("conversation_id").and_then(Value::as_str) else {
        anyhow::bail!(t("error.conversation_id_required"));
    };
    let Some(checkpoint_id) = params.get("checkpoint_id").and_then(Value::as_str) else {
        anyhow::bail!(t("error.checkpoint_id_required"));
    };

    let branch_id = params
        .get("branch_id")
        .or_else(|| params.get("branch"))
        .and_then(Value::as_str)
        .unwrap_or("main");
    let checkpoint = match find_checkpoint(server, conversation_id, checkpoint_id).await {
        Some(checkpoint) => checkpoint,
        None => {
            anyhow::bail!(tf("error.checkpoint_not_found", &[("id", checkpoint_id)]));
        }
    };
    let previous_head = get_branch_head_id(server, conversation_id, branch_id).await;
    let mut rollback = create_checkpoint_record(
        server,
        conversation_id,
        branch_id,
        checkpoint.messages.clone(),
        Some(format!("rollback:{}", checkpoint_id)),
        Some(checkpoint_id.to_string()),
    )
    .await;
    let metacognitive_loop = crate::acp::r#impl::request::persist_checkpoint_metacognitive_loop(
        server,
        conversation_id,
        branch_id,
        &rollback.checkpoint_id,
        checkpoint.metacognitive_loop.clone().unwrap_or_else(|| {
            json!({
                "active": true,
                "schema_version": "blue25-metacognitive-loop-v1",
                "last_reflection": format!("rollback:{}", checkpoint_id),
                "reflection_trigger": "rollback_restore",
            })
        }),
    )
    .await;
    rollback.metacognitive_loop = Some(metacognitive_loop.clone());

    Ok(DispatchOutput::ok(json!({
        "ok": true,
        "conversation_id": conversation_id,
        "branch_id": branch_id,
        "checkpoint": rollback,
        "metacognitive_loop": metacognitive_loop,
        "previous_head": previous_head,
        "current_head": rollback.checkpoint_id,
    })))
}

pub(super) async fn handle_conversation_checkpoint_prune(
    server: &AcpServer,
    params: Value,
) -> Result<DispatchOutput> {
    let Some(conversation_id) = params.get("conversation_id").and_then(Value::as_str) else {
        anyhow::bail!(t("error.conversation_id_required"));
    };
    let keep = params.get("keep").and_then(Value::as_u64).unwrap_or(1) as usize;
    if keep == 0 {
        anyhow::bail!(t("error.keep_must_be_ge_1"));
    }
    let branch_id = params
        .get("branch_id")
        .or_else(|| params.get("branch"))
        .and_then(Value::as_str)
        .unwrap_or("main");
    let (removed, repaired_heads, dropped_heads) =
        prune_checkpoints(server, conversation_id, branch_id, keep).await;

    Ok(DispatchOutput::deleted(json!({
        "conversation_id": conversation_id,
        "removed": removed,
        "repaired_heads": repaired_heads,
        "dropped_heads": dropped_heads,
    })))
}

pub(super) async fn autotune_status_payload(server: &AcpServer) -> Result<Value> {
    let autotune_state = if let Some(autotune) = server.cache_deps.autotune.as_ref() {
        let lock = autotune.lock().await;
        Some(lock.clone())
    } else {
        None
    };

    let autotune_config = server.cache_deps.autotune_config.as_ref().cloned();
    let enabled = autotune_config
        .as_ref()
        .map(|cfg| cfg.enabled)
        .unwrap_or(false);

    Ok(json!({
        "enabled": enabled,
        "state": autotune_state,
        "autotune": {
            "enabled": enabled,
            "state": autotune_state,
        },
    }))
}

pub(super) async fn autotune_get_payload(server: &AcpServer) -> Result<Value> {
    let Some(autotune) = server.cache_deps.autotune.as_ref() else {
        return Ok(json!({
            "enabled": false,
            "autotune": null,
            "params": null,
        }));
    };

    let state = autotune.lock().await;
    let snap = state.snapshot();
    let mut result = snap.clone();
    if let Value::Object(ref mut map) = result {
        map.insert("enabled".to_string(), json!(true));
        map.insert("autotune".to_string(), snap.clone());
        map.insert("params".to_string(), snap);
    }
    Ok(result)
}

pub(super) fn selector_status_payload(server: &AcpServer) -> Result<Value> {
    let snapshot = server
        .model_deps
        .adaptive_model_selector
        .lock()
        .map(|selector| selector.snapshot())
        .unwrap_or_default();

    Ok(json!({ "selector": snapshot }))
}

pub(super) fn hardness_status_payload(_server: &AcpServer, params: Value) -> Result<Value> {
    let task = params
        .get("task")
        .and_then(Value::as_str)
        .or_else(|| params.get("objective").and_then(Value::as_str))
        .unwrap_or("");
    let hardness = summarize_hardness(task, &params);

    Ok(json!({
        "ok": true,
        "hardness": hardness,
        "routing": {
            "mode": hardness.budget.recommended_mode,
            "parallelism_cap": hardness.budget.parallelism_cap,
            "timeout_seconds": hardness.budget.timeout_seconds,
            "required_reviews": hardness.budget.required_reviews,
        },
    }))
}

pub(super) fn error_contract_payload(_server: &AcpServer) -> Result<Value> {
    Ok(json!({
    "contract": {
            "version": "x8-error-contract-v1",
            "kinds": [
                {
                    "kind": "InvalidParams",
                    "codes": [-32602],
                    "retry": {"retryable": false, "strategy": "none", "max_retries": 0}
                },
                {
                    "kind": "MethodNotFound",
                    "codes": [-32601],
                    "retry": {"retryable": false, "strategy": "none", "max_retries": 0}
                },
                {
                    "kind": "AuthRequired",
                    "codes": [-32001],
                    "retry": {"retryable": false, "strategy": "none", "max_retries": 0}
                },
                {
                    "kind": "PuaViolation",
                    "codes": [-32003],
                    "retry": {"retryable": false, "strategy": "none", "max_retries": 0}
                },
                {
                    "kind": "ConsultationBlocked",
                    "codes": [-32007],
                    "retry": {"retryable": false, "strategy": "none", "max_retries": 0}
                },
                {
                    "kind": "ServerError",
                    "codes": [-32000],
                    "retry": {"retryable": true, "strategy": "exponential_backoff", "base_delay_ms": 500, "max_delay_ms": 10000, "max_retries": 3}
                },
                {
                    "kind": "RateLimited",
                    "codes": [-32029],
                    "retry": {"retryable": true, "strategy": "exponential_backoff", "base_delay_ms": 500, "max_delay_ms": 10000, "max_retries": 3}
                },
                {
                    "kind": "UpstreamTimeout",
                    "codes": [-32603],
                    "retry": {"retryable": true, "strategy": "exponential_backoff", "base_delay_ms": 500, "max_delay_ms": 10000, "max_retries": 3}
                },
                {
                    "kind": "BudgetExceeded",
                    "codes": [-32603],
                    "retry": {"retryable": false, "strategy": "none", "max_retries": 0}
                },
                {
                    "kind": "SandboxBlocked",
                    "codes": [-32603],
                    "retry": {"retryable": false, "strategy": "none", "max_retries": 0}
                },
                {
                    "kind": "InternalError",
                    "codes": [-32603],
                    "retry": {"retryable": false, "strategy": "none", "max_retries": 0}
                }
            ],
            "compatibility": {
                "request_error_context_prefix": "acp.handle_request.dispatch"
            }
        }
    }))
}

pub(super) fn cost_status_payload(server: &AcpServer, params: Value) -> Result<Value> {
    let task = params
        .get("task")
        .and_then(Value::as_str)
        .or_else(|| params.get("objective").and_then(Value::as_str))
        .unwrap_or("");
    let hardness = summarize_hardness(task, &params);
    let cost = summarize_token_cost_governance(
        task,
        &params,
        hardness,
        &server.observability.metrics.snapshot(),
    );

    Ok(json!({ "ok": true, "cost": cost }))
}

pub(super) async fn autotune_reset_payload(server: &AcpServer) -> Result<Value> {
    let (Some(autotune), Some(config)) = (
        server.cache_deps.autotune.as_ref(),
        server.cache_deps.autotune_config.as_ref(),
    ) else {
        return Ok(json!({
            "ok": true,
            "autotune": "disabled",
            "reset": false,
            "enabled": false,
        }));
    };

    let mut lock = autotune.lock().await;
    let before = lock.snapshot();
    *lock = AutoTuneState::new(config);
    let after = lock.snapshot();

    let mut persisted = false;
    let mut warning = None::<String>;
    if let Some(path) = &server.cache_deps.autotune_state_path {
        let path = path.clone();
        // Snapshot the state after reset so we can save outside the lock.
        let state_snapshot = lock.clone();
        // Drop the mutex guard before spawn_blocking.
        drop(lock);
        match tokio::task::spawn_blocking(move || state_snapshot.save(&path))
            .await
            .expect("spawn_blocking for autotune save panicked")
        {
            Ok(()) => persisted = true,
            Err(err) => {
                warning = Some(tf(
                    "warning.failed_save_autotune",
                    &[("error", &format!("{}", err))],
                ));
            }
        }
    }

    Ok(json!({
        "ok": true,
        "autotune": "reset",
        "reset": true,
        "enabled": true,
        "persisted": persisted,
        "state_before": before,
        "state_after": after,
        "warning": warning,
    }))
}

fn provider_models_for(server: &AcpServer, provider: &str) -> Vec<crate::agent::ModelInfo> {
    server
        .model_deps
        .agent_registry
        .as_ref()
        .map(|registry| {
            registry
                .models()
                .into_iter()
                .find(|(name, _, _)| name == provider)
                .map(|(_, _, models)| models)
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

pub(super) async fn provider_test_connection_payload(
    server: &AcpServer,
    params: &Value,
) -> Result<Value> {
    let started = Instant::now();
    let provider = params
        .get("provider")
        .or_else(|| params.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default();

    if provider.trim().is_empty() {
        anyhow::bail!("provider is required");
    }

    let models = provider_models_for(server, provider);
    let account = format!("{}_api_key", provider);
    // Keyring I/O blocks on a D-Bus pipe — must run on the blocking pool.
    let keyring_has_key =
        crate::shared::secret_override::get_keyring_cached_async("go-on", &account)
            .await
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false);

    let mut api_ref_has_key = false;
    let mut secret_ref_has_key = false;
    let mut secret_ref_required = false;
    // Use the already-loaded AppConfig held by the FlowManager instead of
    // re-loading + re-parsing the config file from disk on every request
    // (synchronous file I/O on the async worker).
    if let Some(cfg) = server
        .model_deps
        .flow_manager
        .as_ref()
        .map(|fm| fm.config())
    {
        for agent in cfg.agents().values() {
            if agent.agent_type.eq_ignore_ascii_case(provider) {
                if let Some(secret_ref) = agent.api_key_env.as_deref() {
                    if crate::agents::agent::inspect_secret_pool(secret_ref, secret_ref).is_ok() {
                        api_ref_has_key = true;
                    }
                }
                if let Some(secret_ref) = agent.secret_key_env.as_deref() {
                    secret_ref_required = true;
                    if crate::agents::agent::inspect_secret_pool(secret_ref, secret_ref).is_ok() {
                        secret_ref_has_key = true;
                    }
                }
                break;
            }
        }
    }

    let default_env_name = if provider.eq_ignore_ascii_case("copilot") {
        "GITHUB_COPILOT_TOKEN".to_string()
    } else if provider.eq_ignore_ascii_case("replicate") {
        "REPLICATE_API_TOKEN".to_string()
    } else {
        format!("{}_API_KEY", provider.to_ascii_uppercase())
    };
    let env_has_key = std::env::var(&default_env_name)
        .ok()
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);

    let api_configured = keyring_has_key || api_ref_has_key || env_has_key;
    let key_configured = if secret_ref_required {
        api_configured && secret_ref_has_key
    } else {
        api_configured
    };
    // For Copilot: if the key is present but models list is empty (e.g. first
    // restart before registry rebuild), treat provider as ready anyway.
    let ok = key_configured && (!models.is_empty() || provider.eq_ignore_ascii_case("copilot"));
    let message = if ok {
        "provider configuration is ready"
    } else if secret_ref_required && !secret_ref_has_key {
        "provider secret key is not configured"
    } else if models.is_empty() {
        "provider has no available models"
    } else {
        "provider api key is not configured"
    };

    Ok(json!({
        "ok": ok,
        "provider": provider,
        "latency_ms": started.elapsed().as_millis() as u64,
        "model_count": models.len(),
        "key_configured": key_configured,
        "secret_key_required": secret_ref_required,
        "keyring_has_key": keyring_has_key,
        "api_ref_has_key": api_ref_has_key,
        "secret_ref_has_key": secret_ref_has_key,
        "env_has_key": env_has_key,
        "checked_env": default_env_name,
        "message": message,
        "error_code": if ok { Value::Null } else { json!("provider_not_ready") },
        "error_message": if ok { Value::Null } else { json!(message) },
    }))
}

pub(super) fn provider_test_completion_payload(
    server: &AcpServer,
    params: &Value,
) -> Result<Value> {
    let started = Instant::now();
    let provider = params
        .get("provider")
        .or_else(|| params.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default();

    if provider.trim().is_empty() {
        anyhow::bail!("provider is required");
    }

    let models = provider_models_for(server, provider);
    let selected_model = params
        .get("model")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            models
                .iter()
                .find(|model| model.is_default)
                .map(|model| model.id.clone())
        })
        .or_else(|| models.first().map(|model| model.id.clone()));

    let ok = selected_model.is_some();
    let message = if ok {
        "provider completion route resolved"
    } else {
        "provider has no available model for completion"
    };

    Ok(json!({
        "ok": ok,
        "provider": provider,
        "model": selected_model,
        "latency_ms": started.elapsed().as_millis() as u64,
        "error_code": if ok { Value::Null } else { json!("model_not_found") },
        "error_message": if ok { Value::Null } else { json!(message) },
        "preview": if ok {
            json!({
                "type": "route_preview",
                "note": "request routing validated; execution can use this provider/model"
            })
        } else {
            Value::Null
        },
    }))
}

pub(super) fn provider_capabilities_payload(server: &AcpServer, params: &Value) -> Result<Value> {
    let provider = params
        .get("provider")
        .or_else(|| params.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default();

    if provider.trim().is_empty() {
        anyhow::bail!("provider is required");
    }

    let models = provider_models_for(server, provider)
        .into_iter()
        .map(|model| {
            let supports_tool_calling = model.capabilities.iter().any(|cap| {
                cap.eq_ignore_ascii_case("function_calling")
                    || cap.eq_ignore_ascii_case("tool_calling")
            });
            let supports_vision = model
                .capabilities
                .iter()
                .any(|cap| cap.eq_ignore_ascii_case("vision"));
            let cost_tier = match model.context_window {
                Some(window) if window >= 128_000 => "high",
                Some(window) if window >= 32_000 => "standard",
                Some(_) => "economy",
                None => "unknown",
            };
            json!({
                "id": model.id,
                "name": model.name,
                "description": model.description,
                "is_default": model.is_default,
                "context_window": model.context_window,
                "capabilities": model.capabilities,
                "tool_calling": supports_tool_calling,
                "vision": supports_vision,
                "rate_limit": {
                    "rpm": server.runtime_config.entry_rate_limit_rpm,
                    "burst": server.runtime_config.entry_rate_limit_burst,
                },
                "cost_tier": cost_tier,
            })
        })
        .collect::<Vec<_>>();

    Ok(json!({
        "ok": !models.is_empty(),
        "provider": provider,
        "capabilities": {
            "models": models,
        },
    }))
}

/// Handle `provider.catalog` RPC — returns the full built-in provider catalog
/// with all spec metadata (URL, model, api_key_env, capabilities, etc.)
/// so GUI and VS Code extension can avoid hardcoding provider data.
pub(super) fn provider_catalog_payload(_server: &AcpServer) -> Result<Value> {
    let specs = crate::core::config::provider_specs();
    let catalog: Vec<Value> = specs
        .iter()
        .map(|spec| {
            json!({
                "name": spec.name,
                "type": spec.agent_type,
                "url": spec.url,
                "chat_path": spec.chat_path,
                "model": spec.model,
                "api_key_env": spec.api_key_env,
                "secret_key_env": spec.secret_key_env,
                "anthropic_version": spec.anthropic_version,
                "max_tokens": spec.max_tokens,
                "supports_system": spec.supports_system,
                "supports_vision": spec.supports_vision,
            })
        })
        .collect();

    Ok(json!({
        "ok": true,
        "catalog": catalog,
        "total": catalog.len(),
    }))
}

pub(super) async fn provider_list_models_payload(
    server: &AcpServer,
    params: Value,
) -> Result<Value> {
    let provider = params
        .get("provider")
        .or_else(|| params.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();

    if provider.is_empty() {
        anyhow::bail!("provider is required");
    }

    let static_models = provider_models_for(server, &provider);
    let mut model_order = if provider.eq_ignore_ascii_case("copilot") {
        resolve_copilot_models_dynamic().await
    } else {
        Vec::new()
    };

    for model in &static_models {
        if !model_order.iter().any(|id| id == &model.id) {
            model_order.push(model.id.clone());
        }
    }

    let default_model = static_models
        .iter()
        .find(|model| model.is_default)
        .map(|model| model.id.clone())
        .or_else(|| model_order.first().cloned());

    let models = model_order
        .iter()
        .map(|id| {
            if let Some(info) = static_models.iter().find(|model| model.id == *id) {
                json!({
                    "id": info.id,
                    "name": info.name,
                    "description": info.description,
                    "is_default": info.is_default,
                    "capabilities": info.capabilities,
                    "context_window": info.context_window,
                })
            } else {
                json!({
                    "id": id,
                    "name": id,
                    "description": Value::Null,
                    "is_default": false,
                    "capabilities": [],
                    "context_window": Value::Null,
                })
            }
        })
        .collect::<Vec<_>>();

    Ok(json!({
        "ok": true,
        "provider": provider,
        "default_model": default_model,
        "model_ids": model_order,
        "models": models,
        "source": if provider.eq_ignore_ascii_case("copilot") { "copilot_models" } else { "registry" },
    }))
}

/// Configure a keychain item's ACL so ANY process (not just the creator)
/// can read the password without triggering the macOS permission dialog.
/// This is essential for the backend (a headless child process) to access
/// API keys stored in the login keychain.
///
/// Matches by service name (`-d "go-on"`) because the `keyring` crate stores
/// the service as "go-on" but does NOT set a custom keychain "description" field.
/// Using `-D` (description) would therefore be a silent no-op.
#[cfg(target_os = "macos")]
fn ensure_keyring_item_accessible(account: &str) {
    use std::process::Command;
    let _ = Command::new("security")
        .args([
            "set-key-partition-list",
            "-S",
            "apple:default,apple:toolbar,apple:unknown,apple:keychain:basic",
            "-k",
            "",
            "-d",
            "go-on",
            "-a",
            account,
            "login.keychain",
        ])
        .output();
}

#[cfg(not(target_os = "macos"))]
fn ensure_keyring_item_accessible(_account: &str) {
    // Keychain ACL partitioning is macOS-specific (security set-key-partition-list).
    // On Linux/Windows, the keyring crate handles access control natively.
    tracing::debug!("ensure_keyring_item_accessible: no-op on non-macOS platform");
}

/// Handle provider configuration request from GUI or other clients.
/// Stores the provider config to system keyring.
pub(super) async fn handle_provider_configure(
    _server: &AcpServer,
    params: Value,
) -> Result<DispatchOutput> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let api_key = params.get("api_key").and_then(Value::as_str).unwrap_or("");
    let model = params
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("auto");
    let secret_key = params
        .get("secret_key")
        .and_then(Value::as_str)
        .unwrap_or("");

    info!(
        "{}",
        tf(
            "Provider configured: name={}, model={}, has_secret_key={}",
            &[
                ("name", name),
                ("model", model),
                (
                    "has_secret_key",
                    if secret_key.is_empty() { "no" } else { "yes" }
                )
            ]
        )
    );

    // ── Persist API key to system keyring ──────────────────────────
    if !api_key.is_empty() {
        let account = format!("{}_api_key", name);
        // Async wrapper: keyring set_password performs blocking I/O (D-Bus /
        // Keychain) and must not run on a tokio worker.
        match crate::shared::secret_override::set_keyring_async("go-on", &account, api_key).await {
            Ok(()) => ensure_keyring_item_accessible(&account),
            Err(e) => tracing::warn!("failed to save API key for '{}' to keyring: {}", name, e),
        }

        // ── Copilot needs additional secret overrides + keyring entries ──
        if name == "copilot" {
            // Set secret overrides that CopilotAgent reads (thread-safe alternative
            // to std::env::set_var, which is UB in multi-threaded programs).
            set_secret_override("GITHUB_TOKEN", api_key);
            set_secret_override("GITHUB_COPILOT_TOKEN", api_key);
            tracing::info!(
                "Set GITHUB_TOKEN and GITHUB_COPILOT_TOKEN secret overrides for copilot"
            );
            // The built-in provider spec uses api_key_env="GITHUB_COPILOT_TOKEN",
            // which setup.rs maps to keyring://go-on/github_copilot_token.
            // Without this entry, CopilotAgent fails with "keyring lookup failed".
            match crate::shared::secret_override::set_keyring_async(
                "go-on",
                "github_copilot_token",
                api_key,
            )
            .await
            {
                Ok(()) => ensure_keyring_item_accessible("github_copilot_token"),
                Err(e) => {
                    tracing::warn!(
                        "failed to save Copilot token to keyring account github_copilot_token: {}",
                        e
                    )
                }
            }
        }
    }

    // ── Persist secret_key to system keyring (wenxin dual-auth) ────
    if !secret_key.is_empty() {
        let account = format!("{}_secret_key", name);
        match crate::shared::secret_override::set_keyring_async("go-on", &account, secret_key).await
        {
            Ok(()) => ensure_keyring_item_accessible(&account),
            Err(e) => tracing::warn!("failed to save secret key for '{}' to keyring: {}", name, e),
        }
    }

    Ok(DispatchOutput::ok(json!({
        "ok": true,
        "provider": name,
        "model": model,
    })))
}

/// Handle GitHub Copilot OAuth Device Code flow initiation.
/// Returns a `device_code`, `user_code`, and `verification_uri` (like GitHub's API).
/// The caller (GUI) should display the URI + user_code and then poll
/// `provider.copilot_device_code_poll` with the returned `device_code`.
pub(super) async fn handle_copilot_device_code_request(
    _server: &AcpServer,
    params: Value,
) -> Result<DispatchOutput> {
    info!("GitHub Copilot Device Code flow requested");

    let client_id = params
        .get("client_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("01ab8ac9400c4e429b23");
    let device_code_url = "https://github.com/login/device/code";
    let scope = params
        .get("scope")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("read:user");

    // Build reqwest client with proxy support
    let client = build_github_client().await;

    let device_params = [("client_id", client_id), ("scope", scope)];

    match client
        .post(device_code_url)
        .header("Accept", "application/json")
        .form(&device_params)
        .send()
        .await
    {
        Ok(resp) => {
            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                let err_msg = format!("GitHub device code request failed ({status}): {body}");
                tracing::error!("{}", err_msg);
                anyhow::bail!(err_msg);
            }
            match resp.json::<Value>().await {
                Ok(body) => {
                    let device_code = body["device_code"].as_str().unwrap_or("").to_string();
                    let user_code = body["user_code"].as_str().unwrap_or("").to_string();
                    let verification_uri = body["verification_uri"]
                        .as_str()
                        .unwrap_or("https://github.com/login/device")
                        .to_string();
                    let interval = body["interval"].as_u64().unwrap_or(5);

                    info!(
                        "Copilot Device Code issued: user_code={}, uri={}",
                        user_code, verification_uri
                    );

                    Ok(DispatchOutput::ok(json!({
                        "ok": true,
                        "device_code": device_code,
                        "user_code": user_code,
                        "verification_uri": verification_uri,
                        "interval": interval,
                    })))
                }
                Err(e) => {
                    let err_msg = format!("Failed to parse GitHub device code response: {}", e);
                    tracing::error!("{}", err_msg);
                    anyhow::bail!(err_msg)
                }
            }
        }
        Err(e) => {
            let err_msg = format!("Failed to connect to GitHub device code endpoint: {}", e);
            tracing::error!("{}", err_msg);
            anyhow::bail!(err_msg)
        }
    }
}

/// Poll GitHub for the access token after device code authorization.
/// The GUI should call this repeatedly (every `interval` seconds) until
/// either a token is returned or the device_code expires.
pub(super) async fn handle_copilot_device_code_poll(
    _server: &AcpServer,
    params: Value,
) -> Result<DispatchOutput> {
    let device_code = params
        .get("device_code")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    if device_code.is_empty() {
        anyhow::bail!("Missing 'device_code' parameter");
    }

    info!(
        "Copilot Device Code poll: device_code={}",
        &device_code[..8.min(device_code.len())]
    );

    let client_id = params
        .get("client_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("01ab8ac9400c4e429b23");
    let token_url = "https://github.com/login/oauth/access_token";

    let client = build_github_client().await;

    let poll_params = [
        ("client_id", client_id),
        ("device_code", &device_code),
        ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
    ];

    match client
        .post(token_url)
        .header("Accept", "application/json")
        .form(&poll_params)
        .send()
        .await
    {
        Ok(resp) => {
            match resp.json::<Value>().await {
                Ok(body) => {
                    // Check for error responses
                    if let Some(error) = body.get("error").and_then(Value::as_str) {
                        match error {
                            "authorization_pending" => {
                                // User hasn't authorized yet — keep polling
                                return Ok(DispatchOutput::ok(json!({
                                    "ok": true,
                                    "status": "pending",
                                    "error": error,
                                })));
                            }
                            "slow_down" => {
                                // Poll too fast — slow down
                                return Ok(DispatchOutput::ok(json!({
                                    "ok": true,
                                    "status": "slow_down",
                                    "error": error,
                                })));
                            }
                            "expired_token" => {
                                // Device code expired
                                return Ok(DispatchOutput::ok(json!({
                                    "ok": true,
                                    "status": "expired",
                                    "error": error,
                                })));
                            }
                            "access_denied" => {
                                return Ok(DispatchOutput::ok(json!({
                                    "ok": true,
                                    "status": "denied",
                                    "error": error,
                                })));
                            }
                            _ => {
                                return Ok(DispatchOutput::ok(json!({
                                    "ok": true,
                                    "status": "error",
                                    "error": error,
                                })));
                            }
                        }
                    }

                    // Success! We got an access_token
                    let access_token = body
                        .get("access_token")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let token_type = body
                        .get("token_type")
                        .and_then(Value::as_str)
                        .unwrap_or("bearer");
                    let scope = body.get("scope").and_then(Value::as_str).unwrap_or("");

                    info!(
                        "Copilot Device Code flow completed — access_token obtained ({} chars)",
                        access_token.len()
                    );

                    // Set both secret overrides so CopilotAgent works regardless
                    // of configured token_env (thread-safe alternative to
                    // std::env::set_var, which is UB in multi-threaded programs).
                    set_secret_override("GITHUB_TOKEN", access_token);
                    set_secret_override("GITHUB_COPILOT_TOKEN", access_token);

                    // Persist both Copilot keyring aliases for backward/forward compatibility.
                    if !access_token.is_empty() {
                        // Async wrapper: keyring set_password performs blocking
                        // I/O and must not run on a tokio worker.
                        match crate::shared::secret_override::set_keyring_async(
                            "go-on",
                            "copilot_api_key",
                            access_token,
                        )
                        .await
                        {
                            Ok(()) => ensure_keyring_item_accessible("copilot_api_key"),
                            Err(e) => tracing::warn!(
                                "failed to save Copilot token to keyring account copilot_api_key: {}",
                                e
                            ),
                        }

                        match crate::shared::secret_override::set_keyring_async(
                            "go-on",
                            "github_copilot_token",
                            access_token,
                        )
                        .await
                        {
                            Ok(()) => ensure_keyring_item_accessible("github_copilot_token"),
                            Err(e) => tracing::warn!(
                                "failed to save Copilot token to keyring account github_copilot_token: {}",
                                e
                            ),
                        }
                    }

                    Ok(DispatchOutput::ok(json!({
                        "ok": true,
                        "status": "authorized",
                        "access_token": access_token,
                        "token_type": token_type,
                        "scope": scope,
                    })))
                }
                Err(e) => {
                    let err_msg = format!("Failed to parse GitHub token response: {}", e);
                    tracing::error!("{}", err_msg);
                    anyhow::bail!(err_msg)
                }
            }
        }
        Err(e) => {
            let err_msg = format!("Failed to connect to GitHub token endpoint: {}", e);
            tracing::error!("{}", err_msg);
            anyhow::bail!(err_msg)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::estimate_p95_from_buckets;
    use super::{ProxyEnvSnapshot, GITHUB_CLIENT_CACHE};

    #[test]
    fn estimate_p95_from_buckets_skewed_samples_differ_from_average() {
        // Create a skewed distribution: 90 samples in fast bucket (1-5ms), 10 in slow bucket (500-1000ms)
        // Bucket indices align with P95_BUCKET_BOUNDARIES_MS:
        //   [1]=5.0ms boundary => bucket covers [1.0, 5.0)
        //   [6]=1000.0ms boundary => bucket covers [500.0, 1000.0)
        let mut buckets = [0u64; 10];
        buckets[1] = 90; // 5ms bucket boundary - fast
        buckets[6] = 10; // 1000ms bucket boundary - slow

        let p95 = estimate_p95_from_buckets(&buckets);
        // Weighted average using bucket boundaries as simplified approximations
        let avg = (90.0 * 5.0 + 10.0 * 1000.0) / 100.0;

        assert!(
            p95 > avg * 2.0,
            "p95 ({}) should be significantly higher than avg ({}) for skewed distribution",
            p95,
            avg
        );
        assert!(
            p95 > 500.0,
            "p95 should be in the higher bucket for skewed distribution"
        );
    }

    #[test]
    fn github_client_cache_rebuilds_only_when_proxy_env_changes() {
        use temp_env::with_vars;

        // build_github_client is async now (its probe loop runs on the tokio
        // blocking pool), so each invocation runs on a dedicated thread with its
        // own current-thread runtime — a nested runtime would panic, and env
        // vars are process-global so `with_vars` still applies. The shared
        // GITHUB_CLIENT_CACHE is process-wide, so cache reuse across the two
        // invocations is still exercised.
        fn build_client_sync() -> reqwest::Client {
            std::thread::spawn(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("build test runtime")
                    .block_on(super::build_github_client())
            })
            .join()
            .expect("client build thread panicked")
        }

        // Both proxy URLs point at closed ports, so building the client is
        // fast and never touches the network.
        with_vars(
            [
                ("HTTPS_PROXY", Some("http://127.0.0.1:1")),
                ("https_proxy", None),
                ("ALL_PROXY", None),
                ("all_proxy", None),
            ],
            || {
                let _first = build_client_sync();
                // Unchanged env: the second call must take the fast path and
                // reuse the cached client instead of re-probing proxy ports.
                let _second = build_client_sync();

                let cache = GITHUB_CLIENT_CACHE.get_or_init(|| std::sync::Mutex::new(None));
                let guard = cache.lock().unwrap();
                let (cached_env, _client) = guard.as_ref().expect("client must be cached");
                assert_eq!(*cached_env, ProxyEnvSnapshot::capture());
            },
        );

        // Changing the proxy env must trigger a rebuild and refresh the cache
        // with the new snapshot.
        with_vars(
            [
                ("HTTPS_PROXY", Some("http://127.0.0.1:2")),
                ("https_proxy", None),
                ("ALL_PROXY", None),
                ("all_proxy", None),
            ],
            || {
                let _rebuilt = build_client_sync();
                let cache = GITHUB_CLIENT_CACHE.get_or_init(|| std::sync::Mutex::new(None));
                let guard = cache.lock().unwrap();
                let (cached_env, _client) = guard.as_ref().expect("client must be cached");
                assert_eq!(*cached_env, ProxyEnvSnapshot::capture());
            },
        );
    }
}
