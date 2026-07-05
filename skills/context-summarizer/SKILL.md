---
name: context-summarizer
description: Summarize long conversations or task histories to fit within token windows. Enables the agent to maintain context across sessions without hitting token limits.
version: 1.0.0
author: go-on-team
tags: [context, summarization, token, utility]
min_go_on_version: 1.0.0
---

# Context Summarizer

Summarize long conversations to fit within token windows.

## When to use

- Conversation history exceeds 50% of the available token window
- Starting a new context window after a long agent session
- Preparing context for handoff between agents
- Reducing token usage to improve response latency

## Summarization strategy

1. **Priority system**: Keep these items at full fidelity:
   - User's current request
   - Active file paths and code changes
   - Unresolved errors or warnings
   - Current task status (from progress-tracker)

2. **Compression rules**:
   - Previous successful steps → one-line summary each
   - Debug output → discard (unless relevant to current issue)
   - Error traces → keep only the key error message (strip full backtrace)
   - Code diffs → keep only the summary line (file changed + +/- stats)

3. **Output format**:

```markdown
## Context Summary
- **Task**: {current objective}
- **Files changed**: {list of files}
- **Progress**: {N/M steps completed}
- **Active issues**: {error descriptions}
- **History**: {2-3 sentence summary of what was done}
```

## Commands

- `/summarize` — Generate context summary
- `/summarize save` — Save summary to `docs/context-summary.md`
