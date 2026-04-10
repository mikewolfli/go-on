# BLUE9 — Remaining Evolutionary Gaps

BLUE8 complete. This document captures items that were investigated and partially implemented during the BLUE9 analysis pass but deferred due to architectural constraints.

---

## Status

One-shot implemented (within BLUE9 analysis pass):

| ID | Item | File | Status |
|----|------|------|--------|
| B9-P0-1 | Replace stub `OnlineControllerState` with real sliding-window impl from `governance/runtime_controls.rs` | `src/acp/prelude.rs` | ✅ Done |
| B9-P0-2 | Wire `record_agent_outcome()` into every subtask execution — online controller now learns from outcomes | `src/acp/impl/request.rs` | ✅ Done |
| B9-P1-1 | Use `get_best_model()` to reorder candidates before execution loop | `src/acp/impl/request.rs` | ✅ Done |
| B9-P1-2 | Check `should_degrade()` before assigning subtask to agent; skip degraded agents with healthy fallback | `src/acp/impl/request.rs` | ✅ Done |
| B9-P2-1 | Remove `#![allow(dead_code)]` from `adaptive_selector.rs` | `src/intelligence/adaptive_selector.rs` | ✅ Done |

---

## Closed Items (blue9 milestones)

Final closure completed on 2026-04-10.

| ID | Item | Status |
|----|------|--------|
| M1 | Bounded channels (backpressure for agent streaming) | ✅ Done |
| M2 | MemoryStore as server-level singleton | ✅ Done |
| M3 | Wire `record_agent_outcome` for review gate outcomes | ✅ Done |
| M4 | FailurePrevention health feedback loop | ✅ Done |
| M5 | Skills interface | ✅ Done |

### M1 — Bounded channels (backpressure for agent streaming)

**Problem:** Three call sites use `mpsc::unbounded_channel::<String>()`, giving the server no backpressure on token streaming. Under a slow consumer and a fast agent, memory grows without bound.

**Root cause:** The `Agent` trait declares `sender: mpsc::UnboundedSender<String>`. Changing the channel type requires changing the trait and all ~40 agent implementations simultaneously.

**Locations:**
- `src/acp/impl/chat.rs:424` — `run_agent_collecting`
- `src/acp/impl/agent.rs:329` — `run_single_review`
- `src/acp/impl/request.rs:2803` — `run_agent_chat_collecting`

**Implementation plan:**
1. Change `Agent::chat` signature in `src/agents/agent.rs:130`:
   ```rust
   sender: mpsc::Sender<String>,  // was UnboundedSender<String>
   ```
2. Update all agent implementations under `src/agents/` (~40 files): replace `sender.send(token)?` patterns — `Sender::send` is async, so every `agent.chat` implementation must become `async` and `await` the send, or use `try_send` with drop-on-full.
3. Update the three call sites to use `mpsc::channel::<String>(2048)`.

**Recommended capacity:** 2048 tokens per channel (covers typical streaming response without truncation).

---

Status: Implemented.

### M2 — MemoryStore as server-level singleton

**Problem:** `MemoryStore::new(MemoryPolicy::default())` is called on every `execute_runtime_subtasks` invocation (line ~2468 in `request.rs`). Each request starts with a fresh empty store — learning observations are never carried across requests.

**Root cause:** `MemoryStore` is not `Send + Sync`, and there is no stable identity for cross-request memory outside of the SQLite/vector stores. Lifting it to `AcpServer` requires deciding retention policy (TTL, max size) and a GC thread.

**Implementation plan:**
1. Add `pub memory_store: Arc<StdMutex<MemoryStore>>` to `AcpServer` in `src/acp/server.rs`.
2. Initialize in `ServerBuilder::build()`:
   ```rust
   memory_store: Arc::new(StdMutex::new(MemoryStore::new(MemoryPolicy::default()))),
   ```
3. Pass `server.memory_store.clone()` into `RuntimeExecutionContext`.
4. In `execute_runtime_subtasks`, use `context.memory_store` instead of creating a new store.
5. Schedule periodic GC in the background task loop (`src/acp/background.rs`).

---

Status: Implemented.

### M3 — Wire `record_agent_outcome` for review gate outcomes

**Problem:** After `run_single_review` / `run_dual_review_gate` returns, the reviewer agent's outcome (pass/fail, latency) is not recorded to `online_controller`. The controller's reliability scores for reviewer agents never update.

**Implementation plan:**
In `src/acp/impl/agent.rs`, pass `Arc<StdMutex<OnlineControllerState>>` as a parameter to `run_single_review`, and record after the reviewer responds:
```rust
if let Ok(mut ctrl) = online_controller.lock() {
    ctrl.record_agent_outcome(phase_name, reviewer_name, approved, duration_ms);
}
```
This requires threading `online_controller` from `AcpServer` through the review gate call chain.

---

Status: Implemented.

### M4 — FailurePrevention health feedback loop

**Problem:** `FailurePrevention::update_health()` and `record_failure()` / `record_success()` are never called from the execution path. Health monitors are never seeded with real data, so `should_degrade()` always returns `false` for unregistered agents.

**Implementation plan:**
In `execute_single_subtask`, after `record_agent_outcome`:
```rust
if let Ok(mut fp) = context.failure_prevention.lock() {
    if run_result.is_err() {
        fp.record_failure(agent_name);
    } else {
        fp.record_success(agent_name);
    }
}
```
Ensure agents are registered at server startup via `fp.register_service(agent_name)`.

---

Status: Implemented.

### M5 — Skills interface

**Problem:** The review described the system as lacking a formal Skills plugin point. There is no `SkillRegistry`, no `SkillHandler` trait, and no way for external tools / MCP servers to register skills that participate in the execution loop.

**Implementation plan:**
1. Define `Skill` trait in `src/orchestration/skill.rs`:
   ```rust
   #[async_trait]
   pub trait Skill: Send + Sync {
       fn name(&self) -> &str;
       async fn execute(&self, input: &str) -> Result<String>;
   }
   ```
2. Add `skill_registry: HashMap<String, Arc<dyn Skill>>` to `AcpServer`.
3. Route `mcp.tools.call` to skill handlers when tool name matches a registered skill.
4. Allow MCP servers to register skills via `server.skill_registry.insert(name, handler)`.

---

## Acceptance Criteria

- All M1-M5 implemented
- 160 unit + 27 ACP integration tests green
- `cargo clippy -- -D warnings` clean
- Agent streaming channels are bounded at capacity `2048`
