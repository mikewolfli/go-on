//! Profile construction — governance metrics snapshot
//!
//! Builds a `StartupContextProfile` from the cached context and configuration
//! for use by governance/status endpoints.

use super::*;

/// Build a `StartupContextProfile` from the cached context and configuration.
#[allow(
    dead_code,
    reason = "Public API surface for governance/status endpoint (cfg(test) re-exported)"
)]
pub fn startup_context_profile(
    ctx: &StartupContext,
    cfg: &StartupContextConfig,
) -> StartupContextProfile {
    StartupContextProfile {
        enabled: cfg.enabled,
        loaded: ctx.loaded,
        loaded_components: ctx.loaded_components.clone(),
        file_count: ctx.file_count,
        char_count: ctx.char_count,
        readme_chars: ctx.readme_chars,
        build_command_count: ctx.build_commands.len(),
        style_rule_count: ctx.style_rules.len(),
        commit_count: ctx.recent_commits.len(),
        has_code_repo: ctx.has_code_repo,
    }
}
