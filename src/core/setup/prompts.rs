//! Interactive setup prompt sub-module (GAP-B53-23).
//!
//! Re-exports setup prompt functions from the parent `setup::mod.rs`.
//! These are the actual implementations:
//!   - `run_setup` / `run_setup_with_options`
//!   - `prompt_provider_selection*`
//!   - `prompt_setup_level`
//!   - `prompt_additional_agents`
//!
//! TODO: Extract these functions from `super::mod.rs` into this file
//!       to reduce the size of mod.rs.

use anyhow::Result;

/// Run the interactive setup flow. Delegates to `super::run_setup`.
pub fn run_interactive_setup(config_path: &std::path::Path) -> Result<()> {
    super::run_setup(config_path)
}
