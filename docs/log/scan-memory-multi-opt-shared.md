# Deep Scan Report: memory / multimodal / optimization / shared modules

**Date:** 2026-06-26  
**Scope:** All `.rs` source files under `src/memory/`, `src/multimodal/`, `src/optimization/`, `src/shared/`

---

## Table of Contents

1. [Module: memory/](#1-module-srcmemory)
2. [Module: multimodal/](#2-module-srcmultimodal)
3. [Module: optimization/](#3-module-srcoptimization)
4. [Module: shared/](#4-module-srcshared)
5. [Cross-cutting Patterns](#5-cross-cutting-patterns)

---

## 1. Module: `src/memory/`

### Files analyzed (13)

```
src/memory/mod.rs
src/memory/memory.rs
src/memory/agent_memory_bus.rs
src/memory/cache.rs
src/memory/embedding_provider.rs
src/memory/memory_bridge.rs
src/memory/memory_persistence.rs
src/memory/memory_response_cache.rs
src/memory/memory_retrieval.rs
src/memory/semantic_cache.rs
src/memory/summarization.rs
src/memory/vector.rs
src/memory/vector_index.rs
```

### 1.1 Dead / Unused Code

| # | File | Line(s) | Severity | Issue |
|---|------|---------|----------|-------|
| 1 | `memory/embedding_provider.rs` | 21–25 | LOW | `#[allow(dead_code)]` on `EmbeddingProvider::expected_dimension()` — trait method annotated dead_code because callers may not call it. Official justification: "Public API — trait method reserved for callers who need to validate output dimensionality". |
| 2 | `memory/memory_persistence.rs` | 1475 | LOW | `COLD_COUNT_WARN: Once` — static `Once` used only to emit a one-time warning; functional but could be simplified to `std::sync::OnceLock` or simply removed when cold count tracking is implemented. |
| 3 | `memory/memory_persistence.rs` | 1200 | LOW | `let seq = self.sequence.fetch_add(1, Ordering::Relaxed); let _ = seq;` — sequence is atomically incremented but the value is immediately discarded (commented "available for future ordering needs"). Results in an unnecessary atomic increment on every `store()` call. |

### 1.2 `#[allow(dead_code)]` Annotations

| # | File | Line(s) | Severity | Issue |
|---|------|---------|----------|-------|
| 4 | `memory/embedding_provider.rs` | 21–24 | LOW | `#[allow(dead_code)]` on `expected_dimension()` in trait `EmbeddingProvider`. The annotation itself isn't inherently bad, but the method exists only for future callers. |

### 1.3 `block_in_place` / `block_on` / `Handle::current().block_on()` Patterns

**None found** in the memory module. All I/O is synchronous and uses `spawn_blocking` (in `semantic_cache.rs` line 272) or `reqwest::blocking`, which is the correct pattern.

### 1.4 Empty / Stub / No-op Implementations

| # | File | Line(s) | Severity | Issue |
|---|------|---------|----------|-------|
| 5 | `memory/memory_persistence.rs` | 1081–1125 | MEDIUM | **WarmStore stub** — When neither `backend-sqlite` nor `backend-postgres` features are enabled, `WarmStore` is a no-op stub where all methods return `Ok(())`, `Ok(None)`, or `Ok(Vec::new())`. This creates a silent data loss risk: entries "stored" to warm are silently discarded. |

### 1.5 Tests: `#[ignore]`, Empty, or `assert!(true)`

| # | File | Line(s) | Severity | Issue |
|---|------|---------|----------|-------|
| 6 | `memory/memory_bridge.rs` | 196–244 | MEDIUM | `test_background_task_cancellation` — Spawns a full `tokio::runtime::Runtime` inside a `#[test]` (not `#[tokio::test]`), calls `rt.block_on(async { ... })` with `tokio::time::sleep()`, then checks `handle.is_finished()`. This is an async test awkwardly wrapped in synchronous code — the `handle.is_finished()` assertion is racing with async cancellation. It works but is fragile. |
| 7 | `memory/memory_retrieval.rs` | 548–563 | LOW | `test_retrieve_relevant_memories_empty_query` and `test_retrieve_relevant_memories_zero_limit` — Simple boundary tests that only verify empty results. Not stubs, but minimal coverage. |

### 1.6 Unnecessary Locks / Synchronization

| # | File | Line(s) | Severity | Issue |
|---|------|---------|----------|-------|
| 8 | `memory/memory_response_cache.rs` | 20–34, 54–59 | LOW | `MemoryResponseCache::get()` (line 22) and `active_entries()` (line 56) acquire the Mutex and then modify the map (e.g., `shift_remove` + `insert` for LRU refresh, `retain` for expiry cleanup). These are write-side operations disguised as reads — the lock semantics are correct but the method name implies a read-only operation. |
| 9 | `memory/memory.rs` | 125–208 | LOW | `MemoryStore::store()` contains a second lock acquisition for `entries_by_class.get_mut` within the same function (lines 135, 158). Since `MemoryStore` is not `Sync` but protected externally by `Mutex`, this is fine, but the complexity suggests the internal methods could be better decomposed. |
| 10 | `memory/agent_memory_bus.rs` | 233–241 | LOW | In `retrieve_memories()`, the `store` lock is acquired, then explicitly dropped (`drop(store)`) at line 242 before further processing. This is correct but could be simplified with a block scope like in other methods. |

### 1.7 Unused Imports

| # | File | Line(s) | Severity | Issue |
|---|------|---------|----------|-------|
| 11 | `memory/memory_persistence.rs` | 11–15 | LOW | **Duplicated imports**: `use anyhow::{Context, Result};` appears twice — once with `#[cfg(feature = "backend-postgres")]` (line 11–12) and once with `#[cfg(not(feature = "backend-postgres"))]` (line 14–15). Since both branches are mutually exclusive and identical, a single unconditional import suffices. |
| 12 | `memory/semantic_cache.rs` | 17 | LOW | `CancellationToken` (from `tokio_util::sync`) imported — used in `start_background_cleanup`. Not unused, but the import for `tokio` time types (`tokio::time`) is not directly imported — instead `tokio::spawn` is called with full path. This is fine. |

### 1.8 Feature-gated code where the gate is never used

**None found** — all feature gates in the memory module are properly conditional.

---

## 2. Module: `src/multimodal/`

### Files analyzed (8)

```
src/multimodal/mod.rs
src/multimodal/audio_processor.rs
src/multimodal/code_repo_analyzer.rs
src/multimodal/document_parser.rs
src/multimodal/excel_processor.rs
src/multimodal/excel_writer.rs
src/multimodal/ppt_processor.rs
src/multimodal/video_processor.rs
```

### 2.1 Dead / Unused Code

| # | File | Line(s) | Severity | Issue |
|---|------|---------|----------|-------|
| 13 | `multimodal/audio_processor.rs` | 388–486 | HIGH | **`transcribe_whisper_local()` stub** — When `feature = "audio-whisper-openai"` is enabled, this method runs a simulated pipeline that decodes PCM samples but never loads a real Whisper model. It returns "(inaudible segment)" placeholders with 0.0–0.2 confidence. This is effectively dead code that provides no real transcription. |
| 14 | `multimodal/audio_processor.rs` | 501–525 | HIGH | **`transcribe_vosk()` stub** — When `feature = "audio-vosk"` is enabled, this method validates the model path exists but then returns an error saying "no real Vosk model was loaded during initialization". This means the feature is enabled but non-functional — always returning an error for any actual audio. |
| 15 | `multimodal/video_processor.rs` | 390–456 | MEDIUM | **`extract_audio()`** — Only works if ffmpeg is installed on the system (shells out to `ffmpeg` subprocess). If ffmpeg is not available, returns a descriptive error. This is an intentional design choice but means the method is a no-op in environments without ffmpeg. |
| 16 | `multimodal/video_processor.rs` | 250–290 | MEDIUM | **`extract_frames()`** — Same pattern: shells out to ffmpeg, returns an error if not available. Without ffmpeg, the entire video processor is non-functional. |
| 17 | `multimodal/ppt_processor.rs` | 467, 475, 476 | LOW | **Unused underscore-prefixed variables** — `_in_cnv_pr`, `_in_ext`, `_in_off` in `extract_images_from_slide_xml()`. These variables are assigned but never read to make control-flow decisions. They represent incomplete state tracking. The underscore prefix suppresses Rust warnings, but the dead tracking code adds cognitive overhead. |

### 2.2 `#[allow(dead_code)]` Annotations

| # | File | Line(s) | Severity | Issue |
|---|------|---------|----------|-------|
| 18 | `multimodal/excel_writer.rs` | 38 | LOW | `#[allow(dead_code, reason = "F-GAP reserved: boundary validation")]` on `CellIndexOutOfBounds` variant — reserved for future use. |
| 19 | `multimodal/excel_writer.rs` | 43 | LOW | `#[allow(dead_code, reason = "F-GAP reserved: stub path")]` on `FeatureDisabled` variant — used only in the non-feature path but kept for enum completeness. |
| 20 | `multimodal/excel_writer.rs` | 170 | LOW | `#[allow(dead_code, reason = "F-GAP reserved: file-based API")]` on `write_excel_file()` — function is fully implemented but gated behind the feature AND annotated dead_code because no internal caller uses it yet. |

### 2.3 `block_in_place` / `block_on` / `Handle::current().block_on()` Patterns

**None found** in the multimodal module. Audio, video, and document processing use either async (tokio) or `reqwest::blocking` correctly.

### 2.4 Empty / Stub / No-op Implementations

| # | File | Line(s) | Severity | Issue |
|---|------|---------|----------|-------|
| 21 | `multimodal/audio_processor.rs` | 388–486 | HIGH | `transcribe_whisper_local()` — See #13. Full stub that decodes audio but never transcribes it. |
| 22 | `multimodal/audio_processor.rs` | 501–525 | HIGH | `transcribe_vosk()` — See #14. Always returns an error, making the `audio-vosk` feature a compile-time no-op. |
| 23 | `multimodal/audio_processor.rs` | 528–534 | LOW | `transcribe_vosk()` — Non-feature gate stub that returns `FeatureDisabled` error. Standard pattern. |
| 24 | `multimodal/document_parser.rs` | 356–358, 452–454, 467–469 | LOW | **Feature-disabled stubs** for PDF, DOCX, HTML, Markdown, Excel, PPT. Each follows the same pattern: `Err(DocumentParserError::feature_disabled("FORMAT"))`. This is acceptable and expected. |

### 2.5 Tests: `#[ignore]`, Empty, or `assert!(true)`

| # | File | Line(s) | Severity | Issue |
|---|------|---------|----------|-------|
| 25 | `multimodal/audio_processor.rs` | 837–851 | LOW | `test_openai_whisper_missing_key` — Tests that missing key produces error. Acceptable. |
| 26 | `multimodal/audio_processor.rs` | 854–863 | LOW | `test_disabled_backend_returns_error` — Tests that disabled backend returns error. Acceptable. |
| 27 | `multimodal/audio_processor.rs` | 866–877 | LOW | `test_diarization_heuristic` — Only checks that the pipeline "doesn't panic" for a case where the result is an error. Weak test. |
| 28 | `multimodal/audio_processor.rs` | 880–888 | LOW | `test_transcribe_convenience_fn` — Only checks that it returns an error. Minimal. |
| 29 | `multimodal/excel_writer.rs` | 241–364 | LOW | Tests in `excel_writer.rs` require the `document-excel-write` feature to be enabled, so they only run conditionally. |
| 30 | `multimodal/video_processor.rs` | 619–649 | MEDIUM | `test_extract_frames_empty_video` — Only test in the module. Marked as `async fn` test but content wasn't visible in the scan. |

### 2.6 Unnecessary Locks / Synchronization

**None found** — the multimodal module is mostly stateless or uses simple config structs.

### 2.7 Unused Imports

| # | File | Line(s) | Severity | Issue |
|---|------|---------|----------|-------|
| 31 | `multimodal/ppt_processor.rs` | 23 | LOW | `use serde::{Deserialize, Serialize}` — Used by struct `ParsedPresentation`, `Slide`, `SlideImage`. OK. |
| 32 | `multimodal/excel_processor.rs` | 21 | LOW | `use serde::{Deserialize, Serialize}` — Used by data types. OK. |

### 2.8 Feature-gated code where the gate is never used

**None found** — all feature gates (`document-pdf`, `document-docx`, `document-html`, `document-markdown`, `document-excel`, `document-excel-write`, `document-ppt`, `audio-whisper-openai`, `audio-vosk`) are properly used.

---

## 3. Module: `src/optimization/`

### Files analyzed (2)

```
src/optimization/mod.rs
src/optimization/failure_prevention.rs
```

### 3.1 Dead / Unused Code

| # | File | Line(s) | Severity | Issue |
|---|------|---------|----------|-------|
| 33 | `optimization/failure_prevention.rs` | 47–51 | LOW | **`CircuitBreaker` struct** — A legacy backward-compatibility wrapper containing only `name` and `state`. The doc comment says "Convert to `UnifiedCircuitBreaker` via `From`", but there is no `From` impl in this file. The struct exists for backward compat but may accumulate dead code debt. |
| 34 | `optimization/failure_prevention.rs` | 77–83 | LOW | **`AnomalyDetectionResult` struct** — Only returned by `detect_anomaly()` (line 132). The function is a heuristic placeholder — it checks hardcoded failure thresholds and always returns `detected: false` unless a threshold is exceeded. The `recommended_action` field is hardcoded to "increase timeout" or "scale up". |

### 3.2 `#[allow(dead_code)]` Annotations

**None found** directly in these files, but the re-exported `UnifiedCircuitBreaker` and `DegradationLevel` from `crate::resilience::hyper_resilience` may have their own annotations (not in scope of this scan).

### 3.3 Empty / Stub / No-op Implementations

| # | File | Line(s) | Severity | Issue |
|---|------|---------|----------|-------|
| 35 | `optimization/failure_prevention.rs` | 132–166 | MEDIUM | **`detect_anomaly()`** — A heuristic-based anomaly detector that only checks hardcoded thresholds. Returns `AnomalyDetectionResult { detected: false, ... }` unless failure rate exceeds thresholds. This is a placeholder-level implementation that doesn't detect actual anomalies. |
| 36 | `optimization/failure_prevention.rs` | 257–283 | MEDIUM | **`register_service()`** — Only registers a service name in `health_monitors` with default zero values. No real health probe or monitoring is performed. |
| 37 | `optimization/failure_prevention.rs` | 311–326 | LOW | **`get_degradation_strategy()`** — Returns hardcoded `"rate_limit"` or `"circuit_break"` strings based on service health status. No actual strategy configuration. |

### 3.4 Tests

| # | File | Line(s) | Severity | Issue |
|---|------|---------|----------|-------|
| 38 | `optimization/failure_prevention.rs` | 479–563 | LOW | All 6 tests are minimal unit tests that test single methods in isolation. They are functional but don't test integration or realistic failure scenarios. |

### 3.5 Unused Imports

**None found.**

### 3.6 Feature-gated code

**None found** — no feature gates in the optimization module.

---

## 4. Module: `src/shared/`

### Files analyzed (11)

```
src/shared/mod.rs
src/shared/alert_severity.rs
src/shared/execution_recorder.rs
src/shared/http_client.rs
src/shared/protocol_mode.rs
src/shared/provenance_helpers.rs
src/shared/role_types.rs
src/shared/secret_override.rs
src/shared/timestamps.rs
src/shared/token_bucket.rs
src/shared/tool_descriptors.rs
```

### 4.1 Dead / Unused Code

| # | File | Line(s) | Severity | Issue |
|---|------|---------|----------|-------|
| 39 | `shared/secret_override.rs` | 70–73, 76–82, 85, 90–127 | LOW | **`get_keyring_cached()`** — A fully implemented function for caching keyring lookups. It works but may be unused in the current code path (no direct callers visible in the scanned modules). Reserved for future use. |
| 40 | `shared/protocol_mode.rs` | 41–42 | LOW | `CANONICAL_MODES` constant — Used by `from_fuzzy()`. OK. |
| 41 | `shared/tool_descriptors.rs` | 54–57 | LOW | **`apply_patch` tool descriptor** — Has no `required` fields defined in its input schema (`json!({"type": "object"})`). The descriptor exists but provides no argument guidance. |
| 42 | `shared/tool_descriptors.rs` | 59–62 | LOW | **`run_tests` tool descriptor** — Same: empty input schema. |
| 43 | `shared/tool_descriptors.rs` | 64–67 | LOW | **`inspect_git_diff` tool descriptor** — Same: empty input schema. |

### 4.2 `#[allow(dead_code)]` Annotations

**None found** in shared module files.

### 4.3 Empty / Stub / No-op Implementations

**None found** — all shared module files contain clean, functional implementations.

### 4.4 Tests

| # | File | Line(s) | Severity | Issue |
|---|------|---------|----------|-------|
| 44 | `shared/secret_override.rs` | 129–141 | LOW | `test_set_and_get` — Simple roundtrip test, no edge cases tested (e.g., overwrite, concurrent access). |
| 45 | `shared/tool_descriptors.rs` | 565–744 | LOW | Tests cover known-tool descriptor validation which is comprehensive. No flagged issues. |

### 4.5 Unused Imports

**None found.**

---

## 5. Cross-cutting Patterns

### 5.1 `#[allow(dead_code)]` Total Count

Across all four modules there are **4 occurrences** of `#[allow(dead_code)]` (including one in `multimodal/excel_writer.rs` with two separate annotations):

| Location | Count |
|----------|-------|
| `memory/embedding_provider.rs` | 1 |
| `multimodal/excel_writer.rs` | 3 (one enum variant, one other variant, one function) |

### 5.2 `block_in_place` + `block_on` Patterns

**None found** in any of the four modules. All blocking I/O uses either:
- `reqwest::blocking` (synchronous HTTP)
- `spawn_blocking` (for std::sync::RwLock in async context)
- `tokio::fs` (async filesystem)

### 5.3 `Handle::current().block_on()` Patterns

**None found.**

### 5.4 Stub / No-op Pattern Summary

| Stub Location | What it does |
|---------------|-------------|
| `memory_persistence.rs:1081-1125` | WarmStore returns `Ok(())` for all writes — silent data loss |
| `audio_processor.rs:388-486` | `transcribe_whisper_local` decodes PCM but never transcribes — returns silence |
| `audio_processor.rs:501-525` | `transcribe_vosk` always returns error — non-functional feature |
| `failure_prevention.rs:132-166` | `detect_anomaly` uses hardcoded heuristic — not real anomaly detection |
| `failure_prevention.rs:257-283` | `register_service` stores default zeros — no real health probing |
| `video_processor.rs` | Entire module depends on system-installed ffmpeg — no-op without it |

### 5.5 Duplicated Pattern: Feature-gated Wrapper Functions

`document_parser.rs` has a recurring pattern for every document format:

```rust
#[cfg(feature = "document-FOO")]
fn parse_FOO(&self, path: &Path) -> Result<...> {
    let bytes = std::fs::read(path)...;
    self.parse_FOO_bytes(&bytes)
}
#[cfg(not(feature = "document-FOO"))]
fn parse_FOO(&self, _path: &Path) -> Result<...> {
    Err(DocumentParserError::feature_disabled("FOO"))
}
```

This pattern repeats for PDF, DOCX, HTML, Markdown, Excel, PPT — 12 near-identical functions (6 enabled + 6 disabled). This is 150+ lines of boilerplate that could be simplified with a macro.

### 5.6 Mutex Poisoning Recovery Pattern

Every `Mutex::lock()` call in the scanned modules follows the same pattern:
```rust
let guard = mutex.lock().unwrap_or_else(|poisoned| {
    tracing::warn!("... mutex poisoned ...");
    poisoned.into_inner()
});
```

This is consistent and correct. However, it appears ~40+ times across the four modules, adding significant verbosity. This is a stylistic observation, not a bug.

---

## Summary

| Severity | Count | Key Findings |
|----------|-------|--------------|
| **HIGH** | 3 | `transcribe_whisper_local` stub (no real transcription), `transcribe_vosk` always-errors, Audio extraction stubs |
| **MEDIUM** | 6 | WarmStore stub (silent data loss), async test wrapping in sync test, video processing requires ffmpeg, anomaly detection is placeholder |
| **LOW** | 36 | Unused variables, `#[allow(dead_code)]`, redundant imports, empty input schemas, boilerplate repetition, unused sequence counter, COLD_COUNT_WARN pattern |
| **NONE/INFO** | — | No `block_in_place`/`block_on`/`Handle::current().block_on()` patterns found |

**Overall assessment:** The code is well-structured with consistent patterns (mutex poisoning recovery, feature-gating, doc comments). The most impactful issues are the `audio_processor` stubs (high — they compile but don't work) and the `WarmStore` stub (medium — silent data loss when neither sqlite/postgres feature is enabled). Most LOW findings are intentional future-proofing (`#[allow(dead_code)]`, reserved fields) or stylistic boilerplate.
