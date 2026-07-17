---
name: note-taking
description: Maintain structured working notes across sessions for project context and decisions
version: 1.0.0
---

# Note-Taking Skill

Maintains structured working notes that persist across sessions, enabling project context retention and decision tracking.

## How It Works

1. **Parse operation** — Reads the input and determines the operation type (read, write, delete, list)
2. **Execute operation** — Performs the requested action against the note store
3. **Confirm result** — Returns the result or confirmation to the user

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `input` | string | Operation string in the format: `operation: topic \| content` |

## Supported Operations

| Operation | Format | Description |
|-----------|--------|-------------|
| `read` | `read: <topic>` | Read notes for a topic |
| `save` | `save: <topic> \| <content>` | Save/replace notes for a topic |
| `append` | `append: <topic> \| <content>` | Append to existing notes |
| `delete` | `delete: <topic>` | Delete notes for a topic |
| `list` | `list` | List all topics with notes |

## Example

```
Input: save: api-redesign | Decided to move from REST to GraphQL for the /users endpoint. Rationale: reduces over-fetching.
Output: Notes saved for topic "api-redesign".
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
