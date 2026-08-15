#!/usr/bin/env python3
"""Analyze identifier usage in a Rust file, ignoring top-level `use` statements.

For each candidate identifier, reports whether it appears in the file outside
of any `use` declaration (single-line or multi-line). Used to compute correct
import sets when splitting modules (M0.4 phases split).

Usage: analyze_phase_imports.py FILE [IDENT ...]
If no identifiers given, reads them from stdin (one per line).
"""
import re
import sys


def strip_imports(src: str) -> str:
    """Remove top-level `use` statements (including multi-line ones)."""
    lines = src.splitlines()
    out = []
    i = 0
    depth = 0
    in_use = False
    for line in lines:
        stripped = line.strip()
        if not in_use:
            if stripped.startswith("use "):
                in_use = True
                # track brace depth within this statement
                depth = line.count("{") - line.count("}")
                if depth <= 0 and ";" in line:
                    in_use = False
                continue
            out.append(line)
        else:
            depth += line.count("{") - line.count("}")
            if depth <= 0 and ";" in line:
                in_use = False
    return "\n".join(out)


def main() -> int:
    path = sys.argv[1]
    idents = sys.argv[2:]
    if not idents:
        idents = [l.strip() for l in sys.stdin if l.strip()]
    with open(path, "r", encoding="utf-8") as f:
        src = f.read()
    body = strip_imports(src)
    for ident in idents:
        # word-boundary match, not matching part of a longer identifier
        count = len(re.findall(r"(?<![A-Za-z0-9_])" + re.escape(ident) + r"(?![A-Za-z0-9_])", body))
        print(f"{ident}: {count}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
