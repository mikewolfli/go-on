# Refinement and Expansion Plan (Priority: High to Low)

## Execution Status (2026-04-07)
- Completion marking rule:
  - After each completed delivery step, explicitly update BLUE1 completion percentage and item-level completion status.
- Overall completion:
  - BLUE1 plan overall progress: 100% (9 / 9 weighted milestones).
- Implemented now:
  - Mandatory runtime pipeline entry for non-trivial chat tasks: Analyze -> Route(Hard Gate) -> Execute -> Verify -> Evaluate -> Learn.
  - Hard-gate routing enforcement wired into ACP runtime path (not advisory-only).
  - Reviewer-required tasks are now blocked when mode/strategy does not enable dual review gate.
  - Pipeline stage events are emitted into runtime trace stream for observability.
  - Phase 11 online controller consumes live failure/latency outcomes and escalates full_auto strategy from simple to complex when risk rises.
  - Phase 11 deepening completed for runtime control: online signals now drive main-phase candidate ordering/fallback chain and review-phase reviewer ordering.
  - Online controller feedback loop now ingests main execution outcomes and review outcomes per agent/phase.
  - Item 4 progress: extracted runtime_controls and review_controls from ACP super-module into dedicated modules.
  - Item 4 progress: extracted memory_response_cache from ACP super-module into dedicated module.
  - Item 4 completion: extracted rpc_protocol and observability helpers from ACP super-module into dedicated modules.
  - Item 5 completion: unified quality verdict/signal/result model across verification/evaluation/reliability and replaced loose verification signal strings with typed structures.
  - Item 6 completion: request-level trace causality strengthened by propagating request trace_id through chat pipeline, route/review/agent stages, and review route adaptation events.
  - Item 6 completion: standardized stage trace attributes (event_type/phase/stage/policy_status), added explicit review gate outcome traces (approved/rejected/degraded/failed), and added all-agent-failed evaluate trace.
  - Item 7 progress: delivered conversation control RPC surfaces with in-memory branch/checkpoint state, including conversation.checkpoint.create, conversation.checkpoint.list, and conversation.rollback.
  - Item 7 progress (continued): added auto-checkpoint on every successful agent response (keyed to conversation_id or trace_id), emits conversation.checkpoint notification to client, and added conversation.checkpoint.prune RPC method with TTL-based and count-based pruning.
  - Item 9 progress: added 3 new process-level integration tests — conversation checkpoint/rollback/prune flow, circuit-breaker status and reset, and cache.clear + parameter-validation error paths.
  - Item 7 progress (continued): added debug.panel.get RPC endpoint for WebUI runtime panel aggregation (trace stage transitions, selected agents, review outcomes, runtime health, conversation counters, review-gate counters).
  - Item 7 progress (continued): added MCP-compatible adapter path on ACP runtime with mcp.initialize, mcp.tools.list, and mcp.tools.call.
  - Item 7 progress (continued): refined UI-facing streaming telemetry by adding chunk progress metadata (chunk_index/total_chars/phase/trace_id) and explicit chat.stream.done heartbeat events for live streaming and cache-hit streaming paths.
  - Item 8 progress: enhanced layered configuration UX with suspicious-combination warning rules (cache/vector explicitly disabled, cache churn risk, observability overhead risk) and profile recommendation generation (minimal/balanced/full).
  - Item 8 progress (continued): surfaced profile_recommendation and recommendations in config.reload RPC response for direct UI/CLI consumption.
  - Item 9 progress (continued): added integration test for debug.panel.get snapshot shape and conversation-stat correctness.
  - Item 9 progress (continued): added integration test for MCP adapter compatibility path (initialize/list/call + error path).
  - Item 9 progress (continued): added unit-level verification for streaming notification payload schema and completion heartbeat payload.
  - Item 9 progress (continued): added config-health unit tests for layered profile recommendation and suspicious-combination warning coverage.
  - Item 9 completion: added fault-injection integration tests for upstream provider failure fallback degradation and review-timeout collision behavior, with deterministic local test agents and validated runtime outcomes.
  - Item 9 completion (stability): hardened provider-failure scenario with explicit per-phase request timeout and validated trace/runtime-health evidence for degraded execution paths.
  - Item 7 completion (stability): fixed conversation checkpoint prune branch-head consistency to avoid dangling references after branch/global prune, with unit and process-level regression coverage.
  - Item 7 completion (consistency): upgraded rollback to persisted copy-on-rollback checkpoints so branch list/head semantics stay consistent after rollback and prune.
  - Item 7 completion (hardening): added conversation/branch/checkpoint identifier validation for checkpoint RPCs and sanitized chat conversation_id input before auto-checkpointing.
  - Item 7 completion (capacity): added checkpoint memory guardrails (per-conversation checkpoint cap and per-checkpoint message-char cap) with automatic branch-head repair after enforced trimming.
  - Item 7 completion (observability): extended conversation.checkpoint.prune RPC response with repaired_heads and dropped_heads for client-side state sync.
  - Item 9 completion (follow-up): aligned review-timeout collision test naming with actual gate outcomes (reject or degrade) to avoid semantic drift.
  - Item 9 completion (follow-up): expanded integration coverage for checkpoint identifier validation, prune observability fields, and rollback branch visibility after prune.
- Completion by item:
  - Item 1 (Single mandatory pipeline): Completed (100%).
  - Item 2 (Phase 10 hard gates): Completed (100%).
  - Item 3 (Phase 11 online controllers): Completed (100%).
  - Item 4 (ACP module split): Completed (100%).
  - Item 5 (Quality model unification): Completed (100%).
  - Item 6 (Deep observability): Completed (100%).
  - Item 7 (Developer control + UX surfaces): Completed (100%).
  - Item 8 (Layered config UX): Completed (100%).
  - Item 9 (Failure-driven integration tests): Completed (100%).
- Validation proof:
  - cargo check --all: passed
  - cargo test: passed (170 unit + 14 integration)
- Next execution focus:
  - BLUE1 roadmap closed at 100%; next work should be post-roadmap hardening/performance initiatives.

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
