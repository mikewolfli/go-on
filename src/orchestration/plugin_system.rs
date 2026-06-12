//! Plugin System — Extensible plugin architecture for go-on.
//!
//! Provides a trait-based plugin system that allows third-party extensions
//! to register tool implementations, skill providers, mode runtimes,
//! and policy enforcers without modifying core code.
//!
//! # Built-in plugins
//!
//! Two `NoOpPlugin` instances are registered at startup as fallback
//! placeholders for mode and policy. Two real plugin implementations
//! — `TelemetryPlugin` and `MetricsPlugin` — are registered for tool
//! and skill, providing lifecycle hooks for observability.
//! External plugins implement [`Plugin`] directly.

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

// ---------------------------------------------------------------------------
// PluginContext / PluginResult — lifecycle event payloads
// ---------------------------------------------------------------------------

/// Context passed to plugin lifecycle hooks.
#[derive(Debug, Clone)]
pub struct PluginContext {
    pub agent_name: String,
    pub task_id: String,
    pub session_id: Option<String>,
}

/// Result returned by an agent execution, passed to plugin completion hooks.
#[derive(Debug, Clone)]
pub struct PluginResult {
    pub success: bool,
    pub error: Option<String>,
    pub duration_ms: u64,
    pub tool_call_count: u64,
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

    // ── Lifecycle hooks (all have default no-op impls) ────────────────────

    /// Called when an agent starts executing a task.
    fn on_agent_start(&self, _ctx: &PluginContext) -> anyhow::Result<()> {
        Ok(())
    }

    /// Called when an agent completes a task (success or failure).
    fn on_agent_complete(
        &self,
        _ctx: &PluginContext,
        _result: &PluginResult,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// Called before a tool is executed. Return `false` to block the call.
    fn on_tool_execute(&self, _tool_name: &str) -> anyhow::Result<bool> {
        Ok(true)
    }

    /// Called when an error occurs during agent execution.
    fn on_error(&self, _ctx: &PluginContext, _error: &str) -> anyhow::Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// TelemetryPlugin — logging/telemetry for agent lifecycle events
// ---------------------------------------------------------------------------

/// A plugin that records agent lifecycle events via `tracing::info!`.
///
/// Registered under the ID `builtin:tool` at startup. Provides
/// real implementations of `on_agent_start` and `on_agent_complete`
/// so that operators can observe agent execution flow through structured
/// log output without external monitoring infrastructure.
pub struct TelemetryPlugin {
    manifest: PluginManifest,
    state: PluginState,
}

impl TelemetryPlugin {
    pub fn new(manifest: PluginManifest) -> Self {
        Self {
            manifest,
            state: PluginState::Registered,
        }
    }
}

#[async_trait]
impl Plugin for TelemetryPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    async fn initialize(&mut self) -> anyhow::Result<()> {
        self.state = PluginState::Active;
        tracing::info!(target: "plugin", "{} initialized", self.manifest.name);
        Ok(())
    }

    async fn shutdown(&mut self) -> anyhow::Result<()> {
        self.state = PluginState::Unloaded;
        tracing::info!(target: "plugin", "{} shut down", self.manifest.name);
        Ok(())
    }

    fn state(&self) -> PluginState {
        self.state
    }

    fn on_agent_start(&self, ctx: &PluginContext) -> anyhow::Result<()> {
        tracing::info!(
            target: "plugin",
            event = "agent_start",
            agent = %ctx.agent_name,
            task = %ctx.task_id,
            session = %ctx.session_id.as_deref().unwrap_or("none"),
            "Agent started"
        );
        Ok(())
    }

    fn on_agent_complete(&self, ctx: &PluginContext, result: &PluginResult) -> anyhow::Result<()> {
        tracing::info!(
            target: "plugin",
            event = "agent_complete",
            agent = %ctx.agent_name,
            task = %ctx.task_id,
            success = %result.success,
            duration_ms = %result.duration_ms,
            tool_calls = %result.tool_call_count,
            error = %result.error.as_deref().unwrap_or("none"),
            "Agent completed"
        );
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// MetricsPlugin — execution stats recording for plugin events
// ---------------------------------------------------------------------------

/// A plugin that records agent execution metrics via `tracing::info!`.
///
/// Registered under the ID `builtin:skill` at startup. Provides
/// implementations of `on_agent_start`, `on_agent_complete`, and
/// `on_tool_execute` to emit structured metrics that can be consumed
/// by observability pipelines.
pub struct MetricsPlugin {
    manifest: PluginManifest,
    state: PluginState,
}

impl MetricsPlugin {
    pub fn new(manifest: PluginManifest) -> Self {
        Self {
            manifest,
            state: PluginState::Registered,
        }
    }
}

#[async_trait]
impl Plugin for MetricsPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    async fn initialize(&mut self) -> anyhow::Result<()> {
        self.state = PluginState::Active;
        tracing::info!(target: "plugin", "{} initialized", self.manifest.name);
        Ok(())
    }

    async fn shutdown(&mut self) -> anyhow::Result<()> {
        self.state = PluginState::Unloaded;
        tracing::info!(target: "plugin", "{} shut down", self.manifest.name);
        Ok(())
    }

    fn state(&self) -> PluginState {
        self.state
    }

    fn on_agent_start(&self, ctx: &PluginContext) -> anyhow::Result<()> {
        tracing::info!(
            target: "plugin",
            metric = "agent_start_count",
            agent = %ctx.agent_name,
            value = 1u64,
            "[METRIC] agent_start_count"
        );
        Ok(())
    }

    fn on_agent_complete(&self, ctx: &PluginContext, result: &PluginResult) -> anyhow::Result<()> {
        tracing::info!(
            target: "plugin",
            metric = "agent_complete_count",
            agent = %ctx.agent_name,
            success = %result.success,
            value = 1u64,
            "[METRIC] agent_complete_count"
        );
        tracing::info!(
            target: "plugin",
            metric = "agent_duration_ms_total",
            agent = %ctx.agent_name,
            value = %result.duration_ms,
            "[METRIC] agent_duration_ms_total"
        );
        tracing::info!(
            target: "plugin",
            metric = "agent_tool_calls_total",
            agent = %ctx.agent_name,
            value = %result.tool_call_count,
            "[METRIC] agent_tool_calls_total"
        );
        Ok(())
    }

    fn on_tool_execute(&self, tool_name: &str) -> anyhow::Result<bool> {
        tracing::info!(
            target: "plugin",
            metric = "tool_execute_count",
            tool = %tool_name,
            value = 1u64,
            "[METRIC] tool_execute_count"
        );
        Ok(true)
    }
}

// ---------------------------------------------------------------------------
// NoOpPlugin — default placeholder for built-in plugin types
// ---------------------------------------------------------------------------

/// A no-operation plugin that implements [`Plugin`] with minimal behavior.
///
/// Two instances are registered at startup (mode, policy) as fallback
/// placeholders until the real ModePlugin and PolicyPlugin implementations
/// are available (planned for Phase 8+). All instances share the same
/// initialization and shutdown logic: mark the plugin as Active on init,
/// Unloaded on shutdown, and log a trace message.
///
/// Using a single struct eliminates the 4x boilerplate of separate
/// `ToolPlugin` / `SkillPlugin` / `ModePlugin` / `PolicyPlugin` types.
///
/// # Real plugin availability
///
/// - **ModePlugin**: Planned for Phase 8 — will provide mode-specific
///   lifecycle hooks (e.g. ask-mode vs full-auto-mode orchestration).
/// - **PolicyPlugin**: Planned for Phase 8 — will allow external policy
///   enforcers (e.g. Sentry, OpenPolicyAgent) to hook into agent execution
///   via the `on_tool_execute` / `on_agent_complete` callbacks.
/// - **ToolPlugin / SkillPlugin**: Available today via `TelemetryPlugin`
///   (builtin:tool) and `MetricsPlugin` (builtin:skill).
pub struct NoOpPlugin {
    manifest: PluginManifest,
    state: PluginState,
}

impl NoOpPlugin {
    /// Create a new no-op plugin with the given manifest.
    pub fn new(manifest: PluginManifest) -> Self {
        Self {
            manifest,
            state: PluginState::Registered,
        }
    }

    /// Shorthand constructor for a built-in plugin with the given parameters.
    fn builtin(id: &str, name: &str, description: &str, provides: &str) -> Self {
        Self::new(PluginManifest {
            id: format!("builtin:{}", id),
            name: name.to_string(),
            version: "1.0.0".to_string(),
            author: "go-on core".to_string(),
            description: description.to_string(),
            min_go_on_version: "1.0.0".to_string(),
            provides: vec![provides.to_string()],
            dependencies: HashMap::new(),
        })
    }
}

#[async_trait]
impl Plugin for NoOpPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    async fn initialize(&mut self) -> anyhow::Result<()> {
        self.state = PluginState::Active;
        tracing::info!("{} initialized", self.manifest.name);
        Ok(())
    }

    async fn shutdown(&mut self) -> anyhow::Result<()> {
        self.state = PluginState::Unloaded;
        tracing::info!("{} shut down", self.manifest.name);
        Ok(())
    }

    fn state(&self) -> PluginState {
        self.state
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
            Box::new(TelemetryPlugin::new(PluginManifest {
                id: "builtin:tool".to_string(),
                name: "Telemetry Plugin".to_string(),
                version: "1.0.0".to_string(),
                author: "go-on core".to_string(),
                description: "Records agent lifecycle events via tracing".to_string(),
                min_go_on_version: "1.0.0".to_string(),
                provides: vec!["tool".to_string()],
                dependencies: HashMap::new(),
            })),
            Box::new(MetricsPlugin::new(PluginManifest {
                id: "builtin:skill".to_string(),
                name: "Metrics Plugin".to_string(),
                version: "1.0.0".to_string(),
                author: "go-on core".to_string(),
                description: "Records agent execution metrics via tracing".to_string(),
                min_go_on_version: "1.0.0".to_string(),
                provides: vec!["skill".to_string()],
                dependencies: HashMap::new(),
            })),
            Box::new(NoOpPlugin::builtin(
                "mode",
                "Mode Plugin",
                "Handles protocol mode selection and runtime mode switching",
                "mode",
            )),
            Box::new(NoOpPlugin::builtin(
                "policy",
                "Policy Plugin",
                "Enforces governance policies across agent operations",
                "policy",
            )),
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
