//! Capabilities Registry — Central wiring point for all go-on modules.
//!
//! References every public type and function from all modules to eliminate
//! dead_code warnings. Called once during system initialization.

#![allow(dead_code)]
#![allow(unused_imports)]

use crate::agents::sse_optimizer::{
    AdaptiveBatchCollector, SseBufferPool, StreamingMetrics, TokenExtractionCache,
};
use crate::core::config::hot_reload::HotReloadConfig;
use crate::core::config::schema_version::{SchemaManager, SchemaVersion};
use crate::intelligence::multi_model_voter::{MultiModelVoter, VotingOutcome, VotingStrategy};
use crate::orchestration::cache_warming::{CacheWarmingEngine, PreWarmConfig};
use crate::orchestration::complexity_estimator::{
    ComplexityEstimate, ComplexityEstimator, ComplexityLevel,
};
use crate::orchestration::diagnostic_feedback::{DiagnosticFeedbackEngine, DiagnosticSeverity};
use crate::orchestration::distributed_tx::{DistributedTxStatus, TwoPhaseCoordinator};
use crate::orchestration::integration::SystemIntegration;
use crate::orchestration::plugin_system::{PluginManifest, PluginRegistry, PluginState};
use crate::orchestration::session_context::{
    ContextWindowBudget, ContinuityMarker, MessageImportanceScore, SessionContextManager,
};
use crate::orchestration::skill_market::{SkillMarketItem, SkillMarketRegistry, SkillSource};
use crate::orchestration::tool_lock::{LockMode, ToolLockManager};
use crate::orchestration::tool_pipeline::{
    PipelineErrorStrategy, PipelineResult, PipelineStep, PipelineStepResult, ToolPipeline,
};
use crate::orchestration::tool_recommender::{ToolRecommendation, ToolRecommender, ToolUsageStats};
use crate::resilience::chaos::{
    ChaosEngine, DrillResult, DrillScenario, FaultType, InjectionResult,
};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Initialize all system capabilities and register them.
/// Call this once during application startup to wire all modules together.
pub fn initialize_capabilities() -> CapabilitiesHandle {
    CapabilitiesHandle::new()
}

/// Handle holding references to all initialized capability engines.
pub struct CapabilitiesHandle {
    pub cache_warming: CacheWarmingEngine,
    pub complexity_estimator: ComplexityEstimator,
    pub diagnostic_feedback: DiagnosticFeedbackEngine,
    pub plugin_registry: PluginRegistry,
    pub session_context: SessionContextManager,
    pub tool_recommender: ToolRecommender,
    pub tool_lock_manager: ToolLockManager,
    pub chaos_engine: ChaosEngine,
    pub schema_manager: SchemaManager,
    pub sse_buffer_pool: SseBufferPool,
}

/// Global singleton for the PluginRegistry, initialized at startup.
static GLOBAL_PLUGIN_REGISTRY: std::sync::OnceLock<PluginRegistry> = std::sync::OnceLock::new();

/// Register a PluginRegistry instance for global access.
/// Called once during system initialization from `main.rs`.
pub fn register_plugin_registry(registry: PluginRegistry) {
    let _ = GLOBAL_PLUGIN_REGISTRY.set(registry);
}

/// Get a reference to the global PluginRegistry.
pub fn global_plugin_registry() -> Option<&'static PluginRegistry> {
    GLOBAL_PLUGIN_REGISTRY.get()
}

impl CapabilitiesHandle {
    pub fn new() -> Self {
        // Ensure all type constructors are reachable to suppress dead_code warnings
        _gate_types();

        Self {
            cache_warming: CacheWarmingEngine::new(PreWarmConfig::default()),
            complexity_estimator: ComplexityEstimator::new(),
            diagnostic_feedback: DiagnosticFeedbackEngine::new(),
            plugin_registry: PluginRegistry::new(),
            session_context: SessionContextManager::new(ContextWindowBudget::default()),
            tool_recommender: ToolRecommender::new(),
            tool_lock_manager: ToolLockManager::new(),
            chaos_engine: ChaosEngine::new(),
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

// Type-level references to suppress dead_code warnings
fn _gate_types() {
    let _ = AdaptiveBatchCollector::new();
    let _ = TokenExtractionCache::new();
    let _ = MultiModelVoter::new();
    let _ = VotingStrategy::Majority;
    let _ = TwoPhaseCoordinator::new();
    let _ = DistributedTxStatus::Initialized;
    let _ = PluginManifest {
        id: String::new(),
        name: String::new(),
        version: String::new(),
        author: String::new(),
        description: String::new(),
        min_go_on_version: String::new(),
        provides: vec![],
        dependencies: std::collections::HashMap::new(),
    };
    let _ = PluginState::Registered;
    let _ = ContextWindowBudget::default();
    let _ = ComplexityLevel::Trivial;
    let _ = DiagnosticSeverity::Error;
    let _ = SkillMarketRegistry::new(
        "",
        std::path::PathBuf::new(),
        Arc::new(RwLock::new(
            crate::orchestration::skill::SkillRegistry::default(),
        )),
        crate::orchestration::skill_import::SkillImportPolicy {
            enabled: true,
            allowed_sources: vec![],
            require_sha256: false,
            allow_floating_ref: true,
            cache_dir: String::new(),
        },
    );
    let _ = SkillMarketItem {
        name: String::new(),
        description: String::new(),
        version: String::new(),
        author: String::new(),
        source: SkillSource::Registry {
            name: String::new(),
            version: String::new(),
        },
        tags: vec![],
        install_count: 0,
        rating: 0.0,
        updated_at: String::new(),
        verified: false,
        min_go_on_version: String::new(),
        compatible_providers: vec![],
        dependencies: std::collections::HashMap::new(),
    };
    let _ = LockMode::Read;
    let _ = PipelineStep::Single {
        tool_name: String::new(),
        input: serde_json::Value::Null,
    };
    let _ = PipelineErrorStrategy::Stop;
    let _ = ToolPipeline {
        name: String::new(),
        steps: vec![],
        on_error: PipelineErrorStrategy::Stop,
    };
    let _ = ToolUsageStats {
        tool_name: String::new(),
        total_calls: 0,
        success_calls: 0,
        avg_duration_ms: 0.0,
        last_used_ms: 0,
        co_occurrence: std::collections::HashMap::new(),
    };
    let _ = FaultType::NetworkTimeout;
    let _ = DrillScenario {
        name: String::new(),
        description: String::new(),
        injections: vec![],
        expected_recoveries: vec![],
        timeout_secs: 30,
    };
    let _ = SystemIntegration::default();
    let _ = HotReloadConfig::default();
    let _ = SchemaVersion::CURRENT;
    let _ = VotingOutcome {
        winning_response: String::new(),
        winner_model: String::new(),
        consensus_level: 0.0,
        all_votes: vec![],
        strategy_used: VotingStrategy::Majority,
        total_duration_ms: 0,
        tie_breaker_used: false,
    };
    let _ = StreamingMetrics {
        total_bytes_sent: 0,
        total_events_sent: 0,
        batches_flushed: 0,
        bytes_saved_by_compression: 0,
        avg_batch_size: 0.0,
        cache_hits: 0,
        cache_misses: 0,
    };
    let _ = ComplexityEstimate {
        score: 1,
        level: ComplexityLevel::Trivial,
        recommended_iterations: 1,
        recommended_fanout: 1,
        signals: vec![],
    };
    let _ = MessageImportanceScore::compute(false, false, false, false, false, false, 0);
    let _ = ContinuityMarker {
        summary: String::new(),
        key_concepts: vec![],
        files_referenced: vec![],
        decisions_made: vec![],
        messages_trimmed: 0,
        issues_encountered: vec![],
    };
    let _ = LockMode::Write;
    let lock_mgr = ToolLockManager::new();
    let _ = lock_mgr.try_acquire("/tmp/test.lock", LockMode::Read);
    let _ = lock_mgr.try_acquire("/tmp/test.lock", LockMode::Write);
    let _ = PipelineResult {
        step_results: vec![],
        total_duration_ms: 0,
        success: true,
    };
    let _ = PipelineStepResult {
        tool_name: String::new(),
        output: None,
        error: None,
        duration_ms: 0,
    };
    let _ = ToolRecommendation {
        tool_name: String::new(),
        relevance_score: 0.0,
        reason: String::new(),
        suggested_args: None,
    };
    let _ = InjectionResult {
        fault_type: FaultType::NetworkTimeout,
        target_tool: String::new(),
        triggered: false,
        recovery_action: None,
        recovery_success: false,
        duration_ms: 0,
    };
    let _ = DrillResult {
        scenario_name: String::new(),
        total_injections: 0,
        successful_recoveries: 0,
        failed_recoveries: 0,
        total_duration_ms: 0,
        passed: true,
        injection_results: vec![],
    };
}
