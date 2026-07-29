---
name: note-taking
description: Maintain structured working notes and architectural decision records across sessions for project context, decisions, and knowledge retention
version: 1.0.0
author: go-on-team
tags: [notes, decision, logging, architecture, adr, knowledge]
min_go_on_version: 1.0.0
---

# Note-Taking Skill

Maintains structured working notes and decision records that persist across sessions, enabling project context retention, decision tracking, and knowledge reuse.

## How It Works

1. **Parse operation** — Reads the input and determines the operation type (read, write, delete, list)
2. **Execute operation** — Performs the requested action against the note store
3. **Confirm result** — Returns the result or confirmation to the user

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `input` | string | Operation string — see supported formats below |

## Supported Entry Types

The skill supports typed entries. Each entry type has a built-in schema:

### `note` — General notes

Free-form notes for any topic.

| Operation | Format | Description |
|-----------|--------|-------------|
| `read` | `read: <topic>` | Read notes for a topic |
| `save` | `save: <topic> \| <content>` | Save/replace notes for a topic |
| `append` | `append: <topic> \| <content>` | Append to existing notes |
| `delete` | `delete: <topic>` | Delete notes for a topic |
| `list` | `list` | List all topics with notes |

### `decision` — Architectural Decision Records (ADR)

Structured records of architectural and design trade-off decisions, stored so agents don't re-analyze the same choices.

Use when:
- Making architectural decisions with multiple alternatives
- Choosing between libraries, frameworks, or design patterns
- Recording why a particular approach was chosen over alternatives
- Any decision that should be consistent across agent invocations

**Decision record format:**

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

**Decision operations:**

| Operation | Format | Description |
|-----------|--------|-------------|
| `decision:log` | `decision:log \| <title> \| <json-payload>` | Log a new decision record |
| `decision:list` | `decision:list` | List all recorded decisions |
| `decision:show` | `decision:show \| <title>` | Show a specific decision |
| `decision:related` | `decision:related \| <topic>` | Find decisions related to a topic |

## Examples

### General note

```
Input: save: api-redesign | Decided to move from REST to GraphQL for the /users endpoint. Rationale: reduces over-fetching.
Output: Notes saved for topic "api-redesign".
```

### Decision log

```
Input: decision:log | caching-strategy | {"title": "Adopt Redis for session caching", "date": "2025-03-15", "context": "Session store experiencing high latency with PostgreSQL", "options": [{"name": "Redis", "pros": ["low latency", "built-in TTL"], "cons": ["additional infrastructure"]}, {"name": "Memcached", "pros": ["simple", "fast"], "cons": ["no persistence", "limited data types"]}], "decision": "Redis", "rationale": "Built-in TTL and persistence support better match our session expiry requirements", "consequences": "Team needs to learn Redis; add Helm chart for Redis cluster"}
Output: Decision "caching-strategy" logged.
```

---

You are a note-taking assistant. Based on the user's input:

If the input is a READ operation:
- Return the current notes for the given topic/project
- If no notes exist, return "No notes yet"

If the input is a WRITE operation (prefixed with "save:" or "append:"):
- Save or append the content to the notes for the given topic
- Confirm with the saved content preview

If the input is a DELETE operation (prefixed with "delete:" or "clear:"):
- Clear the notes for the given topic
- Confirm deletion

If the input is a DECISION operation (prefixed with "decision:"):
- Log, list, show, or find decisions based on the sub-operation
- Use the structured decision schema for logging

Input:
```
{{input}}
```

Supported operations:
- `read: <topic>` — Read notes for a topic
- `save: <topic> | <content>` — Save/replace notes for a topic
- `append: <topic> | <content>` — Append to existing notes
- `delete: <topic>` — Delete notes for a topic
- `list` — List all topics with notes
- `decision:log | <title> | <json-payload>` — Log a decision record
- `decision:list` — List all decision records
- `decision:show | <title>` — Show a specific decision
- `decision:related | <topic>` — Find decisions related to a topic
