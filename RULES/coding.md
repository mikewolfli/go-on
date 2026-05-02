# Universal Coding Phase Rules

## Enhanced Development Workflow Alignment

- Treat coding tasks as a strict think -> act -> check -> done loop.
- Before implementation, record assumptions and file/symbol scope to reduce unintended edits.
- During implementation, keep changes atomic and avoid unrelated refactors.
- Before completion, run mandatory checks that match the touched surface:
	- Rust runtime/core changes: `cargo check --all-targets` and profile-scoped clippy/tests.
	- VS Code addon changes: `npm --prefix vscode-addon run check` and `npm --prefix vscode-addon run test:contract`.
	- GUI changes: `npm --prefix GUI run build` and `npm --prefix GUI run test:contract`.
- If any required check fails, fix and re-run until green before entering done phase.

- Prioritize correctness and maintainability over clever optimizations.
- Keep control flow straightforward; avoid deeply nested branches.
- Preserve public behavior unless the task explicitly requires a change.
- When changing runtime behavior, add targeted unit and regression tests.
- For protocol changes, include process-level integration tests.
- Errors returned to clients must be stable and understandable.
- Strictly forbid any placeholder, incomplete, or fake implementation, as well as unclosed symbols or bulk edits that break structure.
- All code must compile and pass self-checks in the target language.
- All changes must include clear comments and self-check for forbidden patterns.
- Task lists must be detailed, strictly ordered, and each step must be fully completed before proceeding.
- If a rule below is marked as language-specific (e.g., Rust), and the current project is not that language, skip or adapt the rule accordingly.

## Phase 4 Coding Patterns

### F-GAP Module Template
- Each F-GAP module file starts with `//! <ModuleName> — F-GAP-<NN>` doc comment.
- Core types (structs, enums) come first, then public methods, then private helpers, then `#[cfg(test)] mod tests`.
- Every public function must have a doc comment describing its purpose, arguments, and return value.
- Every `#[allow(dead_code)]` must have a trailing comment explaining why (e.g., `// Bucket F — used by evolve() trait`).

### Bus Pattern
- Each bus struct must have a `Profile` struct (returned by `profile()`) and a `Builder` struct (returned by `builder()`).
- Health reporting: each bus must expose a method returning health status that integrates into `handle_health` endpoint.
- Profile structs must derive `Serialize` for integration with governance.status and health endpoint.

### Fault Tolerance Patterns
```rust
// Recovery plan lifecycle
let plan = ft.create_recovery_plan(&node_id, &fault_type)?;
ft.execute_recovery_plan(&plan.id)?;
ft.complete_recovery_plan(&plan.id)?;
ft.reintegrate_node(&node_id)?;
// After reintegrate, verify all faults are resolved
assert!(ft.node_faults(&node_id)?.is_empty());
```

### Transport Patterns
```rust
// ExactlyOnce with dedup
transport.send_with_qos(&msg, QosLevel::ExactlyOnce)?;
// Peek without dequeue
let head = transport.peek(&ChannelId::Control)?;
// Convenience send
transport.send_heartbeat(&node_id, &status)?;
```

### Checkpoint Pattern
```rust
// Always auto-infer parent from branch head when not specified
let parent_id = parent_checkpoint_id
    .clone()
    .or_else(|| state.branch_heads.get(&branch).cloned());
```

### Test Pattern
- Unit tests at the bottom of the source file in a `#[cfg(test)] mod tests` block.
- E2E tests in dedicated test files under `src/` (not `tests/`) for internal access.
- Stress tests use `#[ignore]` or a separate binary target to avoid slowing normal runs.
- Use `tempfile` crate for any file-system-based test fixtures.

### i18n Pattern
```rust
// Use tr!() macro for all user-facing strings
tr!("agent.connection_timeout", &[("provider", provider_name), ("timeout_s", &timeout_str)]);
// Always add new keys to all three language files simultaneously
// en_US.json: "agent.connection_timeout": "Connection to {provider} timed out after {timeout_s}s"
// zh_CN.json: "agent.connection_timeout": "连接到 {provider} 超时（{timeout_s}秒）"
// zh_TW.json: "agent.connection_timeout": "連線到 {provider} 逾時（{timeout_s}秒）"
```

### CLI Message Pattern
```rust
// Use tr!() for setup/init messages too — not just error responses
tr!("setup.directory_created", &[("path", dir_path)]);
```

### Forbidden Patterns (Phase 4 additions)
- No hardcoded string literals in error responses (must use `tr!()`).
- No `let _ =` ignoring of errors from critical operations (checkpoint save, fault recovery, transport send).
- No `Mutex::lock()` calls that could cause double-lock deadlocks — prefer single lock scope with `.get_mut()`.
- No bridge-stub test modules that duplicate production interfaces.

## Strict Structural Safety Rules

Forbidden:
- deleting code without validating symbol pairs: {}, (), [], <>
- leaving unclosed symbols
- bulk edits that break syntax structure
- partial, placeholder, or fake implementations
- modifying unrelated code during a focused task

Mandatory:
1. Validate symbol pairs before and after edits.
2. Keep modified functions structurally complete.
3. Ensure resulting code is compilable.
4. Check syntax errors before delivery.
5. Preserve behavior unless change is explicitly requested.

# Language-Specific Rules

## Rust
- If the current project is not Rust, skip the following:
	- Code must support WASM compilation.
	- Use Rust Result-based error handling.
	- Validate all symbol pairs: {}, (), [], <> before and after every edit.
	- Never use `todo!()`, `unimplemented!()`, empty blocks `{}`, or any placeholder/fake implementations.
	- Never use `simple_impl`, `stub`, `placeholder`, or fake logic.
	- Never use `Ok(())` or `Ok(Default::default())` as placeholders.
	- Never guess crate features or functions that do not exist.
	- Never delete or modify unrelated code.
	- Never blindly rely on tool output; always manually validate syntax.

## C/C++
- If the current project is not C or C++, skip the following:
	- Always check for memory leaks and undefined behavior; use tools like Valgrind or ASan.
	- Use RAII for resource management; avoid manual memory management when possible.
	- Never use uninitialized variables or unsafe casts.
	- All pointer dereferences must be checked for null.
	- Never use empty function bodies or stubs (e.g., `{}` or `;` with no logic).
	- Always match every `malloc`/`new` with a corresponding `free`/`delete`.
	- Avoid macros for logic unless absolutely necessary; prefer inline functions or templates.
	- Validate all symbol pairs: {}, (), [], <> before and after every edit.

## Python
- If the current project is not Python, skip the following:
	- Never use `pass` as a placeholder for required logic.
	- Avoid global variables; prefer function arguments or class attributes.
	- Always handle exceptions explicitly; never use bare `except:`.
	- Use type hints for all public functions and methods.
	- Never leave function bodies empty or with only `...` (ellipsis).
	- Validate all indentation and symbol pairs before and after every edit.
	- Avoid circular imports; refactor modules if necessary.

## Ruby
- If the current project is not Ruby, skip the following:
	- Never use `nil` as a placeholder for required logic.
	- Avoid monkey patching core classes unless absolutely necessary.
	- Use `begin...rescue...end` for error handling; never swallow exceptions silently.
	- Always use snake_case for method and variable names.
	- Never leave method bodies empty or with only `# TODO` comments.
	- Validate all `end` keywords and block structure before and after every edit.

## Mini Program (小程序)
- If the current project is not a Mini Program, skip the following:
	- Always separate logic, view, and style files according to platform conventions (e.g., .js/.ts, .json, .wxml, .wxss for WeChat).
	- Never use empty event handlers or lifecycle methods.
	- Validate all data bindings and event wiring.
	- Avoid direct DOM manipulation; use framework APIs.
	- Always test on both Android and iOS simulators before merging.

## Flutter / Dart
- If the current project is not Flutter/Dart, skip the following:
	- Use const constructors and extract stable subtrees to minimize widget rebuilds.
	- Check mounted before setState after await boundaries in async code.
	- Use null-safety correctly; avoid force-unwrap patterns that crash at runtime.
	- Keep business logic out of widget build methods; use state management boundaries.
	- Handle timeout, error, and fallback paths in platform channel and plugin usage.
	- Add widget/golden/integration tests for new UI behavior or critical interaction changes.

## JavaScript / TypeScript
- If the current project is not JavaScript/TypeScript, skip the following:
	- Use explicit async error handling try/catch or promise rejection handling on network and IO paths.
	- Avoid implicit any leakage in TypeScript-critical modules; use type-safe interfaces for public APIs.
	- Validate runtime input schemas at boundaries API config env user input.
	- Ensure no blocking work in hot request or UI paths; flag unbounded loops and heavy synchronous operations.
	- Keep frontend state changes predictable and side effects isolated.
	- Add unit and integration tests where behavior changes cross module boundaries.
