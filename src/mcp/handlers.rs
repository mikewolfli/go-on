use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Duration;
use tracing::{info, warn};

use crate::acp::r#impl::request::{
    inject_platform_profiles_if_absent, record_tool_call_audit_with_protocol,
    tools_pack::build_mcp_tool_descriptors,
};
use crate::protocol::rpc_protocol::RequestTraceContext;
use crate::tool::ToolInput;

use super::tools::validate_required_arguments;
use super::{
    JsonRpcError, JsonRpcRequest, JsonRpcResponse, McpCallToolResult, McpInitializeResult,
    McpListResourcesResult, McpListToolsResult, McpResource, McpServer, JSONRPC_VERSION,
    MCP_VERSION, SUPPORTED_MCP_VERSIONS,
};

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
    #[allow(dead_code)] // F-GAP reserved
    pub cost_priority: Option<f64>,
    #[serde(default)]
    #[allow(dead_code)] // F-GAP reserved
    pub speed_priority: Option<f64>,
    #[serde(default)]
    #[allow(dead_code)] // F-GAP reserved
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
    #[serde(default)]
    #[allow(dead_code)] // F-GAP reserved
    pub metadata: Option<HashMap<String, Value>>,
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

        let result = match request.method.as_str() {
            "initialize" => Ok(self.handle_initialize(&request).await),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(self.handle_list_tools(&request).await),
            "tools/call" => self.handle_call_tool_with_control(&request).await,
            "resources/list" => Ok(self.handle_list_resources(&request).await),
            "resources/read" => self.handle_read_resource(&request).await,
            "resources/subscribe" => {
                let uri = request
                    .params
                    .as_ref()
                    .and_then(|p| p.get("uri"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let uri = match uri {
                    Some(u) => u,
                    None => return Err(invalid_params("missing 'uri' in params")),
                };
                {
                    let mut subs = self
                        .resource_subscriptions
                        .lock()
                        .unwrap_or_else(|poisoned| {
                            warn!("resource_subscriptions lock poisoned, recovering");
                            poisoned.into_inner()
                        });
                    // Use the request id as the subscriber identifier.
                    let subscriber = request
                        .id
                        .as_ref()
                        .map(|id| id.to_string())
                        .unwrap_or_default();
                    subs.entry(uri.clone()).or_default().insert(subscriber);
                }
                info!("MCP: subscribed to resource '{}'", uri);
                Ok(json!({"meta": {}}))
            }
            "resources/unsubscribe" => {
                let uri = request
                    .params
                    .as_ref()
                    .and_then(|p| p.get("uri"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let uri = match uri {
                    Some(u) => u,
                    None => return Err(invalid_params("missing 'uri' in params")),
                };
                {
                    let mut subs = self
                        .resource_subscriptions
                        .lock()
                        .unwrap_or_else(|poisoned| {
                            warn!("resource_subscriptions lock poisoned, recovering");
                            poisoned.into_inner()
                        });
                    let subscriber = request
                        .id
                        .as_ref()
                        .map(|id| id.to_string())
                        .unwrap_or_default();
                    if let Some(members) = subs.get_mut(&uri) {
                        members.retain(|s| s != &subscriber);
                        if members.is_empty() {
                            subs.remove(&uri);
                        }
                    }
                }
                info!("MCP: unsubscribed from resource '{}'", uri);
                Ok(json!({"meta": {}}))
            }
            "prompts/list" => Ok(self.handle_list_prompts(&request).await),
            "prompts/get" => self.handle_get_prompt(&request).await,
            "agents/list" => Ok(self.handle_list_agents(&request).await),
            "notifications/initialized" => {
                // MCP notification — no response expected per JSON-RPC spec.
                // Zed's context_server client logs an error for id:null responses,
                // so we skip sending any response at all.
                info!("MCP: received notifications/initialized (no response sent)");
                return Ok(JsonRpcResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: None,
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
                    let mut guard = self.logging_level.lock().unwrap_or_else(|poisoned| {
                        tracing::warn!("lock poisoned, recovering");
                        poisoned.into_inner()
                    });
                    *guard = Some(lvl.clone());

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

                    // Update RUST_LOG so any future subscribers pick it up.
                    std::env::set_var("RUST_LOG", directive);

                    // Try to reload the active tracing filter immediately.
                    match crate::observability::telemetry_enhanced::reload_log_filter(directive) {
                        Ok(()) => {
                            info!(
                                "MCP: logging level set to \"{}\" (RUST_LOG={})",
                                lvl, directive
                            );
                        }
                        Err(e) => {
                            warn!(
                                "MCP: logging level stored as \"{}\" but filter not reloaded: {}; RUST_LOG set for future subscribers",
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
                        // Resource template argument completions — at minimum return
                        // an empty list; this avoids silent failures for supported types.
                        vec![]
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

        // Negotiate the highest mutually supported version.
        let negotiated_version = SUPPORTED_MCP_VERSIONS
            .iter()
            .rev()
            .find(|v| **v == client_version)
            .copied()
            .unwrap_or(MCP_VERSION);

        if negotiated_version != client_version {
            info!(
                "MCP: version negotiation: client requested '{}', negotiated to '{}'",
                client_version, negotiated_version
            );
        }

        serde_json::to_value(McpInitializeResult::new(
            negotiated_version,
            json!({
                "resources": {
                    "subscribe": true,
                    "listChanged": true
                },
                "tools": {
                    "listChanged": true
                },
                "prompts": {
                    "listChanged": true
                },
                "roots": {
                    "listChanged": false
                },
                "sampling": {},
                "experimental": {
                    "agents": {}
                }
            }),
            self.server_info.clone(),
        ))
        .unwrap_or_else(|e| {
            warn!("MCP: failed to serialize initialize result: {e}");
            json!({})
        })
    }

    async fn handle_list_tools(&self, _request: &JsonRpcRequest) -> Value {
        if let Some(acp_server) = self.acp_server.as_ref() {
            let tools = build_mcp_tool_descriptors(Some(acp_server.as_ref()));
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

        // Inject registered skills from ACP server (if available)
        if let Some(registry) = self.skill_registry() {
            let guard = registry.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("MCP skill_registry lock poisoned – recovered");
                poisoned.into_inner()
            });
            for descriptor in guard.list() {
                tools.push(json!({
                    "name": descriptor.name,
                    "description": descriptor.description,
                    "input_schema": descriptor.input_schema,
                }));
            }
        }

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
                        acp, params, None, &trace,
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
                        acp, params, None, &trace,
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
                    crate::acp::r#impl::request::workflow_pack::handle_workflow_generate(
                        acp, params, None, &trace,
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
                _ => {} // Fall through to tool_registry + skill_registry
            }
        }

        // Step 1: Try tool_registry first (existing behavior)
        if let Some(tool) = self.tool_registry.get(&tool_name) {
            validate_required_arguments(&tool_name, &tool_input)
                .map_err(|e| invalid_params(e.to_string()))?;
            let result = tool.run(&ToolInput {
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
            })?;

            info!("MCP: Tool '{}' returned: {:?}", tool_name, result);
            record_tool_call_audit_with_protocol(
                &tool_name,
                &tool_input,
                true,
                "tool executed via mcp",
                "mcp_stdio",
            );
            let result_value: Value = serde_json::to_value(&result)?;
            return Ok(serde_json::to_value(
                McpCallToolResult::new(
                    vec![json!({
                        "type": "text",
                        "text": serde_json::to_string(&result)?,
                    })],
                    Some(result_value),
                )
                .with_is_error(false),
            )?);
        }

        // Step 2: Try skill registry fallback
        if let Some(registry) = self.skill_registry() {
            // Extract skill from lock, then drop the guard before async execution
            let skill_to_call = match registry.lock() {
                Ok(guard) => {
                    // Try exact name match first
                    if let Some(skill) = guard.get(&tool_name) {
                        // Clone while lock is held so skill is fully owned data
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
                info!(
                    "MCP: Calling skill '{}' with input: {:?}",
                    resolved_name, tool_input
                );
                let result = skill.execute(&tool_input).await?;

                record_tool_call_audit_with_protocol(
                    &resolved_name,
                    &tool_input,
                    true,
                    "skill executed via mcp",
                    "mcp_stdio",
                );

                info!("MCP: Skill '{}' returned: {:?}", resolved_name, result);

                let mut response = serde_json::to_value(
                    McpCallToolResult::new(
                        vec![
                            json!({"type": "text", "text": serde_json::to_string_pretty(&result)?}),
                        ],
                        Some(result),
                    )
                    .with_is_error(false),
                )?;
                if resolved_name != tool_name {
                    response["x_resolved_skill"] = json!(resolved_name);
                }
                return Ok(response);
            }
        }

        // Step 4: Not found — error
        warn!("MCP: Unknown tool or skill '{}'", tool_name);
        Err(invalid_params(format!(
            "Unknown tool or skill: {}",
            tool_name
        )))
    }

    async fn handle_list_resources(&self, _request: &JsonRpcRequest) -> Value {
        let resources: Vec<Value> = vec![
            serde_json::to_value(McpResource {
                uri: "go-on://agents".to_string(),
                name: "Available Agents".to_string(),
                description: Some("List of deployed agents".to_string()),
                mime_type: "application/json".to_string(),
            })
            .unwrap_or_else(|e| {
                warn!("MCP: failed to serialize resource 'go-on://agents': {e}");
                json!({})
            }),
            serde_json::to_value(McpResource {
                uri: "go-on://tools".to_string(),
                name: "Available Tools".to_string(),
                description: Some("List of available tools".to_string()),
                mime_type: "application/json".to_string(),
            })
            .unwrap_or_else(|e| {
                warn!("MCP: failed to serialize resource 'go-on://tools': {e}");
                json!({})
            }),
        ];

        serde_json::to_value(McpListResourcesResult::new(resources)).unwrap_or_else(|e| {
            warn!("MCP: failed to serialize list_resources result: {e}");
            json!({})
        })
    }

    async fn handle_read_resource(&self, request: &JsonRpcRequest) -> Result<Value> {
        let params = request
            .params
            .as_ref()
            .ok_or_else(|| invalid_params("Missing parameters"))?;

        let uri = params["uri"]
            .as_str()
            .ok_or_else(|| invalid_params("Missing URI"))?;

        match uri {
            "go-on://agents" => Ok(json!({
                "contents": [{
                    "uri": "go-on://agents",
                    "mimeType": "application/json",
                    "text": serde_json::to_string(&json!({"agents": self.agent_registry.names()}))?
                }]
            })),
            "go-on://tools" => Ok(json!({
                "contents": [{
                    "uri": "go-on://tools",
                    "mimeType": "application/json",
                    "text": serde_json::to_string(&json!({"tools": self.tool_registry.names()}))?
                }]
            })),
            _ => {
                warn!("MCP: unknown resource '{}'", uri);
                Err(invalid_params(format!("Unknown resource: {}", uri)))
            }
        }
    }

    async fn handle_list_agents(&self, _request: &JsonRpcRequest) -> Value {
        info!("MCP: Listing available agents from agent_registry");
        json!({ "agents": self.agent_registry.names() })
    }

    async fn handle_list_models(&self, _request: &JsonRpcRequest) -> Value {
        info!("MCP: Listing available models");
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
        let agent_name = create_request
            .model_preferences
            .as_ref()
            .and_then(|prefs| prefs.hints.as_ref())
            .and_then(|hints| hints.first())
            .and_then(|hint| hint.name.as_ref())
            .cloned()
            .unwrap_or_else(|| "primary".to_string());

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

        // Build options
        let mut options = HashMap::new();
        options.insert("max_tokens".to_string(), json!(create_request.max_tokens));
        if !create_request.stop_sequences.is_empty() {
            options.insert("stop".to_string(), json!(create_request.stop_sequences));
        }

        // Channel to collect streaming response
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(1024);
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
    /// This is a lightweight implementation that surfaces the agent system prompts
    /// as discoverable prompt templates.  Full template parameterisation is a
    /// future enhancement.
    async fn handle_list_prompts(&self, request: &JsonRpcRequest) -> Value {
        let lang = self.resolve_prompt_lang(request);
        let prompts = self.build_prompt_list(&lang);
        json!({ "prompts": prompts })
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

        if let Some(resolved) = self.resolve_template_prompt(name, &lang, request.params.as_ref()) {
            return Ok(resolved);
        }

        // Try to resolve as an agent system prompt
        if let Some(messages) = self.resolve_agent_prompt(name) {
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

    /// Build a list of discoverable prompt templates from agent configurations
    /// and registered skills.
    fn build_prompt_list(&self, lang: &str) -> Vec<Value> {
        let mut prompts = Vec::new();

        // Prompt templates from ACP prompt manager
        if let Some(acp) = &self.acp_server {
            if let Ok(collection) = acp.prompt_manager.get_all_templates(lang) {
                for category in collection.categories {
                    for template in category.templates {
                        prompts.push(json!({
                            "name": format!("template://{}.{}", category.id, template.id),
                            "description": template.description,
                            "arguments": [
                                {
                                    "name": "input",
                                    "description": "Optional input for replacing {{input}} placeholder",
                                    "required": false
                                }
                            ]
                        }));
                    }
                }
            }
        }

        // Agent system prompts
        for name in self.agent_registry.names() {
            prompts.push(json!({
                "name": format!("agent://{}", name),
                "description": format!("System prompt for '{}' agent", name),
                "arguments": []
            }));
        }

        prompts
    }

    fn resolve_template_prompt(
        &self,
        name: &str,
        lang: &str,
        params: Option<&Value>,
    ) -> Option<Value> {
        let acp = self.acp_server.as_ref()?;

        let normalized = name
            .strip_prefix("template://")
            .unwrap_or(name)
            .trim()
            .trim_start_matches('/');

        let (cat_id, cat_name, tpl) = acp.prompt_manager.get_template(lang, normalized)?;

        let input = params
            .and_then(|p| p.get("arguments"))
            .and_then(|a| a.get("input"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim();

        let content = if tpl.content.contains("{{input}}") {
            tpl.content.replace("{{input}}", input)
        } else if input.is_empty() {
            tpl.content
        } else {
            format!("{}\n\n{}", tpl.content, input)
        };

        Some(json!({
            "description": format!("Template prompt '{}.{}'", cat_id, tpl.id),
            "messages": [
                {
                    "role": "system",
                    "content": content
                }
            ],
            "template": {
                "category_id": cat_id,
                "category_name": cat_name,
                "id": tpl.id,
                "title": tpl.title,
            }
        }))
    }

    /// Resolve an agent prompt by name.
    /// Returns `None` if no matching agent is found.
    fn resolve_agent_prompt(&self, name: &str) -> Option<Vec<Value>> {
        let agent_name = name.strip_prefix("agent://").unwrap_or(name);
        // Check agent exists and resolve available models.
        let models = self.agent_registry.get(agent_name).map(|agent| {
            agent
                .available_models()
                .into_iter()
                .map(|m| m.id)
                .collect::<Vec<_>>()
                .join(", ")
        });

        let model_hint = models
            .filter(|m| !m.is_empty())
            .map(|m| format!(" Available models: {}.", m))
            .unwrap_or_default();

        Some(vec![
            json!({
                "role": "system",
                "content": format!(
                    "You are a '{}' agent providing AI assistance.{}",
                    agent_name, model_hint
                )
            }),
            json!({
                "role": "user",
                "content": "Hello!"
            }),
        ])
    }

    /// Notify all subscribers that a resource has changed.
    ///
    /// Logs the change event and updates the subscription tracking.
    /// Transport-level push of `notifications/resources/list_changed`
    /// is wired when the ACP server provides a notification channel.
    ///
    /// This is a public API surface reserved for external callers who
    /// need to notify subscribers of resource changes.
    #[allow(dead_code)]
    pub fn notify_resource_changed(&self, resource_uri: &str) {
        let has_subscribers = {
            let subs = self
                .resource_subscriptions
                .lock()
                .unwrap_or_else(|poisoned| {
                    warn!("resource_subscriptions lock poisoned, recovering");
                    poisoned.into_inner()
                });
            subs.get(resource_uri)
                .map(|s| !s.is_empty())
                .unwrap_or(false)
        };

        if !has_subscribers {
            return;
        }

        info!(
            "MCP: resource '{}' changed, notifying subscriber(s)",
            resource_uri,
        );

        // Push the change notification through the SSE broadcaster if one is
        // configured, so connected SSE clients receive the real-time update.
        if let Some(ref broadcaster) = self.sse_broadcaster {
            let notification = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "notifications/resources/updated",
                "params": {
                    "uri": resource_uri,
                },
            });
            let payload = serde_json::to_string(&notification).unwrap_or_default();
            let _ = broadcaster.send(payload);
        }
    }
}
