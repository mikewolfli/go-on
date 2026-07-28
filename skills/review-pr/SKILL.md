---
name: review-pr
description: Review a pull request diff and provide comprehensive feedback
version: 1.0.0
author: go-on-team
min_go_on_version: 1.0.0
---

# Pull Request Review Skill

Reviews pull request diffs and provides comprehensive, actionable feedback organized by severity with concrete improvement suggestions.

## How It Works

1. **Parse input** — Accepts a pull request diff (unified format), optionally with PR description and commit messages
2. **Analyze changes** — Evaluates the diff for correctness, style, performance, security, and test coverage
3. **Organize feedback** — Groups issues by severity (Critical/Major/Minor) and provides concrete code examples for improvements
4. **Assess test coverage** — Evaluates whether the change is adequately tested
5. **Format output** — Returns a structured review with overview, strengths, issues, suggestions, test assessment, and a decision

## Input Schema

| Parameter | Type | Description |
|-----------|------|-------------|
| `input` | string | Pull request diff (unified format), optionally including PR description and commit messages |

## Example

```
Input:
## Description
Add input validation to the user registration endpoint

## Diff
diff --git a/src/handlers/auth.rs b/src/handlers/auth.rs
+fn validate_registration(input: &RegistrationInput) -> Result<(), ValidationError> {
+    if input.username.len() < 3 {
+        return Err(ValidationError::new("Username must be at least 3 characters"));
+    }
+    if input.password.len() < 8 {
+        return Err(ValidationError::new("Password must be at least 8 characters"));
+    }
+    Ok(())
+}
```

## Example Output

```
Overview: Adds input validation to the user registration endpoint

Strengths:
- Clear validation logic with descriptive error messages

Issues:
- [Minor] Missing email format validation
- [Minor] Username length upper bound not enforced

Suggestions:
- Consider adding email regex validation
- Add a maximum length check for username (e.g., 50 chars)

Tests: No tests provided. Should add unit tests for valid/invalid inputs.

Decision: Needs Discussion
```

---

Review the following pull request diff and provide comprehensive, actionable feedback.

Input:
```
{{input}}
```

Provide a structured review with:
- **Overview**: What this PR does in one sentence
- **Strengths**: What's done well
- **Issues**: Problems found, organized by severity (Critical/Major/Minor)
- **Suggestions**: Concrete improvement suggestions with code examples
- **Tests**: Are tests adequate? What's missing?
- **Decision**: Approve / Changes Requested / Needs Discussion
