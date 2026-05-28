//! Capabilities Registry — Central integration hub for go-on subsystems.
//!
//! Initializes and holds references to all major capability engines during
//! application startup. Consumers access capabilities via dependency injection
//! rather than through global state (except `PluginRegistry` which has a
//! dedicated global singleton for plugin discovery at arbitrary call sites).
//!
//! ## Architecture
//!
//! Each field in [`CapabilitiesHandle`] corresponds to a subsystem that is
//! actively wired into the execution path of at least one consumer module:
//!
//! | Field                | Consumer(s)                                   |
//! |----------------------|-----------------------------------------------|
//! | `cache_warming`      | `orchestrator.rs`                             |
//! | `complexity_estimator` | `full_auto.rs`                              |
//! | `diagnostic_feedback`  | `full_auto.rs`                              |
//! | `plugin_registry`    | `main.rs` (global registration)               |
//! | `session_context`    | `chat.rs`                                     |
//! | `tool_recommender`   | `full_auto.rs`                                |
//! | `tool_lock_manager`  | `full_auto.rs`                                |
//! | `schema_manager`     | `config::load.rs`                             |
//! | `sse_buffer_pool`    | `chat.rs` (via `OnceLock`)                    |

use crate::agents::sse_optimizer::SseBufferPool;
use crate::core::config::schema_version::SchemaManager;
use crate::orchestration::cache_warming::{CacheWarmingEngine, PreWarmConfig};
use crate::orchestration::complexity_estimator::ComplexityEstimator;
use crate::orchestration::diagnostic_feedback::DiagnosticFeedbackEngine;
use crate::orchestration::plugin_system::PluginRegistry;
use crate::orchestration::session_context::{ContextWindowBudget, SessionContextManager};
use crate::orchestration::tool_lock::ToolLockManager;
use crate::orchestration::tool_recommender::ToolRecommender;

/// Initialize all system capabilities and register them.
///
/// Call this once during application startup (from `main.rs`) to construct
/// every subsystem with sensible defaults. The returned [`CapabilitiesHandle`]
/// is intentionally discarded in the current bootstrap phase; callers that
/// require a specific engine obtain it via direct construction or the global
/// plugin registry.
#[allow(dead_code)]
pub fn initialize_capabilities() -> CapabilitiesHandle {
    CapabilitiesHandle::new()
}

/// Handle holding references to all initialized capability engines.
///
/// Each field corresponds to a subsystem that has its producer wired to at
/// least one consumer in the production codebase. Fields are `pub` to allow
/// direct field access when the handle is eventually threaded through the
/// application's dependency graph.
#[allow(dead_code)] // F-GAP-12 — reserved for capabilities handle integration
pub struct CapabilitiesHandle {
    pub cache_warming: CacheWarmingEngine,
    pub complexity_estimator: ComplexityEstimator,
    pub diagnostic_feedback: DiagnosticFeedbackEngine,
    pub plugin_registry: PluginRegistry,
    pub session_context: SessionContextManager,
    pub tool_recommender: ToolRecommender,
    pub tool_lock_manager: ToolLockManager,
    pub schema_manager: SchemaManager,
    pub sse_buffer_pool: SseBufferPool,
}

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

impl CapabilitiesHandle {
    /// Construct every capability engine with default configuration.
    ///
    /// This is the single chokepoint for subsystem initialization. Adding a
    /// new field requires:
    /// 1. An entry in this constructor
    /// 2. Its type's module wired into at least one consumer
    /// 3. The consumer's usage documented in the struct-level table above
    pub fn new() -> Self {
        Self {
            cache_warming: CacheWarmingEngine::new(PreWarmConfig::default()),
            complexity_estimator: ComplexityEstimator::new(),
            diagnostic_feedback: DiagnosticFeedbackEngine::new(),
            plugin_registry: PluginRegistry::new(),
            session_context: SessionContextManager::new(ContextWindowBudget::default()),
            tool_recommender: ToolRecommender::new(),
            tool_lock_manager: ToolLockManager::new(),
            schema_manager: SchemaManager::new(),
            sse_buffer_pool: SseBufferPool::new(4, 4096),
        }
    }
}

impl Default for CapabilitiesHandle {
    fn default() -> Self {
        Self::new()
    }
}
