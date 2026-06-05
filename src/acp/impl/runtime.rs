//! Runtime implementation functions for ACP server
//!
//! This module contains standalone functions that implement the core runtime
//! functionality previously in the `impl AcpServer` block.
//! These functions take `AcpServer` as their first parameter to maintain
//! compatibility with the original implementation.

use std::mem;
use std::net::SocketAddr;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

/// Serializes concurrent `/rpc` calls to prevent pipe-swapping race conditions.
/// `server.output` is a global singleton — without this guard, two concurrent
/// `/rpc` requests would corrupt each other's response capture pipes.
static RPC_SERIAL: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

#[allow(dead_code)]
/// Limits concurrent ACP HTTP connections to prevent unbounded tokio task growth.
/// TODO-BLUE64: Extract http_server module uses this; kept for compatibility.
static CONNECTION_SEMAPHORE: Semaphore = Semaphore::const_new(1000);

use anyhow::Result;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::signal;
use tokio::sync::Semaphore;
use tracing::{debug, error, info, warn};

use crate::acp::background::start_background_tasks;
pub mod auth;
pub mod cors;
pub(crate) mod http_server;
pub(crate) mod server_builder;
pub(crate) mod tls;

use crate::acp::r#impl::cors::{
    build_cors_headers, build_preflight_response_headers, is_origin_allowed,
};
use crate::acp::r#impl::io::send_error;
use crate::acp::r#impl::request::{handle_request, inject_platform_profiles_if_absent};
use crate::i18n::runtime::{t, tf};

use crate::acp::server::AcpServer;
use crate::agent::AgentRegistry;
use crate::config::{AutoTuneState, VectorConfig};
use crate::flow::FlowManager;
use crate::governance::rbac::{AccessDecision, Permission, Principal};
use crate::rpc_protocol::{chat_trace_context, JsonRpcRequest, RequestTraceContext};
use crate::shared::secret_override::get_secret;
use crate::vector::VectorStore;

static RESPONSES_ID_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn next_responses_api_id(prefix: &str) -> String {
    let seq = RESPONSES_ID_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{}_{}_{}", prefix, crate::acp::prelude::now_ts_ms(), seq)
}

// Re-export server builder functions from server_builder module
pub(crate) use server_builder::new_acp_server;
// wire_server is used internally by new_acp_server within server_builder

/// Run the ACP server
///
/// This function replaces the `AcpServer::run` method.
pub async fn run_acp_server(server: &mut AcpServer) -> Result<()> {
    info!("ACP server starting");

    // GAP-B58-B13: Wire memory bridge — run initial promotion on startup
    if let Some(mp) = server.governance_deps.memory_persistence.as_ref() {
        let memory_store = &server.memory_store;
        match crate::memory::memory_bridge::bridge_promote(memory_store, mp) {
            Ok(report) => {
                if report.promoted_count > 0 {
                    tracing::info!(
                        "memory bridge: initial promote moved {} entries",
                        report.promoted_count
                    );
                }
            }
            Err(e) => {
                tracing::warn!("memory bridge: initial bridge_promote failed: {e}");
            }
        }
    }

    let shutdown_notify = Arc::clone(&server.shutdown_notify);

    // Start background tasks
    if let Err(e) = start_background_tasks(server, Arc::clone(&shutdown_notify)).await {
        error!("Failed to start background tasks: {}", e);
        return Err(e);
    }

    // ── Spawn EvolutionLoop (BLUE56-B03) ─────────────────────────────
    if let Some(ref evo) = server.governance_deps.evolution_loop {
        let evo_clone = Arc::clone(evo);
        tokio::spawn(async move {
            loop {
                let mut guard = evo_clone.lock().await;
                if let Err(e) = guard.run().await {
                    tracing::warn!("Evolution loop cycle ended: {}; retrying after 60s", e);
                }
                drop(guard);
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
        });
        tracing::info!(target: "intelligence", "EvolutionLoop spawned");
    }

    info!("ACP server running");

    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();

    // Set up signal watchers for graceful shutdown
    let mut sigterm = std::pin::pin!(async {
        #[cfg(unix)]
        {
            match signal::unix::signal(signal::unix::SignalKind::terminate()) {
                Ok(mut stream) => {
                    stream.recv().await;
                }
                Err(e) => {
                    warn!("failed to register SIGTERM handler: {e}; graceful shutdown via SIGTERM disabled");
                    std::future::pending::<()>().await;
                }
            }
        }
        #[cfg(not(unix))]
        std::future::pending::<()>().await;
    });

    loop {
        if server.shutdown_requested() {
            break;
        }

        let next_line = tokio::select! {
            _ = shutdown_notify.notified() => {
                break;
            }
            _ = signal::ctrl_c() => {
                info!("Received SIGINT (Ctrl+C), initiating graceful shutdown...");
                break;
            }
            _ = sigterm.as_mut() => {
                info!("Received SIGTERM, initiating graceful shutdown...");
                break;
            }
            line = lines.next_line() => line?,
        };

        let Some(line) = next_line else {
            break;
        };

        if server.shutdown_requested() {
            break;
        }

        if line.trim().is_empty() {
            continue;
        }

        let request = match serde_json::from_str::<JsonRpcRequest>(&line) {
            Ok(request) => request,
            Err(err) => {
                send_error(
                    server,
                    None,
                    -32700,
                    tf("error.parse_error", &[("error", &err.to_string())]),
                    None,
                )
                .await?;
                continue;
            }
        };

        if request.jsonrpc != "2.0" {
            send_error(
                server,
                request.id,
                -32600,
                t("error.jsonrpc_must_be_2_0").to_string(),
                None,
            )
            .await?;
            continue;
        }

        if let Err(err) = handle_request(server, request, None).await {
            error!("request failed: {err:#}");
        }
    }

    // ── Graceful shutdown ──────────────────────────────────────────
    server.begin_shutdown();

    // Notify background tasks to shut down.  No drain for stdio — the
    // server runs until stdin EOF / SIGINT / SIGTERM, so there are no
    // in-flight network connections to drain.
    shutdown_notify.notify_waiters();
    info!("ACP server shutting down");
    Ok(())
}

pub use http_server::run_acp_http_server;
pub(crate) use tls::build_root_capabilities_response;

/// Get routing handles (flow manager and agent registry)
pub fn routing_handles(server: &AcpServer) -> Result<(Arc<FlowManager>, Arc<AgentRegistry>)> {
    let flow = server
        .model_deps
        .flow_manager
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("flow manager not initialized"))?;
    let registry = server
        .model_deps
        .agent_registry
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("agent registry not initialized"))?;
    Ok((Arc::clone(flow), Arc::clone(registry)))
}

/// Get cache handle
#[allow(dead_code)] // F-GAP-49 — planned wiring: memory/caching accessor
#[must_use]
pub fn cache_handle(server: &AcpServer) -> Option<Arc<crate::cache::ResponseCache>> {
    server.cache_deps.cache.response_cache.clone()
}

/// Get artifact ledger
pub fn artifact_ledger(_server: &AcpServer) -> crate::reinforcement::ArtifactLedger {
    _server
        .artifact_ledger
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_else(|_| {
            crate::reinforcement::ArtifactLedger::new(
                _server.config_path.as_deref().map(std::path::Path::new),
            )
        })
}

/// Get vector store handle
#[allow(dead_code)] // F-GAP-49 — planned wiring: learning/intelligence accessor
#[must_use]
pub fn vector_store_handle(server: &AcpServer) -> Option<Arc<VectorStore>> {
    server.cache_deps.cache.vector_store.clone()
}

/// Get vector configuration snapshot
#[allow(dead_code)] // F-GAP-49 — planned wiring: learning/intelligence accessor
pub fn vector_config_snapshot(server: &AcpServer) -> Option<VectorConfig> {
    server.cache_deps.vector_config.clone()
}

/// Get autotune handle
#[allow(dead_code)] // F-GAP-49 — planned wiring: learning/intelligence accessor
#[must_use]
pub fn autotune_handle(server: &AcpServer) -> Option<Arc<tokio::sync::Mutex<AutoTuneState>>> {
    server.cache_deps.autotune.clone()
}

#[derive(Debug, Clone, Deserialize)]
struct OpenAiChatRequest {
    model: Option<String>,
    messages: Vec<OpenAiChatMessage>,
    temperature: Option<f64>,
    top_p: Option<f64>,
    max_tokens: Option<u64>,
    n: Option<u64>,
    stop: Option<serde_json::Value>,
    presence_penalty: Option<f64>,
    frequency_penalty: Option<f64>,
    logit_bias: Option<serde_json::Value>,
    user: Option<String>,
    seed: Option<i64>,
    response_format: Option<serde_json::Value>,
    tools: Option<serde_json::Value>,
    tool_choice: Option<serde_json::Value>,
    parallel_tool_calls: Option<bool>,
    function_call: Option<serde_json::Value>,
    functions: Option<serde_json::Value>,
    #[serde(default)]
    stream: bool,
    #[serde(flatten)]
    extra: std::collections::HashMap<String, serde_json::Value>,
}

/// Responses API (Phase R1 baseline) request schema.
/// Maps `input` (string or array of message objects) instead of `messages`.
#[derive(Debug, Clone, Deserialize)]
struct ResponsesApiRequest {
    model: Option<String>,
    input: Option<serde_json::Value>,
    #[serde(default)]
    stream: bool,
    temperature: Option<f64>,
    /// Responses API uses `max_output_tokens` instead of `max_tokens`.
    max_output_tokens: Option<u64>,
    tools: Option<serde_json::Value>,
    tool_choice: Option<serde_json::Value>,
    metadata: Option<serde_json::Value>,
    reasoning: Option<serde_json::Value>,
    #[serde(flatten)]
    extra: std::collections::HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct OpenAiChatMessage {
    role: String,
    #[serde(default)]
    content: serde_json::Value,
    name: Option<String>,
    tool_call_id: Option<String>,
    function_call: Option<serde_json::Value>,
    tool_calls: Option<serde_json::Value>,
    refusal: Option<serde_json::Value>,
}

impl OpenAiChatMessage {
    fn normalized_role(&self) -> String {
        match self.role.as_str() {
            "system" | "user" | "assistant" => self.role.clone(),
            _ => "user".to_string(),
        }
    }

    fn content_text(&self) -> String {
        if let Some(text) = self.content.as_str() {
            return text.to_string();
        }

        if self.content.is_null() {
            return String::new();
        }

        if let Some(parts) = self.content.as_array() {
            let merged = parts
                .iter()
                .filter_map(|part| {
                    part.get("type")
                        .and_then(|value| value.as_str())
                        .filter(|value| *value == "text")?;
                    part.get("text")
                        .and_then(|value| value.as_str())
                        .map(ToString::to_string)
                })
                .collect::<Vec<_>>()
                .join("\n");
            if !merged.is_empty() {
                return merged;
            }

            return serde_json::to_string(parts).unwrap_or_default();
        }

        if self.content.is_object() {
            if let Some(text) = self.content.get("text").and_then(|value| value.as_str()) {
                return text.to_string();
            }

            return serde_json::to_string(&self.content).unwrap_or_default();
        }

        serde_json::to_string(&self.content).unwrap_or_default()
    }

    fn to_agent_message(&self) -> crate::agent::Message {
        let mut metadata: Vec<String> = Vec::new();
        if let Some(name) = &self.name {
            metadata.push(format!("[name] {}", name));
        }
        if let Some(tool_call_id) = &self.tool_call_id {
            metadata.push(format!("[tool_call_id] {}", tool_call_id));
        }
        if let Some(function_call) = &self.function_call {
            metadata.push(format!(
                "[function_call] {}",
                serde_json::to_string(function_call).unwrap_or_default()
            ));
        }
        if let Some(tool_calls) = &self.tool_calls {
            metadata.push(format!(
                "[tool_calls] {}",
                serde_json::to_string(tool_calls).unwrap_or_default()
            ));
        }
        if let Some(refusal) = &self.refusal {
            metadata.push(format!(
                "[refusal] {}",
                serde_json::to_string(refusal).unwrap_or_default()
            ));
        }

        let content = self.content_text();
        let final_content = if metadata.is_empty() {
            content
        } else if content.is_empty() {
            metadata.join("\n")
        } else {
            format!("{}\n{}", content, metadata.join("\n"))
        };

        crate::agent::Message {
            role: self.normalized_role(),
            content: final_content,
        }
    }
}

fn openai_to_chat_params(req: &OpenAiChatRequest) -> crate::acp::r#impl::chat::ChatParams {
    let messages = req
        .messages
        .iter()
        .map(OpenAiChatMessage::to_agent_message)
        .collect::<Vec<_>>();

    let mut extra = req.extra.clone();
    if let Some(model) = &req.model {
        extra.insert("model".to_string(), serde_json::json!(model));
    }
    if let Some(value) = req.temperature {
        extra.insert("temperature".to_string(), serde_json::json!(value));
    }
    if let Some(value) = req.top_p {
        extra.insert("top_p".to_string(), serde_json::json!(value));
    }
    if let Some(value) = req.max_tokens {
        extra.insert("max_tokens".to_string(), serde_json::json!(value));
    }
    if let Some(value) = req.n {
        extra.insert("n".to_string(), serde_json::json!(value));
    }
    if let Some(value) = &req.stop {
        extra.insert("stop".to_string(), value.clone());
    }
    if let Some(value) = req.presence_penalty {
        extra.insert("presence_penalty".to_string(), serde_json::json!(value));
    }
    if let Some(value) = req.frequency_penalty {
        extra.insert("frequency_penalty".to_string(), serde_json::json!(value));
    }
    if let Some(value) = &req.logit_bias {
        extra.insert("logit_bias".to_string(), value.clone());
    }
    if let Some(value) = &req.user {
        extra.insert("user".to_string(), serde_json::json!(value));
    }
    if let Some(value) = req.seed {
        extra.insert("seed".to_string(), serde_json::json!(value));
    }
    if let Some(value) = &req.response_format {
        extra.insert("response_format".to_string(), value.clone());
    }
    if let Some(value) = &req.tools {
        extra.insert("tools".to_string(), value.clone());
    }
    if let Some(value) = &req.tool_choice {
        extra.insert("tool_choice".to_string(), value.clone());
    }
    if let Some(value) = req.parallel_tool_calls {
        extra.insert("parallel_tool_calls".to_string(), serde_json::json!(value));
    }
    if let Some(value) = &req.function_call {
        extra.insert("function_call".to_string(), value.clone());
    }
    if let Some(value) = &req.functions {
        extra.insert("functions".to_string(), value.clone());
    }

    let options = if extra.is_empty() {
        None
    } else {
        Some(crate::config::PhaseOptions {
            extra,
            ..Default::default()
        })
    };

    crate::acp::r#impl::chat::ChatParams {
        mode: "ask".to_string(),
        messages,
        conversation_id: None,
        branch_id: None,
        // Use configured default phase instead of forcing delivery,
        // so deployment-specific fallback chains can be honored.
        phase: None,
        options,
        requirement_contract: None,
        plan: None,
        vector_hits: None,
        execution_decision_candidate: None,
    }
}

fn build_openai_completion(
    request_id: &str,
    model: &str,
    response_text: &str,
) -> serde_json::Value {
    serde_json::json!({
        "id": request_id,
        "object": "chat.completion",
        "created": crate::acp::prelude::now_ts(),
        "model": model,
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": response_text,
            },
            "finish_reason": "stop",
        }]
    })
}

fn build_openai_models_response() -> serde_json::Value {
    serde_json::json!({
        "object": "list",
        "data": [{
            "id": "go-on",
            "object": "model",
            "created": crate::acp::prelude::now_ts(),
            "owned_by": "go-on",
        }]
    })
}

fn build_responses_api_response(
    request_id: &str,
    model: &str,
    response_text: &str,
) -> serde_json::Value {
    let msg_id = format!("msg_{}", crate::acp::prelude::now_ts_ms());
    serde_json::json!({
        "id": request_id,
        "object": "response",
        "created_at": crate::acp::prelude::now_ts(),
        "model": model,
        "status": "completed",
        "output": [{
            "type": "message",
            "id": msg_id,
            "role": "assistant",
            "status": "completed",
            "content": [{
                "type": "output_text",
                "text": response_text,
            }],
        }],
        "usage": {
            "input_tokens": 0,
            "output_tokens": 0,
            "total_tokens": 0,
        },
        "error": null,
        "incomplete_details": null,
    })
}

fn attach_responses_token_economy(
    mut payload: serde_json::Value,
    messages: &[crate::agent::Message],
    response_text: &str,
) -> serde_json::Value {
    let token_economy = crate::acp::r#impl::chat::estimate_token_economy(messages, response_text);
    payload["usage"] = serde_json::json!({
        "input_tokens": token_economy["input_tokens"].clone(),
        "output_tokens": token_economy["output_tokens"].clone(),
        "total_tokens": token_economy["total_tokens"].clone(),
    });
    payload["token_economy"] = token_economy;
    payload
}

fn build_responses_api_queued_response(request_id: &str, model: &str) -> serde_json::Value {
    serde_json::json!({
        "id": request_id,
        "object": "response",
        "created_at": crate::acp::prelude::now_ts(),
        "model": model,
        "status": "queued",
        "output": [],
        "usage": {
            "input_tokens": 0,
            "output_tokens": 0,
            "total_tokens": 0,
        },
        "error": null,
        "incomplete_details": null,
    })
}

fn build_responses_api_in_progress_response(request_id: &str, model: &str) -> serde_json::Value {
    serde_json::json!({
        "id": request_id,
        "object": "response",
        "created_at": crate::acp::prelude::now_ts(),
        "model": model,
        "status": "in_progress",
        "output": [],
        "usage": {
            "input_tokens": 0,
            "output_tokens": 0,
            "total_tokens": 0,
        },
        "error": null,
        "incomplete_details": null,
    })
}

fn build_responses_api_tool_call_response(
    request_id: &str,
    model: &str,
    tool_call_id: &str,
    tool_name: &str,
) -> serde_json::Value {
    serde_json::json!({
        "id": request_id,
        "object": "response",
        "created_at": crate::acp::prelude::now_ts(),
        "model": model,
        "status": "incomplete",
        "output": [{
            "type": "tool_call",
            "id": tool_call_id,
            "status": "incomplete",
            "name": tool_name,
            "arguments": "{}",
        }],
        "usage": {
            "input_tokens": 0,
            "output_tokens": 0,
            "total_tokens": 0,
        },
        "error": null,
        "incomplete_details": {
            "reason": "tool_calls_required"
        },
    })
}

fn build_responses_api_tool_result_response(
    request_id: &str,
    model: &str,
    previous_response_id: &str,
    tool_call_id: &str,
    tool_result_text: &str,
) -> serde_json::Value {
    let msg_id = next_responses_api_id("msg");
    let tool_result_id = next_responses_api_id("tr");
    serde_json::json!({
        "id": request_id,
        "object": "response",
        "created_at": crate::acp::prelude::now_ts(),
        "model": model,
        "previous_response_id": previous_response_id,
        "status": "completed",
        "output": [
            {
                "type": "tool_result",
                "id": tool_result_id,
                "status": "completed",
                "tool_call_id": tool_call_id,
                "content": [{
                    "type": "output_text",
                    "text": tool_result_text,
                }],
            },
            {
                "type": "message",
                "id": msg_id,
                "role": "assistant",
                "status": "completed",
                "content": [{
                    "type": "output_text",
                    "text": format!("Tool result received for {}: {}", tool_call_id, tool_result_text),
                }],
            }
        ],
        "usage": {
            "input_tokens": 0,
            "output_tokens": 0,
            "total_tokens": 0,
        },
        "error": null,
        "incomplete_details": null,
    })
}

fn merge_responses_status_history(
    previous: Option<&serde_json::Value>,
    next: &mut serde_json::Value,
) {
    let Some(next_status) = next.get("status").and_then(|value| value.as_str()) else {
        return;
    };

    let mut history: Vec<serde_json::Value> = previous
        .and_then(|value| value.get("status_history"))
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();

    let last_status = history
        .last()
        .and_then(|event| event.get("status"))
        .and_then(|value| value.as_str());
    if last_status != Some(next_status) {
        history.push(serde_json::json!({
            "status": next_status,
            "at": crate::acp::prelude::now_ts_ms(),
        }));
    }

    if let Some(object) = next.as_object_mut() {
        object.insert(
            "status_history".to_string(),
            serde_json::Value::Array(history),
        );
    }
}

fn store_responses_api_payload(server: &AcpServer, payload: &serde_json::Value) {
    let Some(id) = payload.get("id").and_then(|value| value.as_str()) else {
        return;
    };
    let mut store = server
        .responses_api_store
        .lock()
        .unwrap_or_else(|poisoned| {
            tracing::warn!("responses_api_store lock poisoned in store_responses_api_payload");
            poisoned.into_inner()
        });
    // Evict oldest entries when store exceeds 1000 items
    if store.len() >= 1000 {
        // Remove 200 oldest entries
        let keys: Vec<String> = store.keys().take(200).cloned().collect();
        for key in keys {
            store.remove(&key);
        }
    }
    let previous = store.get(id).cloned();
    let mut next = payload.clone();
    merge_responses_status_history(previous.as_ref(), &mut next);
    store.insert(id.to_string(), next);
}

fn load_responses_api_payload(server: &AcpServer, response_id: &str) -> Option<serde_json::Value> {
    server
        .responses_api_store
        .lock()
        .ok()
        .and_then(|store| store.get(response_id).cloned())
}

fn list_responses_api_payloads(server: &AcpServer) -> Vec<serde_json::Value> {
    let mut values = server
        .responses_api_store
        .lock()
        .ok()
        .map(|store| store.values().cloned().collect::<Vec<_>>())
        .unwrap_or_default();

    fn latest_status_at(value: &serde_json::Value) -> i64 {
        value
            .get("status_history")
            .and_then(|v| v.as_array())
            .and_then(|events| events.last())
            .and_then(|event| event.get("at"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
    }

    fn id_sequence(value: &serde_json::Value) -> u64 {
        value
            .get("id")
            .and_then(|v| v.as_str())
            .and_then(|id| id.rsplit('_').next())
            .and_then(|part| part.parse::<u64>().ok())
            .unwrap_or(0)
    }

    values.sort_by_key(|value| {
        std::cmp::Reverse((
            value
                .get("created_at")
                .and_then(|v| v.as_i64())
                .unwrap_or(0),
            latest_status_at(value),
            id_sequence(value),
        ))
    });
    values
}

fn extract_response_id_from_path(path: &str) -> Option<&str> {
    path.strip_prefix("/v1/responses/")
        .filter(|value| !value.is_empty() && !value.contains('/'))
}

fn build_responses_error(
    code: &str,
    error_type: &str,
    message: impl Into<String>,
) -> serde_json::Value {
    serde_json::json!({
        "error": {
            "code": code,
            "type": error_type,
            "message": message.into()
        }
    })
}

async fn write_http_json_response_with_context(
    socket: &mut TcpStream,
    status: u16,
    body: serde_json::Value,
    method: &str,
    extra_headers: &str,
) -> Result<()> {
    let body = inject_platform_profiles_if_absent(body, method);
    write_http_json_response(socket, status, body, extra_headers).await
}

async fn write_responses_api_error(
    socket: &mut TcpStream,
    payload: serde_json::Value,
    extra_headers: &str,
) -> Result<()> {
    let result =
        write_http_json_response_with_context(socket, 400, payload, "responses.api", extra_headers)
            .await;
    if let Err(ref e) = result {
        tracing::warn!(
            "write_responses_api_error: failed to write error response: {}",
            e
        );
    }
    result
}

fn is_supported_responses_tool_choice(value: &serde_json::Value) -> bool {
    matches!(value.as_str(), Some("auto" | "none" | "required"))
}

fn validate_responses_tool(value: &serde_json::Value) -> Option<&'static str> {
    let Some(tool) = value.as_object() else {
        return Some("tool entry must be an object");
    };

    if tool.get("type").and_then(|v| v.as_str()) != Some("function") {
        return Some("tool entry must use type=function");
    }

    let Some(function) = tool.get("function").and_then(|value| value.as_object()) else {
        return Some("tool entry must include a function object");
    };

    let Some(name) = function.get("name").and_then(|value| value.as_str()) else {
        return Some("tool entry must include function.name");
    };

    if name.trim().is_empty() {
        return Some("tool entry must include a non-empty function.name");
    }

    if function
        .get("description")
        .is_some_and(|value| !value.is_string())
    {
        return Some("tool entry function.description must be a string");
    }

    if let Some(parameters) = function.get("parameters") {
        let Some(schema) = parameters.as_object() else {
            return Some("tool entry function.parameters must be an object");
        };

        if schema.get("type").and_then(|value| value.as_str()) != Some("object") {
            return Some("tool entry function.parameters.type must be 'object'");
        }

        if schema
            .get("properties")
            .is_some_and(|value| !value.is_object())
        {
            return Some("tool entry function.parameters.properties must be an object");
        }

        if schema.get("required").is_some_and(|value| {
            value
                .as_array()
                .is_none_or(|items| !items.iter().all(|item| item.is_string()))
        }) {
            return Some("tool entry function.parameters.required must be an array of strings");
        }
    }

    None
}

fn responses_tool_name(value: &serde_json::Value) -> Option<&str> {
    value
        .get("function")
        .and_then(|value| value.as_object())
        .and_then(|function| function.get("name"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|name| !name.is_empty())
}

fn pending_tool_call_id(response: &serde_json::Value) -> Option<&str> {
    response
        .get("output")
        .and_then(|value| value.as_array())
        .and_then(|items| {
            items.iter().find_map(|item| {
                (item.get("type").and_then(|value| value.as_str()) == Some("tool_call"))
                    .then(|| item.get("id").and_then(|value| value.as_str()))
                    .flatten()
            })
        })
}

fn extract_tool_result_for_call(input: &serde_json::Value, tool_call_id: &str) -> Option<String> {
    input.as_array().and_then(|items| {
        items.iter().find_map(|item| {
            let obj = item.as_object()?;
            if obj.get("role").and_then(|value| value.as_str()) != Some("tool") {
                return None;
            }
            if obj.get("tool_call_id").and_then(|value| value.as_str()) != Some(tool_call_id) {
                return None;
            }

            let content = obj.get("content")?;
            let text = if let Some(value) = content.as_str() {
                value.trim().to_string()
            } else if let Some(value) = content.get("text").and_then(|v| v.as_str()) {
                value.trim().to_string()
            } else if let Some(parts) = content.as_array() {
                parts
                    .iter()
                    .filter_map(|part| {
                        if let Some(text) = part.as_str() {
                            return Some(text.to_string());
                        }
                        part.get("text")
                            .and_then(|value| value.as_str())
                            .map(ToString::to_string)
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
                    .trim()
                    .to_string()
            } else {
                String::new()
            };

            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        })
    })
}

fn classify_responses_upstream_error_code(err: &anyhow::Error) -> &'static str {
    let message = err.to_string().to_lowercase();
    if message.contains("timed out") || message.contains("timeout") {
        "timeout"
    } else if message.contains("rate limit") || message.contains("too many requests") {
        "rate_limit"
    } else {
        "upstream_error"
    }
}

fn build_openai_chunk(
    request_id: &str,
    model: &str,
    content: &str,
    finish_reason: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "id": request_id,
        "object": "chat.completion.chunk",
        "created": crate::acp::prelude::now_ts(),
        "model": model,
        "choices": [{
            "index": 0,
            "delta": {
                "content": content,
            },
            "finish_reason": finish_reason,
        }]
    })
}

fn is_setup_or_upstream_unavailable(err: &anyhow::Error) -> bool {
    let msg = err.to_string().to_lowercase();
    msg.contains("missing environment variable")
        || msg.contains("error sending request")
        || msg.contains("connection refused")
        || msg.contains("timed out")
        || msg.contains("error.chat.agent_error_prefix")
        || msg.contains("error.chat.all_agents_failed")
}

fn degraded_openai_message(err: &anyhow::Error) -> String {
    format!(
        "go-on is running, but upstream model service is unavailable. {}. Configure at least one reachable provider (for example set DEEPSEEK_API_KEY) or start your copilot-compatible upstream on 127.0.0.1:8080.",
        err
    )
}

/// Write data to a TcpStream with a 30-second timeout.
/// Returns an error if the write times out or the connection is broken.
/// Write data to a TcpStream with a 30-second timeout.
/// Returns an error if the write times out or the connection is broken.
async fn tcp_write_timeout(
    socket: &mut (impl tokio::io::AsyncWrite + Unpin),
    data: &[u8],
) -> Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(30), socket.write_all(data))
        .await
        .map_err(|_| anyhow::anyhow!("timeout writing to socket"))?
        .map_err(|e| anyhow::anyhow!("socket write error: {e}"))
}

async fn write_openai_sse_data(
    socket: &mut (impl tokio::io::AsyncWrite + Unpin),
    payload: &serde_json::Value,
) -> Result<()> {
    let json_str = serde_json::to_string(payload)?;
    // Pre-allocate: "data: " (6) + json + "\n\n" (2)
    let mut frame = String::with_capacity(6 + json_str.len() + 2);
    frame.push_str("data: ");
    frame.push_str(&json_str);
    frame.push_str("\n\n");
    tcp_write_timeout(socket, frame.as_bytes()).await?;
    tokio::time::timeout(std::time::Duration::from_secs(30), socket.flush())
        .await
        .map_err(|_| anyhow::anyhow!("timeout flushing socket"))?
        .map_err(|e| anyhow::anyhow!("socket flush error: {e}"))?;
    Ok(())
}

async fn write_openai_sse_done(socket: &mut (impl tokio::io::AsyncWrite + Unpin)) -> Result<()> {
    tcp_write_timeout(socket, b"data: [DONE]\n\n").await?;
    tokio::time::timeout(std::time::Duration::from_secs(30), socket.flush())
        .await
        .map_err(|_| anyhow::anyhow!("timeout flushing socket"))?
        .map_err(|e| anyhow::anyhow!("socket flush error: {e}"))?;
    let _ = socket.shutdown().await;
    Ok(())
}

async fn handle_openai_chat_completions(
    socket: &mut TcpStream,
    server: Arc<AcpServer>,
    body: serde_json::Value,
    user_session: Option<crate::acp::r#impl::session::UserSession>,
    cors_headers: &str,
) -> Result<()> {
    let started = std::time::Instant::now();
    let record_outcome = |success: bool| {
        server
            .observability
            .metrics
            .record_request_outcome(success, started.elapsed().as_millis() as f64);
    };

    let openai_req: OpenAiChatRequest = match serde_json::from_value(body) {
        Ok(value) => value,
        Err(err) => {
            let payload = serde_json::json!({
                "error": {
                    "message": tf("error.invalid_openai_chat_request", &[("error", &err.to_string())]),
                    "type": "invalid_request_error"
                }
            });
            write_http_json_response_with_context(
                socket,
                400,
                payload,
                "openai.chat.completions",
                cors_headers,
            )
            .await?;
            record_outcome(false);
            return Ok(());
        }
    };
    let model = openai_req
        .model
        .clone()
        .unwrap_or_else(|| "go-on".to_string());
    let request_id = format!("chatcmpl-{}", crate::acp::prelude::now_ts_ms());
    let params = openai_to_chat_params(&openai_req);

    if !openai_req.stream {
        let trace = http_trace_context("openai.chat.completions");
        let ctx = Some(crate::acp::r#impl::chat::ChatRequestContext::new(
            user_session.clone(),
        ));
        let result = crate::acp::r#impl::chat::process_chat_request(
            server.as_ref(),
            &params,
            None,
            &trace,
            None,
            ctx,
        )
        .await;
        let result = match result {
            Ok(result) => result,
            Err(err) => {
                if is_setup_or_upstream_unavailable(&err) {
                    let payload = build_openai_completion(
                        &request_id,
                        &model,
                        &degraded_openai_message(&err),
                    );
                    let payload =
                        inject_platform_profiles_if_absent(payload, "openai.chat.completions");
                    write_http_json_response(socket, 200, payload, cors_headers).await?;
                    record_outcome(true);
                    return Ok(());
                }
                let payload = serde_json::json!({
                    "error": {
                        "message": err.to_string(),
                        "type": "go_on_upstream_error"
                    }
                });
                write_http_json_response_with_context(
                    socket,
                    502,
                    payload,
                    "openai.chat.completions",
                    cors_headers,
                )
                .await?;
                record_outcome(false);
                return Ok(());
            }
        };
        let response_text = result
            .get("response")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let payload = build_openai_completion(&request_id, &model, response_text);
        let payload = inject_platform_profiles_if_absent(payload, "openai.chat.completions");
        write_http_json_response(socket, 200, payload, cors_headers).await?;
        record_outcome(true);
        return Ok(());
    }

    write_sse_headers(socket, cors_headers).await?;

    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    let trace = http_trace_context("openai.chat.completions.stream");
    let ctx = Some(crate::acp::r#impl::chat::ChatRequestContext::new(
        user_session,
    ));
    let server_ref = Arc::clone(&server);
    let task = tokio::spawn(async move {
        crate::acp::r#impl::chat::process_chat_request(
            server_ref.as_ref(),
            &params,
            Some(crate::acp::r#impl::chat::StreamObserver::sse(tx)),
            &trace,
            None,
            ctx,
        )
        .await
    });

    while let Some(frame) = rx.recv().await {
        if frame.event != "chunk" {
            continue;
        }
        let token = frame
            .payload
            .get("token")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        if token.is_empty() {
            continue;
        }
        let payload = build_openai_chunk(&request_id, &model, token, None);
        if let Err(err) = write_openai_sse_data(socket, &payload).await {
            // Client disconnected while backend task is still producing tokens.
            // Abort task to avoid orphan compute and channel buildup.
            task.abort();
            record_outcome(false);
            return Err(err);
        }
    }

    match task.await {
        Ok(Ok(_)) => {
            let done_payload = build_openai_chunk(&request_id, &model, "", Some("stop"));
            write_openai_sse_data(socket, &done_payload).await?;
            write_openai_sse_done(socket).await?;
            record_outcome(true);
        }
        Ok(Err(err)) => {
            if is_setup_or_upstream_unavailable(&err) {
                let payload = build_openai_chunk(
                    &request_id,
                    &model,
                    &degraded_openai_message(&err),
                    Some("stop"),
                );
                write_openai_sse_data(socket, &payload).await?;
                write_openai_sse_done(socket).await?;
                record_outcome(true);
                return Ok(());
            }
            let payload = serde_json::json!({"error": {"message": err.to_string()}});
            write_openai_sse_data(socket, &payload).await?;
            write_openai_sse_done(socket).await?;
            record_outcome(false);
        }
        Err(err) => {
            let payload = serde_json::json!({"error": {"message": tf("error.chat_task_panicked", &[("error", &err.to_string())])}});
            write_openai_sse_data(socket, &payload).await?;
            write_openai_sse_done(socket).await?;
            record_outcome(false);
        }
    }

    Ok(())
}

/// Convert Responses API `input` field to internal agent messages.
///
/// Supports:
/// - String → single user message
/// - Array of `{role, content}` objects (content may be string or array of input items)
fn responses_input_to_messages(input: &serde_json::Value) -> Vec<crate::agent::Message> {
    if let Some(text) = input.as_str() {
        if text.trim().is_empty() {
            return vec![];
        }
        return vec![crate::agent::Message {
            role: "user".to_string(),
            content: text.to_string(),
        }];
    }

    if let Some(items) = input.as_array() {
        return items
            .iter()
            .filter_map(|item| {
                let role = item
                    .get("role")
                    .and_then(|v| v.as_str())
                    .unwrap_or("user")
                    .to_string();
                let role = match role.as_str() {
                    "system" | "user" | "assistant" => role,
                    _ => "user".to_string(),
                };

                let content = if let Some(text) = item.get("content").and_then(|v| v.as_str()) {
                    text.to_string()
                } else if let Some(parts) = item.get("content").and_then(|v| v.as_array()) {
                    parts
                        .iter()
                        .filter_map(|p| {
                            let ptype = p.get("type").and_then(|t| t.as_str()).unwrap_or("");
                            if ptype == "input_text" || ptype == "text" {
                                p.get("text")
                                    .and_then(|t| t.as_str())
                                    .map(|s| s.to_string())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                } else if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                    text.to_string()
                } else {
                    String::new()
                };

                if content.trim().is_empty() && role == "user" {
                    None
                } else {
                    Some(crate::agent::Message { role, content })
                }
            })
            .collect();
    }

    vec![]
}

/// Validate and deserialize a Responses API POST request.
/// On error, writes the error to the socket and returns `Err(())` so the caller can bail.
async fn validate_responses_post_request(
    socket: &mut TcpStream,
    body: &serde_json::Value,
    extra_headers: &str,
) -> Result<ResponsesApiRequest, ()> {
    if !body.is_object() {
        let payload = build_responses_error(
            "invalid_request_error",
            "invalid_request_error",
            t("error.request_body_must_be_json_object"),
        );
        let _ = write_responses_api_error(socket, payload, extra_headers).await;
        return Err(());
    }

    // Validate required fields before deserialization.
    if body.get("model").and_then(|v| v.as_str()).is_none() {
        if body.get("model").is_some() {
            let payload = build_responses_error(
                "invalid_input",
                "invalid_request_error",
                t("error.model_must_be_string"),
            );
            let _ = write_responses_api_error(socket, payload, extra_headers).await;
            return Err(());
        }
        let payload = build_responses_error(
            "missing_required_field",
            "invalid_request_error",
            t("error.model_required"),
        );
        let _ = write_responses_api_error(socket, payload, extra_headers).await;
        return Err(());
    }
    match body.get("input") {
        None => {
            let payload = build_responses_error(
                "missing_required_field",
                "invalid_request_error",
                t("error.input_required"),
            );
            let _ = write_responses_api_error(socket, payload, extra_headers).await;
            return Err(());
        }
        Some(value) if value.is_null() || (!value.is_string() && !value.is_array()) => {
            let payload = build_responses_error(
                "invalid_input",
                "invalid_request_error",
                t("error.input_must_be_string_or_array"),
            );
            let _ = write_responses_api_error(socket, payload, extra_headers).await;
            return Err(());
        }
        Some(_) => {}
    }
    if let Some(v) = body.get("max_output_tokens") {
        if !v.is_u64() {
            let payload = build_responses_error(
                "invalid_input",
                "invalid_request_error",
                t("error.max_output_tokens_invalid"),
            );
            let _ = write_responses_api_error(socket, payload, extra_headers).await;
            return Err(());
        }
    }
    if let Some(v) = body.get("temperature") {
        let Some(value) = v.as_f64() else {
            let payload = build_responses_error(
                "invalid_input",
                "invalid_request_error",
                t("error.temperature_must_be_number"),
            );
            let _ = write_responses_api_error(socket, payload, extra_headers).await;
            return Err(());
        };
        if !(0.0..=2.0).contains(&value) {
            let payload = build_responses_error(
                "invalid_input",
                "invalid_request_error",
                t("error.temperature_invalid"),
            );
            let _ = write_responses_api_error(socket, payload, extra_headers).await;
            return Err(());
        }
    }
    if let Some(v) = body.get("metadata") {
        if !v.is_object() {
            let payload = build_responses_error(
                "invalid_input",
                "invalid_request_error",
                t("error.metadata_invalid"),
            );
            let _ = write_responses_api_error(socket, payload, extra_headers).await;
            return Err(());
        }
    }
    if let Some(v) = body.get("reasoning") {
        if !v.is_object() {
            let payload = build_responses_error(
                "invalid_input",
                "invalid_request_error",
                t("error.reasoning_invalid"),
            );
            let _ = write_responses_api_error(socket, payload, extra_headers).await;
            return Err(());
        }
    }
    if let Some(v) = body.get("tools") {
        if !v.is_array() {
            let payload = build_responses_error(
                "invalid_input",
                "invalid_request_error",
                t("error.tools_invalid"),
            );
            let _ = write_responses_api_error(socket, payload, extra_headers).await;
            return Err(());
        }
        if v.as_array()
            .is_some_and(|items| items.iter().any(|item| !item.is_object()))
        {
            let payload = build_responses_error(
                "invalid_input",
                "invalid_request_error",
                t("error.tools_entries_object"),
            );
            let _ = write_responses_api_error(socket, payload, extra_headers).await;
            return Err(());
        }
        if let Some(reason) = v
            .as_array()
            .and_then(|items| items.iter().find_map(validate_responses_tool))
        {
            let payload = build_responses_error("invalid_input", "invalid_request_error", reason);
            let _ = write_responses_api_error(socket, payload, extra_headers).await;
            return Err(());
        }
    }
    if let Some(v) = body.get("tool_choice") {
        if !v.is_string() && !v.is_object() {
            let payload = build_responses_error(
                "invalid_input",
                "invalid_request_error",
                t("error.tool_choice_invalid"),
            );
            let _ = write_responses_api_error(socket, payload, extra_headers).await;
            return Err(());
        }
        if v.is_string() && !is_supported_responses_tool_choice(v) {
            let payload = build_responses_error(
                "invalid_input",
                "invalid_request_error",
                t("error.tool_choice_value_invalid"),
            );
            let _ = write_responses_api_error(socket, payload, extra_headers).await;
            return Err(());
        }
        if let Some(reason) = v.as_object().and_then(|_| validate_responses_tool(v)) {
            let payload = build_responses_error("invalid_input", "invalid_request_error", reason);
            let _ = write_responses_api_error(socket, payload, extra_headers).await;
            return Err(());
        }
    }
    if let Some(v) = body.get("previous_response_id") {
        if v.as_str().is_none_or(|value| value.trim().is_empty()) {
            let payload = build_responses_error(
                "invalid_input",
                "invalid_request_error",
                t("error.previous_response_id_invalid"),
            );
            let _ = write_responses_api_error(socket, payload, extra_headers).await;
            return Err(());
        }
    }

    let req: ResponsesApiRequest = match serde_json::from_value(body.clone()) {
        Ok(r) => r,
        Err(err) => {
            let payload = build_responses_error(
                "invalid_request_error",
                "invalid_request_error",
                tf("error.invalid_request", &[("error", &err.to_string())]),
            );
            let _ = write_responses_api_error(socket, payload, extra_headers).await;
            return Err(());
        }
    };
    Ok(req)
}

/// Handle POST /v1/responses — Responses API Phase R1 baseline.
///
/// Accepts the Responses API request schema, maps internally to chat_params,
/// and returns a structured `response` object (not a chat.completion object).
async fn handle_responses_api(
    socket: &mut TcpStream,
    server: Arc<AcpServer>,
    body: serde_json::Value,
    user_session: Option<crate::acp::r#impl::session::UserSession>,
    cors_headers: &str,
) -> Result<()> {
    let req = match validate_responses_post_request(socket, &body, cors_headers).await {
        Ok(r) => r,
        Err(()) => return Ok(()),
    };

    let raw_model = req.model.clone().unwrap_or_default();
    let model = raw_model.trim().to_string();
    if model.is_empty() {
        let payload = build_responses_error(
            "invalid_input",
            "invalid_request_error",
            t("error.model_required"),
        );
        write_responses_api_error(socket, payload, cors_headers).await?;
        return Ok(());
    }

    if let Some(0) = req.max_output_tokens {
        let payload = build_responses_error(
            "invalid_input",
            "invalid_request_error",
            t("error.max_output_tokens_gt_zero"),
        );
        write_responses_api_error(socket, payload, cors_headers).await?;
        return Ok(());
    }

    let declared_tool_names = req
        .tools
        .as_ref()
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(responses_tool_name)
                .collect::<std::collections::HashSet<_>>()
        })
        .unwrap_or_default();

    if let Some(tool_choice) = req.tool_choice.as_ref() {
        if matches!(tool_choice.as_str(), Some("required")) && declared_tool_names.is_empty() {
            let payload = build_responses_error(
                "invalid_input",
                "invalid_request_error",
                "tool_choice=required requires at least one declared tool",
            );
            write_responses_api_error(socket, payload, cors_headers).await?;
            return Ok(());
        }

        if let Some(name) = tool_choice
            .as_object()
            .and_then(|_| responses_tool_name(tool_choice))
        {
            if declared_tool_names.is_empty() {
                let payload = build_responses_error(
                    "invalid_input",
                    "invalid_request_error",
                    "tool_choice object requires tools to be provided",
                );
                write_responses_api_error(socket, payload, cors_headers).await?;
                return Ok(());
            }
            if !declared_tool_names.contains(name) {
                let payload = build_responses_error(
                    "invalid_input",
                    "invalid_request_error",
                    "tool_choice function.name must match a declared tool",
                );
                write_responses_api_error(socket, payload, cors_headers).await?;
                return Ok(());
            }
        }
    }

    let Some(input) = req.input.as_ref() else {
        let payload = build_responses_error(
            "invalid_input",
            "invalid_request_error",
            "input must be a string or an array of input messages",
        );
        write_responses_api_error(socket, payload, cors_headers).await?;
        return Ok(());
    };
    if !input.is_string() && !input.is_array() {
        let payload = build_responses_error(
            "invalid_input",
            "invalid_request_error",
            "input must be a string or an array of input messages",
        );
        write_responses_api_error(socket, payload, cors_headers).await?;
        return Ok(());
    }

    let previous_response_id = req
        .extra
        .get("previous_response_id")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);

    let messages = responses_input_to_messages(input);
    let has_non_empty_user_message = messages
        .iter()
        .any(|m| m.role == "user" && !m.content.trim().is_empty());
    if previous_response_id.is_none() && !has_non_empty_user_message {
        let payload = build_responses_error(
            "invalid_input",
            "invalid_request_error",
            "input must contain at least one non-empty user message",
        );
        write_responses_api_error(socket, payload, cors_headers).await?;
        return Ok(());
    }

    let request_id = next_responses_api_id("resp");
    store_responses_api_payload(
        server.as_ref(),
        &build_responses_api_queued_response(&request_id, &model),
    );
    store_responses_api_payload(
        server.as_ref(),
        &build_responses_api_in_progress_response(&request_id, &model),
    );

    // Tool-result path: continuing a previous conversation with a tool result.
    if let Some(previous_response_id) = previous_response_id.as_deref() {
        return handle_response_tool_result(
            socket,
            &server,
            &request_id,
            &model,
            input,
            previous_response_id,
            cors_headers,
        )
        .await;
    }

    // Tool-call path: tool_choice=required, immediately produce a tool call.
    if matches!(
        req.tool_choice.as_ref().and_then(|v| v.as_str()),
        Some("required")
    ) {
        return handle_response_required_tool_call(
            socket,
            &server,
            &request_id,
            &model,
            &req,
            cors_headers,
        )
        .await;
    }

    // Normal create path (possibly streaming).
    handle_response_create(
        socket,
        server,
        &request_id,
        &model,
        req,
        messages,
        user_session,
        cors_headers,
    )
    .await
}

/// Handle tool result (previous_response_id) — stores and writes tool result response.
async fn handle_response_tool_result(
    socket: &mut TcpStream,
    server: &Arc<AcpServer>,
    request_id: &str,
    model: &str,
    input: &serde_json::Value,
    previous_response_id: &str,
    cors_headers: &str,
) -> Result<()> {
    let Some(previous_response) = load_responses_api_payload(server.as_ref(), previous_response_id)
    else {
        let payload = build_responses_error(
            "not_found",
            "invalid_request_error",
            "previous_response_id not found",
        );
        write_responses_api_error(socket, payload, cors_headers).await?;
        return Ok(());
    };

    let Some(tool_call_id) = pending_tool_call_id(&previous_response) else {
        let payload = build_responses_error(
            "tool_error",
            "tool_error",
            "previous_response_id has no pending tool_call",
        );
        write_responses_api_error(socket, payload, cors_headers).await?;
        return Ok(());
    };

    let Some(tool_result_text) = extract_tool_result_for_call(input, tool_call_id) else {
        let payload = build_responses_error(
            "tool_error",
            "tool_error",
            "input must include a tool result with matching tool_call_id",
        );
        write_responses_api_error(socket, payload, cors_headers).await?;
        return Ok(());
    };

    let payload = build_responses_api_tool_result_response(
        request_id,
        model,
        previous_response_id,
        tool_call_id,
        &tool_result_text,
    );
    store_responses_api_payload(server.as_ref(), &payload);
    write_http_json_response(socket, 200, payload, cors_headers).await?;
    Ok(())
}

/// Handle tool_choice=required — immediately produce a tool call response.
async fn handle_response_required_tool_call(
    socket: &mut TcpStream,
    server: &Arc<AcpServer>,
    request_id: &str,
    model: &str,
    req: &ResponsesApiRequest,
    cors_headers: &str,
) -> Result<()> {
    let tool_name = req
        .tools
        .as_ref()
        .and_then(|value| value.as_array())
        .and_then(|items| items.iter().find_map(responses_tool_name))
        .unwrap_or("tool");
    let tool_call_id = next_responses_api_id("call");
    let payload =
        build_responses_api_tool_call_response(request_id, model, &tool_call_id, tool_name);
    store_responses_api_payload(server.as_ref(), &payload);
    write_http_json_response(socket, 200, payload, cors_headers).await?;
    Ok(())
}

/// Handle normal create path (non-tool, non-stream) for POST /v1/responses.
#[allow(clippy::too_many_arguments)]
async fn handle_response_create(
    socket: &mut TcpStream,
    server: Arc<AcpServer>,
    request_id: &str,
    model: &str,
    req: ResponsesApiRequest,
    messages: Vec<crate::agent::Message>,
    user_session: Option<crate::acp::r#impl::session::UserSession>,
    cors_headers: &str,
) -> Result<()> {
    let mut extra = req.extra.clone();
    extra.remove("previous_response_id");
    extra.insert("model".to_string(), serde_json::json!(model));
    if let Some(t) = req.temperature {
        extra.insert("temperature".to_string(), serde_json::json!(t));
    }
    if let Some(t) = req.max_output_tokens {
        extra.insert("max_tokens".to_string(), serde_json::json!(t));
    }
    if let Some(v) = &req.tools {
        extra.insert("tools".to_string(), v.clone());
    }
    if let Some(v) = &req.tool_choice {
        extra.insert("tool_choice".to_string(), v.clone());
    }
    if let Some(v) = &req.metadata {
        extra.insert("metadata".to_string(), v.clone());
    }
    if let Some(v) = &req.reasoning {
        extra.insert("reasoning".to_string(), v.clone());
    }

    let params = crate::acp::r#impl::chat::ChatParams {
        mode: "ask".to_string(),
        messages,
        conversation_id: None,
        branch_id: None,
        phase: None,
        options: if extra.is_empty() {
            None
        } else {
            Some(crate::config::PhaseOptions {
                extra,
                ..Default::default()
            })
        },
        requirement_contract: None,
        plan: None,
        vector_hits: None,
        execution_decision_candidate: None,
    };

    let trace = http_trace_context("responses.api");

    if req.stream {
        return handle_response_stream(
            socket,
            server,
            request_id,
            model,
            params,
            &trace,
            user_session,
            cors_headers,
        )
        .await;
    }

    let ctx = Some(crate::acp::r#impl::chat::ChatRequestContext::new(
        user_session.clone(),
    ));
    let result = crate::acp::r#impl::chat::process_chat_request(
        server.as_ref(),
        &params,
        None,
        &trace,
        None,
        ctx,
    )
    .await;

    let result = match result {
        Ok(r) => r,
        Err(err) => {
            if is_setup_or_upstream_unavailable(&err) {
                let payload = attach_responses_token_economy(
                    build_responses_api_response(request_id, model, &degraded_openai_message(&err)),
                    &params.messages,
                    &degraded_openai_message(&err),
                );
                let payload = inject_platform_profiles_if_absent(payload, "responses.api");
                store_responses_api_payload(server.as_ref(), &payload);
                write_http_json_response(socket, 200, payload, cors_headers).await?;
                return Ok(());
            }
            let code = classify_responses_upstream_error_code(&err);
            let payload = build_responses_error(code, "upstream_error", err.to_string());
            store_responses_api_payload(
                server.as_ref(),
                &serde_json::json!({
                    "id": request_id,
                    "object": "response",
                    "created_at": crate::acp::prelude::now_ts(),
                    "model": model,
                    "status": "failed",
                    "output": [],
                    "usage": {
                        "input_tokens": 0,
                        "output_tokens": 0,
                        "total_tokens": 0,
                    },
                    "error": payload["error"].clone(),
                    "incomplete_details": null,
                }),
            );
            write_http_json_response_with_context(
                socket,
                502,
                payload,
                "responses.api",
                cors_headers,
            )
            .await?;
            return Ok(());
        }
    };

    let response_text = result
        .get("response")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let payload = attach_responses_token_economy(
        build_responses_api_response(request_id, model, response_text),
        &params.messages,
        response_text,
    );
    let payload = inject_platform_profiles_if_absent(payload, "responses.api");
    store_responses_api_payload(server.as_ref(), &payload);
    write_http_json_response(socket, 200, payload, cors_headers).await?;
    Ok(())
}

/// Handle GET /v1/responses/{id} — retrieve a single response by its ID.
async fn handle_response_get(
    socket: &mut TcpStream,
    server: &AcpServer,
    response_id: &str,
    cors_headers: &str,
) -> Result<()> {
    if let Some(payload) = load_responses_api_payload(server, response_id) {
        write_http_json_response_with_context(socket, 200, payload, "responses.api", cors_headers)
            .await?;
    } else {
        write_http_json_response_with_context(
            socket,
            404,
            build_responses_error(
                "not_found",
                "invalid_request_error",
                "response id not found",
            ),
            "responses.api",
            cors_headers,
        )
        .await?;
    }
    Ok(())
}

/// Streaming (SSE) path for POST /v1/responses when stream=true.
/// Sends: response.created → response.output_text.delta → response.completed, then [DONE].
#[allow(clippy::too_many_arguments)]
async fn handle_response_stream(
    socket: &mut TcpStream,
    server: Arc<AcpServer>,
    request_id: &str,
    model: &str,
    params: crate::acp::r#impl::chat::ChatParams,
    trace: &crate::protocol::rpc_protocol::RequestTraceContext,
    user_session: Option<crate::acp::r#impl::session::UserSession>,
    cors_headers: &str,
) -> Result<()> {
    let created_at = crate::acp::prelude::now_ts();

    let initial_response = serde_json::json!({
        "id": request_id,
        "object": "response",
        "created_at": created_at,
        "model": model,
        "status": "in_progress",
        "output": [],
        "usage": null,
        "incomplete_details": null,
    });

    write_sse_headers(socket, cors_headers).await?;
    write_sse_event(
        socket,
        "response.created",
        &serde_json::json!({
            "type": "response.created",
            "response": initial_response,
        }),
    )
    .await?;

    let (tx, mut rx) = tokio::sync::mpsc::channel::<crate::acp::r#impl::chat::StreamFrame>(256);
    let observer = crate::acp::r#impl::chat::StreamObserver::sse(tx);
    let ctx = Some(crate::acp::r#impl::chat::ChatRequestContext::new(
        user_session,
    ));
    let server_ref = Arc::clone(&server);
    let trace_for_task = trace.clone();
    let params_for_task = params.clone();
    let task = tokio::spawn(async move {
        crate::acp::r#impl::chat::process_chat_request(
            server_ref.as_ref(),
            &params_for_task,
            Some(observer),
            &trace_for_task,
            None,
            ctx,
        )
        .await
    });

    // Forward SSE frames from the channel to the socket
    // (process_chat_request now streams tokens through the observer in real time)
    while let Some(frame) = rx.recv().await {
        if let Err(err) = write_sse_event(socket, frame.event, &frame.payload).await {
            task.abort();
            return Err(err);
        }
    }

    let result = match task.await {
        Ok(r) => r,
        Err(err) => {
            let payload = serde_json::json!({"message": format!("chat task panicked: {err}")});
            write_sse_event(socket, "error", &payload).await?;
            return Ok(());
        }
    };

    match result {
        Ok(r) => {
            let text = r
                .get("response")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let item_id = next_responses_api_id("msg");

            write_sse_event(
                socket,
                "response.output_text.delta",
                &serde_json::json!({
                    "type": "response.output_text.delta",
                    "output_index": 0,
                    "content_index": 0,
                    "delta": text,
                    "item_id": item_id,
                    "response_id": request_id,
                }),
            )
            .await?;

            write_sse_event(
                socket,
                "response.token_economy",
                &serde_json::json!({
                    "type": "response.token_economy",
                    "response_id": request_id,
                    "token_economy": crate::acp::r#impl::chat::estimate_token_economy(&params.messages, text),
                }),
            )
            .await?;

            let completed = attach_responses_token_economy(
                build_responses_api_response(request_id, model, text),
                &params.messages,
                text,
            );
            let completed = inject_platform_profiles_if_absent(completed, "responses.api");
            store_responses_api_payload(server.as_ref(), &completed);

            write_sse_event(
                socket,
                "response.completed",
                &serde_json::json!({
                    "type": "response.completed",
                    "response": completed,
                }),
            )
            .await?;
        }
        Err(err) => {
            if is_setup_or_upstream_unavailable(&err) {
                let text = degraded_openai_message(&err);
                let item_id = next_responses_api_id("msg");

                write_sse_event(
                    socket,
                    "response.output_text.delta",
                    &serde_json::json!({
                        "type": "response.output_text.delta",
                        "output_index": 0,
                        "content_index": 0,
                        "delta": text,
                        "item_id": item_id,
                        "response_id": request_id,
                    }),
                )
                .await?;

                write_sse_event(
                    socket,
                    "response.token_economy",
                    &serde_json::json!({
                        "type": "response.token_economy",
                        "response_id": request_id,
                        "token_economy": crate::acp::r#impl::chat::estimate_token_economy(&params.messages, &text),
                    }),
                )
                .await?;

                let completed = attach_responses_token_economy(
                    build_responses_api_response(request_id, model, &text),
                    &params.messages,
                    &text,
                );
                let completed = inject_platform_profiles_if_absent(completed, "responses.api");
                store_responses_api_payload(server.as_ref(), &completed);

                write_sse_event(
                    socket,
                    "response.completed",
                    &serde_json::json!({
                        "type": "response.completed",
                        "response": completed,
                    }),
                )
                .await?;

                write_openai_sse_done(socket).await?;
                return Ok(());
            }

            let code = classify_responses_upstream_error_code(&err);
            write_sse_event(
                socket,
                "response.token_economy",
                &serde_json::json!({
                    "type": "response.token_economy",
                    "response_id": request_id,
                    "token_economy": crate::acp::r#impl::chat::estimate_token_economy(&params.messages, ""),
                }),
            )
            .await?;
            let failed = serde_json::json!({
                "id": request_id,
                "object": "response",
                "created_at": created_at,
                "model": model,
                "status": "failed",
                "output": [],
                "usage": {"input_tokens": 0, "output_tokens": 0, "total_tokens": 0},
                "error": {"code": code, "type": "upstream_error", "message": err.to_string()},
                "incomplete_details": null,
            });
            let failed = inject_platform_profiles_if_absent(failed, "responses.api");
            store_responses_api_payload(server.as_ref(), &failed);

            write_sse_event(
                socket,
                "response.failed",
                &serde_json::json!({
                    "type": "response.failed",
                    "response": failed,
                }),
            )
            .await?;
        }
    }

    write_openai_sse_done(socket).await?;
    Ok(())
}

/// Parse the raw HTTP request text into method, path, header_part, body_initial_part,
/// and adaptive_signal.
struct ParsedHttpRequest<'a> {
    method: &'a str,
    path: &'a str,
    header_part: &'a str,
    body_initial_part: &'a str,
    #[allow(dead_code)] // F-GAP-49 — reserved for planner/executor adaptive signal
    adaptive_signal: &'static str,
}

fn parse_http_request(request_text: &str) -> Result<ParsedHttpRequest<'_>> {
    let header_end = request_text
        .find("\r\n\r\n")
        .ok_or_else(|| anyhow::anyhow!("invalid HTTP request: missing header terminator"))?;

    let (header_part, body_initial_part) = request_text.split_at(header_end + 4);
    let request_line = header_part
        .lines()
        .next()
        .ok_or_else(|| anyhow::anyhow!("invalid HTTP request: missing request line"))?;

    let mut request_line_parts = request_line.split_whitespace();
    let method = request_line_parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("invalid HTTP request: missing method"))?;
    let path = request_line_parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("invalid HTTP request: missing path"))?;
    let adaptive_signal = infer_adaptive_signal(method, path, header_part);

    Ok(ParsedHttpRequest {
        method,
        path,
        header_part,
        body_initial_part,
        adaptive_signal,
    })
}

/// Apply entry guards and return `true` if the request was rejected (response already written).
async fn http_entry_guard(
    socket: &mut TcpStream,
    server: &AcpServer,
    header_part: &str,
    method: &str,
    path: &str,
    peer_addr: SocketAddr,
    cors_headers: &str,
) -> Result<bool> {
    apply_entry_guards(
        socket,
        server,
        header_part,
        method,
        path,
        peer_addr,
        cors_headers,
    )
    .await
}

/// Route an HTTP GET request based on the path and write the response back to the socket.
async fn route_http_get(
    socket: &mut TcpStream,
    server: &AcpServer,
    path: &str,
    cors_headers: &str,
) -> Result<()> {
    match path {
        "/metrics" => {
            let prometheus =
                crate::observability::metrics_exporter::build_prometheus_metrics(server).await;
            // Write Prometheus text format directly
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\n{}\r\n\r\n{}",
                prometheus.len(),
                cors_headers,
                prometheus
            );
            use tokio::io::AsyncWriteExt;
            socket.write_all(response.as_bytes()).await?;
        }
        "/health" => {
            write_http_json_response_with_context(
                socket,
                200,
                serde_json::to_value(server.get_status())?,
                "health",
                cors_headers,
            )
            .await?;
        }
        "/health/ready" => {
            if server.drain_guard.is_draining() {
                // 503 Service Unavailable during drain
                write_http_json_response_with_context(
                    socket,
                    503,
                    serde_json::json!({
                        "ok": false,
                        "status": "draining",
                        "message": "Server is shutting down"
                    }),
                    "health",
                    cors_headers,
                )
                .await?;
            } else {
                write_http_json_response_with_context(
                    socket,
                    200,
                    serde_json::json!({
                        "ok": true,
                        "status": "ready",
                        "healthy": server.is_healthy(),
                    }),
                    "health",
                    cors_headers,
                )
                .await?;
            }
        }
        "/v1/responses" => {
            let data = list_responses_api_payloads(server);
            write_http_json_response_with_context(
                socket,
                200,
                serde_json::json!({
                    "object": "list",
                    "data": data,
                }),
                "responses.api",
                cors_headers,
            )
            .await?;
        }
        "/v1/models" | "/v1/model" | "/models" => {
            write_http_json_response_with_context(
                socket,
                200,
                build_openai_models_response(),
                "openai.chat.completions",
                cors_headers,
            )
            .await?;
        }
        "/" => {
            write_http_json_response_with_context(
                socket,
                200,
                build_root_capabilities_response(),
                "initialize",
                cors_headers,
            )
            .await?;
        }
        _ if extract_response_id_from_path(path).is_some() => {
            let response_id = extract_response_id_from_path(path).ok_or_else(|| {
                anyhow::anyhow!("response_id extraction failed despite prior is_some check")
            })?;
            handle_response_get(socket, server, response_id, cors_headers).await?;
        }
        _ => {
            write_http_json_response_with_context(
                socket,
                404,
                serde_json::json!({"error": t("error.not_found")}),
                "chat",
                cors_headers,
            )
            .await?;
        }
    }
    Ok(())
}

/// Route a POST request — reads body, dispatches to the appropriate handler,
/// and writes the response to the socket. Returns the path label for logging.
///
/// `body_initial_part` is the portion of the body already in the initial buffer read.
#[allow(clippy::question_mark)]
// Intentional — early return for the !path check and JSON parse error below,
// where we write an error response to the socket before returning Ok(path).
// Using `?` would propagate the error upward without writing the response.
async fn route_http_post(
    socket: &mut TcpStream,
    server: Arc<AcpServer>,
    path: &str,
    header_part: &str,
    body_initial_part: &str,
    user_session: Option<crate::acp::r#impl::session::UserSession>,
    cors_headers: &str,
) -> Result<String> {
    let responses_path = path == "/v1/responses";
    let content_length = extract_content_length(header_part).unwrap_or(0);
    if content_length == 0 {
        if responses_path {
            write_http_json_response_with_context(
                socket,
                400,
                build_responses_error(
                    "missing_required_field",
                    "invalid_request_error",
                    t("error.body_required"),
                ),
                "responses.api",
                cors_headers,
            )
            .await?;
        } else {
            write_http_json_response_with_context(
                socket,
                400,
                serde_json::json!({"error": t("error.body_required")}),
                "chat",
                cors_headers,
            )
            .await?;
        }
        return Ok(path.to_string());
    }

    let mut body_bytes = body_initial_part.as_bytes().to_vec();
    if body_bytes.len() < content_length {
        let mut remaining = vec![0u8; content_length - body_bytes.len()];
        tokio::time::timeout(
            std::time::Duration::from_secs(30),
            socket.read_exact(&mut remaining),
        )
        .await
        .map_err(|_| anyhow::anyhow!("timeout reading HTTP body"))?
        .map_err(|e| anyhow::anyhow!("HTTP body read error: {e}"))?;
        body_bytes.extend_from_slice(&remaining);
    }
    body_bytes.truncate(content_length);

    // Enforce max body size (10MB)
    const MAX_BODY_SIZE: usize = 10 * 1024 * 1024;
    if body_bytes.len() > MAX_BODY_SIZE {
        anyhow::bail!(
            "HTTP body too large: {} bytes (max {})",
            body_bytes.len(),
            MAX_BODY_SIZE
        );
    }

    let body: serde_json::Value = match serde_json::from_slice(&body_bytes) {
        Ok(value) => value,
        Err(err) => {
            if responses_path {
                write_http_json_response_with_context(
                    socket,
                    400,
                    build_responses_error(
                        "invalid_request_error",
                        "invalid_request_error",
                        tf("error.invalid_json", &[("error", &err.to_string())]),
                    ),
                    "responses.api",
                    cors_headers,
                )
                .await?;
            } else {
                write_http_json_response_with_context(
                    socket,
                    400,
                    serde_json::json!({"error": tf("error.invalid_json", &[("error", &err.to_string())])}),
                    "chat",
                    cors_headers,
                )
                .await?;
            }
            return Ok(path.to_string());
        }
    };

    let (dispatch_result, duration) =
        crate::observability::performance::utils::measure_time_async(move || async move {
            match path {
                "/chat" => {
                    let params: crate::acp::r#impl::chat::ChatParams =
                        match serde_json::from_value(body) {
                            Ok(value) => value,
                            Err(err) => {
                                write_http_json_response_with_context(
                                socket,
                                400,
                                serde_json::json!({"error": tf("error.invalid_chat_params", &[("error", &err.to_string())])}),
                                "chat",
                                cors_headers,
                            )
                            .await?;
                                return Ok(());
                            }
                        };
                    let trace = http_trace_context("chat");
                    let ctx = Some(crate::acp::r#impl::chat::ChatRequestContext::new(
                        user_session,
                    ));
                    let result = match crate::acp::r#impl::chat::process_chat_request(
                        server.as_ref(),
                        &params,
                        None,
                        &trace,
                        None,
                        ctx,
                    )
                    .await
                    {
                        Ok(result) => result,
                        Err(err) => {
                            write_http_json_response_with_context(
                                socket,
                                502,
                                serde_json::json!({
                                    "error": {
                                        "message": err.to_string(),
                                        "type": "go_on_upstream_error"
                                    }
                                }),
                                "chat",
                                cors_headers,
                            )
                            .await?;
                            return Ok(());
                        }
                    };
                    let result = inject_platform_profiles_if_absent(result, "chat");
                    write_http_json_response(socket, 200, result, cors_headers).await?;
                }
                "/chat/stream" => {
                    let params: crate::acp::r#impl::chat::ChatParams =
                        match serde_json::from_value(body) {
                            Ok(value) => value,
                            Err(err) => {
                                write_http_json_response_with_context(
                                socket,
                                400,
                                serde_json::json!({"error": tf("error.invalid_chat_params", &[("error", &err.to_string())])}),
                                "chat",
                                cors_headers,
                            )
                            .await?;
                                return Ok(());
                            }
                        };
                    write_sse_headers(socket, cors_headers).await?;

                    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
                    let trace = http_trace_context("chat.stream");
                    let ctx = Some(crate::acp::r#impl::chat::ChatRequestContext::new(
                        user_session,
                    ));
                    let server_ref = Arc::clone(&server);
                    let task = tokio::spawn(async move {
                        crate::acp::r#impl::chat::process_chat_request(
                            server_ref.as_ref(),
                            &params,
                            Some(crate::acp::r#impl::chat::StreamObserver::sse(tx)),
                            &trace,
                            None,
                            ctx,
                        )
                        .await
                    });

                    while let Some(frame) = rx.recv().await {
                        if let Err(err) = write_sse_event(socket, frame.event, &frame.payload).await {
                            // Client disconnected while backend task is still active.
                            // Abort task to avoid orphan compute and channel buildup.
                            task.abort();
                            return Err(err);
                        }
                    }

                    match task.await {
                        Ok(Ok(result)) => {
                            let result = inject_platform_profiles_if_absent(result, "chat");
                            write_sse_event(socket, "result", &result).await?
                        }
                        Ok(Err(err)) => {
                            let payload = inject_platform_profiles_if_absent(
                                serde_json::json!({"message": err.to_string()}),
                                "chat",
                            );
                            write_sse_event(
                                socket,
                                "error",
                                &payload,
                            )
                            .await?
                        }
                        Err(err) => {
                            let payload = inject_platform_profiles_if_absent(
                                serde_json::json!({"message": format!("chat task panicked: {err}")}),
                                "chat",
                            );
                            write_sse_event(socket, "error", &payload).await?
                        }
                    }
                }
                "/chat/completions" | "/v1/chat/completions" | "/chat/chat/completions" => {
                    handle_openai_chat_completions(
                        socket,
                        Arc::clone(&server),
                        body,
                        user_session,
                        cors_headers,
                    )
                    .await?;
                }
                "/" | "/rpc" => {
                    // SERIALIZED: Only one RPC call at a time.
                    // server.output is a global singleton used for pipe-based response
                    // capture. Without this lock, concurrent RPC calls would corrupt
                    // the pipe assignment (swap-in → dispatch → swap-out is not atomic).
                    let _rpc_guard = RPC_SERIAL.lock().await;

                    let request: JsonRpcRequest = match serde_json::from_value(body) {
                        Ok(r) => r,
                        Err(e) => {
                            write_http_json_response_with_context(
                                socket,
                                400,
                                serde_json::json!({"error": format!("invalid RPC request: {}", e)}),
                                path,
                                cors_headers,
                            )
                            .await?;
                            return Ok(());
                        }
                    };

                    // Create a pipe to capture the JSON-RPC response written to server.output
                    // Buffer must be large enough to hold all notifications + final response.
                    // AI responses with tool results can exceed 64KB, so use 10MB.
                    let (pipe_writer, mut pipe_reader) = tokio::io::duplex(10 * 1024 * 1024);

                    // Temporarily swap stdout with the pipe writer
                    {
                        let mut guard = server.output.lock().await;
                        let _ = mem::replace(&mut *guard, Box::new(pipe_writer));
                    }

                    // Spawn the RPC handler so we can read from the pipe concurrently,
                    // preventing a deadlock if the response exceeds the duplex buffer size.
                    let server_ref = Arc::clone(&server);
                    let headers_owned = header_part.to_string();
                    let rpc_task = tokio::spawn(async move {
                        handle_request(server_ref.as_ref(), request, Some(&headers_owned)).await
                    });

                    // Read the captured RPC response from the pipe concurrently.
                    // The pipe may contain multiple JSON-RPC messages
                    // (notifications such as chat.stream.chunk + final response).
                    // Parse line by line and find the last line that is a
                    // valid JSON-RPC response (has "id" field).
                    let mut response_bytes = Vec::new();
                    let read_result = tokio::time::timeout(
                        std::time::Duration::from_secs(60),
                        pipe_reader.read_to_end(&mut response_bytes),
                    ).await;

                    // Wait for the RPC task to complete
                    let rpc_result = rpc_task.await;

                    // Restore stdout
                    {
                        let mut guard = server.output.lock().await;
                        let _ = mem::replace(
                            &mut *guard,
                            Box::new(tokio::io::stdout()) as Box<dyn tokio::io::AsyncWrite + Send + Unpin>,
                        );
                    }

                    // Check RPC task result
                    match rpc_result {
                        Err(join_err) => {
                            write_http_json_response_with_context(
                                socket,
                                500,
                                serde_json::json!({"error": format!("RPC task panicked: {}", join_err)}),
                                path,
                                cors_headers,
                            )
                            .await?;
                            return Ok(());
                        }
                        Ok(Err(err)) => {
                            write_http_json_response_with_context(
                                socket,
                                500,
                                serde_json::json!({"error": format!("RPC dispatch error: {}", err)}),
                                path,
                                cors_headers,
                            )
                            .await?;
                            return Ok(());
                        }
                        Ok(Ok(())) => {}
                    }

                    // Check read result
                    read_result
                        .map_err(|_| anyhow::anyhow!("timeout reading RPC pipe response"))?
                        .map_err(|e| anyhow::anyhow!("RPC pipe read error: {e}"))?;

                    let response_str = String::from_utf8_lossy(&response_bytes);
                    // Parse each line; find the last JSON value that has an "id" field
                    // (i.e. a JSON-RPC response, not a notification).
                    let response_value: serde_json::Value = {
                        let mut last_response =
                            serde_json::json!({"raw": response_str.to_string()});
                        for line in response_str.lines() {
                            let trimmed = line.trim();
                            if trimmed.is_empty() {
                                continue;
                            }
                            if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
                                if val.get("id").is_some() {
                                    last_response = val;
                                }
                            }
                        }
                        last_response
                    };

                    write_http_json_response(socket, 200, response_value, cors_headers).await?;
                }
                "/v1/responses" => {
                    handle_responses_api(
                        socket,
                        Arc::clone(&server),
                        body,
                        user_session,
                        cors_headers,
                    )
                    .await?;
                }
                _ => {
                    write_http_json_response_with_context(
                        socket,
                        404,
                        serde_json::json!({"error": t("error.not_found")}),
                        "chat",
                        cors_headers,
                    )
                    .await?;
                }
            }

            Ok(())
        })
        .await;

    let path_label = path.to_string();
    let success = dispatch_result.is_ok();
    crate::observability::performance::record_global_operation(
        success,
        duration.as_secs_f64() * 1000.0,
    );
    info!(
        "HTTP {} completed in {:?} (ok={})",
        path_label, duration, success,
    );

    if let Err(e) = dispatch_result {
        return Err(e);
    }
    Ok(path_label)
}

/// Write a standard HTTP JSON response. Thin wrapper for consistency.
#[allow(dead_code)] // F-GAP-49 — planned wiring: lifecycle/utility
async fn write_http_response(
    socket: &mut TcpStream,
    status: u16,
    body: serde_json::Value,
) -> Result<()> {
    write_http_json_response(socket, status, body, "").await
}

/// Compute CORS response headers for an incoming request.
///
/// Extracts the `Origin` header from the request, checks it against the
/// server's CORS configuration, and returns a formatted string of CORS
/// headers (each ending with `\r\n`).  Returns an empty string when CORS
/// is disabled or the origin is not allowed.
pub(crate) fn compute_cors_response_headers(headers: &str, server: &AcpServer) -> String {
    let config = match server.runtime_config.cors_config() {
        Some(c) => c,
        None => return String::new(),
    };
    let origin = extract_header_value(headers, "origin");
    let cors_headers = build_cors_headers(origin.as_deref(), &config);
    if cors_headers.is_empty() {
        return String::new();
    }
    cors_headers
        .iter()
        .map(|(k, v)| format!("{}: {}\r\n", k, v))
        .collect()
}

/// Handle an OPTIONS (CORS preflight) request.
async fn handle_cors_preflight(
    socket: &mut TcpStream,
    headers: &str,
    server: &AcpServer,
) -> Result<()> {
    let config = match server.runtime_config.cors_config() {
        Some(c) => c,
        None => {
            write_http_json_response(
                socket,
                405,
                serde_json::json!({"error": "Method Not Allowed"}),
                "",
            )
            .await?;
            return Ok(());
        }
    };
    let origin = extract_header_value(headers, "origin");
    let allow_origin = origin.as_deref().filter(|o| is_origin_allowed(o, &config));

    if allow_origin.is_none() && !config.allowed_origins.contains(&"*".to_string()) {
        write_http_json_response(
            socket,
            403,
            serde_json::json!({"error": "Origin not allowed"}),
            "",
        )
        .await?;
        return Ok(());
    }

    let rh = extract_header_value(headers, "access-control-request-headers");
    let preflight_headers = build_preflight_response_headers(rh.as_deref(), &config);
    let origin_val = allow_origin.unwrap_or("*").to_string();

    let mut cors_str = format!("Access-Control-Allow-Origin: {}\r\n", origin_val);
    for (k, v) in &preflight_headers {
        cors_str.push_str(&format!("{}: {}\r\n", k, v));
    }
    cors_str.push_str("Access-Control-Max-Age: ");
    cors_str.push_str(&config.max_age_seconds.to_string());
    cors_str.push_str("\r\n");

    write_http_json_response(socket, 200, serde_json::json!({"ok": true}), &cors_str).await?;
    Ok(())
}

/// Check if the user session is authorized for the given request path and method.
/// Returns `Ok(true)` if a response has been written (request is handled/denied),
/// or `Ok(false)` if the request should proceed.
async fn check_http_authorization(
    socket: &mut TcpStream,
    server: &AcpServer,
    user_session: Option<&crate::acp::r#impl::session::UserSession>,
    method: &str,
    path: &str,
    cors_headers: &str,
) -> Result<bool> {
    // If user auth is disabled, allow everything
    if !server.runtime_config.user_auth_enabled {
        return Ok(false);
    }

    // If no session, reject with 401
    let session = match user_session {
        Some(s) => s,
        None => {
            write_http_json_response(
                socket,
                401,
                serde_json::json!({"error": "Authentication required", "code": "AUTH_REQUIRED"}),
                cors_headers,
            )
            .await?;
            return Ok(true);
        }
    };

    // Exempt paths (health, root capabilities — GET only for root)
    if path == "/health" || (path == "/" && method == "GET") {
        return Ok(false);
    }

    // Map HTTP method + path to required permission
    let required_perm = match (method, path) {
        // Admin-only operations
        ("POST", "/rpc") => Permission::Execute,
        ("GET", _) => Permission::Read,
        ("POST", "/chat" | "/chat/stream") => Permission::Execute,
        ("POST", "/chat/completions" | "/v1/chat/completions") => Permission::Execute,
        ("POST", "/v1/responses") => Permission::Execute,
        _ => Permission::Read,
    };

    // Create principal from session
    let principal = Principal::new(
        &session.user_id,
        session.roles.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        session.tenant_id.as_deref(),
    );

    // Resolve permissions from roles (lock is scoped to avoid holding a non-Send
    // guard across .await points)
    let access_decision = server
        .governance_deps
        .rbac_enforcer
        .as_ref()
        .map(|enforcer| {
            let guard = enforcer.read().unwrap_or_else(|e| e.into_inner());
            let mut p = principal.clone();
            guard.resolve_permissions(&mut p);
            guard.check_access(&p, &required_perm)
        });

    if let Some(decision) = access_decision {
        match decision {
            AccessDecision::Allow => {
                return Ok(false);
            }
            AccessDecision::Deny { reason } => {
                write_http_json_response(
                    socket,
                    403,
                    serde_json::json!({
                        "error": "Forbidden",
                        "code": "ACCESS_DENIED",
                        "reason": reason
                    }),
                    cors_headers,
                )
                .await?;
                return Ok(true);
            }
            AccessDecision::Escalate { required_role } => {
                write_http_json_response(
                    socket,
                    403,
                    serde_json::json!({
                        "error": "Insufficient privileges",
                        "code": "PRIVILEGE_ESCALATION_REQUIRED",
                        "required_role": required_role
                    }),
                    cors_headers,
                )
                .await?;
                return Ok(true);
            }
        }
    }

    // No RBAC enforcer configured — allow (backward compat)
    Ok(false)
}

/// Main HTTP connection handler — parses, guards, routes, and times the request.
async fn handle_http_connection(
    socket: &mut TcpStream,
    server: Arc<AcpServer>,
    peer_addr: SocketAddr,
) -> Result<()> {
    let mut buffer = vec![0u8; 64 * 1024];
    let bytes_read =
        tokio::time::timeout(std::time::Duration::from_secs(30), socket.read(&mut buffer))
            .await
            .map_err(|_| anyhow::anyhow!("timeout reading HTTP request"))??;
    if bytes_read == 0 {
        return Ok(());
    }

    let request_text = String::from_utf8_lossy(&buffer[..bytes_read]);
    let parsed = parse_http_request(&request_text)?;

    // Compute CORS headers for this request (empty string when disabled)
    let cors_headers = compute_cors_response_headers(parsed.header_part, server.as_ref());

    // Extract user session if user auth is enabled
    let user_session: Option<crate::acp::r#impl::session::UserSession> =
        server.session_manager.as_ref().and_then(|sm| {
            let session = sm.extract_user_from_request(parsed.header_part);
            if let Some(ref s) = session {
                debug!("Authenticated user: {} (roles: {:?})", s.user_id, s.roles);
            }
            session
        });

    // ── RBAC authorization check ──────────────────────────────
    if check_http_authorization(
        socket,
        server.as_ref(),
        user_session.as_ref(),
        parsed.method,
        parsed.path,
        &cors_headers,
    )
    .await?
    {
        return Ok(());
    }

    if parsed.method == "OPTIONS" {
        return handle_cors_preflight(socket, parsed.header_part, server.as_ref()).await;
    }

    if http_entry_guard(
        socket,
        server.as_ref(),
        parsed.header_part,
        parsed.method,
        parsed.path,
        peer_addr,
        &cors_headers,
    )
    .await?
    {
        return Ok(());
    }

    if parsed.method == "GET" {
        return route_http_get(socket, server.as_ref(), parsed.path, &cors_headers).await;
    }

    if parsed.method != "POST" {
        write_http_json_response_with_context(
            socket,
            405,
            serde_json::json!({"error": t("error.method_not_allowed")}),
            "chat",
            &cors_headers,
        )
        .await?;
        return Ok(());
    }

    let _path_label = route_http_post(
        socket,
        server,
        parsed.path,
        parsed.header_part,
        parsed.body_initial_part,
        user_session,
        &cors_headers,
    )
    .await?;

    Ok(())
}
fn infer_adaptive_signal(method: &str, path: &str, headers: &str) -> &'static str {
    if matches!(path, "/chat" | "/chat/stream") {
        return "acp_http_path";
    }
    if matches!(
        path,
        "/chat/completions" | "/v1/chat/completions" | "/v1/responses"
    ) {
        return "openai_http_path";
    }
    if path.starts_with("/v1/") {
        return "openai_api_prefix";
    }

    if let Some(protocol_hint) = extract_header_value(headers, "x-go-on-protocol") {
        let hint = protocol_hint.trim().to_ascii_lowercase();
        if hint == "acp" {
            return "header_hint_acp";
        }
        if hint == "mcp" {
            return "header_hint_mcp";
        }
    }

    if let Some(content_type) = extract_header_value(headers, "content-type") {
        if content_type
            .to_ascii_lowercase()
            .contains("application/json")
        {
            if method == "POST" {
                return "json_post_fallback";
            }
            return "json_http_fallback";
        }
    }

    if method == "GET" {
        "read_probe_fallback"
    } else {
        "generic_http_fallback"
    }
}

fn extract_content_length(headers: &str) -> Option<usize> {
    let mut found: Option<usize> = None;
    for line in headers.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if !name.trim().eq_ignore_ascii_case("content-length") {
            continue;
        }
        let val: usize = value.trim().parse().ok()?;
        match found {
            None => found = Some(val),
            Some(prev) if prev == val => {} // duplicate with same value — OK
            Some(_) => return None,         // different values — reject per RFC 7230
        }
    }
    found
}

fn extract_header_value(headers: &str, header_name: &str) -> Option<String> {
    headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.trim().eq_ignore_ascii_case(header_name) {
            Some(value.trim().to_string())
        } else {
            None
        }
    })
}

fn extract_entry_token(headers: &str) -> Option<String> {
    if let Some(auth) = extract_header_value(headers, "authorization") {
        let lower = auth.to_ascii_lowercase();
        if lower.starts_with("bearer ") {
            return Some(auth[7..].trim().to_string());
        }
    }

    extract_header_value(headers, "x-api-key")
        .or_else(|| extract_header_value(headers, "x-go-on-key"))
        .filter(|value| !value.trim().is_empty())
}

fn entry_guard_exempt_path(path: &str) -> bool {
    matches!(path, "/" | "/health")
}

#[allow(clippy::too_many_arguments)]
async fn write_entry_rejection(
    socket: &mut TcpStream,
    status: u16,
    code: &str,
    kind: &str,
    message: String,
    source: &str,
    path: &str,
    policy: &str,
    cors_headers: &str,
) -> Result<()> {
    let trace_id = format!("entry-{}", crate::acp::prelude::now_ts_ms());
    write_http_json_response(
        socket,
        status,
        serde_json::json!({
            "ok": false,
            "error": {
                "code": code,
                "kind": kind,
                "message": message,
                "source": source,
                "path": path,
                "policy": policy,
                "trace_id": trace_id,
            }
        }),
        cors_headers,
    )
    .await
}

async fn apply_entry_guards(
    socket: &mut TcpStream,
    server: &AcpServer,
    headers: &str,
    method: &str,
    path: &str,
    peer_addr: SocketAddr,
    cors_headers: &str,
) -> Result<bool> {
    if entry_guard_exempt_path(path) {
        return Ok(false);
    }

    let source = peer_addr.ip().to_string();

    if server.runtime_config.entry_auth_enabled {
        let env_name = server.runtime_config.entry_auth_api_key_env.trim();
        let expected_key = get_secret(env_name)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());

        if expected_key.is_none() {
            warn!(
                "entry auth enabled but env is missing/empty; denying {} {} from {}",
                method, path, source
            );
            write_entry_rejection(
                socket,
                503,
                "ENTRY_AUTH_MISCONFIGURED",
                "service_unavailable",
                format!(
                    "entry auth is enabled but env '{}' is missing or empty",
                    env_name
                ),
                &source,
                path,
                "entry_auth",
                cors_headers,
            )
            .await?;
            return Ok(true);
        }

        let provided = extract_entry_token(headers)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        if provided != expected_key {
            warn!(
                "entry auth rejected {} {} from {} (missing or invalid key)",
                method, path, source
            );
            write_entry_rejection(
                socket,
                401,
                "ENTRY_AUTH_REQUIRED",
                "unauthorized",
                "missing or invalid entry API key".to_string(),
                &source,
                path,
                "entry_auth",
                cors_headers,
            )
            .await?;
            return Ok(true);
        }
    }

    let key = format!("entry:{}", source);
    let rpm_limit = server.runtime_config.entry_rate_limit_rpm.max(1);
    let burst = server.runtime_config.entry_rate_limit_burst.max(1);
    let allowed = server
        .phase_rate_limiter
        .lock()
        .map(|guard| guard.allow(&key, rpm_limit, Some(burst)))
        .unwrap_or(true);

    if !allowed {
        warn!(
            "entry rate limit rejected {} {} from {} (rpm={}, burst={})",
            method, path, source, rpm_limit, burst
        );
        write_entry_rejection(
            socket,
            429,
            "ENTRY_RATE_LIMITED",
            "rate_limited",
            "entry rate limit exceeded".to_string(),
            &source,
            path,
            "entry_rate_limit",
            cors_headers,
        )
        .await?;
        return Ok(true);
    }

    Ok(false)
}

fn http_trace_context(method: &str) -> RequestTraceContext {
    let request_id = format!("http-{}", crate::acp::prelude::now_ts_ms());
    let seed = Some(serde_json::json!(request_id.clone()));
    let mut trace = chat_trace_context(&seed, "chat.http");
    trace.method = method.to_string();
    trace.request_id = request_id;
    trace
}

async fn write_http_json_response(
    socket: &mut TcpStream,
    status: u16,
    value: serde_json::Value,
    extra_headers: &str,
) -> Result<()> {
    let status_text = match status {
        200 => "OK",
        401 => "Unauthorized",
        429 => "Too Many Requests",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "OK",
    };
    let body = serde_json::to_vec(&value)?;
    let headers = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n{}\r\n",
        status,
        status_text,
        body.len(),
        extra_headers
    );
    tcp_write_timeout(socket, headers.as_bytes()).await?;
    tcp_write_timeout(socket, &body).await?;
    let _ = socket.shutdown().await;
    Ok(())
}

async fn write_sse_headers(
    socket: &mut (impl tokio::io::AsyncWrite + Unpin),
    extra_headers: &str,
) -> Result<()> {
    let header_bytes = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\nX-Accel-Buffering: no\r\n{}\r\n",
        extra_headers
    );
    tcp_write_timeout(socket, header_bytes.as_bytes()).await?;
    Ok(())
}

async fn write_sse_event(
    socket: &mut (impl tokio::io::AsyncWrite + Unpin),
    event: &str,
    payload: &serde_json::Value,
) -> Result<()> {
    // Use a pooled buffer to avoid allocation churn during high-frequency
    // SSE streaming.  The buffer is released back to the pool after writing.
    let mut frame = crate::acp::r#impl::chat::acquire_sse_buffer();
    frame.extend_from_slice(b"event: ");
    frame.extend_from_slice(event.as_bytes());
    frame.extend_from_slice(b"\ndata: ");
    serde_json::to_writer(&mut frame, payload)?;
    frame.extend_from_slice(b"\n\n");
    debug!("ACP SSE event: {}", event);
    tcp_write_timeout(socket, &frame).await?;
    // Release the buffer back to the pool immediately after writing;
    // the flush below only synchronises the socket, not the buffer.
    crate::acp::r#impl::chat::release_sse_buffer(frame);
    tokio::time::timeout(std::time::Duration::from_secs(30), socket.flush())
        .await
        .map_err(|_| anyhow::anyhow!("timeout flushing socket"))?
        .map_err(|e| anyhow::anyhow!("socket flush error: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_to_chat_params_maps_options_and_roles() {
        let req = OpenAiChatRequest {
            model: Some("m1".to_string()),
            messages: vec![
                OpenAiChatMessage {
                    role: "assistant".to_string(),
                    content: serde_json::Value::Null,
                    name: None,
                    tool_call_id: None,
                    function_call: None,
                    tool_calls: Some(serde_json::json!([{"id":"t1"}])),
                    refusal: None,
                },
                OpenAiChatMessage {
                    role: "tool".to_string(),
                    content: serde_json::json!({"result": 3}),
                    name: None,
                    tool_call_id: Some("t1".to_string()),
                    function_call: None,
                    tool_calls: None,
                    refusal: None,
                },
                OpenAiChatMessage {
                    role: "user".to_string(),
                    content: serde_json::json!([
                        {"type":"text","text":"hello"},
                        {"type":"text","text":"world"}
                    ]),
                    name: None,
                    tool_call_id: None,
                    function_call: None,
                    tool_calls: None,
                    refusal: None,
                },
            ],
            temperature: Some(0.2),
            top_p: Some(0.9),
            max_tokens: Some(64),
            n: Some(2),
            stop: Some(serde_json::json!(["END"])),
            presence_penalty: Some(0.1),
            frequency_penalty: Some(0.2),
            logit_bias: Some(serde_json::json!({"10": -1})),
            user: Some("u1".to_string()),
            seed: Some(42),
            response_format: Some(serde_json::json!({"type":"json_object"})),
            tools: Some(serde_json::json!([{"type":"function"}])),
            tool_choice: Some(serde_json::json!("auto")),
            parallel_tool_calls: Some(true),
            function_call: Some(serde_json::json!("auto")),
            functions: Some(serde_json::json!([{"name":"f"}])),
            stream: false,
            extra: std::collections::HashMap::from([(
                "custom_flag".to_string(),
                serde_json::json!(true),
            )]),
        };

        let params = openai_to_chat_params(&req);
        assert_eq!(params.mode, "ask");
        assert_eq!(params.messages.len(), 3);
        assert_eq!(params.messages[1].role, "user"); // tool role normalized
        assert!(params.messages[1].content.contains("tool_call_id"));
        assert!(params.messages[2].content.contains("hello"));

        let options = params.options.expect("expected options");
        assert_eq!(options.extra.get("model"), Some(&serde_json::json!("m1")));
        assert_eq!(
            options.extra.get("max_tokens"),
            Some(&serde_json::json!(64))
        );
        assert_eq!(
            options.extra.get("response_format"),
            Some(&serde_json::json!({"type":"json_object"}))
        );
        assert_eq!(
            options.extra.get("custom_flag"),
            Some(&serde_json::json!(true))
        );
    }

    #[test]
    fn responses_api_maps_input_to_messages() {
        // String input → single user message.
        let text_input = serde_json::json!("hello world");
        let msgs = responses_input_to_messages(&text_input);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].content, "hello world");

        // Array of role/content objects.
        let arr_input = serde_json::json!([
            {"role": "user", "content": "ping"},
            {"role": "assistant", "content": "pong"},
        ]);
        let msgs = responses_input_to_messages(&arr_input);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].content, "ping");
        assert_eq!(msgs[1].role, "assistant");
        assert_eq!(msgs[1].content, "pong");

        // Nested content items (Responses API format).
        let nested_input = serde_json::json!([{
            "role": "user",
            "content": [
                {"type": "input_text", "text": "hello"},
                {"type": "input_text", "text": "world"},
            ]
        }]);
        let msgs = responses_input_to_messages(&nested_input);
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].content.contains("hello"));
        assert!(msgs[0].content.contains("world"));

        // Empty string input should be rejected by mapper.
        let empty_input = serde_json::json!("   ");
        let msgs = responses_input_to_messages(&empty_input);
        assert!(msgs.is_empty());

        // Unsupported input type should produce no mapped messages.
        let invalid_input = serde_json::json!({"text": "not valid responses input"});
        let msgs = responses_input_to_messages(&invalid_input);
        assert!(msgs.is_empty());

        let null_input = serde_json::Value::Null;
        let msgs = responses_input_to_messages(&null_input);
        assert!(msgs.is_empty());
    }

    #[test]
    fn responses_api_id_generation_is_unique() {
        use std::collections::HashSet;

        let mut ids = HashSet::new();
        for _ in 0..128 {
            let id = next_responses_api_id("resp");
            assert!(
                ids.insert(id),
                "next_responses_api_id should never generate duplicate ids"
            );
        }
    }

    #[test]
    fn responses_api_upstream_error_classification_is_stable() {
        let timeout_err = anyhow::anyhow!("request timed out while connecting upstream");
        assert_eq!(
            classify_responses_upstream_error_code(&timeout_err),
            "timeout"
        );

        let rate_limit_err = anyhow::anyhow!("429 too many requests from provider");
        assert_eq!(
            classify_responses_upstream_error_code(&rate_limit_err),
            "rate_limit"
        );

        let generic_err = anyhow::anyhow!("provider returned malformed payload");
        assert_eq!(
            classify_responses_upstream_error_code(&generic_err),
            "upstream_error"
        );
    }

    #[test]
    fn responses_api_stream_event_types_are_correct() {
        // Verify that the expected SSE event type strings are consistent with
        // the contract. These are the canonical event names sent during streaming.
        let event_names = [
            "response.created",
            "response.output_text.delta",
            "response.token_economy",
            "response.completed",
            "response.failed",
        ];

        for name in &event_names {
            assert!(
                name.starts_with("response."),
                "event type must start with 'response.': {name}"
            );
            assert!(
                !name.ends_with('.'),
                "event type must not end with '.': {name}"
            );
        }

        // Verify created event payload shape
        let created_payload = serde_json::json!({
            "type": "response.created",
            "response": {
                "id": "resp_001",
                "object": "response",
                "status": "in_progress",
            }
        });
        assert_eq!(
            created_payload["type"].as_str().unwrap_or_default(),
            "response.created"
        );
        assert_eq!(
            created_payload["response"]["status"]
                .as_str()
                .unwrap_or_default(),
            "in_progress"
        );

        // Verify delta event payload shape
        let delta_payload = serde_json::json!({
            "type": "response.output_text.delta",
            "output_index": 0,
            "content_index": 0,
            "delta": "Hello",
            "item_id": "msg_001",
            "response_id": "resp_001",
        });
        assert_eq!(delta_payload["output_index"].as_u64().unwrap_or(99), 0);
        assert_eq!(delta_payload["content_index"].as_u64().unwrap_or(99), 0);
        assert!(delta_payload["delta"].is_string());
        assert!(delta_payload["item_id"].is_string());
        assert!(delta_payload["response_id"].is_string());

        let telemetry_payload = serde_json::json!({
            "type": "response.token_economy",
            "response_id": "resp_001",
            "token_economy": {
                "compression_ratio": 0.5,
                "input_tokens": 24,
                "output_tokens": 12,
                "total_tokens": 36,
            }
        });
        assert_eq!(
            telemetry_payload["type"].as_str().unwrap_or_default(),
            "response.token_economy"
        );
        assert!(telemetry_payload["token_economy"].is_object());

        // Verify completed event payload shape
        let completed_payload = serde_json::json!({
            "type": "response.completed",
            "response": {
                "id": "resp_001",
                "object": "response",
                "status": "completed",
            }
        });
        assert_eq!(
            completed_payload["response"]["status"]
                .as_str()
                .unwrap_or_default(),
            "completed"
        );
    }

    #[test]
    fn responses_api_r4_golden_snapshot() {
        // 1. Text response: all required top-level fields present with correct types.
        let resp = build_responses_api_response("resp_123_456", "gpt-4", "Hello world");
        assert_eq!(resp["id"].as_str(), Some("resp_123_456"), "id field");
        assert_eq!(resp["object"].as_str(), Some("response"), "object field");
        assert!(resp["created_at"].is_number(), "created_at must be numeric");
        assert_eq!(resp["model"].as_str(), Some("gpt-4"), "model field");
        assert_eq!(resp["status"].as_str(), Some("completed"), "status field");
        assert!(resp["output"].is_array(), "output must be array");
        assert!(resp["usage"].is_object(), "usage must be object");
        assert!(resp["error"].is_null(), "error must be null on success");
        assert!(
            resp["incomplete_details"].is_null(),
            "incomplete_details must be null on success"
        );

        let output = resp["output"].as_array().expect("output array");
        assert_eq!(output.len(), 1, "one output item");
        assert_eq!(output[0]["type"].as_str(), Some("message"), "output type");
        assert_eq!(output[0]["role"].as_str(), Some("assistant"), "output role");
        assert_eq!(
            output[0]["status"].as_str(),
            Some("completed"),
            "output item status"
        );
        let content = output[0]["content"].as_array().expect("content array");
        assert_eq!(content.len(), 1, "one content item");
        assert_eq!(
            content[0]["type"].as_str(),
            Some("output_text"),
            "content type"
        );
        assert_eq!(
            content[0]["text"].as_str(),
            Some("Hello world"),
            "content text"
        );

        let usage = &resp["usage"];
        assert_eq!(usage["input_tokens"].as_u64(), Some(0), "input_tokens");
        assert_eq!(usage["output_tokens"].as_u64(), Some(0), "output_tokens");
        assert_eq!(usage["total_tokens"].as_u64(), Some(0), "total_tokens");

        // 2. Error shape: error object must have code, type, message.
        let err = build_responses_error("invalid_input", "invalid_request_error", "bad field");
        assert_eq!(
            err["error"]["code"].as_str(),
            Some("invalid_input"),
            "error.code"
        );
        assert_eq!(
            err["error"]["type"].as_str(),
            Some("invalid_request_error"),
            "error.type"
        );
        assert_eq!(
            err["error"]["message"].as_str(),
            Some("bad field"),
            "error.message"
        );
        assert!(
            err.as_object().is_none_or(|o| !o.contains_key("id")),
            "error shape must not include id"
        );
        assert!(
            err.as_object().is_none_or(|o| !o.contains_key("status")),
            "error shape must not include status"
        );

        // 3. Tool-call (incomplete) response shape.
        let tc = build_responses_api_tool_call_response(
            "resp_456_789",
            "gpt-4",
            "call_001_2",
            "get_weather",
        );
        assert_eq!(tc["id"].as_str(), Some("resp_456_789"));
        assert_eq!(tc["object"].as_str(), Some("response"));
        assert_eq!(
            tc["status"].as_str(),
            Some("incomplete"),
            "tool-call status"
        );
        let tc_output = tc["output"].as_array().expect("tool-call output array");
        assert_eq!(tc_output.len(), 1, "one tool_call item");
        assert_eq!(
            tc_output[0]["type"].as_str(),
            Some("tool_call"),
            "output item type"
        );
        assert_eq!(
            tc_output[0]["id"].as_str(),
            Some("call_001_2"),
            "tool_call id"
        );
        assert_eq!(
            tc_output[0]["name"].as_str(),
            Some("get_weather"),
            "tool_call name"
        );
        assert!(
            tc["incomplete_details"].is_object(),
            "incomplete_details must be object when status=incomplete"
        );
        assert_eq!(
            tc["incomplete_details"]["reason"].as_str(),
            Some("tool_calls_required")
        );

        // 4. Queued response shape.
        let queued = build_responses_api_queued_response("resp_q_1", "gpt-4");
        assert_eq!(queued["status"].as_str(), Some("queued"));
        assert_eq!(queued["object"].as_str(), Some("response"));
        let queued_output = queued["output"].as_array().expect("queued output array");
        assert!(queued_output.is_empty(), "queued response has empty output");
        assert!(
            queued["error"].is_null(),
            "queued response error must be null"
        );

        // 5. In-progress response shape.
        let in_prog = build_responses_api_in_progress_response("resp_p_1", "gpt-4");
        assert_eq!(in_prog["status"].as_str(), Some("in_progress"));
        assert_eq!(in_prog["object"].as_str(), Some("response"));
        let in_prog_output = in_prog["output"]
            .as_array()
            .expect("in_progress output array");
        assert!(
            in_prog_output.is_empty(),
            "in_progress response has empty output"
        );
    }
}
