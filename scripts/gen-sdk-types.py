#!/usr/bin/env python3
"""Generate the TypeScript SDK's src/types.ts from the canonical ACP stream contract.

Reads `contracts/acp-stream-events.json` (single source of truth) and:
  1. Verifies the stream event names match the `STREAM_EVENT_*` constants in
     `src/acp/impl/chat/streaming.rs` (the src-side emission vocabulary), so
     the contract cannot drift from the Rust producers/classifiers.
  2. Verifies every type imported from "./types" by
     `sdk/typescript/src/index.ts` and `sdk/typescript/src/client.ts` is
     derivable from the contract (either from `types` or the `StreamEvent`
     union), so regenerating can never drop a public type.
  3. Writes `sdk/typescript/src/types.ts` with deterministic output: types and
     events are emitted in sorted name order and fields keep the contract's
     property order, so repeated runs are byte-identical.

Run from the repository root:
    python3 scripts/gen-sdk-types.py            # regenerate types.ts
    python3 scripts/gen-sdk-types.py --check    # fail if the committed types.ts drifted
"""

import argparse
import difflib
import json
import os
import re
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CONTRACT = ROOT / "contracts" / "acp-stream-events.json"
OUT_TS = ROOT / "sdk" / "typescript" / "src" / "types.ts"
INDEX_TS = ROOT / "sdk" / "typescript" / "src" / "index.ts"
CLIENT_TS = ROOT / "sdk" / "typescript" / "src" / "client.ts"
STREAMING_RS = ROOT / "src" / "acp" / "impl" / "chat" / "streaming.rs"

HEADER = [
    "// GENERATED FILE — do not edit. Run: python3 scripts/gen-sdk-types.py",
    "// Source of truth: contracts/acp-stream-events.json",
]

# Structural type tokens recognized in bare-string property schemas.
PRIMITIVES = {"string", "number", "boolean", "object", "unknown", "any", "null"}


def load_contract():
    with open(CONTRACT, encoding="utf-8") as f:
        return json.load(f)


def rust_stream_event_names(path):
    """Event names from the `STREAM_EVENT_*: &str = "..."` constants."""
    text = path.read_text(encoding="utf-8")
    return re.findall(r'STREAM_EVENT_[A-Z_]+\s*:\s*&str\s*=\s*"([^"]+)"', text)


def types_imported_from(path):
    """Type names pulled from `import/export type { ... } from "./types"`."""
    text = path.read_text(encoding="utf-8")
    names = []
    for block in re.findall(r'(?:import|export)\s+type\s*\{([^}]*)\}\s*from\s*"\./types"', text):
        names.extend(n.strip() for n in block.split(",") if n.strip())
    return names


def map_type(schema):
    """Map a contract property schema to a TypeScript type expression."""
    if isinstance(schema, str):
        return _map_bare_type(schema)
    if "enum" in schema:
        # Literal union, e.g. ["user", "assistant"] -> "user" | "assistant".
        return " | ".join(json.dumps(v) for v in schema["enum"])
    kind = schema.get("type")
    if kind == "array":
        return f"{map_type(schema['items'])}[]"
    if kind == "object":
        if "properties" in schema:
            return _render_inline_object(schema)
        return "Record<string, unknown>"
    if isinstance(kind, list):
        # JSON-Schema-style union of types, e.g. ["string", "null"] -> string | null.
        return " | ".join(_map_bare_type(item) for item in kind)
    if kind in PRIMITIVES:
        return kind
    raise ValueError(f"Unsupported property schema: {json.dumps(schema)}")


def _map_bare_type(token):
    if " | " in token:
        return " | ".join(_map_bare_type(part) for part in token.split(" | "))
    if token in PRIMITIVES:
        return "Record<string, unknown>" if token == "object" else token
    if token.endswith("[]"):
        return f"{_map_bare_type(token[:-2])}[]"
    # Otherwise a named reference (a declared type or a generic parameter like "T").
    return token


def _render_inline_object(schema):
    """Render an anonymous object literal type, e.g. { uri: string; text?: string }."""
    parts = []
    for name, prop in schema["properties"].items():
        optional = "" if name in schema.get("required", []) else "?"
        parts.append(f"{name}{optional}: {map_type(prop)}")
    return "{ " + "; ".join(parts) + " }"


def _render_variant(variant):
    """Render one member of a `union` declaration (or of the StreamEvent union)."""
    parts = []
    for name, prop in variant["properties"].items():
        optional = "" if name in variant.get("required", []) else "?"
        parts.append(f"{name}{optional}: {map_type(prop)}")
    return "{ " + "; ".join(parts) + " }"


def _jsdoc(description):
    """Render a description as one JSDoc line, or several for embedded newlines."""
    if "\n" in description:
        return ["/**"] + [f" * {line}" for line in description.split("\n")] + [" */"]
    return [f"/** {description} */"]


def _prop_lines(name, prop, required):
    lines = []
    if isinstance(prop, dict) and prop.get("description"):
        for line in _jsdoc(prop["description"]):
            lines.append(f"  {line}")
    optional = "" if name in required else "?"
    lines.append(f"  {name}{optional}: {map_type(prop)};")
    return lines


def render_type(name, schema):
    """Render one named type declaration (interface, generic interface, or union)."""
    lines = []
    if schema.get("description"):
        lines.extend(_jsdoc(schema["description"]))
    if "union" in schema:
        lines.append(f"export type {name} =")
        variants = schema["union"]
        for i, variant in enumerate(variants):
            suffix = ";" if i == len(variants) - 1 else ""
            lines.append(f"  | {_render_variant(variant)}{suffix}")
        return lines
    generic = schema.get("generic")
    header = f"export interface {name}"
    if generic:
        header += f"<{generic}>"
    lines.append(header + " {")
    required = schema.get("required", [])
    for prop_name, prop in schema["properties"].items():
        lines.extend(_prop_lines(prop_name, prop, required))
    lines.append("}")
    return lines


def render_stream_event(events):
    """Render the discriminated `StreamEvent` union from the events map."""
    lines = ["/** Discriminated union of every SSE stream event emitted by the backend. */"]
    lines.append("export type StreamEvent =")
    names = sorted(events)
    for i, name in enumerate(names):
        event = events[name]
        parts = [f'type: "{name}"']
        for prop_name, prop in event["properties"].items():
            optional = "" if prop_name in event.get("required", []) else "?"
            parts.append(f"{prop_name}{optional}: {map_type(prop)}")
        suffix = ";" if i == len(names) - 1 else ""
        lines.append(f"  | {{ {'; '.join(parts)} }}{suffix}")
    return lines


def render_ts(contract):
    lines = list(HEADER)
    lines.append("")
    for name in sorted(contract["types"]):
        lines.extend(render_type(name, contract["types"][name]))
        lines.append("")
    lines.extend(render_stream_event(contract["events"]))
    lines.append("")
    return "\n".join(lines)


def verify(contract):
    """Cross-checks that keep the contract meaningful (mirrors gen-state-sync-types.py)."""
    events = contract["events"]
    types = contract["types"]

    # 1. Stream event names must equal the Rust STREAM_EVENT_* vocabulary.
    expected = sorted(events)
    actual = sorted(rust_stream_event_names(STREAMING_RS))
    if actual != expected:
        print("ERROR: stream event names mismatch:")
        print(f"  contract    : {expected}")
        print(f"  streaming.rs: {actual}")
        sys.exit(1)
    print(f"OK: contract events match STREAM_EVENT_* constants ({len(actual)})")

    # 2. Every type the SDK imports from "./types" must be derivable, so
    #    regenerating can never break index.ts / client.ts consumers.
    emitted = set(types) | {"StreamEvent"}
    imported = sorted(set(types_imported_from(INDEX_TS) + types_imported_from(CLIENT_TS)))
    missing = [name for name in imported if name not in emitted]
    if missing:
        print("ERROR: SDK imports types missing from the contract:")
        for name in missing:
            print(f"  {name}")
        sys.exit(1)
    print(f"OK: all {len(imported)} SDK type imports are covered by the contract")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="generate to a temp file and fail if the committed types.ts is not byte-identical",
    )
    args = parser.parse_args()

    contract = load_contract()
    verify(contract)
    generated = render_ts(contract)

    if args.check:
        fd, tmp_path = tempfile.mkstemp(suffix=".ts", text=True)
        try:
            with os.fdopen(fd, "w", encoding="utf-8") as f:
                f.write(generated)
            committed = OUT_TS.read_bytes() if OUT_TS.exists() else b""
            if committed == Path(tmp_path).read_bytes():
                print(f"OK: {OUT_TS.relative_to(ROOT)} is up to date (no drift)")
                return
            print(f"ERROR: {OUT_TS.relative_to(ROOT)} is out of date.")
            print("Regenerate with: python3 scripts/gen-sdk-types.py")
            diff = difflib.unified_diff(
                committed.decode("utf-8").splitlines(True),
                generated.splitlines(True),
                fromfile=str(OUT_TS),
                tofile="generated (fresh)",
            )
            sys.stdout.writelines(diff)
            sys.exit(1)
        finally:
            Path(tmp_path).unlink(missing_ok=True)

    OUT_TS.write_text(generated, encoding="utf-8")
    print(f"Wrote {OUT_TS.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
