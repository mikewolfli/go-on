use super::*;

/// Handle `initialize` — agent capability negotiation.
///
/// Negotiates the protocol version against the client's requested version
/// (params.protocolVersion if present): the server picks the highest version
/// it supports that does not exceed the client's request. The negotiated
/// version is stored process-wide so later handlers (e.g. SSE capability
/// reporting) can honour it.
pub async fn initialize_payload(_server: &AcpServer, params: &Option<Value>) -> Result<Value> {
    use crate::schema::{
        AgentCapabilities, AuthMethod, AuthMethodAgent, Implementation, InitializeResponse,
        McpCapabilities, PromptCapabilities, ProtocolVersion, SessionCapabilities,
        SessionCloseCapabilities, SessionListCapabilities, SessionResumeCapabilities,
    };

    // ── Real version negotiation (was decorative: always LATEST) ───────
    let requested = params
        .as_ref()
        .and_then(|p| p.get("protocolVersion"))
        .and_then(Value::as_u64)
        .map(|v| ProtocolVersion::from_u16(v as u16));
    let negotiated_version = super::negotiate_protocol_version(requested);

    let auth_methods = vec![AuthMethod::Agent(AuthMethodAgent {
        id: "bearer_token".to_string(),
        name: "Bearer Token".to_string(),
        description: Some(
            "Authenticate using a bearer token from the Authorization header".to_string(),
        ),
        meta: None,
    })];
    let mut resp = InitializeResponse::new(negotiated_version)
        .agent_info(Implementation::new("go-on", env!("CARGO_PKG_VERSION")))
        .agent_capabilities(AgentCapabilities {
            load_session: true,
            prompt_capabilities: PromptCapabilities {
                image: false,
                audio: false,
                embedded_context: false,
                ..Default::default()
            },
            mcp_capabilities: McpCapabilities {
                http: true,
                sse: false,
                ..Default::default()
            },
            session_capabilities: SessionCapabilities {
                list: Some(SessionListCapabilities { meta: None }),
                close: Some(SessionCloseCapabilities { meta: None }),
                resume: Some(SessionResumeCapabilities { meta: None }),
                ..Default::default()
            },
            ..Default::default()
        });
    resp.auth_methods = auth_methods;

    let mut value = serde_json::to_value(&resp)?;

    // ── Backward-compat legacy fields ─────────────────────────────────
    let negotiated_ver_num = negotiated_version.as_u16();
    if let Some(obj) = value.as_object_mut() {
        obj.insert("name".to_string(), serde_json::json!("go-on"));
        obj.insert("protocol".to_string(), serde_json::json!("acp"));
        obj.insert(
            "version".to_string(),
            serde_json::json!(env!("CARGO_PKG_VERSION")),
        );
        obj.insert(
            "protocol_version".to_string(),
            serde_json::json!(negotiated_ver_num),
        );
        let sse_enabled = negotiated_ver_num >= 3;
        let caps_obj = serde_json::json!({
            "chat": true,
            "phase": true,
            "metrics": true,
            "shutdown": true,
            "health": true,
            "debug_panel": true,
            "mcp_adapter": true,
            "sse_transport": sse_enabled,
            "tools_list": true,
            "tools_call": true,
            "tools": true,
            "acp_stdio": true,
            "protocol_version": env!("CARGO_PKG_VERSION"),
        });
        obj.insert("capabilities".to_string(), caps_obj);
    }

    let method = super::super::DISPATCH_REQUEST_METHOD
        .try_with(|m| m.clone())
        .unwrap_or_else(|_| "initialize".to_string());
    let value = super::super::inject_platform_profiles_if_absent(value, &method);

    Ok(value)
}

/// Handle `mcp.initialize` — MCP protocol initialization.
///
/// Advertises the same capabilities as the standalone MCP transport via the
/// shared `crate::mcp::schema::mcp_initialize_capabilities` single source of
/// truth.
pub async fn mcp_initialize_payload(_server: &AcpServer) -> Result<Value> {
    use crate::mcp::{McpInitializeResult, ServerInfo};
    let result = McpInitializeResult::new(
        MCP_VERSION,
        crate::mcp::mcp_initialize_capabilities(),
        ServerInfo {
            name: "go-on".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
    );
    Ok(serde_json::to_value(&result)?)
}

/// Handle `chat` — ACP chat endpoint with SSE streaming.
pub async fn handle_chat(
    server: &AcpServer,
    request_id: Option<Value>,
    params: Value,
    trace: &RequestTraceContext,
) -> Result<DispatchOutput> {
    use crate::acp::r#impl::chat::handle_chat as chat_handler;
    use crate::acp::r#impl::chat::streaming::{StreamFrame, StreamObserver};
    use tokio::sync::mpsc;

    let (tx, rx) = mpsc::unbounded_channel::<StreamFrame>();
    let observer = StreamObserver::sse(tx);

    match chat_handler(
        server,
        request_id,
        Some(params),
        None,
        Some(trace.clone()),
        Some(observer),
    )
    .await
    {
        Ok(()) => Ok(DispatchOutput::Stream { receiver: rx }),
        Err(err) => {
            let message = err.to_string();
            if super::is_rate_limited_message(&message) {
                Ok(DispatchOutput::error(
                    crate::acp::r#impl::request::protocol::AcpErrorCode::RateLimited as i32,
                    super::normalize_rate_limited_message(&message),
                ))
            } else {
                Ok(DispatchOutput::error(
                    crate::acp::r#impl::request::protocol::AcpErrorCode::InternalError as i32,
                    message,
                ))
            }
        }
    }
}

/// Handle `phase` / `phase.status` — get phase rate limiter and inflight state.
pub async fn phase_payload(
    server: &AcpServer,
    _params: Value,
    _trace: &RequestTraceContext,
) -> Result<Value> {
    let rate_limiter = server
        .resilience
        .phase_rate_limiter
        .lock()
        .map(|guard| {
            let mut m = serde_json::Map::new();
            m.insert(
                "tracked".to_string(),
                Value::Number(guard.tracked_phases().into()),
            );
            m.insert(
                "buckets".to_string(),
                serde_json::to_value(guard.snapshot()).unwrap_or_default(),
            );
            Value::Object(m)
        })
        .unwrap_or_else(|_| {
            let mut m = serde_json::Map::new();
            m.insert("tracked".to_string(), Value::Number(0.into()));
            m.insert("buckets".to_string(), Value::Object(serde_json::Map::new()));
            Value::Object(m)
        });

    let response = PhaseResponse { rate_limiter };
    Ok(serde_json::to_value(&response)?)
}

/// Handle `models.list` / `models/list` — list available models.
pub async fn models_list_payload(server: &AcpServer, _params: Value) -> Result<Value> {
    let models = server
        .model_deps
        .agent_registry
        .as_ref()
        .map(|registry| {
            registry
                .models()
                .into_iter()
                .flat_map(|(provider_name, _default_model, models)| {
                    models.into_iter().map(move |m| {
                        let mut model = serde_json::Map::new();
                        model.insert("id".to_string(), Value::String(m.id));
                        model.insert("name".to_string(), Value::String(m.name));
                        model.insert("description".to_string(), Value::String(m.description));
                        model.insert("provider".to_string(), Value::String(provider_name.clone()));
                        model.insert("is_default".to_string(), Value::Bool(m.is_default));
                        model.insert(
                            "capabilities".to_string(),
                            Value::Array(m.capabilities.into_iter().map(Value::String).collect()),
                        );
                        if let Some(cw) = m.context_window {
                            model.insert(
                                "context_window".to_string(),
                                Value::Number((cw as u64).into()),
                            );
                        }
                        Value::Object(model)
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let response = ModelsListResponse { models };
    Ok(serde_json::to_value(&response)?)
}
