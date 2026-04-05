# Refinement and Expansion Plan (Priority: High to Low)

## Execution Status (2026-04-05)
- Implemented now:
  - Mandatory runtime pipeline entry for non-trivial chat tasks: Analyze -> Route(Hard Gate) -> Execute -> Verify -> Evaluate -> Learn.
  - Hard-gate routing enforcement wired into ACP runtime path (not advisory-only).
  - Reviewer-required tasks are now blocked when mode/strategy does not enable dual review gate.
  - Pipeline stage events are emitted into runtime trace stream for observability.
  - Phase 11 foundation: online controller now ingests live failure/latency outcomes and can auto-escalate full_auto strategy from simple to complex when risk signals rise.
- Validation proof:
  - cargo check --all: passed
  - cargo test: passed (163 unit + 5 integration)
- Next execution focus:
  - Item 3 deepening: feed online controller signals into routing order/agent fallback selection (not just approval strategy).

## 1) Enforce a Single Execution Pipeline (Highest Priority)
The system should use one mandatory runtime pipeline for all non-trivial tasks:
Analyze -> Route -> Execute -> Verify -> Evaluate -> Learn.

Why:
- Prevents "defined but not used" modules.
- Ensures Phase 10/11 logic participates in real execution, not just side analysis.

Action:
- Make pipeline stages mandatory in ACP request flow.
- Reject or degrade execution when required stage outputs are missing.

## 2) Turn Phase 10 Routing into Hard Gates
Routing output should not be advisory only. It must become enforcement input.

Why:
- Role plans are currently strong in structure but weak in runtime authority.
- High-risk work should not bypass planner/tester/reviewer constraints.

Action:
- Convert routing decisions and PUA enforcement into pre-execution gates.
- Block execution when mandatory roles/safeguards are not satisfied.

## 3) Turn Phase 11 Optimizers into Online Controllers
Workflow and reliability optimizers should consume live outcomes and influence next decisions.

Why:
- Current heuristic models are useful, but mostly static.
- Without feedback loops, optimization quality does not improve over time.

Action:
- Feed real signals: success/failure, retries, timeouts, review outcomes, latency.
- Update strategy selection and phase sequence based on rolling windows.

## 4) Reduce ACP Super-Module Weight
ACP currently carries too many responsibilities and is at risk of becoming a change bottleneck.

Why:
- Large files increase regression risk and review cost.
- Cross-cutting logic is harder to test and reason about.

Action:
- Split ACP into focused modules:
  - rpc_handler
  - execution_pipeline
  - runtime_controls
  - observability

## 5) Unify Quality Models Across Verification/Evaluation/Reliability
Quality and verification semantics should be represented by one core model family.

Why:
- Overlapping types create drift and inconsistent policy enforcement.
- Mixed string-based and structured signals reduce reliability.

Action:
- Consolidate verdict/signal/result models.
- Replace loose string fields with typed enums/structs where practical.

## 6) Strengthen Observability Beyond Basic Counters
Current metrics are good, but request-level causality should be first-class.

Why:
- Multi-agent paths are hard to debug without trace-level context.
- PUA enforcement should be auditable per stage, not inferred.

Action:
- Keep expanding trace coverage for all major stage transitions.
- Ensure each stage emits structured trace attributes and policy status.

## 7) Add Developer-Control and UX Surfaces (Needed for Zed ACP)
For a Zed ACP agent, these are not optional polish items; they are core operational capabilities.

Why:
- Streaming model output is required for trust and usability: users need to see progress while the model is generating.
- Conversation branching/rollback is required for safe iteration: when the agent makes a wrong turn, users must recover quickly without losing prior context.
- MCP protocol support is required for ecosystem interoperability: custom tool-call formats increase maintenance and reduce compatibility with existing MCP tooling.
- A WebUI debug panel is required for diagnosability: teams need to inspect routing, tool decisions, safeguards, and review outcomes in real time.

Action:
- Implement token-level streaming in the chat channel so users can watch incremental generation.
- Add conversation checkpoints and branch IDs to support rollback and alternate-path continuation.
- Provide an MCP-compatible adapter layer (or native MCP path) alongside existing internal RPC.
- Add a WebUI runtime panel showing stage transitions, selected agents, tool invocations, and review-gate verdicts.

## 8) Improve Configuration UX with Layered Profiles
Configuration is powerful but cognitively heavy for new users.

Why:
- Too many knobs can cause misconfiguration and low adoption.
- Setup should provide safe defaults first, power controls second.

Action:
- Maintain a minimal profile for quick start.
- Keep a full profile for advanced users.
- Continue adding recommendation-style warnings for suspicious combinations.

## 9) Upgrade Test Strategy from Unit-Heavy to Failure-Driven
Unit coverage is strong; resilience coverage should catch real-world operational failures.

Why:
- System behavior under reviewer timeout, provider failure, or degraded infra matters most.
- PUA claims require robust error-path validation.

Action:
- Add fault-injection integration tests:
  - reviewer timeout/deadline collisions
  - upstream provider failures
  - cache/vector unavailability
  - rate-limit and inflight saturation
- Validate graceful degradation paths explicitly.

## Suggested Delivery Sequence
1. Pipeline hardening + Phase 10 enforcement gates.
2. Phase 11 online feedback integration.
3. Observability + developer-control surfaces (streaming, rollback, MCP, debug UI).
4. ACP module split + quality model unification.
5. Failure-injection expansion and resilience hardening.

## Definition of "Not Empty Shell" for Phase 10/11
Phase 10/11 should be considered production-ready only when:
- Their outputs directly affect runtime decisions.
- Their states are visible in traces/metrics.
- Their behavior is covered by integration tests under failure conditions.
