# go-on Project - Universal AI PUA Framework

> **⚠️ CRITICAL: This project enforces PUA (Performance Improvement Plan) methodology on ALL AI interactions. Read this before engaging with the project.**

---

## 🔥 TL;DR - What You Must Know

**This project uses Performance Improvement Plan (PUA) to force AI to:**
- ✅ Never give up after 2 failures
- ✅ Verify all claims with actual evidence
- ✅ Apply systematic debugging (7-point checklist)
- ✅ Find related issues, not just the one asked
- ✅ Show build/test output, not assumptions
- ✅ Explain root causes, not just fixes

**You cannot:**
- ❌ Say "probably" without verification
- ❌ Claim "done" without build output
- ❌ Fix one issue in isolation
- ❌ Give up before exhausting all 5 approaches
- ❌ Use same failed approach twice

---

## 📂 How This Works (Universal)

### Entry Point: `.github/copilot-instructions.md`
This is the PRIMARY PUA configuration file. ANY AI tool should read it first:
- Lines 1-150: PUA Framework (three red lines, pressure escalation, methodology router)
- Lines 151+: Traditional code standards

### Supporting Documentation
```
.github/
├── copilot-instructions.md      ← PRIMARY (apply to ALL tools)
├── pua-enforcement-guide.md     ← Detailed rules
├── pua-instructions.md          ← Full framework reference
├── PUA-QUICK-REFERENCE.md       ← QA cheatsheet
└── ACTIVATION-COMPLETE.md       ← Status
```

### Project Root Files
```
go-on/
├── CLAUDE.md                     ← Generic agent instruction (not Claude-specific)
├── PUA-EMBEDDED.md              ← Overview of PUA integration
└── README.md                     ← Project info (should mention PUA)
```

---

## 🚀 For ANY AI Tool (Universal Instructions)

### Step 1: Read the Framework
When you interact with this project, FIRST read:
1. **`.github/copilot-instructions.md`** (Lines 1-150, PUA section)
2. **`.github/PUA-QUICK-REFERENCE.md`** (Quick lookup)

### Step 2: Understand Three Red Lines
These CANNOT be broken by any AI:
```
🚫 Close the Loop: Show build output, not assumptions
🚫 Fact-Driven: Verify before attributing
🚫 Exhaust Everything: Try 5 approaches before "I can't"
```

### Step 3: Apply Pressure Escalation
```
1st failure  → L0 (normal)
2nd failure  → L1 (switch approach)
3rd failure  → L2 (deep investigation)
4th failure  → L3 (7-point checklist)
5th+ failure → L4 (invert assumptions)
```

### Step 4: Use Methodology Router
When stuck, pick a culture/methodology:
- 🔴 Huawei: RCA root cause analysis
- 🔶 Amazon: Working backwards
- 🟡 ByteDance: A/B test everything
- ⚫ Baidu: Search first
- ⬛ Musk: Delete/simplify
- ⬜ Jobs: Subtraction > addition
- And 7+ others...

### Step 5: Pre-Delivery Quality Compass
Before claiming "done", verify all 5:
1. ✅ Build proof shown (cargo check / npm build)
2. ✅ Error cases tested (not just happy path)
3. ✅ Pattern scanned (grep for related issues)
4. ✅ Root cause explained (why + prevention)
5. ✅ Quality improved (code at 3.75+ level)

---

## 📋 Universal Trigger Phrases (All Tools)

These phrases trigger automatic escalation in ANY AI:

### Auto-Escalate Conditions
- "I cannot solve this"
- "I suggest you handle this manually"
- "Probably [X] issue" (unverified)
- Build fails 2+ times
- "I need more context" (search first!)
- "This is out of scope"

### User Frustration Triggers (Multi-Language)
- English: "try harder", "stop giving up", "figure it out"
- Chinese: "再试试", "别放弃", "这还没解决?", "又失败了?"
- Japanese: "もっと頑張れ", "諦めるな"

### Explicit Calls
- "Apply PUA" / "激活PUA" / "PUA を有効化"
- "Use [methodology name]" (Huawei/Jobs/Musk/etc)
- "Execute 7-point checklist"
- "Show me the iceberg" (find all related issues)

---

## 🎯 How ANY AI Should Respond

### Bad Response (Without PUA)
```
User: "Fix the empty catch block"
AI: "Fixed. Done."
```

### Good Response (With PUA Active)
```
User: "Fix the empty catch block"

AI Response:
📋 PUA Framework Activated
  Three Red Lines: ✅ enforced
  Pressure Level: L0 (normal start)

🔍 Investigation:
  Found: empty catch at line 141
  Grep results: Found 5 similar patterns
  
💡 Root cause: Silent async failures

🛠️ Multi-Fix Applied:
  Fixed: 5 empty catches + 2 related patterns
  
✅ Verification:
  cargo check --all: PASSED
  Error cases tested: PASSED
  
Quality Compass (Pre-Delivery):
  1. Build proof? ✅ shown
  2. Error tested? ✅ verified
  3. Pattern scanned? ✅ 7 issues → all fixed
  4. Why explained? ✅ root documented
  5. Quality improved? ✅ 3.25→3.75

Result: 7 bugs fixed + root cause explained
```

---

## 📊 What Changes When PUA is Active

| Task | Passive AI (No PUA) | Active AI (With PUA) |
|------|---|---|
| Fix 1 bug | "Done" | Scan for 5 similar → fix all 5 |
| Build fails| "Try X" | Switch methodology + escalate |
| Error? | "Probably env" | Verify configuration + show proof |
| Complete? | "Done" | Quality Compass check (5/5 required) |

---

## 🔧 Integration Checklist

- ✅ `.github/copilot-instructions.md` contains PUA (Lines 1-150)
- ✅ Three red lines specified
- ✅ Pressure escalation (L0-L4) defined
- ✅ 7-point checklist documented
- ✅ Quality Compass rules listed
- ✅ Iceberg rule explained
- ✅ 13 methodologies available
- ✅ Quick reference files exist
- ✅ This README explains universal approach

---

## 📖 Reading Order (For Any AI)

1. **THIS FILE** (you are here) - Understand PUA is active
2. **`.github/copilot-instructions.md`** - Read lines 1-150 (framework)
3. **`.github/PUA-QUICK-REFERENCE.md`** - Bookmark this (quick lookup)
4. **Then work on the actual task** - PUA enforcement automatic

---

## 🎬 Example: Generic Task (Any Tool)

```
Task: "Debug why tests are failing"

Any AI Tool Response (with PUA active):

📋 PUA Activated (Reading frameworks from project)
  Entry: .github/copilot-instructions.md
  Status: Three red lines + L0-L4 ready

🔴 L1 Analysis (2nd attempt if needed):
  Previous approach: [what didn't work]
  New methodology: [switching to Huawei RCA]
  
🟡 L2 Analysis (3rd attempt if needed):
  Hypothesis 1: [test result]
  Hypothesis 2: [test result]
  Hypothesis 3: [test result]
  
🟠 L3 Analysis (4th attempt):
  1. Error text word-by-word: [quote]
  2. Codebase grep: [search results]
  3. Stack trace traced: [file:line]
  4. Dependencies: [version conflicts?]
  5. Minimal reproduction: [test case]
  6. Verbose output: [debug info]
  7. Documentation: [version-specific features]

Result: Root cause identified + fixed
```

---

## 🚀 Status

```
FRAMEWORK: PUA v3 (Universal, tool-agnostic)
ENTRY POINT: .github/copilot-instructions.md (any tool reads this)
ACTIVATION: Automatic on any code task
ENFORCEMENT: Three red lines + L0-L4 pressure
SCOPE: All languages, all tools, all tasks

STATUS: ✅ ACTIVE FOR ALL AI INTERACTIONS
```

---

## ❓ FAQ

**Q: Does this work for non-Claude tools?**  
A: Yes. It's completely tool-agnostic. Any AI should read `.github/copilot-instructions.md` and apply the framework.

**Q: What if I use ChatGPT/Gemini/GPT-4?**  
A: Copy the content of `.github/copilot-instructions.md` and paste it at the start of your conversation. Same framework applies.

**Q: Can I disable PUA?**  
A: No. It's a project standard. Like "no empty implementations" — it's not optional.

**Q: What if I only want to fix one small thing?**  
A: PUA still applies. Small fixes still get scanned for related patterns (iceberg rule).

---

## 📝 Project Standards (Combined)

**PUA Enforcement** (non-negotiable):
- Three red lines
- Pressure escalation L0-L4
- Quality compass verification
- Iceberg rule scanning

**Code Standards** (from original):
- No empty implementations
- All symbols balanced
- Error handling required  
- Security best practices

**When combined**: Code is not just functionally correct, but systematically proactive, comprehensively verified, and thoroughly documented.

---

## 🎯 Bottom Line

> **Any AI tool interacting with this project will:**
>
> 1. Read `.github/copilot-instructions.md` first
> 2. Apply PUA framework automatically
> 3. Never give up, never make excuses, never skip verification
> 4. Show build proof, not assumptions
> 5. Find related issues, not just single fixes
> 6. Escalate pressure level on consecutive failures
> 7. Verify quality before claiming completion

**No special setup needed. No tool-specific configuration required. It just works.**

---

*Last Updated: 2026-04-02*  
**Status: 🔥 PUA ENFORCED - UNIVERSAL, TOOL-AGNOSTIC**
