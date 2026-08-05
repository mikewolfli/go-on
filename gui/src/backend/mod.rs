pub mod rpc;
pub mod state;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

// Re-export standalone types from sub-modules
pub use state::{AbortController, StreamProcessor, TokenProgress};

// ── Constants ───────────────────────────────────────────────────────────────

const MODELS_CACHE_TTL_SECS: u64 = 300;

// ── Type aliases ────────────────────────────────────────────────────────────

type ProviderModels = std::collections::HashMap<String, Vec<String>>;
type ModelsCacheState = (Option<ProviderModels>, std::time::Instant);
type ModelsCache = Arc<std::sync::Mutex<ModelsCacheState>>;

// ── Input validation ────────────────────────────────────────────────────────

/// Validate that `data` does not exceed `max_bytes`. Returns `Ok(())` if
/// within bounds, `Err` with a descriptive message otherwise.
pub fn validate_input_size(data: &[u8], max_bytes: usize) -> Result<(), String> {
    if data.len() > max_bytes {
        return Err(format!(
            "input exceeds maximum size limit: {} bytes > {} bytes",
            data.len(),
            max_bytes
        ));
    }
    Ok(())
}

// ── Shared HTTP client / request-body helpers ───────────────────────────────

/// Build an HTTP client with the given total request timeout (in seconds),
/// optional read timeout, and optional TCP keepalive. The builder is retried
/// once without keepalive and finally falls back to `reqwest::Client::new()`.
/// All GUI HTTP clients share this construction so timeout/fallback behavior
/// stays consistent.
pub(crate) fn build_http_client(
    timeout_secs: u64,
    read_timeout_secs: Option<u64>,
    keepalive_secs: Option<u64>,
) -> reqwest::Client {
    let build = |with_keepalive: bool| {
        let mut builder = reqwest::Client::builder().timeout(Duration::from_secs(timeout_secs));
        if let Some(rt) = read_timeout_secs {
            builder = builder.read_timeout(Duration::from_secs(rt));
        }
        if with_keepalive {
            if let Some(ka) = keepalive_secs {
                builder = builder.tcp_keepalive(Some(Duration::from_secs(ka)));
            }
        }
        builder.build()
    };
    build(true).unwrap_or_else(|_| build(false).unwrap_or_else(|_| reqwest::Client::new()))
}

/// Build the JSON request body for the chat stream endpoint, shared by the
/// streaming chat path (`chat_impl`) and the non-streaming
/// `BackendClient::chat_with_options` fallback.
///
/// The `phase` field is included when non-empty so the backend uses the
/// user-selected phase instead of default/adaptive inference. `options_extra`
/// values are flattened into `options` (NOT under an `extra` key), conversation
/// tracking IDs are emitted as top-level keys, and a non-empty `selected_agent`
/// is emitted as `options.preferred_agent`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_chat_request_body(
    messages: &[serde_json::Value],
    mode: &str,
    phase: &str,
    model: Option<&str>,
    options_extra: Option<&serde_json::Value>,
    conv_id: Option<&str>,
    branch_id: Option<&str>,
    selected_agent: &str,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "messages": messages,
        "mode": mode,
    });

    // Include the phase so the backend uses the user-selected phase instead of
    // default/adaptive inference.
    if !phase.is_empty() {
        body["phase"] = serde_json::json!(phase);
    }

    // Only set an explicit model when one was selected (skip empty/"auto").
    if let Some(selected_model) = model.filter(|m| !m.trim().is_empty() && *m != "auto") {
        body["options"] = serde_json::json!({
            "model": selected_model,
        });
    }

    if let Some(extra) = options_extra {
        if body.get("options").is_none() {
            body["options"] = serde_json::json!({});
        }
        // Flatten extra values into options, NOT under an "extra" key.
        if let Some(obj) = extra.as_object() {
            for (k, v) in obj {
                body["options"][k] = v.clone();
            }
        }
    }

    // Conversation tracking IDs as top-level keys.
    if let Some(cid) = conv_id {
        body["conversation_id"] = serde_json::json!(cid);
    }
    if let Some(bid) = branch_id {
        body["branch_id"] = serde_json::json!(bid);
    }

    // Always send preferred_agent when explicitly selected.
    if !selected_agent.is_empty() {
        if let Some(serde_json::Value::Object(ref mut options_map)) = body.get_mut("options") {
            options_map.insert(
                "preferred_agent".to_string(),
                serde_json::Value::String(selected_agent.to_string()),
            );
        } else {
            body["options"] = serde_json::json!({"preferred_agent": selected_agent});
        }
    }

    body
}

// ── BackendClient ───────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct BackendClient {
    /// Client for short-lived requests (health checks, probes - 5s timeout)
    quick_client: reqwest::Client,
    /// Client for long-lived requests (chat - 180s timeout)
    long_client: reqwest::Client,
    base_url: String,
    /// Monotonically increasing JSON-RPC request id (per JSON-RPC 2.0 spec)
    next_id: Arc<AtomicU64>,
    /// Model list cache with timestamp
    models_cache: ModelsCache,
    /// Flag set when fetch_models falls back to expired cache
    stale_models_flag: Arc<AtomicBool>,
}

impl BackendClient {
    pub fn new(base_url: &str) -> Self {
        // Quick client: 5s total timeout + 30s keepalive (keepalive is
        // primary-only, matching the historical fallback chain).
        let quick_client = build_http_client(5, None, Some(30));
        // Long client: 180s total timeout + 45s keepalive (as before).
        let long_client = build_http_client(180, None, Some(45));
        Self {
            quick_client,
            long_client,
            base_url: base_url.trim_end_matches('/').to_string(),
            next_id: Arc::new(AtomicU64::new(1)),
            models_cache: Arc::new(std::sync::Mutex::new((None, std::time::Instant::now()))),
            stale_models_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    pub async fn fetch_models(&self) -> ProviderModels {
        let mut stale_cached: Option<ProviderModels> = None;

        // Check cache (valid for 5 minutes)
        {
            let cache = self.models_cache.lock().unwrap_or_else(|e| e.into_inner());
            let (cached_models, timestamp) = &*cache;
            if let Some(models) = cached_models {
                if timestamp.elapsed().as_secs() < MODELS_CACHE_TTL_SECS {
                    return models.clone();
                }
                stale_cached = Some(models.clone());
            }
        }

        // Fast gate: timebox the first RPC to ensure the UI remains responsive.
        // 2000ms gives the backend enough time to start up on fresh launches
        // while still failing fast for genuinely unreachable backends.
        // When timed out, we return stale cache (or empty) — the caller (show())
        // will retry after 3 seconds via models_loaded == false.
        let resp = match tokio::time::timeout(
            Duration::from_millis(2000),
            self.rpc_call("models.list", None),
        )
        .await
        {
            Ok(result) => result,
            Err(_elapsed) => {
                tracing::warn!(
                    "[fetch_models] models.list timed out after 2000ms, returning stale cache"
                );
                self.stale_models_flag.store(true, Ordering::SeqCst);
                return stale_cached.unwrap_or_default();
            }
        };
        let mut result = match resp {
            Ok(val) => {
                let mut result: ProviderModels = std::collections::HashMap::new();
                if let Some(models) = val.get("models").and_then(|m| m.as_array()) {
                    for entry_val in models.iter() {
                        let provider = entry_val.get("provider").and_then(Value::as_str);
                        let id = entry_val.get("id").and_then(Value::as_str);
                        if let (Some(provider), Some(id)) = (provider, id) {
                            result
                                .entry(provider.to_string())
                                .or_default()
                                .push(id.to_string());
                        }
                    }
                }

                for models in result.values_mut() {
                    let mut seen = std::collections::HashSet::new();
                    models.retain(|model| seen.insert(model.clone()));
                }

                result
            }
            Err(_) => {
                // If refresh fails, keep showing stale data instead of blanking the model list.
                self.stale_models_flag.store(true, Ordering::SeqCst);
                return stale_cached.unwrap_or_default();
            }
        };

        // Prefer provider.list_models for Copilot so GUI uses the same
        // backend-resolved model ordering/candidates as chat execution.
        // Wrap in a short timeout because the backend may hang while
        // contacting the Copilot API (e.g., no network/proxy).
        // If it times out, the static copilot models from models.list above
        // are already in `result` — no data loss.
        let copilot_val = tokio::time::timeout(
            Duration::from_secs(3),
            self.rpc_call(
                "provider.list_models",
                Some(serde_json::json!({ "provider": "copilot" })),
            ),
        )
        .await
        .ok()
        .and_then(|r| r.ok());
        if let Some(copilot_val) = copilot_val {
            let ids = copilot_val
                .get("model_ids")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|item| item.as_str().map(ToString::to_string))
                        .collect::<Vec<_>>()
                })
                .or_else(|| {
                    copilot_val
                        .get("models")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|item| item.get("id").and_then(Value::as_str))
                                .map(ToString::to_string)
                                .collect::<Vec<_>>()
                        })
                })
                .unwrap_or_default();

            if !ids.is_empty() {
                result.insert("copilot".to_string(), ids);
            }
        }

        // Debug: log how many models were loaded per provider
        let total: usize = result.values().map(|v| v.len()).sum();
        let providers: Vec<String> = result.keys().cloned().collect();
        if !result.is_empty() {
            tracing::info!(
                "[fetch_models] loaded {} models from {:?}: {:?}",
                total,
                providers,
                result
            );
        } else {
            tracing::warn!("[fetch_models] loaded 0 models (result may be empty)");
        }

        // Update cache only on successful refresh.
        {
            let mut cache = self.models_cache.lock().unwrap_or_else(|e| e.into_inner());
            cache.0 = Some(result.clone());
            cache.1 = std::time::Instant::now();
        }

        // Clear stale flag: fresh data successfully retrieved.
        self.stale_models_flag.store(false, Ordering::SeqCst);

        result
    }

    pub fn set_base_url(&mut self, url: &str) {
        self.base_url = url.trim_end_matches('/').to_string();
        {
            let mut cache = self.models_cache.lock().unwrap_or_else(|e| e.into_inner());
            cache.0 = None;
            cache.1 = std::time::Instant::now();
        }
    }

    /// Returns `true` if the last model list fetch fell back to expired cache.
    pub fn stale_models(&self) -> bool {
        self.stale_models_flag.load(Ordering::SeqCst)
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

// ── Health & Status ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct HealthStatus {
    pub connected: bool,
    pub healthy: bool,
    pub uptime: u64,
    pub requests_per_minute: f64,
    pub success_rate: f64,
    pub avg_latency_ms: f64,
    pub backend_version: Option<String>,
    pub backend_build: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderStatus {
    pub name: String,
    pub ready: bool,
    pub model: String,
}

impl BackendClient {
    /// Check backend health via direct HTTP GET /health (5s timeout, silent on failure)
    /// Uses GET /health instead of RPC to avoid the /rpc serial lock bottleneck.
    pub async fn health(&self) -> HealthStatus {
        let url = format!("{}/health", self.base_url);
        match self.quick_client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                match resp.json::<serde_json::Value>().await {
                    Ok(val) => HealthStatus {
                        connected: true,
                        healthy: val["lifecycle"]["is_healthy"].as_bool().unwrap_or(false),
                        uptime: val["lifecycle"]["uptime_seconds"].as_u64().unwrap_or(0),
                        requests_per_minute: {
                            let total = val["metrics"]["total_requests"].as_u64().unwrap_or(0);
                            let uptime = val["lifecycle"]["uptime_seconds"]
                                .as_u64()
                                .unwrap_or(1)
                                .max(1);
                            (total as f64 / uptime as f64) * 60.0
                        },
                        success_rate: {
                            let total = val["metrics"]["total_requests"]
                                .as_u64()
                                .unwrap_or(0)
                                .max(1);
                            let success =
                                val["metrics"]["successful_requests"].as_u64().unwrap_or(0);
                            (success as f64 / total as f64) * 100.0
                        },
                        avg_latency_ms: val["metrics"]["avg_request_duration_ms"]
                            .as_f64()
                            .unwrap_or(0.0),
                        backend_version: val
                            .get("backend_version")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                        backend_build: val
                            .get("backend_build")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string()),
                    },
                    Err(_) => HealthStatus {
                        connected: true,
                        healthy: false,
                        uptime: 0,
                        requests_per_minute: 0.0,
                        success_rate: 0.0,
                        avg_latency_ms: 0.0,
                        backend_version: None,
                        backend_build: None,
                    },
                }
            }
            _ => HealthStatus {
                connected: false,
                healthy: false,
                uptime: 0,
                requests_per_minute: 0.0,
                success_rate: 0.0,
                avg_latency_ms: 0.0,
                backend_version: None,
                backend_build: None,
            },
        }
    }

    /// Get provider status (5s timeout, silent on failure)
    pub async fn provider_status(&self) -> Vec<ProviderStatus> {
        // Try provider.status RPC first (new format with agents array)
        match self.rpc_call_quick("provider.status", None).await {
            Some(val) => {
                if let Some(ps) = val.get("provider_status") {
                    if let Some(agents) = ps.get("configured_agents").and_then(|a| a.as_array()) {
                        return agents
                            .iter()
                            .filter_map(|a| {
                                let name = a.get("name")?.as_str()?;
                                let ready = a.get("ready")?.as_bool().unwrap_or(false);
                                Some(ProviderStatus {
                                    name: name.to_string(),
                                    ready,
                                    model: String::new(),
                                })
                            })
                            .collect();
                    }
                    let summary = ps.get("summary").and_then(|s| s.as_object());
                    let configured = summary
                        .and_then(|s| s.get("configured"))
                        .and_then(|c| c.as_u64())
                        .unwrap_or(0);
                    if configured > 0 {
                        // Agents exist in registry but not in configured_agents array
                        // Fallback: read from registry_catalog
                        if let Some(catalog) = ps.get("registry_catalog").and_then(|c| c.as_array())
                        {
                            return catalog
                                .iter()
                                .filter_map(|a| {
                                    let name = a.get("agent")?.as_str()?;
                                    Some(ProviderStatus {
                                        name: name.to_string(),
                                        ready: false,
                                        model: String::new(),
                                    })
                                })
                                .collect();
                        }
                    }
                }
                Vec::new()
            }
            None => {
                // Fallback to legacy health.probes format
                match self.rpc_call_quick("health.probes", None).await {
                    Some(val) => {
                        let probes = val.get("probes");
                        let deps = probes
                            .and_then(|p| p.get("dependencies"))
                            .and_then(|d| d.as_array());
                        if let Some(deps) = deps {
                            for dep in deps {
                                if dep.get("name").and_then(|n| n.as_str())
                                    == Some("provider_dependencies")
                                {
                                    if let Some(details) = dep.get("details") {
                                        if let Some(api_map) = details
                                            .get("provider_api_map")
                                            .and_then(|m| m.as_array())
                                        {
                                            return api_map
                                                .iter()
                                                .filter_map(|p| {
                                                    let name = p.get("provider")?.as_str()?;
                                                    let status = p.get("status")?.as_str()?;
                                                    Some(ProviderStatus {
                                                        name: name.to_string(),
                                                        ready: status == "set",
                                                        model: String::new(),
                                                    })
                                                })
                                                .collect();
                                        }
                                    }
                                }
                            }
                        }
                        Vec::new()
                    }
                    None => Vec::new(),
                }
            }
        }
    }
}

// ── Chat ────────────────────────────────────────────────────────────────────

impl BackendClient {
    #[allow(clippy::too_many_arguments)]
    pub async fn chat_with_options(
        &self,
        message: &str,
        mode: &str,
        phase: &str,
        model: Option<&str>,
        options_extra: Option<Value>,
        history: Option<Vec<Value>>,
        abort_ctrl: Option<AbortController>,
    ) -> Result<(String, String, String, Option<String>), String> {
        let messages = if let Some(hist) = history {
            let mut msgs = hist;
            msgs.push(serde_json::json!({ "role": "user", "content": message }));
            msgs
        } else {
            vec![serde_json::json!({ "role": "user", "content": message })]
        };

        // Use the UI-selected mode (edit, ask, plan, etc.).
        // Default to "edit" if mode is empty.
        let effective_mode = if mode.is_empty() { "edit" } else { mode };
        let body = build_chat_request_body(
            &messages,
            effective_mode,
            phase,
            model,
            options_extra.as_ref(),
            // Conversation tracking IDs ride inside `options_extra`; lift them
            // to top-level keys (same shape as the streaming chat path).
            options_extra
                .as_ref()
                .and_then(|extra| extra.get("conversation_id").and_then(|v| v.as_str())),
            options_extra
                .as_ref()
                .and_then(|extra| extra.get("branch_id").and_then(|v| v.as_str())),
            "",
        );

        let mut last_err = String::new();
        let mut response = None;

        // Use /chat/stream (native Go-On endpoint) which accepts ChatParams format
        // (mode, phase, options, preferred_agent, etc.). Do NOT use the protocol-
        // negotiated endpoint (/v1/chat/completions) because it crashes on GUI-
        // specific fields that OpenAiChatRequest cannot deserialize.
        let endpoint = "/chat/stream";
        for attempt in 1..=3 {
            match self
                .long_client
                .post(format!("{}{}", self.base_url, endpoint))
                .json(&body)
                .send()
                .await
            {
                Ok(resp) => {
                    response = Some(resp);
                    break;
                }
                Err(e) => {
                    last_err = format!("HTTP error: {}", e);
                    if attempt < 3 {
                        tokio::time::sleep(std::time::Duration::from_millis(200 * attempt as u64))
                            .await;
                        continue;
                    }
                }
            }
        }
        let resp = response.ok_or_else(|| last_err.clone())?;

        let resp = resp
            .error_for_status()
            .map_err(|e| format!("HTTP error: {}", e))?;

        // Validate response size before reading the body (limit to 64 MB)
        if let Some(content_length) = resp.content_length() {
            // Pre-validate the declared content-length to avoid allocating
            // an excessively large response buffer.
            let max_response_bytes: usize = 64 * 1024 * 1024;
            if (content_length as usize) > max_response_bytes {
                return Err(format!(
                    "response content length too large: {} bytes > 64 MB",
                    content_length
                ));
            }
        }

        // Race the response body against abort signal so the user can cancel
        // an in-flight HTTP request during the response-read phase.
        // Create the futures outside select! to avoid ownership conflicts:
        // `resp.text()` consumes `resp`, so we move it into a pinned future.
        let resp_body = if let Some(abort_ctrl) = abort_ctrl {
            let text_fut = resp.text();
            let abort_fut = abort_ctrl.wait_for_abort();
            tokio::pin!(text_fut);
            tokio::pin!(abort_fut);
            tokio::select! {
                body = &mut text_fut => {
                    body.map_err(|e| format!("SSE body read error: {}", e))?
                }
                _ = &mut abort_fut => {
                    // Dropping text_fut will drop `resp` and close the connection.
                    // Fire-and-forget a JSON-RPC cancel notification so the backend
                    // can stop processing on its side.
                    let cancel_url = format!("{}/rpc", self.base_url);
                    let cancel_body = serde_json::json!({
                        "jsonrpc": "2.0",
                        "method": "$/cancel_request",
                        "params": {},
                    });
                    let cancel_client = self.long_client.clone();
                    tokio::spawn(async move {
                        let _ = cancel_client
                            .post(&cancel_url)
                            .json(&cancel_body)
                            .send()
                            .await;
                    });
                    return Err("Request cancelled by user".to_string());
                }
            }
        } else {
            resp.text()
                .await
                .map_err(|e| format!("SSE body read error: {}", e))?
        };

        // Validate actual body size after reading
        validate_input_size(resp_body.as_bytes(), 64 * 1024 * 1024)?;

        // Parse SSE events using StreamProcessor
        let mut response_text = String::new();
        let mut thinking_text = String::new();
        let mut agent_text = String::new();
        let mut selected_model: Option<String> = None;

        let mut processor = StreamProcessor::new();
        let events = processor.push_chunk(resp_body.as_bytes());
        for event_result in events {
            match event_result {
                Ok(val) => {
                    // Handle [DONE] sentinel (with or without an event type)
                    if val.is_string() && val.as_str() == Some("[DONE]") {
                        break;
                    }
                    if val.get("data").and_then(|v| v.as_str()) == Some("[DONE]") {
                        break;
                    }

                    let event_type = val
                        .get("_event_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    match event_type {
                        "chunk" | "" => {
                            // Shared chunk field extraction (token/text fallback +
                            // reasoning) — single source of truth with the rich
                            // stream path in views/chat/chat_impl/runtime.rs.
                            let (text, reasoning) = crate::backend::state::extract_chunk_text(&val);
                            response_text.push_str(&text);
                            thinking_text.push_str(&reasoning);
                            // agent/model may appear on the first event or the done event
                            let (agent, model) = crate::backend::state::extract_agent_model(&val);
                            if let Some(agent) = agent {
                                agent_text = agent;
                            }
                            if let Some(model) = model {
                                selected_model = Some(model);
                            }
                        }
                        "result" | "done" => {
                            // Final result event — extract response content.
                            // This is used by non-streaming responses from /chat/stream.
                            let meta = crate::backend::state::extract_result_meta(&val);
                            if let Some(text) = meta.response {
                                response_text = text;
                            }
                            if let Some(r) = meta.thinking {
                                thinking_text = r;
                            }
                            if let Some(agent) = meta.agent {
                                if !agent.is_empty() {
                                    agent_text = agent;
                                }
                            }
                            if let Some(model) = meta.model {
                                if !model.is_empty() {
                                    selected_model = Some(model);
                                }
                            }
                        }
                        "error" => {
                            if let Some(err_msg) = val
                                .get("error")
                                .or_else(|| val.get("message"))
                                .and_then(|v| v.as_str())
                            {
                                return Err(format!("Chat error: {}", err_msg));
                            }
                            // If neither "error" nor "message" field is present,
                            // serialize the entire payload for debugging.
                            return Err(format!(
                                "Chat error: {}",
                                serde_json::to_string(&val).unwrap_or_default()
                            ));
                        }
                        "telemetry" => {
                            // Telemetry events carry token economy data — ignore here
                            // as they don't contribute to response text.
                        }
                        _ => {}
                    }
                }
                Err(e) => {
                    return Err(format!("SSE parse error: {}", e));
                }
            }
        }

        if response_text.is_empty() && thinking_text.is_empty() {
            // Empty response: return a descriptive warning that preserves
            // the agent and selected_model metadata so the GUI can still
            // show the model info even when the response is empty.
            //
            // NOTE: We do NOT silently replace with a canned message here.
            // The GUI should display the empty response so the user can
            // diagnose the issue (misconfigured provider, empty model output,
            // etc.). If the agent had an error, it should have emitted an
            // SSE error event which would have been caught above.
            tracing::warn!(
                "Backend returned empty response for chat (agent={:?}, model={:?})",
                agent_text,
                selected_model
            );
            Ok((String::new(), String::new(), agent_text, selected_model))
        } else {
            Ok((response_text, thinking_text, agent_text, selected_model))
        }
    }
}

// ── Provider RPC wrappers ───────────────────────────────────────────────────

impl BackendClient {
    pub async fn configure_provider(
        &self,
        name: &str,
        api_key: &str,
        model: &str,
    ) -> Result<Value, String> {
        let params = serde_json::json!({
            "name": name,
            "api_key": api_key,
            "model": model,
        });
        self.rpc_call("provider.configure", Some(params)).await
    }

    /// Configure a provider with both api_key and secret_key (e.g. wenxin).
    pub async fn configure_provider_with_secret(
        &self,
        name: &str,
        api_key: &str,
        secret_key: &str,
        model: &str,
    ) -> Result<Value, String> {
        let params = serde_json::json!({
            "name": name,
            "api_key": api_key,
            "secret_key": secret_key,
            "model": model,
        });
        self.rpc_call("provider.configure", Some(params)).await
    }

    pub async fn restart_backend(&self) -> Result<Value, String> {
        self.rpc_call("runtime.restart", None).await
    }

    /// Start the GitHub Copilot OAuth device-code flow via the backend RPC.
    /// Returns `device_code`, `user_code`, `verification_uri`, and `interval`.
    pub async fn copilot_device_code(&self) -> Result<Value, String> {
        self.rpc_call("provider.copilot_device_code", Some(serde_json::json!({})))
            .await
    }

    /// Poll for the Copilot access token with the device code issued by
    /// [`copilot_device_code`](Self::copilot_device_code).
    pub async fn copilot_device_code_poll(&self, device_code: &str) -> Result<Value, String> {
        self.rpc_call(
            "provider.copilot_device_code_poll",
            Some(serde_json::json!({ "device_code": device_code })),
        )
        .await
    }

    /// Approve a tool that was blocked by the sandbox whitelist.
    /// Called when the user clicks "Approve" in the chat UI.
    pub async fn approve_tool(&self, tool_name: &str) -> Result<Value, String> {
        self.rpc_call(
            "tool.approve",
            Some(serde_json::json!({"tool_name": tool_name})),
        )
        .await
    }

    pub async fn provider_test_connection(&self, provider: &str) -> Result<Value, String> {
        self.rpc_call(
            "provider.test_connection",
            Some(serde_json::json!({"provider": provider})),
        )
        .await
    }

    pub async fn provider_test_completion(
        &self,
        provider: &str,
        model: Option<&str>,
    ) -> Result<Value, String> {
        let mut params = serde_json::json!({"provider": provider});
        if let Some(model) = model {
            params["model"] = Value::String(model.to_string());
        }
        self.rpc_call("provider.test_completion", Some(params))
            .await
    }

    /// Fetch the full provider catalog from the backend.
    /// Calls `provider.catalog` RPC directly (no health check needed).
    /// Returns a JSON value with provider specs including agent_type, default_url,
    /// default_model, and supports_system per provider.
    /// This is the canonical source when the backend is reachable; the GUI falls
    /// back to `built_in_provider_specs()` in `catalog.rs` when offline.
    pub async fn provider_catalog(&self) -> Result<Value, String> {
        self.rpc_call_quick("provider.catalog", None)
            .await
            .ok_or_else(|| "Failed to fetch provider catalog from backend".to_string())
    }

    /// Fetch the provider catalog from the backend with availability checking.
    ///
    /// First checks backend health, then calls the catalog endpoint.
    /// Returns `Ok(Some(catalog))` on success, `Ok(None)` if backend is unreachable,
    /// and `Err` on communication failure. This is the preferred async entry point
    /// for GUI startup flows; it wraps health check + remote call in one call.
    pub async fn provider_capabilities(
        &self,
        provider: &str,
    ) -> Result<Vec<ProviderCapabilityModel>, String> {
        let result = self
            .rpc_call(
                "provider.capabilities",
                Some(serde_json::json!({"provider": provider})),
            )
            .await?;
        let models = result
            .get("capabilities")
            .and_then(|caps| caps.get("models"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(models
            .into_iter()
            .filter_map(|value| serde_json::from_value::<ProviderCapabilityModel>(value).ok())
            .collect())
    }
}

// ── Provider Capability Model ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderCapabilityModel {
    pub id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub is_default: Option<bool>,
    pub context_window: Option<u64>,
    pub capabilities: Option<Vec<String>>,
    pub tool_calling: Option<bool>,
    pub vision: Option<bool>,
    pub cost_tier: Option<String>,
}

// ── Skill RPC wrappers ──────────────────────────────────────────────────────

impl BackendClient {
    pub async fn create_skill(
        &self,
        name: &str,
        description: &str,
        prompt: &str,
        input_schema: &str,
    ) -> Result<Value, String> {
        let schema_value: serde_json::Value =
            serde_json::from_str(input_schema).unwrap_or_else(|_| serde_json::json!({}));
        let params = serde_json::json!({
            "name": name,
            "description": description,
            "prompt_template": prompt,
            "input_schema": schema_value,
        });
        self.rpc_call("skill.create", Some(params)).await
    }

    /// Import a skill from a remote URL or GitHub repo via the backend's skill.import RPC.
    /// The backend handles downloading, SHA-256 verification, and manifest validation.
    ///
    /// For GitHub repos, use `{"source": {"Github": {"repo": "owner/repo", "ref": "main"}}}`.
    /// For direct URLs, use `{"source": {"Url": {"url": "https://..."}}}`.
    pub async fn import_skill(&self, source: serde_json::Value) -> Result<Value, String> {
        let params = serde_json::json!({
            "source": source
        });
        self.rpc_call("skill.import", Some(params)).await
    }

    pub async fn list_skills(&self) -> Result<Value, String> {
        self.rpc_call("skill.list_imported", None).await
    }

    pub async fn update_skill(
        &self,
        name: &str,
        description: Option<String>,
        prompt_template: Option<String>,
        input_schema: Option<Value>,
        version: Option<String>,
    ) -> Result<Value, String> {
        let mut params = serde_json::json!({"name": name});
        if let Some(description) = description {
            params["description"] = Value::String(description);
        }
        if let Some(prompt_template) = prompt_template {
            params["prompt_template"] = Value::String(prompt_template);
        }
        if let Some(input_schema) = input_schema {
            params["input_schema"] = input_schema;
        }
        if let Some(version) = version {
            params["version"] = Value::String(version);
        }
        self.rpc_call("skill.update", Some(params)).await
    }

    pub async fn list_skill_versions(&self, name: &str) -> Result<Value, String> {
        self.rpc_call(
            "skill.version.list",
            Some(serde_json::json!({"name": name})),
        )
        .await
    }

    pub async fn rollback_skill_version(&self, name: &str, version: &str) -> Result<Value, String> {
        self.rpc_call(
            "skill.version.rollback",
            Some(serde_json::json!({"name": name, "version": version})),
        )
        .await
    }

    pub async fn enable_skill(&self, name: &str) -> Result<Value, String> {
        self.rpc_call("skill.enable", Some(serde_json::json!({"name": name})))
            .await
    }

    pub async fn disable_skill(&self, name: &str) -> Result<Value, String> {
        self.rpc_call("skill.disable", Some(serde_json::json!({"name": name})))
            .await
    }

    pub async fn remove_skill(&self, name: &str) -> Result<Value, String> {
        self.rpc_call("skill.remove", Some(serde_json::json!({"name": name})))
            .await
    }

    pub async fn test_skill(&self, name: &str, input: Value) -> Result<Value, String> {
        self.rpc_call(
            "mcp.tools.call",
            Some(serde_json::json!({
                "name": name,
                "arguments": input,
            })),
        )
        .await
    }
}

// ── Workflow RPC wrappers ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkflowRunRecord {
    pub run_id: String,
    pub task: String,
    pub status: String,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub phase: String,
    pub error: Option<String>,
    pub artifacts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkflowRunsResult {
    pub runs: Vec<WorkflowRunRecord>,
}

impl BackendClient {
    pub async fn list_workflow_runs_typed(
        &self,
        limit: usize,
        offset: usize,
        status: Option<&str>,
    ) -> Result<Vec<WorkflowRunRecord>, String> {
        let mut params = serde_json::json!({"limit": limit, "offset": offset});
        if let Some(status) = status {
            params["status"] = Value::String(status.to_string());
        }
        let value = self.rpc_call("workflow.run.list", Some(params)).await?;
        Self::decode_workflow_runs(value)
    }

    pub async fn get_workflow_run_typed(&self, run_id: &str) -> Result<WorkflowRunRecord, String> {
        let value = self
            .rpc_call(
                "workflow.run.get",
                Some(serde_json::json!({"run_id": run_id})),
            )
            .await?;
        let candidate = value
            .get("run")
            .cloned()
            .or_else(|| value.get("result").and_then(|r| r.get("run")).cloned())
            .unwrap_or(value);
        serde_json::from_value::<WorkflowRunRecord>(candidate)
            .map_err(|e| format!("workflow.run.get decode error: {e}"))
    }

    pub async fn transition_workflow_run(
        &self,
        run_id: &str,
        action: &str,
    ) -> Result<Value, String> {
        let method = match action {
            "cancel" => "workflow.run.cancel",
            "pause" => "workflow.run.pause",
            "resume" => "workflow.run.resume",
            _ => return Err(format!("unsupported workflow action: {action}")),
        };
        self.rpc_call(method, Some(serde_json::json!({"run_id": run_id})))
            .await
    }

    pub async fn execute_workflow(
        &self,
        task: &str,
        phase: Option<&str>,
        options_extra: Option<Value>,
    ) -> Result<Value, String> {
        let mut params = serde_json::json!({"task": task});
        if let Some(phase) = phase {
            if !phase.trim().is_empty() {
                params["phase"] = Value::String(phase.to_string());
            }
        }
        if let Some(extra) = options_extra {
            params["options"] = serde_json::json!({"extra": extra});
        }
        self.rpc_call("workflow.execute", Some(params)).await
    }
}

// ── Metrics RPC wrappers ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MetricsWindowPoint {
    pub ts: i64,
    pub qps: f64,
    pub p95: f64,
    pub error_rate: f64,
    pub success_rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ErrorGroup {
    pub error_type: String,
    pub count: usize,
}

impl BackendClient {
    pub async fn metrics_window_query(
        &self,
        window: &str,
    ) -> Result<Vec<MetricsWindowPoint>, String> {
        let result = self
            .rpc_call(
                "metrics.window.query",
                Some(serde_json::json!({"window": window})),
            )
            .await?;
        let series = result
            .get("series")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(series
            .into_iter()
            .filter_map(|value| serde_json::from_value::<MetricsWindowPoint>(value).ok())
            .collect())
    }

    pub async fn metrics_errors_summary(
        &self,
        window: &str,
        limit: usize,
    ) -> Result<(Vec<ErrorGroup>, Vec<Value>), String> {
        let result = self
            .rpc_call(
                "metrics.errors.summary",
                Some(serde_json::json!({"window": window, "limit": limit})),
            )
            .await?;
        let groups = result
            .get("error_groups")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|value| serde_json::from_value::<ErrorGroup>(value).ok())
            .collect();
        let failures = result
            .get("sample_failures")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok((groups, failures))
    }

    pub async fn config_baseline(&self) -> Result<Value, String> {
        self.rpc_call("config.baseline", None).await
    }
}
