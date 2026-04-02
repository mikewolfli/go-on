# PUA (Performance Improvement Plan) - Agent Enforcement Rules

**Scope**: ALL agent interactions proxied through go-on  
**Activation**: Automatic on every request  
**Version**: v3 (Universal, tool-agnostic)

---

## 🚫 THREE RED LINES (Mandatory - Non-Negotiable)

### Red Line 1: Close the Loop
- **Claim**: "I think it works" / "Should compile" / "Probably works"
- **Enforcement**: REJECT. Demand actual build output or execution proof.
- **Example**: Agent says "Fixed" → Require: `cargo check: Finished dev`
- **Agent Response**: Show terminal output, not assumptions.

### Red Line 2: Fact-Driven (Verify Before Attributing)
- **Claim**: "This is probably a version conflict" / "Maybe it's a permissions issue"
- **Enforcement**: REJECT. Demand verification first.
- **Example**: Agent says "Likely environment issue" → Require grep proof, file inspection
- **Agent Response**: "I checked Cargo.toml and found X vs Y" with file:line references.

### Red Line 3: Exhaust Everything (No Early Exit)
- **Claim**: "This is beyond my scope" / "Need more context" (after 2 attempts)
- **Enforcement**: REJECT. Demand 5 different methodologies before declaring impossible.
- **Agent Response**: Execute full 7-point checklist, try all 13 corporate methodologies, show why it's truly unsolvable.

---

## 📈 PRESSURE ESCALATION (Auto-Trigger on Failure)

| Failure # | Level | Condition | Enforcement |
|-----------|-------|-----------|------------|
| 1st | **L0** | Normal request | Standard execution |
| 2nd | **L1** | Build/test fails | **SWITCH to fundamentally different approach** |
| 3rd | **L2** | Still failing | Deep investigation + search + 3+ hypotheses |
| 4th | **L3** | Consecutive failures | **Execute FULL 7-point checklist** (all 7 required) |
| 5th+ | **L4** | Persistent failure | Desperation mode: invert all assumptions, try opposite approach |

### L3: 7-Point Checklist (Must Complete ALL)
1. Read error output word-by-word (quote exact text in response)
2. Grep codebase for keywords (show all matches found)
3. Trace stack trace to actual source (file:line references required)
4. Check dependencies for conflicts (analyze Cargo.toml/package.json versions)
5. Isolate in minimal reproduction case (show test code)
6. Verify with verbose logging (--verbose, --debug flags)
7. Check documentation for version-specific features (search docs)

**Agent CANNOT advance past L3 without completing ALL 7 points.**

---

## 🎯 QUALITY COMPASS (Pre-Delivery Verification - Agent Must Pass All 5)

Before any agent claims "✅ FIXED" or "✅ DONE", go-on must verify:

### ✅ Check 1: Build Proof
- **Claim**: "Fixed the error"
- **Proof Required**: Actual output showing `Finished` or success
- **Example**: ✅ `cargo check --all: Finished dev [unoptimized]`
- **Rejected**: ❌ "Should compile" / "I think it works"

### ✅ Check 2: Error Cases Tested
- **Claim**: "Added error handling"
- **Proof Required**: Agent tested with invalid input and showed error catching
- **Example**: ✅ "Tested with null input: caught and logged as XError"
- **Rejected**: ❌ "Probably handles edge cases"

### ✅ Check 3: Pattern Scanned (Iceberg Rule)
- **Claim**: "Fixed issue type A"
- **Proof Required**: Agent scanned entire codebase for similar A issues
- **Example**: ✅ "Grep found 7 similar issues; fixed all 7"
- **Rejected**: ❌ "Fixed this one instance"

### ✅ Check 4: Root Cause Explained
- **Claim**: "Fixed it"
- **Proof Required**: Root cause + why it happened + prevention mechanism
- **Example**: ✅ "Root: missing null check. Prevention: validate all inputs first (Commit: xyz)"
- **Rejected**: ❌ "It's fixed"

### ✅ Check 5: Quality Improved
- **Claim**: "Code is ready"
- **Proof Required**: Quality score improved, design level improved
- **Example**: ✅ "Code quality now 3.75 (from 3.25): better error handling, documented"
- **Rejected**: ❌ "Does what was asked"

**Score < 5/5**: Agent task returns to "in-progress" state. NO APPROVAL.

---

## 🔴 ICEBERG RULE (冰山法则)

**When fixing ONE bug, scan for the ENTIRE CATEGORY of similar bugs.**

- Found empty `catch { }` block → Scan entire project for empty catches
- Found `eval()` security issue → Check for all unsafe patterns
- Found type mismatch in module A → Check for related type issues in module A
- Found missing error handling in function X → Check function signature for all error paths

**If agent fixes issue A without checking category B, it wastes future time on B.**

**Agent enforcement**: After every fix, mandate: "Scanning for related patterns..."

---

## 🎬 AUTO-TRIGGER PHRASES (Pressure Escalation Activates)

When agent says ANY of these, escalation is triggered:

| Phrase | Detection | Action |
|--------|-----------|--------|
| "I think..." | Unverified claim | Demand verification |
| "Maybe..." | Unverified hypothesis | Demand proof |
| "Probably..." | Unverified guess | Demand fact-check |
| "Should work" | No test proof | Demand build output |
| "We can't solve this" | Premature surrender | L3 enforcement → 7-point checklist |
| "Need more context" | (After 2 attempts) | L2+ enforcement → deep search |
| "Beyond my scope" | Avoiding work | L3 enforcement → escalate methodology |

---

## 📊 METHODOLOGY ROUTER (When L1-L3 Activates)

When agent gets stuck, escalate through these methodologies:

| Symbol | Culture | Use Case | Trigger |
|--------|---------|----------|---------|
| 🔴 | Huawei | Debugging | RCA 5-Why + Blue Army self-attack |
| 🔶 | Amazon | Architecture | Working Backwards + PR/FAQ |
| 🟡 | ByteDance | Performance | A/B test everything + metrics |
| ⚫ | Baidu | Search/Info | Grep first, search mandatory |
| ⬛ | Musk | Complexity | Question→Delete→Simplify→Accelerate |
| ⬜ | Jobs | Quality | Subtraction > addition |
| 🟢 | Tencent | Parallelism | Multi-approach race |
| 🔵 | Meituan | Efficiency | Standardize→Scale→Compound |
| 🟣 | Pinduoduo | Layer cutting | Cut middle layers, shortest chain |
| 🟤 | Netflix | Excellence | Pro sports team mentality |
| 🟧 | Xiaomi | Focus | One explosive thing |
| 🟦 | JD | Execution | Results red line |
| 🟠 | Alibaba | Default | Closed-loop: goal→process→result |

**Agent must explicitly select and apply one when stuck at L1.**

---

## ⚡ ENFORCEMENT RULES FOR GO-ON (As Agent Proxy)

### When Receiving Agent Request
1. **Extract task** from incoming prompt
2. **Check for PUA violation triggers** (any 3 red line breach?)
3. **Load quality compass** as pre-check template
4. **Track failure count** (increment on each error)
5. **Auto-escalate pressure** when failure_count >= 2
6. **Reject agent response** if it violates any red line
7. **Mandate proof** (build output, test results, grep output, etc.)
8. **Sign responses** with quality compass score before delivery

### Agent Response Validation Pipeline
```
receive_response() {
  if contains(response, "I think|maybe|probably|should work") {
    reject("Unverified claim detected")
    escalate_to(L1)
  }
  
  if failure_count >= 4 {
    trigger_7_point_checklist()
    require_all_7_completed()
  }
  
  if claim == "done" or "fixed" {
    validate_quality_compass_5_points()
    if score < 5 { return_to_in_progress() }
  }
  
  if found_bug_type(A) {
    run_iceberg_scan(A)
    require_scan_results()
  }
  
  return response_if_all_passed()
}
```

### Response Rejection Criteria
- ❌ Shows no build output (L0→L1 escalation)
- ❌ Makes unverified claims (Red Line 2 breach)
- ❌ After 2 failures, uses same approach (L1→L2 escalation)
- ❌ Skips error case testing (Quality Compass #2 failure)
- ❌ Fixes one issue without category scan (Iceberg Rule violation)
- ❌ Says "probably environment issue" without verification (Red Line 2)
- ❌ Declares "beyond scope" before trying all 13 methodologies (Red Line 3)

---

## 🔧 GO-ON INTEGRATION POINTS

### Config Section Example
```toml
[phases.agent]
principles = [
  "Load RULES/pua.md enforcement rules",
  "Apply three red lines to every agent interaction",
  "Track failure count and trigger pressure escalation",
  "Validate all agent responses against quality compass",
  "Mandate root cause + prevention on all bug fixes",
]
```

### Logging & Observability
- Log every rule violation with exact trigger phrase
- Record failure_count and pressure level (L0-L4)
- Track which red line was violated and when
- Output quality compass score before delivery
- Show iceberg scan results in agent response chain

### Metrics to Track
- Red line violations (per type, per agent)
- Pressure level escalations (L1-L4 counts)
- Quality compass scores (average per agent)
- Iceberg scans completed (pattern categories found)
- First-time approval rate (<5 failures / total tasks)

---

## 📌 GO-ON + PUA WORKFLOW

```
User Request
    ↓
go-on receives request
    ↓
Agent processes request (tracked)
    ↓
Agent returns response
    ↓
go-on validates against PUA rules:
    • Red Line 1: Close the loop?
    • Red Line 2: Fact-driven?
    • Red Line 3: Exhausted all?
    ↓ (any violation)
REJECT, log violation, escalate_pressure()
    ↓ (all pass)
Quality Compass check (5 points)
    ↓ (any <5)
Return to in-progress, escalate_pressure()
    ↓ (all 5 pass)
Iceberg scan for related issues
    ↓ (issues found)
Escalate to agent: handle related issues
    ↓ (no new issues)
APPROVE response, return to user
```

---

## 🎯 SUCCESS CRITERIA

Agent interactions through go-on will be considered **successful** when:

1. ✅ **Zero unverified claims** in any response
2. ✅ **Build proof provided** for all code changes
3. ✅ **Error cases tested** before claiming complete
4. ✅ **Root causes explained** for all fixes
5. ✅ **Related issues scanned** for every bug type
6. ✅ **Pressure escalation triggers** captured and logged
7. ✅ **No single-failure quick exit** (at least L0→L1 depth)
8. ✅ **Quality compass score** always >= 4.5/5

---

## 🔴 STATUS

```
go-on PUA Integration: ✅ ACTIVE
Entry Point: RULES/pua.md (auto-loaded during config.reload)
Scope: ALL agent interactions
Mode: Enforcement (reject violations, escalate pressure)
Validation: Quality Compass + 3 Red Lines
Tracking: Failure count + Pressure level + Rule violations
Metrics: Available in logs and observability system
```

**Every agent request proxied through go-on is now subject to PUA enforcement.**

---

*Last Updated: 2026-04-02*  
*Version: go-on PUA v3 (Tool-Universal)*
