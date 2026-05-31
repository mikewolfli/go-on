pub mod artifact;
pub mod audit;
pub mod autonomy_runtime;
pub mod brain_loop; // F-GAP-17 (flat, legacy — kept for backward compatibility; use `r#loop` for new code)
pub mod cache_warming;
pub mod capabilities_registry;
pub mod capability_signals; // BLUE41: Structured capability decision data
pub mod complexity_estimator;
pub mod context;
#[cfg(any(
    feature = "sub-bus-tool",
    feature = "profile-simple-server",
    feature = "profile-multi-users-server"
))]
pub mod council; // F-GAP-15
pub mod dag_driver; // BLUE42: DAG execution driver for autonomy loop
pub mod dag_execution; // AUTON-07: DAG-driven execution adapter
pub mod dag_executor; // GAP-46-02: Real topological DAG executor
#[allow(unused_imports)]
pub use dag_executor::TaskContext; // GAP-B50-05: TaskContext re-export for chain-of-thought propagation
pub mod diagnostic_feedback;
pub mod distributed_tx; // BLUE45 item 4: Two-Phase Commit (2PC) over multiple nodes
pub mod execution_graph;
pub mod fast_path_cache; // BLUE43 Steps 11-14: Fast-path cache
pub mod flow;
pub mod flow_with_models;
pub mod fork_registry;
pub mod full_auto; // BLUE43 Step 10: Full-auto flow orchestrator
pub mod integration;
pub mod r#loop; // F-GAP-17 (structured sub-module — replaces brain_loop; new code should use this)
pub mod mode;
pub mod omnipotent;
pub mod orchestrator;
pub mod planner_embedding; // BLUE47 Step 7: Embedding-based task classification
pub mod planner_execution_graph; // Bridge: Planner → ExecutionGraph DAG
pub mod planner_executor;
pub mod plugin_system;
pub mod promotion_plugin;
pub mod prompt_layers;
pub mod recovery; // BLUE43 Step 16: Auto recovery orchestration with escalation
pub mod roles;
pub mod scheduler;
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
pub mod tool_extended; // BLUE44: Extended built-in tools (10 additional tools)
pub mod tool_lock; // BLUE44: Tool lock manager for concurrent file access
pub mod tool_native; // BLUE44: Native function call bridge for OpenAI/Anthropic
pub mod tool_pipeline; // BLUE44: Tool pipeline for composing multi-step workflows
pub mod tool_recommender; // BLUE44: Dynamic tool recommendation engine
pub mod tool_transaction; // BLUE43 Step 15: Tool call transactional semantics
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
