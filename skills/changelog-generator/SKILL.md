---
name: conventional-commits-toolkit
description: Generates structured changelogs and Conventional Commits-style commit messages from git history, diffs, or structured input
version: 1.0.0
author: go-on-team
tags: [changelog, commit, git, conventional-commits, release, version-control, documentation]
min_go_on_version: 1.0.0
---

# Conventional Commits Toolkit

Produces well-formatted changelogs and commit messages following the Conventional Commits specification. Operates in two modes: `generate-changelog` and `generate-commit`.

## Mode: `generate-changelog`

Produces structured, well-formatted changelogs from git commit history or from a provided list of commits/notes. Supports Keep a Changelog format, Conventional Commits categorization, and custom templates.

### How It Works

1. **Fetch** — Reads git log input (or accepts structured commit data) to extract commit messages, authors, and hashes
2. **Categorize** — Groups commits by Conventional Commits type (`feat:`, `fix:`, `docs:`, `refactor:`, `perf:`, `test:`, `chore:`, `ci:`, `style:`, `build:`, `revert:`)
3. **Generate** — Produces changelog in the requested format with semantic version bumps inferred from the commit types
4. **Highlight** — Identifies breaking changes (`BREAKING CHANGE` footers or `!` after the type) and lists them first

### Input Schema (generate-changelog)

| Parameter | Type | Description |
|-----------|------|-------------|
| `commits` | string[] | Array of commit messages (or raw `git log` output) |
| `from_ref` | string | Optional: starting git ref (tag/branch/commit) |
| `to_ref` | string | Optional: ending git ref (default: HEAD) |
| `format` | string | Output format: `keep-a-changelog`, `markdown`, `json` (default: `markdown`) |
| `include_author` | boolean | Optional: include commit authors (default: false) |

### Example (generate-changelog)

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

---

## Mode: `generate-commit`

Analyzes code changes (from git diff or natural-language description) and generates well-structured commit messages following the Conventional Commits specification. Includes scope detection, breaking change identification, and body generation.

### How It Works

1. **Analyze** — Examines the diff or change description to categorize the type of change (feature, fix, refactor, docs, test, chore, etc.)
2. **Scope** — Detects the affected module/component from file paths (e.g. `src/api/` → `api`, `src/db/` → `db`)
3. **Compose** — Writes a concise subject line (<72 chars), optional body with context, and footer with breaking change notes or issue references
4. **Validate** — Checks the output conforms to the Conventional Commits specification and suggests improvements

### Input Schema (generate-commit)

| Parameter | Type | Description |
|-----------|------|-------------|
| `diff` | string | Git diff output (`git diff`) or path-based change description |
| `description` | string | Optional: natural-language description of the change |
| `style` | string | Commit style: `conventional`, `angular`, `simple` (default: `conventional`) |
| `include_body` | boolean | Optional: generate detailed body (default: true) |
| `breaking_change` | string | Optional: description if the change is breaking |

### Example (generate-commit)

```json
{
  "diff": "diff --git a/src/cache/lru.rs b/src/cache/lru.rs\n@@ -42,7 +42,7 @@ impl LruCache {\n         self.order.pop_front()\n     }\n \n-    pub fn insert(&mut self, key: K, value: V) {\n+    pub fn insert(&mut self, key: K, value: V) -> Option<V> {\n         if self.order.len() >= self.capacity {\n             self.evict();\n         }\n-        self.order.push_back((key, value));\n+        let replaced = self.map.insert(key.clone(), value);\n+        self.order.push_back((key, value.clone()));\n+        replaced\n     }\n }\n",
  "description": "Make LruCache.insert return the previous value if the key already existed",
  "style": "conventional",
  "include_body": true
}
```

**Example output:**

```
feat(cache): make insert() return the replaced value

Changed LruCache::insert() to return Option<V> instead of unit,
providing callers with the previously stored value when a key
is overwritten. This enables callers to track evictions and
handle value replacement without an additional lookup.

Closes: #142
```
