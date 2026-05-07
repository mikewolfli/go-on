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
                    deps.iter()
                        .filter_map(|d| {
                            let name = d.get("name")?.as_str()?;
                            let details = d.get("details")?;
                            Some(ProviderStatus {
                                name: name.to_string(),
                                ready: details
                                    .get("ready")
                                    .and_then(serde_json::Value::as_u64)
                                    .unwrap_or(0)
                                    > 0,
                                model: details
                                    .get("model")
                                    .and_then(|m| m.as_str())
                                    .unwrap_or("")
                                    .to_string(),
                            })
                        })
                        .collect()
                } else {
                    Vec::new()
                }
            }
            Err(_) => Vec::new(),
        }
    }

    /// Send a chat message
    pub async fn chat(&self, message: &str, mode: &str, phase: &str) -> Result<String, String> {
        let params = serde_json::json!({
            "messages": [{"role": "user", "content": message}],
            "mode": mode,
            "phase": phase
        });
        let resp = self.rpc_call("chat", Some(params)).await?;
        Ok(resp["response"]
            .as_str()
            .unwrap_or("No response")
            .to_string())
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRecord {
    pub name: Option<String>,
    pub description: Option<String>,
    pub version: Option<String>,
    pub enabled: Option<bool>,
    pub imported_at: Option<u64>,
}
