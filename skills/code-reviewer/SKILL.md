---
name: code-reviewer
description: Automated code review with customizable quality rules
version: 1.2.0
author: go-on-team
tags: [code, review, quality, rust, python]
min_go_on_version: 1.0.0
---

# Code Reviewer Skill

Automated code review that checks for common issues, style violations,
and potential bugs across multiple programming languages.

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `code` | string | Source code to review |
| `language` | string | Programming language (rust, python, ts, go) |
| `rules` | string[] | Optional: specific rules to check |

## Example

```json
{
  "code": "fn add(a: i32, b: i32) -> i32 { a + b }",
  "language": "rust",
  "rules": ["naming", "documentation"]
}
```
