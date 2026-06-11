pub mod artifact;
pub mod audit;
pub mod autonomy_runtime;
pub mod brain_loop; // F-GAP-17 (flat, legacy — kept for backward compatibility; use `r#loop` for new code)
pub mod cache_warming;
pub mod capabilities_registry;
pub mod capability_signals; // BLUE41: Structured capability decision data
pub mod complexity_estimator;
pub mod context;
pub mod core_dag; // DAG-UNIFY: Unified generic DAG — prefer over dag_executor, task_graph, execution_graph
#[cfg(any(
    feature = "sub-bus-tool",
    feature = "profile-simple-server",
    feature = "profile-multi-users-server"
))]
pub mod council; // F-GAP-15
                 // DAG modules — all deprecated in favor of `core_dag`.
                 // New code should use `crate::orchestration::core_dag` directly.
#[doc = "Deprecated legacy DAG driver — use [`core_dag`] instead."]
// BLUE42 / BLUE64: Remove in next major version — use core_dag instead
// Note: #[allow(deprecated)] is intentionally NOT used here — the inner
// `#![deprecated]` on dag_driver only fires when the module is *accessed*,
// not at its declaration site.
pub mod dag_driver;
#[allow(unused_imports)]
pub use core_dag::TaskContext; // GAP-B50-05 (migrated from dag_executor to core_dag)
pub mod diagnostic_feedback;
pub mod distributed; // GAP-B52-21/22
pub mod distributed_tx; // BLUE45 item 4: Two-Phase Commit (2PC) over multiple nodes
pub mod execution_graph;
pub mod fast_path_cache; // BLUE43 Steps 11-14: Fast-path cache
pub mod flow;
pub mod flow_with_models;
pub mod fork_registry;
pub mod full_auto; // BLUE43 Step 10: Full-auto flow orchestrator
pub mod integration;

pub mod mode;
pub mod multi_agent_pipeline;
pub mod omnipotent;
pub mod orchestrator;
pub mod planner_embedding; // BLUE47 Step 7: Embedding-based task classification
pub mod planner_execution_graph; // Bridge: Planner → ExecutionGraph DAG
pub mod planner_executor;
pub mod plugin_system;
pub mod promotion_plugin;
pub mod prompt_layers;
pub mod provider_impl;
pub mod recovery; // BLUE43 Step 16: Auto recovery orchestration with escalation
pub mod roles;
pub mod scheduler;
pub mod self_evolution; // GAP-B52: Self-evolution infrastructure
pub mod session_compressor; // BLUE44: Session summary compression for memory management
pub mod session_context; // BLUE44: Key concept extraction & intelligent message retention
pub mod skill;
pub mod skill_discovery;
pub mod skill_import;
pub mod skill_market;
pub mod skills_folder;
pub mod startup_context;
pub mod task_decomposer;
pub mod task_graph;
pub mod task_graph_store;
pub mod task_router;
pub mod task_schema;
pub mod threshold_learner; // BLUE44: Dynamic threshold learning for skill matching
pub mod token_layers;
pub mod tool;
pub use tool::extended as tool_extended;
pub use tool::lock as tool_lock;
#[allow(unused_imports)]
pub use tool::native as tool_native;
pub use tool::pipeline as tool_pipeline;
pub use tool::recommender as tool_recommender;
#[allow(unused_imports)]
pub use tool::transaction as tool_transaction;
pub mod workflow_optimizer;
pub mod workflow_registry;

// Suppress dead-code warnings for not-yet-integrated modules.
// These modules are publicly exported and will be fully wired in upcoming integrations.
#[cfg(test)]
mod integration_gate {
    // These imports ensure the module-level types are reachable by the test harness
    // and prevent spurious dead_code warnings.
    use super::*;
    fn _gate_session_context() {
        let _ = session_context::ContextWindowBudget::default();
    }
    fn _gate_cache_warming() {
        let _ = cache_warming::PreWarmConfig::default();
    }
    fn _gate_complexity_estimator() {
        let _ = complexity_estimator::ComplexityEstimator::default();
    }
    fn _gate_plugin_system() {
        let _ = plugin_system::PluginRegistry::default();
    }
    fn _gate_diagnostic_feedback() {
        let _ = diagnostic_feedback::DiagnosticFeedbackEngine::default();
    }
    fn _gate_sse_optimizer() {
        let _ = crate::agents::sse_optimizer::SseBufferPool::new(4, 1024);
    }
}
