//! GoOnClient and GoOnClientBuilder for the go-on Rust SDK.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures::Stream;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::StreamExt;

use crate::error::SdkError;
use crate::types::*;

/// Atomic counter for unique JSON-RPC request IDs.
static NEXT_RPC_ID: AtomicU64 = AtomicU64::new(1);

// ---------------------------------------------------------------------------
// GoOnClientBuilder
// ---------------------------------------------------------------------------

/// Builder for constructing a [`GoOnClient`] with custom configuration.
///
/// # Example
///
/// ```ignore
/// use std::time::Duration;
/// use go_on_sdk::GoOnClientBuilder;
///
/// let client = GoOnClientBuilder::new("http://127.0.0.1:8090")
///     .with_timeout(Duration::from_secs(15))
///     .with_max_retries(5)
///     .build()
///     .expect("failed to build client");
/// ```
pub struct GoOnClientBuilder {
    base_url: String,
    timeout: Duration,
    max_retries: u32,
    retry_delay: Duration,
}

impl GoOnClientBuilder {
    /// Create a new builder targeting the go-on HTTP endpoint.
    pub fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.to_string(),
            timeout: Duration::from_secs(30),
            max_retries: 3,
            retry_delay: Duration::from_secs(1),
        }
    }

    /// Set the per-request timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set the maximum number of retry attempts for transient failures.
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Set the delay between retry attempts.
    pub fn with_retry_delay(mut self, retry_delay: Duration) -> Self {
        self.retry_delay = retry_delay;
        self
    }

    /// Consume the builder and produce a [`GoOnClient`].
    pub fn build(self) -> Result<GoOnClient, SdkError> {
        let http = reqwest::Client::builder()
            .timeout(self.timeout)
            .build()
            .map_err(SdkError::Http)?;

        Ok(GoOnClient {
            base_url: self.base_url,
            http,
            timeout: Some(self.timeout),
            max_retries: self.max_retries,
            retry_delay: self.retry_delay,
        })
    }
}

// ---------------------------------------------------------------------------
// GoOnClient
// ---------------------------------------------------------------------------

/// Async client for go-on ACP JSON-RPC endpoints.
///
/// Targets `POST {base_url}/v1/responses` for JSON-RPC calls
/// and direct HTTP GET for `/health`.
///
/// Phase 4 coverage: runtime, governance, observability, reliability,
/// checkpoint, workflow, learning, optimization, and streaming chat.
#[derive(Debug, Clone)]
pub struct GoOnClient {
    base_url: String,
    http: reqwest::Client,
    timeout: Option<Duration>,
    max_retries: u32,
    retry_delay: Duration,
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
            timeout: Some(Duration::from_secs(30)),
            max_retries: 0,
            retry_delay: Duration::from_millis(500),
        }
    }

    /// Create a client from a builder (public for ergonomics; prefer `GoOnClientBuilder`).
    pub fn from_builder(builder: GoOnClientBuilder) -> Result<Self, SdkError> {
        builder.build()
    }

    // ── Streaming chat ────────────────────────────────────────────────

    /// Send a chat request and receive the response as a real-time SSE stream
    /// of JSON chunks, using `reqwest::Response::bytes_stream()` under the hood
    /// with `tokio::sync::mpsc` channel for lock-free chunk delivery.
    ///
    /// Each item in the returned stream is a `Result<Value, SdkError>`.
    /// The outer `Result` covers the initial HTTP handshake; the inner ones
    /// cover per-chunk parse errors.
    ///
    /// ```ignore
    /// use go_on_sdk::{ChatMessage, ChatRequest};
    /// use tokio_stream::StreamExt;
    ///
    /// let request = ChatRequest {
    ///     messages: vec![ChatMessage {
    ///         role: "user".into(),
    ///         content: "Hello!".into(),
    ///     }],
    ///     model: None,
    ///     temperature: None,
    ///     max_tokens: None,
    ///     stream: Some(true),
    /// };
    ///
    /// let mut stream = client.chat_stream(request).await?;
    /// while let Some(chunk) = stream.next().await {
    ///     match chunk {
    ///         Ok(val) => println!("chunk: {val}"),
    ///         Err(e) => eprintln!("error: {e}"),
    ///     }
    /// }
    /// # Ok::<_, go_on_sdk::SdkError>(())
    /// ```
    pub async fn chat_stream(
        &self,
        request: ChatRequest,
    ) -> Result<impl Stream<Item = Result<Value, SdkError>>, SdkError> {
        let mut req = self.http.post(format!("{}/acp/chat", self.base_url));

        if let Some(timeout) = self.timeout {
            req = req.timeout(timeout);
        }

        let response = req.json(&request).send().await.map_err(SdkError::Http)?;

        let (tx, rx) = mpsc::channel::<Result<Value, SdkError>>(256);

        // Spawn a background task that reads the byte stream, parses SSE
        // frames delimited by \n\n, and sends parsed events through the channel.
        tokio::spawn(async move {
            let mut byte_stream = response.bytes_stream();
            let mut sse_buf = String::with_capacity(4096);

            while let Some(chunk_result) = byte_stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = tx
                            .send(Err(SdkError::Stream(format!("http stream: {e}"))))
                            .await;
                        return;
                    }
                };

                // Normalize CRLF -> LF for consistent frame splitting
                let part = String::from_utf8_lossy(&chunk);
                sse_buf.push_str(&part.replace("\r", ""));

                // Process complete SSE frames (delimited by \n\n)
                while let Some(split_at) = sse_buf.find("\n\n") {
                    let frame = sse_buf[..split_at].to_string();
                    sse_buf.drain(..split_at + 2);

                    for line in frame.lines() {
                        let trimmed = line.trim();
                        if let Some(data) = trimmed.strip_prefix("data: ") {
                            let data = data.trim();
                            if data == "[DONE]" {
                                continue;
                            }
                            match serde_json::from_str::<Value>(data) {
                                Ok(val) => {
                                    if tx.send(Ok(val)).await.is_err() {
                                        // Receiver dropped; stop processing
                                        return;
                                    }
                                }
                                Err(e) => {
                                    let _ = tx
                                        .send(Err(SdkError::Stream(format!("json parse: {e}"))))
                                        .await;
                                    return;
                                }
                            }
                        }
                    }
                }
            }
        });

        Ok(ReceiverStream::new(rx))
    }

    // ── Internal helpers ──────────────────────────────────────────────

    async fn json_rpc(&self, method: &str, params: Value) -> Result<Value, SdkError> {
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": NEXT_RPC_ID.fetch_add(1, Ordering::Relaxed),
            "method": method,
            "params": params,
        });

        let mut last_error = None;

        for attempt in 0..=self.max_retries {
            let mut req = self.http.post(format!("{}/v1/responses", self.base_url));

            if let Some(timeout) = self.timeout {
                req = req.timeout(timeout);
            }

            match req.json(&payload).send().await {
                Ok(resp) => match resp.json::<Value>().await {
                    Ok(val) => {
                        if let Some(err) = val.get("error") {
                            return Err(SdkError::JsonRpc {
                                code: err.get("code").and_then(Value::as_i64).unwrap_or(-1),
                                message: err
                                    .get("message")
                                    .and_then(Value::as_str)
                                    .unwrap_or("unknown")
                                    .to_string(),
                            });
                        }
                        return Ok(val.get("result").cloned().unwrap_or(Value::Null));
                    }
                    Err(e) => {
                        last_error = Some(SdkError::Http(e));
                    }
                },
                Err(e) => {
                    last_error = Some(SdkError::Http(e));
                }
            }

            if attempt < self.max_retries {
                tokio::time::sleep(self.retry_delay).await;
            }
        }

        Err(last_error.unwrap_or_else(|| {
            SdkError::UnexpectedShape(format!(
                "retries exhausted after {} attempts",
                self.max_retries + 1
            ))
        }))
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
        let mut req = self.http.get(format!("{}/health", self.base_url));

        if let Some(timeout) = self.timeout {
            req = req.timeout(timeout);
        }

        let resp: Value = req.send().await?.json().await?;
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
        result.as_str().map(|s| s.to_string()).ok_or_else(|| {
            SdkError::UnexpectedShape("metrics.prometheus returned non-string value".to_string())
        })
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
