use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

const QUICK_RPC_ATTEMPTS: usize = 2;
const FULL_RPC_ATTEMPTS: usize = 3;
const MODELS_CACHE_TTL_SECS: u64 = 300;

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

// ── StreamProcessor ────────────────────────────────────────────────────────

const MAX_SSE_LINE_LENGTH: usize = 1024 * 1024; // 1 MB per SSE line
const MAX_SSE_CHUNK_SIZE: usize = 16 * 1024 * 1024; // 16 MB total per push call

/// Incrementally parses an SSE byte stream frame-by-frame, extracting `event:`
/// and `data:` fields. Returns parsed JSON values with the event type attached
/// (either as a top-level `"_event_type"` field, or through the existing structure).
/// Tracks token count and total bytes processed for progress reporting in the UI.
pub struct StreamProcessor {
    buffer: String,
    max_buffer_size: usize,
    /// Number of JSON tokens (events) parsed so far.
    pub token_count: usize,
    /// Total bytes consumed from the wire.
    pub total_bytes_processed: usize,
}

impl StreamProcessor {
    pub fn new() -> Self {
        Self {
            buffer: String::with_capacity(16_384),
            max_buffer_size: MAX_SSE_LINE_LENGTH,
            token_count: 0,
            total_bytes_processed: 0,
        }
    }

    /// Feed a chunk of raw bytes into the processor.
    /// Returns a batch of parsed results (Ok(values) or Err(errors)).
    /// Each parsed value now includes an `"_event_type"` field extracted from
    /// the SSE `event:` line, allowing the caller to distinguish between chunk,
    /// done, telemetry, and other event types emitted by the backend.
    pub fn push_chunk(&mut self, chunk: &[u8]) -> Vec<Result<Value, String>> {
        let mut events: Vec<Result<Value, String>> = Vec::new();

        // Validate input size before processing
        if let Err(e) = validate_input_size(chunk, MAX_SSE_CHUNK_SIZE) {
            events.push(Err(e));
            return events;
        }

        // Overflow guard
        if self.buffer.len() + chunk.len() > self.max_buffer_size {
            events.push(Err("SSE buffer overflow (exceeded 1 MB)".to_string()));
            return events;
        }

        self.total_bytes_processed += chunk.len();

        // Normalise CRLF → LF for consistent frame splitting
        let part = String::from_utf8_lossy(chunk);
        self.buffer.push_str(&part.replace('\r', ""));

        // Consume complete SSE frames (delimited by \n\n, fallback \n)
        loop {
            let (delim, delim_len) = if self.buffer.contains("\n\n") {
                ("\n\n", 2usize)
            } else {
                ("\n", 1usize)
            };

            let pos = match self.buffer.find(delim) {
                Some(p) => p,
                None => break,
            };

            let segment = self.buffer[..pos].to_string();
            self.buffer = self.buffer[pos + delim_len..].to_string();

            // Safety: reject unbounded lines
            if segment.len() > MAX_SSE_LINE_LENGTH {
                events.push(Err("SSE line exceeds maximum length (1 MB)".to_string()));
                return events;
            }

            // Collect lines from the segment.
            // When using \n\n delimiter the segment may contain embedded \n
            // (multi-line SSE data), so split further on single \n.
            let sub_lines: Vec<&str> = if delim_len == 2 {
                segment.split('\n').collect()
            } else {
                vec![&segment]
            };

            let mut current_event_type: Option<String> = None;
            let mut current_data: Option<String> = None;

            for line in &sub_lines {
                if let Some(event) = line.strip_prefix("event: ") {
                    current_event_type = Some(event.trim().to_string());
                } else if let Some(data) = line.strip_prefix("data: ") {
                    current_data = Some(data.trim().to_string());
                } else if let Some(data) = line.strip_prefix("data:") {
                    // Handle "data: {json}" without space after colon
                    current_data = Some(data.trim().to_string());
                }
            }

            // Emit a single event per frame, combining event type + data
            if let Some(data_str) = current_data {
                if data_str == "[DONE]" {
                    let mut val = Value::String("[DONE]".to_string());
                    if let Some(ev) = current_event_type {
                        // Wrap [DONE] in an object with event type
                        val = serde_json::json!({
                            "_event_type": ev,
                            "data": "[DONE]",
                        });
                    }
                    events.push(Ok(val));
                    continue;
                }

                match serde_json::from_str::<Value>(&data_str) {
                    Ok(mut val) => {
                        self.token_count += 1;
                        // Inject the event type so callers can distinguish
                        // "chunk", "done", "telemetry", etc.
                        if let Some(ev) = current_event_type {
                            if let Some(obj) = val.as_object_mut() {
                                obj.insert("_event_type".to_string(), Value::String(ev));
                            }
                        }
                        events.push(Ok(val));
                    }
                    Err(e) => {
                        events.push(Err(format!("JSON parse error: {}", e)));
                    }
                }
            } else if let Some(ev) = current_event_type {
                // Event with no data payload — emit a minimal object
                events.push(Ok(serde_json::json!({
                    "_event_type": ev,
                    "_no_data": true,
                })));
            }
        }

        events
    }

    /// Drain any remaining partial segment from the buffer.
    /// Returns `None` if the buffer is empty.
    #[allow(dead_code)]
    pub fn drain_remaining(&mut self) -> Option<String> {
        if self.buffer.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.buffer))
        }
    }
}

impl Default for StreamProcessor {
    fn default() -> Self {
        Self::new()
    }
}

// ── AbortController ────────────────────────────────────────────────────────

/// Shared cancellation signal for in-progress SSE streams.
/// Cloning produces another handle to the same underlying signal.
/// Uses a `tokio::sync::Notify` so callers can `tokio::select!` on the
/// abort signal and cancel the actual in-flight HTTP request.
#[derive(Clone)]
pub struct AbortController {
    cancelled: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
}

impl AbortController {
    pub fn new() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            notify: Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// Signal abort.  Idempotent — safe to call multiple times.
    pub fn abort(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    /// Returns `true` if abort has been signalled.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Returns a future that resolves when `abort()` is called.
    /// Use with `tokio::select!` to cancel in-flight HTTP requests:
    ///
    /// ```ignore
    /// tokio::select! {
    ///     result = http_request => { … },
    ///     _ = abort_ctrl.wait_for_abort() => { … },
    /// }
    /// ```
    pub async fn wait_for_abort(&self) {
        self.notify.notified().await;
    }

    /// Reset the signal for reuse.
    pub fn reset(&self) {
        self.cancelled.store(false, Ordering::SeqCst);
    }
}

impl Default for AbortController {
    fn default() -> Self {
        Self::new()
    }
}

// ── TokenProgress ───────────────────────────────────────────────────────────

/// Lightweight snapshot of streaming progress for the UI.
#[derive(Debug, Clone, Default)]
pub struct TokenProgress {
    /// Number of tokens (SSE events) received so far.
    pub tokens_received: usize,
    /// Total bytes processed from the wire.
    pub bytes_processed: usize,
    /// Input token count reported by telemetry.
    pub input_tokens: usize,
    /// Output token count reported by telemetry.
    pub output_tokens: usize,
    /// Total token count reported by telemetry.
    pub total_tokens: usize,
}

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

impl BackendClient {
    pub fn new(base_url: &str) -> Self {
        let quick_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .tcp_keepalive(Some(Duration::from_secs(30)))
            .build()
            .unwrap_or_else(|e| {
                eprintln!("Failed to build quick HTTP client: {e}; retrying with builder");
                reqwest::Client::builder()
                    .timeout(Duration::from_secs(5))
                    .build()
                    .unwrap_or_else(|e| {
                        eprintln!("Failed to build HTTP client on retry: {e}; using default");
                        reqwest::Client::new()
                    })
            });
        let long_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(180))
            .tcp_keepalive(Some(Duration::from_secs(45)))
            .build()
            .unwrap_or_else(|e| {
                eprintln!("Failed to build long HTTP client: {e}; retrying with builder");
                reqwest::Client::builder()
                    .timeout(Duration::from_secs(180))
                    .build()
                    .unwrap_or_else(|e| {
                        eprintln!("Failed to build HTTP client on retry: {e}; using default");
                        reqwest::Client::new()
                    })
            });
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

        // Fast gate: timebox the first RPC to 500ms so that an unreachable backend
        // doesn't block the UI for 5+ seconds. If the backend is slow, we return
        // stale cache (or empty) and let the next refresh attempt succeed.
        let resp = match tokio::time::timeout(
            Duration::from_millis(500),
            self.rpc_call("models.list", None),
        )
        .await
        {
            Ok(result) => result,
            Err(_elapsed) => {
                eprintln!(
                    "[fetch_models] models.list timed out after 500ms, returning stale cache"
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
        if let Ok(copilot_val) = self
            .rpc_call(
                "provider.list_models",
                Some(serde_json::json!({ "provider": "copilot" })),
            )
            .await
        {
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

    fn is_retryable_status(status: reqwest::StatusCode) -> bool {
        matches!(status.as_u16(), 408 | 429 | 502 | 503 | 504)
    }

    fn is_retryable_rpc_error_code(code: i64) -> bool {
        code == -32603 || (-32099..=-32000).contains(&code)
    }

    fn is_retryable_transport_error(err: &reqwest::Error) -> bool {
        // Timeouts are always retryable
        if err.is_timeout() {
            return true;
        }
        // Connection errors
        if err.is_connect() {
            return true;
        }
        // Body I/O errors (connection interrupted mid-response)
        if err.is_body() {
            return true;
        }
        // Status errors: check both server errors (5xx) and client errors (408 Request Timeout, 429 Too Many Requests)
        if let Some(status) = err.status() {
            return status.is_server_error() || status.as_u16() == 429 || status.as_u16() == 408;
        }
        false
    }

    fn retry_backoff(attempt: usize) -> Duration {
        // Exponential backoff with 30% jitter to prevent thundering herd:
        // delay = base * (0.7 + random * 0.3)
        // Attempt 1: ~100ms, Attempt 2: ~200ms, Attempt 3+: ~400ms
        let base_ms: u64 = match attempt {
            1 => 100,
            2 => 200,
            _ => 400,
        };
        let jitter_factor = 0.7 + fastrand::f64() * 0.3;
        Duration::from_secs_f64((base_ms as f64 * jitter_factor) / 1000.0)
    }

    fn parse_rpc_error(err: &Value) -> String {
        err.get("message")
            .and_then(Value::as_str)
            .or_else(|| err.get("data").and_then(Value::as_str))
            .filter(|s| !s.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| err.to_string())
    }

    fn summarize_http_body(body: &str) -> String {
        let trimmed = body.trim();
        if trimmed.is_empty() {
            return "empty response body".to_string();
        }
        let mut compact = trimmed.replace(['\n', '\r'], " ");
        if compact.len() > 240 {
            compact.truncate(240);
            compact.push_str("...");
        }
        compact
    }

    async fn rpc_call_internal(
        &self,
        client: &reqwest::Client,
        method: &str,
        params: Option<Value>,
        attempts: usize,
    ) -> Result<Value, String> {
        let req_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "method": method,
            "params": params.unwrap_or(Value::Null),
        });
        let url = format!("{}/rpc", self.base_url);
        let mut last_err = String::new();

        const MAX_RPC_RESPONSE_BYTES: usize = 64 * 1024 * 1024; // 64 MB

        for attempt in 1..=attempts {
            let response = match client.post(&url).json(&body).send().await {
                Ok(resp) => resp,
                Err(err) => {
                    last_err = format!("HTTP error: {err}");
                    let retryable = Self::is_retryable_transport_error(&err);
                    if attempt < attempts && retryable {
                        let backoff = Self::retry_backoff(attempt);
                        eprintln!(
                            "[RPC] Attempt {}/{} transport error (retryable={}): {}, backing off {:?}",
                            attempt,
                            attempts,
                            retryable,
                            err,
                            backoff
                        );
                        tokio::time::sleep(backoff).await;
                        continue;
                    }
                    return Err(last_err);
                }
            };

            // Pre-validate content-length header if present
            if let Some(cl) = response.content_length() {
                if (cl as usize) > MAX_RPC_RESPONSE_BYTES {
                    last_err = format!("response content length too large: {} bytes > 64 MB", cl);
                    return Err(last_err);
                }
            }

            let status = response.status();
            let response_text = match response.text().await {
                Ok(text) => {
                    // Validate actual body size
                    if let Err(e) = validate_input_size(text.as_bytes(), MAX_RPC_RESPONSE_BYTES) {
                        last_err = e;
                        return Err(last_err);
                    }
                    text
                }
                Err(err) => {
                    last_err = format!("HTTP body read error: {err}");
                    // Retry on ANY body read error regardless of status code
                    if attempt < attempts {
                        let backoff = Self::retry_backoff(attempt);
                        eprintln!("[RPC] Attempt {}: Body read error, retrying", attempt);
                        tokio::time::sleep(backoff).await;
                        continue;
                    }
                    return Err(last_err);
                }
            };

            if !status.is_success() {
                let detail = serde_json::from_str::<Value>(&response_text)
                    .ok()
                    .and_then(|json| json.get("error").cloned())
                    .map(|err| Self::parse_rpc_error(&err))
                    .unwrap_or_else(|| Self::summarize_http_body(&response_text));
                last_err = format!("HTTP status error {}: {}", status.as_u16(), detail);
                if attempt < attempts && Self::is_retryable_status(status) {
                    let backoff = Self::retry_backoff(attempt);
                    eprintln!(
                        "[RPC] Attempt {}: Status {} (retryable), backing off {:?}",
                        attempt,
                        status.as_u16(),
                        backoff
                    );
                    tokio::time::sleep(backoff).await;
                    continue;
                }
                return Err(last_err);
            }

            // Parse JSON with better error context
            let result: Value = match serde_json::from_str(&response_text) {
                Ok(json) => json,
                Err(parse_err) => {
                    let detail = Self::summarize_http_body(&response_text);
                    let err_msg = format!(
                        "JSON parse error: {} (attempt {}/{}); body={}",
                        parse_err, attempt, attempts, detail
                    );
                    // Retry on JSON parse errors as they might be transient
                    if attempt < attempts {
                        let backoff = Self::retry_backoff(attempt);
                        eprintln!("[RPC] Attempt {}: JSON parse failed, retrying", attempt);
                        tokio::time::sleep(backoff).await;
                        continue;
                    }
                    return Err(err_msg);
                }
            };

            // Check for RPC error in response - might be retryable
            if let Some(rpc_err) = result.get("error") {
                let err_msg = Self::parse_rpc_error(rpc_err);
                // RPC error codes that might be transient:
                // -32603: Internal error (could be backend temporarily overwhelmed)
                // -32000 to -32099: Reserved for implementation-defined server errors
                let code = rpc_err.get("code").and_then(|v| v.as_i64());
                if attempt < attempts && code.is_some_and(Self::is_retryable_rpc_error_code) {
                    let backoff = Self::retry_backoff(attempt);
                    eprintln!(
                        "[RPC] Attempt {}: RPC error code {:?} (retryable), backing off {:?}",
                        attempt, code, backoff
                    );
                    tokio::time::sleep(backoff).await;
                    continue;
                }
                return Err(err_msg);
            }

            // Extract result field, fallback to entire response if no "result" key
            return Ok(result.get("result").cloned().unwrap_or(result));
        }

        Err(if last_err.is_empty() {
            "RPC request failed for unknown reason".to_string()
        } else {
            last_err
        })
    }

    /// Quick RPC call for health / status checks (5s timeout).
    /// Returns None if the backend is unreachable (no error message).
    async fn rpc_call_quick(&self, method: &str, params: Option<Value>) -> Option<Value> {
        self.rpc_call_internal(&self.quick_client, method, params, QUICK_RPC_ATTEMPTS)
            .await
            .ok()
    }

    /// Full RPC call for normal requests (180s timeout).
    pub async fn rpc_call(&self, method: &str, params: Option<Value>) -> Result<Value, String> {
        self.rpc_call_internal(&self.long_client, method, params, FULL_RPC_ATTEMPTS)
            .await
    }

    /// Check backend health (5s timeout, silent on failure)
    pub async fn health(&self) -> HealthStatus {
        match self.rpc_call_quick("runtime.health", None).await {
            Some(val) => {
                #[cfg(debug_assertions)]
                if !val.is_object() {
                    eprintln!("health: unexpected response type: {:?}", val);
                }
                HealthStatus {
                    connected: true,
                    healthy: val["lifecycle"]["is_healthy"].as_bool().unwrap_or(false),
                    uptime: val["lifecycle"]["uptime_seconds"].as_u64().unwrap_or(0),
                    requests_per_minute: val["stats"]["requests_per_minute"]
                        .as_f64()
                        .unwrap_or(0.0),
                    success_rate: val["stats"]["success_rate"].as_f64().unwrap_or(0.0),
                    avg_latency_ms: val["stats"]["avg_latency_ms"].as_f64().unwrap_or(0.0),
                    backend_version: val
                        .pointer("/lifecycle/version")
                        .and_then(Value::as_str)
                        .or_else(|| val.get("version").and_then(Value::as_str))
                        .or_else(|| val.pointer("/meta/version").and_then(Value::as_str))
                        .or_else(|| val.pointer("/info/version").and_then(Value::as_str))
                        .map(ToString::to_string),
                    backend_build: val
                        .pointer("/lifecycle/build")
                        .and_then(Value::as_str)
                        .or_else(|| val.pointer("/lifecycle/build_id").and_then(Value::as_str))
                        .or_else(|| val.pointer("/meta/build").and_then(Value::as_str))
                        .or_else(|| val.pointer("/meta/git_commit").and_then(Value::as_str))
                        .or_else(|| val.pointer("/info/build").and_then(Value::as_str))
                        .or_else(|| val.pointer("/info/build_id").and_then(Value::as_str))
                        .map(ToString::to_string),
                }
            }
            None => HealthStatus {
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
        let phase_val = if phase.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::Value::String(phase.to_string())
        };

        let messages = if let Some(hist) = history {
            let mut msgs = hist;
            msgs.push(serde_json::json!({ "role": "user", "content": message }));
            msgs
        } else {
            vec![serde_json::json!({ "role": "user", "content": message })]
        };

        let mut body = serde_json::json!({
            "messages": messages,
            "mode": mode,
            "phase": phase_val,
        });

        if let Some(selected_model) = model.filter(|m| !m.trim().is_empty() && *m != "auto") {
            body["options"] = serde_json::json!({
                "model": selected_model,
            });
        }

        if let Some(ref extra) = options_extra {
            if body.get("options").is_none() {
                body["options"] = serde_json::json!({});
            }
            // Flatten extra values into options, NOT under "extra" key
            if let Some(obj) = extra.as_object() {
                for (k, v) in obj {
                    body["options"][k] = v.clone();
                }
            }
            // Include conversation tracking IDs if available
            if let Some(cid) = extra.get("conversation_id").and_then(|v| v.as_str()) {
                body["conversation_id"] = serde_json::json!(cid);
            }
            if let Some(bid) = extra.get("branch_id").and_then(|v| v.as_str()) {
                body["branch_id"] = serde_json::json!(bid);
            }
        }

        let mut last_err = String::new();
        let mut response = None;

        // Try /v1/chat/completions first (OpenAI-compatible), fall back to /chat/stream
        let endpoints = ["/v1/chat/completions", "/chat/stream"];
        for endpoint in &endpoints {
            for attempt in 1..=3 {
                match self
                    .long_client
                    .post(format!("{}{}", self.base_url, endpoint))
                    .json(&body)
                    .send()
                    .await
                {
                    Ok(resp) => {
                        if resp.status() == 404 && *endpoint == "/v1/chat/completions" {
                            // Endpoint not found — fall back to legacy
                            last_err = format!("endpoint {} not found, falling back", endpoint);
                            break;
                        }
                        response = Some(resp);
                        break;
                    }
                    Err(e) => {
                        last_err = format!("HTTP error: {}", e);
                        if attempt < 3 {
                            tokio::time::sleep(std::time::Duration::from_millis(
                                200 * attempt as u64,
                            ))
                            .await;
                            continue;
                        }
                    }
                }
            }
            if response.is_some() {
                break;
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

        // Reset abort controller in case it's being reused across requests
        if let Some(ref ctrl) = abort_ctrl {
            ctrl.reset();
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
                        "method": "request.cancel",
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
                            if let Some(text) = val
                                .get("token")
                                .or_else(|| val.get("text"))
                                .and_then(|v| v.as_str())
                            {
                                response_text.push_str(text);
                            }
                            if let Some(r) = val.get("reasoning").and_then(|v| v.as_str()) {
                                thinking_text.push_str(r);
                            }
                            // agent field may appear on the first event or the done event
                            if let Some(agent) = val
                                .get("agent")
                                .or_else(|| val.get("selected_agent"))
                                .and_then(|v| v.as_str())
                            {
                                if !agent.is_empty() {
                                    agent_text = agent.to_string();
                                }
                            }
                            if let Some(model) = val.get("selected_model").and_then(|v| v.as_str())
                            {
                                if !model.is_empty() {
                                    selected_model = Some(model.to_string());
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
            Ok((
                "(empty)".to_string(),
                String::new(),
                agent_text,
                selected_model,
            ))
        } else {
            Ok((response_text, thinking_text, agent_text, selected_model))
        }
    }

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

    pub async fn list_workflow_runs(
        &self,
        limit: usize,
        offset: usize,
        status: Option<&str>,
    ) -> Result<Value, String> {
        let mut params = serde_json::json!({"limit": limit, "offset": offset});
        if let Some(status) = status {
            params["status"] = Value::String(status.to_string());
        }
        self.rpc_call("workflow.run.list", Some(params)).await
    }

    pub async fn list_workflow_runs_typed(
        &self,
        limit: usize,
        offset: usize,
        status: Option<&str>,
    ) -> Result<Vec<WorkflowRunRecord>, String> {
        let value = self.list_workflow_runs(limit, offset, status).await?;
        Self::decode_workflow_runs(value)
    }

    pub async fn get_workflow_run(&self, run_id: &str) -> Result<Value, String> {
        self.rpc_call(
            "workflow.run.get",
            Some(serde_json::json!({"run_id": run_id})),
        )
        .await
    }

    pub async fn get_workflow_run_typed(&self, run_id: &str) -> Result<WorkflowRunRecord, String> {
        let value = self.get_workflow_run(run_id).await?;
        let candidate = value
            .get("run")
            .cloned()
            .or_else(|| value.get("result").and_then(|r| r.get("run")).cloned())
            .unwrap_or(value);
        serde_json::from_value::<WorkflowRunRecord>(candidate)
            .map_err(|e| format!("workflow.run.get decode error: {e}"))
    }

    fn decode_workflow_runs(value: Value) -> Result<Vec<WorkflowRunRecord>, String> {
        if let Ok(parsed) = serde_json::from_value::<WorkflowRunsResult>(value.clone()) {
            return Ok(parsed.runs);
        }

        if let Some(runs) = value.get("runs").and_then(Value::as_array) {
            return runs
                .iter()
                .cloned()
                .map(serde_json::from_value::<WorkflowRunRecord>)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("workflow.run.list decode error: {e}"));
        }

        if let Some(runs) = value
            .get("result")
            .and_then(|r| r.get("runs"))
            .and_then(Value::as_array)
        {
            return runs
                .iter()
                .cloned()
                .map(serde_json::from_value::<WorkflowRunRecord>)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("workflow.run.list decode error: {e}"));
        }

        if let Some(runs) = value.as_array() {
            return runs
                .iter()
                .cloned()
                .map(serde_json::from_value::<WorkflowRunRecord>)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("workflow.run.list decode error: {e}"));
        }

        Err("workflow.run.list decode error: unsupported payload shape".to_string())
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

    /// F-GAP-48: Reserved for future provider catalog browsing
    /// DEPRECATED: Unused. Provider metadata is currently hardcoded in app.rs `provider_meta()`.
    /// This RPC exists on the backend but the GUI does not call it.
    /// Retained for reference; remove in a future cleanup round.
    #[allow(dead_code)]
    pub async fn provider_catalog(&self) -> Result<Value, String> {
        self.rpc_call_quick("provider.catalog", None)
            .await
            .ok_or_else(|| "Failed to fetch provider catalog from backend".to_string())
    }

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRecord {
    pub name: Option<String>,
    pub description: Option<String>,
    pub version: Option<String>,
    pub enabled: Option<bool>,
    #[serde(alias = "importedAt")]
    pub imported_at: Option<u64>,
}
