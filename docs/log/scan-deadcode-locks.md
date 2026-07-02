# Dead Code & Lock Contention Scan Report

**Date:** 2026-07-02
**Scope:** `src/` (all `.rs` files in orchestration, acp, cli, core, governance, intelligence, memory, observability, optimization, protocol, security, shared, fault_tolerance, multimodal, schema, resilience, agents, i18n, mcp)
**Files scanned:** 492

---

## 1. `#[allow(dead_code)]` Attributes — Items That Should Be Wired or Removed

### ⚠️ `src/acp/helpers/autonomy/execution_intelligence.rs` — Entire Module Dead

| Line | Item | Severity |
|------|------|----------|
| L3 | `#![allow(dead_code)]` — crate-level suppression for the entire module | **HIGH** |
| L16-20 | `EXECUTION_INTELLIGENCE_RECORD_FAILURE_TOTAL` static | **HIGH** |
| L29-33 | `PostCheckOutcome` struct | **HIGH** |
| L35-36 | `WORLD_MODEL` static `OnceLock` | **HIGH** |
| L37-38 | `SELF_MODEL` static `OnceLock` | **HIGH** |
| L41-44 | `metacognitive()` function | **HIGH** |
| L46-49 | `world_model()` function | **HIGH** |
| L51-54 | `self_model()` function | **HIGH** |

**Total: ~280 lines of dead code.** The `pre_check` and `post_check` functions are called only by `#[cfg(test)]` module. The diagnostic comment says "Retained for future wiring when enable_execution_intelligence is active" — this has been deferred code for long enough that it should either be wired into the autonomy loop (likely in `autonomy.rs` or `autonomy_loop.rs`) or deleted. The metacognitive/world-model wiring these functions set up is never used at runtime.

**Recommendation:** Either wire this module into `autonomy.rs` where `pre_check`/`post_check` would be called, or delete the file and the `mod execution_intelligence` declaration in `src/acp/helpers/autonomy/mod.rs`.

---

### ⚠️ `src/acp/impl/chat_phases.rs` — Dead Fields in ThinkOutput

| Line | Item | Severity |
|------|------|----------|
| L95-96 | `preferred_agent_from_request: Option<String>` | **MEDIUM** |
| L104-105 | `enable_high_risk_vote: bool` | **MEDIUM** |

Both fields are populated during the think phase but never read outside `#[cfg(test)]`. The `preferred_agent_from_request` field was part of the agent preference resolution pipeline but the resolved value is consumed through `configured_primary_agent` instead. `enable_high_risk_vote` looks like it was meant to gate the high-risk voting path but the actual gate uses `risk_policy` and `enable_high_risk_multi_agent_vote`.

**Recommendation:** If these fields are for observability/debugging, make them `#[cfg(test)]` or add doc comments explaining why they're retained. Otherwise, remove them.

---

### ⚠️ `src/orchestration/bulkhead.rs` — Dead Method

| Line | Item | Severity |
|------|------|----------|
| L45-55 | `Bulkhead::set_limit()` | **MEDIUM** |

The `set_limit` method allows runtime configuration of per-provider concurrency limits. The comment says "The binary build does not invoke it yet". It's only exercised by `#[cfg(test)]`. The `try_acquire` path uses the `default_limit` always.

**Recommendation:** Either wire `set_limit` into the config hot-reload path (so the admin can change bulkhead limits at runtime), or remove it. The method is small (~9 lines) so the maintenance cost is low, but it's misleading API surface.

---

### ℹ️ `src/security/vulnerability_scan.rs` — Platform-Conditional `#[allow(dead_code)]`

| Line | Item | Severity |
|------|------|----------|
| L898 | `sensitive_patterns: Vec<Regex>` (non-Unix) | **LOW (justified)** |
| L901 | `check_setuid: bool` (non-Unix) | **LOW (justified)** |
| L904 | `check_sticky: bool` (non-Unix) | **LOW (justified)** |

These fields only compile on Unix and are suppressed on other platforms. This is a standard Rust pattern and is **acceptable** — removing these fields would mean non-Unix builds lose structure documentation even if fields are unused.

---

### ℹ️ `src/schema/mod.rs` — Re-export `#[allow(unused_imports)]`

| Line | Item | Severity |
|------|------|----------|
| L31-40 | `pub use agent::*; pub use client::*;` etc. | **LOW (acceptable)** |

These are public API re-exports. The `#[allow(unused_imports)]` is needed because the current binary (`main.rs`) doesn't consume some of these types directly. As an SDK/schema module, these are the intended public surface for downstream consumers. **Leave as-is.**

---

## 2. Lock Contention / Unnecessary Locks

### ⚠️ `src/resilience/hyper_resilience.rs` — Sequential Lock Acquisition

**Issue:** `system_health()` and `profile()` acquire multiple independent locks sequentially, each blocking while the previous lock is released. This increases contention on hot paths.

**`system_health()` (lines 608-643) — 4 lock acquisitions:**
1. `lock_mutex(&self.circuit_breakers)` → `drop(cbs)`
2. `lock_mutex(&self.test_avg_latency_ms)` → auto-dropped
3. `lock_mutex(&self.test_error_rate)` → auto-dropped
4. `lock_mutex(&self.failover_groups)` → `drop(fgs)`

**`profile()` (lines 830-852) — 2 lock acquisitions:**
1. `lock_mutex(&self.circuit_breakers)` → `drop(cbs)`
2. `lock_mutex(&self.failover_groups)` → `drop(fgs)`

**Analysis:** Each lock is held for a brief period (HashMap lookups), so in practice contention is low. However, the repeated lock acquire/release pattern wastes CPU cycles. Since `circuit_breakers` and `failover_groups` are never accessed together under a single lock, they could be restructured as a single struct behind one `Mutex`, or at minimum the methods should document why they don't hold both simultaneously.

**`execute_healing()` (lines 649-827) — sequential lock patterns across branches:**
- `ClearCircuitBreaker`: acquires `circuit_breakers` mutex
- `PromoteReplica`: acquires `failover_groups` mutex
- `RestartNode`: acquires `circuit_breakers` first, then `failover_groups` (lines 720-735)
- `ScaleResources`: acquires `failover_groups` mutex
- `ReinitializeComponent`: acquires `circuit_breakers` mutex

**Deadlock risk:** `RestartNode` acquires `circuit_breakers` → `failover_groups`. If any future code path acquires them in reverse order (`failover_groups` → `circuit_breakers`), a deadlock occurs. The existing docs in `is_available()` (line 469-477) show the author was aware of this risk but only documented it for that one method.

**Recommendation:** Consolidate `circuit_breakers` and `failover_groups` into a single struct behind one `Mutex`. This eliminates both the sequential acquisition cost and the deadlock risk, at the cost of slightly reduced read/write concurrency between the two maps (which are already both behind exclusive `Mutex` anyway, so no `RwLock` benefit is being lost).

---

### ⚠️ `src/acp/prelude/inflight.rs` — Useless Default Implementation

| Line | Issue | Severity |
|------|-------|----------|
| L43-50 | `InflightGuard::default()` creates `Arc::new(InflightLimiter::default())` | **MEDIUM** |

The `Default` impl creates a standalone `InflightLimiter` with `max_inflight: 0` (unlimited) and wraps it in a new `Arc`. This guard is never connected to a real shared limiter, so `try_enter()` would always succeed and `leave()` would decrement a private counter. Since `InflightGuard` has a `Drop` impl that calls `leave()`, an accidentally-default-constructed guard would corrupt its own internal count but not affect anything else.

**Recommendation:** Remove the `Default` impl for `InflightGuard`. No legitimate use case exists for a guard that doesn't reference a shared limiter. If one exists for testing, add `#[cfg(test)]`.

---

## 3. Redundant Clones / Unnecessary Arcs

### ℹ️ `src/i18n/watcher.rs:203` — Double-Arc Wrapper

```rust
Arc::new((*mgr).clone())
```

**Issue:** `I18N` stores `RwLock<Option<I18nManager>>`. Dereferencing `*mgr` gets the `I18nManager`, cloning it (which is `Arc::clone(&self.inner)` — cheap), and then wrapping in a new `Arc`. This creates `Arc<I18nManager<Arc<I18nInner>>>` — two levels of Arc indirection.

**Impact:** Very low. The outer Arc adds one heap allocation and one word of overhead. The inner sharing via `Arc<I18nInner>` is correct.

**Alternative:** Either have `I18N` store `Arc<I18nManager>` directly, or have `start_watcher` take ownership of the `I18nManager` directly instead of wrapping in another Arc.

---

## 4. Files with No Significant Issues (Noted Clean)

The following modules were scanned and found to have no dead code, no problematic `#[allow(dead_code)]`, and no unnecessary locks or performance drags worth reporting:

- `src/cli/` — `mod.rs`, `chat.rs`, `markdown_renderer.rs`: clean
- `src/core/` — bootstrap, config, error, providers, setup: clean
- `src/governance/` — approval_engine, drift, hardening, harness_bus, pua, rationalization, rbac, reloadable_policy, review_controls, runtime_controls, security_governor, status, audit, approval_learning: clean
- `src/intelligence/` — all capability_bus modules, metacognitive, self_model, world_model, reinforcement, token_cache: clean
- `src/memory/` — all modules: clean
- `src/observability/` — all modules: clean
- `src/optimization/` — failure_prevention: clean
- `src/protocol/` — all modules: clean
- `src/security/` — audit_integrity, content_safety, mtls, prompt_injection, rate_limiter, request_signing, secret_rotation, security_advisor: clean (except vulnerability_scan noted above)
- `src/shared/` — all modules: clean
- `src/fault_tolerance/` — detector, recovery, types: clean
- `src/multimodal/` — all modules: clean
- `src/schema/` — agent, client, content, mcp, skills: clean (except mod.rs re-exports noted above)
- `src/agents/` — all 30+ agent implementations: clean
- `src/mcp/` — handlers, schema, tools: clean
- `src/orchestration/` — most files; only `bulkhead.rs` flagged above
- `src/acp/` — most files; only the three issues flagged above
- `src/resilience/` — chaos.rs clean; hyper_resilience.rs flagged above
- `src/i18n/` — runtime.rs clean; watcher.rs flagged above (minor)

---

## Summary Table

| # | File | Line(s) | Category | Severity | Description |
|---|------|---------|----------|----------|-------------|
| 1 | `src/acp/helpers/autonomy/execution_intelligence.rs` | L3 | Dead code | **HIGH** | Entire module suppressed; ~280 lines of dead code including world model/self-model/metacognitive wiring that is never used in production |
| 2 | `src/acp/impl/chat_phases.rs` | L95-96 | Dead code | **MEDIUM** | `preferred_agent_from_request` field in `ThinkOutput` is populated but never read |
| 3 | `src/acp/impl/chat_phases.rs` | L104-105 | Dead code | **MEDIUM** | `enable_high_risk_vote` field in `ThinkOutput` is populated but never read |
| 4 | `src/orchestration/bulkhead.rs` | L45 | Dead code | **MEDIUM** | `Bulkhead::set_limit()` never called in production code; only in tests |
| 5 | `src/resilience/hyper_resilience.rs` | L608-643 | Lock contention | **MEDIUM** | `system_health()` acquires 4 separate locks sequentially; `profile()` acquires 2 |
| 6 | `src/resilience/hyper_resilience.rs` | L720-735 | Deadlock risk | **LOW** | `execute_healing(RestartNode)` acquires `circuit_breakers` → `failover_groups`; reverse order in a future code path would deadlock |
| 7 | `src/acp/prelude/inflight.rs` | L43-50 | Useless default | **LOW** | `InflightGuard::default()` creates orphaned `Arc<InflightLimiter>` that is never connected to the shared limiter |
| 8 | `src/i18n/watcher.rs` | L203 | Redundant Arc | **LOW** | `Arc::new((*mgr).clone())` creates double-Arc wrapper; adds one unnecessary heap allocation |

**Total real issues:** 4 meaningful (HIGH/MEDIUM) items that should be addressed
**Total minor issues:** 3 items worth monitoring
**Unnecessary changes to avoid:** The `#[allow(dead_code)]` on `PermitExposureAnalyzer` (platform-conditional) and schema re-exports are justified.

### Priority Recommendations

1. **HIGH — `execution_intelligence.rs`:** Either wire it into the autonomy loop or delete it. 280 lines of dead module-level suppressed code is a maintenance liability.
2. **MEDIUM — `chat_phases.rs`:** Drop the two dead `ThinkOutput` fields or add a documented reason for keeping them.
3. **MEDIUM — `bulkhead.rs`:** Wire `set_limit` into config hot-reload or remove the method.
4. **MEDIUM — `hyper_resilience.rs`:** Consolidate `circuit_breakers` + `failover_groups` into a single struct behind one `Mutex` to eliminate 4 sequential lock acquisitions in `system_health()`.
