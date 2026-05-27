//! Transport factory — unified ACP/MCP server construction for all 5 protocol modes.
//! Handles server initialization (cache/vector/autotune), capability routing,
//! and protocol-mode-specific dispatch.

use crate::agent::AgentRegistry;
use crate::cache::ResponseCache;
use crate::config::{AutoTuneConfig, AutoTuneState, CacheConfig, VectorConfig};
use crate::flow::FlowManager;
use crate::vector::VectorStore;
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
        tokio::task::spawn_blocking(move || {
            ResponseCache::new(&url, cfg.default_ttl_seconds, cfg.max_entries)
                .map(Arc::new)
                .map(Some)
        })
        .await
        .map_err(|e| anyhow::anyhow!("cache init: {e}"))?
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
        #[cfg(all(
            feature = "profile-local",
            not(feature = "profile-simple-server"),
            not(feature = "profile-multi-users-server")
        ))]
        {
            match r {
                Ok(c) => Ok(c),
                Err(e) => {
                    tracing::warn!("cache init failed: {e}; continuing without cache");
                    Ok(None)
                }
            }
        }
        #[cfg(any(
            feature = "profile-simple-server",
            feature = "profile-multi-users-server"
        ))]
        {
            r
        }
    }
}

/// Initialize vector store.
#[allow(unused_variables)] // config_path unused in backend-postgres code path
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

    #[cfg(feature = "backend-postgres")]
    {
        let url = cfg
            .connection_string
            .ok_or_else(|| anyhow::anyhow!("vector.connection_string required"))?;
        tokio::task::spawn_blocking(move || {
            VectorStore::new(&url, cfg.dimensions, cfg.max_entries)
                .map(Arc::new)
                .map(Some)
        })
        .await
        .map_err(|e| anyhow::anyhow!("vector init: {e}"))?
    }

    #[cfg(not(feature = "backend-postgres"))]
    {
        let sp = resolve_path(config_path, &cfg.path);
        let r = tokio::task::spawn_blocking(move || {
            VectorStore::new(&sp, cfg.dimensions, cfg.max_entries)
                .map(Arc::new)
                .map(Some)
        })
        .await
        .map_err(|e| anyhow::anyhow!("vector init: {e}"))?;
        #[cfg(all(
            feature = "profile-local",
            not(feature = "profile-simple-server"),
            not(feature = "profile-multi-users-server")
        ))]
        {
            match r {
                Ok(v) => Ok(v),
                Err(e) => {
                    tracing::warn!("vector init failed: {e}; continuing without vector");
                    Ok(None)
                }
            }
        }
        #[cfg(any(
            feature = "profile-simple-server",
            feature = "profile-multi-users-server"
        ))]
        {
            r
        }
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
    client: reqwest::Client,
) -> Result<()> {
    let runtime_flow = flow_manager(config_path);

    match protocol_mode {
        "acp_stdio" | "adaptive" => {
            let mut server = crate::acp::r#impl::runtime::new_acp_server(
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
                Some(client),
                false,
                None,
            );
            crate::acp::r#impl::runtime::run_acp_server(&mut server).await
        }
        "acp_http" => {
            let server = crate::acp::r#impl::runtime::new_acp_server(
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
                Some(client),
                false,
                None,
            );
            crate::acp::r#impl::runtime::run_acp_http_server(
                Arc::new(server),
                acp_http_bind.to_string(),
            )
            .await
        }
        "mcp_stdio" => {
            let tr = crate::orchestration::tool::ToolRegistry::new();
            let s = crate::protocol::mcp_server::McpStdioServer::new(
                registry,
                Arc::new(tr),
                "go-on".into(),
                "1.1.0".into(),
            );
            s.run().await
        }
        "mcp_http" => {
            let tr = crate::orchestration::tool::ToolRegistry::new();
            let s = crate::protocol::mcp_server::McpHttpServer::new(
                registry,
                Arc::new(tr),
                "go-on".into(),
                "1.1.0".into(),
                acp_http_bind.into(),
            );
            s.run().await
        }
        other => anyhow::bail!("unsupported protocol mode: {other}"),
    }
}

fn flow_manager(config_path: &Path) -> Arc<FlowManager> {
    let app_config = match crate::config::AppConfig::load(config_path) {
        Ok(config) => Arc::new(config),
        Err(err) => {
            tracing::warn!(
                "failed to load app config for flow manager from {}: {}; falling back to defaults",
                config_path.display(),
                err
            );
            Arc::new(crate::config::AppConfig::default())
        }
    };

    Arc::new(FlowManager::new(app_config, None))
}
