# Advanced Orchestration

The `go-on` runtime includes a suite of advanced orchestration modules that manage complex workflow execution, caching, recovery, and diagnostics. These components form the backbone of the FullAutoFlow system and enable sophisticated multi-agent task decomposition, dependency-aware scheduling, and resilient fault handling.

---

## 1. DAG Execution Engine

The DAG (Directed Acyclic Graph) Execution Engine is responsible for scheduling and running tasks that have explicit dependency relationships. Tasks are organized as a graph where edges represent data or control dependencies.

### Topological Execution

The engine computes a topological ordering of all nodes in the DAG before execution begins using Kahn's algorithm. Nodes with no dependencies (root nodes) are scheduled first, and a node is only dispatched once all its predecessors have completed successfully.

The `CoreDag<T>` type provides the generic DAG implementation. For tool execution, `execute_tool_dag()` auto-selects between flat parallel fan-out and plan-respecting level-by-level execution based on whether an `ExecutionPlan` is provided.

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

## 2. FullAutoFlow

FullAutoFlow is a fully autonomous 5-stage pipeline that takes a high-level user goal and produces a completed result with minimal human intervention.

### Five-Stage Pipeline

| Stage       | Description |
|-------------|-------------|
| **Parse**       | The incoming user request is parsed to extract intent, constraints, and success criteria. Natural language is converted into a structured goal specification. |
| **Discover**    | The system discovers relevant skills, tools, and context from the available registries. Skills are ranked by relevance to the parsed goals. |
| **Prepare**     | A task plan is generated, dependencies are resolved, and the execution DAG is constructed. Required inputs are gathered and validated. |
| **Execute**     | The DAG Execution Engine runs the plan, with tasks dispatched to appropriate skill handlers. Progress is monitored and intermediate results are collected. |
| **Report**      | Results from all tasks are consolidated into a final report. The report includes outputs, metrics, warnings, and a summary of decisions made. |

### Skill-Aware Task Decomposition

During the **Discover** stage, FullAutoFlow decomposes the high-level goal into sub-tasks using the available skill registry via `discover_skills()`. Each sub-task is matched to a skill using token-based similarity scoring (name overlap 35%, description overlap 40%, runtime success rate 25%). Results are cached by goal text for repeated queries.

The decomposition strategy considers:

- **Token similarity**: weighted composite score of name, description, and runtime performance
- **Dynamic threshold**: `ThresholdLearner` adjusts the minimum match score based on historical outcomes
- **Fast-path cache**: SHA-256 fingerprinted routes bypass parsing/discovery for known task types
- **Input/output compatibility**: whether the output of one skill can feed into another.
- **Execution constraints**: timeout, memory, or provider requirements declared by the skill.

The result is a DAG where each node is annotated with its assigned skill, its input bindings, and expected output schema.

```yaml
# Conceptual decomposition output
pipeline:
  - stage: Parse
    result: structured_goal
  - stage: Discover
    result: [skill_a, skill_b, skill_c]
  - stage: Prepare
    tasks:
      - id: t1
        skill: skill_a
        inputs: { goal: structured_goal }
      - id: t2
        skill: skill_b
        depends_on: [t1]
        inputs: { output_a: t1.result }
      - id: t3
        skill: skill_c
        depends_on: [t1]
        inputs: { output_a: t1.result }
```

---

## 3. FastPathCache

FastPathCache provides a high-performance, multi-tier caching layer that reduces redundant computation and accelerates repeated requests.

### SHA-256 Fingerprint

Every cache entry is keyed by a SHA-256 fingerprint computed from the request's canonical representation. The fingerprint includes:

- The raw input text after normalization.
- The active skill and model identifiers.
- Relevant environment variables and provider configuration.
- Route template parameters.

```rust
fn compute_fingerprint(input: &str, context: &CacheContext) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hasher.update(&context.skill_id.to_le_bytes());
    hasher.update(&context.model_id.to_le_bytes());
    hasher.update(&context.env_hash);
    hasher.finalize().into()
}
```

### TTL / LRU Eviction

The cache supports two complementary eviction strategies:

- **TTL (Time-To-Live)**: Each entry expires after a configurable duration (default: 5 minutes). Expired entries are lazily evicted on access and eagerly evicted by a background sweep.
- **LRU (Least Recently Used)**: When the cache reaches its capacity limit (default: 10,000 entries), the least recently accessed entries are evicted first.

These strategies are combined: an entry is removed if it has expired OR if it is the LRU victim when capacity is exceeded.

### Four-Tier Caching Model

| Tier   | Scope     | Key                                      | TTL     |
|--------|-----------|------------------------------------------|---------|
| L1     | Intent    | User intent fingerprint                  | 30 s    |
| L2     | Skill     | Skill + input fingerprint                | 5 min   |
| L3     | Env       | Environment + configuration fingerprint  | 10 min  |
| L4     | Route     | Route template + parameter fingerprint   | 1 min   |

Tiers are checked in order (L1 → L2 → L3 → L4). A hit at an earlier tier short-circuits the lookup and provides faster response times.

### RouteTemplate Matching

Route-level caching (L4) uses `RouteTemplate` pattern matching to handle parameterized routes. A route template such as `/skills/{skill_id}/execute` is matched against incoming requests with concrete parameters. The cache stores results keyed by the matched template and its parameter hash, enabling reuse across different parameter values that produce identical results.

---

## 4. Tool Transaction System

The Tool Transaction System provides transactional guarantees for tool execution, ensuring that side effects are consistent even in the presence of failures.

### Idempotency

Every tool invocation includes an idempotency key derived from the DAG node ID, retry count, and a session nonce. The system records completed invocations and rejects duplicate requests with the same idempotency key, returning the previously computed result.

```rust
struct IdempotencyKey {
    node_id: Uuid,
    retry_attempt: u32,
    nonce: u64,
}
```

### Write-Ahead Log (WAL)

Before any tool mutation is applied, a WAL entry is persisted. The entry records:

- The tool identity and arguments.
- The expected state transition (pre-image and post-image).
- The idempotency key.
- A transaction status (Pending / Committed / Aborted).

If the process crashes mid-transaction, the WAL is replayed on startup to commit or roll back in-flight operations.

### Compensation Actions

Each tool may declare a compensation action — an inverse operation that undoes the tool's effect. When a transaction fails after some tools have already executed successfully, the system runs compensation actions in reverse order to restore a consistent state.

```rust
trait Compensatable {
    async fn execute(&self, ctx: &Context) -> Result<Output>;
    async fn compensate(&self, ctx: &Context, output: &Output) -> Result<()>;
}
```

### TwoPhaseCoordinator (2PC)

The `TwoPhaseCoordinator` implements a distributed two-phase commit protocol for transactions that span multiple tools or providers:

1. **Prepare Phase**: The coordinator sends a prepare request to all participants. Each participant validates that it can commit and locks the necessary resources, then responds with a vote (Yes / No / ReadOnly).
2. **Commit Phase**: If all participants voted Yes, the coordinator sends a commit request. If any participant voted No, the coordinator sends an abort request to all participants.

The coordinator maintains a transaction log and can recover from failures by consulting the persisted state.

---

## 5. Recovery Orchestrator

The Recovery Orchestrator is responsible for handling task failures and determining the best recovery action based on the failure context, task metadata, and historical outcomes.

### Six Recovery Strategies

| Strategy    | Description |
|-------------|-------------|
| **Retry**       | Re-execute the failed task, optionally with a backoff delay and an incremented retry counter. Suitable for transient failures (network timeouts, rate limits). |
| **Reroute**     | Route the task to an alternative provider or skill handler. Used when the primary provider returns a provider-level error. |
| **Replan**      | Re-run the Prepare stage to generate a new plan that avoids the failing task. Used when the failure indicates a planning error. |
| **Repair**      | Apply a targeted repair (e.g., patch a malformed input, fix a schema mismatch) and retry. Repair actions are drawn from the Diagnostic Feedback Engine. |
| **Escalate**    | Hand the failed task to a human operator or a higher-authority agent. The escalation includes the full failure context and a diagnostic summary. |
| **Degrade**     | Drop the failed task and continue execution with reduced functionality. The final report notes the degradation and its impact. |

### Strategy Tree

Strategies are organized into a decision tree that the orchestrator traverses based on failure characteristics:

```
                    ┌──────────────────┐
                    │  Task Failed      │
                    └────────┬─────────┘
                             │
                    ┌────────v─────────┐
                    │ Is transient?     │
                    └────────┬─────────┘
                         Yes │    No
                     ┌───────v────┐  ┌────────v─────────┐
                     │   Retry    │  │ Is provider error? │
                     └───────┬────┘  └────────┬─────────┘
                         continue         Yes │    No
                                          ┌────v─────┐  ┌────────v────────┐
                                          │ Reroute   │  │ Is planning OK? │
                                          └────┬─────┘  └────────┬────────┘
                                              continue        Yes │    No
                                                              ┌───v────┐  ┌────v─────┐
                                                              │ Repair  │  │ Replan   │
                                                              └───┬────┘  └────┬─────┘
                                                                  │            │
                                                         If all fail ────────┘
                                                                  │
                                                          ┌───────v────────┐
                                                          │ Choose Escalate │
                                                          │  or Degrade     │
                                                          └────────────────┘
```

Each strategy node records its outcome (success / failure) in the task's execution history. If a strategy fails, the orchestrator falls through to the next applicable strategy in the tree.

---

## 6. Session Context Manager

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

- After a stage in FullAutoFlow completes.
- After a tool transaction commits.
- After a recovery action resolves a failure.

### Window Budget

The context window is finite. The Session Context Manager enforces a budget by:

1. Always retaining the most recent `n` messages (configurable, default: 5).
2. Filling the remaining budget with the highest-scoring older messages.
3. Stripping low-importance messages to make room when the budget is exceeded.
4. Compressing long messages by summarizing them when necessary.

---

## 7. Complexity Estimator

The Complexity Estimator analyzes incoming tasks to predict their computational and structural complexity, enabling the system to allocate appropriate resources and configure the BrainLoop iteration count dynamically.

### Four-Signal Analysis

The estimator evaluates four independent signals:

| Signal           | Description |
|------------------|-------------|
| **Lexical**      | Token count, vocabulary diversity, and structural markers (e.g., code blocks, bullet lists). Longer, more diverse inputs indicate higher complexity. |
| **Semantic**     | Number of distinct concepts extracted from the input. More concepts imply broader scope and higher complexity. |
| **Dependency**   | Number of implicit task dependencies inferred from the input (e.g., "first do X, then Y"). More dependencies suggest multi-step workflows. |
| **Historical**   | Average completion time and failure rate for similar past requests. Higher variance correlates with uncertainty and complexity. |

Each signal produces a normalized score (0.0 – 1.0). The final complexity score is a weighted combination:

```rust
let complexity = lexical * 0.25 + semantic * 0.30 + dependency * 0.25 + historical * 0.20;
```

### Dynamic BrainLoop Iteration Sizing

The BrainLoop iteration count — the number of reasoning passes the system performs before producing an answer — is scaled based on the complexity score:

| Complexity Range | BrainLoop Iterations |
|------------------|---------------------|
| 0.0 – 0.3        | 1 (fast path)       |
| 0.3 – 0.6        | 2–3                 |
| 0.6 – 0.8        | 4–5                 |
| 0.8 – 1.0        | 6–8 (deep analysis) |

The mapping is configurable per skill and can be overridden by the incoming request's `complexity_hint` field.

---

## 8. Diagnostic Feedback Engine

The Diagnostic Feedback Engine (DFE) provides automated analysis of compiler errors, runtime exceptions, and test failures. It parses error output, matches known patterns, and recommends targeted repair strategies.

### Compiler Error Parsing

The DFE includes parsers for common compiler and interpreter outputs:

- **Rust** (`rustc`): Extracts error codes (e.g., `E0308`), spanned locations, and type mismatch details.
- **TypeScript / JavaScript** (`tsc`, `node`): Extracts stack traces, syntax error locations, and type errors.
- **Python**: Extracts traceback entries, exception types, and line numbers.
- **Generic**: Falls back to line-column extraction and message deduplication for unhandled formats.

```rust
enum DiagnosticSource {
    Rustc(RustcDiagnostic),
    TypeScript(TsDiagnostic),
    Python(PyDiagnostic),
    Generic { message: String, line: u32, column: u32 },
}
```

### Pattern Matching

Parsed diagnostics are matched against a library of known error patterns. Each pattern includes:

- A regex or structural matcher against the diagnostic's fields.
- A severity classification (Info / Warning / Error / Critical).
- Zero or more candidate repair strategies.

Patterns are loaded from the skill registry and can be extended by custom skills. The matching engine uses a trie-based data structure for efficient lookup.

```rust
struct ErrorPattern {
    id: String,
    matcher: PatternMatcher,
    severity: Severity,
    strategies: Vec<RepairStrategy>,
}
```

### Repair Strategy Recommendation

Based on the matched pattern, the DFE recommends one or more repair strategies:

| Strategy         | Description |
|------------------|-------------|
| **Type Fix**         | Suggest a type annotation change or type cast. |
| **Import Fix**       | Suggest adding a missing import or correcting a module path. |
| **Syntax Fix**       | Suggest a syntax correction (e.g., missing semicolon, unmatched bracket). |
| **Signature Fix**    | Suggest aligning function or method signatures with their call sites. |
| **Lifetime Fix**     | Suggest lifetime annotation adjustments for Rust borrow checker errors. |
| **Custom Strategy**  | A strategy defined by a skill or a user-authored patch template. |

Each recommendation includes:

- A human-readable explanation of the error.
- The concrete code change suggested (as a diff or edit operation).
- A confidence score (0.0 – 1.0) based on pattern match specificity and historical repair success rate.

The DFE integrates with the Recovery Orchestrator: when a tool execution fails with a diagnostic output, the Recovery Orchestrator invokes the DFE to determine whether a **Repair** strategy is appropriate before falling through to Escalate or Degrade.
