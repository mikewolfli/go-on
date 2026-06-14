//! Provider catalog sub-module.
//!
//! Handles fetching provider catalog from the backend's `provider.catalog` RPC endpoint
//! vs. the hardcoded built-in provider list.
//!
//! The backend's `/provider/catalog` endpoint is the authoritative source for provider
//! definitions. The GUI uses `built_in_provider_specs()` as a sync fallback when the
//! backend is unreachable (e.g., first-run setup before any backend is running).
//!
//! FUTURE: After startup, the GUI can call `fetch_catalog()` to get the authoritative
//! catalog and overlay it on top of the built-in fallback.
//!
//! TODO-BACKEND-ENDPOINT: The backend `/v1/providers/catalog` (or `provider.catalog` RPC)
//! endpoint exists but is not yet wired into the GUI startup flow. Once wired, the startup
//! sequence should:
//!   1. Check backend availability via `health()`
//!   2. Call `fetch_catalog()` to get the authoritative catalog
//!   3. Merge or overlay the backend catalog on top of `built_in_provider_specs()`
//!   4. Cache the result so subsequent lookups use the backend data
//!      See F-GAP-59 for tracking.

use crate::backend::BackendClient;
use serde_json::Value;

/// Provider metadata: agent type, default URL, default model, supports system prompt.
pub struct ProviderSpec {
    pub agent_type: &'static str,
    pub default_url: Option<&'static str>,
    pub default_model: &'static str,
    pub supports_system: bool,
}

/// Hardcoded catalog fallback used when backend is unreachable.
/// Keep in sync with backend's `built_in_provider_specs()`.
pub fn built_in_provider_specs(name: &str) -> ProviderSpec {
    match name {
        "openai" => ProviderSpec {
            agent_type: "openai",
            default_url: Some("https://api.openai.com/v1"),
            default_model: "gpt-4o-mini",
            supports_system: true,
        },
        "openai_compatible" => ProviderSpec {
            agent_type: "openai_compatible",
            default_url: Some("http://127.0.0.1:8080/v1"),
            default_model: "compatible-model",
            supports_system: true,
        },
        "anthropic" => ProviderSpec {
            agent_type: "claude",
            default_url: Some("https://api.anthropic.com"),
            default_model: "claude-sonnet-4-20250514",
            supports_system: true,
        },
        "deepseek" => ProviderSpec {
            agent_type: "deepseek",
            default_url: Some("https://api.deepseek.com"),
            default_model: "deepseek-v4-flash",
            supports_system: true,
        },
        "gemini" => ProviderSpec {
            agent_type: "gemini",
            default_url: Some("https://generativelanguage.googleapis.com/v1beta"),
            default_model: "gemini-2.5-flash",
            supports_system: false,
        },
        "copilot" => ProviderSpec {
            agent_type: "openai_compatible",
            default_url: Some("https://api.githubcopilot.com"),
            default_model: "auto",
            supports_system: false,
        },
        _ => ProviderSpec {
            agent_type: "openai_compatible",
            default_url: None,
            default_model: "auto",
            supports_system: false,
        },
    }
}

/// Fetch provider catalog from the backend asynchronously.
/// Returns `None` if backend is unreachable.
/// F-GAP-59: Reserved for post-startup catalog overlay
#[expect(dead_code)] // F-GAP-59: Wired when startup flow calls fetch_catalog
pub async fn fetch_catalog(backend: &BackendClient) -> Option<Value> {
    match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        backend.provider_catalog(),
    )
    .await
    {
        Ok(Ok(value)) => Some(value),
        _ => None,
    }
}
