---
name: regex-builder
description: Builds, explains, and tests regular expressions with step-by-step breakdowns
version: 1.0.0
author: go-on-team
tags: [regex, regular-expression, text-processing, pattern-matching]
min_go_on_version: 1.0.0
---

# Regex Builder Skill

Helps construct regular expressions from natural-language descriptions, explains what an existing regex does in plain English, and tests patterns against sample text. Supports PCRE, ECMAScript, Rust `regex` crate, Python `re` module, and Go `regexp` syntax variants.

## How It Works

1. **Describe to Regex** — Converts natural language ("match an email address") into a working regex with explanation of each component
2. **Regex to Explain** — Parses an existing regex token by token and produces a plain-English breakdown of what it matches
3. **Test** — Runs the regex against provided sample strings and reports matches, captures, and positions
4. **Optimize** — Suggests simplifications, removes unnecessary escapes, converts capturing groups to non-capturing, and flags catastrophic backtracking patterns
5. **Translate** — Converts regex between different engine syntaxes (e.g., PCRE lookbehinds → JavaScript alternatives)

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `description` | string | Natural-language description (used when `action=generate`) |
| `pattern` | string | Existing regex pattern (used when `action=explain` or `action=test`) |
| `test_strings` | string[] | Optional: sample strings to test the pattern against |
| `engine` | string | Regex engine: `rust`, `python`, `javascript`, `pcre`, `go` (default: `rust`) |
| `action` | string | Action: `generate`, `explain`, `test`, `optimize` (default: `generate`) |
| `flags` | string | Optional: regex flags (`i`, `m`, `s`, `x`, combined) |

## Example

```json
{
  "description": "Match a valid HTTP or HTTPS URL with optional path, query string, and fragment",
  "engine": "rust",
  "action": "generate",
  "test_strings": [
    "https://example.com/path?q=search#section",
    "http://localhost:8080",
    "ftp://invalid"
  ]
}
```

**Example output (abbreviated):**

```rust
// Generated pattern:
// ^https?://[\\w.-]+(?::\\d{1,5})?(?:/[\\w./-]*)*(?:\\?[\\w&=.-]*)?(?:#[\\w-]*)?$

// Breakdown:
// ^                     ─ Start of string
// https?                ─ Literal "http" + optional "s"
// ://                   ─ Literal "://"
// [\\w.-]+              ─ Domain name (word chars, dots, hyphens)
// (?::\\d{1,5})?        ─ Optional port number
// (?:/[\\w./-]*)*       ─ Optional path segments
// (?:\\?[\\w&=.-]*)?    ─ Optional query string
// (?:#[\\w-]*)?         ─ Optional fragment
// $                     ─ End of string

// Test results:
// ✅ https://example.com/path?q=search#section — MATCH
// ✅ http://localhost:8080 — MATCH
// ❌ ftp://invalid — NO MATCH (expected: wrong protocol)
```
