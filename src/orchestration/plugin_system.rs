//! Plugin System — Extensible plugin architecture for go-on.
//!
//! Provides a trait-based plugin system that allows third-party extensions
//! to register tool implementations, skill providers, mode runtimes,
//! and policy enforcers without modifying core code.
//!
//! # Plugin types
//!
//! - `ToolPlugin`: Register custom tool implementations
//! - `SkillPlugin`: Register custom skill providers
//! - `ModePlugin`: Register custom mode runtimes
//! - `PolicyPlugin`: Register custom governance policies

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

// ---------------------------------------------------------------------------
// PluginManifest
// ---------------------------------------------------------------------------

/// Metadata describing a plugin.
#[allow(dead_code)] // F-GAP-12 — reserved for plugin system integration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// Unique plugin identifier.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Semantic version.
    pub version: String,
    /// Plugin author.
    pub author: String,
    /// Short description.
    pub description: String,
    /// Minimum go-on version required.
    pub min_go_on_version: String,
    /// What this plugin provides (tool, skill, mode, policy).
    pub provides: Vec<String>,
    /// Dependencies on other plugins.
    #[serde(default)]
    pub dependencies: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// PluginState
// ---------------------------------------------------------------------------

/// Current state of a loaded plugin.
#[allow(dead_code)] // F-GAP-12 — reserved for plugin system integration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginState {
    /// Plugin manifest has been read but not initialized.
    Registered,
    /// Plugin has been initialized and is ready.
    Loaded,
    /// Plugin is actively serving requests.
    Active,
    /// Plugin encountered an error.
    Error,
    /// Plugin has been unloaded.
    Unloaded,
}

impl PluginState {
    #[allow(dead_code)] // F-GAP-12 — reserved for plugin system integration
    pub fn label(&self) -> &str {
        match self {
            Self::Registered => "registered",
            Self::Loaded => "loaded",
            Self::Active => "active",
            Self::Error => "error",
            Self::Unloaded => "unloaded",
        }
    }
}

// ---------------------------------------------------------------------------
// Plugin trait
// ---------------------------------------------------------------------------

/// Core trait that all plugins must implement.
#[async_trait]
#[allow(dead_code)] // F-GAP-12 — reserved for plugin system integration
pub trait Plugin: Send + Sync {
    /// Get the plugin manifest.
    fn manifest(&self) -> &PluginManifest;

    /// Initialize the plugin. Called once when the plugin is loaded.
    async fn initialize(&mut self) -> anyhow::Result<()>;

    /// Shutdown the plugin. Called when the plugin is unloaded.
    async fn shutdown(&mut self) -> anyhow::Result<()>;

    /// Get the current plugin state.
    fn state(&self) -> PluginState;
}

// ---------------------------------------------------------------------------
// Built-in plugins
// ---------------------------------------------------------------------------

/// Built-in plugin that provides tool execution capabilities.
pub struct ToolPlugin {
    manifest: PluginManifest,
    state: PluginState,
}

impl ToolPlugin {
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest {
                id: "builtin:tool".to_string(),
                name: "Tool Plugin".to_string(),
                version: "1.0.0".to_string(),
                author: "go-on core".to_string(),
                description: "Provides tool execution capabilities for the agent runtime"
                    .to_string(),
                min_go_on_version: "1.0.0".to_string(),
                provides: vec!["tool".to_string()],
                dependencies: HashMap::new(),
            },
            state: PluginState::Registered,
        }
    }
}

#[async_trait]
impl Plugin for ToolPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    async fn initialize(&mut self) -> anyhow::Result<()> {
        self.state = PluginState::Active;
        tracing::info!("ToolPlugin initialized");
        Ok(())
    }

    async fn shutdown(&mut self) -> anyhow::Result<()> {
        self.state = PluginState::Unloaded;
        tracing::info!("ToolPlugin shut down");
        Ok(())
    }

    fn state(&self) -> PluginState {
        self.state
    }
}

impl Default for ToolPlugin {
    fn default() -> Self {
        Self::new()
    }
}

/// Built-in plugin that provides skill discovery and execution.
pub struct SkillPlugin {
    manifest: PluginManifest,
    state: PluginState,
}

impl SkillPlugin {
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest {
                id: "builtin:skill".to_string(),
                name: "Skill Plugin".to_string(),
                version: "1.0.0".to_string(),
                author: "go-on core".to_string(),
                description: "Provides skill discovery and execution for agent workflows"
                    .to_string(),
                min_go_on_version: "1.0.0".to_string(),
                provides: vec!["skill".to_string()],
                dependencies: HashMap::new(),
            },
            state: PluginState::Registered,
        }
    }
}

#[async_trait]
impl Plugin for SkillPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    async fn initialize(&mut self) -> anyhow::Result<()> {
        self.state = PluginState::Active;
        tracing::info!("SkillPlugin initialized");
        Ok(())
    }

    async fn shutdown(&mut self) -> anyhow::Result<()> {
        self.state = PluginState::Unloaded;
        tracing::info!("SkillPlugin shut down");
        Ok(())
    }

    fn state(&self) -> PluginState {
        self.state
    }
}

impl Default for SkillPlugin {
    fn default() -> Self {
        Self::new()
    }
}

/// Built-in plugin that handles protocol mode selection.
pub struct ModePlugin {
    manifest: PluginManifest,
    state: PluginState,
}

impl ModePlugin {
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest {
                id: "builtin:mode".to_string(),
                name: "Mode Plugin".to_string(),
                version: "1.0.0".to_string(),
                author: "go-on core".to_string(),
                description: "Handles protocol mode selection and runtime mode switching"
                    .to_string(),
                min_go_on_version: "1.0.0".to_string(),
                provides: vec!["mode".to_string()],
                dependencies: HashMap::new(),
            },
            state: PluginState::Registered,
        }
    }
}

#[async_trait]
impl Plugin for ModePlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    async fn initialize(&mut self) -> anyhow::Result<()> {
        self.state = PluginState::Active;
        tracing::info!("ModePlugin initialized");
        Ok(())
    }

    async fn shutdown(&mut self) -> anyhow::Result<()> {
        self.state = PluginState::Unloaded;
        tracing::info!("ModePlugin shut down");
        Ok(())
    }

    fn state(&self) -> PluginState {
        self.state
    }
}

impl Default for ModePlugin {
    fn default() -> Self {
        Self::new()
    }
}

/// Built-in plugin that enforces governance policies.
pub struct PolicyPlugin {
    manifest: PluginManifest,
    state: PluginState,
}

impl PolicyPlugin {
    pub fn new() -> Self {
        Self {
            manifest: PluginManifest {
                id: "builtin:policy".to_string(),
                name: "Policy Plugin".to_string(),
                version: "1.0.0".to_string(),
                author: "go-on core".to_string(),
                description: "Enforces governance policies across agent operations".to_string(),
                min_go_on_version: "1.0.0".to_string(),
                provides: vec!["policy".to_string()],
                dependencies: HashMap::new(),
            },
            state: PluginState::Registered,
        }
    }
}

#[async_trait]
impl Plugin for PolicyPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    async fn initialize(&mut self) -> anyhow::Result<()> {
        self.state = PluginState::Active;
        tracing::info!("PolicyPlugin initialized");
        Ok(())
    }

    async fn shutdown(&mut self) -> anyhow::Result<()> {
        self.state = PluginState::Unloaded;
        tracing::info!("PolicyPlugin shut down");
        Ok(())
    }

    fn state(&self) -> PluginState {
        self.state
    }
}

impl Default for PolicyPlugin {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// PluginRegistry
// ---------------------------------------------------------------------------

/// Central registry for all plugins.
pub struct PluginRegistry {
    plugins: Mutex<HashMap<String, Box<dyn Plugin>>>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            plugins: Mutex::new(HashMap::new()),
        }
    }

    /// Register the 4 built-in plugins (Tool, Skill, Mode, Policy).
    /// This should be called once at startup after creating the registry.
    pub fn register_builtin_plugins(&self) {
        let builtins: Vec<Box<dyn Plugin>> = vec![
            Box::new(ToolPlugin::new()),
            Box::new(SkillPlugin::new()),
            Box::new(ModePlugin::new()),
            Box::new(PolicyPlugin::new()),
        ];
        for plugin in builtins {
            let id = plugin.manifest().id.clone();
            match self.register(plugin) {
                Ok(()) => tracing::info!("Registered built-in plugin: {id}"),
                Err(e) => tracing::warn!("Failed to register built-in plugin {id}: {e}"),
            }
        }
    }

    /// Register a new plugin.
    pub fn register(&self, plugin: Box<dyn Plugin>) -> Result<(), String> {
        let id = plugin.manifest().id.clone();
        let mut plugins = self.plugins.lock().map_err(|e| e.to_string())?;
        if plugins.contains_key(&id) {
            return Err(format!("Plugin {} is already registered", id));
        }
        plugins.insert(id, plugin);
        Ok(())
    }

    /// Get a plugin by ID.
    #[allow(dead_code)] // F-GAP-12 — reserved for plugin system integration
    pub fn get(&self, id: &str) -> Option<PluginState> {
        let plugins = self.plugins.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        plugins.get(id).map(|p| p.state())
    }

    /// List all registered plugin IDs.
    #[allow(dead_code)] // F-GAP-12 — reserved for plugin system integration
    pub fn list(&self) -> Vec<String> {
        self.plugins
            .lock()
            .map(|p| p.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Count registered plugins.
    pub fn count(&self) -> usize {
        self.plugins.lock().map(|p| p.len()).unwrap_or(0)
    }

    /// Unregister a plugin by ID.
    #[allow(dead_code)] // F-GAP-12 — reserved for plugin system integration
    pub fn unregister(&self, id: &str) -> Result<(), String> {
        let mut plugins = self.plugins.lock().map_err(|e| e.to_string())?;
        plugins
            .remove(id)
            .ok_or_else(|| format!("Plugin {} not found", id))?;
        Ok(())
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct TestPlugin {
        manifest: PluginManifest,
        state: PluginState,
        initialized: AtomicBool,
    }

    impl TestPlugin {
        fn new(id: &str) -> Self {
            Self {
                manifest: PluginManifest {
                    id: id.to_string(),
                    name: format!("Test {}", id),
                    version: "1.0.0".to_string(),
                    author: "test".to_string(),
                    description: "Test plugin".to_string(),
                    min_go_on_version: "1.0.0".to_string(),
                    provides: vec!["tool".to_string()],
                    dependencies: HashMap::new(),
                },
                state: PluginState::Registered,
                initialized: AtomicBool::new(false),
            }
        }
    }

    #[async_trait]
    impl Plugin for TestPlugin {
        fn manifest(&self) -> &PluginManifest {
            &self.manifest
        }
        async fn initialize(&mut self) -> anyhow::Result<()> {
            self.initialized.store(true, Ordering::Relaxed);
            self.state = PluginState::Active;
            Ok(())
        }
        async fn shutdown(&mut self) -> anyhow::Result<()> {
            self.state = PluginState::Unloaded;
            Ok(())
        }
        fn state(&self) -> PluginState {
            self.state
        }
    }

    #[test]
    fn test_plugin_registry_register_and_list() {
        let registry = PluginRegistry::new();
        let plugin = Box::new(TestPlugin::new("test-1"));
        registry.register(plugin).unwrap();
        assert_eq!(registry.count(), 1);
        assert_eq!(registry.list(), vec!["test-1".to_string()]);
    }

    #[test]
    fn test_plugin_registry_duplicate_fails() {
        let registry = PluginRegistry::new();
        registry
            .register(Box::new(TestPlugin::new("test-1")))
            .unwrap();
        let result = registry.register(Box::new(TestPlugin::new("test-1")));
        assert!(result.is_err());
    }

    #[test]
    fn test_plugin_registry_unregister() {
        let registry = PluginRegistry::new();
        registry
            .register(Box::new(TestPlugin::new("test-1")))
            .unwrap();
        registry.unregister("test-1").unwrap();
        assert_eq!(registry.count(), 0);
    }

    #[test]
    fn test_plugin_state_labels() {
        assert_eq!(PluginState::Loaded.label(), "loaded");
        assert_eq!(PluginState::Active.label(), "active");
    }

    #[test]
    fn test_plugin_manifest_structure() {
        let manifest = PluginManifest {
            id: "test".to_string(),
            name: "Test".to_string(),
            version: "1.0.0".to_string(),
            author: "test".to_string(),
            description: "test".to_string(),
            min_go_on_version: "1.0.0".to_string(),
            provides: vec!["tool".to_string()],
            dependencies: HashMap::new(),
        };
        assert_eq!(manifest.id, "test");
        assert!(manifest.provides.contains(&"tool".to_string()));
    }
}
