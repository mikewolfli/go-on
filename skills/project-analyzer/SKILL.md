---
name: project-analyzer
description: Analyzes full project structure, dependencies, architecture patterns, and generates comprehensive reports
version: 1.0.0
author: go-on-team
tags: [analysis, architecture, dependencies, project, documentation, refactoring]
min_go_on_version: 1.0.0
---

# Project Analyzer Skill

Performs deep analysis of a project's structure, dependency graph, architecture patterns, code quality metrics, and generates comprehensive reports. Helps developers understand codebase health, identify technical debt, and plan refactoring efforts.

## How It Works

1. **Structure scan** — Walks the directory tree and maps the project's file organization
2. **Dependency analysis** — Parses dependency files (Cargo.toml, package.json, requirements.txt, go.mod, Gemfile) and builds a dependency graph
3. **Architecture detection** — Identifies architectural patterns (MVC, layered, hexagonal, microservices) from directory structure and imports
4. **Code quality metrics** — Estimates code complexity, file sizes, language distribution, and test coverage gaps
5. **Report generation** — Produces structured JSON and optional Markdown summary

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `directory` | string | Root directory of the project to analyze |
| `depth` | integer | Directory traversal depth (default: 5, max: 20) |
| `include_patterns` | array | Optional glob patterns to include (e.g. `["**/*.rs", "**/*.py"]`) |
| `exclude_patterns` | array | Optional glob patterns to exclude (e.g. `["**/node_modules/**"]`) |
| `analyze_deps` | boolean | Whether to parse and analyze dependency files (default: true) |
| `output_format` | string | Report format: "json" or "markdown" (default: "json") |

## Example

```json
{
  "directory": "/home/user/projects/my-app",
  "depth": 5,
  "exclude_patterns": ["**/node_modules/**", "**/target/**", "**/.git/**"],
  "analyze_deps": true,
  "output_format": "json"
}
```

**Example output (abbreviated):**

```json
{
  "project_name": "my-app",
  "language": "Rust",
  "total_files": 156,
  "total_lines": 28450,
  "languages": {
    "Rust": {"files": 98, "lines": 22100},
    "TypeScript": {"files": 34, "lines": 5200},
    "Markdown": {"files": 24, "lines": 1150}
  },
  "dependencies": {
    "direct": 42,
    "transitive": 156,
    "outdated": 3,
    "vulnerable": 1
  },
  "architecture": {
    "pattern": "layered",
    "layers": ["presentation", "domain", "infrastructure"],
    "circular_deps": 0
  }
}
```
