---
name: decision-logger
description: Record architectural and design trade-off decisions so the agent doesn't re-analyze the same choices. Especially useful for long-running projects.
---

# Decision Logger

Record and retrieve past decisions so agents don't re-analyze the same trade-offs.

## When to use

- Making architectural decisions with multiple alternatives
- Choosing between libraries, frameworks, or design patterns
- Recording why a particular approach was chosen over alternatives
- Any decision that should be consistent across agent invocations

## Decision record format

Each decision should be logged as a structured entry:

```markdown
## Decision: {title}

- **Date**: YYYY-MM-DD
- **Context**: What prompted this decision
- **Options Considered**:
  - Option A: pros/cons
  - Option B: pros/cons
- **Decision**: Option A
- **Rationale**: Why this option was chosen
- **Consequences**: What this decision means going forward
```

## Storage

Decisions are stored in `docs/decisions/` as markdown files named `YYYY-MM-DD-short-title.md`.

## Commands

- `/decisions list` — List all recorded decisions
- `/decisions show <title>` — Show a specific decision
- `/decisions related <topic>` — Find decisions related to a topic
