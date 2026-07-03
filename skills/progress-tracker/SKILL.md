---
name: progress-tracker
description: Persist completed and blocked steps across multi-turn agent invocations. Avoid re-analyzing already-resolved subtasks. Use when a task spans multiple agent turns.
---

# Progress Tracker

Track task progress across agent invocations.

## When to use

- Multi-step tasks that span multiple agent turns
- Tasks where intermediate state must survive between invocations
- Coordinating work across agent sessions

## How it works

1. **Load** existing progress from `progress.json` at the workspace root
2. **Check** which steps are completed, in-progress, or blocked
3. **Skip** completed steps and re-start from the first in-progress or unstarted step
4. **Update** progress after each step completes
5. **Flag** blocked steps and suggest alternatives

## Progress file format

```json
{
  "task_id": "task-001",
  "status": "in_progress",
  "steps": [
    {"id": 1, "description": "Analyze requirements", "status": "completed"},
    {"id": 2, "description": "Design architecture", "status": "completed"},
    {"id": 3, "description": "Implement core logic", "status": "in_progress"},
    {"id": 4, "description": "Write tests", "status": "pending"}
  ],
  "blocked_steps": [],
  "artifacts": {
    "3": {"output": "src/core.rs"}
  }
}
```

## Commands

- `/progress` — Show current progress
- `/progress mark <step_id> done` — Mark step as completed
- `/progress mark <step_id> blocked` — Mark step as blocked
- `/progress reset` — Reset progress for the current task
