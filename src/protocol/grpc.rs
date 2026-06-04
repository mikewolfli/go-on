//! HTTP JSON-RPC service definitions for distributed execution (GAP-B54-052).
//!
//! Instead of full gRPC/tonic, this module provides lightweight JSON-RPC
//! over HTTP using `reqwest`. This keeps dependencies minimal while still
//! enabling remote node communication for DAG task dispatch.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::LazyLock;

/// Atomic counter for generating JSON-RPC 2.0 request IDs.
///
/// Per JSON-RPC 2.0 specification (§3.1), request IDs should be unique.
/// Using `Ordering::Relaxed` because the only requirement is that IDs
/// are non-zero and unique within the current process; there is no need
/// for strict happens-before ordering between different callers.
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// Generate a monotonically increasing request ID.
fn next_request_id() -> u64 {
    NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed)
}

/// Shared reqwest client reused across all gRPC calls to avoid creating
/// a new HTTP client (and TLS session) on every request.
static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("failed to build static reqwest::Client")
});

// ---------------------------------------------------------------------------
// JSON-RPC envelope types
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 version constant
// ---------------------------------------------------------------------------

/// The JSON-RPC protocol version string mandated by the 2.0 specification.
pub const JSONRPC_VERSION: &str = "2.0";

/// Validate that a JSON-RPC version string is `"2.0"` per the specification.
///
/// Returns `Ok(())` if the version is `"2.0"`, or `Err` with an `InvalidRequest`
/// error code (-32600) otherwise.
pub fn validate_jsonrpc_version(version: &str) -> Result<(), JsonRpcError> {
    if version == JSONRPC_VERSION {
        Ok(())
    } else {
        Err(JsonRpcError {
            code: -32600, // Invalid Request per JSON-RPC 2.0 spec
            message: format!(
                "invalid JSON-RPC version '{}'; expected '{}'",
                version, JSONRPC_VERSION
            ),
        })
    }
}

/// A JSON-RPC 2.0 request envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest<T: Serialize> {
    pub jsonrpc: String,
    pub method: String,
    pub params: T,
    pub id: u64,
}

impl<T: Serialize> JsonRpcRequest<T> {
    /// Create a new JSON-RPC 2.0 request with version validation.
    ///
    /// # Errors
    /// Returns `JsonRpcError` if the version is not `"2.0"`.
    pub fn new(method: impl Into<String>, params: T, id: u64) -> Result<Self, JsonRpcError> {
        let r = Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            method: method.into(),
            params,
            id,
        };
        Ok(r)
    }

    /// Validate the version field of an existing request.
    pub fn validate(&self) -> Result<(), JsonRpcError> {
        validate_jsonrpc_version(&self.jsonrpc)
    }
}

/// A JSON-RPC 2.0 response envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse<T> {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
    pub id: u64,
}

impl<T> JsonRpcResponse<T> {
    /// Create a new JSON-RPC 2.0 response.
    pub fn new(id: u64) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            result: None,
            error: None,
            id,
        }
    }

    /// Validate the version field of an existing response.
    pub fn validate(&self) -> Result<(), JsonRpcError> {
        validate_jsonrpc_version(&self.jsonrpc)
    }
}

/// A JSON-RPC error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
}

// ---------------------------------------------------------------------------
// Remote execution service definitions (mirror RemoteExecutor trait)
// ---------------------------------------------------------------------------

/// Parameters for the `execute` RPC method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteParams {
    pub node_id: String,
    pub dag_id: String,
    pub tool_name: String,
    pub input: serde_json::Value,
    pub dep_outputs: std::collections::HashMap<String, serde_json::Value>,
    pub retry_count: u32,
    pub max_retries: u32,
}

/// Result returned by a remote `execute` RPC call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteResult {
    pub node_id: String,
    pub dag_id: String,
    pub tool_name: String,
    pub success: bool,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
    pub duration_ms: u64,
    pub completed_at_ms: u64,
}

/// Parameters for the `health_check` RPC method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckParams {
    pub node_id: String,
}

/// Result of a health check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckResult {
    pub alive: bool,
    pub node_id: String,
}

// ---------------------------------------------------------------------------
// HTTP transport helper
// ---------------------------------------------------------------------------

/// Execute a remote task via HTTP JSON-RPC POST to `{base_url}/jsonrpc`.
///
/// This is the core transport used by `GrpcRemoteExecutor` to dispatch
/// `TaskPacket`s to remote nodes.
pub async fn call_execute_remote(
    base_url: &str,
    params: &ExecuteParams,
    _timeout_s: u64,
) -> Result<ExecuteResult, String> {
    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "execute".to_string(),
        params: params.clone(),
        id: next_request_id(),
    };

    let url = format!("{}/jsonrpc", base_url.trim_end_matches('/'));

    // Use module-level shared client with per-request timeout via tokio::time::timeout
    let response = CLIENT
        .post(&url)
        .json(&request)
        .send()
        .await
        .map_err(|e| format!("HTTP POST to {url} failed: {e}"))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!("HTTP {} from {url}", status.as_u16()));
    }

    let response_body: JsonRpcResponse<ExecuteResult> = response
        .json()
        .await
        .map_err(|e| format!("failed to decode JSON-RPC response: {e}"))?;

    if let Some(err) = response_body.error {
        return Err(format!("JSON-RPC error ({}): {}", err.code, err.message));
    }

    response_body
        .result
        .ok_or_else(|| "JSON-RPC response has neither result nor error".to_string())
}

/// Perform a health check against a remote node via HTTP JSON-RPC.
pub async fn call_health_check(
    base_url: &str,
    node_id: &str,
    _timeout_s: u64,
) -> Result<bool, String> {
    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "health_check".to_string(),
        params: HealthCheckParams {
            node_id: node_id.to_string(),
        },
        id: next_request_id(),
    };

    let url = format!("{}/jsonrpc", base_url.trim_end_matches('/'));

    // Use module-level shared client with per-request timeout via tokio::time::timeout
    let response = CLIENT
        .post(&url)
        .json(&request)
        .send()
        .await
        .map_err(|e| format!("health check HTTP POST failed: {e}"))?;

    let response_body: JsonRpcResponse<HealthCheckResult> = response
        .json()
        .await
        .map_err(|e| format!("failed to decode health check response: {e}"))?;

    if let Some(err) = response_body.error {
        return Err(format!(
            "health check JSON-RPC error ({}): {}",
            err.code, err.message
        ));
    }

    Ok(response_body.result.map(|r| r.alive).unwrap_or(false))
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

impl From<&crate::orchestration::distributed::remote_executor::TaskPacket> for ExecuteParams {
    fn from(packet: &crate::orchestration::distributed::remote_executor::TaskPacket) -> Self {
        Self {
            node_id: packet.node_id.to_string(),
            dag_id: packet.dag_id.to_string(),
            tool_name: packet.tool_name.clone(),
            input: packet.input.clone(),
            dep_outputs: packet
                .dep_outputs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
            retry_count: packet.retry_count,
            max_retries: packet.max_retries,
        }
    }
}

impl From<ExecuteResult> for crate::orchestration::distributed::remote_executor::NodeOutput {
    fn from(res: ExecuteResult) -> Self {
        Self {
            node_id: crate::orchestration::distributed::remote_executor::NodeId(res.node_id),
            dag_id: crate::orchestration::distributed::remote_executor::DagId(res.dag_id),
            tool_name: res.tool_name,
            success: res.success,
            output: res.output,
            error: res.error,
            duration_ms: res.duration_ms,
            completed_at_ms: res.completed_at_ms,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_rpc_envelope_serialization() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "execute".to_string(),
            params: ExecuteParams {
                node_id: "node-1".into(),
                dag_id: "dag-1".into(),
                tool_name: "tool-a".into(),
                input: serde_json::json!({"key": "value"}),
                dep_outputs: std::collections::HashMap::new(),
                retry_count: 0,
                max_retries: 3,
            },
            id: 42,
        };

        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"method\":\"execute\""));
        assert!(json.contains("\"id\":42"));

        // Round-trip
        let deserialized: JsonRpcRequest<ExecuteParams> = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.method, "execute");
        assert_eq!(deserialized.params.node_id, "node-1");
    }

    #[test]
    fn test_execute_result_conversion() {
        let result = ExecuteResult {
            node_id: "n1".into(),
            dag_id: "d1".into(),
            tool_name: "t1".into(),
            success: true,
            output: Some(serde_json::json!({"result": "ok"})),
            error: None,
            duration_ms: 42,
            completed_at_ms: 1000,
        };

        let output: crate::orchestration::distributed::remote_executor::NodeOutput = result.into();

        assert!(output.success);
        assert_eq!(output.node_id, "n1");
        assert_eq!(output.duration_ms, 42);
    }
}
