//! Capabilities Registry — Plugin registry singleton.
//!
//! Provides global access to the `PluginRegistry` for plugin discovery
//! at arbitrary call sites across the system.

use crate::orchestration::plugin_system::PluginRegistry;

/// Global singleton for the PluginRegistry, initialized at startup.
static GLOBAL_PLUGIN_REGISTRY: std::sync::OnceLock<PluginRegistry> = std::sync::OnceLock::new();

/// Register a PluginRegistry instance for global access.
///
/// Called once during system initialization from `main.rs`. Subsequent calls
/// are silently ignored (the first registration wins).
pub fn register_plugin_registry(registry: PluginRegistry) {
    let _ = GLOBAL_PLUGIN_REGISTRY.set(registry);
}

/// Get a reference to the global PluginRegistry, if one has been registered.
#[allow(dead_code)] // F-GAP-12 — reserved for global plugin registry access
pub fn global_plugin_registry() -> Option<&'static PluginRegistry> {
    GLOBAL_PLUGIN_REGISTRY.get()
}
