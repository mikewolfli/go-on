//! Capabilities Registry — Plugin info singleton.
//!
//! Provides global access to the plugin info list for plugin discovery
//! at arbitrary call sites across the system.

/// Simple plugin descriptor, replacing the previous `PluginRegistry`.
#[derive(Debug, Clone)]
pub struct PluginInfo {
    /// Unique plugin identifier.
    pub id: String,
    /// Current state label (e.g. "registered", "active").
    pub state_label: &'static str,
}

/// Global singleton for the plugin info list, initialized at startup.
static GLOBAL_PLUGIN_REGISTRY: std::sync::OnceLock<Vec<PluginInfo>> = std::sync::OnceLock::new();

/// Register a plugin info list for global access.
///
/// Called once during system initialization from `main.rs`. Subsequent calls
/// are silently ignored (the first registration wins).
pub fn register_plugin_registry(plugins: Vec<PluginInfo>) {
    let _ = GLOBAL_PLUGIN_REGISTRY.set(plugins);
}
