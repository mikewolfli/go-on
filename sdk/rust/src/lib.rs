//! Rust SDK for go-on — Phase 4, 14-bus architecture, 21 F-GAP modules.
//!
//! Provides typed async clients for ACP JSON-RPC endpoints:
//!   - Runtime: health, initialize, shutdown
//!   - Governance: status, plan, audit
//!   - Observability: metrics, trace, health probes
//!   - Reliability: breaker, checkpoint, maintenance
//!   - Workflow / Task: execute, plan
//!   - Learning / Intelligence: summary, selector, knowledge, rl
//!   - Optimization / Operations: cost, config baseline, harness
//!
//! All methods send JSON-RPC 2.0 requests to `POST {base_url}/v1/responses`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum SdkError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON-RPC error: code={code}, message={message}")]
    JsonRpc { code: i64, message: String },

    #[error("unexpected response shape: {0}")]
    UnexpectedShape(String),
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub uptime_seconds: u64,
    #[serde(default)]
    pub modules: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GovernanceStatusResponse {
    pub ok: bool,
    pub governance: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthProbesResponse {
    pub modules: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsResponse {
    pub metrics: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BreakerStatusResponse {
    pub breakers: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointListResponse {
    pub checkpoints: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPlanResponse {
    pub plan: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningSummaryResponse {
    pub summary: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectorStatusResponse {
    pub selector: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostStatusResponse {
    pub cost: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigBaselineResponse {
    pub baseline: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessStatusResponse {
    pub harness: Value,
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct GoOnClient {
    base_url: String,
    http: reqwest::Client,
}

impl GoOnClient {
    /// Create a new client targeting the go-on HTTP endpoint.
    ///
    /// Example:
    /// ```ignore
    /// let client = GoOnClient::new("http://127.0.0.1:8090");
    /// ```
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
            http: reqwest::Client::new(),
        }
    }

    // ── Internal helpers ──────────────────────────────────────────────

    async fn json_rpc(&self, method: &str, params: Value) -> Result<Value, SdkError> {
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
        });
        let resp: Value = self
            .http
            .post(format!("{}/v1/responses", self.base_url))
            .json(&payload)
            .send()
            .await?
            .json()
            .await?;

        if let Some(err) = resp.get("error") {
            return Err(SdkError::JsonRpc {
                code: err.get("code").and_then(Value::as_i64).unwrap_or(-1),
                message: err
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string(),
            });
        }
        Ok(resp.get("result").cloned().unwrap_or(Value::Null))
    }

    fn extract<T>(&self, result: Value) -> Result<T, SdkError>
    where
        T: serde::de::DeserializeOwned,
    {
        serde_json::from_value(result).map_err(|e| SdkError::UnexpectedShape(e.to_string()))
    }

    // ── Core Runtime ──────────────────────────────────────────────────

    /// GET /health — quick health check.
    pub async fn health(&self) -> Result<HealthResponse, SdkError> {
        let resp: Value = self
            .http
            .get(format!("{}/health", self.base_url))
            .send()
            .await?
            .json()
            .await?;
        self.extract(resp)
    }

    /// runtime.health — full runtime health via JSON-RPC.
    pub async fn runtime_health(&self) -> Result<HealthResponse, SdkError> {
        let result = self
            .json_rpc("runtime.health", serde_json::json!({}))
            .await?;
        self.extract(result)
    }

    /// runtime.stability — runtime stability snapshot.
    pub async fn runtime_stability(&self) -> Result<Value, SdkError> {
        self.json_rpc("runtime.stability", serde_json::json!({}))
            .await
    }

    /// initialize — initialize the runtime.
    pub async fn initialize(&self, setup_level: &str) -> Result<Value, SdkError> {
        self.json_rpc(
            "initialize",
            serde_json::json!({ "setup_level": setup_level }),
        )
        .await
    }

    /// shutdown — gracefully shut down the runtime.
    pub async fn shutdown(&self) -> Result<Value, SdkError> {
        self.json_rpc("shutdown", serde_json::json!({})).await
    }

    // ── Governance ────────────────────────────────────────────────────

    /// governance.status — full governance status (~120+ capability profiles).
    pub async fn governance_status(&self) -> Result<GovernanceStatusResponse, SdkError> {
        let result = self
            .json_rpc("governance.status", serde_json::json!({}))
            .await?;
        self.extract(result)
    }

    /// governance.plan.get — get active governance plan.
    pub async fn governance_plan_get(&self) -> Result<Value, SdkError> {
        self.json_rpc("governance.plan.get", serde_json::json!({}))
            .await
    }

    /// governance.audit.recent — view recent audit entries.
    pub async fn governance_audit_recent(&self, limit: u32) -> Result<Value, SdkError> {
        self.json_rpc(
            "governance.audit.recent",
            serde_json::json!({ "limit": limit }),
        )
        .await
    }

    // ── Observability ─────────────────────────────────────────────────

    /// health.probes — module-level health probes (Phase 4: harness_bus + capability_bus).
    pub async fn health_probes(&self) -> Result<HealthProbesResponse, SdkError> {
        let result = self
            .json_rpc("health.probes", serde_json::json!({}))
            .await?;
        self.extract(result)
    }

    /// metrics.get — get current runtime metrics.
    pub async fn metrics_get(&self) -> Result<MetricsResponse, SdkError> {
        let result = self.json_rpc("metrics.get", serde_json::json!({})).await?;
        self.extract(result)
    }

    /// metrics.prometheus — get Prometheus-formatted metrics.
    pub async fn metrics_prometheus(&self) -> Result<String, SdkError> {
        let result = self
            .json_rpc("metrics.prometheus", serde_json::json!({}))
            .await?;
        Ok(result.as_str().unwrap_or("").to_string())
    }

    /// trace.get — get trace entries.
    pub async fn trace_get(&self, limit: u32) -> Result<Value, SdkError> {
        self.json_rpc("trace.get", serde_json::json!({ "limit": limit }))
            .await
    }

    // ── Reliability ───────────────────────────────────────────────────

    /// breaker.status — get circuit breaker status.
    pub async fn breaker_status(&self) -> Result<BreakerStatusResponse, SdkError> {
        let result = self
            .json_rpc("breaker.status", serde_json::json!({}))
            .await?;
        self.extract(result)
    }

    /// breaker.reset — reset a circuit breaker.
    pub async fn breaker_reset(&self, name: &str) -> Result<Value, SdkError> {
        self.json_rpc("breaker.reset", serde_json::json!({ "name": name }))
            .await
    }

    /// maintenance.gc — run garbage collection.
    pub async fn maintenance_gc(&self) -> Result<Value, SdkError> {
        self.json_rpc("maintenance.gc", serde_json::json!({})).await
    }

    // ── Checkpoint (Phase 4) ──────────────────────────────────────────

    /// checkpoint.create — create a runtime checkpoint.
    pub async fn checkpoint_create(&self, branch: &str) -> Result<Value, SdkError> {
        self.json_rpc("checkpoint.create", serde_json::json!({ "branch": branch }))
            .await
    }

    /// checkpoint.list — list available checkpoints.
    pub async fn checkpoint_list(&self) -> Result<CheckpointListResponse, SdkError> {
        let result = self
            .json_rpc("checkpoint.list", serde_json::json!({}))
            .await?;
        self.extract(result)
    }

    /// conversation.rollback — roll back to a checkpoint.
    pub async fn conversation_rollback(&self, checkpoint_id: &str) -> Result<Value, SdkError> {
        self.json_rpc(
            "conversation.rollback",
            serde_json::json!({ "checkpoint_id": checkpoint_id }),
        )
        .await
    }

    // ── Workflow / Task ───────────────────────────────────────────────

    /// workflow.execute — execute the current workflow.
    pub async fn workflow_execute(&self) -> Result<Value, SdkError> {
        self.json_rpc("workflow.execute", serde_json::json!({}))
            .await
    }

    /// task.plan — plan a task.
    pub async fn task_plan(&self, description: &str) -> Result<TaskPlanResponse, SdkError> {
        let result = self
            .json_rpc(
                "task.plan",
                serde_json::json!({ "description": description }),
            )
            .await?;
        self.extract(result)
    }

    /// task.execute — execute a planned task.
    pub async fn task_execute(&self, plan_id: &str) -> Result<Value, SdkError> {
        self.json_rpc("task.execute", serde_json::json!({ "plan_id": plan_id }))
            .await
    }

    // ── Learning / Intelligence ───────────────────────────────────────

    /// learning.summary — get learning loop summary.
    pub async fn learning_summary(&self) -> Result<LearningSummaryResponse, SdkError> {
        let result = self
            .json_rpc("learning.summary", serde_json::json!({}))
            .await?;
        self.extract(result)
    }

    /// selector.status — get model selector status.
    pub async fn selector_status(&self) -> Result<SelectorStatusResponse, SdkError> {
        let result = self
            .json_rpc("selector.status", serde_json::json!({}))
            .await?;
        self.extract(result)
    }

    /// knowledge.distill — run knowledge distillation.
    pub async fn knowledge_distill(&self, source: &str) -> Result<Value, SdkError> {
        self.json_rpc("knowledge.distill", serde_json::json!({ "source": source }))
            .await
    }

    /// rl.alignment.offline_eval — run RL alignment offline evaluation.
    pub async fn rl_alignment_offline_eval(&self) -> Result<Value, SdkError> {
        self.json_rpc("rl.alignment.offline_eval", serde_json::json!({}))
            .await
    }

    // ── Optimization / Operations ─────────────────────────────────────

    /// cost.status — get cost optimization status.
    pub async fn cost_status(&self) -> Result<CostStatusResponse, SdkError> {
        let result = self.json_rpc("cost.status", serde_json::json!({})).await?;
        self.extract(result)
    }

    /// config.baseline — get config baseline snapshot.
    pub async fn config_baseline(&self) -> Result<ConfigBaselineResponse, SdkError> {
        let result = self
            .json_rpc("config.baseline", serde_json::json!({}))
            .await?;
        self.extract(result)
    }

    /// config.reload — reload runtime config.
    pub async fn config_reload(&self) -> Result<Value, SdkError> {
        self.json_rpc("config.reload", serde_json::json!({})).await
    }

    /// harness.status — get test harness status.
    pub async fn harness_status(&self) -> Result<HarnessStatusResponse, SdkError> {
        let result = self
            .json_rpc("harness.status", serde_json::json!({}))
            .await?;
        self.extract(result)
    }
}
