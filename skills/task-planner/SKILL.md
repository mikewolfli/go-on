---
name: task-planner
description: Breaks down complex objectives into atomic subtasks, identifies dependencies, and produces an execution plan
version: 1.0.0
author: go-on-team
tags: [planning, tasks, dependencies, execution, workflow]
min_go_on_version: 1.0.0
---

# Task Planner Skill

Accepts a high-level objective along with contextual information and constraints, then produces a structured, dependency-aware execution plan composed of atomic subtasks.

## How It Works

1. **Decompose** — The objective is split into the smallest meaningful units of work. Each subtask represents a single logical change or decision.
2. **Order & Depend** — Subtasks are ordered topologically: if subtask B requires the output of subtask A, A is listed first and the dependency is recorded.
3. **Constrain** — Explicit constraints (e.g. "must not touch the database layer", "must use v2 API only") are validated against each subtask. Subtasks that violate a constraint are flagged or removed.
4. **Estimate** — Each subtask receives a relative effort estimate (small, medium, large) based on complexity and scope.
5. **Output** — The result is a JSON plan with a summary, a dependency graph, and an ordered list of subtasks.

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `objective` | string | The high-level goal to be broken down |
| `context` | string | Background information, relevant file paths, or prior work |
| `constraints` | string[] | Explicit boundaries or requirements that must be respected |

## Example

```json
{
  "objective": "Add rate-limiting middleware to the public API gateway",
  "context": "Gateway lives at src/gateway/. Uses actix-web 4. Existing middleware chain is in src/gateway/middleware.rs.",
  "constraints": [
    "Must not introduce new external dependencies",
    "Rate limits must be configurable via environment variables",
    "Must return 429 Too Many Requests with a Retry-After header"
  ]
}
```

### Example Output

```json
{
  "summary": "6 subtasks, estimated effort: 2 small, 3 medium, 1 large. No circular dependencies detected.",
  "dependency_graph": [
    ["subtask-1", []],
    ["subtask-2", ["subtask-1"]],
    ["subtask-3", ["subtask-1"]],
    ["subtask-4", ["subtask-2", "subtask-3"]],
    ["subtask-5", ["subtask-4"]],
    ["subtask-6", ["subtask-5"]]
  ],
  "subtasks": [
    {
      "id": "subtask-1",
      "title": "Define rate-limit configuration struct and env-var parsing",
      "effort": "small",
      "depends_on": []
    },
    {
      "id": "subtask-2",
      "title": "Implement token-bucket state store (in-memory)",
      "effort": "medium",
      "depends_on": ["subtask-1"]
    },
    {
      "id": "subtask-3",
      "title": "Build middleware extractor to read client IP / API key",
      "effort": "small",
      "depends_on": ["subtask-1"]
    },
    {
      "id": "subtask-4",
      "title": "Wire rate-limiter into the middleware chain",
      "effort": "medium",
      "depends_on": ["subtask-2", "subtask-3"]
    },
    {
      "id": "subtask-5",
      "title": "Return 429 + Retry-After when limit is exceeded",
      "effort": "medium",
      "depends_on": ["subtask-4"]
    },
    {
      "id": "subtask-6",
      "title": "Add integration tests for rate-limiting scenarios",
      "effort": "large",
      "depends_on": ["subtask-5"]
    }
  ]
}
```
