# Universal Review Phase Rules

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
