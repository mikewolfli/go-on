use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

#[derive(Clone)]
pub struct BackendClient {
    client: reqwest::Client,
    base_url: String,
}

#[derive(Debug, Clone)]
pub struct HealthStatus {
    pub connected: bool,
    pub healthy: bool,
    pub uptime: u64,
    pub requests_per_minute: f64,
    pub success_rate: f64,
    pub avg_latency_ms: f64,
}

#[derive(Debug, Clone)]
pub struct ProviderStatus {
    pub name: String,
    pub ready: bool,
    pub model: String,
}

impl BackendClient {
    pub fn new(base_url: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|e| {
                eprintln!("Failed to build HTTP client with custom timeout: {e}");
                reqwest::Client::new()
            });
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    /// Update the base URL at runtime (e.g. when user changes it in settings)
    pub fn set_base_url(&mut self, url: &str) {
        self.base_url = url.trim_end_matches('/').to_string();
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Call a JSON-RPC method on the backend
    pub async fn rpc_call(&self, method: &str, params: Option<Value>) -> Result<Value, String> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params.unwrap_or(Value::Null),
        });

        let resp = self
            .client
            .post(format!("{}/rpc", self.base_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("HTTP error: {e}"))?;

        // Check for HTTP-level errors (4xx, 5xx)
        let resp = resp
            .error_for_status()
            .map_err(|e| format!("HTTP status error: {e}"))?;

        let result: Value = resp.json().await.map_err(|e| format!("JSON error: {e}"))?;

        if let Some(err) = result.get("error") {
            return Err(format!("RPC error: {err}"));
        }

        Ok(result.get("result").cloned().unwrap_or(result))
    }

    /// Check backend health
    pub async fn health(&self) -> HealthStatus {
        let resp = self.rpc_call("runtime.health", None).await;
        match resp {
            Ok(val) => HealthStatus {
                connected: true,
                healthy: val["lifecycle"]["is_healthy"].as_bool().unwrap_or(false),
                uptime: val["lifecycle"]["uptime_seconds"].as_u64().unwrap_or(0),
                requests_per_minute: val["stats"]["requests_per_minute"].as_f64().unwrap_or(0.0),
                success_rate: val["stats"]["success_rate"].as_f64().unwrap_or(0.0),
                avg_latency_ms: val["stats"]["avg_latency_ms"].as_f64().unwrap_or(0.0),
            },
            Err(_) => HealthStatus {
                connected: false,
                healthy: false,
                uptime: 0,
                requests_per_minute: 0.0,
                success_rate: 0.0,
                avg_latency_ms: 0.0,
            },
        }
    }

    /// Get provider status
    pub async fn provider_status(&self) -> Vec<ProviderStatus> {
        let resp = self.rpc_call("health.probes", None).await;
        match resp {
            Ok(val) => {
                let probes = val.get("probes");
                let deps = probes
                    .and_then(|p| p.get("dependencies"))
                    .and_then(|d| d.as_array());
                if let Some(deps) = deps {
                    // Find the provider_dependency entry which contains the actual provider list
                    for dep in deps {
                        if dep.get("name").and_then(|n| n.as_str()) == Some("provider_dependency") {
                            if let Some(details) = dep.get("details") {
                                if let Some(api_map) =
                                    details.get("provider_api_map").and_then(|m| m.as_array())
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
            Err(_) => Vec::new(),
        }
    }

    /// Send a chat message. Returns (response_text, thinking_text).
    pub async fn chat(
        &self,
        message: &str,
        mode: &str,
        phase: &str,
    ) -> Result<(String, String), String> {
        let phase_val = if phase.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::Value::String(phase.to_string())
        };

        let params = serde_json::json!({
            "messages": [{"role": "user", "content": message}],
            "mode": mode,
            "phase": phase_val,
        });

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "chat",
            "params": params,
        });

        let resp = self
            .client
            .post(format!("{}/rpc", self.base_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e))?;

        let resp = resp
            .error_for_status()
            .map_err(|e| format!("HTTP error: {}", e))?;
        let text = resp
            .text()
            .await
            .map_err(|e| format!("read error: {}", e))?;

        // Parse the outer JSON-RPC response
        let outer: Value =
            serde_json::from_str(&text).map_err(|e| format!("JSON parse error: {}", e))?;

        // Check for JSON-RPC error
        if let Some(err) = outer.get("error") {
            return Err(format!(
                "RPC error: {}",
                err.get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown")
            ));
        }

        // Get the inner content
        // Try result wrapper first (standard JSON-RPC), then top-level raw
        let raw_text = if let Some(result) = outer.get("result") {
            // Try result.response first (non-streaming)
            if let Some(response) = result.get("response").and_then(|r| r.as_str()) {
                if !response.is_empty() {
                    return Ok((response.to_string(), String::new()));
                }
            }
            // Try result.raw
            match result.get("raw").and_then(|r| r.as_str()) {
                Some(raw) => raw.to_string(),
                None => return Ok(("(no response)".to_string(), String::new())),
            }
        } else if let Some(raw) = outer.get("raw").and_then(|r| r.as_str()) {
            // Top-level raw field (non-standard JSON-RPC, but backend uses this for SSE)
            raw.to_string()
        } else {
            return Ok(("(no response)".to_string(), String::new()));
        };

        // Parse SSE events from raw_text
        let mut response_text = String::new();
        let mut reasoning_text = String::new();
        for line in raw_text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(chunk) = serde_json::from_str::<Value>(trimmed) {
                if chunk.get("method").and_then(|m| m.as_str()) == Some("chat.stream.chunk") {
                    if let Some(params) = chunk.get("params") {
                        if let Some(token) = params.get("token").and_then(|t| t.as_str()) {
                            response_text.push_str(token);
                        }
                        if let Some(reasoning) = params.get("reasoning").and_then(|t| t.as_str()) {
                            reasoning_text.push_str(reasoning);
                        }
                    }
                }
            }
        }

        // Return (response_text, reasoning_text)
        if response_text.is_empty() && reasoning_text.is_empty() {
            Ok(("(empty)".to_string(), String::new()))
        } else {
            Ok((response_text, reasoning_text))
        }
    }

    /// Configure a provider on the backend (send API key so backend can use it)
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

    /// Restart the backend runtime (so it picks up new env vars)
    pub async fn restart_backend(&self) -> Result<Value, String> {
        self.rpc_call("runtime.restart", None).await
    }

    /// Create a new skill on the backend
    pub async fn create_skill(
        &self,
        name: &str,
        description: &str,
        prompt: &str,
        input_schema: &str,
    ) -> Result<Value, String> {
        let params = serde_json::json!({
            "name": name,
            "description": description,
            "prompt_template": prompt,
            "input_schema": input_schema,
        });
        self.rpc_call("skill.create", Some(params)).await
    }

    /// List imported skills from the backend
    pub async fn list_skills(&self) -> Result<Value, String> {
        self.rpc_call("skill.list_imported", None).await
    }

    /// Get config baseline from backend (includes phase list, providers, etc.)
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
