# FUTURE Roadmap

This document defines a practical upgrade path for go-on as a modern coding-agent runtime.

It is written from an engineering perspective rather than a marketing perspective. The goal is not to chase every trend, but to close the specific gaps between the current implementation and the capabilities expected from state-of-the-art coding agents and multi-agent systems as of 2026.

## 1. Current Position

go-on already has a strong foundation:

- ACP / JSON-RPC transport for editor integration
- configurable phase routing
- broad provider abstraction
- fallback across agents in a phase
- rule and principle injection
- cache, vector memory, and summary memory
- runtime protections such as rate limit, concurrency caps, circuit breaker, timeout, and graceful shutdown
- metrics, Prometheus export, and runtime control RPCs
- a basic review-gate model for `full_auto`

That means go-on is already more than a thin provider proxy.

However, by current SOTA standards, it is still primarily a phase-routed single-runtime system with a fixed review gate, not yet a full agentic operating model.

## 2. Practical SOTA Definitions

The terms below are used in this roadmap with strict implementation-oriented meanings.

### 2.1 Assistant Mode

A single model turn answers a request or streams an answer.

Expected behavior:

- minimal planning
- no durable task graph
- no autonomous tool loop
- user remains the primary control point

This is the baseline chat or ask experience.

### 2.2 Tool-Using Agent Mode

A single agent can decide to use tools, inspect tool results, and continue reasoning before producing a final answer.

Expected behavior:

- iterative think-act-observe loop
- tool permission policy
- bounded retries and stop conditions
- tool result grounding

This is the minimum bar for a real coding agent.

### 2.3 Workflow Mode

Execution follows a predefined graph or state machine with explicit stages such as discover, plan, edit, test, review, and finalize.

Expected behavior:

- deterministic stage progression
- typed stage inputs and outputs
- resumable state
- policy gates between stages

This is stronger than a free-form agent loop because the system has an explicit orchestration contract.

### 2.4 Planner-Executor Mode

One component creates or updates a task plan while another component performs execution steps.

Expected behavior:

- explicit task decomposition
- plan revision after tool results
- separation between reasoning and action execution
- partial completion and resume support

This is one of the most common SOTA patterns for coding agents because it improves controllability and debuggability.

### 2.5 Reviewer-Gated Agent Mode

Execution is allowed only after one or more review passes validate the proposed change or action.

Expected behavior:

- reviewer independence
- structured approval criteria
- explicit reject / revise / approve outcomes
- timeout and degradation policy

go-on already implements an early version of this pattern.

### 2.6 Multi-Agent Collaboration

Multiple specialized agents cooperate under a shared objective, with clear contracts for delegation, context handoff, and convergence.

Expected behavior:

- role specialization such as planner, coder, tester, reviewer, researcher
- explicit handoff format
- bounded context per sub-agent
- aggregation or arbitration of results
- failure isolation per role

Important: having multiple providers available is not the same as having a multi-agent system.

### 2.7 Graph-Orchestrated Multi-Agent Runtime

The system runs agents as nodes in a graph with conditional transitions, branching, joins, retries, and recovery edges.

Expected behavior:

- task graph or DAG representation
- node-level state
- resumable execution
- deterministic replay or trace reconstruction
- policy at graph edges

This is closer to the current SOTA for complex coding and operations agents.

### 2.8 Memory-Aware Agent Runtime

The runtime uses multiple memory layers with explicit policy rather than only raw transcript replay.

Expected behavior:

- short-term conversational state
- episodic memory from past tasks
- semantic retrieval over prior artifacts
- distilled summaries and task state
- memory write policy and garbage collection

go-on already has useful primitives here, but not yet a full memory policy layer.

### 2.9 Evaluated and Observable Agent Runtime

The system is designed to be measured, replayed, and improved using traces, benchmark tasks, and policy-level metrics.

Expected behavior:

- traceable turn and tool execution
- outcome-level evaluation
- failure taxonomy
- scenario replay
- regression suite for agent behaviors

This is mandatory for production-grade agent systems.

## 3. Gap Analysis Against go-on

### 3.1 What go-on already does well

- It has a stable runtime shell.
- It already treats phases as first-class routing units.
- It already supports guarded automation, not just raw chat.
- It already includes operational protections that many agent demos skip.
- It already has memory and metrics primitives that can support a more advanced runtime.

### 3.2 What is still missing relative to SOTA

#### A. No general tool-execution loop

The current system routes requests to model providers, but it does not yet expose a first-class internal tool protocol for iterative act-observe cycles comparable to modern coding agents.

#### B. No explicit task graph

Phases exist, but execution is still largely request-scoped rather than graph-scoped. There is no durable representation of task nodes, dependencies, retries, joins, or branch outcomes.

#### C. No planner-executor separation

`ask`, `edit`, `agent`, and `full_auto` describe approval behavior, but they do not yet define different orchestration semantics at the runtime level.

#### D. Multi-agent is still narrow

Today the main multi-agent pattern is fallback routing plus review gate. That is useful, but it is not yet role-specialized collaboration with explicit handoff contracts.

#### E. Memory is retrieval-capable but not policy-driven

The project has cache, vector memory, and rolling summaries, but not yet a unified memory policy that decides what should be stored, promoted, compacted, or reused across task boundaries.

#### F. Observability is strong at runtime health level, but weak at trace semantics level

Metrics and Prometheus are in place, but there is not yet a first-class trace model for plans, tool calls, handoffs, reviewer decisions, or graph transitions.

#### G. Evaluation is missing as a product feature

There is no built-in benchmark harness for measuring task completion quality, review precision, tool loop reliability, or regression against known scenarios.

### 3.3 Operational Agent Discipline Additions

Beyond the architectural gaps listed above, modern coding-agent systems must enforce hard behavioral contracts at runtime. Here are the key patterns from Claude Code's 8-skill and 6-engineering-optimization synthesis that should be adopted:

#### Skill 1: Coordinator Anti-Lazy-Delegation Contract

Every delegation from a coordinator (or manager) agent to a worker agent must include:

- explicit task objective with acceptance criteria
- constraints on tool use, budget, and timeframe
- evidence pointers or artifact references from prior steps
- explicit failure modes and recovery signals
- confidence expectation from the worker

Lazy delegation (passing raw unstructured context and hoping the worker figures it out) guarantees failure in concurrent or unsupervised settings.

#### Skill 2: Worker Task Schema Discipline

Each agent role must have a strictly typed input schema and output schema:

- input schema defines context envelope, objective, constraints, and evidence references
- output schema defines deliverables, artifacts, confidence score, and failure signals
- schema enforcement is non-negotiable; invalid output is treated as fatal
- schemas are versioned to allow runtime negotiation

This prevents context pollution and ensures traceability.

#### Skill 3: Adversarial Verification Policy

After primary generation, run an independent verification check that:

- assumes the primary output may be wrong or incomplete
- uses deterministic checks (syntax, tests, compile) where available
- uses adversarial questioning if deterministic checks pass
- returns a structured verdict: approve, revise, reject, or insufficient-evidence

Do not use a single reviewer; use multiple independent signals.

#### Skill 4: Self-Rationalization Guard

Agents must not accept their own reasoning uncritically. Require:

- explicit assumption statements (what does the agent believe to be true?)
- evidence pointers for each claim made in the task
- auto-flag when assumptions contradict evidence
- forced re-examination if confidence < threshold or evidence is weak

This prevents agents from spinning narratives that ignore contradictory signals.

#### Skill 5: Typed Memory Classes

Do not treat all memory as one bin. Implement separate memory classes:

- transient: current turn conversation only
- episodic: this task's artifacts and decisions
- semantic: reusable facts and code patterns
- project-state: distilled project facts, architecture, style
- observation: raw tool results and terminal output

Each class has different write policy, retention, retrieval ranking, and promotion rules.

#### Skill 6: Memory Drift and Correctness Guards

Memory is a liability if not maintained. Require:

- usefulness scoring on retrieved memory (is it relevant?)
- staleness detection (is it outdated?)
- size bounds per memory class
- periodic revalidation of semantic memory against project state
- garbage collection and compaction

#### Skill 7: Startup Repository Context Loading

When an agent task begins, asynchronously load and cache:

- project README and architecture overview
- build and test commands
- recent commits and refactor patterns
- lint and style rules
- team conventions and decision history

This should happen in parallel with user interaction, not block the user.

#### Skill 8: Static and Dynamic Prompt Segmentation

Organize prompts into layers:

- layer 0: static role-based system prompt (planner, coder, reviewer, etc.)
- layer 1: static project conventions and constraints
- layer 2: static phase and mode instructions
- layer 3: dynamic task objective and context envelope
- layer 4: dynamic evidence pointers and prior decisions
- layer 5: dynamic conversation turn and new information

This structure allows reuse of static layers, reduces token waste, and makes prompts debuggable.

#### Engineering Optimization 1: Circuit-Breaker Based Fallback

Already partially present in go-on; strengthen with:

- per-provider circuit breaker state in traces
- fallback triggering logged with reason and destination
- circuit state reset policy and observability

#### Engineering Optimization 2: Tool-Execution Budgets

Bounded tool loops need enforced budgets:

- max tool calls per agent per task
- max wall-clock time per tool call
- max tokens per tool invocation
- max tokens per multi-step loop
- graceful degradation when budget exhausted

#### Engineering Optimization 3: Task-Level Audit Logging

Every agent decision must be logged:

- what decision was made (tool call, approval, rejection, etc.)
- who made it (which agent or policy)
- why (which inputs or signals drove the decision)
- outcome (what happened after)
- confidence/risk score from decision maker

#### Engineering Optimization 4: Forked Sub-Agent Isolation

When spawning child agents (for parallelism or specialization):

- each child runs in an isolated execution context
- kill signal from parent or policy timeout kills child
- child failure must not corrupt parent state
- child output is validated before merge
- resource limits (tokens, time, API calls) are per-child

#### Engineering Optimization 5: Prompt Architecture Optimization

Prompts must be structured for token efficiency and clarity:

- separate system prompt (static, reused) from task prompt (dynamic, per-turn)
- use compact formats for structured data (JSON/YAML vs. prose)
- compress task history using summaries and semantic compression
- truncate irrelevant context before model invocation
- use token budgets per layer

#### Engineering Optimization 6: Memory Promotion Pipeline

Explicit pipeline from raw output to durable knowledge:

- layer 0: model output (raw, noisy, tool results)
- layer 1: parsed and validated (schema enforced)
- layer 2: summarized (key decision + rationale)
- layer 3: indexed (semantic vectors, metadata tags)
- layer 4: promoted to project state (only high-confidence facts)

Each promotion requires gating; not everything graduates.

### 3.4 Gap Against Current go-on Implementation

Comparing the 8 skills and 6 optimizations against current go-on:

**Already covered or easy to add to existing infrastructure:**

- Tool-execution budgets (can layer on top of Phase 1)
- Task-level audit logging (can extend metrics and trace system)
- Circuit-breaker fallback (partially exists; easy to expose in traces)
- Prompt architecture optimization (tool, not runtime change; can guide prompt templates)

**Partially covered:**

- Review gate exists, but not structured with typed schemas or adversarial checks
- Memory exists (cache, vector, summary) but lacks unified policy and guards
- Multi-agent is phase-routed, but not role-specialized with typed contracts

**Missing entirely:**

- Coordinator anti-lazy-delegation contract (requires explicit task envelope types)
- Worker task schema discipline (requires typed handoff layer)
- Self-rationalization guard (requires assumption tracking and revalidation logic)
- Typed memory classes (requires memory policy and promotion pipeline)
- Startup repository context loading (requires async bootstrap phase)
- Forked sub-agent isolation (requires process/execution isolation for parallelism)

### 3.5 Recommended Additions to the Roadmap

To incorporate the 8+6 synthesis, add these targeted enhancements:

**Immediate (Phases 0-1):**

- Add typed task envelope and handoff schema to enable Skills 1-2
- Document agent roles and their input/output schema contracts
- Add deterministic verification signals to strengthen existing review gate

**Near-term (Phases 2-3):**

- Implement memory policy layer with typed classes and promotion pipeline (Skill 5-6)
- Add self-rationalization logic: assumption tracking, evidence validation, re-examine triggers (Skill 4)
- Add startup context loading for repository baseline (Skill 7)
- Extend audit logging to cover all agent decisions (Optimization 3)

**Mid-term (Phases 4-5):**

- Enforce prompt architecture segmentation (Skill 8, Optimization 5)
- Implement coordinator anti-lazy-delegation contract in delegation logic (Skill 1)
- Add forked sub-agent isolation primitives for parallel branches (Optimization 6)
- Strengthen review gate with structured verification and adversarial checks (Skill 3)

**Long-term (Phases 6-9):**

- Promote memory pipeline through all phases (Optimization 4)
- Integrate all agent-discipline rules into graph execution model (Phase 6)
- Build evaluation suite that validates agent-discipline compliance at task level

## 4. Future Operating Model

The recommended target is:

"A policy-governed, phase-aware, graph-orchestrated coding-agent runtime with typed tools, role-specialized sub-agents, durable task state, memory policy, and evaluation-grade observability."

That target keeps the current strengths of go-on:

- ACP compatibility
- provider flexibility
- operational guardrails
- configurable rules and phases

But upgrades the core from "provider router with review gate" into "agent runtime with controlled automation".

## 5. Phased Roadmap

## Phase 0: Stabilize The Current Base

Goal: make the current system an explicit platform before adding deeper agent behavior.

Deliverables:

- define a formal runtime architecture note for request lifecycle, phase lifecycle, and review gate lifecycle
- document invariants for cache, vector memory, summary memory, and breaker state
- standardize RPC schemas for diagnostics and future extensibility
- add a compatibility matrix for modes, phases, and provider capabilities
- **[Agent Discipline]** document the contract for agent task envelopes (objective, constraints, evidence, failure modes)
- **[Agent Discipline]** define input/output schemas for each planned agent role (planner, coder, tester, reviewer, researcher)
- **[Agent Discipline]** add audit logging schema for agent decisions and outcomes

Exit criteria:

- architecture boundaries are documented
- all current control RPCs have contract tests
- mode semantics are stated precisely in docs and config validation
- agent task envelope contract is defined and validated at phase boundaries

## Phase 1: Add A First-Class Tool Runtime

Goal: move from model-only execution to real agent execution.

Deliverables:

- introduce an internal tool interface with typed input and output schemas
- support a minimal safe tool set for coding workflows:
  - read file
  - search files
  - apply patch
  - run tests
  - inspect git diff
- add per-tool permission levels and audit records
- expose tool call metrics and tool failure taxonomy
- **[Agent Discipline]** enforce tool call budgets (max calls, max tokens per call, max wall-clock per loop)
- **[Agent Discipline]** implement deterministic verification signals for tool outputs (syntax, schema, compile checks)
- **[Agent Discipline]** add structured task envelope validation at tool invocation (confirm constraints and objective are still valid)

Recommended implementation notes:

- keep provider adapters separate from tool orchestration
- treat tools as runtime capabilities, not provider-specific features
- require bounded tool loops with max steps and max wall-clock budget
- treat tool output validation as a first-class verification signal alongside LLM review

Exit criteria:

- `agent` mode can complete at least one multi-step tool loop
- tool calls are visible in traces and metrics
- permission gating works per tool class
- tool budgets are enforced and visible in telemetry

## Phase 2: Separate Modes By Orchestration Semantics

Goal: make modes reflect different runtime behaviors, not only different approval levels.

Target definitions:

- `ask`: single-turn assistant mode with optional retrieval, no write tools
- `edit`: constrained edit workflow with plan, patch, and verification
- `agent`: iterative planner-executor loop with tool use
- `full_auto`: workflow or graph mode with gated autonomy and recovery policy

Deliverables:

- a mode runtime trait or strategy layer
- separate budgets and stop conditions per mode
- explicit allowed tool surface per mode
- explicit user-approval policy per mode

Exit criteria:

- each mode maps to a distinct execution contract
- telemetry reports mode-specific outcomes and failure classes

## Phase 3: Introduce Durable Task State And Plans

Goal: make long-running agent work resumable and inspectable.

Deliverables:

- task model with task id, state, current node, pending actions, and outcome
- plan representation with ordered steps and status transitions
- persistence for in-progress and completed tasks
- new RPCs for task inspect, task resume, task cancel, and task list

Recommended implementation notes:

- begin with a linear plan model before moving to graphs
- persist enough state to resume after process restart
- keep conversational transcript separate from task-state snapshot

Exit criteria:

- interrupted `agent` sessions can resume from saved state
- plan progress is externally visible and testable

## Phase 4: Upgrade Review Gate Into Structured Verification

Goal: make review a typed decision layer instead of a raw second opinion.

Deliverables:

- structured review schema:
  - approve
  - reject
  - revise
  - insufficient-evidence
- reviewer rationale fields and confidence fields
- verification stages such as syntax, tests, policy, and task-completion checks
- disagreement handling and arbitration policy
- support for model reviewer plus deterministic verifier combination
- **[Agent Discipline: Adversarial Verification Policy]** implement independent verifier that questions primary output assumptions
- **[Agent Discipline: Self-Rationalization Guard]** require reviewers to validate assumptions against evidence; auto-flag weak evidence
- **[Agent Discipline: Structured Verification]** introduce verification confidence score and require high confidence + deterministic pass for `full_auto` approval

Recommended implementation notes:

- treat tests, lint, and compile checks as independent verification signals
- do not rely only on LLM reviewers for execution approval
- do not allow a single reviewer to approve high-risk operations; require consensus or multiple signal types

Exit criteria:

- `full_auto` approval can depend on both LLM review and deterministic verification
- invalid reviewer output is isolated and measurable
- structured verification verdicts include confidence and reasoning trace

## Phase 5: Add Role-Specialized Multi-Agent Collaboration

Goal: evolve from fallback-plus-review into genuine multi-agent cooperation.

Recommended initial roles:

- Planner
- Researcher
- Coder
- Tester
- Reviewer

Deliverables:

- sub-agent handoff contract with compact structured context
- role-specific prompt templates and policy budgets
- manager or coordinator role for delegation
- merge policy for multiple agent outputs
- **[Agent Discipline: Coordinator Anti-Lazy-Delegation]** enforce that each delegation includes explicit task objective, constraints, evidence pointers, and failure modes
- **[Agent Discipline: Worker Task Schema]** define and enforce input/output schemas for each role; reject invalid output as fatal
- **[Agent Discipline: Typed Memory Classes]** implement role-aware memory retrieval: each role gets project-state + episodic context sized appropriately to role
- **[Agent Discipline: Startup Context Loading]** async load and cache project README, architecture, build commands, and style rules before spawning worker agents

Recommended implementation notes:

- do not start with fully dynamic agent swarms
- begin with a fixed small set of roles and explicit handoff schema
- reuse current phase model to constrain which roles can run in which phases
- treat handoff contract violation as task failure, not graceful degradation

Exit criteria:

- one task can involve at least planner -> coder -> tester -> reviewer handoff
- each handoff is traceable and replayable
- each agent receives compact context envelope matching its schema
- schema violations in handoff are caught and logged

## Phase 6: Move From Phase Chain To Execution Graph

Goal: support branching, joins, retries, and recovery as first-class orchestration.

Deliverables:

- graph or DAG execution model
- node types such as plan, act, verify, review, summarize, and finalize
- conditional transitions based on tool results, test outcomes, and reviewer decisions
- checkpointing per node

Recommended implementation notes:

- preserve the current phase abstraction as a policy lens
- add graph execution underneath rather than replacing config-driven phases immediately

Exit criteria:

- one workflow can branch on verification outcome and re-enter correction flow automatically
- graph state is persisted and resumable

## Phase 7: Build A Real Memory Policy Layer

Goal: promote existing memory primitives into a coherent long-term system.

Memory layers:

- transient conversation state
- task-state memory
- episodic memory from prior tasks
- semantic artifact memory
- distilled project memory

Deliverables:

- explicit write policy for what gets stored and when
- promotion policy from transcript -> summary -> durable memory
- decay and pruning policy
- phase-aware and task-aware retrieval ranking
- provenance tags on retrieved memory
- **[Agent Discipline: Typed Memory Classes]** enforce separate memory classes with different policies: transient, episodic, semantic, project-state, observation
- **[Agent Discipline: Memory Promotion Pipeline]** implement gated pipeline: raw output -> validated -> summarized -> indexed -> promoted to project state
- **[Agent Discipline: Memory Guards]** implement correctness guards: usefulness scoring, staleness detection, size bounds per class, periodic revalidation, garbage collection

Exit criteria:

- memory retrieval is explainable
- noisy or low-value memory does not accumulate without control
- memory promotion requires gating; not all artifacts graduate to durable state
- retrieved memory includes confidence and staleness metadata

## Phase 8: Add Trace And Evaluation Infrastructure

Goal: make go-on measurable as an agent platform.

Deliverables:

- structured traces for turns, tools, plans, graph transitions, and reviewer decisions
- trace export format for offline analysis
- benchmark suite with scenario datasets such as:
  - coding task completion
  - edit correctness
  - test-fix loops
  - review precision / false approval rate
  - fallback quality under provider failure
- regression gates in CI for critical agent workflows

Recommended implementation notes:

- keep metrics for operations, traces for semantics, and evals for quality
- do not treat Prometheus alone as sufficient observability for agents

Exit criteria:

- major workflow regressions are detectable before release
- trace replay can explain why a task succeeded or failed

## Phase 9: Production Hardening For Autonomous Operation

Goal: support larger-scale or unattended use without losing safety.

Deliverables:

- idempotent task execution where practical
- stronger sandboxing for write and shell tools
- resource budgeting per task and per tenant
- queueing and scheduling for concurrent agent tasks
- audit log for autonomous edits and approvals
- policy bundles for local-dev, CI, and managed-service deployments

Exit criteria:

- unattended workflows are bounded, auditable, and recoverable
- operational posture is suitable for CI or team-shared environments

## 5B. Token Cost Optimization Blueprint (Layered Triggering)

The most effective cost reduction pattern for coding agents is:

"Do not run expensive reasoning until cheaper filters prove it is necessary."

This section proposes a strict layered triggering design similar in spirit to modern high-efficiency agent runtimes.

### 5B.1 Core Principle

Every request flows through a cascade of layers. A request only escalates to the next layer if the current layer cannot safely finish it.

If a request is resolved in a lower layer, higher-cost layers are never triggered.

### 5B.2 Recommended Layer Stack

#### Layer 0: Deterministic Fast Reject / Fast Route

Purpose:

- reject invalid or oversized requests
- route clearly trivial requests to `ask` behavior
- block unnecessary expensive pipelines

Typical checks:

- schema validity
- message count and character budget
- obvious duplicate requests
- explicit command shortcuts

Token usage target: zero model tokens.

#### Layer 1: Cache And Reuse Layer

Purpose:

- return from exact cache or near-exact response reuse
- avoid model invocation entirely

Typical checks:

- normalized request hash match
- phase-aware cache validity
- stale-policy checks

Token usage target: zero model tokens.

#### Layer 2: Cheap Classification Layer

Purpose:

- estimate complexity, risk, and required capability using a tiny budget
- decide if the request should stay in cheap mode or escalate

Typical outputs:

- complexity class: low / medium / high
- required mode: ask / edit / agent / full_auto
- likely tool necessity: none / read-only / write-needed
- confidence score

Implementation note:

- use either deterministic heuristics first, then a low-cost classifier model only when heuristics are inconclusive

Token usage target: very low.

#### Layer 3: Context Compression Layer

Purpose:

- shrink prompt and memory payload before any expensive generation

Typical operations:

- history truncation by relevance
- summary compaction
- vector retrieval cap by dynamic top-k
- principle deduplication and compression

Token usage target: low and bounded.

#### Layer 4: Primary Generation Layer

Purpose:

- execute the selected mode with the minimum sufficient model capacity

Policy:

- default to smaller / cheaper model tier
- escalate model tier only on confidence failure or verification failure

Token usage target: moderate, tightly bounded by per-mode budgets.

#### Layer 5: Verification / Review Escalation Layer

Purpose:

- run expensive dual-review or multi-agent checks only for high-risk or high-impact outputs

Trigger conditions:

- write operations
- low confidence from primary generation
- deterministic verifier failure
- policy-sensitive phases

Token usage target: highest, but only for a small subset of requests.

### 5B.3 Escalation Rules (Strict Gates)

Use explicit gate conditions between layers:

- Gate A (L1 -> L2): no valid cache hit
- Gate B (L2 -> L3): complexity >= medium OR confidence < threshold
- Gate C (L3 -> L4): request still unresolved after compression + retrieval
- Gate D (L4 -> L5): output risk high OR verification failed OR mode is `full_auto`

If a gate is not passed, stop and return.

This is the key to reducing token spend.

### 5B.4 Budget Model

Define hard budgets at three levels:

- per request budget
- per mode budget (`ask`, `edit`, `agent`, `full_auto`)
- per phase budget (for example stricter caps in `coding`, higher caps in `review`)

Recommended controls:

- `max_input_tokens_per_request`
- `max_output_tokens_per_request`
- `max_total_tokens_per_turn`
- `max_tool_iterations`
- `max_review_tokens`

When any budget is exceeded, degrade safely:

- reduce context window
- reduce retrieval depth
- skip optional reviewers
- return partial result with explicit status

### 5B.5 Dynamic Triggering Signals

Use runtime signals to decide escalation, not static thresholds alone:

- cache hit rate trend
- vector precision trend
- summary usefulness score
- prior turn failure category
- latency pressure and queue depth

This lets the system auto-tighten or auto-relax without manual retuning each week.

### 5B.6 Concrete Upgrades For go-on

Add these features in order:

1. `token.classify` stage before model invocation
2. per-mode token budget and hard stop policy
3. confidence + risk fields in model output envelope
4. verification-trigger policy before dual-review gate
5. model tier escalation ladder (small -> medium -> premium)
6. Prometheus counters per gate transition, for example:
  - `acp_gate_a_pass_total`
  - `acp_gate_b_block_total`
  - `acp_layer5_invocations_total`
7. monthly policy replay based on trace samples to tune thresholds

### 5B.7 Definition Of Success For Cost Control

The layered strategy is working when:

- most requests are resolved by Layer 0-4
- Layer 5 invocation rate stays low and intentional
- token spend per successful task decreases without quality regression
- escalation decisions are explainable from trace records

In short: cheap paths handle common work, expensive paths handle only hard or risky work.

### 5B.8 Prompt Architecture Optimization

Prompts are often the largest hidden cost in agent systems. Structure prompts in layers to maximize reuse and minimize token waste:

**Static Layers (reused across many requests):**

- system.role: role-specific instructions (planner vs. coder vs. reviewer tone and constraints)
- system.mode: mode-specific rules (ask vs. edit vs. agent vs. full_auto semantics)
- system.phase: phase-specific guidance (discovery, implementation, testing, review phases)
- system.conventions: project-specific constraints, style rules, approved tool usage

**Dynamic Layers (per-task):**

- task.objective: what result is needed, acceptance criteria
- task.constraints: budget, deadline, tool restrictions, scope limits
- task.evidence: prior artifacts, decisions, evidence pointers from prior steps

**Per-Turn Layers:**

- turn.context: current state, recent decisions
- turn.new_info: new input, tool results, feedback

**Implementation:**

- cache static layer hashes; send only delta if unchanged
- tokenize each layer independently to measure per-layer cost
- use compact formats (JSON keys vs. prose) for structured data
- compress history using summaries before sending; never send raw transcript to expensive models
- use vector retrieval to insert only relevant prior evidence, not all past context

**Cost Impact:**

Aligned prompt architecture can reduce per-request cost by 20-40% without quality loss, because dynamic and per-turn layers are much smaller than full raw transcript.

## 5C. Multi-Agent And Multi-Task Concurrency Blueprint

This section defines a practical high-throughput design for running many agent tasks in parallel without losing safety, fairness, or quality.

### 5C.1 Core Principle

"Parallelize by default, but isolate by policy."

Concurrency is valuable only when each unit of work remains:

- bounded by budget
- isolated by failure domain
- auditable by trace
- stoppable by policy

### 5C.2 Concurrency Model

Use a two-level scheduler.

Level 1: Task Scheduler

- schedules user tasks and workflow jobs
- enforces tenant/session fairness
- applies global concurrency caps

Level 2: Agent Worker Scheduler

- schedules agent role workers within a task
- enforces per-role and per-provider limits
- controls fan-out and join behavior

This prevents one noisy task from starving all other tasks.

### 5C.3 Recommended Execution Pattern

For each task, use a DAG-style execution contract:

- fan-out nodes for independent work
- join nodes for result aggregation
- verify nodes before critical transitions
- retry nodes with bounded backoff
- cancel-propagation on hard policy failures

Typical coding pipeline:

- planner -> parallel researcher/coder/tester branches -> reviewer -> finalize

### 5C.4 Parallelism Classes

Class A: Safe Parallelism

- read-only search and analysis
- independent file inspections
- benchmark/evaluation probes

Class B: Controlled Parallelism

- patch drafting in isolated workspaces
- test runs in sandboxed workers
- reviewer fan-out with deterministic schema

Class C: Serialized Critical Section

- final write/apply step
- merge conflict resolution
- promotion to durable memory

Rule:

- allow high concurrency for Class A
- medium capped concurrency for Class B
- strict serialization for Class C

### 5C.5 Resource Governance

Define hard limits at four scopes:

- global process limits
- per-tenant or per-session limits
- per-task limits
- per-agent-role limits

Recommended controls:

- `max_concurrent_tasks`
- `max_workers_per_task`
- `max_parallel_reviews`
- `max_provider_rps` and `max_provider_inflight`
- `max_queue_depth`
- `max_task_wall_clock_seconds`

If pressure exceeds limits:

- queue new work
- degrade optional branches
- drop low-priority speculative branches
- preserve reviewer and verifier capacity first

### 5C.6 Failure Isolation And Recovery

Use strict failure domains:

- one worker crash must not crash sibling workers
- one provider outage must not block unrelated providers
- one task timeout must not poison scheduler state

Recovery policy:

- retry transient failures with jittered backoff
- trip circuit breaker per provider/role
- fallback to smaller branch plan when retries exhausted
- return partial artifacts with explicit completion state

### 5C.7 Multi-Agent Coordination Contract

Require typed handoff payloads between agents:

- task objective
- constraints and acceptance criteria
- evidence references
- produced artifacts
- confidence and risk score

Do not pass full raw transcript by default.

Use compact context envelopes to reduce token cost and cross-agent noise.

### 5C.8 Queueing And Priority Strategy

Use weighted priority queues with anti-starvation:

- priority dimensions:
  - user urgency
  - task risk
  - estimated token cost
  - deadline proximity
- anti-starvation policy:
  - aging factor increases waiting task priority over time

This avoids permanent blocking of long tasks by frequent small tasks.

### 5C.9 Observability Requirements For Concurrency

Track both throughput and quality under load:

- queue wait time
- scheduler latency
- worker utilization by role
- branch cancellation rate
- join failure rate
- per-role timeout and retry rate
- reviewer disagreement rate under concurrency
- token spend per completed task

Add trace fields:

- `task_id`
- `workflow_node_id`
- `worker_id`
- `branch_id`
- `join_id`
- `scheduler_decision_reason`

### 5C.10 Concrete Upgrades For go-on

Implement in this order:

1. Introduce task queue plus worker pool abstraction
2. Add per-phase and global task concurrency caps
3. Add role-based worker pools (planner/coder/tester/reviewer)
4. Add fan-out/join primitives with bounded branch count
5. Add typed handoff schema and validation
6. Add cancellation propagation and partial-result contract
7. Add concurrency-specific Prometheus metrics and alerts
8. Add load-test scenarios and regression gates for scheduler behavior

### 5C.11 Definition Of Success For Multi-Agent Concurrency

The design is successful when:

- throughput increases with parallelism without quality collapse
- critical write paths remain deterministic and safe
- queue delays remain bounded under expected load
- one provider or worker failure does not cause systemic outage
- traces can fully reconstruct branch decisions and joins

In short: more parallel work, but with predictable control and recoverable outcomes.

### 5C.12 Forked Sub-Agent Isolation

When spawning parallel branches (for example planner + concurrency research while coder works), isolate each branch strictly:

**Execution Isolation:**

- each forked agent runs in a separate execution context
- child agent cannot modify parent state
- child failure (crash, timeout, invalid output) must not corrupt parent
- resource limits (token count, wall-clock time, RPC quota) are per-child, not shared

**Lifespan Control:**

- parent can terminate child with kill signal
- policy timeout kills child automatically
- child cannot escape its designated time/token budget
- parent waits for all children, with explicit join semantics

**Output Validation:**

- child output is validated against schema before merge
- invalid output is treated as task failure, not silent drop
- output from multiple children is aggregated by merge policy, not concatenated
- confidence and provenance metadata from each child is retained

**Failure Recovery:**

- child timeouts trigger automatic fallback (smaller branch plan, skip optional child)
- child crash does not propagate exception to parent
- failed child's partial output is tagged failed + incomplete
- parent continues with remaining children or degraded outcome

**Observability:**

- each forked agent has unique `child_id` in traces
- fork decision, fork point, join point all logged
- child resource usage (tokens, time, calls) measured independently
- child-to-child causality is visible in trace graph

**Implementation Notes:**

- use process-level or container-level isolation if possible (for true process isolation)
- at minimum, use execution scope isolation + resource limits
- validate kill-signal handling; do not leave zombie children
- test fork/join under provider failures and token exhaustion scenarios

## 6. Recommended Build Order

If engineering bandwidth is limited, implement in this order:

1. Phase 1: first-class tool runtime
2. Phase 2: true mode semantics
3. Phase 3: durable task state
4. Phase 4: structured verification
5. Phase 5: role-specialized multi-agent collaboration
6. Phase 8: trace and evaluation
7. Phase 6: execution graph
8. Phase 7: memory policy layer
9. Phase 9: production hardening

Reasoning:

- tools are the missing foundation for a real coding agent
- mode semantics and durable task state are needed before serious multi-agent work
- verification is more urgent than agent swarms
- traces and evals should arrive before graph complexity becomes hard to debug

## 7. Explicit Non-Goals For The Near Term

The following should not be prioritized early:

- unconstrained autonomous shell execution by default
- fully dynamic swarm architectures without role contracts
- provider-specific orchestration logic embedded in core runtime
- adding more model backends before orchestration quality improves
- cosmetic UI work before traceability and verification exist

## 8. Near-Term Release Plan

### Release A: Agent Runtime Foundation

- first-class tool interface
- bounded tool loop in `agent` mode
- tool metrics and permission policies
- contract tests for tool orchestration

### Release B: Controlled Edit Automation

- edit workflow semantics
- plan / patch / verify cycle
- deterministic verification hooks
- improved review gate outputs

### Release C: Durable Tasks

- persisted task state
- resume and inspect RPCs
- task-level metrics and trace ids

### Release D: Real Multi-Agent Collaboration

- planner, coder, tester, reviewer roles
- structured handoffs
- multi-agent traces and arbitration policy

### Release E: Evaluation-Grade Platform

- trace export
- benchmark suite
- regression evaluation in CI

## 9. Success Criteria

go-on should be considered upgraded to a modern agent runtime when all of the following are true:

- modes correspond to distinct orchestration contracts
- at least one bounded tool-using coding workflow works end-to-end
- review and verification combine model judgment with deterministic checks
- multi-agent collaboration is role-based, traceable, and resumable
- memory is policy-driven rather than purely opportunistic
- trace and evaluation systems can detect quality regressions
- autonomous behavior remains auditable and bounded by explicit policy

## 10. Final Recommendation

Do not position go-on as a generic "multi-agent platform" yet.

The stronger and more accurate positioning for the next stage is:

"A phase-aware Rust coding-agent runtime with strong operational guardrails, evolving toward a graph-orchestrated and evaluation-driven multi-agent system."

That framing matches the codebase more honestly and leads to a better upgrade strategy.