---
name: refactoring-advisor
description: Analyzes source code for refactoring opportunities, design smells, and improvement suggestions
version: 1.0.0
author: go-on-team
tags: [refactoring, code-quality, design-patterns, clean-code, best-practices]
min_go_on_version: 1.0.0
---

# Refactoring Advisor Skill

Analyzes source code for common design smells, over-engineering, duplication, and violation of best practices. Produces prioritized, actionable refactoring suggestions with before/after examples and estimated effort.

## How It Works

1. **Scan** — Parses the provided source code to identify structural patterns, function sizes, nesting depth, and naming conventions
2. **Detect** — Recognizes 20+ code smells: god functions, shotgun surgery, primitive obsession, long parameter lists, deep nesting, magic numbers/strings, duplicate logic, premature optimization, over-abstraction, inappropriate intimacy
3. **Prioritize** — Ranks findings by impact (maintainability, performance, testability, readability) and estimated effort (minutes, hours, days)
4. **Suggest** — Produces concrete refactoring recommendations with idiomatic before/after code examples and references to standard refactoring patterns (Extract Method, Replace Conditional with Polymorphism, Introduce Parameter Object, etc.)

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `code` | string | Source code to analyze |
| `language` | string | Programming language (rust, python, ts, go, java) |
| `strictness` | string | Analysis depth: `basic`, `balanced`, `thorough` (default: `balanced`) |
| `include_examples` | boolean | Optional: include before/after code examples (default: true) |

## Example

```json
{
  "code": "fn process(data: &str) -> String {\n    let parts: Vec<&str> = data.split(',').collect();\n    let mut result = String::new();\n    for (i, part) in parts.iter().enumerate() {\n        let trimmed = part.trim();\n        if trimmed.len() > 0 {\n            if i > 0 {\n                result.push_str(\", \");\n            }\n            if trimmed.starts_with(\"$\") || trimmed.starts_with(\"#\") {\n                if trimmed.starts_with(\"$\") {\n                    result.push_str(&format!(\"VARIABLE:{}\", &trimmed[1..]));\n                } else {\n                    result.push_str(&format!(\"META:{}\", &trimmed[1..]));\n                }\n            } else if trimmed.parse::<f64>().is_ok() {\n                result.push_str(&format!(\"NUM:{}\", trimmed));\n            } else {\n                result.push_str(&format!(\"STR:{}\", trimmed));\n            }\n        }\n    }\n    result\n}",
  "language": "rust",
  "strictness": "balanced",
  "include_examples": true
}
```

**Example output (abbreviated):**

```markdown
# Refactoring Analysis

## 🔴 High Priority

### 1. God Function — process() does too much
**Lines:** 1-28 | **Effort:** ~30 minutes

The function handles splitting, type detection, formatting, and string building.
**Suggestion:** Extract type detection and formatting into separate functions.

```rust
enum Token { Variable(String), Meta(String), Numeric(f64), Text(String) }

fn classify_token(s: &str) -> Option<Token> {
    let s = s.trim();
    if s.is_empty() { return None; }
    Some(if let Some(name) = s.strip_prefix('$') {
        Token::Variable(name.to_string())
    } else if let Some(name) = s.strip_prefix('#') {
        Token::Meta(name.to_string())
    } else if let Ok(n) = s.parse::<f64>() {
        Token::Numeric(n)
    } else {
        Token::Text(s.to_string())
    })
}
```

## 🟡 Medium Priority

### 2. Magic Strings — Use an enum instead of inline "$" / "#" prefixes
**Lines:** 11-12 | **Effort:** ~15 minutes

## 🟢 Low Priority

### 3. Unnecessary Allocation — `format!()` on every iteration
**Lines:** 13-19 | **Effort:** ~10 minutes

**Summary:** 1 critical, 2 medium, 4 low suggestions found. Estimated total effort: ~2 hours.
```
