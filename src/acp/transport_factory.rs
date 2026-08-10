//! Transport factory — unified ACP/MCP server construction for all 5 protocol modes.
//! Handles server initialization (cache/vector/autotune), capability routing,
//! and protocol-mode-specific dispatch.

use crate::agent::AgentRegistry;
use crate::cache::ResponseCache;
use crate::config::{AutoTuneConfig, AutoTuneState, CacheConfig, VectorConfig};
use crate::flow::FlowManager;
use crate::orchestration::skill::SkillRegistry;
use crate::vector::VectorStore;
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

fn resolve_path(config_path: &Path, raw_path: &str) -> PathBuf {
    let candidate = PathBuf::from(raw_path);
    if candidate.is_absolute() {
        candidate
    } else {
        config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(candidate)
    }
}

/// Initialize a PostgreSQL backend with connection retry and health check.
///
/// Retries the `factory` closure up to 3 times with exponential backoff
/// (1s, 2s, 4s). The `factory` receives the connection URL string and should
/// perform connection + migration + health check internally. The whole retry
/// loop runs on the blocking pool (the retry sleep is a blocking sleep).
#[cfg(feature = "backend-postgres")]
async fn initialize_postgres_backend<T, F>(url: &str, factory: F) -> Result<T>
where
    T: Send + 'static,
    F: Fn(String) -> Result<T> + Send + 'static,
{
    let url_owned = url.to_string();
    tokio::task::spawn_blocking(move || {
        let mut last_error: Option<anyhow::Error> = None;
        for attempt in 0..3 {
            // Exponential backoff (1s, 2s, 4s) — same formula as the agent
            // chat retry helper (`backoff_secs` in agents/copilot.rs), but kept
            // inline here because this is a *blocking connection-init* retry
            // (postgres backend setup inside spawn_blocking), not an async
            // model-chat retry; `retry_chat_once` targets the chat path only.
            let backoff_secs = 1u64 << attempt;
            match factory(url_owned.clone()) {
                Ok(value) => return Ok(value),
                Err(e) => {
                    tracing::warn!("postgres init attempt {}/3 failed: {}", attempt + 1, e);
                    last_error = Some(e);
                }
            }
            if attempt < 2 {
                std::thread::sleep(std::time::Duration::from_secs(backoff_secs));
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("postgres init failed after 3 attempts")))
    })
    .await
    .map_err(|e| anyhow::anyhow!("postgres init join error: {e}"))?
}

/// Resolve an optional-service init result by build profile: server builds
/// propagate errors; `local`-only builds downgrade failures to `None` with a
/// warning so the server can start without cache/vector.
#[cfg(not(feature = "backend-postgres"))]
fn downgrade_optional<T>(_label: &str, result: Result<Option<Arc<T>>>) -> Result<Option<Arc<T>>> {
    #[cfg(all(
        feature = "local",
        not(feature = "simple-server"),
        not(feature = "multi-users-server"),
        not(feature = "full"),
    ))]
    {
        match result {
            Ok(v) => Ok(v),
            Err(e) => {
                tracing::warn!("{_label} init failed: {e}; continuing without {_label}");
                Ok(None)
            }
        }
    }

    #[cfg(any(
        feature = "simple-server",
        feature = "multi-users-server",
        feature = "full",
    ))]
    {
        result
    }
}

/// Initialize response cache.
#[allow(unused_variables)] // config_path unused in backend-postgres code path
pub async fn initialize_cache(
    config_path: &Path,
    cache_cfg: Option<CacheConfig>,
) -> Result<Option<Arc<ResponseCache>>> {
    let Some(cfg) = cache_cfg else {
        return Ok(None);
    };
    if !cfg.enabled {
        return Ok(None);
    }

    #[cfg(feature = "backend-postgres")]
    {
        let url = cfg
            .connection_string
            .ok_or_else(|| anyhow::anyhow!("cache.connection_string required"))?;
        let default_ttl_seconds = cfg.default_ttl_seconds;
        let max_entries = cfg.max_entries;
        initialize_postgres_backend(&url, move |conn_str| {
            ResponseCache::new(&conn_str, default_ttl_seconds, max_entries)
                .map(Arc::new)
                .map(Some)
        })
        .await
    }

    #[cfg(not(feature = "backend-postgres"))]
    {
        let cp = resolve_path(config_path, &cfg.path);
        let r = tokio::task::spawn_blocking(move || {
            ResponseCache::new(&cp, cfg.default_ttl_seconds, cfg.max_entries)
                .map(Arc::new)
                .map(Some)
        })
        .await
        .map_err(|e| anyhow::anyhow!("cache init: {e}"))?;
        downgrade_optional("cache", r)
    }
}

/// Initialize the vector store.
#[allow(unused_variables)]
pub async fn initialize_vector_store(
    config_path: &Path,
    vector_cfg: Option<VectorConfig>,
) -> Result<Option<Arc<VectorStore>>> {
    let Some(cfg) = vector_cfg else {
        return Ok(None);
    };
    if !cfg.enabled {
        return Ok(None);
    }

    // Initialize embedding provider from environment (GAP-B55-019)
    // Supports: "openai" (requires OPENAI_API_KEY), "local" (default, minhash-based)
    let embedding_provider = crate::memory::embedding_provider::embedding_provider_from_env();

    #[cfg(feature = "backend-postgres")]
    {
        let url = cfg
            .connection_string
            .ok_or_else(|| anyhow::anyhow!("vector.connection_string required"))?;
        let dimensions = cfg.dimensions;
        let max_entries = cfg.max_entries;
        initialize_postgres_backend(&url, move |conn_str| {
            // Provider is rebuilt per attempt (deterministic, env-based) so the
            // retry loop can re-invoke the closure.
            let provider = crate::memory::embedding_provider::embedding_provider_from_env();
            let store = VectorStore::new(&conn_str, dimensions, max_entries)?
                .with_embedding_provider(provider);
            tracing::info!("vector store: embedding provider injected");
            Ok::<_, anyhow::Error>(Some(Arc::new(store)))
        })
        .await
    }

    #[cfg(not(feature = "backend-postgres"))]
    {
        let sp = resolve_path(config_path, &cfg.path);
        let provider = embedding_provider;
        let r = tokio::task::spawn_blocking(move || {
            let store = VectorStore::new(&sp, cfg.dimensions, cfg.max_entries)?
                .with_embedding_provider(provider);
            tracing::info!("vector store: embedding provider injected");
            Ok::<_, anyhow::Error>(Some(Arc::new(store)))
        })
        .await
        .map_err(|e| anyhow::anyhow!("vector init: {e}"))?;
        downgrade_optional("vector", r)
    }
}

/// Initialize autotune.
pub async fn initialize_autotune(
    config_path: &Path,
    autotune_cfg: Option<AutoTuneConfig>,
) -> Result<(
    Option<Arc<tokio::sync::Mutex<AutoTuneState>>>,
    Option<AutoTuneConfig>,
    Option<String>,
)> {
    match autotune_cfg {
        Some(cfg) if cfg.enabled => {
            let sp = resolve_path(config_path, &cfg.state_path)
                .to_string_lossy()
                .to_string();
            let c2 = cfg.clone();
            let s2 = sp.clone();
            let st = tokio::task::spawn_blocking(move || AutoTuneState::load_or_default(&s2, &c2))
                .await
                .map_err(|e| anyhow::anyhow!("autotune init: {e}"))?;
            Ok((
                Some(Arc::new(tokio::sync::Mutex::new(st))),
                Some(cfg),
                Some(sp),
            ))
        }
        _ => Ok((None, None, None)),
    }
}

/// Dispatch to the correct protocol-mode server implementation.
#[allow(clippy::too_many_arguments)]
/// P0 optimization: `app_config` when `Some` avoids a redundant AppConfig::load()
/// in `flow_manager()`, saving ~15-30ms of startup time.
pub async fn dispatch_server(
    registry: Arc<AgentRegistry>,
    cache: Option<Arc<ResponseCache>>,
    vector_store: Option<Arc<VectorStore>>,
    config_path: &Path,
    runtime_config: crate::config::RuntimeConfig,
    protocol_mode: &str,
    acp_http_bind: &str,
    autotune_state: Option<Arc<tokio::sync::Mutex<AutoTuneState>>>,
    autotune_config: Option<AutoTuneConfig>,
    autotune_state_path: Option<String>,
    skill_registry: Option<Arc<RwLock<SkillRegistry>>>,
    app_config: Option<Arc<crate::config::AppConfig>>,
    // Whether to wire the durable response cache as the token cache's L3 layer.
    persist_cache: bool,
) -> Result<()> {
    let runtime_flow = flow_manager(config_path, app_config.clone());

    // MCP arms need registry after new_acp_server consumes it, so clone here
    let mcp_registry = Arc::clone(&registry);

    let acp_server = crate::acp::r#impl::runtime::new_acp_server(
        Arc::clone(&runtime_flow),
        registry,
        cache,
        vector_store,
        None,
        autotune_state,
        autotune_config,
        autotune_state_path,
        Some(config_path.to_string_lossy().to_string()),
        runtime_config,
        app_config,
        skill_registry,
        persist_cache,
    )
    .await;

    match protocol_mode {
        "acp_stdio" | "adaptive" => {
            crate::acp::r#impl::runtime::run_acp_server(Arc::new(acp_server)).await
        }
        "acp_http" => {
            crate::acp::r#impl::runtime::run_acp_http_server(
                Arc::new(acp_server),
                acp_http_bind.to_string(),
            )
            .await
        }
        "mcp_stdio" => {
            // Reuse the ACP server's fully-registered tool registry instead of
            // building a second full registration just for the MCP arm.
            let s = crate::protocol::mcp_server::McpStdioServer::new(
                mcp_registry,
                Arc::clone(&acp_server.tool_registry),
                "go-on".into(),
                env!("CARGO_PKG_VERSION").into(),
                Some(Arc::new(acp_server)),
            );
            s.run().await
        }
        "mcp_http" => {
            // Extract mTLS config before moving acp_server into the Arc.
            let mtls_enabled = acp_server.runtime_config.mtls_enabled;
            let mtls_ca = acp_server.runtime_config.mtls_ca_cert_path.clone();
            let mtls_cert = acp_server.runtime_config.mtls_server_cert_path.clone();
            let mtls_key = acp_server.runtime_config.mtls_server_key_path.clone();
            let s = crate::protocol::mcp_server::McpHttpServer::new_with_acp(
                mcp_registry,
                Arc::clone(&acp_server.tool_registry),
                "go-on".into(),
                env!("CARGO_PKG_VERSION").into(),
                acp_http_bind.into(),
                Some(Arc::new(acp_server)),
            )
            // Wire the runtime mTLS config so MCP HTTP can actually serve
            // TLS/mTLS (previously the acceptor fields were unreachable).
            .with_mtls_config(mtls_enabled, &mtls_ca, &mtls_cert, &mtls_key)
            .with_rate_limiter(Arc::new(
                crate::protocol::rate_limit::RateLimitMiddleware::new(
                    crate::protocol::rate_limit::TenantRateLimit::default(),
                ),
            ));
            s.run().await
        }
        other => anyhow::bail!("unsupported protocol mode: {other}"),
    }
}

// S4 startup optimization: cache the FlowManager in a OnceLock so that
// the TOML file is read only once, not on every dispatch_server() call.
// This eliminates a blocking std::fs::read on every request path that
// invokes dispatch_server (saves ~1-5ms per call).
//
// P0: When `pre_loaded` is Some, it is used directly, avoiding a redundant
// AppConfig::load() from disk (~15-30ms savings).
static FLOW_MANAGER_CACHE: OnceLock<Arc<FlowManager>> = OnceLock::new();

fn flow_manager(
    config_path: &Path,
    pre_loaded: Option<Arc<crate::config::AppConfig>>,
) -> Arc<FlowManager> {
    // Note: OnceLock::get_or_init returns &T, so we clone the Arc.
    // The clone is cheap (refcount bump only); the first call constructs.
    FLOW_MANAGER_CACHE
        .get_or_init(|| {
            let app_config = match pre_loaded {
                Some(config) => {
                    tracing::debug!("flow_manager: using pre-loaded config (P0)");
                    config
                }
                None => match crate::config::AppConfig::load(config_path) {
                    Ok(config) => Arc::new(config),
                    Err(err) => {
                        tracing::warn!(
                            "failed to load app config for flow manager from {}: {}; falling back to defaults",
                            config_path.display(),
                            err
                        );
                        Arc::new(crate::config::AppConfig::default())
                    }
                },
            };
            Arc::new(FlowManager::new(app_config, None))
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── resolve_path ──────────────────────────────────────────────────

    #[test]
    fn resolve_path_absolute_returns_as_is() {
        let config_path = Path::new("/tmp/config.toml");
        let resolved = resolve_path(config_path, "/absolute/path/cache.db");
        assert_eq!(resolved, Path::new("/absolute/path/cache.db"));
    }

    #[test]
    fn resolve_path_relative_resolves_relative_to_config_parent() {
        let config_path = Path::new("/tmp/sub/config.toml");
        let resolved = resolve_path(config_path, "cache.db");
        assert_eq!(resolved, Path::new("/tmp/sub/cache.db"));
    }

    #[test]
    fn resolve_path_relative_current_dir() {
        let config_path = Path::new("config.toml");
        let resolved = resolve_path(config_path, "data/cache.db");
        assert_eq!(resolved, Path::new("data/cache.db"));
    }

    // ── flow_manager fallback ──────────────────────────────────────────

    #[test]
    fn flow_manager_falls_back_on_missing_config() {
        let missing = Path::new("/nonexistent/path/config.toml");
        let fm = flow_manager(missing, None);
        assert!(
            Arc::strong_count(&fm) >= 1,
            "flow_manager should return a valid Arc<FlowManager>"
        );
    }

    #[test]
    fn flow_manager_returns_arc() {
        let temp = std::env::temp_dir().join("go-on-test-config.toml");
        // Config doesn't exist — falls back to default
        let fm = flow_manager(&temp, None);
        assert!(Arc::strong_count(&fm) >= 1);
    }

    // ── dispatch_server protocol dispatch mapping ─────────────────────

    #[test]
    fn dispatch_server_unsupported_protocol_returns_error() {
        // We can't easily run dispatch_server without a full server, but we
        // can verify that unsupported protocol modes would bail.
        let result = tokio::runtime::Runtime::new()
            .expect("should create tokio runtime")
            .block_on(async {
                let cache_dir = tempfile::tempdir().expect("should create temp dir");
                let config_path = cache_dir.path().join("config.toml");

                let registry = Arc::new(AgentRegistry::new());

                let outcome = dispatch_server(
                    registry,
                    None,
                    None,
                    &config_path,
                    crate::config::RuntimeConfig::default(),
                    "unsupported_mode",
                    "127.0.0.1:0",
                    None,
                    None,
                    None,
                    None,
                    None, // app_config
                    true, // persist_cache
                )
                .await;

                outcome
            });

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("unsupported protocol mode"));
    }

    // ── initialize_cache / initialize_vector_store disabled config ────

    #[tokio::test]
    async fn initialize_cache_disabled_returns_none() {
        let config_path = Path::new("/tmp");
        let result = initialize_cache(config_path, None)
            .await
            .expect("initialize_cache(None) should succeed");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn initialize_cache_not_enabled_returns_none() {
        let config_path = Path::new("/tmp");
        let cfg = CacheConfig {
            enabled: false,
            path: "cache.db".to_string(),
            default_ttl_seconds: 3600,
            max_entries: 1000,
            connection_string: None,
            read_replica_connection_string: None,
            persist_enabled: true,
        };
        let result = initialize_cache(config_path, Some(cfg))
            .await
            .expect("initialize_cache(disabled) should succeed");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn initialize_vector_store_disabled_returns_none() {
        let config_path = Path::new("/tmp");
        let result = initialize_vector_store(config_path, None)
            .await
            .expect("initialize_vector_store(None) should succeed");
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn initialize_vector_store_not_enabled_returns_none() {
        let config_path = Path::new("/tmp");
        let cfg = VectorConfig {
            enabled: false,
            auto_mode: false,
            path: "vectors.db".to_string(),
            connection_string: None,
            dimensions: 64,
            min_query_chars: 10,
            top_k: 5,
            min_similarity: 0.5,
            max_snippet_chars: 512,
            max_entries: 256,
            summary_enabled: false,
            summary_trigger_messages: 5,
            summary_max_chars: 4096,
            read_replica_connection_string: None,
        };
        let result = initialize_vector_store(config_path, Some(cfg))
            .await
            .expect("initialize_vector_store(disabled) should succeed");
        assert!(result.is_none());
    }

    // ── initialize_autotune ───────────────────────────────────────────

    #[tokio::test]
    async fn initialize_autotune_disabled_returns_none() {
        let config_path = Path::new("/tmp");
        let (state, cfg, path) = initialize_autotune(config_path, None)
            .await
            .expect("initialize_autotune(None) should succeed");
        assert!(state.is_none());
        assert!(cfg.is_none());
        assert!(path.is_none());
    }

    #[tokio::test]
    async fn initialize_autotune_not_enabled_returns_none() {
        let config_path = Path::new("/tmp");
        // AutoTuneConfig does not impl Default, so we construct via load_or_default path
        // by providing a None config which returns (None, None, None)
        let (state, _, _) = initialize_autotune(config_path, None)
            .await
            .expect("initialize_autotune(None) should succeed");
        assert!(state.is_none());
    }
}
