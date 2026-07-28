---
name: error-recovery-planner
description: Analyze build, test, and runtime failures and propose structured recovery plans. Use when the agent encounters errors it cannot immediately resolve.
version: 1.0.0
author: go-on-team
tags: [error, recovery, planning, debug]
min_go_on_version: 1.0.0
---

# Error Recovery Planner

Analyze failures and propose structured recovery plans.

## When to use

- Build failures that require multiple fix attempts
- Test failures that are complex or non-obvious
- Runtime errors that need root cause analysis
- Any failure where the first fix attempt didn't work

## Recovery plan format

```markdown
## Recovery Plan: {failure_description}

### Root Cause Analysis
- **Error type**: {build_error | test_failure | runtime_error | unknown}
- **Location**: {file:line}
- **Likely cause**: {description}

### Attempted Fixes
1. {fix_description} — {success | failed: reason}

### Proposed Next Steps
1. {step description} — {expected outcome}
2. {step description} — {expected outcome}

### Escalation Criteria
- If step 2 fails, consider: {alternative approach}
- If all steps fail: {fallback plan}
```

## Workflow

1. Capture the full error output
2. Analyze root cause using error patterns
3. Propose up to 3 fix attempts in priority order
4. After each attempt, record whether it succeeded or failed
5. If all attempts fail, propose a fallback approach

## Input

When invoked, the user's request is available as:

```
{{input}}
```
