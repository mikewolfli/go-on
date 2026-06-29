---
name: commit-message-generator
description: Generates Conventional Commits-style commit messages from git diffs or change descriptions
version: 1.0.0
author: go-on-team
tags: [git, commit, conventional-commits, version-control, workflow]
min_go_on_version: 1.0.0
---

# Commit Message Generator Skill

Analyzes code changes (from git diff or natural-language description) and generates well-structured commit messages following the Conventional Commits specification. Includes scope detection, breaking change identification, and body generation.

## How It Works

1. **Analyze** — Examines the diff or change description to categorize the type of change (feature, fix, refactor, docs, test, chore, etc.)
2. **Scope** — Detects the affected module/component from file paths (e.g. `src/api/` → `api`, `src/db/` → `db`)
3. **Compose** — Writes a concise subject line (<72 chars), optional body with context, and footer with breaking change notes or issue references
4. **Validate** — Checks the output conforms to the Conventional Commits specification and suggests improvements

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `diff` | string | Git diff output (`git diff`) or path-based change description |
| `description` | string | Optional: natural-language description of the change |
| `style` | string | Commit style: `conventional`, `angular`, `simple` (default: `conventional`) |
| `include_body` | boolean | Optional: generate detailed body (default: true) |
| `breaking_change` | string | Optional: description if the change is breaking |

## Example

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
