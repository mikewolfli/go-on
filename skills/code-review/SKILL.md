---
name: code-review
description: Two-mode code review — diff review (git PR/branch changes) and snippet review (static code quality analysis with language-aware scoring)
version: 2.0.0
tags: [code, review, quality, pr, git, rust, python, typescript, go]
---

# Code Review Skill

Two modes of code review:

- **`diff`** — Review changes since a fixed point (commit, branch, tag) along two axes: **Standards** (code conventions + Fowler smells) and **Spec** (does the code match the issue/PRD?)
- **`snippet`** — Static analysis of a code snippet with language-aware checks for naming, documentation, error handling, security, and performance

---

## Mode: `diff` — Git Diff / PR Review

Review the diff between `HEAD` and a user-supplied fixed point:

- **Standards** — does the code conform to the repo's documented coding standards?
- **Spec** — does the code faithfully implement the originating issue / PRD / spec?

Both axes run as **parallel sub-agents** so they don't pollute each other's context.

### Process

#### 1. Pin the fixed point

The fixed point the user specified — commit SHA, branch name, tag, `main`, `HEAD~5`, etc. If they didn't specify one, ask.

Capture: `git diff <fixed-point>...HEAD` (three-dot, merge-base comparison). Also `git log <fixed-point>..HEAD --oneline`.

Confirm the ref resolves (`git rev-parse <fixed-point>`) and the diff is non-empty.

#### 2. Identify the spec source

Look for the originating spec, in order:
1. Issue references in commit messages (`#123`, `Closes #45`, etc.)
2. A path the user passed as an argument
3. A PRD/spec file under `docs/`, `specs/`, `.scratch/`
4. If nothing found, ask. If none exists, the **Spec** sub-agent skips.

#### 3. Identify the standards sources

Repo standards files (`CODING_STANDARDS.md`, `CONTRIBUTING.md`, etc.) plus the **Fowler smell baseline**:

- **Mysterious Name** / **Duplicated Code** / **Feature Envy** / **Data Clumps**
- **Primitive Obsession** / **Repeated Switches** / **Shotgun Surgery**
- **Divergent Change** / **Speculative Generality** / **Message Chains**
- **Middle Man** / **Refused Bequest**

Two rules: (a) repo overrides baseline; (b) each smell is a judgement call, not a hard violation.

#### 4. Spawn parallel sub-agents

Send one message with two `Agent` tool calls (general-purpose subagent).

#### 5. Aggregate

Present `## Standards` and `## Spec` sections verbatim. Do not merge or rerank. End with one-line summary per axis.

---

## Mode: `snippet` — Static Code Snippet Analysis

Analyze a code snippet for quality, style, and bugs across multiple languages.

### How It Works

1. **Parse input**: Extract source code, language, and optional rule set
2. **Analyze code**: Scan for issues — naming conventions, documentation, error handling, anti-patterns. Each language has built-in checks.
3. **Score findings**: Each issue assigned a severity — `error` (definite bugs), `warning` (style/correctness), `suggestion` (improvements)
4. **Format output**: Return structured JSON with overall score and findings

### Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `mode` | string | `"diff"` or `"snippet"` (default: `"diff"`) |
| `code` | string | Source code to review (required for `snippet` mode) |
| `language` | string | Programming language (`rust`, `python`, `typescript`, `go`, `java`, `cpp`) |
| `rules` | string[] | Optional: rule categories (`naming`, `documentation`, `error-handling`, `security`, `performance`) |

### Example

```json
{
  "mode": "snippet",
  "code": "fn add(a: i32, b: i32) -> i32 { a + b }",
  "language": "rust",
  "rules": ["naming", "documentation"]
}
```

### Example Output

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
