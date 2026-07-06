---
name: performance-analyzer
description: Analyzes code for performance bottlenecks including N+1 queries, O(n^2) or worse algorithms, unnecessary allocations, sync I/O in async contexts, excessive cloning, missing caching opportunities, and large payload transfers. Supports Rust, Python, TypeScript, and Go.
version: 1.0.0
author: go-on-team
tags: [performance, optimization, profiling, code-review]
min_go_on_version: 1.0.0
---

# Performance Analyzer

Scans source code and runtime traces to detect performance bottlenecks and produces structured findings with estimated impact, exact location, and actionable optimization suggestions.

## How It Works

1. **Parse & Analyze** — The skill reads the provided source files or snippets across the supported languages (Rust, Python, TypeScript, Go) and builds a lightweight AST or pattern-matching pass to identify performance-sensitive patterns.

2. **Detect Bottlenecks** — The analyzer checks for the following categories:
   - **N+1 Queries** — Loop-housed database or API calls that should be batched.
   - **O(n²) or Worse Algorithms** — Nested loops over the same collection without early exit, hash-based lookups, or algorithmic improvement.
   - **Unnecessary Allocations** — Repeated heap allocations in hot paths, temporary strings/vectors allocated per iteration.
   - **Sync I/O in Async Contexts** — Blocking calls (e.g. `std::fs::read`, `requests.get`, `time.sleep`) inside `async fn` or green-thread contexts.
   - **Excessive Cloning** — `.clone()`, `.copy()`, or implicit deep copies on large data structures in loops or hot paths.
   - **Missing Caching Opportunities** — Repeatedly computed or fetched values that are invariant across calls (e.g. repeated regex compilation, repeated DB lookups of the same row).
   - **Large Payload Transfers** — Serialization/deserialization of large objects on every request, oversized JSON responses, unbounded page sizes.

3. **Estimate Impact** — Each finding is assigned an impact level:
   - `critical` — Likely causes production latency spikes, timeouts, or OOM.
   - `high` — Noticeably degrades throughput or latency under moderate load.
   - `medium` — Wastes resources but may not trigger under low-to-moderate load.
   - `low` — Minor inefficiency, good to fix opportunistically.

4. **Output Findings** — Results are returned as a structured JSON array, each entry containing the file path, line range, category, impact, and an optimization suggestion with optional code sketch.

## Input Schema

The skill accepts a single JSON object with the following fields:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `files` | `array` | Yes | Array of file objects, each with `path` (string) and `content` (string) |
| `language` | `string` | No | Explicit language hint (`rust`, `python`, `typescript`, `go`). Auto-detected from file extensions if omitted. |
| `options` | `object` | No | Optional configuration object (see below) |

### `options` Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `min_impact` | `string` | `"low"` | Minimum impact threshold to include in results. One of `"critical"`, `"high"`, `"medium"`, `"low"`. |
| `include_suggestions` | `boolean` | `true` | Whether to include code-level optimization suggestions. |
| `max_findings` | `integer` | `50` | Maximum number of findings to return. |
| `categories` | `array` | `[]` (all) | Filter to specific categories: `"n_plus_one"`, `"quadratic_algo"`, `"unnecessary_alloc"`, `"sync_in_async"`, `"excessive_clone"`, `"missing_cache"`, `"large_payload"`. |

## Output Schema

| Field | Type | Description |
|-------|------|-------------|
| `findings` | `array` | Array of finding objects (see below) |
| `summary` | `object` | Aggregated summary: `total_findings`, `by_impact`, `by_category` |
| `analyzed_files` | `integer` | Number of files processed |

### Finding Object

| Field | Type | Description |
|-------|------|-------------|
| `file` | `string` | File path (matches input `path`) |
| `line_start` | `integer` | Start line number (1-based) |
| `line_end` | `integer` | End line number (1-based, inclusive) |
| `category` | `string` | Bottleneck category key |
| `category_label` | `string` | Human-readable category name |
| `impact` | `string` | Impact level |
| `message` | `string` | Description of the issue |
| `suggestion` | `string` | Optimization suggestion (omitted if `include_suggestions` is `false`) |
| `code_sketch` | `string` | Optional code snippet showing the fix |

## Example

```json
{
  "files": [
    {
      "path": "src/users/service.py",
      "content": "def get_user_orders(user_ids: list[int]) -> list[dict]:\n    results = []\n    for uid in user_ids:\n        user = db.query(\"SELECT * FROM users WHERE id = ?\", uid)\n        orders = db.query(\"SELECT * FROM orders WHERE user_id = ?\", uid)\n        results.append({\"user\": user, \"orders\": orders})\n    return results"
    }
  ],
  "options": {
    "min_impact": "medium",
    "categories": ["n_plus_one"]
  }
}
```

### Example Output

```json
{
  "findings": [
    {
      "file": "src/users/service.py",
      "line_start": 2,
      "line_end": 6,
      "category": "n_plus_one",
      "category_label": "N+1 Query",
      "impact": "critical",
      "message": "Each user_id triggers a separate user query and a separate orders query, resulting in 2N+1 database round-trips.",
      "suggestion": "Use a single JOIN or two batched queries with `WHERE id IN (...)` to reduce to 2 queries total.",
      "code_sketch": "def get_user_orders(user_ids: list[int]) -> list[dict]:\n    users = db.query(\"SELECT * FROM users WHERE id IN (?)\", user_ids)\n    orders = db.query(\"SELECT * FROM orders WHERE user_id IN (?)\", user_ids)\n    user_map = {u[\"id\"]: u for u in users}\n    order_map = defaultdict(list)\n    for o in orders:\n        order_map[o[\"user_id\"]].append(o)\n    return [{\"user\": user_map[uid], \"orders\": order_map[uid]} for uid in user_ids]"
    }
  ],
  "summary": {
    "total_findings": 1,
    "by_impact": {
      "critical": 1,
      "high": 0,
      "medium": 0,
      "low": 0
    },
    "by_category": {
      "n_plus_one": 1
    }
  },
  "analyzed_files": 1
}
```
