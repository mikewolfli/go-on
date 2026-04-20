# BLUE10 — Adaptive Main-Chain Completion

This document tracks the BLUE10 one-shot closure focused on adaptive reinforcement wiring in the primary execution chain.

---

## Status

Implemented in this pass:

| ID | Item | File | Status |
|----|------|------|--------|
| B10-P0-1 | Chat main chain now reorders phase agents using runtime online-controller scores before fallback execution | `src/acp/impl/chat.rs` | ✅ Done |
| B10-P0-2 | Execution context now defaults `failure_strategy` from persisted learning recommendations when not explicitly provided | `src/acp/impl/request.rs` | ✅ Done |
| B10-P0-3 | Execution context now defaults `mode/work_grade` from persisted learning recommendations when not explicitly provided | `src/acp/impl/request.rs` | ✅ Done |
| B10-P0-4 | `workflow.generate`, `workflow.execute`, and `task.execute` now tune predicted success rate from LearningBus before workflow generation | `src/acp/impl/request.rs` | ✅ Done |
| B10-P0-5 | `workflow.generate`, `workflow.execute`, and `task.execute` now apply learned parallelism to execution-order chunking | `src/acp/impl/request.rs` | ✅ Done |

Strengthening completed in follow-up pass:

| ID | Item | File | Status |
|----|------|------|--------|
| B10-P1-1 | Adaptive decision transparency: workflow/task responses now include applied planning/execution adaptive reports | `src/acp/impl/request.rs` | ✅ Done |
| B10-P1-2 | Unified learning feedback helper to avoid drift across handlers | `src/acp/impl/request.rs` | ✅ Done |
| B10-P1-3 | Added unit tests for execution-order rebalance and inferred parallelism behavior | `src/acp/impl/request.rs` | ✅ Done |

---

## Main-Chain Reinforcement Wiring

### 1) Chat adaptive agent ordering

`process_chat_request` now applies runtime score ordering for phase candidates before execution. This preserves fallback semantics while improving first-choice quality under changing agent health/reliability.

### 2) Learning-driven runtime defaults

`build_execution_context` now derives defaults from persisted workflow learning:
- `failure_strategy`: recommended from historical fail patterns
- `mode`: recommended work grade from historical execution outcomes

Explicit request parameters still override these defaults.

### 3) Learning-driven planning feedback

Before persisting/using generated workflows in key handlers, BLUE10 now:
- adjusts `predicted_success_rate` via learning regression
- rebalances workflow execution phases to respect learned parallelism targets

This turns LearningBus recommendations into active planning behavior rather than passive analytics.

---

## Acceptance Criteria

- Adaptive ranking is applied in chat main path before provider invocation
- Learning recommendations are used by execution defaults when request does not pin values
- Planning handlers consume learning recommendations for success-rate/parallelism
- Full test and clippy suite pass

---

## 2026-04-11 Final Acceptance (BLUE10)

| Check | Result |
|------|--------|
| Chat adaptive ordering | ✅ Passed. Main-chain chat now applies online-controller ranking before fallback loop. |
| Learning-driven execution defaults | ✅ Passed. `failure_strategy` and `mode` default from LearningBus recommendations when request does not pin them. |
| Learning-driven planning feedback | ✅ Passed. Predicted success and parallelism are tuned from LearningBus for workflow/task execution paths. |
| Tests | ✅ `cargo test --all-targets --all-features` passed (167 unit + 28 integration). |
| Static checks | ✅ `cargo clippy --all-targets --all-features --message-format=short` passed with zero warnings. |
| Strengthening pass | ✅ Adaptive response reports and helper/test hardening landed and validated. |

Final state for BLUE10 scope: implemented and validated end-to-end.

---

## Round-3 items (2026-04-11)

| ID | Priority | Item | File | Status |
|----|----------|------|------|--------|
| B10-R3-1 | P1 | KnowledgeBus `persist_knowledge_insight_event` gains dedup + confidence-arbitration: same `(task, phase, agent)` key with lower confidence is silently discarded; higher confidence replaces the old event | `src/intelligence/reinforcement.rs` | ✅ Done |
| B10-R3-2 | P1 | `VectorStore::search` adds time-decay weighted scoring: recent entries are boosted proportionally (`1/(1 + age_days * decay_factor)`) before final sort, so stale knowledge is demoted without being deleted | `src/memory/vector.rs` | ✅ Done |
| B10-R3-3 | P2 | `rpc_conversation_checkpoint_and_rollback` hardens shutdown flush: explicit `drop(harness.stdin)` before `wait_for_exit` prevents write-side pipe race in multi-process harness | `tests/acp_runtime_rpc_integration.rs` | ✅ Done |

---

## 2026-04-11 Round-3 Final Acceptance

| Check | Result |
|-------|--------|
| KnowledgeBus dedup + confidence arbitration | ✅ `persist_knowledge_insight_event` skips lower-confidence duplicates and replaces same `(task, phase, agent)` key only when the incoming event has strictly higher confidence. 2 new unit tests. |
| VectorStore time-decay weighted scoring | ✅ `search` now fetches `updated_at`, computes `recency_weight = 1/(1 + age_days×0.05)`, blends 70% similarity + 30% recency before sorting. 1 new unit test verifies stale entries rank below fresh ones. |
| Integration test pipe-race fix | ✅ `RpcHarness.stdin` changed to `Option<ChildStdin>`; `close_stdin()` added; `wait_for_exit` calls it first so the child sees EOF before the process-wait loop. Eliminates the write-side pipe-reader hang. |
| Tests | ✅ `cargo test --all-targets --all-features` passed (170 unit + 28 integration). |
| Static checks | ✅ `cargo clippy --all-targets --all-features --message-format=short` passed with zero warnings. |

---

## Round-4 strengthening (2026-04-11)

| ID | Priority | Item | File | Status |
|----|----------|------|------|--------|
| B10-R4-1 | P1 | Wire historical execution-success ordering into workflow subtask candidate ranking: `rank_execution_agents(...)` scores are now blended with `recommend_agent_order_from_execution_history(...)` so list-position heuristics are corrected by real historical outcomes | `src/acp/impl/request.rs` | ✅ Done |
| B10-R4-2 | P1 | Wire vector memory into workflow/task subtask execution context: `execute_single_subtask` now injects top relevant vector hits into the user message before agent invocation, enabling request-path retrieval parity with chat-path retrieval | `src/acp/impl/request.rs` | ✅ Done |

---

## 2026-04-11 Round-4 Final Acceptance

| Check | Result |
|-------|--------|
| Historical execution ranking wiring | ✅ `recommend_agent_order_from_execution_history` is now called from workflow execution ranking path; candidates get blended score (60% policy heuristic + 40% historical rank) and deterministic re-sort before dispatch. |
| Request-path vector retrieval wiring | ✅ `RuntimeExecutionContext` now carries `vector_store`; `execute_single_subtask` queries vector memory and injects retrieved snippets into prompt context. Retrieval now uses both execution phase key (`phase-N`) and semantic default phase key (e.g. `coding`) to avoid phase-key mismatch and improve real hit rate. |
| Legacy scaffolding cleanup with extension points | ✅ Unwired heavy scaffolding (`promotion`/`workflow_optimizer`) was replaced by lightweight plugin interfaces (`Noop` baseline + trait contracts) so main chain stays clean while future expansion remains stable. |
| Tests | ✅ `cargo test --all-targets --all-features` passed (170 unit + 28 integration). |
| Static checks | ✅ `cargo clippy --all-targets --all-features --message-format=short` passed with zero warnings. |
| Compile validation | ✅ `cargo build` passed after wiring changes. |

---

## Completion

- BLUE10 P0 completion: **100%** (5/5)
- BLUE10 P1 completion: **100%** (3/3)
- BLUE10 Round-3 completion: **100%** (3/3)
- BLUE10 Round-4 completion: **100%** (2/2)
- Overall BLUE10 completion: **100%** (13/13)
