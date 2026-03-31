# Universal Project Runtime Rules

- Follow all repository-wide rules from DEVELOPMENT_RULES.md and top-level policies.
- Preserve API and protocol compatibility; never break method contracts.
- Prefer minimal, reviewable diffs; avoid broad refactoring unless necessary.
- Ensure all behavior is deterministic and observable with clear logs and metrics.
- Never expose secrets, tokens, or private file contents in any response.
- Treat all external inputs as untrusted; always validate before use.
- If requirements are ambiguous, state your assumptions explicitly before proceeding.
- Favor safe fallback behavior over hard failure whenever possible.
- Strictly forbid any form of placeholder, incomplete, or fake implementation, as well as unclosed symbols or bulk edits that break structure.
- All code must compile and pass self-checks in the target language.
- All changes must include tests and documentation updates.
- Code review and CI must focus on these standards and enforce them automatically.
- If a rule below is marked as language-specific (e.g., Rust), and the current project is not that language, skip or adapt the rule accordingly.
