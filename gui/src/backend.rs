use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

#[derive(Clone)]
pub struct BackendClient {
    /// Client for short-lived requests (health checks, probes - 5s timeout)
    quick_client: reqwest::Client,
    /// Client for long-lived requests (chat - 180s timeout)
    long_client: reqwest::Client,
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
        let quick_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_else(|e| {
                eprintln!("Failed to build quick HTTP client: {e}");
                reqwest::Client::new()
            });
        let long_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(180))
            .build()
            .unwrap_or_else(|e| {
                eprintln!("Failed to build long HTTP client: {e}");
                reqwest::Client::new()
            });
        Self {
            quick_client,
            long_client,
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    pub async fn fetch_models(&self) -> std::collections::HashMap<String, Vec<String>> {
        let resp = self.rpc_call("models.list", None).await;
        match resp {
            Ok(val) => {
                let mut result: std::collections::HashMap<String, Vec<String>> =
                    std::collections::HashMap::new();
                if let Some(models) = val.get("models").and_then(|m| m.as_array()) {
                    for m in models {
                        if let (Some(provider), Some(model_id)) = (
                            m.get("provider").and_then(|p| p.as_str()),
                            m.get("id").and_then(|id| id.as_str()),
                        ) {
                            result
                                .entry(provider.to_string())
                                .or_default()
                                .push(model_id.to_string());
                        }
                    }
                }
                result
            }
            Err(_) => std::collections::HashMap::new(),
        }
    }

    pub fn set_base_url(&mut self, url: &str) {
        self.base_url = url.trim_end_matches('/').to_string();
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Quick RPC call for health / status checks (5s timeout).
    /// Returns None if the backend is unreachable (no error message).
    async fn rpc_call_quick(&self, method: &str, params: Option<Value>) -> Option<Value> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params.unwrap_or(Value::Null),
        });
        let resp = self
            .quick_client
            .post(format!("{}/rpc", self.base_url))
            .json(&body)
            .send()
            .await
            .ok()?;
        let resp = resp.error_for_status().ok()?;
        let result: Value = resp.json().await.ok()?;
        if result.get("error").is_some() {
            return None;
        }
        result.get("result").cloned()
    }

    /// Full RPC call for normal requests (180s timeout).
    pub async fn rpc_call(&self, method: &str, params: Option<Value>) -> Result<Value, String> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params.unwrap_or(Value::Null),
        });
        let resp = self
            .long_client
            .post(format!("{}/rpc", self.base_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("HTTP error: {e}"))?;
        let resp = resp
            .error_for_status()
            .map_err(|e| format!("HTTP status error: {e}"))?;
        let result: Value = resp.json().await.map_err(|e| format!("JSON error: {e}"))?;
        if let Some(err) = result.get("error") {
            return Err(format!("{}", err));
        }
        Ok(result.get("result").cloned().unwrap_or(result))
    }

    /// Check backend health (5s timeout, silent on failure)
    pub async fn health(&self) -> HealthStatus {
        match self.rpc_call_quick("runtime.health", None).await {
            Some(val) => HealthStatus {
                connected: true,
                healthy: val["lifecycle"]["is_healthy"].as_bool().unwrap_or(false),
                uptime: val["lifecycle"]["uptime_seconds"].as_u64().unwrap_or(0),
                requests_per_minute: val["stats"]["requests_per_minute"].as_f64().unwrap_or(0.0),
                success_rate: val["stats"]["success_rate"].as_f64().unwrap_or(0.0),
                avg_latency_ms: val["stats"]["avg_latency_ms"].as_f64().unwrap_or(0.0),
            },
            None => HealthStatus {
                connected: false,
                healthy: false,
                uptime: 0,
                requests_per_minute: 0.0,
                success_rate: 0.0,
                avg_latency_ms: 0.0,
            },
        }
    }

    /// Get provider status (5s timeout, silent on failure)
    pub async fn provider_status(&self) -> Vec<ProviderStatus> {
        match self.rpc_call_quick("health.probes", None).await {
            Some(val) => {
                let probes = val.get("probes");
                let deps = probes
                    .and_then(|p| p.get("dependencies"))
                    .and_then(|d| d.as_array());
                if let Some(deps) = deps {
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
            None => Vec::new(),
        }
    }

    /// Send a chat message via /chat endpoint. Works on all platforms.
    /// Backend returns JSON with "response" and optionally "thinking" fields.
    /// Uses the long timeout client (180s) for AI provider response time.
    pub async fn chat(
        &self,
        message: &str,
        mode: &str,
        phase: &str,
        model: Option<&str>,
    ) -> Result<(String, String), String> {
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

        let resp = self
            .long_client
            .post(format!("{}/chat", self.base_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("HTTP error: {}", e))?;

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

        if response_text.is_empty() && thinking_text.is_empty() {
            Ok(("(empty)".to_string(), String::new()))
        } else {
            Ok((response_text, thinking_text))
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
