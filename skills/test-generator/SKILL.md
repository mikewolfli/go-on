---
name: test-generator
description: Generates unit, integration, and property-based tests for source code in multiple languages
version: 1.0.0
author: go-on-team
tags: [test, testing, coverage, rust, python, typescript]
min_go_on_version: 1.0.0
---

# Test Generator Skill

Analyzes source code and generates comprehensive test suites including unit tests, integration tests, edge cases, and property-based tests. Supports Rust (`#[cfg(test)]` modules), Python (`pytest`/`unittest`), TypeScript (`jest`/`vitest`), and Go (`testing` package).

## How It Works

1. **Parse** — Analyzes the provided source code to extract function signatures, types, trait bounds, and control flow
2. **Identify** — Detects edge cases (empty input, null/none, boundary values, error paths) and property invariants
3. **Generate** — Produces idiomatic test code matching the target language's testing conventions
4. **Validate** — Ensures generated tests compile (syntax-check) and reference only existing identifiers

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `code` | string | Source code to generate tests for |
| `language` | string | Programming language (rust, python, ts, go) |
| `test_framework` | string | Optional: specific framework (default: language-standard) |
| `include_edge_cases` | boolean | Optional: include boundary/edge-case tests (default: true) |
| `include_property_tests` | boolean | Optional: include property-based tests (default: false) |

## Example

```json
{
  "code": "fn parse_version(s: &str) -> Result<(u32, u32, u32), String> {\n    let parts: Vec<&str> = s.split('.').collect();\n    if parts.len() != 3 { return Err(\"expected 3 parts\".into()); }\n    let major = parts[0].parse().map_err(|_| \"invalid major\")?;\n    let minor = parts[1].parse().map_err(|_| \"invalid minor\")?;\n    let patch = parts[2].parse().map_err(|_| \"invalid patch\")?;\n    Ok((major, minor, patch))\n}",
  "language": "rust",
  "include_edge_cases": true,
  "include_property_tests": false
}
```

**Example output (abbreviated):**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_semver() {
        assert_eq!(parse_version("1.2.3").unwrap(), (1, 2, 3));
    }

    #[test]
    fn rejects_empty_string() {
        assert!(parse_version("").is_err());
    }

    #[test]
    fn rejects_too_few_parts() {
        assert!(parse_version("1.2").is_err());
    }

    #[test]
    fn rejects_too_many_parts() {
        assert!(parse_version("1.2.3.4").is_err());
    }

    #[test]
    fn rejects_non_numeric_component() {
        assert!(parse_version("1.x.3").is_err());
    }
}
```
