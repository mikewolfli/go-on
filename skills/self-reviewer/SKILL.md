---
name: self-reviewer
description: Agent reviews its own work before presenting to the user. Catches errors, style violations, and omissions before the user sees the output.
version: 1.0.0
author: go-on-team
tags: [review, quality, self-check, verification]
min_go_on_version: 1.0.0
---

# Self Reviewer

Performs a structured self-review of the agent's own work before submission. Checks for completeness, correctness, safety, and adherence to project principles.

## Tags

review, code-quality, self-improvement

## Description

A meta-skill that guides an agent through a structured self-review of its own output before presenting it to the user. Applies a comprehensive checklist across code changes, documentation, performance, and safety dimensions. Catches errors, style violations, omissions, and regressions before the user sees the result.

Use this skill before presenting any work product — especially code changes, documentation updates, configuration changes, or generated artifacts. It is the final quality gate in the agent's workflow.

## Usage

Invoke after completing work but before presenting results to the user.

- `/self-review` — Run full self-review on all pending changes
- `/self-review code` — Review only code changes (compile, lint, test, style)
- `/self-review docs` — Review only documentation changes
- `/self-review safety` — Review for security and safety concerns only

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `scope` | string | Review scope: `full`, `code`, `docs`, `safety` (default: `full`) |
| `changes` | string | Optional: description or diff of the changes to review |
| `strictness` | string | Optional: `normal`, `strict` (default: `normal`). Strict mode flags warnings as errors |

## Output

Returns a structured review report containing:

1. **Summary** — Overall verdict: Pass, Pass with Warnings, or Needs Fixes
2. **Per-category Checklists** — Checked items with pass/fail status for each review category
3. **Issues Found** — Specific, actionable issues with file paths, line references, and suggested fixes
4. **Fix Recommendations** — For each issue, a concrete proposed change or remediation step
5. **Resubmission Checklist** — Re-check items that must pass before final delivery

## When to Use

- Before presenting final results to the user
- After completing a code change
- Before committing changes
- After writing documentation

## Review checklist

### Code changes
- [ ] Does the code compile? (`cargo check` or equivalent)
- [ ] Do existing tests pass? (`cargo test` for affected area)
- [ ] Are there new clippy warnings?
- [ ] Is the change consistent with the project's coding style?
- [ ] Are there any `TODO` or `FIXME` markers left in the code?
- [ ] Are error messages user-friendly and properly localized?
- [ ] Are there any debug/println statements that should be removed?
- [ ] Is the change minimal (no unrelated modifications)?

### Documentation
- [ ] Are public APIs documented?
- [ ] Are complex algorithms explained with comments?
- [ ] Is the rationale for design decisions recorded?
- [ ] Are configuration changes documented?

### Performance
- [ ] Are there any unnecessary allocations or clones?
- [ ] Are hot-path operations async-compatible?
- [ ] Are there any blocking calls in async contexts?
- [ ] Could the solution be simplified?

## Workflow

1. Present the proposed changes
2. Run the self-review checklist
3. Fix any issues found
4. Present the final version

## Commands

- `/self-review` — Run self-review on pending changes
- `/self-review code` — Review only code changes
- `/self-review docs` — Review only documentation
