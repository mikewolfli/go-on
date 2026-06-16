//! Distributed DAG End-to-End
//!
//! Tests the core DAG data structure used by orchestration for task
//! graph construction, topological ordering, and node dependency
//! resolution. These are lightweight structural tests that verify
//! the DAG's basic invariants.

use go_on::orchestration::core_dag::CoreDag;

/// Verify that an empty DAG can be created and topological sort works.
#[test]
fn test_empty_dag_topological_sort() {
    let dag = CoreDag::<String>::new();
    let sorted = dag.topological_sort();
    assert!(sorted.is_ok(), "empty DAG must sort without error");
    assert!(
        sorted.unwrap().is_empty(),
        "sorted result of empty DAG must be empty"
    );
}

/// Verify that a single node produces a valid topological ordering.
#[test]
fn test_single_node_dag() {
    let mut dag = CoreDag::<String>::new();
    dag.add_node("task-1".to_string(), "Run build".to_string(), vec![]);

    let sorted = dag
        .topological_sort()
        .expect("topological sort must succeed");
    assert_eq!(sorted, vec!["task-1"], "single node must appear in sort");
}

/// Verify that a linear graph (A → B → C) produces a valid
/// topological ordering (A has dep on B, B has dep on C).
#[test]
fn test_linear_dag_topological_order() {
    let mut dag = CoreDag::<String>::new();
    // A depends on B, B depends on C → linear order C → B → A
    dag.add_node("A".to_string(), "First".to_string(), vec!["B".to_string()]);
    dag.add_node("B".to_string(), "Second".to_string(), vec!["C".to_string()]);
    dag.add_node("C".to_string(), "Third".to_string(), vec![]);

    let sorted = dag
        .topological_sort()
        .expect("topological sort must succeed");
    assert_eq!(sorted.len(), 3, "all 3 nodes must be in sort");
    // C has no deps → first, B depends on C → second, A depends on B → third
    assert_eq!(sorted[0], "C", "C must come first (no dependencies)");
    assert_eq!(sorted[1], "B", "B must come second (depends on C)");
    assert_eq!(sorted[2], "A", "A must come third (depends on B)");
}

/// Verify that a cycle is detected during topological sort.
#[test]
fn test_dag_detects_cycles() {
    let mut dag = CoreDag::<String>::new();
    // A depends on B, B depends on A → cycle
    dag.add_node("A".to_string(), "Node A".to_string(), vec!["B".to_string()]);
    dag.add_node("B".to_string(), "Node B".to_string(), vec!["A".to_string()]);

    let sorted = dag.topological_sort();
    assert!(sorted.is_err(), "cycle must be detected");
    assert!(
        sorted.unwrap_err().contains("cycle"),
        "error message must mention cycle"
    );
}

/// Verify that DAG metrics are computed correctly.
#[test]
fn test_dag_metrics() {
    let mut dag = CoreDag::<String>::new();
    // Root nodes (level 1)
    dag.add_node("A".to_string(), "Root A".to_string(), vec![]);
    dag.add_node("B".to_string(), "Root B".to_string(), vec![]);
    // Children of A (level 2)
    dag.add_node(
        "C".to_string(),
        "Child of A".to_string(),
        vec!["A".to_string()],
    );
    // Children of B (level 2)
    dag.add_node(
        "D".to_string(),
        "Child of B".to_string(),
        vec!["B".to_string()],
    );

    let metrics = dag.metrics();
    assert_eq!(metrics.depth, 2, "max depth must be 2");
    assert_eq!(
        metrics.width, 2,
        "max width must be 2 (two nodes at each level)"
    );
}
