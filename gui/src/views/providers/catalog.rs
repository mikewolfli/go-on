//! Provider catalog sub-module.
//!
//! Handles fetching provider catalog from the backend's `provider.catalog` RPC endpoint
//! vs. the offline fallback in `generated_catalog`.
//!
//! The backend's `/provider/catalog` endpoint is the authoritative source for provider
//! definitions. The GUI uses the generated offline fallback (`generated_catalog`, kept in
//! sync with the backend by `scripts/gen-provider-catalog.py`) when the backend is
//! unreachable (e.g., first-run setup before any backend is running).

/// Provider metadata: agent type, default URL, default model, supports system prompt.
pub struct ProviderSpec {
    pub agent_type: &'static str,
    pub default_url: Option<&'static str>,
    pub default_model: &'static str,
    pub supports_system: bool,
}

/// Offline provider spec lookup — delegates to the generated catalog which is
/// kept in sync with the backend's `built_in_provider_specs()` (single source
/// of truth). Unknown names fall back to the generic openai_compatible shape.
pub use super::generated_catalog::built_in_provider_specs;
