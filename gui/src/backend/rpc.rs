use serde_json::Value;
use std::sync::atomic::Ordering;
use std::time::Duration;

use super::validate_input_size;
use super::BackendClient;

// Unified strategy: bounded retries with exponential backoff + jitter + 30s max interval.
// 20 attempts @ 30s max cap ≈ 5 minutes of retrying before giving up.
// See contracts/cross-client-sync.md for the full specification.
pub(super) const QUICK_RPC_ATTEMPTS: usize = 20;
pub(super) const FULL_RPC_ATTEMPTS: usize = 20;

impl BackendClient {
    fn is_retryable_status(status: reqwest::StatusCode) -> bool {
        // Unified retryable status set across all four clients (Rust/Python/TS
        // SDKs + GUI): 408 (Request Timeout) + 429 (Too Many Requests) + all
        // 5xx. See contracts/cross-client-sync.md.
        status.is_server_error() || status.as_u16() == 408 || status.as_u16() == 429
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
        // Unified exponential backoff with 30% jitter, capped at 30s:
        // delay = min(1000 * 2^(attempt-1), 30000) * (0.7 + random * 0.3)
        // Attempt 1: ~1s, 2: ~2s, 3: ~4s, 4: ~8s, 5: ~16s, 6+: ~30s
        // See contracts/cross-client-sync.md for the full specification.
        let capped_ms =
            crate::backoff::exp_backoff_ms(1000, attempt.saturating_sub(1) as u32, 30_000);
        let jitter_factor = 0.7 + fastrand::f64() * 0.3;
        Duration::from_secs_f64((capped_ms as f64 * jitter_factor) / 1000.0)
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
                        #[cfg(debug_assertions)]
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
                        #[cfg(debug_assertions)]
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
                    #[cfg(debug_assertions)]
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
                        #[cfg(debug_assertions)]
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
                    #[cfg(debug_assertions)]
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
    pub(crate) async fn rpc_call_quick(
        &self,
        method: &str,
        params: Option<Value>,
    ) -> Option<Value> {
        self.rpc_call_internal(&self.quick_client, method, params, QUICK_RPC_ATTEMPTS)
            .await
            .ok()
    }

    /// Full RPC call for normal requests (180s timeout).
    pub async fn rpc_call(&self, method: &str, params: Option<Value>) -> Result<Value, String> {
        self.rpc_call_internal(&self.long_client, method, params, FULL_RPC_ATTEMPTS)
            .await
    }

    pub(crate) fn decode_workflow_runs(
        value: Value,
    ) -> Result<Vec<super::WorkflowRunRecord>, String> {
        if let Ok(parsed) = serde_json::from_value::<super::WorkflowRunsResult>(value.clone()) {
            return Ok(parsed.runs);
        }

        if let Some(runs) = value.get("runs").and_then(Value::as_array) {
            return runs
                .iter()
                .cloned()
                .map(serde_json::from_value::<super::WorkflowRunRecord>)
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
                .map(serde_json::from_value::<super::WorkflowRunRecord>)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("workflow.run.list decode error: {e}"));
        }

        if let Some(runs) = value.as_array() {
            return runs
                .iter()
                .cloned()
                .map(serde_json::from_value::<super::WorkflowRunRecord>)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("workflow.run.list decode error: {e}"));
        }

        Err("workflow.run.list decode error: unsupported payload shape".to_string())
    }
}
