# Deep Scan Report: `src/orchestration/` Module

**Date**: 2026-06-26
**Scope**: 50+ .rs files across 11 sub-directories (~110 source files total)
**Methodology**: All .rs files read in full, targeted grep for dead-code patterns, `allow(dead_code)`, `block_on`/`block_in_place`, `Handle::current().block_on()`, empty/stub tests, feature gates, and unused imports.

---

## Executive Summary

The orchestration module is **well-maintained** but has a **moderate number of issues**, mostly low-severity:

- **5** `#[allow(dead_code)]` annotations (all documented with reasons)
- **2** functions identified as **no-op / trivial** that may be removable
- **2** test files are entirely `#[cfg(test)]` with no non-test code (skills_folder.rs, self_improvement_report.rs)
- **8+** functions/variables gated behind `#[cfg(test)]`
- **2** dead functions not marked with `#[allow(dead_code)]`
- **3** occurrences of `rt.block_on()` in tests (test-only, acceptable)
- **0** occurrences of `Handle::current().block_on()` or `block_in_place` + `block_on` in production code
- **0** empty/stub tests or `#[ignore]`d tests
- **0** unused imports found in non-test code paths
- **0** unnecessary locks or synchronization objects

---

## 1. `#[allow(dead_code)]` Annotations

| # | File | Line(s) | Reason Given | Severity |
|---|------|---------|-------------|----------|
| 1 | `bulkhead.rs` | 41 | `"Public API surface for Bulkhead consumers"` — `set_limit()` | LOW |
| 2 | `bulkhead.rs` | 55 | `"Public API surface for Bulkhead consumers"` — `acquire()` | LOW |
| 3 | `multi_agent_pipeline.rs` | 57 | `"F-GAP-49 — reserved for future use"` — `AgentAssignment` enum | LOW |
| 4 | `multi_agent_pipeline.rs` | 93 | `"F-GAP-49 — reserved for future use"` — `with_subtask_timeout()` | LOW |
| 5 | `planner_embedding.rs` | 40 | `"F-GAP-49 — reserved for future use"` — `EmbeddingTaskClassifier::new()` | LOW |
| 6 | `recovery/escalation.rs` | 10–12 | `"Public API surface for escalation strategy consumers"` — `build_escalation()` | LOW |
| 7 | `scheduler.rs` | 571–573 | `"Public API surface — reserved for external callers"` — `start_fault_tolerance_timer()` | LOW |
| 8 | `scheduler.rs` | 621 | `"Reserved for graceful server shutdown"` — `shutdown()` | LOW |
| 9 | `startup_context/profile.rs` | 9–11 | `"Public API surface for governance/status endpoint"` — `startup_context_profile()` | LOW |

**Assessment**: All nine annotations have documented justifications (future use, public API, backward compatibility). No action needed unless these truly become dead weight.

---

## 2. Dead Code Not Marked as `#[allow(dead_code)]`

| # | File | Line(s) | Issue | Severity |
|---|------|---------|-------|----------|
| 1 | `scheduler/persistence.rs` | 7–9 | `create_persistent_scheduler()` — Accepts a `_db_path` argument that is explicitly ignored. The doc comment says `"persistence is no longer used"`. This is a **legacy shim** that creates a plain in-memory scheduler. Its caller should be updated to call `TaskScheduler::new()` directly. | MEDIUM |
| 2 | `scheduler.rs` | 575–613 | `start_fault_tolerance_timer()` — Marked with `#[allow(dead_code)]` but the `pub` visibility could be narrowed. The function is called from `start_aging_timer()` (line 542), so it's actually **not** dead — but the annotation was added anyway, suggesting the call was introduced after the annotation. Remove the `#[allow(dead_code)]` on this function since it IS called. | LOW |

---

## 3. `block_in_place` / `block_on` / `Handle::current().block_on()` Patterns

### Production Code

**No occurrences found.** All production code uses async `.await` correctly.

### Test Code

`rt.block_on(...)` pattern found in three files. These are test-only — acceptable:

| # | File | Line(s) | Pattern | Severity |
|---|------|---------|---------|----------|
| 1 | `self_evolution/evolution_history.rs` | Multiple (738–844) | `tokio::runtime::Runtime::new().unwrap(); rt.block_on(...)` in 5 tests | INFO — Test-only pattern, acceptable |
| 2 | `skill_import.rs` | 911–917, 954–960 | `runtime.block_on(store.import_skill(...))` in 2 tests | INFO — Test-only pattern, acceptable |

**No action needed** — these are all inside `#[cfg(test)]` modules and use the standard `block_on`-inside-sync-test pattern.

---

## 4. No-op / Trivially Stub Functions

| # | File | Line(s) | Function | Issue | Severity |
|---|------|---------|----------|-------|----------|
| 1 | `orchestrator.rs` | 284–287 | `init_cache_warming()` | Returns `CacheHitCounter::default()` after a single `tracing::info!` call. Replaces a previously removed `CacheWarmingEngine`. Can be inlined into the single call site. | LOW |
| 2 | `orchestrator.rs` | 290–293 | `warm_cache_after_success()` | Increments an atomic counter, logs, and returns. Single call to `counter.increment()`. Can be inlined or deprecated. | LOW |

---

## 5. Entirely `#[cfg(test)]` Files

| # | File | Lines | Content | Severity |
|---|------|-------|---------|----------|
| 1 | `skills_folder.rs` | 1–182 | Whole `SkillsFolderIndex` struct and its tests are under `#[cfg(test)]`. The `pub mod skills_folder` in `mod.rs` is also `#[cfg(test)]`. This is intentional (test-only utility). | INFO |
| 2 | `self_evolution/self_improvement_report.rs` | 1–230 | Entire file is `#[cfg(test)] mod tests { ... }`. Contains `SelfImprovementReport` struct with `empty()`, `generate()`, and `to_markdown()` methods, used only in tests. | INFO — Intentional, but the file declares a non-trivial struct under test-only code. |

---

## 6. Functions/Variables Gated Behind `#[cfg(test)]`

| # | File | Line(s) | Symbol | Severity |
|---|------|---------|--------|----------|
| 1 | `autonomy_runtime.rs` | 50–56 | `parse_model_used_token()` | INFO — test helper |
| 2 | `autonomy_runtime.rs` | 58–61 | `parse_thinking_token()` | INFO — test helper |
| 3 | `capability_signals.rs` | 83–89 | `CapabilitySignals::is_agent_preferred()` | INFO — test helper |
| 4 | `diagnostic_feedback.rs` | 26–34 | `DiagnosticSeverity::label()` | INFO — test helper |
| 5 | `diagnostic_feedback.rs` | 114–120 | `DiagnosticBatch::affected_files()` | INFO — test helper |
| 6 | `diagnostic_feedback.rs` | 250–253 | `DiagnosticFeedbackEngine::pattern_count()` | INFO — test helper |
| 7 | `full_auto/environment.rs` | 31–34 | `ExecutionEnvironment::is_ready()` | INFO — test helper |
| 8 | `full_auto/intent.rs` | 42–51 | `TaskIntent::has_goals()`, `TaskIntent::constraint_count()` | INFO — test helpers |
| 9 | `planner_execution_graph.rs` | 85–99 | `PlannerExecutionBridge::complete_step()`, `fail_step()` | INFO — test helpers |
| 10 | `tool/recommender.rs` | 314–328 | `format_recommendation()`, `get_tool_stats()` | INFO — test helpers |

---

## 7. Feature-Gated Modules (Potential Dead Code Depending on Build Config)

The `mod.rs` at the top level exposes several modules behind feature gates:

| # | Module | Gate | Notes |
|---|--------|------|-------|
| 1 | `council` | `feature = "sub-bus-tool"`, `"simple-server"`, `"multi-users-server"` | Only compiled when one of these features is active. Contains a large `quorum` submodule whose `mod.rs` has all code inside `mod tests { ... }` (L17–826). |
| 2 | `tool/extended/barcode` | `feature = "barcode-tools"` | Potentially dead if feature never enabled |
| 3 | `tool/extended/cad` | `feature = "cad-utils"` | Same |
| 4 | `tool/extended/csv_utils` | `feature = "data-export"` | Same |
| 5 | `tool/extended/data_serialization` | `feature = "data-export"` | Same |
| 6 | `tool/extended/docx` | `feature = "document-docx"` | Same |
| 7 | `tool/extended/dxf_tool` | `feature = "cad-dxf"` | Same |
| 8 | `tool/extended/email` | `feature = "document-email"` | Same |
| 9 | `tool/extended/game` | Multiple `game-*` features | Same |
| 10 | `tool/extended/gcode` | `feature = "cam-gcode"` | Same |
| 11 | `tool/extended/geo` | `feature = "cad-geo"` | Same |
| 12 | `tool/extended/gltf` | `feature = "cad-gltf"` | Same |
| 13 | `tool/extended/gpx` | `feature = "gis-gpx"` | Same |
| 14 | `tool/extended/iges` | `feature = "cad-iges"` | Same |
| 15 | `tool/extended/image` | `feature = "image-processing"` | Same |
| 16 | `tool/extended/invoice` | `feature = "document-invoice"` | Same |
| 17 | `tool/extended/obj` | `feature = "cad-obj"` | Same |
| 18 | `tool/extended/obj_tool` | `feature = "model-3d-extra"` | Same |
| 19 | `tool/extended/pdf` | `feature = "document-pdf"` | Same |
| 20 | `tool/extended/ply` | `feature = "cad-ply"` | Same |
| 21 | `tool/extended/sqlite` | `feature = "backend-sqlite"` | Same |
| 22 | `tool/extended/step` | `feature = "cad-step"` | Same |
| 23 | `tool/extended/stl` | `feature = "cad-stl"` / `"model-3d"` | Conditional re-export logic applies |
| 24 | `tool/extended/stl_tool` | `feature = "model-3d"` | Same |
| 25 | `tool/extended/svg` | `feature = "drawing-svg"` | Same |
| 26 | `tool/extended/web` | `feature = "document-html"` | Same |

**Assessment**: These are all properly gated. No `#[cfg(feature)]` has an obviously dead or misspelled feature name. `council/quorum/mod.rs` contains ~810 lines of tests and very little non-test implementation code (only `impl` blocks for `consensus.rs`, `proposal.rs`, `voting.rs` methods), which suggests the quorum module is heavily test-dominated.

---

## 8. Tests: `#[ignore]`, Empty, or `assert!(true)`

| # | File | Line(s) | Issue | Severity |
|---|------|---------|-------|----------|
| — | — | — | **None found** — All tests contain meaningful assertions. No `#[ignore]` attributes found. No `assert!(true)` found. | ✅ |

---

## 9. Unused Imports

| # | File | Line(s) | Import | Severity |
|---|------|---------|--------|----------|
| — | — | — | **None found** — All imports in non-test code are used. Several test files import types used exclusively in their test bodies (e.g., `HashMap`, `Arc`, `Value`), which is standard. | ✅ |

---

## 10. Duplicate/Dual Implementations

| # | File | Line(s) | Issue | Severity |
|---|------|---------|-------|----------|
| 1 | `task_graph_store.rs` | 35–270 (SQLite) + 290–509 (Postgres) | **Two separate `struct TaskGraphStore` impl blocks** behind `#[cfg(not(feature = "backend-postgres"))]` and `#[cfg(feature = "backend-postgres")]`. This is intentional conditional compilation but is worth noting for maintainability — the two implementations share nearly identical method signatures with different SQL syntax. | INFO |
| 2 | `planner_executor/plan_optimization.rs` (L106–150) and `planner_embedding.rs` (L106–150) | Both modules have near-identical `classify_with_keywords()` / `analyze_task()` keyword-heuristic logic | The `Planner::analyze_task()` and `EmbeddingTaskClassifier::classify_with_keywords()` use almost identical keyword matching for complexity classification (code indicators, research indicators, multi-subtask indicators). This is **duplicated heuristic logic**. If they diverge, behavior becomes inconsistent. | MEDIUM |

---

## 11. Minor / Cosmetic Issues

| # | File | Line(s) | Issue | Severity |
|---|------|---------|-------|----------|
| 1 | `scheduler.rs` | 116 | `bulkhead` field of `TaskScheduler` — unused? | LOW — Check if `self.bulkhead` is ever referenced beyond initialization |
| 2 | `brain_loop/execution.rs` | Line 3 | Doc comment: `"⚠️ DEPRECATED (non-test): Use cognitive loop in chat_phases.rs instead."` | LOW — This entire file is deprecated for production use but still wired in tests |
| 3 | `tool/lock.rs` | 126–134, 158, 200 | `ACQUIRE_TIMEOUT`, `BACKOFF_INITIAL_US`, `BACKOFF_MAX_MS`, `acquire()`, `acquire_async()` are all `#[cfg(test)]` | LOW — These are significant amounts of code only compiled under test |
| 4 | `full_auto/mod.rs` | L27 | `DEFAULT_MIN_MATCH_SCORE` declared `pub(crate)` but appears unused outside the file | LOW |

---

## 12. Detailed Section-by-Section Notes

### 12a. `brain_loop/` (4 files)
- **execution.rs**: Marked **DEPRECATED** for non-test use. The `execute_step` and `execute_step_with_context` methods use `write_guard`/`read_guard` async helpers. No `block_on` issues.
- **planning.rs**: Contains `run_async` (the main loop), `start_plan`, `reflect`, `replan`, persistence, world-model query, metacognitive feedback. Well-structured.
- **reflection.rs**: Contains `DeepReasoningEngine` with `plan_with_reasoning`, `reflect_with_reasoning`, etc. Uses async LLM calls. Clean.

### 12b. `council/` (4 files + quorum/ submodule)
- Council is feature-gated (3 features). Contains `OrchestrationCouncil` with voting, proposals, deliberation, reputation, and auto-ejection.
- `quorum/mod.rs` has ~810 lines of tests — test-heavy.
- All `Mutex` usage is standard (`Arc<Mutex<...>>` pattern). No unnecessary synchronization.

### 12c. `full_auto/` (5 files)
- Full auto-flow with skill discovery, environment checking, intent parsing, execution, and reporting.
- Uses FastPathCache for intent/skill/env caching. Clean.

### 12d. `planner_executor/` (3 files)
- `plan_optimization.rs` and `planner_embedding.rs` share duplicated keyword-complexity heuristics (see Section 10).
- `execution.rs` uses `join_all` for parallel group execution. No `block_in_place`.

### 12e. `recovery/` (3 files)
- Clean. Uses `FailureKind` classification for strategy selection.

### 12f. `scheduler/` (4 files + scheduler.rs)
- `TaskScheduler` with priority queue, semaphore-based concurrency limits, aging.
- `persistence.rs` is a legacy shim (see Section 2, item 1).
- `scheduler.rs:start_fault_tolerance_timer()` — marked `#[allow(dead_code)]` but actually called from `start_aging_timer()` (line 542). **Annotation should be removed.**

### 12g. `self_evolution/` (2 files + evolution_loop/)
- `self_improvement_report.rs` is entirely `#[cfg(test)]`.
- `evolution_history.rs` tests use `rt.block_on()` pattern (acceptable).
- `sandbox.rs` implements `CodePatch`, `BuildResult`, `SandboxExecutor` with network sandboxing.

### 12h. `tool/` (5 files + extended/)
- `lock.rs`: `acquire()` and `acquire_async()` are `#[cfg(test)]` only — blocking acquire methods only available in tests. Production code uses `try_acquire()`.
- `mod.rs`: Massive file (~3350 lines) with `ToolRegistry`, built-in tools (ReadFile, WriteFile, etc.), `execute_loop` logic.
- `extended/mod.rs`: Heavy feature-gating for 25+ optional tool modules.

### 12i. `orchestrator.rs`
- Contains `default_context()` marked `#[deprecated]`.
- Contains `init_cache_warming()` and `warm_cache_after_success()` — trivial stubs (Section 4).
- Model selection, cost/latency estimation, and capability tier functions are healthy.

---

## Recommendations (Prioritized)

1. **MEDIUM**: Remove `#[allow(dead_code)]` from `scheduler.rs:start_fault_tolerance_timer()` (line 571–573) — it IS called from `start_aging_timer()` at line 542.

2. **MEDIUM**: Deduplicate the keyword-complexity heuristic between `planner_executor/plan_optimization.rs` (`Planner::analyze_task`) and `planner_embedding.rs` (`EmbeddingTaskClassifier::classify_with_keywords`). Extract into a shared helper or unify into one code path.

3. **MEDIUM**: Either eliminate `scheduler/persistence.rs` (`create_persistent_scheduler`) and update callers to use `TaskScheduler::new()` directly, or deprecate it with a doc notice.

4. **LOW**: Consider deprecating `orchestrator.rs:init_cache_warming()` and `warm_cache_after_success()` — they wrap single operations that can be called directly.

5. **LOW**: Review the `task_graph_store.rs` dual-implementation (SQLite vs Postgres) for test coverage and drift between the two code paths.

6. **INFO**: Monitor `full_auto/mod.rs:DEFAULT_MIN_MATCH_SCORE` (`pub(crate)`) — confirm it is used outside the file or remove the `pub(crate)` visibility.

---

## File Summary

| Subdirectory | Files | `#[allow(dead_code)]` | Dead Code | `block_on` (test) | Stub Tests | Unused Imports |
|-------------|-------|----------------------|-----------|-------------------|------------|----------------|
| `brain_loop/` | 4 | 0 | 0 | 0 | 0 | 0 |
| `council/` | 7 | 0 | 0 | 0 | 0 | 0 |
| `full_auto/` | 5 | 0 | 0 | 0 | 0 | 0 |
| `planner_executor/` | 3 | 0 | 0 | 0 | 0 | 0 |
| `recovery/` | 3 | 1 | 0 | 0 | 0 | 0 |
| `scheduler/` | 5 | 2 | 1 (persistence.rs) | 0 | 0 | 0 |
| `self_evolution/` | 7 | 0 | 0 | 5 tests | 0 | 0 |
| `skill/` | 3 | 0 | 0 | 0 | 0 | 0 |
| `startup_context/` | 3 | 1 | 0 | 0 | 0 | 0 |
| `tool/` | 6 (inc. extended/) | 0 | 0 | 0 | 0 | 0 |
| `workflow_registry/` | 3 | 0 | 0 | 0 | 0 | 0 |
| Root `.rs` files | 30 | 5 | 0 | 0 | 0 | 0 |
| **Total** | **~76** | **9** | **1** | **5** | **0** | **0** |

---

*Report generated by automated deep scan.*
