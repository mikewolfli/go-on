---
name: workflow-optimizer
description: Analyzes multi-step workflows and agent pipelines for bottlenecks, unnecessary serialization, and optimization opportunities
version: 1.0.0
author: go-on-team
tags: [workflow, optimization, pipeline]
min_go_on_version: 1.0.0
---

# Workflow Optimizer

Analyzes multi-step workflows and agent pipelines for bottlenecks, unnecessary serialization, and optimization opportunities. Suggests parallelization, caching, and batching strategies.

## Tags

workflow, optimization, pipeline

## Description

Inspects directed acyclic graphs (DAGs) of agent pipeline steps to identify optimization opportunities. Detects sequential dependencies that could run in parallel, redundant computations that could be cached, and batchable operations that could be grouped. Produces an actionable optimization report with estimated speedup ratios and refactored pipeline definitions.

Use this skill when you have a multi-step agent pipeline, build workflow, CI/CD sequence, data processing chain, or any ordered set of operations where performance matters. It works best with pipelines of 3 or more steps and supports both linear chains and branching DAG topologies.

## Usage

Invoke by describing your workflow steps in detail. The optimizer will analyze the graph and produce structured recommendations.

- `/workflow-optimize analyze` — Analyze an existing workflow for bottlenecks
- `/workflow-optimize refactor` — Generate an optimized pipeline definition

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `steps` | array | Array of workflow step definitions: `[{"name": "...", "dependencies": [...], "description": "..."}]` |
| `action` | string | Action: `analyze`, `refactor` (default: `analyze`) |
| `constraints` | object | Optional: constraints like `{"max_parallel": 4, "cache_enabled": true, "batch_size": 10}` |

Each step in the `steps` array expects:

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Unique step identifier |
| `dependencies` | string[] | Names of steps that must complete before this one (empty for root steps) |
| `description` | string | What this step does, including any side effects or external calls |

## Output

Returns a structured optimization report containing:

1. **Dependency Graph** — Visual representation of the original workflow DAG
2. **Bottleneck Analysis** — Critical path identification with per-step timing estimates
3. **Optimization Suggestions** — Specific recommendations with:
   - Parallelization candidates (steps with no interdependencies)
   - Caching opportunities (steps with deterministic, repeatable outputs)
   - Batching candidates (similar operations on different data)
4. **Estimated Speedup** — Conservative, expected, and optimistic speedup ratios
5. **Refactored Pipeline** — An optimized step ordering with parallelization hints, ready to implement

## Example

```json
{
  "steps": [
    {"name": "fetch-data", "dependencies": [], "description": "Fetch raw data from 3 external APIs"},
    {"name": "validate", "dependencies": ["fetch-data"], "description": "Validate schema and format of each record"},
    {"name": "enrich", "dependencies": ["fetch-data"], "description": "Enrich records with geo-ip lookup"},
    {"name": "transform", "dependencies": ["validate", "enrich"], "description": "Transform to output format"},
    {"name": "upload", "dependencies": ["transform"], "description": "Upload to S3 bucket"}
  ],
  "action": "refactor"
}
```

**Example output (abbreviated):**

```markdown
## Optimization Report

### Original DAG
```
fetch-data ─┬─→ validate ──┐
             └─→ enrich ───┤
                            └─→ transform → upload
```

### Critical Path
fetch-data → validate → transform → upload (4 sequential steps)

### Optimization Suggestions
1. ⚡ **Parallelize** — `validate` and `enrich` can run concurrently (saves ~40%)
2. 💾 **Cache** — `enrich` results are deterministic; cache by input fingerprint
3. 📦 **Batch** — `upload` can batch records in groups of 50

### Estimated Speedup
| Scenario | Time | vs Original |
|----------|------|-------------|
| Original | 100% | — |
| Parallel only | ~60% | 1.67× |
| Parallel + Cache | ~45% | 2.2× |
| Full (parallel + cache + batch) | ~35% | 2.86× |

### Refactored Pipeline
```
fetch-data ──┬─→ validate ──┐
              │               ├─→ transform(parallel=2) → upload(batch=50)
              └─→ enrich ────┘
	```
```

## Input

When invoked, the user's request is available as:

```
{{input}}
```
