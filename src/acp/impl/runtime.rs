//! Runtime implementation functions for ACP server
//!
//! This module contains standalone functions that implement the core runtime
//! functionality previously in the `impl AcpServer` block.
//! These functions take `AcpServer` as their first parameter to maintain
//! compatibility with the original implementation.

use std::path::Path;
use std::sync::{Arc, Mutex as StdMutex};

use anyhow::Result;
use reqwest;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, Notify};
use tracing::{debug, error, info, warn};

use crate::acp::background::start_background_tasks;
use crate::acp::r#impl::io::send_error;
use crate::acp::r#impl::request::handle_request;

use crate::acp::server::AcpServer;
use crate::adaptive_selector::AdaptiveModelSelector;
use crate::advanced_modules::{DynamicParameterTuner, ResourceAllocator};
use crate::agent::AgentRegistry;
use crate::config::{AutoTuneConfig, AutoTuneState, RuntimeConfig, VectorConfig};
use crate::cost_optimizer::CostOptimizer;
use crate::failure_prevention::FailurePrevention;
use crate::flow::FlowManager;
use crate::flow_with_models::FlowModelSelector;
use crate::memory_module::{MemoryPolicy, MemoryStore};
use crate::memory_response_cache::MemoryResponseCache;
use crate::observability::telemetry::TelemetryRuntime;
use crate::orchestration::skill::SkillRegistry;
use crate::reinforcement::ArtifactLedger;
use crate::rpc_protocol::{chat_trace_context, JsonRpcRequest, RequestTraceContext};
use crate::vector::VectorStore;

/// Create a new ACP server instance
///
/// This function replaces the `AcpServer::new` constructor.
#[allow(clippy::too_many_arguments)]
pub fn new_acp_server(
    flow: Arc<FlowManager>,
    registry: Arc<AgentRegistry>,
    cache: Option<Arc<crate::cache::ResponseCache>>,
    vector_store: Option<Arc<VectorStore>>,
    vector_config: Option<VectorConfig>,
    autotune: Option<Arc<tokio::sync::Mutex<AutoTuneState>>>,
    autotune_config: Option<AutoTuneConfig>,
    autotune_state_path: Option<String>,
    config_path: Option<String>,
    runtime_config: RuntimeConfig,
    _http_client: Option<reqwest::Client>,
    _verbose: bool,
) -> AcpServer {
    // Use ServerBuilder to create the server with correct field names and types
    use crate::acp::server::ServerBuilder;

    let mut builder = ServerBuilder::new();

    // Set the components that ServerBuilder supports
    builder = builder.with_flow_manager(flow.clone());
    builder = builder.with_agent_registry(registry.clone());

    if let Some(ref cache) = cache {
        builder = builder.with_response_cache(cache.clone());
    }

    if let Some(ref vector_store) = vector_store {
        builder = builder.with_vector_store(vector_store.clone());
    }
    if let Some(ref path) = config_path {
        builder = builder.with_artifact_ledger(ArtifactLedger::new(Some(Path::new(path))));
    }
    builder = builder.with_config_path(config_path.clone());

    // Note: ServerBuilder doesn't have methods for all parameters yet
    // For now, we'll build with defaults and let the caller set additional fields
    match builder.build() {
        Ok(mut server) => {
            // Set fields that aren't available in ServerBuilder yet
            server.vector_config = vector_config;
            server.autotune = autotune;
            server.autotune_config = autotune_config;
            server.autotune_state_path = autotune_state_path;
            server.config_path = config_path;
            server.runtime_config = runtime_config;
            server.verbose = _verbose;

            server
        }
        Err(err) => {
            // Fallback to creating a minimal server if builder fails
            tracing::error!("Failed to build server with ServerBuilder: {}", err);

            // Create a minimal server with just the essential components
            use crate::acp::prelude::{
                CircuitBreakerRegistry, ConversationState, InflightLimiter, LifecycleState,
                MaintenanceTracker, OnlineControllerState, PhaseRateLimiter, ReviewTimeoutPolicy,
                RuntimeMetrics,
            };

            let mut failure_prevention_state = FailurePrevention::new();
            for name in registry.names() {
                failure_prevention_state.register_service(&name);
            }

            AcpServer {
                flow_manager: Some(flow.clone()),
                agent_registry: Some(registry.clone()),
                response_cache: cache.clone(),
                vector_store: vector_store.clone(),
                vector_config,
                autotune,
                autotune_config,
                autotune_state_path,
                config_path: config_path.clone(),
                runtime_config: runtime_config.clone(),
                metrics: Arc::new(RuntimeMetrics::new()),
                online_controller: Arc::new(StdMutex::new(OnlineControllerState::default())),
                circuit_breakers: Arc::new(StdMutex::new(CircuitBreakerRegistry::new())),
                maintenance_tracker: Arc::new(StdMutex::new(MaintenanceTracker::new())),
                inflight_limiter: Arc::new(StdMutex::new(InflightLimiter::default())),
                lifecycle_state: Arc::new(StdMutex::new(LifecycleState::new())),
                conversation_state: Arc::new(Mutex::new(ConversationState::default())),
                phase_rate_limiter: Arc::new(StdMutex::new(PhaseRateLimiter::default())),
                review_timeout_policy: Arc::new(StdMutex::new(ReviewTimeoutPolicy {
                    timeout_seconds: None,
                    fail_on_timeout: false,
                })),
                adaptive_model_selector: Arc::new(StdMutex::new(AdaptiveModelSelector::new())),
                dynamic_parameter_tuner: Arc::new(StdMutex::new(DynamicParameterTuner::default())),
                resource_allocator: Arc::new(StdMutex::new(ResourceAllocator {})),
                cost_optimizer: Arc::new(StdMutex::new(CostOptimizer::new())),
                failure_prevention: Arc::new(StdMutex::new(failure_prevention_state)),
                flow_model_selector: Arc::new(StdMutex::new(FlowModelSelector {})),
                memory_response_cache: Arc::new(StdMutex::new(MemoryResponseCache::default())),
                memory_store: Arc::new(StdMutex::new(MemoryStore::new(MemoryPolicy::default()))),
                skill_registry: Arc::new(StdMutex::new(SkillRegistry::default())),
                telemetry_runtime: Arc::new(StdMutex::new(TelemetryRuntime::new(&runtime_config))),
                pua_enforcement_plan: Arc::new(StdMutex::new(crate::pua::PuaEnforcementPlan {
                    escalation_level: String::new(),
                    mandatory_roles: Vec::new(),
                    red_lines: Vec::new(),
                    quality_compass: Vec::new(),
                    mandatory_safeguards: Vec::new(),
                    mandatory_evidence: Vec::new(),
                    stage_requirements: Vec::new(),
                })),
                artifact_ledger: Arc::new(StdMutex::new(ArtifactLedger::new(
                    config_path.as_deref().map(Path::new),
                ))),
                verbose: _verbose,
                output: Arc::new(Mutex::new(tokio::io::stdout())),
                shutdown_notify: Arc::new(Notify::new()),
            }
        }
    }
}

/// Run the ACP server
///
/// This function replaces the `AcpServer::run` method.
pub async fn run_acp_server(server: &mut AcpServer) -> Result<()> {
    info!("ACP server starting");

    let shutdown_notify = Arc::clone(&server.shutdown_notify);

    // Start background tasks
    if let Err(e) = start_background_tasks(server, Arc::clone(&shutdown_notify)).await {
        error!("Failed to start background tasks: {}", e);
        return Err(e);
    }

    info!("ACP server running");

    let stdin = tokio::io::stdin();
    let mut lines = BufReader::new(stdin).lines();

    loop {
        if server.shutdown_requested() {
            break;
        }

        let next_line = tokio::select! {
            _ = shutdown_notify.notified() => {
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
                send_error(server, None, -32700, format!("parse error: {err}"), None).await?;
                continue;
            }
        };

        if request.jsonrpc != "2.0" {
            send_error(
                server,
                request.id,
                -32600,
                "jsonrpc must be 2.0".to_string(),
                None,
            )
            .await?;
            continue;
        }

        if let Err(err) = handle_request(server, request).await {
            error!("request failed: {err:#}");
        }
    }

    // Notify background tasks to shutdown
    server.begin_shutdown();
    shutdown_notify.notify_waiters();

    info!("ACP server shutting down");
    Ok(())
}

pub async fn run_acp_http_server(server: Arc<AcpServer>, bind_addr: String) -> Result<()> {
    info!("ACP HTTP server starting on {}", bind_addr);

    let shutdown_notify = Arc::clone(&server.shutdown_notify);

    if let Err(err) = start_background_tasks(server.as_ref(), Arc::clone(&shutdown_notify)).await {
        error!("Failed to start background tasks: {}", err);
        return Err(err);
    }

    let listener = TcpListener::bind(&bind_addr).await?;
    loop {
        tokio::select! {
            _ = shutdown_notify.notified() => {
                break;
            }
            incoming = listener.accept() => {
                let (mut socket, peer_addr) = incoming?;
                let server_ref = Arc::clone(&server);
                tokio::spawn(async move {
                    if let Err(err) = handle_http_connection(&mut socket, server_ref).await {
                        warn!("ACP HTTP connection {} failed: {}", peer_addr, err);
                    }
                });
            }
        }
    }

    server.begin_shutdown();
    server.shutdown_notify.notify_waiters();
    info!("ACP HTTP server shutting down");
    Ok(())
}

/// Get routing handles (flow manager and agent registry)
pub fn routing_handles(server: &AcpServer) -> Result<(Arc<FlowManager>, Arc<AgentRegistry>)> {
    let flow = server
        .flow_manager
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("flow manager not initialized"))?;
    let registry = server
        .agent_registry
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("agent registry not initialized"))?;
    Ok((Arc::clone(flow), Arc::clone(registry)))
}

/// Get cache handle
pub fn cache_handle(server: &AcpServer) -> Option<Arc<crate::cache::ResponseCache>> {
    server.response_cache.clone()
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
pub fn vector_store_handle(server: &AcpServer) -> Option<Arc<VectorStore>> {
    server.vector_store.clone()
}

/// Get vector configuration snapshot
pub fn vector_config_snapshot(server: &AcpServer) -> Option<VectorConfig> {
    server.vector_config.clone()
}

/// Get autotune handle
pub fn autotune_handle(server: &AcpServer) -> Option<Arc<tokio::sync::Mutex<AutoTuneState>>> {
    server.autotune.clone()
}

async fn handle_http_connection(socket: &mut TcpStream, server: Arc<AcpServer>) -> Result<()> {
    let mut buffer = vec![0u8; 64 * 1024];
    let bytes_read = socket.read(&mut buffer).await?;
    if bytes_read == 0 {
        return Ok(());
    }

    let request_text = String::from_utf8_lossy(&buffer[..bytes_read]);
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

    if method == "GET" && path == "/health" {
        write_http_json_response(socket, 200, serde_json::to_value(server.get_status())?).await?;
        return Ok(());
    }

    if method != "POST" {
        write_http_json_response(
            socket,
            405,
            serde_json::json!({"error": "method not allowed"}),
        )
        .await?;
        return Ok(());
    }

    let content_length = extract_content_length(header_part).unwrap_or(0);
    let mut body_bytes = body_initial_part.as_bytes().to_vec();
    if body_bytes.len() < content_length {
        let mut remaining = vec![0u8; content_length - body_bytes.len()];
        socket.read_exact(&mut remaining).await?;
        body_bytes.extend_from_slice(&remaining);
    }
    body_bytes.truncate(content_length);
    let body: serde_json::Value = serde_json::from_slice(&body_bytes)?;

    match path {
        "/chat" => {
            let params: crate::acp::r#impl::chat::ChatParams = serde_json::from_value(body)?;
            let trace = http_trace_context("chat");
            let result = crate::acp::r#impl::chat::process_chat_request(
                server.as_ref(),
                &params,
                None,
                &trace,
                None,
            )
            .await?;
            write_http_json_response(socket, 200, result).await?;
        }
        "/chat/stream" => {
            let params: crate::acp::r#impl::chat::ChatParams = serde_json::from_value(body)?;
            write_sse_headers(socket).await?;

            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            let trace = http_trace_context("chat.stream");
            let server_ref = Arc::clone(&server);
            let task = tokio::spawn(async move {
                crate::acp::r#impl::chat::process_chat_request(
                    server_ref.as_ref(),
                    &params,
                    Some(crate::acp::r#impl::chat::StreamObserver::sse(tx)),
                    &trace,
                    None,
                )
                .await
            });

            while let Some(frame) = rx.recv().await {
                write_sse_event(socket, &frame.event, &frame.payload).await?;
            }

            match task.await {
                Ok(Ok(result)) => write_sse_event(socket, "result", &result).await?,
                Ok(Err(err)) => {
                    write_sse_event(
                        socket,
                        "error",
                        &serde_json::json!({"message": err.to_string()}),
                    )
                    .await?
                }
                Err(err) => {
                    write_sse_event(
                        socket,
                        "error",
                        &serde_json::json!({"message": format!("chat task panicked: {err}")}),
                    )
                    .await?
                }
            }
        }
        _ => {
            write_http_json_response(socket, 404, serde_json::json!({"error": "not found"}))
                .await?;
        }
    }

    Ok(())
}

fn extract_content_length(headers: &str) -> Option<usize> {
    headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.trim().eq_ignore_ascii_case("content-length") {
            value.trim().parse::<usize>().ok()
        } else {
            None
        }
    })
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
) -> Result<()> {
    let status_text = match status {
        200 => "OK",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "OK",
    };
    let body = serde_json::to_vec(&value)?;
    let headers = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        status,
        status_text,
        body.len()
    );
    socket.write_all(headers.as_bytes()).await?;
    socket.write_all(&body).await?;
    Ok(())
}

async fn write_sse_headers(socket: &mut TcpStream) -> Result<()> {
    socket
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\nX-Accel-Buffering: no\r\n\r\n",
        )
        .await?;
    Ok(())
}

async fn write_sse_event(
    socket: &mut TcpStream,
    event: &str,
    payload: &serde_json::Value,
) -> Result<()> {
    let frame = format!(
        "event: {}\ndata: {}\n\n",
        event,
        serde_json::to_string(payload)?
    );
    debug!("ACP SSE event: {}", event);
    socket.write_all(frame.as_bytes()).await?;
    Ok(())
}
