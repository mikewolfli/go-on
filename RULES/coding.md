# Universal Coding Phase Rules

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
