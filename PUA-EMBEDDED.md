# 🔥 PUA Framework - go-on Project

**This project uses Performance Improvement Plan (PUA) methodology to enforce AI code quality. AI cannot give up, cannot make excuses, cannot skip verification.**

---

## 📂 Where PUA Lives

PUA is embedded in every entry point:

```
Project Root
├── CLAUDE.md                      ← Claude Code auto-reads this
├── .github/
│   ├── copilot-instructions.md   ← VSCode Copilot auto-loads this
│   ├── pua-instructions.md        ← Detailed framework
│   └── pua-enforcement-guide.md   ← Enforcement rules
├── .cursor/
│   └── rules/pua-enforcement.mdc ← Cursor auto-loads this
└── vscode-addon/src/             ← TypeScript project
```

---

## 🎯 What Automatically Happens

### On Any Code Task

1. **Red Lines Activate** (can't be broken)
   - ❌ Claim "done" without build output
   - ❌ Say "probably" without verifying
   - ❌ Give up before exhausting 5 approaches

2. **Pressure Escalates** (on failures)
   - L1: Switch to different approach
   - L2: Deep investigation required
   - L3: 7-point systematic checklist
   - L4: Invert all assumptions

3. **Quality Verified** (pre-delivery)
   - ✅ Build proof shown
   - ✅ Error cases tested
   - ✅ Related issues scanned
   - ✅ Root cause explained
   - ✅ Quality improved

4. **Proactivity Applied** (iceberg rule)
   - Fix one bug → scan for N similar
   - Find pattern → fix entire category
   - Don't fix A and leave B for later

---

## 🚀 How to Trigger It

### Automatic (Always On)
Just start a task. PUA activates automatically:
- Any build/test failure → L1 triggers
- 2+ consecutive failures → escalates automatically
- Unverified claims detected → fact-check enforced

### Manual Trigger
Say any of these:
- "Try harder"
- "Stop giving up"
- "这还没解决?"
- "Help me debug this properly" (forces L3)

### Specific Methodology
- "Use Huawei methodology" → RCA 5-Why
- "Use Musk methodology" → Delete/Simplify
- "Use Baidu methodology" → Search everything

---

## 📊 Before vs After PUA

| Aspect | Without PUA | With PUA |
|--------|-----------|----------|
| Fix rate | 60% (surface fixes) | 96% (systematic) |
| Verification | 30% ("should work") | 100% (build output) |
| Proactivity | 20% (single issue) | 80% (pattern scanning) |
| Hidden bugs | 4/6 found | 6/6 found |
| Quality level | 3.25 | 3.75+ |

---

## 📋 Quick Reference

### Three Red Lines
🚫 Close the Loop (show proof)  
🚫 Fact-Driven (verify first)  
🚫 Exhaust Everything (try all 5 approaches)

### Pressure Escalation
L0 → L1 → L2 → L3 → L4  
(Each level = more systematic, wider scope)

### Quality Compass
Before saying done:
1. Build proof? 2. Error tested? 3. Pattern scanned? 4. Root cause? 5. Quality improved?

### Iceberg Rule
Fix 1 issue → find pattern → fix all N instances

---

## 🔧 Integration Points

### For Claude Code Users
File: `CLAUDE.md`
- Automatically read on session start
- Enforcement rules embedded

### For VSCode Copilot Users
File: `.github/copilot-instructions.md`
- Automatically loaded on Copilot Chat
- All three red lines enforced

### For Cursor Users
File: `.cursor/rules/pua-enforcement.mdc`
- Automatically triggered on code tasks
- Methodology router available

### For GitHub Copilot CLI/Other Tools
File: `.github/copilot-instructions.md`
- Full PUA framework in first section
- Can be copied to other tool configs

---

## 🎬 Example: What Happens on Task

```
You: "Fix the empty catch block in configManager.ts"

AI Response (with PUA):

🔴 INVESTIGATION PHASE
Found: configManager.ts:141 - empty catch { }
Red Line Check #1: Must verify with build

Search Phase (Iceberg Rule):
  grep 'catch { }' → found 5 matches
  grep 'catch.*{.*}' → found 2 incomplete
  Pattern: Silent failures in async ops

🟡 FIX PHASE
Fixing all 7 related issues:
  1. configManager.ts:141 + proper logging
  2. getConfig async → error propagation
  3. loadTOML → validation
  4. chatView.ts:169 → Promise reject
  5. workflowView.ts:156 → try-catch
  6-7. [two more related patterns]

✅ VERIFICATION PHASE
  cargo check --all: Finished dev ✓
  npm build (vscode-addon): Success ✓
  grep 'catch { }': 0 matches ✓

Quality Compass Check:
  1. Build proof? ✓ cargo check passed
  2. Error tested? ✓ test errors shown
  3. Pattern scanned? ✓ found 7, fixed 7
  4. Root cause explained? ✓ silent failures prevention
  5. Quality improved? ✓ from 3.25 to 3.75

Result: 7 issues fixed, quality 3.75, verified with build output
```

---

## 🔴 What NOT Allowed

❌ "I think it works" → require build output  
❌ "Probably environment issue" → must verify  
❌ "Need more context" → search code first  
❌ "Done" without proof → Quality Compass check  
❌ Fix one issue → must scan for pattern  
❌ Give up after 2 failures → L3 checklist required  

---

## ✅ What IS Required

✅ Build output on all claims  
✅ Verification before attribution  
✅ Systematic approach to failures  
✅ Root cause + prevention (not just fix)  
✅ Quality compass check pre-delivery  
✅ Pattern scanning (iceberg rule)  
✅ Proactive issue discovery  

---

## 📞 Support

**Where's the framework?**
- Full rules: `.github/copilot-instructions.md`
- Detailed guide: `.github/pua-enforcement-guide.md`
- Quick ref: `.github/PUA-QUICK-REFERENCE.md`

**How to check status?**
```bash
bash .github/check-pua-status.sh
```

**Need to adjust?**
Edit `.github/copilot-instructions.md` and commit. PUA will read the updated version.

---

## 🏆 Expected Results

Starting from 2026-04-02:
- **Debugging speed**: +14-50% (via methodology switching)
- **Issue discovery**: +50% (via iceberg rule)
- **Verification rate**: +65% (via Quality Compass)
- **Code quality**: From 3.25 to 3.75+ level

---

## 🚀 Status

```
✅ All PUA framework files in place
✅ Auto-activation configured for multiple tools
✅ Three red lines enforced
✅ Pressure escalation (L0-L4) ready
✅ 13 methodologies available
✅ Quality Compass integrated
✅ Iceberg rule / pattern scanning active

🔴 STATUS: LIVE AND ACTIVE
```

---

*Last Updated: 2026-04-02*  
*Framework Source: PUA v3 (tanweai/pua) adapted*  
*Project: go-on (Rust + TypeScript)*  
**All AI assistants start with PUA enabled by default.**
