---
name: semantic-diff
description: Analyze code changes semantically — understand what changed, why, and potential impacts
version: 1.0.0
---

# Semantic Diff Skill

Analyzes the semantic meaning of code changes rather than just textual diffs. Understands what changed, why, and what the potential impacts are across the codebase.

## How It Works

1. **Parse input** — Accepts a diff (unified format) or a before/after code pair
2. **Semantic analysis** — Identifies the intent of the change, not just the line-level modifications. Understands function scope, module boundaries, and data flow.
3. **Impact assessment** — Evaluates side effects, potential regressions, and compatibility concerns
4. **Risk scoring** — Assigns a risk level based on change scope, affected dependencies, and test coverage gaps
5. **Format output** — Returns a structured analysis with summary, affected areas, purpose, risk assessment, and recommendation

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `input` | string | Code diff in unified format or before/after code blocks |

## Example

```
Input:
--- a/src/validation.rs
+++ b/src/validation.rs
@@ -15,7 +15,7 @@
 pub fn validate_email(email: &str) -> bool {
-    let re = Regex::new(r"^[^@]+@[^@]+\.[^@]+$").unwrap();
+    let re = Regex::new(r"^[^\s@]+@[^\s@]+\.[^\s@]+$").unwrap();
     re.is_match(email)
 }
```

## Example Output

```
**Summary**: Tighten email regex to reject addresses containing whitespace
**Affected Areas**: EmailValidator in `src/validation.rs`
**Purpose**: Prevent invalid email addresses with whitespace from passing validation
**Risk Assessment**: Low — change is narrowly scoped to one regex pattern
**Recommendation**: Approve
```

---

Analyze the following code changes semantically. Instead of just listing what lines changed, explain:

1. What is the **purpose** of this change?
2. What **functions, classes, or modules** were affected?
3. Are there any **potential side effects** or regressions?
4. Is the change **safe** to merge?

Input:
```
{{input}}
```

Provide a structured analysis with these sections:
- **Summary**: One-line description of the change
- **Affected Areas**: Files and components changed
- **Purpose**: Why this change was made
- **Risk Assessment**: Low/Medium/High with reasoning
- **Recommendation**: Approve, Reject, or Needs Changes
