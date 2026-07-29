# Universal Project Runtime Rules

Authoritative policy sources (single source of truth):
- RULES/global.md
- RULES/common.md
- RULES/coding.md
- RULES/review.md
- RULES/pua.md

Compatibility bootstrap:
- .github/copilot-instructions.md points to RULES and is not the long-form authority.

## Universal Workflow Baseline

- Default runtime workflow is `think -> act -> check -> done` (from `config.toml`).
- Any non-trivial change must produce evidence for all four phases.
- `think` must begin with fact-driven verification: verify each claim against codebase evidence (file reads, grep, logs) before defining scope. Speculative claims must be rejected or downgraded.
- If validation fails in `check`, re-enter `think` with root-cause and remediation notes.
- Do not mark completion in `done` without runnable verification evidence.
- PUA Red Line 2 (fact-driven verification) is enforced in `think`. Quality Compass is enforced in `done`.

## Archive/index policy:
- Historical or duplicate policy documents must be reduced to short index pages.
- Index pages should only point to the authoritative sources above.

## Phase 4 Architecture Rules

- All sub-buses must integrate through CapabilityBus sense/decide/evolve lifecycle
- All policy evaluation and governance checks must go through HarnessBus
- Every new bus must implement Builder pattern with with_*_bus injection
- Every F-GAP module must have unit tests covering normal/edge/error paths
- Feature-gated modules must use cfg feature guards and conditionally compile tests
- No file-level or module-level allow dead_code is permitted
- Use precise per-item allow dead_code with justification comment for planned wiring
- Use cfg test instead of allow dead_code for test-only code
- Each new module must i18n-cover its error surfaces and user-facing messages via tr macro
- All three language files must be updated in parallel for any new message key
- All user-facing strings must go through i18n runtime tr macro
- Every change must compile and pass clippy D warnings under all three profiles
- No test may be flaky; flaky tests must be investigated and fixed
- All capability dimensions must reach real starred rating with implementation tests and bus wiring
- Dimensional ratings must be validated before any release
- Never remove allow dead_code without verifying the annotated item is used in production code
- Prefer cfg test over allow dead_code for test-only code

## Universal Cross-Cutting Rules

- Follow all repository-wide rules from DEVELOPMENT_RULES.md and top-level policies.
- Preserve API and protocol compatibility; never break method contracts.
- Prefer optimal, high-quality, reviewable diffs; avoid broad refactoring unless necessary.
- Ensure all behavior is deterministic and observable with clear logs and metrics.
- Never expose secrets, tokens, or private file contents in any response.
- Treat all external inputs as untrusted; always validate before use.
- If requirements are ambiguous, state your assumptions explicitly before proceeding.
- Favor safe fallback behavior over hard failure whenever possible.
- Strictly forbid any form of placeholder, incomplete, or fake implementation, as well as unclosed symbols or bulk edits that break structure.
- Strictly forbid bridge-stub workarounds (especially test-only local shim modules that mimic production modules). If a dependency boundary issue appears, fix it in real project architecture (module ownership/export/refactor), not by patching test-local stubs.
- All code must compile and pass self-checks in the target language.
- All changes must include tests and documentation updates.
- Code review and CI must focus on these standards and enforce them automatically.
- If a rule below is marked as language-specific (e.g., Rust), and the current project is not that language, skip or adapt the rule accordingly.
