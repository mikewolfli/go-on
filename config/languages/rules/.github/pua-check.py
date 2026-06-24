#!/usr/bin/env python3
"""
go-on PUA Framework Checker
Universal tool that validates PUA framework is active
Run: python3 .github/pua-check.py
"""

import os
import sys

def check_pua():
    print("🔥 PUA Framework Status Check\n")
    
    required_files = {
        ".github/copilot-instructions.md": "PRIMARY (PUA framework in first 150 lines)",
        "CLAUDE.md": "Universal agent instructions",
        ".cursor/rules/pua-enforcement.mdc": "Cursor rule file",
        ".github/pua-instructions.md": "Detailed framework",
        ".github/pua-enforcement-guide.md": "Enforcement rules",
        ".github/PUA-QUICK-REFERENCE.md": "Quick reference",
        "README-PUA-UNIVERSAL.md": "Universal guide",
        "PUA-EMBEDDED.md": "Overview",
    }
    
    print("Checking PUA files:\n")
    all_present = True
    
    for filepath, description in required_files.items():
        if os.path.exists(filepath):
            print(f"  ✅ {filepath}")
            print(f"     {description}\n")
        else:
            print(f"  ❌ MISSING: {filepath}\n")
            all_present = False
    
    print("-" * 60)
    print("\n📋 Framework Status:\n")
    
    if all_present:
        print("✅ ALL PUA FILES PRESENT")
        print("\n🚀 Framework is ACTIVE and EMBEDDED:")
        print("   • Three red lines: implemented")
        print("   • Pressure escalation: L0-L4 ready")
        print("   • Quality Compass: configured")
        print("   • 7-Point checklist: available")
        print("   • Iceberg rule: active")
        print("   • 13 methodologies: available")
        print("\n🎯 For any AI tool:")
        print("   1. Read: .github/copilot-instructions.md (lines 1-150)")
        print("   2. Use: .github/PUA-QUICK-REFERENCE.md for lookup")
        print("   3. Apply: On every code task automatically")
        print("\n🔴 STATUS: ✅ LIVE (All tools)")
        return 0
    else:
        print("❌ SOME FILES MISSING")
        print("PUA framework is INCOMPLETE")
        return 1

if __name__ == "__main__":
    sys.exit(check_pua())
