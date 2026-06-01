//! Real DAG Executor — topological dependency resolution, parallel group
//! identification, node output propagation, and failure isolation.

// F-GAP-51: dead_code allowed on items below when sub-bus-tool-future is disabled

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Instant;
use tokio::sync::Notify;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::orchestration::tool::{ToolInput, ToolRegistry};

// ---------------------------------------------------------------------------
// DagNode
// ---------------------------------------------------------------------------

#[cfg_attr(not(feature = "sub-bus-tool-future"), allow(dead_code))] // F-GAP-51
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagNode {
    pub id: String,
    pub tool_name: String,
    pub input: Value,
    /// IDs of nodes that must complete before this node runs.
    pub dependencies: Vec<String>,
    /// Node output (populated after execution).
    pub output: Option<Value>,
    /// Error message if this node failed.
    pub error: Option<String>,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Chain-of-Thought context propagated from upstream nodes.
    pub context: Option<TaskContext>,
}

// ---------------------------------------------------------------------------
// TaskContext — Chain-of-Thought context propagated between DAG nodes
// ---------------------------------------------------------------------------

/// Chain-of-Thought context propagated between DAG nodes.
#[cfg_attr(not(feature = "sub-bus-tool-future"), allow(dead_code))] // F-GAP-51
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskContext {
    pub id: String,
    pub reasoning_trace: Vec<String>,
    pub intermediate_findings: HashMap<String, Value>,
    pub confidence: f64,
    pub open_questions: Vec<String>,
    pub assumptions: Vec<String>,
    pub parent_context_id: Option<String>,
}

#[cfg_attr(not(feature = "sub-bus-tool-future"), allow(dead_code))] // F-GAP-51
impl TaskContext {
    /// Create a new TaskContext with the given id.
    pub fn new(id: String) -> Self {
        Self {
            id,
            reasoning_trace: Vec::new(),
            intermediate_findings: HashMap::new(),
            confidence: 1.0,
            open_questions: Vec::new(),
            assumptions: Vec::new(),
            parent_context_id: None,
        }
    }

    /// Merge multiple parent contexts into a single child context.
    /// Generates a new UUID for the merged context's id.
    pub fn merge(parents: &[TaskContext]) -> Self {
        let mut reasoning_trace = Vec::new();
        let mut intermediate_findings = HashMap::new();
        let mut confidences_sum = 0.0;
        let mut open_questions = Vec::new();
        let mut assumptions = Vec::new();

        for parent in parents {
            reasoning_trace.extend(parent.reasoning_trace.clone());
            intermediate_findings.extend(parent.intermediate_findings.clone());
            confidences_sum += parent.confidence;
            open_questions.extend(parent.open_questions.clone());
            assumptions.extend(parent.assumptions.clone());
        }

        let parent_context_id = parents.first().map(|p| p.id.clone());

        Self {
            id: Uuid::new_v4().to_string(),
            reasoning_trace,
            intermediate_findings,
            confidence: if parents.is_empty() {
                1.0
            } else {
                confidences_sum / parents.len() as f64
            },
            open_questions,
            assumptions,
            parent_context_id,
        }
    }
}

// ---------------------------------------------------------------------------
// DagGraph
// ---------------------------------------------------------------------------

#[cfg_attr(not(feature = "sub-bus-tool-future"), allow(dead_code))] // F-GAP-51
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagGraph {
    pub nodes: HashMap<String, DagNode>,
    pub entry_points: Vec<String>,
    pub width: usize,
    pub depth: usize,
}

#[cfg_attr(not(feature = "sub-bus-tool-future"), allow(dead_code))] // F-GAP-51
impl DagGraph {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            entry_points: Vec::new(),
            width: 0,
            depth: 0,
        }
    }

    /// Add a node with explicit dependencies.
    ///
    /// Automatically maintains `entry_points`: a node is an entry point
    /// if it has no dependencies AND no other node depends on it.
    pub fn add_node(
        &mut self,
        id: String,
        tool_name: String,
        input: Value,
        dependencies: Vec<String>,
    ) {
        // Remove this node from entry_points if it was previously registered
        self.entry_points.retain(|e| e != &id);

        // Remove any dependency that was an entry point — it's no longer one
        for dep in &dependencies {
            self.entry_points.retain(|e| e != dep);
        }

        // If this node has no dependencies, it's an entry point
        if dependencies.is_empty() {
            self.entry_points.push(id.clone());
        }

        self.nodes.insert(
            id.clone(),
            DagNode {
                id: id.clone(),
                tool_name,
                input,
                dependencies,
                output: None,
                error: None,
                duration_ms: 0,
                context: None,
            },
        );
    }

    /// Compute topological order. Returns an error if a cycle is detected.
    pub fn topological_sort(&mut self) -> Result<Vec<Vec<String>>, String> {
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();

        for (id, node) in &self.nodes {
            in_degree.entry(id.as_str()).or_insert(0);
            adjacency.entry(id.as_str()).or_default();
            for dep in &node.dependencies {
                adjacency.entry(dep.as_str()).or_default().push(id.as_str());
                *in_degree.entry(id.as_str()).or_insert(0) += 1;
            }
        }

        // Kahn's algorithm: find levels (parallel groups)
        let mut queue: VecDeque<&str> = in_degree
            .iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(&id, _)| id)
            .collect();

        if queue.is_empty() && !self.nodes.is_empty() {
            return Err("Cycle detected: no entry points in DAG".to_string());
        }

        let mut levels: Vec<Vec<String>> = Vec::new();
        let mut visited = 0usize;

        while !queue.is_empty() {
            let level_size = queue.len();
            let mut level: Vec<String> = Vec::with_capacity(level_size);

            for _ in 0..level_size {
                let node_id = queue
                    .pop_front()
                    .expect("queue non-empty: guarded by level_size loop bound");
                level.push(node_id.to_string());
                visited += 1;

                if let Some(neighbors) = adjacency.get(node_id) {
                    for neighbor in neighbors {
                        let deg = in_degree
                            .get_mut(neighbor)
                            .expect("in_degree invariant: every adjacency neighbor has an entry");
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back(neighbor);
                        }
                    }
                }
            }
            levels.push(level);
        }

        if visited != self.nodes.len() {
            return Err(format!(
                "Cycle detected: only {}/{} nodes reachable",
                visited,
                self.nodes.len()
            ));
        }

        self.width = levels.iter().map(|l| l.len()).max().unwrap_or(0);
        self.depth = levels.len();
        Ok(levels)
    }

    /// Get the output of a specific node (for dependency injection).
    pub fn get_output(&self, node_id: &str) -> Option<&Value> {
        self.nodes.get(node_id).and_then(|n| n.output.as_ref())
    }
}

#[cfg_attr(not(feature = "sub-bus-tool-future"), allow(dead_code))] // F-GAP-51
impl Default for DagGraph {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// DagExecutorConfig
// ---------------------------------------------------------------------------

/// Configuration for the DAG executor.
#[cfg_attr(not(feature = "sub-bus-tool-future"), allow(dead_code))] // F-GAP-51
#[derive(Debug, Clone)]
pub struct DagExecutorConfig {
    pub max_concurrency: usize,
    pub speculative_execution: bool,
}

#[cfg_attr(not(feature = "sub-bus-tool-future"), allow(dead_code))] // F-GAP-51
impl Default for DagExecutorConfig {
    fn default() -> Self {
        Self {
            max_concurrency: 10,
            speculative_execution: true,
        }
    }
}

// ---------------------------------------------------------------------------
// DagExecutor
// ---------------------------------------------------------------------------

#[cfg_attr(not(feature = "sub-bus-tool-future"), allow(dead_code))] // F-GAP-51
pub struct DagExecutor {
    config: DagExecutorConfig,
    semaphore: Arc<Semaphore>,
    tool_registry: Option<Arc<ToolRegistry>>,
}

#[cfg_attr(not(feature = "sub-bus-tool-future"), allow(dead_code))] // F-GAP-51
impl DagExecutor {
    pub fn new(max_concurrency: usize) -> Self {
        Self {
            config: DagExecutorConfig {
                max_concurrency,
                speculative_execution: true,
            },
            semaphore: Arc::new(Semaphore::new(max_concurrency)),
            tool_registry: None,
        }
    }

    pub fn with_config(config: DagExecutorConfig) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(config.max_concurrency)),
            config,
            tool_registry: None,
        }
    }

    pub fn with_tool_registry(mut self, registry: Arc<ToolRegistry>) -> Self {
        self.tool_registry = Some(registry);
        self
    }

    /// Execute a DAG graph. Returns the graph with outputs and contexts populated.
    /// Dispatches to speculative or level-by-level execution based on config.
    pub async fn execute(&self, graph: &mut DagGraph) {
        // First, detect cycles via topological sort
        if let Err(e) = graph.topological_sort() {
            warn!("DAG execution failed: {}", e);
            for node in graph.nodes.values_mut() {
                node.error = Some(format!("DAG error: {}", e));
            }
            return;
        }

        if self.config.speculative_execution {
            self.execute_speculative(graph).await;
        } else {
            self.execute_level_by_level(graph).await;
        }
    }

    /// Speculative (dataflow) execution: a node starts as soon as all its
    /// dependencies have completed, without waiting for the rest of its level.
    async fn execute_speculative(&self, graph: &mut DagGraph) {
        let node_ids: Vec<String> = graph.nodes.keys().cloned().collect();
        let total_nodes = node_ids.len();

        if total_nodes == 0 {
            info!("DAG execution: empty graph — nothing to do");
            return;
        }

        info!(
            "DAG speculative execution: {} nodes, max_concurrency={}",
            total_nodes, self.config.max_concurrency
        );

        // Shared state for tracking completion
        let completed: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
        let notify = Arc::new(Notify::new());

        // Shared map for propagating outputs between speculatively executed nodes
        let shared_outputs: Arc<Mutex<HashMap<String, Value>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // Pre-compute dependency lists for each node (as owned strings)
        let deps_map: HashMap<String, Vec<String>> = graph
            .nodes
            .iter()
            .map(|(id, n)| (id.clone(), n.dependencies.clone()))
            .collect();

        let mut handles = Vec::with_capacity(total_nodes);

        for node_id in &node_ids {
            let id = node_id.clone();
            let deps = deps_map.get(&id).cloned().unwrap_or_default();
            let completed_clone = completed.clone();
            let notify_clone = notify.clone();
            let shared_outputs_clone = shared_outputs.clone();
            let input = graph.nodes[&id].input.clone();
            let tool_name = graph.nodes[&id].tool_name.clone();
            let registry = self.tool_registry.clone();
            let semaphore = self.semaphore.clone();

            handles.push(tokio::spawn(async move {
                // Wait until all dependencies are completed
                loop {
                    {
                        let completed_set = completed_clone.lock().unwrap();
                        let all_deps_met = deps.iter().all(|d| completed_set.contains(d));
                        if all_deps_met {
                            break;
                        }
                    }
                    notify_clone.notified().await;
                }

                let _permit = semaphore.acquire_owned().await;
                let start = Instant::now();

                // Build dependency outputs from shared state (actual propagated outputs)
                let dep_outputs = {
                    let outputs = shared_outputs_clone.lock().unwrap();
                    deps.iter()
                        .filter_map(|dep_id| {
                            outputs.get(dep_id).map(|o| (dep_id.clone(), o.clone()))
                        })
                        .collect::<HashMap<String, Value>>()
                };
                let result = {
                    use futures_util::future::FutureExt;
                    let fut = execute_tool(registry.as_deref(), &tool_name, &input, &dep_outputs);
                    match std::panic::AssertUnwindSafe(fut).catch_unwind().await {
                        Ok(Ok(val)) => Ok(val),
                        Ok(Err(e)) => Err(e),
                        Err(panic) => {
                            let msg = panic
                                .downcast_ref::<&str>()
                                .copied()
                                .or_else(|| panic.downcast_ref::<String>().map(|s| s.as_str()))
                                .unwrap_or("unknown panic")
                                .to_string();
                            Err(format!("task panicked: {}", msg))
                        }
                    }
                };

                let duration_ms = start.elapsed().as_millis() as u64;

                // Propagate output to dependent nodes
                if let Ok(ref output) = result {
                    shared_outputs_clone
                        .lock()
                        .unwrap()
                        .insert(id.clone(), output.clone());
                }

                // Mark this node as completed and notify waiters
                {
                    let mut completed_set = completed_clone.lock().unwrap();
                    completed_set.insert(id.clone());
                }
                notify_clone.notify_one();

                (id, result, duration_ms)
            }));
        }

        // Collect results and update the graph
        for handle in handles {
            match handle.await {
                Ok((id, result, duration_ms)) => {
                    if let Some(node) = graph.nodes.get_mut(&id) {
                        node.duration_ms = duration_ms;
                        match result {
                            Ok(output) => {
                                node.output = Some(output);
                                // Build TaskContext from the node's output
                                let mut ctx = TaskContext::new(id.clone());
                                ctx.intermediate_findings.insert(
                                    "output".to_string(),
                                    node.output.clone().unwrap_or(Value::Null),
                                );
                                ctx.reasoning_trace.push(format!(
                                    "Executed tool '{}' ({}ms)",
                                    node.tool_name, duration_ms
                                ));
                                node.context = Some(ctx);
                            }
                            Err(e) => node.error = Some(e),
                        }
                    }
                }
                Err(e) => {
                    warn!("DAG node task panicked: {}", e);
                }
            }
        }
    }

    /// Level-by-level (barrier) execution: wait for all nodes in level N
    /// before starting level N+1. Falls back to this when speculative_execution is false.
    async fn execute_level_by_level(&self, graph: &mut DagGraph) {
        let levels = graph.topological_sort().unwrap_or_default();

        info!(
            "DAG level-by-level execution: {} levels, {} nodes, width={}",
            levels.len(),
            graph.nodes.len(),
            graph.width
        );

        for level in &levels {
            let mut handles = Vec::with_capacity(level.len());

            for node_id in level {
                let node = graph.nodes.get(node_id).expect("node exists");
                let input = node.input.clone();
                let tool_name = node.tool_name.clone();
                let id = node_id.clone();

                let node_deps: Vec<String> = node.dependencies.clone();

                // Check if any dependency has errored — if so, propagate the error
                // instead of executing this node.
                let dep_errors: Vec<String> = graph
                    .nodes
                    .iter()
                    .filter(|(dep_id, _)| node_deps.contains(dep_id))
                    .filter_map(|(dep_id, n)| n.error.clone().map(|e| (dep_id.clone(), e)))
                    .map(|(id, err)| format!("dependency '{}' failed: {}", id, err))
                    .collect();

                if !dep_errors.is_empty() {
                    if let Some(node) = graph.nodes.get_mut(&id) {
                        node.error = Some(format!("Dependency failure: {}", dep_errors.join("; ")));
                    }
                    continue;
                }

                let dep_outputs: HashMap<String, Value> = graph
                    .nodes
                    .iter()
                    .filter(|(dep_id, _)| node_deps.contains(dep_id))
                    .filter_map(|(dep_id, n)| n.output.clone().map(|o| (dep_id.clone(), o)))
                    .collect();

                let permit = self.semaphore.clone().acquire_owned().await;
                let registry = self.tool_registry.clone();
                handles.push(tokio::spawn(async move {
                    let _permit = permit;
                    let start = Instant::now();
                    let result = {
                        use futures_util::future::FutureExt;
                        let fut =
                            execute_tool(registry.as_deref(), &tool_name, &input, &dep_outputs);
                        match std::panic::AssertUnwindSafe(fut).catch_unwind().await {
                            Ok(Ok(val)) => Ok(val),
                            Ok(Err(e)) => Err(e),
                            Err(panic) => {
                                let msg = panic
                                    .downcast_ref::<&str>()
                                    .copied()
                                    .or_else(|| panic.downcast_ref::<String>().map(|s| s.as_str()))
                                    .unwrap_or("unknown panic")
                                    .to_string();
                                Err(format!("task panicked: {}", msg))
                            }
                        }
                    };
                    let duration_ms = start.elapsed().as_millis() as u64;
                    (id, result, duration_ms)
                }));
            }

            // Wait for all nodes in this level
            for (node_id, handle) in level.iter().zip(handles) {
                match handle.await {
                    Ok((id, result, duration_ms)) => {
                        // Collect dependency contexts BEFORE mutably borrowing graph.nodes.
                        // We need to know which nodes are dependencies of `id`.
                        let node_deps: Vec<String> = graph
                            .nodes
                            .get(&id)
                            .map(|n| n.dependencies.clone())
                            .unwrap_or_default();

                        let dep_contexts: Vec<TaskContext> = graph
                            .nodes
                            .iter()
                            .filter(|(dep_id, _)| node_deps.contains(dep_id))
                            .filter_map(|(_, n)| n.context.clone())
                            .collect();

                        if let Some(node) = graph.nodes.get_mut(&id) {
                            node.duration_ms = duration_ms;
                            match result {
                                Ok(output) => {
                                    node.output = Some(output);

                                    let mut ctx = if dep_contexts.is_empty() {
                                        TaskContext::new(id.clone())
                                    } else {
                                        TaskContext::merge(&dep_contexts)
                                    };
                                    ctx.intermediate_findings.insert(
                                        "output".to_string(),
                                        node.output.clone().unwrap_or(Value::Null),
                                    );
                                    ctx.reasoning_trace.push(format!(
                                        "Executed tool '{}' ({}ms)",
                                        node.tool_name, duration_ms
                                    ));
                                    node.context = Some(ctx);
                                }
                                Err(e) => node.error = Some(e),
                            }
                        }
                    }
                    Err(e) => {
                        warn!("DAG node task panicked: {}", e);
                        if let Some(node) = graph.nodes.get_mut(node_id) {
                            node.error = Some(format!("task panicked: {}", e));
                        }
                    }
                }
            }
        }
    }
}

#[cfg_attr(not(feature = "sub-bus-tool-future"), allow(dead_code))] // F-GAP-51
impl Default for DagExecutor {
    fn default() -> Self {
        Self::with_config(DagExecutorConfig::default())
    }
}

/// Execute a single tool with optional dependency outputs injected into context.
///
/// Looks up the tool in the registry (if available) and dispatches execution.
/// If no registry is provided, returns an error indicating the tool is unavailable.
/// Dependency outputs are injected into the evidence field of ToolInput.
#[cfg_attr(not(feature = "sub-bus-tool-future"), allow(dead_code))] // F-GAP-51
async fn execute_tool(
    registry: Option<&ToolRegistry>,
    tool_name: &str,
    input: &Value,
    dep_outputs: &HashMap<String, Value>,
) -> Result<Value, String> {
    let registry = match registry {
        Some(r) => r,
        None => {
            return Err(format!(
                "Tool registry not available; cannot execute '{}'",
                tool_name
            ));
        }
    };

    let tool = registry
        .get(tool_name)
        .ok_or_else(|| format!("Tool '{}' not found in registry", tool_name))?;

    debug!(
        "Executing tool: {} with dep_outputs: {:?}",
        tool_name,
        dep_outputs.keys()
    );

    // Build evidence from dependency outputs.
    let evidence = if dep_outputs.is_empty() {
        None
    } else {
        Some(
            serde_json::to_string(dep_outputs)
                .unwrap_or_else(|_| "<serialization error>".to_string()),
        )
    };

    let tool_input = ToolInput {
        task_id: String::new(),
        phase: "dag-execution".to_string(),
        agent_role: "dag-executor".to_string(),
        objective: format!("Execute tool '{}'", tool_name),
        constraints: None,
        evidence,
        payload: input.clone(),
        allowed_base_dir: None,
    };

    let output = tool
        .run_async(&tool_input)
        .await
        .map_err(|e| format!("Tool '{}' execution failed: {}", tool_name, e))?;

    if output.success {
        Ok(output.result.unwrap_or(serde_json::json!({"status": "ok"})))
    } else {
        Err(output.error.unwrap_or_else(|| {
            format!(
                "Tool '{}' returned failure without error message",
                tool_name
            )
        }))
    }
}

// ---------------------------------------------------------------------------
// Migration adapter — replaces old dag_driver.rs API
// ---------------------------------------------------------------------------

/// Build a DagGraph from the old flat tool call list.
/// This is the replacement for `build_tool_execution_dag()`.
#[cfg_attr(not(feature = "sub-bus-tool-future"), allow(dead_code))] // F-GAP-51
pub fn build_dag_from_tool_calls(tool_calls: &[(String, Value)]) -> DagGraph {
    let mut graph = DagGraph::new();
    for (i, (name, input)) in tool_calls.iter().enumerate() {
        let id = format!("tool-{}-{}", name, i);
        graph.add_node(id, name.clone(), input.clone(), Vec::new());
    }
    // Recompute topological metrics
    let _ = graph.topological_sort();
    graph
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -----------------------------------------------------------------------
    // Existing tests (unchanged)
    // -----------------------------------------------------------------------

    #[test]
    fn test_empty_graph() {
        let mut graph = DagGraph::new();
        assert!(graph.topological_sort().unwrap().is_empty());
    }

    #[test]
    fn test_single_node() {
        let mut graph = DagGraph::new();
        graph.add_node(
            "a".into(),
            "read_file".into(),
            json!({"path": "foo"}),
            vec![],
        );
        let levels = graph.topological_sort().unwrap();
        assert_eq!(levels.len(), 1);
        assert_eq!(levels[0], vec!["a"]);
    }

    #[test]
    fn test_sequential_dependency() {
        let mut graph = DagGraph::new();
        graph.add_node(
            "read".into(),
            "read_file".into(),
            json!({"path": "foo"}),
            vec![],
        );
        graph.add_node(
            "write".into(),
            "write_file".into(),
            json!({"content": "bar"}),
            vec!["read".to_string()],
        );
        let levels = graph.topological_sort().unwrap();
        assert_eq!(levels.len(), 2);
        assert_eq!(levels[0], vec!["read"]);
        assert_eq!(levels[1], vec!["write"]);
    }

    #[test]
    fn test_parallel_execution() {
        let mut graph = DagGraph::new();
        graph.add_node("a".into(), "tool_a".into(), json!({}), vec![]);
        graph.add_node("b".into(), "tool_b".into(), json!({}), vec![]);
        graph.add_node("c".into(), "tool_c".into(), json!({}), vec![]);
        let levels = graph.topological_sort().unwrap();
        assert_eq!(levels.len(), 1);
        assert_eq!(levels[0].len(), 3);
    }

    #[test]
    fn test_diamond_dependency() {
        let mut graph = DagGraph::new();
        graph.add_node("root".into(), "root".into(), json!({}), vec![]);
        graph.add_node(
            "a".into(),
            "branch_a".into(),
            json!({}),
            vec!["root".to_string()],
        );
        graph.add_node(
            "b".into(),
            "branch_b".into(),
            json!({}),
            vec!["root".to_string()],
        );
        graph.add_node(
            "merge".into(),
            "merge".into(),
            json!({}),
            vec!["a".to_string(), "b".to_string()],
        );
        let levels = graph.topological_sort().unwrap();
        assert_eq!(levels.len(), 3);
        assert_eq!(levels[0], vec!["root"]);
        assert_eq!(levels[1].len(), 2); // a and b in parallel
        assert_eq!(levels[2], vec!["merge"]);
    }

    #[test]
    fn test_cycle_detection() {
        let mut graph = DagGraph::new();
        graph.add_node("a".into(), "a".into(), json!({}), vec!["b".to_string()]);
        graph.add_node("b".into(), "b".into(), json!({}), vec!["a".to_string()]);
        assert!(graph.topological_sort().is_err());
    }

    #[test]
    fn test_build_dag_from_tool_calls() {
        let tool_calls = vec![
            ("read_file".to_string(), json!({"path": "a"})),
            ("write_file".to_string(), json!({"content": "b"})),
        ];
        let graph = build_dag_from_tool_calls(&tool_calls);
        assert_eq!(graph.nodes.len(), 2);
        assert!(graph.width > 0);
    }

    #[test]
    fn test_dag_metrics_width_depth() {
        let mut graph = DagGraph::new();
        graph.add_node("a".into(), "a".into(), json!({}), vec![]);
        graph.add_node("b".into(), "b".into(), json!({}), vec![]);
        graph.add_node(
            "c".into(),
            "c".into(),
            json!({}),
            vec!["a".to_string(), "b".to_string()],
        );
        let _levels = graph.topological_sort().unwrap();
        assert_eq!(graph.width, 2);
        assert_eq!(graph.depth, 2);
    }

    // -----------------------------------------------------------------------
    // New tests for TaskContext
    // -----------------------------------------------------------------------

    #[test]
    fn test_task_context_new() {
        let ctx = TaskContext::new("test-1".to_string());
        assert_eq!(ctx.id, "test-1");
        assert!(ctx.reasoning_trace.is_empty());
        assert!(ctx.intermediate_findings.is_empty());
        assert_eq!(ctx.confidence, 1.0);
        assert!(ctx.open_questions.is_empty());
        assert!(ctx.assumptions.is_empty());
        assert!(ctx.parent_context_id.is_none());
    }

    #[test]
    fn test_task_context_merge_empty() {
        let merged = TaskContext::merge(&[]);
        assert!(!merged.id.is_empty());
        assert_eq!(merged.confidence, 1.0);
        assert!(merged.parent_context_id.is_none());
    }

    #[test]
    fn test_task_context_merge_single() {
        let parent = TaskContext::new("parent-1".to_string());
        let merged = TaskContext::merge(&[parent]);
        assert!(!merged.id.is_empty());
        assert_eq!(merged.parent_context_id, Some("parent-1".to_string()));
        assert_eq!(merged.confidence, 1.0);
    }

    #[test]
    fn test_task_context_merge_multiple() {
        let mut p1 = TaskContext::new("p1".to_string());
        p1.confidence = 0.8;
        p1.reasoning_trace.push("step 1".to_string());
        p1.intermediate_findings
            .insert("key1".to_string(), json!("val1"));

        let mut p2 = TaskContext::new("p2".to_string());
        p2.confidence = 0.6;
        p2.reasoning_trace.push("step 2".to_string());
        p2.intermediate_findings
            .insert("key2".to_string(), json!("val2"));
        p2.open_questions.push("why?".to_string());
        p2.assumptions.push("assume X".to_string());

        let merged = TaskContext::merge(&[p1, p2]);

        assert_eq!(merged.parent_context_id, Some("p1".to_string()));
        assert!((merged.confidence - 0.7).abs() < f64::EPSILON);
        assert_eq!(merged.reasoning_trace.len(), 2);
        assert!(merged.reasoning_trace.contains(&"step 1".to_string()));
        assert!(merged.reasoning_trace.contains(&"step 2".to_string()));
        assert_eq!(merged.intermediate_findings.len(), 2);
        assert_eq!(merged.open_questions, vec!["why?"]);
        assert_eq!(merged.assumptions, vec!["assume X"]);
    }

    // -----------------------------------------------------------------------
    // New tests for DagExecutorConfig and speculative execution config
    // -----------------------------------------------------------------------

    #[test]
    fn test_dag_executor_config_default() {
        let config = DagExecutorConfig::default();
        assert_eq!(config.max_concurrency, 10);
        assert!(config.speculative_execution);
    }

    #[test]
    fn test_dag_executor_config_custom() {
        let config = DagExecutorConfig {
            max_concurrency: 5,
            speculative_execution: false,
        };
        assert_eq!(config.max_concurrency, 5);
        assert!(!config.speculative_execution);
    }

    #[test]
    fn test_dag_executor_with_config_disables_speculative() {
        let config = DagExecutorConfig {
            max_concurrency: 4,
            speculative_execution: false,
        };
        let executor = DagExecutor::with_config(config);
        assert!(!executor.config.speculative_execution);
        assert_eq!(executor.config.max_concurrency, 4);
    }

    #[test]
    fn test_dag_executor_default_has_speculative() {
        let executor = DagExecutor::default();
        assert!(executor.config.speculative_execution);
        assert_eq!(executor.config.max_concurrency, 10);
    }

    #[test]
    fn test_dag_executor_new_has_speculative() {
        let executor = DagExecutor::new(8);
        assert!(executor.config.speculative_execution);
        assert_eq!(executor.config.max_concurrency, 8);
    }

    // -----------------------------------------------------------------------
    // New tests for DagNode with context
    // -----------------------------------------------------------------------

    #[test]
    fn test_dag_node_has_context_field() {
        let mut graph = DagGraph::new();
        graph.add_node("a".into(), "tool".into(), json!({}), vec![]);
        let node = graph.nodes.get("a").unwrap();
        assert!(node.context.is_none());
        assert_eq!(node.duration_ms, 0);
    }

    #[test]
    fn test_dag_node_context_assignment() {
        let mut graph = DagGraph::new();
        graph.add_node("a".into(), "tool".into(), json!({}), vec![]);
        let node = graph.nodes.get_mut("a").unwrap();
        let ctx = TaskContext::new("ctx-1".to_string());
        node.context = Some(ctx);
        assert!(node.context.is_some());
        assert_eq!(node.context.as_ref().unwrap().id, "ctx-1");
    }
}
