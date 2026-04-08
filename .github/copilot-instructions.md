
# Copilot Bootstrap Instructions

This file is a compatibility bootstrap for editors/tools that auto-read
.github/copilot-instructions.md.

Authoritative rule sources are in RULES for editor-agnostic reuse:
1. RULES/global.md
2. RULES/common.md
3. RULES/coding.md
4. RULES/review.md
5. RULES/pua.md
6. RULES/README.md

Required behavior summary:
- enforce PUA red lines and pressure escalation
- require build/test proof before completion
- require pattern scanning and root-cause explanation
- forbid placeholders, incomplete implementations, and structural breakage

If this file and RULES files diverge, treat RULES as the source of truth.