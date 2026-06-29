---
name: changelog-generator
description: Generates structured changelogs from git commit history or structured input
version: 1.0.0
author: go-on-team
tags: [changelog, release, git, documentation, versioning]
min_go_on_version: 1.0.0
---

# Changelog Generator Skill

Produces structured, well-formatted changelogs from git commit history or from a provided list of commits/notes. Supports Keep a Changelog format, Conventional Commits categorization, and custom templates.

## How It Works

1. **Fetch** — Reads git log input (or accepts structured commit data) to extract commit messages, authors, and hashes
2. **Categorize** — Groups commits by Conventional Commits type (`feat:`, `fix:`, `docs:`, `refactor:`, `perf:`, `test:`, `chore:`, `ci:`, `style:`, `build:`, `revert:`)
3. **Generate** — Produces changelog in the requested format with semantic version bumps inferred from the commit types
4. **Highlight** — Identifies breaking changes (`BREAKING CHANGE` footers or `!` after the type) and lists them first

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `commits` | string[] | Array of commit messages (or raw `git log` output) |
| `from_ref` | string | Optional: starting git ref (tag/branch/commit) |
| `to_ref` | string | Optional: ending git ref (default: HEAD) |
| `format` | string | Output format: `keep-a-changelog`, `markdown`, `json` (default: `markdown`) |
| `include_author` | boolean | Optional: include commit authors (default: false) |

## Example

```json
{
  "commits": [
    "feat: add streaming response support",
    "fix: correct timeout handling in websocket reconnection",
    "docs: update API reference with new endpoints",
    "feat(api): add pagination to list endpoints",
    "refactor: extract caching layer into standalone module",
    "perf: reduce memory allocation in hot path",
    "BREAKING CHANGE: rename `send()` to `dispatch()` across all public APIs",
    "chore: bump dependencies"
  ],
  "format": "keep-a-changelog"
}
```

**Example output (abbreviated):**

```markdown
# Changelog

## [Unreleased]

### Breaking Changes

- **api**: rename `send()` to `dispatch()` across all public APIs

### Features

- add streaming response support
- **api**: add pagination to list endpoints

### Bug Fixes

- correct timeout handling in websocket reconnection

### Performance Improvements

- reduce memory allocation in hot path

### Documentation

- update API reference with new endpoints

### Refactoring

- extract caching layer into standalone module

### Chores

- bump dependencies
```
