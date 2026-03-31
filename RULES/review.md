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
