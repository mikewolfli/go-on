# go-on Development Rules

This document defines the default engineering rules for this repository.
Follow these rules for all feature work, bug fixes, and refactors.

## 1. Scope And Goals

- Keep ACP JSON-RPC behavior stable and backward compatible.
- Prefer small, verifiable changes over large rewrites.
- Keep runtime safety first: no crashes, no silent data loss, no deadlocks.
- Every meaningful change must be test-backed.

## 2. Code Style And Structure

- Language: Rust 2021.
- Keep modules focused: ACP protocol in `src/acp.rs`, config in `src/config.rs`, routing in `src/flow.rs`, provider logic in `src/agents/*`.
- Avoid cross-module leakage: provider-specific behavior should stay in provider modules.
- Prefer explicit names and straightforward control flow over clever abstractions.
- Keep public APIs and config fields stable unless there is a strong reason to change.

## 3. Error Handling Rules

- Never panic on request path for recoverable errors.
- Convert request/agent/runtime failures into JSON-RPC error responses.
- Error messages must be actionable and include enough context for diagnostics.
- Preserve server availability when one agent fails.
- For fallback phases, agent failures must not terminate the whole request until fallback is exhausted.

## 4. ACP Protocol Contract

- Transport: JSON-RPC 2.0 over stdin/stdout.
- Keep method contracts stable for:
  - `initialize`
  - `chat`
  - `runtime.health`
  - `metrics.get`, `metrics.reset`, `metrics.prometheus`
  - `phase.status`
  - `breaker.status`, `breaker.reset`
  - `config.reload`
  - `maintenance.gc`
  - `cache.clear`, `vector.clear`
  - `autotune.get`, `autotune.reset`
  - `shutdown`
- `jsonrpc != "2.0"` must return an invalid request error.
- `shutdown` must transition lifecycle state and drain in-flight requests.

## 5. Config And Validation Rules

- All config changes must keep `config.toml.autopilot-adaptive` valid.
- `AppConfig::validate()` is the source of truth for constraints.
- Adding new config fields requires:
  - struct update in `src/config.rs`
  - validation update in `src/config.rs`
  - template update in `config.toml.autopilot-adaptive`
  - docs update in `README.md`
  - tests for valid and invalid cases
- Startup and reload must both enforce required env vars via `missing_env_vars`.

## 6. Provider And Model Rules

- Providers should implement the same logical behavior through the common `Agent` trait.
- SSE parsing should use shared parser utilities from `src/agents/mod.rs` where possible.
- Principles injection must remain deterministic per provider type.
- Retry policy must remain bounded and observable (no infinite retries).
- If adding provider-specific options, keep pass-through keys explicit in tests.

## 7. Runtime Guardrails

- Maintain and test these protections:
  - per-phase rate limit
  - in-flight concurrency caps
  - circuit breaker
  - timeouts
  - graceful shutdown drain
- Metrics must be updated consistently for success and failure paths.
- Prometheus metric names are contract-like; avoid renaming without migration notes.

## 8. Testing Rules

- Always run tests before merge.
- Minimum required checks for non-trivial changes:
  - `cargo test`
  - specific test target(s) for touched behavior
- Prefer adding tests close to behavior:
  - Unit tests in module `#[cfg(test)]` blocks.
  - Process-level blackbox tests under `tests/`.
- For ACP RPC behavior, include process-level tests using the compiled binary and stdin/stdout.
- Bug fixes must include a regression test when feasible.

## 9. Documentation Rules

- Keep these docs in sync with behavior:
  - `README.md`
  - `config.toml.autopilot-adaptive`
  - this file (`DEVELOPMENT_RULES.md`)
- Any user-visible behavior change requires README updates in the same change.
- Do not document features that are not implemented.

## 10. Git And Change Management

- Do not mix unrelated refactors with feature/bug changes.
- Keep commits scoped and traceable.
- Do not revert unrelated local changes.
- If a file changed unexpectedly during implementation, stop and re-check before continuing.

## 11. Definition Of Done

A change is done only when all are true:

- Behavior implemented and validated.
- Relevant tests added/updated and passing.
- `cargo test` passes.
- Docs and config example updated when needed.
- No known regressions introduced in ACP protocol behavior.

