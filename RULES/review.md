# Universal Review Phase Rules

## Enhanced Workflow Review Gates

- Review outcome must map to workflow phases:
	- think: scope and assumptions are explicit and justified.
	- act: code changes are optimal, coherent, high-quality, and non-placeholder.
	- check: validation evidence is present for each changed surface.
	- done: delivery notes include residual risks and rollback path.
- Reject changes that claim completion without check-phase evidence.
- Require finding-to-fix closure: each high/medium finding must be resolved or explicitly risk-accepted.
- For release-facing changes, verify packaging/build workflow commands are reproducible in local environment.

- Reject any change that weakens protocol compatibility or runtime safety.
- In early-stage architecture, placeholders are allowed only if interfaces are clear and risks are documented.
- Prioritize boundary correctness, extension points, and rollback safety first.
- Missing parts are allowed only if explicitly marked and test impact is stated; do not block on completeness in early-stage.
- Review must verify edge cases: invalid input, timeout, rate-limit, shutdown, etc.
- Require tests for all bug fixes and newly introduced behavior.
- Documentation and config examples must match implementation details.
- Flag any silent changes in metrics, breaker, or limiter semantics.
- Prefer explicit migration notes when renaming user-visible fields or metrics.
- In strict/mature mode: no placeholders, no silent TODOs, no unhandled error branches.
- Approve only when behavior, tests, and documentation are coherent and consistent.
- If a rule below is marked as language-specific (e.g., Rust), and the current project is not that language, skip or adapt the rule accordingly.

## Phase 4 Review Standards

- Verify the change compiles and passes clippy D warnings under all three profiles
- Feature-gated code must be reviewed under the gate condition cfg blocks must be correct
- All three language files must be updated for new i18n keys rejection if any missing
- i18n keys must follow naming convention module dot descriptive_name
- Review must reject any file-level allow dead_code
- Review must reject any module-level allow dead_code
- Per-item allow dead_code must have a justification comment referencing target F-GAP or phase
- cfg test is the preferred alternative to allow dead_code for test-only code
- New F-GAP modules must have minimum 9 tests covering normal edge error and boundary paths
- Fault tolerance changes must include an E2E lifecycle test
- Transport changes must include QoS dedup and peek tests
- Stress tests with 500 or more iterations are required for scalability-sensitive changes
- Review must verify tests pass under all three profiles
- New sub-buses must integrate into CapabilityBus via with_ name _bus and sense decide profile extensions
- Health reporting must be added to handle_health endpoint under modules harness_bus or capability_bus
- Profile struct must derive Serialize
- All new modules must map to a documented F-GAP or blueprint item
- No orphan modules every module must be registered in its parent mod.rs and the bus system if applicable
- Starred ratings must be real verify implementation tests dead_code status and bus wiring
- Reject any change that inflates star ratings without corresponding code changes
- Before any release perform a 38-dimension audit verify each dimension has dedicated module or integration
- Check for false starred claims must verify code not just documentation
- Validate zero dead_code files and zero module-level dead_code
- Ensure all dimensions with public RPC methods have integration tests

## Mandatory Review Standards (merged from copilot-instructions)

1. Empty implementations and placeholders are forbidden in production logic.
2. Loops and recursion must have reachable and explicit termination conditions.
3. Review cross-calls for circular dependency risk and suggest decoupling where needed.
4. Flag unused code and remove it unless there is justified retention.
5. Validate function completeness:
- inputs validated
- edge cases handled
- error paths explicit
- behavior aligned with function intent
- known limitations documented

# Language-Specific (Rust Only)
- If the current project is not Rust, skip the following:
	- Placeholders are only allowed in early-stage architecture with clear interface and documented risk.
	- Code must support WASM compilation.
	- Use Rust Result-based error handling.
	- Validate all symbol pairs: {}, (), [], <> before and after every edit.
	- Never use `todo!()`, `unimplemented!()`, empty blocks `{}`, or any placeholder/fake implementations.
	- Never use `simple_impl`, `stub`, `placeholder`, or fake logic.
	- Never use `Ok(())` or `Ok(Default::default())` as placeholders.
	- Never guess crate features or functions that do not exist.
	- Never delete or modify unrelated code.
	- Never blindly rely on tool output; always manually validate syntax.

# Language-Specific (Flutter / Dart)
- If the current project is not Flutter/Dart, skip the following:
	- Do not approve widget trees with obvious rebuild hot spots; require use of const constructors and extraction of stable subtrees.
	- Verify async UI safety: check mounted before setState after await boundaries.
	- Require null-safety correctness; reject force-unwrap style patterns that can crash at runtime.
	- Validate state management boundaries (Provider/Bloc/Riverpod/etc.) and avoid business logic in widget build methods.
	- Confirm platform channel and plugin usage handles timeout, error, and fallback paths.
	- Require widget/golden/integration tests for new UI behavior or critical interaction changes.

# Language-Specific (Python)
- If the current project is not Python, skip the following:
	- Reject dynamic behavior that hides errors; require explicit exceptions and actionable error messages.
	- Verify type hints on public functions/classes and consistent typing in critical paths.
	- Ensure loops/comprehensions over large data have clear complexity awareness and memory safety.
	- For async code, validate proper await usage, cancellation handling, and no blocking calls in event loops.
	- Require test updates for bug fixes and behavior changes; prefer pytest-style deterministic assertions.
	- Validate dependency and import hygiene; flag unused imports and circular imports.

# Language-Specific (C/C++)
- If the current project is not C/C++, skip the following:
	- Enforce memory safety review: ownership, lifetime, allocation/free symmetry, and RAII correctness.
	- Validate bounds checks for arrays, buffers, pointer arithmetic, and string operations.
	- Require explicit handling for error codes, null pointers, and system call failure paths.
	- Flag undefined behavior risks (uninitialized data, invalid casts, data races, integer overflow assumptions).
	- Verify thread-safety for shared state and lock ordering to reduce deadlock risk.
	- Require unit/integration tests plus sanitizer-friendly code paths when applicable.

# Language-Specific (JavaScript / TypeScript)
- If the current project is not JavaScript/TypeScript, skip the following:
	- Require explicit async error handling (try/catch or promise rejection handling) on network and IO paths.
	- Reject implicit any leakage in TypeScript-critical modules; require type-safe interfaces for public APIs.
	- Validate runtime input schemas at boundaries (API, config, env, user input).
	- Ensure no blocking work in hot request/UI paths; flag unbounded loops and heavy synchronous operations.
	- Confirm frontend state changes are predictable and side effects are isolated.
	- Require test updates (unit plus integration where behavior changes cross module boundaries).
