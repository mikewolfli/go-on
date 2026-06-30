---
name: code-reviewer
description: Automated code review with customizable quality rules
version: 1.3.0
author: go-on-team
tags: [code, review, quality, rust, python, typescript, go]
min_go_on_version: 1.0.0
---

# Code Reviewer Skill

Automated code review that checks for common issues, style violations,
and potential bugs across multiple programming languages. Reviews are
returned as structured feedback with severity levels and line references.

## How It Works

1. **Parse input**: Extract the source code, language, and optional rule set from the input parameters.
2. **Analyze code**: Scan the code for issues matching the specified rules (or default rules if none are provided). Each language has built-in checks for naming conventions, documentation requirements, error handling patterns, and common anti-patterns.
3. **Score findings**: Each issue is assigned a severity level — `error` for definite bugs, `warning` for style/correctness concerns, and `suggestion` for improvements.
4. **Format output**: Return a structured JSON result with the overall score, a list of findings, and optional fix suggestions.

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `code` | string | Source code to review |
| `language` | string | Programming language (`rust`, `python`, `typescript`, `go`, `java`, `cpp`) |
| `rules` | string[] | Optional: specific rule categories to check (e.g. `naming`, `documentation`, `error-handling`, `security`, `performance`) |

## Example

```json
{
  "code": "fn add(a: i32, b: i32) -> i32 { a + b }",
  "language": "rust",
  "rules": ["naming", "documentation"]
}
```

## Example Output

```json
{
  "score": 0.85,
  "summary": "Minor documentation issues found",
  "findings": [
    {
      "line": 1,
      "column": 1,
      "severity": "suggestion",
      "rule": "documentation",
      "message": "Function `add` is missing doc comment",
      "suggestion": "Add a `///` doc comment describing the function's purpose, arguments, and return value"
    },
    {
      "line": 1,
      "column": 3,
      "severity": "suggestion",
      "rule": "naming",
      "message": "Parameter `a` has a short name; consider a more descriptive identifier",
      "suggestion": "Rename `a` to something meaningful like `left` or `augend`"
    }
  ],
  "language": "rust",
  "total_lines": 1
}
```
