pub mod artifact;
pub mod audit;
pub mod autonomy_runtime;
pub mod brain_loop; // F-GAP-17 (flat, legacy — kept for backward compatibility; use `r#loop` for new code)
pub mod bulkhead; // BLUE68 P1-7: Bulkhead pattern for LLM provider/tool executor
pub mod cache_warming;
pub mod capabilities_registry;
pub mod capability_signals; // BLUE41: Structured capability decision data
pub mod complexity_estimator;
pub mod context;
pub mod core_dag; // DAG-UNIFY: Unified generic DAG — prefer over dag_executor, task_graph, execution_graph
#[cfg(any(
    feature = "sub-bus-tool",
    feature = "simple-server",
    feature = "multi-users-server"
))]
pub mod council; // F-GAP-15
pub mod diagnostic_feedback; // F-GAP-51: Reserved for full diagnostic integration
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
pub mod distributed_tx; // BLUE45 item 4: Two-Phase Commit (2PC) over multiple nodes
pub mod fast_path_cache; // BLUE43 Steps 11-14: Fast-path cache
pub mod flow;
pub mod flow_with_models;
pub mod fork_registry;
pub mod full_auto; // BLUE43 Step 10: Full-auto flow orchestrator

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
pub mod task_graph_store;
pub mod task_router;
pub mod task_schema;
pub mod threshold_learner; // BLUE44: Dynamic threshold learning for skill matching
pub mod token_layers;
pub mod tool;
pub use tool::extended as tool_extended;
pub use tool::lock as tool_lock;
pub use tool::pipeline as tool_pipeline;
pub use tool::recommender as tool_recommender;
#[allow(unused_imports)]
pub use tool::transaction as tool_transaction;
pub mod workflow_optimizer;
pub mod workflow_registry;
