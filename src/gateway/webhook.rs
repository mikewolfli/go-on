//! Concrete JSON webhook adapter + gateway HTTP listener (M3.4, gated on
//! `backend-sqlite` for the delivery ledger).
//!
//! # Webhook contract
//!
//! The adapter serves `POST /webhook/<platform>` with a JSON body:
//!
//! ```json
//! { "chat_id": "chat-123", "text": "hello" }
//! ```
//!
//! (`<platform>` is the adapter's `platform_name()`; the CLI registers the
//! adapter under `webhook`.) The `Content-Type` header is accepted but not
//! interpreted — the contract is JSON.
//!
//! Responses:
//!
//! - `200` + `{"chat_id":"chat-123","text":"<agent reply>"}` — a fresh
//!   delivery; the reply was recorded in the delivery ledger.
//! - `200` + empty body — the inbound message was already delivered (platform
//!   redelivery deduplicated; no second agent turn).
//! - `400` — malformed payload (not JSON, or missing `chat_id`/`text`).
//! - `404` — unknown platform or unknown route; `405` — non-POST.
//! - `409` — a turn for this chat is already in progress.
//! - `500` — agent turn or delivery-ledger failure.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::info;

use crate::config::AppConfig;
use crate::gateway::adapter::{InboundMessage, PlatformAdapter};
use crate::gateway::ledger::DeliveryLedger;
use crate::gateway::registry::PlatformRegistry;
use crate::gateway::session::TurnLease;

// ─────────────────────────────────────────────────────────────────────────────
// WebhookPlatform adapter
// ─────────────────────────────────────────────────────────────────────────────

/// JSON webhook adapter for the gateway (M3.4).
///
/// Inbound contract: `{"chat_id": "...", "text": "..."}`. Reply rendering
/// produces the mirror shape `{"chat_id": "...", "text": "<reply>"}`, addressed
/// to the chat the inbound message came from.
#[derive(Debug, Clone, Copy)]
pub struct WebhookPlatform {
    name: &'static str,
}

impl WebhookPlatform {
    /// A webhook adapter registered under `name` (used as the route segment
    /// `POST /webhook/<name>` and the registry key).
    pub fn new(name: &'static str) -> Self {
        Self { name }
    }
}

impl PlatformAdapter for WebhookPlatform {
    fn platform_name(&self) -> &'static str {
        self.name
    }

    fn parse_inbound(&self, raw: &[u8], _content_type: &str) -> Result<Vec<InboundMessage>> {
        let value: Value = serde_json::from_slice(raw).context("webhook body is not valid JSON")?;
        let chat_id = value
            .get("chat_id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing string field \"chat_id\""))?;
        let text = value
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("missing string field \"text\""))?;
        Ok(vec![InboundMessage {
            platform_chat_id: chat_id.to_string(),
            text: text.to_string(),
            raw: value,
        }])
    }

    fn render_reply(&self, reply: &str, original: &InboundMessage) -> Result<Vec<u8>> {
        let payload = json!({
            "chat_id": original.platform_chat_id,
            "text": reply,
        });
        Ok(serde_json::to_vec(&payload)?)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Webhook processing errors (mapped to HTTP status codes by the listener)
// ─────────────────────────────────────────────────────────────────────────────

/// Typed failure for webhook processing; the listener maps each variant to an
/// HTTP status code.
#[derive(Debug, thiserror::Error)]
pub enum WebhookError {
    /// No adapter registered for the requested platform.
    #[error("no platform adapter registered for '{0}'")]
    UnknownPlatform(String),
    /// The inbound payload could not be parsed.
    #[error("invalid inbound payload: {0}")]
    Parse(String),
    /// A turn is already in progress for the chat.
    #[error("a turn is already in progress for chat '{0}'")]
    Busy(String),
    /// The agent turn failed (chat error, timeout, or no agents configured).
    #[error("agent turn failed: {0:#}")]
    AgentTurn(#[source] anyhow::Error),
    /// The reply could not be rendered into the platform format.
    #[error("reply rendering failed: {0:#}")]
    Render(#[source] anyhow::Error),
    /// The delivery ledger (dedup / record) failed.
    #[error("delivery ledger failed: {0:#}")]
    Ledger(#[source] anyhow::Error),
}

// ─────────────────────────────────────────────────────────────────────────────
// Webhook request handling
// ─────────────────────────────────────────────────────────────────────────────

/// Process one webhook delivery end-to-end:
///
/// 1. resolve the platform adapter;
/// 2. parse the inbound payload;
/// 3. dedup against the delivery ledger (identical redeliveries are skipped);
/// 4. claim the per-chat turn lease (concurrent turns for the same chat are
///    rejected with [`WebhookError::Busy`]);
/// 5. run the agent turn;
/// 6. render and record the reply.
///
/// `leases` is passed in (rather than created per call) so the claim state is
/// shared across concurrent deliveries for the same chat.
///
/// Returns `Ok(Some(bytes))` for a fresh rendered reply (already recorded in
/// the ledger), or `Ok(None)` when the inbound was deduplicated and nothing
/// was delivered.
pub async fn handle_webhook_request<F, Fut>(
    registry: &PlatformRegistry,
    ledger: &DeliveryLedger,
    leases: &TurnLease,
    platform_name: &str,
    body: Vec<u8>,
    content_type: &str,
    run_turn: F,
) -> Result<Option<Vec<u8>>, WebhookError>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Result<String>>,
{
    let adapter = registry
        .adapter(platform_name)
        .ok_or_else(|| WebhookError::UnknownPlatform(platform_name.to_string()))?;

    let message = adapter
        .parse_inbound(&body, content_type)
        .map_err(|e| WebhookError::Parse(format!("{e:#}")))?
        .into_iter()
        .next()
        .ok_or_else(|| WebhookError::Parse("empty inbound payload".to_string()))?;

    // The dedup triple: identical redelivered payloads share the hash.
    let message_hash = crate::shared::sha256_hex(&body);
    tracing::debug!(
        target: "go_on::gateway",
        platform = platform_name,
        chat_id = %message.platform_chat_id,
        raw = ?message.raw,
        "processing inbound webhook message"
    );
    if ledger
        .already_delivered(platform_name, &message.platform_chat_id, &message_hash)
        .map_err(WebhookError::Ledger)?
    {
        tracing::debug!(
            target: "go_on::gateway",
            platform = platform_name,
            chat_id = %message.platform_chat_id,
            "duplicate inbound — already delivered, skipping"
        );
        return Ok(None);
    }

    // Per-chat turn lease: never run two concurrent turns for the same chat.
    let _lease = leases
        .try_claim(platform_name, &message.platform_chat_id)
        .ok_or_else(|| {
            tracing::debug!(
                target: "go_on::gateway",
                platform = platform_name,
                chat_id = %message.platform_chat_id,
                lease_active = leases.is_active(platform_name, &message.platform_chat_id),
                "turn already in progress for chat — rejecting delivery"
            );
            WebhookError::Busy(message.platform_chat_id.clone())
        })?;

    let reply = run_turn(message.text.clone())
        .await
        .map_err(WebhookError::AgentTurn)?;
    let reply_bytes = adapter
        .render_reply(&reply, &message)
        .map_err(WebhookError::Render)?;
    ledger
        .record_delivery(platform_name, &message.platform_chat_id, &message_hash)
        .map_err(WebhookError::Ledger)?;

    Ok(Some(reply_bytes))
}

// ─────────────────────────────────────────────────────────────────────────────
// HTTP listener (self-contained: tokio TcpListener + the ACP HTTP primitives)
// ─────────────────────────────────────────────────────────────────────────────

/// Minimal HTTP listener for the gateway webhook.
///
/// Self-contained on purpose: it reuses the crate's battle-tested HTTP
/// primitives (`acp::impl::runtime::{http::read_http_header,
/// protocol::parse_http_request}`) instead of pulling in a web framework, and
/// it only serves `POST /webhook/<platform>`.
#[derive(Clone)]
pub struct GatewayServer {
    registry: Arc<PlatformRegistry>,
    ledger: Arc<DeliveryLedger>,
    leases: Arc<TurnLease>,
    config: Arc<AppConfig>,
    config_path: PathBuf,
}

impl GatewayServer {
    /// A gateway server wired to the given registry, ledger, lease state, and
    /// agent config. Every turn runs against `config`/`config_path` via
    /// [`crate::gateway::run_agent_turn`].
    pub fn new(
        registry: Arc<PlatformRegistry>,
        ledger: Arc<DeliveryLedger>,
        leases: Arc<TurnLease>,
        config: Arc<AppConfig>,
        config_path: PathBuf,
    ) -> Self {
        Self {
            registry,
            ledger,
            leases,
            config,
            config_path,
        }
    }

    /// Bind and serve `POST /webhook/<platform>` until the process is stopped.
    pub async fn serve(self, bind: &str) -> Result<()> {
        let listener = tokio::net::TcpListener::bind(bind)
            .await
            .with_context(|| format!("bind gateway webhook listener on {bind}"))?;
        info!(
            target: "go_on::gateway",
            bind,
            platforms = ?self.registry.platform_names(),
            "gateway webhook listener ready — POST /webhook/<platform>"
        );
        loop {
            let (socket, peer) = listener.accept().await?;
            let server = self.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_connection(socket, server, peer).await {
                    tracing::debug!(
                        target: "go_on::gateway",
                        %peer,
                        error = %e,
                        "webhook connection error"
                    );
                }
            });
        }
    }

    async fn handle(
        &self,
        platform: &str,
        body: Vec<u8>,
        content_type: &str,
    ) -> Result<Option<Vec<u8>>, WebhookError> {
        handle_webhook_request(
            &self.registry,
            &self.ledger,
            &self.leases,
            platform,
            body,
            content_type,
            |prompt: String| {
                let config = Arc::clone(&self.config);
                let config_path = self.config_path.clone();
                async move { crate::gateway::run_agent_turn(config, &config_path, &prompt).await }
            },
        )
        .await
    }
}

/// Read, route, and answer one HTTP connection on the gateway listener.
async fn handle_connection(
    mut socket: tokio::net::TcpStream,
    server: GatewayServer,
    peer: std::net::SocketAddr,
) -> Result<()> {
    use crate::acp::r#impl::runtime::http::read_http_header;
    use crate::acp::r#impl::runtime::protocol::{
        extract_content_length, extract_header_value, parse_http_request,
    };

    let raw = read_http_header(&mut socket).await?;
    if raw.is_empty() {
        // Clean EOF before any request — close quietly.
        return Ok(());
    }
    let request_text = String::from_utf8_lossy(&raw);
    let parsed = parse_http_request(&request_text)?;

    // Only `POST /webhook/<platform>` is served; anything else is a 404/405.
    let Some(platform) = parsed.path.strip_prefix("/webhook/") else {
        return write_error(&mut socket, 404, "not found").await;
    };
    if platform.is_empty() || platform.contains('/') {
        return write_error(&mut socket, 404, "not found").await;
    }
    if parsed.method != "POST" {
        return write_error(&mut socket, 405, "method not allowed").await;
    }

    let content_type = extract_header_value(parsed.header_part, "content-type").unwrap_or_default();
    let content_length = extract_content_length(parsed.header_part).unwrap_or(0);

    // Assemble the full body: the header read may have carried a body prefix.
    let mut body: Vec<u8> = parsed.body_initial_part.as_bytes().to_vec();
    while body.len() < content_length {
        let mut buf = vec![0u8; content_length - body.len()];
        let n = socket.read(&mut buf).await?;
        if n == 0 {
            break; // truncated body — the parse below surfaces the error
        }
        body.extend_from_slice(&buf[..n]);
    }

    match server.handle(platform, body, &content_type).await {
        Ok(Some(reply)) => write_response(&mut socket, 200, &reply).await,
        // Deduplicated redelivery: nothing to deliver, still a success.
        Ok(None) => write_response(&mut socket, 200, &[]).await,
        Err(WebhookError::UnknownPlatform(_)) => {
            write_error(&mut socket, 404, "unknown platform").await
        }
        Err(WebhookError::Parse(e)) => {
            write_error(&mut socket, 400, &format!("bad request: {e}")).await
        }
        Err(WebhookError::Busy(_)) => {
            write_error(
                &mut socket,
                409,
                "a turn is already in progress for this chat",
            )
            .await
        }
        Err(e) => {
            tracing::warn!(
                target: "go_on::gateway",
                %peer,
                error = %e,
                "webhook processing failed"
            );
            write_error(&mut socket, 500, "internal error").await
        }
    }
}

fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        _ => "Internal Server Error",
    }
}

async fn write_response(
    socket: &mut tokio::net::TcpStream,
    status: u16,
    body: &[u8],
) -> Result<()> {
    let headers = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        status,
        status_text(status),
        body.len()
    );
    socket.write_all(headers.as_bytes()).await?;
    socket.write_all(body).await?;
    socket.flush().await?;
    Ok(())
}

async fn write_error(socket: &mut tokio::net::TcpStream, status: u16, message: &str) -> Result<()> {
    let body = serde_json::to_vec(&json!({ "error": message }))?;
    write_response(socket, status, &body).await
}

// ─────────────────────────────────────────────────────────────────────────────
// CLI entry point
// ─────────────────────────────────────────────────────────────────────────────

/// Entry point for `go-on gateway` (M3.4): loads the agent config exactly like
/// `go-on exec`, registers the JSON [`WebhookPlatform`] adapter, opens the
/// delivery ledger under the project data root, and serves the webhook.
pub async fn run_gateway_server(config_path: &Path, bind: &str) -> Result<()> {
    crate::config::defaults::ensure_bootstrap_config(config_path)?;
    let config = Arc::new(AppConfig::load(config_path)?);

    let registry = Arc::new(PlatformRegistry::new());
    // Held for the server's lifetime: unregisters the adapter on shutdown.
    let _adapter_guard = registry.register(Arc::new(WebhookPlatform::new("webhook")));

    let goon_root = crate::shared::goon_paths::resolve_goon_root(Some(config_path));
    let ledger_path = goon_root.join("gateway").join("deliveries.sqlite3");
    if let Some(parent) = ledger_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let ledger = Arc::new(DeliveryLedger::open(&ledger_path)?);
    let leases = Arc::new(TurnLease::new());

    info!(
        target: "go_on::gateway",
        bind,
        path = %ledger_path.display(),
        "gateway started"
    );

    let server = GatewayServer::new(registry, ledger, leases, config, config_path.to_path_buf());
    server.serve(bind).await
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    use crate::config::{
        AgentConfig, FeatureConfig, FlowConfig, ProviderConfig, RuntimeConfig, SecurityConfig,
    };

    fn local_echo_config() -> AppConfig {
        let mut agents = HashMap::new();
        agents.insert(
            "primary".to_string(),
            AgentConfig {
                agent_type: "local_echo".to_string(),
                url: None,
                chat_path: None,
                api_key_env: None,
                secret_key_env: None,
                anthropic_version: None,
                model: None,
                max_tokens: None,
                supports_system: None,
                supports_vision: None,
            },
        );
        AppConfig {
            schema_version: "1.0.0".to_string(),
            layered_merge: false,
            provider: ProviderConfig {
                default_phase: "coding".to_string(),
                agents,
                role_registry: HashMap::new(),
            },
            flow: FlowConfig::default(),
            phases: HashMap::new(),
            runtime: Some(RuntimeConfig::default()),
            cache: None,
            vector: None,
            autotune: None,
            security: SecurityConfig::default(),
            feature: FeatureConfig::default(),
            compliance: None,
            startup_context: None,
            protocol: None,
        }
    }

    fn echo_turn() -> impl Fn(String) -> std::future::Ready<Result<String>> {
        |prompt: String| std::future::ready(Ok(format!("echo: {prompt}")))
    }

    #[test]
    fn webhook_adapter_parse_and_render_round_trip() {
        let adapter = WebhookPlatform::new("webhook");
        let raw = br#"{"chat_id":"chat-1","text":"hello world"}"#;
        let messages = adapter
            .parse_inbound(raw, "application/json")
            .expect("valid payload parses");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].platform_chat_id, "chat-1");
        assert_eq!(messages[0].text, "hello world");

        let rendered = adapter
            .render_reply("echo: hello world", &messages[0])
            .expect("reply renders");
        let value: Value = serde_json::from_slice(&rendered).expect("rendered bytes are JSON");
        assert_eq!(value["chat_id"], "chat-1");
        assert_eq!(value["text"], "echo: hello world");
        assert_eq!(adapter.platform_name(), "webhook");
    }

    #[test]
    fn webhook_adapter_rejects_malformed_payloads() {
        let adapter = WebhookPlatform::new("webhook");
        assert!(adapter.parse_inbound(b"not json", "text/plain").is_err());
        assert!(adapter
            .parse_inbound(br#"{"chat_id":"c"}"#, "application/json")
            .is_err());
        assert!(adapter
            .parse_inbound(br#"{"text":"hi"}"#, "application/json")
            .is_err());
        assert!(adapter.parse_inbound(b"", "application/json").is_err());
    }

    #[tokio::test]
    async fn end_to_end_webhook_delivery_with_local_echo_agent() {
        // Local test agents are gated behind this env var (build_agent).
        std::env::set_var("GO_ON_ENABLE_LOCAL_TEST_AGENTS", "true");
        let config = Arc::new(local_echo_config());
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_path = tmp.path().join("config.toml");
        // The config file need not exist: the skill bootstrap is best-effort
        // and degrades gracefully (mirrors the exec runtime build).

        let registry = Arc::new(PlatformRegistry::new());
        let _guard = registry.register(Arc::new(WebhookPlatform::new("webhook")));
        let ledger =
            DeliveryLedger::open(std::path::Path::new(":memory:")).expect("in-memory ledger");
        let leases = TurnLease::new();

        let body = br#"{"chat_id":"chat-1","text":"hello from webhook"}"#.to_vec();
        let run_turn = |prompt: String| {
            let config = Arc::clone(&config);
            let config_path = config_path.clone();
            async move { crate::gateway::run_agent_turn(config, &config_path, &prompt).await }
        };

        let reply = handle_webhook_request(
            &registry,
            &ledger,
            &leases,
            "webhook",
            body.clone(),
            "application/json",
            run_turn,
        )
        .await
        .expect("fresh webhook delivery must succeed")
        .expect("a fresh delivery must render a reply (not be deduplicated)");

        let value: Value = serde_json::from_slice(&reply).expect("reply is JSON");
        assert_eq!(value["chat_id"], "chat-1");
        assert_eq!(
            value["text"], "hello from webhook",
            "local_echo echoes the prompt"
        );

        // The delivery ledger recorded this message hash.
        let hash = crate::shared::sha256_hex(&body);
        assert!(ledger
            .already_delivered("webhook", "chat-1", &hash)
            .expect("ledger query succeeds"));

        // A second identical inbound is deduplicated — no second agent turn.
        let replay = handle_webhook_request(
            &registry,
            &ledger,
            &leases,
            "webhook",
            body,
            "application/json",
            run_turn,
        )
        .await;
        assert!(
            matches!(replay, Ok(None)),
            "identical replay must be deduplicated"
        );

        // A distinct message for the same chat is still processed.
        let body2 = br#"{"chat_id":"chat-1","text":"second message"}"#.to_vec();
        let reply2 = handle_webhook_request(
            &registry,
            &ledger,
            &leases,
            "webhook",
            body2.clone(),
            "application/json",
            run_turn,
        )
        .await
        .expect("distinct message must succeed")
        .expect("distinct message must not be deduplicated");
        let value2: Value = serde_json::from_slice(&reply2).expect("reply is JSON");
        assert_eq!(value2["text"], "second message");
    }

    #[tokio::test]
    async fn concurrent_turn_for_same_chat_is_rejected_with_busy() {
        let registry = Arc::new(PlatformRegistry::new());
        let _guard = registry.register(Arc::new(WebhookPlatform::new("webhook")));
        let ledger =
            DeliveryLedger::open(std::path::Path::new(":memory:")).expect("in-memory ledger");
        let leases = TurnLease::new();

        // Hold the lease for chat-1, then the handler must report Busy without
        // running the turn.
        let _held = leases.try_claim("webhook", "chat-1").expect("claim chat-1");
        let body = br#"{"chat_id":"chat-1","text":"hello"}"#.to_vec();
        let err = handle_webhook_request(
            &registry,
            &ledger,
            &leases,
            "webhook",
            body,
            "application/json",
            echo_turn(),
        )
        .await
        .expect_err("lease held by another turn must reject");
        assert!(matches!(err, WebhookError::Busy(_)));
        assert!(err.to_string().contains("chat-1"));
    }

    #[tokio::test]
    async fn unknown_platform_is_rejected() {
        let registry = PlatformRegistry::new();
        let ledger =
            DeliveryLedger::open(std::path::Path::new(":memory:")).expect("in-memory ledger");
        let leases = TurnLease::new();
        let err = handle_webhook_request(
            &registry,
            &ledger,
            &leases,
            "nope",
            b"{}".to_vec(),
            "application/json",
            echo_turn(),
        )
        .await
        .expect_err("unregistered platform must be rejected");
        assert!(matches!(err, WebhookError::UnknownPlatform(_)));
    }
}
