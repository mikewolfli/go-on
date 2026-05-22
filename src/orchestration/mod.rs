pub mod artifact;
pub mod autonomy_runtime;
pub mod brain_loop; // F-GAP-17 (flat, legacy)
pub mod capability_signals; // BLUE41: Structured capability decision data
#[cfg(any(
    feature = "sub-bus-tool",
    feature = "profile-simple-server",
    feature = "profile-multi-users-server"
))]
pub mod council; // F-GAP-15
pub mod dag_driver; // BLUE42: DAG execution driver for autonomy loop
pub mod dag_execution; // AUTON-07: DAG-driven execution adapter
pub mod execution_graph;
pub mod flow;
pub mod flow_with_models;
pub mod fork_registry;
pub mod r#loop; // F-GAP-17 (structured sub-module)
pub mod mode;
pub mod omnipotent;
pub mod orchestrator;
pub mod planner_execution_graph; // Bridge: Planner → ExecutionGraph DAG
pub mod planner_executor;
pub mod promotion_plugin;
pub mod prompt_layers;
pub mod roles;
pub mod scheduler;
pub mod skill;
pub mod skill_import;
pub mod startup_context;
pub mod task_decomposer;
pub mod task_graph;
pub mod task_graph_store;
pub mod task_router;
pub mod task_schema;
pub mod token_layers;
pub mod tool;
pub mod workflow_optimizer;
pub mod workflow_registry;
