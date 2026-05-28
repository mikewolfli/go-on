//! Real DAG Executor — topological dependency resolution, parallel group
//! identification, node output propagation, and failure isolation.

#![cfg_attr(not(feature = "sub-bus-tool-future"), allow(dead_code, unused_imports))]

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

use crate::orchestration::tool::{ToolInput, ToolRegistry};

// ---------------------------------------------------------------------------
// DagNode
// ---------------------------------------------------------------------------

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
}

// ---------------------------------------------------------------------------
// DagGraph
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagGraph {
    pub nodes: HashMap<String, DagNode>,
    pub entry_points: Vec<String>,
    pub width: usize,
    pub depth: usize,
}

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
                let node_id = queue.pop_front().unwrap();
                level.push(node_id.to_string());
                visited += 1;

                if let Some(neighbors) = adjacency.get(node_id) {
                    for neighbor in neighbors {
                        let deg = in_degree.get_mut(neighbor).unwrap();
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

impl Default for DagGraph {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// DagExecutor
// ---------------------------------------------------------------------------

pub struct DagExecutor {
    max_concurrency: usize,
    semaphore: Arc<Semaphore>,
    tool_registry: Option<Arc<ToolRegistry>>,
}

impl DagExecutor {
    pub fn new(max_concurrency: usize) -> Self {
        Self {
            max_concurrency,
            semaphore: Arc::new(Semaphore::new(max_concurrency)),
            tool_registry: None,
        }
    }

    pub fn with_tool_registry(mut self, registry: Arc<ToolRegistry>) -> Self {
        self.tool_registry = Some(registry);
        self
    }

    /// Execute a DAG graph. Returns the graph with outputs populated.
    pub async fn execute(&self, graph: &mut DagGraph) {
        let levels = match graph.topological_sort() {
            Ok(levels) => levels,
            Err(e) => {
                warn!("DAG execution failed: {}", e);
                // Mark all nodes as failed
                for node in graph.nodes.values_mut() {
                    node.error = Some(format!("DAG error: {}", e));
                }
                return;
            }
        };

        info!(
            "DAG execution: {} levels, {} nodes, width={}",
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

                // Collect dependency outputs — iterate over THIS node's dependencies
                // to gather outputs from its actual upstream nodes.
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
            // Use zip to identify which node panicked if the task itself failed.
            for (node_id, handle) in level.iter().zip(handles) {
                match handle.await {
                    Ok((id, result, duration_ms)) => {
                        if let Some(node) = graph.nodes.get_mut(&id) {
                            node.duration_ms = duration_ms;
                            match result {
                                Ok(output) => node.output = Some(output),
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

impl Default for DagExecutor {
    fn default() -> Self {
        Self::new(10)
    }
}

/// Execute a single tool with optional dependency outputs injected into context.
///
/// Looks up the tool in the registry (if available) and dispatches execution.
/// If no registry is provided, returns an error indicating the tool is unavailable.
/// Dependency outputs are injected into the evidence field of ToolInput.
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
}
