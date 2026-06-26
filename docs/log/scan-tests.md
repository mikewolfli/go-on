# Test Scan Report

**Date:** 2026-06-26  
**Scope:** `go-on/tests/`, `go-on/src/` (all `.rs` test modules)  
**Scanned by:** Zed coding agent

---

## 1. `#[ignore]` Attributes

**No `#[ignore]` attributes found on any test function.**

- `tests/e2e_tests.rs` (line 8) has a comment noting that `#[ignore]` is *not* needed because tests use in-memory construction, but no test actually carries the attribute.
- All test files are **active** — none have been explicitly ignored.

---

## 2. Empty Test Bodies or `assert!(true)`

**None found.**

- Grep for `assert!(true)` returned zero matches across all `.rs` files.
- All test functions contain meaningful assertions against real types or workflows.

---

## 3. `unimplemented!()` or `todo!()`

**None found inside any test function or test helper.**

- The only occurrences of `todo!()` / `unimplemented!()` in the codebase are inside `src/intelligence/verification.rs`, where they are used **as scanned patterns** (the verifier checks whether source code contains these macros). These are not actual invocations in test paths.

---

## 4. `#[allow(dead_code)]` Helpers That Are Truly Unused

| File | Function | Status |
|------|----------|--------|
| `src/intelligence/capability_bus/distributed_memory_bus.rs:1150` | `make_bus_with_peers()` | **TRULY UNUSED.** Gated behind `#[cfg(feature = "multi-users-server")]` but never called anywhere in the test module. The regular `make_bus()` is used instead. |
| `src/orchestration/tool/extended/geo.rs:257` | `test_input()` | **FALSE POSITIVE.** Actually used by `distance_calculation`, `centroid_calculation`, `bounding_box_calculation` tests. `#[allow(dead_code)]` is unnecessary. |
| `src/orchestration/tool/extended/gltf.rs:192` | `test_input()` | **FALSE POSITIVE.** Actually used by multiple tests. `#[allow(dead_code)]` is unnecessary. |
| `src/orchestration/tool/extended/iges.rs:278` | `test_input()` | **FALSE POSITIVE.** Actually used by `parse_minimal_iges`, `parse_empty_section` tests. `#[allow(dead_code)]` is unnecessary. |
| `src/orchestration/tool/extended/obj.rs:247` | `test_input()` | **FALSE POSITIVE.** Actually used by multiple tests. `#[allow(dead_code)]` is unnecessary. |
| `src/orchestration/tool/extended/ply.rs:247` | `test_input()` | **FALSE POSITIVE.** Actually used by multiple tests. `#[allow(dead_code)]` is unnecessary. |
| `src/orchestration/tool/extended/stl.rs:625` | `tool_input()` | **FALSE POSITIVE.** Actually used by multiple tests. `#[allow(dead_code)]` is unnecessary. |
| `src/security/mod.rs:30` | `get_content_safety_checker()` | Production API — convenience accessor for a global singleton. Not currently called but intended for feature-gated consumers. |
| `src/security/mod.rs:70` | `get_prompt_injection_detector()` | Production API — same pattern as above. |
| `src/security/mtls.rs:251` | `with_client_cert()` | Builder method for `#[cfg(feature = "multi-users-server")]`. Used via feature gate only. |

**Actionable finding:** `make_bus_with_peers()` in `distributed_memory_bus.rs:1150` is dead code and should be removed or a test should be written that exercises it.

**Cleanup suggestion:** The six `test_input()` / `tool_input()` helpers in geo/gltf/iges/obj/ply/stl carry `#[allow(dead_code)]` but are all actively used. Remove the redundant `#[allow(dead_code)]` attribute (or replace with `#[expect(dead_code)]` if using a modern Rust edition).

---

## 5. Manual Tokio Runtime + `block_on` (Not Using `#[tokio::test]`)

| File | Line(s) | Issue |
|------|---------|-------|
| `tests/chaos_drill.rs` | 16–18, 41–43, 55–57, 105–107 | **4 tests** use `tokio::runtime::Runtime::new().block_on(...)`. These all live inside `mod chaos_drill_tests` with `#[test]` attributes. Could be converted to `#[tokio::test]`. |
| `src/acp/transport_factory.rs` | 377–379 | Test `dispatch_server_unsupported_protocol_returns_error` uses manual runtime. |
| `src/agents/self_evolution_agent.rs` | 1200–1202 | Helper `create_test_agent()` uses manual runtime (but sibling `create_test_agent_async()` exists for `#[tokio::test]` callers). |
| `src/intelligence/reinforcement/federated_transport.rs` | 822–824 | Test `test_in_process_transport_no_callbacks` uses manual runtime. |
| `src/memory/memory_bridge.rs` | 203 | Test `test_background_task_cancellation` uses manual runtime. |
| `src/orchestration/self_evolution/evolution_history.rs` | 740–742, 778–780, 800–802, 824–826 | **4 tests** (`test_evolution_history_find_by_trigger`, `test_evolution_history_failed_entries`, `test_get_metrics_trend`, `test_rolled_back_entries`) each manually create a runtime. |
| `tests/comprehensive_feature_benchmark.rs` | 487–489 | Helper `measure_auto_recovery()` (not a `#[test]` function, so this is less concerning — it's a measurement helper called by real tests). |

**Actionable:** All locations in `tests/chaos_drill.rs` and inline in `src/` test modules could be migrated to `#[tokio::test]`. The chaos drill file (`drill_network_resilience`, `drill_storage_resilience`, `drill_resource_exhaustion`, `drill_custom_scenario`) is the most impactful candidate since it is the only file in `tests/` that still uses this pattern.

---

## 6. `#[cfg(test)]` / `#![cfg(test)]` Modules Containing Substantial Non-Test Code

**No violations found.**

- `tests/acp_runtime_rpc_integration.rs` uses `#![cfg(test)]`, but since the file lives in `tests/`, it is **only compiled for tests anyway** — the attribute is redundant but not harmful.
- All `#[cfg(test)] mod tests` blocks in `src/` contain only test helpers and test functions. No structs, impls, or functions that should be in production code were found inside test modules.
- `tests/common/mod.rs` is shared test utility code (harnesses, locks, helpers) — appropriate for its location outside `src/`.

---

## 7. Empty or Stub Test Files

**None found.**

Every `.rs` test file scanned contains meaningful, non-trivial test functions. There are no completely empty files, no `fn test_placeholder() {}` stubs, and no files consisting solely of comments.

---

## 8. Contract Tests

**`tests/contract_tests/` exists and is meaningful.**

| File | Content |
|------|---------|
| `tests/contract_tests/mod.rs` | Module declaration only; re-exports `resilience_contract`. |
| `tests/contract_tests/resilience_contract.rs` | **6 contract tests** testing: circuit breaker open/close/half-open transitions, success-reset, self-healing recovery, degradation escalation, and resilience profile config defaults. |

The contract tests are **meaningful** — they exercise `CircuitBreaker`, `HyperResilienceEngine`, `SystemHealth`, `ResilienceProfile`, `DegradationLevel`, and other types from `go_on::resilience::hyper_resilience`. Each test validates a specific behavioral contract with multiple assertions. They are structurally well-organized.

---

## Summary of Issues Found

| Priority | Finding | Location | Action |
|----------|---------|----------|--------|
| 🔴 **High** | `make_bus_with_peers()` is dead code | `src/intelligence/capability_bus/distributed_memory_bus.rs:1150` | Remove function or add a test that uses it |
| 🟡 **Medium** | 4 tests use manual `Runtime::new().block_on()` | `tests/chaos_drill.rs` | Convert to `#[tokio::test]` |
| 🟡 **Medium** | 7 tests in `src/` sub-modules use manual `Runtime::new().block_on()` | Various `src/` files (see §5) | Convert to `#[tokio::test]` |
| 🟢 **Low** | Redundant `#[allow(dead_code)]` on used test helpers | 6 files (geo/gltf/iges/obj/ply/stl) | Remove `#[allow(dead_code)]` |
| 🟢 **Low** | Redundant `#![cfg(test)]` in integration test | `tests/acp_runtime_rpc_integration.rs:20` | Remove attribute (tests/ is only compiled for tests) |
| ✅ **Clean** | No `#[ignore]`, no empty tests, no `todo!()`/`unimplemented!()` in tests | — | — |
| ✅ **Clean** | Contract tests are substantive and well-structured | `tests/contract_tests/` | — |
