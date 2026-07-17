---
name: summarize-text
description: Summarize long text into concise, structured summaries
version: 1.0.0
---

# Summarize Text Skill

Condenses long-form text into concise, structured summaries with key points and actionable items.

## How It Works

1. **Parse input** — Reads the full text to summarize
2. **Extract key information** — Identifies the core thesis, supporting points, and any action items
3. **Structure output** — Organizes the summary into a brief overview, bullet-point key takeaways, and action items
4. **Format output** — Returns a structured Markdown result

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `input` | string | Long text to summarize |

## Example

```
Input: "We need to upgrade our database from PostgreSQL 12 to PostgreSQL 15. The upgrade involves migrating 3 TB of data, updating connection strings in 12 microservices, and testing all read and write paths. The migration window is scheduled for Saturday 2 AM to 6 AM. Rollback plan involves restoring from the latest snapshot taken before the upgrade."
```

## Example Output

```
Summary: A plan to upgrade PostgreSQL from version 12 to 15, involving a 3 TB data migration, connection string updates across 12 microservices, and a 4-hour Saturday maintenance window with a snapshot-based rollback plan.

Key Points:
- Upgrade target: PostgreSQL 12 → 15
- Data volume: 3 TB migration required
- Affected services: 12 microservices need connection string updates
- Maintenance window: Saturday 2 AM – 6 AM
- Rollback: Snapshot restoration before upgrade

Action Items:
- Schedule the maintenance window
- Notify all service owners about connection string changes
- Verify snapshot integrity before starting migration
```

---

Summarize the following text in a clear, structured format.

Text to summarize:
```
{{input}}
```

Output:
- **Summary**: 2-3 sentences capturing the key points
- **Key Points**: Bullet list of main takeaways (max 5)
- **Action Items**: Any actionable items mentioned (if none, say "None identified")
