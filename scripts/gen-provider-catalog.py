#!/usr/bin/env python3
"""Generate the GUI's static provider catalog from the backend authority.

The backend's `built_in_provider_specs()` in `src/core/providers.rs` is the
single source of truth for provider names / defaults. The GUI previously kept
two hand-maintained copies (`PROVIDER_NAMES` + `built_in_provider_specs()`),
which drifted from the backend. This script generates
`gui/src/views/providers/generated_catalog.rs` so the offline fallback and the
backend can never diverge (same pattern as `gen-state-sync-types.py`).

Usage:
    python3 scripts/gen-provider-catalog.py            # regenerate
    python3 scripts/gen-provider-catalog.py --check    # verify up-to-date (CI)
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BACKEND = ROOT / "src" / "core" / "providers.rs"
OUTPUT = ROOT / "gui" / "src" / "views" / "providers" / "generated_catalog.rs"

BLOCK_RE = re.compile(r"ProviderSpec \{(?P<body>.*?)\n\s*\},", re.DOTALL)
FIELD_PLAIN = re.compile(r'^\s*(\w+): "((?:[^"\\]|\\.)*)"\.to_string\(\),?\s*$')
FIELD_STR = re.compile(r'^\s*(\w+): Some\("((?:[^"\\]|\\.)*)"\.to_string\(\)\),?\s*$')
FIELD_NONE = re.compile(r"^\s*(\w+): None,?\s*$")
FIELD_BOOL = re.compile(r"^\s*(\w+): Some\((true|false)\),?\s*$")


def parse_specs(src: str) -> list[dict]:
    specs = []
    for block in BLOCK_RE.finditer(src):
        body = block.group("body")
        spec: dict = {}
        for line in body.splitlines():
            m = FIELD_PLAIN.match(line)
            if m:
                spec[m.group(1)] = m.group(2)
                continue
            m = FIELD_STR.match(line)
            if m:
                spec[m.group(1)] = m.group(2)
                continue
            m = FIELD_BOOL.match(line)
            if m:
                spec[m.group(1)] = m.group(2) == "true"
                continue
            m = FIELD_NONE.match(line)
            if m:
                spec[m.group(1)] = None
        if "name" not in spec:
            continue
        specs.append(spec)
    return specs


def rust_str(value: str) -> str:
    return '"' + value.replace('\\', "\\\\").replace('"', '\\"') + '"'


def render(specs: list[dict]) -> str:
    lines = [
        "// Generated provider catalog — DO NOT EDIT.",
        "//",
        "// Regenerate with: python3 scripts/gen-provider-catalog.py",
        "//",
        "// Mirrors the backend's `built_in_provider_specs()` in",
        "// `src/core/providers.rs` (the single source of truth). This is the",
        "// GUI's offline fallback used before the backend is reachable; at",
        "// runtime the `provider.catalog` RPC is the authoritative source.",
        "",
        "use super::catalog::ProviderSpec;",
        "",
        "/// Provider names in backend order (single source of truth for the",
        "/// GUI's offline provider dropdown / keyring sync / config checks).",
        "pub const PROVIDER_NAMES: &[&str] = &[",
    ]
    for s in specs:
        lines.append(f"    {rust_str(s['name'])},")
    lines.append("];")
    lines.append("")
    lines.append(
        "/// Offline provider spec lookup (backend defaults). Unknown names fall"
    )
    lines.append("/// back to the generic openai_compatible shape.")
    lines.append("pub fn built_in_provider_specs(name: &str) -> ProviderSpec {")
    lines.append("    match name {")
    for s in specs:
        url = s.get("url")
        url_expr = f"Some({rust_str(url)})" if url else "None"
        model = s.get("model") or "auto"
        supports = "true" if s.get("supports_system") else "false"
        lines.append(f"        {rust_str(s['name'])} => ProviderSpec {{")
        lines.append(f"            agent_type: {rust_str(s['agent_type'])},")
        lines.append(f"            default_url: {url_expr},")
        lines.append(f"            default_model: {rust_str(model)},")
        lines.append(f"            supports_system: {supports},")
        lines.append("        },")
    lines.append(
        '        _ => ProviderSpec {\n'
        '            agent_type: "openai_compatible",\n'
        '            default_url: None,\n'
        '            default_model: "auto",\n'
        '            supports_system: false,\n'
        "        },"
    )
    lines.append("    }")
    lines.append("}")
    lines.append("")
    return "\n".join(lines)


def main() -> int:
    if not BACKEND.exists():
        print(f"error: backend providers.rs not found at {BACKEND}")
        return 1
    src = BACKEND.read_text(encoding="utf-8")
    # Only the built_in_provider_specs() fn body.
    fn_start = src.index("fn built_in_provider_specs()")
    fn_body = src[fn_start:]
    specs = parse_specs(fn_body)
    if not specs:
        print("error: no ProviderSpec blocks parsed from backend")
        return 1
    generated = render(specs)

    if "--check" in sys.argv:
        if OUTPUT.exists() and OUTPUT.read_text(encoding="utf-8") == generated:
            print(f"OK: {OUTPUT} is up to date ({len(specs)} providers)")
            return 0
        print(
            f"STALE: {OUTPUT} differs from backend authority "
            f"({len(specs)} providers). Run scripts/gen-provider-catalog.py"
        )
        return 1

    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    OUTPUT.write_text(generated, encoding="utf-8")
    print(f"Wrote {OUTPUT} ({len(specs)} providers)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
