# Universal Team Coding Conventions

- Functions and methods must be cohesive and single-responsibility.
- Maintain naming conventions and respect file/module boundaries.
- Add or update tests for all non-trivial logic changes.
- Keep user-visible behavior and documentation aligned in the same change.
- Error messages must be explicit and actionable.
- Never introduce hidden global state or non-deterministic side effects.
- Reuse existing helpers before adding new logic; avoid duplication.
- Make timeout, rate-limit, and breaker behavior explicit in code paths.
- Always check for existing functions, classes, or modules before adding new ones.
- Task lists must be complete, executed in order, and every step must be implemented.
- If a rule below is marked as language-specific (e.g., Rust), and the current project is not that language, skip or adapt the rule accordingly.
