#!/usr/bin/env python3
"""Generate cross-client StateSyncEvent types from the single source of truth.

Reads `contracts/state-sync-events.json` and:
  1. Writes `vscode-addon/src/generated/stateSyncTypes.ts` (TypeScript union).
  2. Verifies the backend (`src/protocol/state_sync.rs`) and GUI
     (`gui/src/state_sync.rs`) Rust enums contain exactly the same variants.

Run from the repository root:
    python3 scripts/gen-state-sync-types.py
"""

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CONTRACT = ROOT / "contracts" / "state-sync-events.json"
OUT_TS = ROOT / "vscode-addon" / "src" / "generated" / "stateSyncTypes.ts"
BACKEND_RS = ROOT / "src" / "protocol" / "state_sync.rs"
GUI_RS = ROOT / "gui" / "src" / "state_sync.rs"


def load_contract():
    with open(CONTRACT, encoding="utf-8") as f:
        return json.load(f)


def rust_variant_names(path):
    text = path.read_text(encoding="utf-8")
    # Find the enum body: `pub enum StateSyncEvent { ... }` (variants contain
    # nested braces, so match until a top-level closing brace on its own line).
    m = re.search(r"pub enum StateSyncEvent\s*\{(.*?)\n\}", text, re.S)
    if not m:
        raise SystemExit(f"StateSyncEvent enum not found in {path}")
    body = m.group(1)
    # Variant names: identifiers immediately followed by `{` at line starts
    # (skipping doc comments that precede each variant).
    return re.findall(r"\n[ \t]*([A-Za-z_][A-Za-z0-9_]*)\s*\{", body)


def render_ts(events):
    lines = [
        "// AUTO-GENERATED from contracts/state-sync-events.json — do not edit.",
        "// Regenerate with: python3 scripts/gen-state-sync-types.py",
        "",
        "/**",
        " * Mirror of the backend's `StateSyncEvent` (single source of truth:",
        " * contracts/state-sync-events.json).",
        " */",
        "export type StateSyncEvent =",
    ]
    for ev in events:
        fields = "; ".join(f"{f['name']}: {f['ts_type']}" for f in ev["fields"])
        lines.append(f'  | {{ type: "{ev["type"]}"; {fields} }}')
    return "\n".join(lines) + ";\n"


def main():
    contract = load_contract()
    events = contract["events"]

    # 1. Verify backend + GUI Rust enums match the contract.
    expected = sorted(ev["rust_variant"] for ev in events)
    for label, path in (("backend", BACKEND_RS), ("gui", GUI_RS)):
        actual = sorted(rust_variant_names(path))
        if actual != expected:
            print(f"ERROR: {label} StateSyncEvent variants mismatch:")
            print(f"  contract : {expected}")
            print(f"  {path.name}: {actual}")
            sys.exit(1)
        print(f"OK: {label} StateSyncEvent variants match contract ({len(actual)})")

    # 2. Emit the TypeScript union.
    OUT_TS.parent.mkdir(parents=True, exist_ok=True)
    OUT_TS.write_text(render_ts(events), encoding="utf-8")
    print(f"Wrote {OUT_TS.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
