pub mod autonomy_runtime;
pub mod brain_loop; // F-GAP-17 (flat, legacy — kept for backward compatibility; use `r#loop` for new code)
pub mod bulkhead;

pub mod plan_output; // BLUE68 P1-7: Bulkhead pattern for LLM provider/tool executor

pub mod context;
pub mod core_dag; // DAG-UNIFY: Unified generic DAG
#[cfg(any(
    feature = "sub-bus-tool",
    feature = "simple-server",
    feature = "multi-users-server"
))]
pub mod council; // F-GAP-15
pub mod flow;

pub mod mode;
pub mod multi_agent_pipeline;
pub mod orchestrator;
pub mod planner_execution_graph; // Bridge: Planner → ExecutionGraph DAG
pub mod planner_executor;
pub mod prompt_layers;
pub mod roles;
pub mod self_evolution; // GAP-B52: Self-evolution infrastructure
pub mod session_compressor; // BLUE44: Session summary compression for memory management
pub mod session_context; // BLUE44: Key concept extraction & intelligent message retention
pub mod skill;

pub mod skill_import;
pub mod skill_market;
pub mod startup_context;
pub mod task_decomposer;
pub mod task_router;
pub mod task_schema;
pub mod tool;
pub use tool::extended as tool_extended;

pub mod workflow_registry;
