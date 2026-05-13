use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

const QUICK_RPC_ATTEMPTS: usize = 2;
const FULL_RPC_ATTEMPTS: usize = 3;
const MODELS_CACHE_TTL_SECS: u64 = 300;

type ProviderModels = std::collections::HashMap<String, Vec<String>>;
type ModelsCacheState = (Option<ProviderModels>, std::time::Instant);
type ModelsCache = Arc<std::sync::Mutex<ModelsCacheState>>;

#[derive(Clone)]
pub struct BackendClient {
    /// Client for short-lived requests (health checks, probes - 5s timeout)
    quick_client: reqwest::Client,
    /// Client for long-lived requests (chat - 180s timeout)
    long_client: reqwest::Client,
    base_url: String,
    /// Model list cache with timestamp
    models_cache: ModelsCache,
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
            models_cache: Arc::new(std::sync::Mutex::new((None, std::time::Instant::now()))),
        }
    }

    pub async fn fetch_models(&self) -> ProviderModels {
        let mut stale_cached: Option<ProviderModels> = None;

        // Check cache (valid for 5 minutes)
        if let Ok(cache) = self.models_cache.lock() {
            let (cached_models, timestamp) = &*cache;
            if let Some(models) = cached_models {
                if timestamp.elapsed().as_secs() < MODELS_CACHE_TTL_SECS {
                    return models.clone();
                }
                stale_cached = Some(models.clone());
            }
        }

        let resp = self.rpc_call("models.list", None).await;
        let result = match resp {
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
                    models.sort();
                    models.dedup();
                }

                result
            }
            Err(_) => {
                // If refresh fails, keep showing stale data instead of blanking the model list.
                return stale_cached.unwrap_or_default();
            }
        };

        // Update cache only on successful refresh.
        if let Ok(mut cache) = self.models_cache.lock() {
            cache.0 = Some(result.clone());
            cache.1 = std::time::Instant::now();
        }

        result
    }

    pub fn set_base_url(&mut self, url: &str) {
        self.base_url = url.trim_end_matches('/').to_string();
        if let Ok(mut cache) = self.models_cache.lock() {
            cache.0 = None;
            cache.1 = std::time::Instant::now();
        }
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
        // Exponential backoff with jitter to prevent thundering herd:
        // Attempt 1: [100,200), Attempt 2: [200,400), Attempt 3+: [400,800)
        let (base_ms, span_ms): (u64, u64) = match attempt {
            1 => (100, 100),
            2 => (200, 200),
            _ => (400, 400),
        };
        let jitter = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| (d.as_nanos() as u64) % span_ms)
            .unwrap_or(0);
        Duration::from_millis(base_ms + jitter)
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
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params.unwrap_or(Value::Null),
        });
        let url = format!("{}/rpc", self.base_url);
        let mut last_err = String::new();

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

            let status = response.status();
            let response_text = match response.text().await {
                Ok(text) => text,
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

    pub async fn chat_with_options(
        &self,
        message: &str,
        mode: &str,
        phase: &str,
        model: Option<&str>,
        options_extra: Option<Value>,
    ) -> Result<(String, String, String), String> {
        let phase_val = if phase.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::Value::String(phase.to_string())
        };

        let mut body = serde_json::json!({
            "messages": [{"role": "user", "content": message}],
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
        for attempt in 1..=3 {
            match self
                .long_client
                .post(format!("{}/chat", self.base_url))
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
        let value: Value = resp
            .json()
            .await
            .map_err(|e| format!("JSON parse error: {}", e))?;

        if let Some(err_msg) = value.get("error").and_then(|e| e.as_str()) {
            return Err(format!("Chat error: {}", err_msg));
        }

        let response_text = value
            .get("response")
            .and_then(|r| r.as_str())
            .unwrap_or("")
            .to_string();

        let thinking_text = value
            .get("thinking")
            .and_then(|r| r.as_str())
            .unwrap_or("")
            .to_string();

        let agent_text = value
            .get("agent")
            .or_else(|| value.get("selected_agent"))
            .or_else(|| value.pointer("/capability_routing/selected_agent"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        if response_text.is_empty() && thinking_text.is_empty() {
            Ok(("(empty)".to_string(), String::new(), agent_text))
        } else {
            Ok((response_text, thinking_text, agent_text))
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
    pub imported_at: Option<u64>,
}
