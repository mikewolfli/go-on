use serde_json::{json, Value};
use std::time::Duration;
use tauri::State;

use crate::state::AppState;

const RPC_HTTP_TIMEOUT_SECS: u64 = 25;

#[tauri::command]
pub fn invoke_runtime_rpc(
    _state: State<'_, AppState>,
    method: String,
    params_json: Option<String>,
) -> Result<String, String> {
    if method.trim().is_empty() {
        return Err("method cannot be empty".to_string());
    }

    let params_value = if let Some(raw) = params_json {
        if raw.trim().is_empty() {
            json!({})
        } else {
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed == "{}" {
                json!({})
            } else {
                serde_json::from_str::<Value>(trimmed)
                    .map_err(|e| format!("invalid params JSON: {e}"))?
            }
        }
    } else {
        json!({})
    };

    let req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params_value
    });

    // Determine the backend endpoint — use state's working_dir to infer the base URL,
    // but default to 127.0.0.1:8090 which is the standard acp_http bind address.
    let endpoint = "http://127.0.0.1:8090/rpc";

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(RPC_HTTP_TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("failed to build HTTP client: {e}"))?;

    let response = client
        .post(endpoint)
        .json(&req)
        .send()
        .map_err(|e| format!("RPC HTTP request failed: {e}"))?;

    let status = response.status();
    let body: Value = response
        .json()
        .map_err(|e| format!("failed to parse RPC response: {e}"))?;

    if !status.is_success() {
        let error_msg = body
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown HTTP error");
        return Err(format!("rpc_http_error:{}:{}", status.as_u16(), error_msg));
    }

    // Check for JSON-RPC error in the response body
    if let Some(err) = body.get("error") {
        let code = err.get("code").and_then(|x| x.as_i64()).unwrap_or(-1);
        let message = err
            .get("message")
            .and_then(|x| x.as_str())
            .unwrap_or("unknown rpc error");
        let data = err.get("data");
        let kind = data
            .and_then(|d| d.get("kind"))
            .and_then(|k| k.as_str())
            .map(|k| k.to_string())
            .unwrap_or_else(|| {
                let lower = message.to_ascii_lowercase();
                if lower.contains("pua") {
                    "PuaViolation".to_string()
                } else if lower.contains("budget") {
                    "BudgetExceeded".to_string()
                } else if lower.contains("sandbox") || lower.contains("hardening policy denied") {
                    "SandboxBlocked".to_string()
                } else {
                    "GeneralError".to_string()
                }
            });
        let context = data
            .and_then(|d| d.get("detail"))
            .and_then(|detail| detail.as_str())
            .filter(|detail| detail.contains("acp.handle_request.dispatch"))
            .map(|_| "acp.handle_request.dispatch")
            .unwrap_or("none");
        return Err(format!(
            "rpc_error:{code}:{kind}:{message} (context={context})"
        ));
    }

    let payload = body.get("result").cloned().unwrap_or(body);
    Ok(serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string()))
}
