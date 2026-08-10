use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tracing::{info, warn};

use crate::acp::r#impl::request::{
    inject_platform_profiles_if_absent, record_tool_call_audit_with_protocol,
    tools_pack::build_mcp_tool_descriptors,
};
use crate::acp::server::AcpServer;

use super::{
    JsonRpcError, JsonRpcRequest, JsonRpcResponse, McpCallToolResult, McpInitializeResult,
    McpListToolsResult, McpServer, JSONRPC_VERSION, MCP_VERSION,
};
use crate::protocol::rpc_protocol::RequestTraceContext;
use crate::tool::ToolInput;

/// Signals an invalid / missing parameter in an MCP request.
/// Dispatched as JSON-RPC INVALID_PARAMS (-32602).
#[derive(Debug)]
struct McpParamError(String);

#[derive(Debug)]
struct McpCodeError {
    code: i32,
    message: String,
}

impl std::fmt::Display for McpParamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for McpParamError {}

impl std::fmt::Display for McpCodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for McpCodeError {}

fn invalid_params(msg: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(McpParamError(msg.into()))
}

fn coded_error(code: i32, msg: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(McpCodeError {
        code,
        message: msg.into(),
    })
}

fn request_timeout_error(timeout_ms: u64) -> anyhow::Error {
    coded_error(
        super::error_codes::REQUEST_TIMEOUT,
        format!("Request timed out after {}ms", timeout_ms),
    )
}

fn request_cancelled_error(request_id: &str) -> anyhow::Error {
    coded_error(
        super::error_codes::REQUEST_CANCELLED,
        format!("Request '{}' was cancelled by client", request_id),
    )
}

// ── MCP sampling/createMessage types ──────────────────────────────────

/// Content item within a sampling result.
#[derive(Debug, Serialize)]
pub struct ContentItem {
    #[serde(rename = "type")]
    pub type_: String,
    pub text: String,
}

/// A single message in a sampling request.
#[derive(Debug, Deserialize)]
pub struct SamplingMessage {
    pub role: String,
    pub content: Value,
}

/// Model preference hints for sampling.
#[derive(Debug, Deserialize)]
pub struct ModelHint {
    pub name: Option<String>,
}

/// Model preferences for sampling.
#[derive(Debug, Deserialize)]
pub struct ModelPreferences {
    #[serde(default)]
    pub hints: Option<Vec<ModelHint>>,
    #[serde(default)]
    pub cost_priority: Option<f64>,
    #[serde(default)]
    pub speed_priority: Option<f64>,
    #[serde(default)]
    pub intelligence_priority: Option<f64>,
}

/// Request for `sampling/createMessage`.
#[derive(Debug, Deserialize)]
pub struct CreateMessageRequest {
    pub messages: Vec<SamplingMessage>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    pub max_tokens: u32,
    #[serde(default)]
    pub stop_sequences: Vec<String>,
    #[serde(default)]
    pub model_preferences: Option<ModelPreferences>,
}

/// Result for `sampling/createMessage`.
#[derive(Debug, Serialize)]
pub struct CreateMessageResult {
    pub role: String,
    pub content: ContentItem,
    pub model: String,
    #[serde(rename = "stopReason")]
    pub stop_reason: String,
}

fn request_id_key(id: &Value) -> String {
    match id {
        Value::String(s) => s.clone(),
        _ => id.to_string(),
    }
}

fn error_code_for(err: &anyhow::Error) -> i32 {
    if err.downcast_ref::<McpParamError>().is_some() {
        super::error_codes::INVALID_PARAMS
    } else if let Some(coded) = err.downcast_ref::<McpCodeError>() {
        coded.code
    } else {
        super::error_codes::INTERNAL_ERROR
    }
}

impl McpServer {
    fn mark_cancelled_request(&self, request_id: &Value) {
        let mut cancelled = self.cancelled_requests.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        // Prevent unbounded growth: evict oldest entry if over 10K limit
        if cancelled.len() >= 10_000 {
            if let Some(oldest) = cancelled.iter().next().cloned() {
                cancelled.remove(&oldest);
            }
        }
        cancelled.insert(request_id_key(request_id));
    }

    fn clear_cancelled_request(&self, request_id: &Value) {
        let mut cancelled = self.cancelled_requests.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("cancelled_requests lock poisoned – recovered data");
            poisoned.into_inner()
        });
        cancelled.remove(&request_id_key(request_id));
    }

    fn is_cancelled_request(&self, request_id: &Value) -> bool {
        self.cancelled_requests
            .lock()
            .unwrap_or_else(|poisoned| {
                tracing::warn!("cancelled_requests lock poisoned in is_cancelled – recovered");
                poisoned.into_inner()
            })
            .contains(&request_id_key(request_id))
    }

    fn request_timeout_ms(&self, request: &JsonRpcRequest) -> u64 {
        request
            .params
            .as_ref()
            .and_then(|params| params.get("timeoutMs"))
            .and_then(Value::as_u64)
            .unwrap_or(30_000)
    }

    fn cancellation_request_id(request: &JsonRpcRequest) -> Option<Value> {
        request
            .params
            .as_ref()
            .and_then(|params| params.get("requestId"))
            .cloned()
    }

    async fn handle_call_tool_with_control(&self, request: &JsonRpcRequest) -> Result<Value> {
        if let Some(ref id) = request.id {
            if self.is_cancelled_request(id) {
                return Err(request_cancelled_error(&request_id_key(id)));
            }
        }

        let timeout_ms = self.request_timeout_ms(request);
        // Blocking tools can execute without yielding; treat extremely small
        // budgets as immediate timeout to preserve deterministic SLA behavior.
        if timeout_ms < 5 {
            return Err(request_timeout_error(timeout_ms));
        }
        let call_result = tokio::time::timeout(
            Duration::from_millis(timeout_ms),
            self.handle_call_tool(request),
        )
        .await;

        match call_result {
            Ok(result) => result,
            Err(_) => Err(request_timeout_error(timeout_ms)),
        }
    }

    fn resolve_prompt_lang(&self, request: &JsonRpcRequest) -> String {
        if let Some(lang) = request
            .params
            .as_ref()
            .and_then(|p| p.get("lang"))
            .and_then(|v| v.as_str())
        {
            return lang.to_string();
        }

        if let Some(acp) = &self.acp_server {
            return acp.runtime_config.i18n_default_language.clone();
        }

        "en".to_string()
    }

    pub async fn handle_request(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse> {
        if request.method != "notifications/cancelled" {
            if let Some(ref id) = request.id {
                if self.is_cancelled_request(id) {
                    let err = request_cancelled_error(&request_id_key(id));
                    let error_data = inject_platform_profiles_if_absent(
                        json!({ "requestId": id }),
                        request.method.as_str(),
                    );
                    return Ok(JsonRpcResponse {
                        jsonrpc: JSONRPC_VERSION.to_string(),
                        result: None,
                        error: Some(JsonRpcError {
                            code: error_code_for(&err),
                            message: err.to_string(),
                            data: Some(error_data),
                        }),
                        id: request.id,
                    });
                }
            }
        }

        // ── Two-phase initialization guard ──────────────────────────────
        // Per the MCP spec, the server MUST reject all methods except
        // "initialize", "ping", and "notifications/cancelled" before the
        // client has sent a valid `initialize` request.
        if !self.initialized.load(Ordering::SeqCst) {
            let method = request.method.as_str();
            let allowed = ["initialize", "ping", "notifications/cancelled"];
            if !allowed.contains(&method) {
                return Ok(JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    result: None,
                    error: Some(JsonRpcError {
                        code: super::error_codes::SERVER_NOT_INITIALIZED,
                        message: "Server not initialized. Send `initialize` first.".to_string(),
                        data: Some(json!({
                            "method": request.method,
                        })),
                    }),
                    id: request.id,
                });
            }
        }

        let result = match request.method.as_str() {
            "initialize" => Ok(self.handle_initialize(&request).await),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(self.handle_list_tools(&request).await),
            "tools/call" => self.handle_call_tool_with_control(&request).await,
            "resources/list" => Ok(self.handle_list_resources(&request).await),
            "resources/read" => self.handle_read_resource(&request).await,
            "resources/subscribe" => {
                // Resource change notifications are not implemented (no event
                // source; the resource list is static). The initialize payload
                // no longer advertises `resources.subscribe`, so this method
                // is rejected rather than silently accepting a subscription
                // that would never receive updates.
                warn!("MCP: resources/subscribe rejected (no change-notification source)");
                return Err(coded_error(
                    super::error_codes::METHOD_NOT_FOUND,
                    "resources/subscribe is not supported".to_string(),
                ));
            }
            "resources/unsubscribe" => {
                warn!("MCP: resources/unsubscribe rejected (no change-notification source)");
                return Err(coded_error(
                    super::error_codes::METHOD_NOT_FOUND,
                    "resources/unsubscribe is not supported".to_string(),
                ));
            }
            "prompts/list" => Ok(self.handle_list_prompts(&request).await),
            "prompts/get" => self.handle_get_prompt(&request).await,
            "agents/list" => Ok(self.handle_list_agents(&request).await),
            "notifications/initialized" => {
                // MCP notification — no response expected per JSON-RPC spec.
                // Zed's context_server client logs an error for id:null responses,
                // so return a silent response marker: id=Some(Value::Null) instead of
                // id=None. The dispatch layer skips sending when id is null sentinel.
                info!("MCP: received notifications/initialized (no response sent)");
                return Ok(JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: Some(serde_json::Value::Null),
                    result: None,
                    error: None,
                });
            }
            "notifications/cancelled" => {
                if let Some(request_id) = Self::cancellation_request_id(&request) {
                    self.mark_cancelled_request(&request_id);
                    info!(
                        "MCP: received notifications/cancelled for request {}",
                        request_id_key(&request_id)
                    );
                } else {
                    warn!("MCP: notifications/cancelled missing requestId");
                }
                return Ok(JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: None,
                    result: None,
                    error: None,
                });
            }
            "logging/setLevel" => {
                let level = request
                    .params
                    .as_ref()
                    .and_then(|p| p.get("level"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                if let Some(ref lvl) = level {
                    // Map MCP log levels to RUST_LOG-compatible directives.
                    // MCP: debug info notice warning error critical alert emergency
                    // RUST_LOG: trace debug info  warn  error
                    let directive = match lvl.as_str() {
                        "debug" => "debug",
                        "info" => "info",
                        "notice" => "info",
                        "warning" => "warn",
                        "error" => "error",
                        "critical" => "error",
                        "alert" => "error",
                        "emergency" => "error",
                        _ => "info",
                    };

                    // Reload the active tracing filter immediately. The
                    // legacy `std::env::set_var("RUST_LOG", ...)` was removed:
                    // set_var is documented as unsound in multithreaded
                    // programs (see src/shared/secret_override.rs) and only
                    // affects hypothetical future subscribers, not the live
                    // filter — reload_log_filter is the real mechanism.
                    match crate::observability::telemetry_enhanced::reload_log_filter(directive) {
                        Ok(()) => {
                            info!("MCP: logging level set to \"{}\" (filter reloaded)", lvl);
                        }
                        Err(e) => {
                            warn!(
                                "MCP: logging level stored as \"{}\" but filter not reloaded: {}",
                                lvl, e
                            );
                        }
                    }
                }
                Ok(json!({}))
            }
            "completion/complete" => {
                // F-GAP-10 — argument name completion for prompts, tools, and resources.
                let ref_obj = request
                    .params
                    .as_ref()
                    .and_then(|p| p.get("ref"))
                    .and_then(|v| v.as_object());

                let ref_type = ref_obj
                    .and_then(|r| r.get("type"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let ref_name = ref_obj
                    .and_then(|r| r.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let argument_name = request
                    .params
                    .as_ref()
                    .and_then(|p| p.get("argument"))
                    .and_then(|v| v.as_object())
                    .and_then(|a| a.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let values: Vec<String> = match ref_type {
                    "ref/prompt" => {
                        if ref_name.is_empty() {
                            // No specific prompt ref — return available top-level completions.
                            // Filter by argument_name if provided.
                            let all: Vec<String> = vec![
                                "template://".to_string(),
                                "agent://".to_string(),
                                "skill://".to_string(),
                            ];
                            if argument_name.is_empty() || argument_name == "name" {
                                all
                            } else {
                                vec![]
                            }
                        } else if argument_name.is_empty() || argument_name == "name" {
                            // Provide name completions based on the ref value.
                            if ref_name.starts_with("agent://") {
                                self.agent_registry
                                    .names()
                                    .iter()
                                    .map(|n| format!("agent://{}", n))
                                    .collect()
                            } else if ref_name.starts_with("template://") {
                                // List available templates from the ACP prompt manager
                                if let Some(acp) = &self.acp_server {
                                    let lang = self.resolve_prompt_lang(&request);
                                    if let Ok(collection) =
                                        acp.prompt_manager.get_all_templates(&lang)
                                    {
                                        collection
                                            .categories
                                            .iter()
                                            .flat_map(|cat| {
                                                cat.templates.iter().map(|t| {
                                                    format!("template://{}.{}", cat.id, t.id)
                                                })
                                            })
                                            .collect()
                                    } else {
                                        vec![]
                                    }
                                } else {
                                    vec![]
                                }
                            } else {
                                vec![]
                            }
                        } else {
                            vec![]
                        }
                    }
                    "ref/resource" => {
                        // Resource template argument completions — return the
                        // actual resource URIs advertised by resources/list so
                        // the completion is functional, not a silent empty list.
                        vec!["go-on://agents".to_string(), "go-on://tools".to_string()]
                    }
                    other => {
                        return Err(coded_error(
                            super::error_codes::INVALID_REQUEST,
                            format!(
                                "Unknown reference type '{}': only ref/prompt and ref/resource are supported",
                                other
                            ),
                        ));
                    }
                };

                info!(
                    count = values.len(),
                    ref_type = ref_type,
                    ref_name = ref_name,
                    arg_name = argument_name,
                    "MCP: completion/complete"
                );
                Ok(json!({
                    "completion": {
                        "values": values,
                        "total": values.len()
                    }
                }))
            }
            "sampling/createMessage" => self.handle_sampling_create_message(&request).await,
            "models/list" => Ok(self.handle_list_models(&request).await),
            _ => {
                warn!("MCP: unknown method '{}'", request.method);
                let error_data =
                    inject_platform_profiles_if_absent(json!({}), "mcp.unknown_method");
                return Ok(JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    result: None,
                    error: Some(JsonRpcError {
                        code: super::error_codes::METHOD_NOT_FOUND,
                        message: format!("Unknown method: {}", request.method),
                        data: Some(error_data),
                    }),
                    id: request.id,
                });
            }
        };

        let (response_result, response_error) = match result {
            Ok(value) => {
                let value = inject_platform_profiles_if_absent(value, request.method.as_str());
                if request.method == "tools/call" {
                    if let Some(ref id) = request.id {
                        self.clear_cancelled_request(id);
                    }
                }
                (Some(value), None)
            }
            Err(err) => {
                let error_data =
                    inject_platform_profiles_if_absent(json!({}), request.method.as_str());
                if request.method == "tools/call" {
                    if let Some(ref id) = request.id {
                        self.clear_cancelled_request(id);
                    }
                }
                (
                    None,
                    Some(JsonRpcError {
                        code: error_code_for(&err),
                        message: err.to_string(),
                        data: Some(error_data),
                    }),
                )
            }
        };

        Ok(JsonRpcResponse {
            jsonrpc: JSONRPC_VERSION.to_string(),
            result: response_result,
            error: response_error,
            id: request.id,
        })
    }

    async fn handle_initialize(&self, request: &JsonRpcRequest) -> Value {
        // ── Version negotiation ───────────────────────────────────────────
        // Extract the client's requested protocol version, defaulting to the
        // latest supported version if not provided.
        let client_version = request
            .params
            .as_ref()
            .and_then(|p| p.get("protocolVersion"))
            .and_then(|v| v.as_str())
            .unwrap_or(MCP_VERSION);

        // Mark the server as initialized so subsequent requests are allowed.
        self.initialized.store(true, Ordering::SeqCst);

        // Negotiate the highest mutually supported version (shared function
        // with the ACP-bridged `mcp.initialize` entry).
        let negotiated_version = crate::mcp::negotiate_mcp_version(client_version);

        if negotiated_version != client_version {
            info!(
                "MCP: version negotiation: client requested '{}', negotiated to '{}'",
                client_version, negotiated_version
            );
        }

        serde_json::to_value(McpInitializeResult::new(
            negotiated_version,
            crate::mcp::mcp_initialize_capabilities(),
            self.server_info.clone(),
        ))
        .unwrap_or_else(|e| {
            warn!("MCP: failed to serialize initialize result: {e}");
            json!({})
        })
    }

    async fn handle_list_tools(&self, _request: &JsonRpcRequest) -> Value {
        if let Some(acp_server) = self.acp_server.as_ref() {
            let mut tools = build_mcp_tool_descriptors(Some(acp_server.as_ref()));
            // Filter to Direct-exposure tools only (deferred/hidden excluded).
            filter_tools_by_exposure(&mut tools, acp_server.as_ref());
            let count = tools.len();
            info!("MCP: Listing {} tools/skills", count);
            let mut result =
                serde_json::to_value(McpListToolsResult::new(tools)).unwrap_or_else(|e| {
                    warn!("MCP: failed to serialize list_tools result: {e}");
                    json!({})
                });
            result["x_skills_available"] = json!(self.skill_registry().is_some());
            return result;
        }

        // Keep fallback semantics aligned with ACP `mcp.tools.list` by
        // reusing the same shared baseline descriptor set.
        let mut tools = build_mcp_tool_descriptors(None);

        tools.sort_by(|a, b| {
            let a_name = a.get("name").and_then(Value::as_str).unwrap_or_default();
            let b_name = b.get("name").and_then(Value::as_str).unwrap_or_default();
            a_name.cmp(b_name)
        });

        let count = tools.len();
        info!("MCP: Listing {} tools/skills", count);
        let mut result = serde_json::to_value(McpListToolsResult::new(tools)).unwrap_or_else(|e| {
            warn!("MCP: failed to serialize list_tools result: {e}");
            json!({})
        });
        result["x_skills_available"] = json!(self.skill_registry().is_some());
        result
    }

    async fn handle_call_tool(&self, request: &JsonRpcRequest) -> Result<Value> {
        let params = request
            .params
            .as_ref()
            .ok_or_else(|| invalid_params("Missing parameters"))?;

        let tool_name = params["name"]
            .as_str()
            .ok_or_else(|| invalid_params("Missing tool name"))?
            .to_string();
        let tool_input = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));

        info!(
            "MCP: Calling tool '{}' with input: {:?}",
            tool_name, tool_input
        );

        // Step 0: Workflow and skill creation tools (require ACP server)
        if let Some(ref acp) = self.acp_server {
            match tool_name.as_str() {
                "workflow_execute" => {
                    let task = tool_input
                        .get("task")
                        .and_then(Value::as_str)
                        .ok_or_else(|| invalid_params("Missing required parameter: task"))?;
                    let params = json!({
                        "task": task,
                        "phase": tool_input.get("phase").and_then(Value::as_str),
                    });
                    let trace = RequestTraceContext {
                        trace_id: "mcp-call".to_string(),
                        span_id: "workflow-execute".to_string(),
                        method: tool_name.clone(),
                        request_id: "mcp-tool-call".to_string(),
                    };
                    crate::acp::r#impl::request::exec_pack::handle_workflow_execute(
                        acp, params, &trace,
                    )
                    .await?;
                    record_tool_call_audit_with_protocol(
                        &tool_name,
                        &tool_input,
                        true,
                        "workflow executed via mcp",
                        "mcp_stdio",
                    );
                    return Ok(serde_json::to_value(McpCallToolResult::new(
                        vec![
                            json!({"type": "text", "text": format!("Workflow executed for task: {}", task)}),
                        ],
                        Some(json!({"ok": true, "task": task})),
                    ))?);
                }
                "workflow_ask" => {
                    let task = tool_input
                        .get("task")
                        .and_then(Value::as_str)
                        .ok_or_else(|| invalid_params("Missing required parameter: task"))?;
                    let params = json!({
                        "task": task,
                        "auto_create_skills": tool_input.get("auto_create_skills").cloned().unwrap_or(json!(true)),
                        "auto_create_workflow": true,
                    });
                    let trace = RequestTraceContext {
                        trace_id: "mcp-call".to_string(),
                        span_id: "workflow-ask".to_string(),
                        method: tool_name.clone(),
                        request_id: "mcp-tool-call".to_string(),
                    };
                    crate::acp::r#impl::request::workflow_pack::handle_workflow_ask(
                        acp, params, &trace,
                    )
                    .await?;
                    record_tool_call_audit_with_protocol(
                        &tool_name,
                        &tool_input,
                        true,
                        "workflow.ask executed via mcp",
                        "mcp_stdio",
                    );
                    return Ok(serde_json::to_value(McpCallToolResult::new(
                        vec![
                            json!({"type": "text", "text": format!("Workflow.ask completed for: {}", task)}),
                        ],
                        Some(json!({"ok": true, "task": task})),
                    ))?);
                }
                "workflow_generate" => {
                    let task = tool_input
                        .get("task")
                        .and_then(Value::as_str)
                        .ok_or_else(|| invalid_params("Missing required parameter: task"))?;
                    let params = json!({"task": task});
                    let trace = RequestTraceContext {
                        trace_id: "mcp-call".to_string(),
                        span_id: "workflow-generate".to_string(),
                        method: tool_name.clone(),
                        request_id: "mcp-tool-call".to_string(),
                    };
                    crate::acp::r#impl::request::workflow_pack::workflow_generate_payload(
                        acp, params, &trace,
                    )
                    .await?;
                    record_tool_call_audit_with_protocol(
                        &tool_name,
                        &tool_input,
                        true,
                        "workflow.generate executed via mcp",
                        "mcp_stdio",
                    );
                    return Ok(serde_json::to_value(McpCallToolResult::new(
                        vec![
                            json!({"type": "text", "text": format!("Workflow generated for: {}", task)}),
                        ],
                        Some(json!({"ok": true, "task": task})),
                    ))?);
                }
                _ => {} // Fall through to unified tool execution chain
            }
        }

        // Steps 1-4: Delegate to the unified tool-execution chain shared with
        // the ACP bridge (`execute_tool_call`). This single chain performs:
        //  1. HarnessBus sandbox / require_review / budget / RBAC checks
        //     (or the default-governance fallback when no HarnessBus is wired)
        //  2. Idempotency dedup via the IdempotencyCache
        //  3. Budget accounting (wall-clock / call-count / PUA tokens)
        //  4. Pre-execute hook chain (async) then `run_async` execution
        //  5. Tool registry → skill registry → imported-skill fallback
        // It used to be re-implemented here (~350 lines), which let the two
        // paths drift (e.g. `tools/list` advertised bridge-only tools that
        // this arm could not execute). The MCP arm now passes its own tool
        // registry so registered tools resolve exactly like on the ACP side.
        if let Some(ref acp) = self.acp_server {
            let structured = match crate::acp::r#impl::request::tools_pack::execute_tool_call(
                acp,
                &self.tool_registry,
                &tool_name,
                &tool_input,
            )
            .await
            {
                Ok(value) => value,
                Err(e) => {
                    // Keep the MCP error-code contract: an unknown tool or
                    // skill is a parameter problem (INVALID_PARAMS), not an
                    // internal error. Governance denials keep their original
                    // (non-param) code so callers can distinguish policy
                    // blocks from bad input.
                    if e.to_string().contains("unknown tool or skill") {
                        return Err(invalid_params(e.to_string()));
                    }
                    return Err(e);
                }
            };
            return Ok(serde_json::to_value(McpCallToolResult::new(
                vec![json!({
                    "type": "text",
                    "text": serde_json::to_string(&structured)?,
                })],
                Some(structured),
            ))?);
        }

        // Minimal server (no ACP server): fall back to the local tool registry
        // and skill registry without governance (governance is delegated to the
        // server side when an ACP server is present).
        if let Some(tool) = self.tool_registry.get_arc(&tool_name) {
            crate::shared::tool_descriptors::validate_required_arguments(&tool_name, &tool_input)
                .map_err(|e| invalid_params(e.to_string()))?;
            let input = ToolInput {
                task_id: request
                    .id
                    .as_ref()
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "mcp-tool-call".to_string()),
                phase: "mcp".to_string(),
                agent_role: "tool".to_string(),
                objective: format!("Execute MCP tool '{}'", tool_name),
                constraints: None,
                evidence: None,
                payload: tool_input.clone(),
                allowed_base_dir: None,
            };
            self.tool_registry
                .hooks
                .run_pre_async(&tool_name, &input)
                .await?;
            let result = tool.run_async(input).await?;
            record_tool_call_audit_with_protocol(
                &tool_name,
                &tool_input,
                true,
                "tool executed via mcp (minimal server)",
                "mcp_stdio",
            );
            let result_value: Value = serde_json::to_value(&result)?;
            return Ok(serde_json::to_value(McpCallToolResult::new(
                vec![json!({
                    "type": "text",
                    "text": serde_json::to_string(&result)?,
                })],
                Some(result_value),
            ))?);
        }

        // Skill registry fallback (minimal server)
        if let Some(registry) = self.skill_registry() {
            let skill_to_call = match registry.read() {
                Ok(guard) => {
                    if let Some(skill) = guard.get(&tool_name) {
                        Some((tool_name.clone(), skill.clone()))
                    } else if let Some(best_match) =
                        guard.best_match_with_input(&tool_name, &tool_input)
                    {
                        guard
                            .get(&best_match)
                            .map(|skill| (best_match.clone(), skill.clone()))
                    } else {
                        None
                    }
                }
                Err(_) => None,
            };

            if let Some((resolved_name, skill)) = skill_to_call {
                let result = skill.execute(&tool_input).await?;
                record_tool_call_audit_with_protocol(
                    &resolved_name,
                    &tool_input,
                    true,
                    "skill executed via mcp (minimal server)",
                    "mcp_stdio",
                );
                let mut response = serde_json::to_value(McpCallToolResult::new(
                    vec![json!({
                        "type": "text",
                        "text": serde_json::to_string_pretty(&result)?,
                    })],
                    Some(result),
                ))?;
                if resolved_name != tool_name {
                    response["x_resolved_skill"] = json!(resolved_name);
                }
                return Ok(response);
            }
        }

        // Not found — error
        warn!("MCP: Unknown tool or skill '{}'", tool_name);
        Err(invalid_params(format!(
            "Unknown tool or skill: {}",
            tool_name
        )))
    }

    async fn handle_list_resources(&self, _request: &JsonRpcRequest) -> Value {
        // Shared with the ACP bridge (single source).
        match crate::acp::r#impl::request::protocol_pack::mcp::mcp_resources_list_value() {
            Ok(value) => value,
            Err(e) => {
                warn!("MCP: failed to build resources list: {e}");
                json!({})
            }
        }
    }

    async fn handle_read_resource(&self, request: &JsonRpcRequest) -> Result<Value> {
        let params = request
            .params
            .as_ref()
            .ok_or_else(|| invalid_params("Missing parameters"))?;

        let uri = params["uri"]
            .as_str()
            .ok_or_else(|| invalid_params("Missing URI"))?;

        let agents: Vec<String> = self.agent_registry.names();
        let tools: Vec<String> = self
            .tool_registry
            .names()
            .into_iter()
            .map(ToString::to_string)
            .collect();
        // Shared with the ACP bridge (single source).
        crate::acp::r#impl::request::protocol_pack::mcp::mcp_resources_read_value(
            &agents, &tools, uri,
        )
        .map_err(|e| {
            warn!("MCP: unknown resource '{}': {}", uri, e);
            invalid_params(format!("Unknown resource: {}", uri))
        })
    }

    async fn handle_list_agents(&self, _request: &JsonRpcRequest) -> Value {
        info!("MCP: Listing available agents from agent_registry");
        json!({ "agents": self.agent_registry.names() })
    }

    async fn handle_list_models(&self, _request: &JsonRpcRequest) -> Value {
        info!("MCP: Listing available models");
        // Unify with the ACP bridge `models.list` / `models/list` payload so
        // both entries return the same rich structure (id/name/provider/
        // is_default/capabilities/context_window) instead of two divergent
        // schemas that confused clients depending on the entry point.
        if let Some(ref acp) = self.acp_server {
            if let Ok(payload) = crate::acp::r#impl::request::protocol_pack::models_list_payload(
                acp.as_ref(),
                Value::Null,
            )
            .await
            {
                return payload;
            }
        }
        // Fallback (minimal server without an ACP server): keep the previous
        // per-agent grouping so `models/list` still returns useful data.
        let models = self
            .agent_registry
            .models()
            .into_iter()
            .map(|(agent, default_model, models)| {
                json!({
                    "agent": agent,
                    "default_model": default_model,
                    "models": models,
                })
            })
            .collect::<Vec<_>>();
        json!({ "models": models })
    }

    /// Handler for `sampling/createMessage`.
    ///
    /// Routes the sampling request through the agent system to get a completion.
    /// Messages are converted from MCP format to internal agent format,
    /// and the streaming response is collected into a single result.
    async fn handle_sampling_create_message(&self, request: &JsonRpcRequest) -> Result<Value> {
        let params = request
            .params
            .as_ref()
            .ok_or_else(|| invalid_params("Missing parameters"))?;

        let create_request: CreateMessageRequest = serde_json::from_value(params.clone())
            .map_err(|e| invalid_params(format!("Invalid sampling request: {}", e)))?;

        // Determine which agent to use based on model preferences
        let (agent_name, overridden_model) = create_request
            .model_preferences
            .as_ref()
            .map(|prefs| {
                let name = prefs
                    .hints
                    .as_ref()
                    .and_then(|hints| hints.first())
                    .and_then(|hint| hint.name.as_ref())
                    .cloned()
                    .unwrap_or_else(|| {
                        // Use priority hints to select a suitable agent
                        if prefs.cost_priority.unwrap_or(0.0) > 0.7 {
                            "economy"
                        } else if prefs.speed_priority.unwrap_or(0.0) > 0.7 {
                            "fast"
                        } else if prefs.intelligence_priority.unwrap_or(0.0) > 0.7 {
                            "reasoning"
                        } else {
                            "primary"
                        }
                        .to_string()
                    });
                (
                    name,
                    prefs
                        .hints
                        .as_ref()
                        .and_then(|h| h.first())
                        .and_then(|h| h.name.clone()),
                )
            })
            .unwrap_or_else(|| ("primary".to_string(), None));

        let agent = self
            .agent_registry
            .get(&agent_name)
            .ok_or_else(|| invalid_params(format!("Agent '{}' not found", agent_name)))?;

        // Convert MCP messages to internal agent Messages
        let mut agent_messages: Vec<crate::agents::agent::Message> = Vec::new();

        if let Some(ref system_prompt) = create_request.system_prompt {
            agent_messages.push(crate::agents::agent::Message {
                role: "system".to_string(),
                content: system_prompt.clone(),
            });
        }

        for msg in &create_request.messages {
            let text = msg
                .content
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            agent_messages.push(crate::agents::agent::Message {
                role: msg.role.clone(),
                content: text,
            });
        }

        // Build options from request parameters and model preferences
        let mut options = HashMap::new();
        options.insert("max_tokens".to_string(), json!(create_request.max_tokens));
        if !create_request.stop_sequences.is_empty() {
            options.insert("stop".to_string(), json!(create_request.stop_sequences));
        }
        // Inject model override from preferences, if provided
        if let Some(ref model_override) = overridden_model {
            options.insert("model".to_string(), json!(model_override));
        }
        // Pass preference hints to the agent so it can optimize accordingly
        if let Some(ref prefs) = create_request.model_preferences {
            if let Some(cost) = prefs.cost_priority {
                options.insert("cost_priority".to_string(), json!(cost));
            }
            if let Some(speed) = prefs.speed_priority {
                options.insert("speed_priority".to_string(), json!(speed));
            }
            if let Some(intel) = prefs.intelligence_priority {
                options.insert("intelligence_priority".to_string(), json!(intel));
            }
        }

        // Channel to collect streaming response
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let sender = crate::agents::agent::StreamingSender::new(tx);

        let model_name = agent
            .default_model()
            .map(|m| m.id.clone())
            .unwrap_or_else(|| agent_name.clone());

        // Call the agent
        agent
            .chat(agent_messages, None, Some(options), sender)
            .await
            .map_err(|e| anyhow::anyhow!("Sampling request failed: {}", e))?;

        // Collect streaming tokens
        let mut full_text = String::new();
        while let Some(token) = rx.recv().await {
            full_text.push_str(&token);
        }

        info!(
            "MCP: sampling/createMessage completed ({} chars generated)",
            full_text.len()
        );

        let result = CreateMessageResult {
            role: "assistant".to_string(),
            content: ContentItem {
                type_: "text".to_string(),
                text: full_text,
            },
            model: model_name,
            stop_reason: "endTurn".to_string(),
        };

        Ok(serde_json::to_value(result)?)
    }

    /// Handler for `prompts/list`.
    ///
    /// Returns available prompt templates. Currently supports:
    /// - system prompts derived from agent configurations
    /// - phase-specific task prompts
    ///
    /// Handler for `prompts/list`.
    ///
    /// This is a lightweight implementation that surfaces the agent system prompts
    /// as discoverable prompt templates.  Full template parameterisation is a
    /// future enhancement.
    async fn handle_list_prompts(&self, request: &JsonRpcRequest) -> Value {
        let lang = self.resolve_prompt_lang(request);
        let agents: Vec<String> = self.agent_registry.names();
        // Shared with the ACP bridge (single source).
        crate::acp::r#impl::request::protocol_pack::mcp::mcp_prompts_list_value(
            self.acp_server.as_deref(),
            &agents,
            &lang,
        )
    }

    /// Handler for `prompts/get`.
    ///
    /// Returns a resolved prompt template by name.  Supports agent system prompts
    /// (prefixed with `agent://`) and built-in skill prompts (prefixed with `skill://`).
    async fn handle_get_prompt(&self, request: &JsonRpcRequest) -> Result<Value> {
        let name = request
            .params
            .as_ref()
            .and_then(|p| p.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let lang = self.resolve_prompt_lang(request);

        // Shared resolvers (single source with the ACP bridge).
        if let Some(resolved) =
            crate::acp::r#impl::request::protocol_pack::mcp::mcp_prompts_get_template_value(
                self.acp_server.as_deref(),
                name,
                &lang,
                request.params.as_ref(),
            )
        {
            return Ok(resolved);
        }

        // Try to resolve as an agent system prompt
        if let Some(messages) =
            crate::acp::r#impl::request::protocol_pack::mcp::mcp_prompts_get_agent_value(
                Some(&self.agent_registry),
                name,
            )
        {
            return Ok(json!({
                "description": format!("Agent system prompt for '{}'", name),
                "messages": messages
            }));
        }

        Err(coded_error(
            super::error_codes::INVALID_REQUEST,
            format!("Prompt '{}' not found", name),
        ))
    }
}

/// Filter a list of MCP tool descriptors to only include Direct-exposure tools.
///
/// Niche/domain-specific tools (CAD, 3D, GIS, games, barcodes, etc.)
/// are classified as `Deferred` — they are hidden from the default tool list
/// but discoverable via the `tool_search` / `skill-finder` tools.
///
/// Infrastructure tools (goon_*, acp_*, prompts_*, skill-*) are always kept.
fn filter_tools_by_exposure(tools: &mut Vec<Value>, _server: &AcpServer) {
    // Deferred tool name prefixes — niche domains not needed in everyday use.
    const DEFERRED_PREFIXES: &[&str] = &[
        "stl_", "obj_", "dxf_", "step_", "ply_", "iges_", "gltf_", "gcode_", "gpx_", "geo_",
        "svg_", "barcode_", "game_", "cad_", "image_",
    ];
    // Deferred exact tool names.
    const DEFERRED_NAMES: &[&str] = &[
        "read_docx",
        "read_excel",
        "read_ppt",
        "read_pdf",
        "write_docx",
        "write_excel",
        "write_ppt",
        "pdf_merge",
        "pdf_split",
        "email_parse",
        "invoice_parse",
        "rss_read",
        "sqlite_query",
        "dns_lookup",
        "ping",
        "port_scan",
        "csv_analyze",
        "csv_write",
        "csv_transform",
        "toml_write",
        "yaml_write",
        "web_scrape",
        "docker_build",
        "docker_push",
        "lint_run",
        "template_render",
        "search_packages",
        "security_scan",
        "uuid_gen",
        "random_token",
        "encode_decode",
        "hash_file",
        "file_watch",
        "file_diff",
        "read_file_lines",
        "code_metrics",
        "code_index_search",
    ];

    tools.retain(|tool| {
        let name = match tool.get("name").and_then(Value::as_str) {
            Some(n) => n,
            None => return true,
        };
        // Always keep infrastructure tools.
        if name.starts_with("goon_")
            || name.starts_with("acp_")
            || name.starts_with("prompts_")
            || name == "skill-finder"
            || name == "skill-creator"
            || name == "builtin.echo"
            || name == "echo_skill"
        {
            return true;
        }
        // Check if this is a deferred (niche) tool.
        if DEFERRED_NAMES.contains(&name) {
            return false;
        }
        if DEFERRED_PREFIXES.iter().any(|p| name.starts_with(p)) {
            return false;
        }
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};

    use crate::acp::server::ServerBuilder;
    use crate::governance::harness_bus::default_harness_bus;
    use crate::tool::{Tool, ToolHook, ToolOutput, ToolRegistry as OrchestrationToolRegistry};

    // ── Test doubles ─────────────────────────────────────────────────────

    /// A tool whose name deliberately avoids every read/write/shell/network/
    /// search keyword in `ToolCapabilityRegistry`, so `check_tool_call`
    /// reports `require_review = true` (unknown operation). Registered tools
    /// pass the require_review gate (same policy as the ACP route) but still
    /// run the pre-execute hook chain.
    #[derive(Clone)]
    struct UnclassifiedTestTool {
        name: &'static str,
        calls: Arc<AtomicUsize>,
    }

    impl Tool for UnclassifiedTestTool {
        fn name(&self) -> &'static str {
            self.name
        }

        fn run(&self, _input: &ToolInput) -> anyhow::Result<ToolOutput> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(ToolOutput {
                success: true,
                result: Some(json!({ "ok": true })),
                error: None,
                verification: None,
                audit_log: None,
                pua_report: None,
            })
        }
    }

    /// A pre-execute review hook (the same mechanism `GuardianHook` uses)
    /// that records reviewed tool names; when `deny` is set it blocks the
    /// call with an error.
    #[derive(Clone)]
    struct RecordingReviewHook {
        reviewed: Arc<StdMutex<Vec<String>>>,
        deny: bool,
    }

    #[async_trait]
    impl ToolHook for RecordingReviewHook {
        async fn async_pre_execute(
            &self,
            tool_name: &str,
            _input: &ToolInput,
        ) -> anyhow::Result<()> {
            if let Ok(mut reviewed) = self.reviewed.lock() {
                reviewed.push(tool_name.to_string());
            }
            if self.deny {
                anyhow::bail!("test review hook denied tool '{}'", tool_name);
            }
            Ok(())
        }
    }

    async fn initialize(server: &McpServer) {
        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "initialize".to_string(),
            params: Some(json!({ "protocolVersion": "2024-11-05" })),
            id: Some(json!(0)),
        };
        let response = server
            .handle_request(request)
            .await
            .expect("initialize should return a response");
        assert!(response.result.is_some(), "initialize must succeed");
    }

    /// Build an McpServer whose ACP server carries a real HarnessBus so the
    /// unified governance chain (`check_tool_call` via `validate_action` +
    /// pre-execute hooks + `run_async`) is exercised end-to-end.
    fn build_gov_server(
        registry: OrchestrationToolRegistry,
    ) -> (McpServer, Arc<crate::governance::harness_bus::HarnessBus>) {
        let mut acp_server = ServerBuilder::new().build();
        let harness_bus = Arc::new(default_harness_bus());
        acp_server.governance_deps.harness_bus = Some(Arc::clone(&harness_bus));
        let server = McpServer::new_with_acp(
            Arc::new(crate::agent::AgentRegistry::new()),
            Arc::new(registry),
            "go-on".to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
            Some(Arc::new(acp_server)),
        );
        (server, harness_bus)
    }

    async fn call_tool(
        server: &McpServer,
        tool_name: &str,
        arguments: Value,
        id: i64,
    ) -> JsonRpcResponse {
        server
            .handle_request(JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "tools/call".to_string(),
                params: Some(json!({ "name": tool_name, "arguments": arguments })),
                id: Some(json!(id)),
            })
            .await
            .expect("tools/call must produce a response envelope")
    }

    // ── Task 1: MCP tool execution runs the unified governance chain ────

    #[tokio::test]
    async fn mcp_tool_call_runs_review_hook_chain_before_execution() {
        // A registered-but-unclassified tool: `check_tool_call` reports
        // require_review = true and the registered pre-execute review hook
        // must fire before the tool runs — proving the MCP entry is no
        // longer a governance-weakened path (check_tool_call + async hook
        // chain + run_async all run here).
        let mut registry = OrchestrationToolRegistry::new_empty();
        let calls = Arc::new(AtomicUsize::new(0));
        let reviewed = Arc::new(StdMutex::new(Vec::new()));
        registry.register(UnclassifiedTestTool {
            name: "test_zzz_gov_review",
            calls: Arc::clone(&calls),
        });
        registry.hooks.register(Arc::new(RecordingReviewHook {
            reviewed: Arc::clone(&reviewed),
            deny: false,
        }));
        let (server, _harness_bus) = build_gov_server(registry);
        initialize(&server).await;

        let response = call_tool(&server, "test_zzz_gov_review", json!({}), 10).await;
        assert!(
            response.error.is_none(),
            "governance review should allow the call, got error: {:?}",
            response.error
        );
        let result = response.result.expect("tool result must be present");
        assert_eq!(result["structuredContent"]["success"], true);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "tool must have executed exactly once"
        );
        // The review hook (same mechanism as GuardianHook) must have fired
        // on the MCP path before execution.
        let reviewed_names = reviewed.lock().unwrap();
        assert_eq!(
            reviewed_names.as_slice(),
            &["test_zzz_gov_review".to_string()],
            "pre-execute review hook must run before execution via the MCP path"
        );
    }

    #[tokio::test]
    async fn mcp_tool_call_denied_by_review_hook_is_blocked_fail_fast() {
        let mut registry = OrchestrationToolRegistry::new_empty();
        let calls = Arc::new(AtomicUsize::new(0));
        registry.register(UnclassifiedTestTool {
            name: "test_zzz_gov_deny",
            calls: Arc::clone(&calls),
        });
        registry.hooks.register(Arc::new(RecordingReviewHook {
            reviewed: Arc::new(StdMutex::new(Vec::new())),
            deny: true,
        }));
        let (server, _harness_bus) = build_gov_server(registry);
        initialize(&server).await;

        let response = call_tool(&server, "test_zzz_gov_deny", json!({}), 11).await;
        let error = response
            .error
            .expect("denying review hook must block the call");
        assert!(
            error.message.contains("test review hook denied"),
            "error must surface the hook denial, got: {}",
            error.message
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "tool must NOT execute when the review hook denies"
        );
    }

    // ── Task 2: IdempotencyCache dedup via the MCP path ─────────────────

    #[tokio::test]
    async fn mcp_tool_call_dedup_repeated_tool_args_via_idempotency_cache() {
        let mut registry = OrchestrationToolRegistry::new_empty();
        let calls = Arc::new(AtomicUsize::new(0));
        registry.register(UnclassifiedTestTool {
            name: "test_zzz_gov_idem",
            calls: Arc::clone(&calls),
        });
        let (server, harness_bus) = build_gov_server(registry);
        initialize(&server).await;

        let arguments = json!({ "k": "v" });
        // First call executes and records the result in the IdempotencyCache
        // keyed by (tool, args) hash.
        let first = call_tool(&server, "test_zzz_gov_idem", arguments.clone(), 20).await;
        assert!(
            first.error.is_none(),
            "first call must succeed, got error: {:?}",
            first.error
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Second call with identical (tool, args) must hit the cache: skip
        // re-execution and return the cached result.
        let second = call_tool(&server, "test_zzz_gov_idem", arguments.clone(), 21).await;
        assert!(
            second.error.is_none(),
            "cached call must succeed, got error: {:?}",
            second.error
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "repeated (tool, args) call must not re-execute the tool"
        );
        assert_eq!(
            second.result.expect("cached result").clone(),
            first.result.expect("first result").clone(),
            "the cached response must match the original execution response"
        );

        // The governance profile must accumulate the idempotency hit.
        let profile = harness_bus.governance_profile();
        assert!(
            profile.idempotency_hits >= 1,
            "idempotency_hits must accumulate on a cache hit, got {}",
            profile.idempotency_hits
        );
    }
}
