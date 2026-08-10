# RULES Templates

These files are auto-loaded by `go-on` when `config.toml` is loaded (startup and `config.reload`).

## Discovery Paths

The loader reads optional files relative to the config directory:

1. `RULES.md`
2. `RULES/global.md`
3. `RULES/common.md`
4. `RULES/pua.md` ⭐ (NEW: PUA enforcement rules for agent proxy)
5. `<phase>.rules.md` (for example `coding.rules.md`)
6. `RULES/<phase>.md`
7. `RULES/<phase>.rules.md`
8. `RULES/<phase>.local.md`

> Note: `RULES/local.md` does not exist yet — create it when local overrides are needed.

## Merge Behavior

- Existing `phases.<name>.principles` from TOML are kept.
- Auto-loaded rules are appended after existing principles.
- Duplicate lines are deduplicated while preserving first appearance order.
- Markdown headings and fenced code blocks are ignored.
- List item prefixes (`-`, `*`, `+`, `1.`, `1)`) are stripped.

## Recommended Workflow

## Universal Enhanced Development Workflow (default)

Default workflow baseline (from `config.toml`):
- `think` -> `act` -> `check` -> `done`

Phase intent and required outputs:
- think:
  1. Verify claims against codebase evidence (grep, file reads, logs) — reject speculation.
  2. Clarify scope, constraints, risks, affected files, and acceptance criteria before editing.
- act: implement optimal, high-quality, reviewable changes; keep behavior deterministic and documented.
- check: run surface-specific verification (build/lint/tests/contracts) and collect evidence.
- done: deliver concise change summary, validation result, and residual risk/rollback notes.

Control rules:
- Do not skip `check` and `done` for non-trivial changes.
- If `check` fails, loop back to `think` with concrete failure evidence and remediation plan.
- Keep authority in `RULES/*.md`; avoid duplicating long policy text elsewhere.
- Fact-driven verification (PUA Red Line 2) and Quality Compass (PUA §40) are enforced during `think` and `done` respectively.

Authoritative source model:
- Keep policy authority in `RULES/*.md` for editor-agnostic reuse.
- Keep `config/languages/rules/.github/copilot-instructions.md` as bootstrap pointer for Copilot compatibility.
- Keep legacy or campaign documents as short index pages pointing to authority files.
- Do not duplicate long-form policy text across multiple root/.github markdown files.

1. Keep stable, cross-project constraints in `RULES/global.md`.
2. Keep team conventions in `RULES/common.md`.
3. Keep machine-local or developer-local overrides in `RULES/local.md` (create it when needed).
4. Keep phase-specific rules in `RULES/coding.md`, `RULES/review.md`, etc.
5. Keep project-local phase overrides in `RULES/<phase>.local.md` or `<phase>.rules.md` sidecar files when needed.

## Phase 4 Rule Coverage

- global.md covers architecture profile gating F-GAP conventions 38-dimension star rating
- common.md covers multi-bus integration patterns E2E and stress test standards transport QoS fault tolerance checkpoint convention dead code management
- coding.md covers F-GAP module coding patterns bus pattern transport checkpoint test pattern i18n pattern
- review.md covers cross-profile validation i18n completeness dead code verification test coverage gate bus integration gate architecture compliance 38-dimension audit
- pua.md covers L3 checklist enhanced with profile-specific verification quality compass additions iceberg rule categories
- Profile gating convention three build profiles local simple-server multi-users-server require different rule scopes
- Annotate profile-specific rules with bracket markers all simple-server or multi-users-server

## Example

If your config defines phases `coding` and `review`, start with:

- `RULES/global.md`
- `RULES/coding.md`
- `RULES/review.md`

Then call `config.reload` to apply changes without restarting.

If a rule file exists but contributes no usable lines, `--doctor` and `config.reload` will report it as a warning.

