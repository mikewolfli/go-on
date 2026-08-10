# Advanced Orchestration

The `go-on` runtime includes a suite of advanced orchestration modules that manage complex workflow execution, recovery, and diagnostics. These components handle dependency-aware scheduling, fault-tolerant recovery, session context management, and resilient fault handling.

---

## 1. DAG Execution Engine

The DAG (Directed Acyclic Graph) Execution Engine is responsible for scheduling and running tasks that have explicit dependency relationships. Tasks are organized as a graph where edges represent data or control dependencies.

### Topological Execution

The engine computes a topological ordering of all nodes in the DAG before execution begins using Kahn's algorithm. Nodes with no dependencies (root nodes) are scheduled first, and a node is only dispatched once all its predecessors have completed successfully.

The `ExecutionGraph` type (`src/orchestration/core_dag.rs`) provides the DAG implementation: `add_node()` / `add_edge()` build the graph, `get_ready_nodes()` returns the nodes whose dependencies are all satisfied, and `set_node_state()` records a node's completion or failure. Execution advances level by level as ready nodes complete.

### Dependency Management

Each task declares its dependencies as a set of upstream task IDs. The engine validates that:

- All referenced dependencies exist in the DAG.
- No circular dependencies are present (see Cycle Detection below).
- Required outputs from dependencies match the declared input types of downstream tasks.

### Parallel Groups

Tasks at the same topological level (i.e., tasks whose dependencies are all satisfied and which are not dependent on each other) are grouped into parallel execution batches. The engine dispatches all tasks in a batch concurrently, subject to configurable concurrency limits.

```
Level 0: [A]           ← no dependencies
Level 1: [B, C]        ← depend on A, run in parallel
Level 2: [D]           ← depends on B and C
Level 3: [E, F, G]     ← depend on D, run in parallel
```

### Cycle Detection

The engine performs cycle detection using Kahn's topological sort. If the sorted result contains fewer nodes than the total, a cycle exists. The DAG execution falls back to flat parallel fan-out when a cycle is detected.

---

## 2. Recovery Orchestrator

The recovery orchestrator (`src/fault_tolerance/`) handles node-level faults and determines the best recovery actions based on the detected fault type and severity. It is a cluster-health oriented engine: nodes register heartbeats, faults are recorded as `FaultEvent`s, and a `FaultToleranceEngine` turns them into executable recovery plans.

### Recovery Actions

There are exactly five `RecoveryAction` variants:

| Action            | Description |
|-------------------|-------------|
| **RestartNode**       | Restart the failed node: the heartbeat record returns to `Online` and missed-beat counters are cleared. |
| **FailoverToBackup**  | Fail traffic over to a backup node under the new leader. |
| **ScaleUp**           | Record a scale-up action for capacity-related faults (hang, resource exhaustion, rate limiting, latency spikes). |
| **Rebalance**         | Rebalance the node, resolving corruption-type faults. |
| **NotifyOperator**    | Surface a notification for operator attention; used for faults that cannot be resolved automatically (I/O errors, auth failures, partial writes) and always for severity ≥ 9. |

### Fault-Type Mapping

`create_recovery_plan()` derives the action set from the active `FaultType`s of a node, deduplicated per fault class:

| FaultType                                   | RecoveryAction          |
|---------------------------------------------|-------------------------|
| `Crash`, `Oom`, `ProcessCrash`              | `RestartNode`           |
| `Hang`, `ResourceExhaustion`                | `ScaleUp`               |
| `NetworkSplit`, `NetworkTimeout`, `NetworkPartition` | `FailoverToBackup` |
| `RateLimit`, `LatencySpike`                 | `ScaleUp`               |
| `DataCorruption`                            | `Rebalance`             |
| `FileIOError`, `AuthFailure`, `PartialWrite`| `NotifyOperator`        |

High-severity faults (`severity >= 9`) always append `NotifyOperator`; a node with no specific action mapping defaults to `NotifyOperator`.

### Plan Lifecycle

The orchestrator dispatches each plan through `execute_recovery_plan()`, which performs the observable in-process effect of every action (state resets, isolation-group changes, fault resolution, operator notification) and then runs a post-recovery consistency check. A plan only completes or fails once that check passes — automatic recovery either genuinely resolves the fault or reports the residual state.

---

## 3. Session Context Manager

The Session Context Manager maintains and optimizes the conversational context across multi-turn interactions, ensuring that the most relevant information is retained within a limited window budget.

### Concept Extraction

Each user message is analyzed to extract key concepts — entities, intents, and domain-specific terms. Extracted concepts are stored in a lightweight in-memory index associated with the session.

```rust
struct ExtractedConcept {
    term: String,
    weight: f64,
    first_seen: Timestamp,
    last_seen: Timestamp,
    frequency: u32,
}
```

Concepts decay over time if not reinforced by subsequent messages. A concept with a weight below a configurable threshold is pruned from the session.

### Message Importance Scoring

Every message in the session history is scored on several dimensions:

- **Recency**: More recent messages receive higher scores.
- **Relevance**: Messages containing concepts that appear in the current query score higher.
- **Role**: System messages and explicit user directives score higher than casual conversation.
- **Actionability**: Messages that triggered a tool invocation or produced an observable side effect score higher.

The scoring function is a weighted sum of these dimensions:

```rust
fn importance_score(msg: &Message, current_query: &str) -> f64 {
    let recency = decay_weight(msg.timestamp);
    let relevance = concept_overlap(msg.concepts, current_query);
    let role_weight = role_boost(msg.role);
    let action_weight = action_boost(msg.tool_calls);
    recency * 0.3 + relevance * 0.4 + role_weight * 0.2 + action_weight * 0.1
}
```

### Continuity Markers

Continuity markers are special annotations injected into the context window that signal narrative progression. They help the model maintain coherence across turns by summarizing what has been accomplished and what remains. Markers are automatically generated at key milestones:

- After a workflow run (`workflow.execute` / `task.execute`) completes.
- After a recovery action resolves a fault.

### Window Budget

The context window is finite. The Session Context Manager enforces a budget by:

1. Always retaining the most recent `n` messages (configurable, default: 5).
2. Filling the remaining budget with the highest-scoring older messages.
3. Stripping low-importance messages to make room when the budget is exceeded.
4. Compressing long messages by summarizing them when necessary.


