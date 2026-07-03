---
name: self-reviewer
description: Agent reviews its own work before presenting to the user. Catches errors, style violations, and omissions before the user sees the output.
---

# Self Reviewer

Agent reviews its own work before finalizing.

## When to use

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
