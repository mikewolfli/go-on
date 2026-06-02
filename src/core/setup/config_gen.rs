//! Config generation sub-module (GAP-B53-23).
//!
//! Re-exports config generation functions from the parent `setup::mod.rs`.
//! These are the actual implementations:
//!   - `recommendation_snapshot_for_config`
//!   - `apply_recommended_to_config`
//!   - `write_default_rules` (in mod.rs)
//!
//! TODO: Extract these functions from `super::mod.rs` into this file
//!       to reduce the size of mod.rs.

use anyhow::Result;

/// Generate a recommended config TOML and write it to the given path.
/// Delegates to `super::apply_recommended_to_config`.
pub fn generate_config(config_path: &std::path::Path) -> Result<()> {
    super::apply_recommended_to_config(config_path)
}

/// Build a recommendation snapshot for diagnostics/display.
pub fn config_recommendation(config: &crate::config::AppConfig) -> Option<super::ProviderRecommendationSnapshot> {
    super::recommendation_snapshot_for_config(config)
}
