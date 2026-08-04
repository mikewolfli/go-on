#!/usr/bin/env python3
"""Generate the GUI + VS Code provider catalogs from the backend authority.

The backend's `built_in_provider_specs()` in `src/core/providers.rs` is the
single source of truth for provider names / defaults. The GUI previously kept
two hand-maintained copies (`PROVIDER_NAMES` + `built_in_provider_specs()`),
and the VS Code addon kept a third hand-maintained copy
(`vscode-addon/src/settings/providerCatalog.ts`), all of which drifted from
the backend. This script generates:

- `gui/src/views/providers/generated_catalog.rs`           — GUI offline fallback
- `vscode-addon/src/settings/providerCatalog.generated.ts` — VS Code catalog

so neither offline fallback can diverge from the backend (same pattern as
`gen-state-sync-types.py`).

VS Code-specific mappings (documented, UI-only):
- `api_key_env` / `secret_key_env` keyring URIs (`keyring://go-on/<name>_api_key`)
  are mapped to conventional env var names (`<NAME>_API_KEY`).
- `group` ("openai" | "chinese" | "other") is a VS Code settings-UI grouping,
  derived from the backend `region` ("China" → "chinese") with an explicit
  override set for the OpenAI-compatible core group.

Usage:
    python3 scripts/gen-provider-catalog.py            # regenerate
    python3 scripts/gen-provider-catalog.py --check    # verify up-to-date (CI)
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BACKEND = ROOT / "src" / "core" / "providers.rs"
GUI_OUTPUT = ROOT / "gui" / "src" / "views" / "providers" / "generated_catalog.rs"
VSCODE_OUTPUT = (
    ROOT / "vscode-addon" / "src" / "settings" / "providerCatalog.generated.ts"
)

BLOCK_RE = re.compile(r"ProviderSpec \{(?P<body>.*?)\n\s*\},", re.DOTALL)
FIELD_PLAIN = re.compile(r'^\s*(\w+): "((?:[^"\\]|\\.)*)"\.to_string\(\),?\s*$')
FIELD_STR = re.compile(r'^\s*(\w+): Some\("((?:[^"\\]|\\.)*)"\.to_string\(\)\),?\s*$')
FIELD_NONE = re.compile(r"^\s*(\w+): None,?\s*$")
FIELD_BOOL = re.compile(r"^\s*(\w+): Some\((true|false)\),?\s*$")
FIELD_INT = re.compile(r"^\s*(\w+): Some\((\d+)\),?\s*$")

# VS Code settings-UI grouping. The OpenAI-compatible core group is an explicit
# override; everything else follows the backend `region` field.
OPENAI_GROUP = {"openai", "openai_compatible", "anthropic", "cohere"}
# Chinese vendor despite a Global `region` flag — preserved from the original
# hand-maintained grouping so the settings UI keeps its intent.
CHINESE_GROUP_OVERRIDES = {"deepseek"}


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
            m = FIELD_INT.match(line)
            if m:
                spec[m.group(1)] = int(m.group(2))
                continue
            m = FIELD_NONE.match(line)
            if m:
                spec[m.group(1)] = None
        if "name" not in spec:
            continue
        specs.append(spec)
    return specs


def rust_str(value: str) -> str:
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def ts_str(value: str) -> str:
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def env_var_from_keyring(uri: str) -> str:
    """Map a backend keyring URI to a conventional env var name.

    `keyring://go-on/openai_api_key` -> `OPENAI_API_KEY`.
    """
    return uri.rsplit("/", 1)[-1].upper()


def group_for(spec: dict) -> str:
    if spec.get("name") in OPENAI_GROUP:
        return "openai"
    if spec.get("name") in CHINESE_GROUP_OVERRIDES or spec.get("region") == "China":
        return "chinese"
    return "other"


def render_gui(specs: list[dict]) -> str:
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


def render_vscode(specs: list[dict]) -> str:
    lines = [
        "// Generated provider catalog — DO NOT EDIT.",
        "//",
        "// Regenerate with: python3 scripts/gen-provider-catalog.py",
        "//",
        "// Mirrors the backend's `built_in_provider_specs()` in",
        "// `src/core/providers.rs` (the single source of truth) so the",
        "// VS Code addon catalog can never diverge from the backend.",
        "",
        'import type { ProviderCatalogSpec } from "./providerCatalog";',
        "",
        "export const BUILTIN_PROVIDER_CATALOG: ProviderCatalogSpec[] = [",
    ]
    for s in specs:
        parts = [
            "  {",
            f"    name: {ts_str(s['name'])}, type: {ts_str(s['agent_type'])}, group: {ts_str(group_for(s))},",
        ]
        if s.get("url"):
            parts.append(f"    url: {ts_str(s['url'])},")
        if s.get("chat_path"):
            parts.append(f"    chat_path: {ts_str(s['chat_path'])},")
        if s.get("model"):
            parts.append(f"    model: {ts_str(s['model'])},")
        if s.get("api_key_env"):
            parts.append(f"    api_key_env: {ts_str(env_var_from_keyring(s['api_key_env']))},")
        if s.get("secret_key_env"):
            parts.append(f"    secret_key_env: {ts_str(env_var_from_keyring(s['secret_key_env']))},")
        if s.get("anthropic_version"):
            parts.append(f"    anthropic_version: {ts_str(s['anthropic_version'])},")
        if s.get("max_tokens") is not None:
            parts.append(f"    max_tokens: {int(s['max_tokens'])},")
        if s.get("supports_system") is not None:
            parts.append(f"    supports_system: {str(s['supports_system']).lower()},")
        parts.append("  },")
        lines.append("\n".join(parts))
    lines.append("];")
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
    gui_generated = render_gui(specs)
    vscode_generated = render_vscode(specs)

    if "--check" in sys.argv:
        ok = True
        if GUI_OUTPUT.exists() and GUI_OUTPUT.read_text(encoding="utf-8") == gui_generated:
            print(f"OK: {GUI_OUTPUT} is up to date ({len(specs)} providers)")
        else:
            print(
                f"STALE: {GUI_OUTPUT} differs from backend authority "
                f"({len(specs)} providers). Run scripts/gen-provider-catalog.py"
            )
            ok = False
        if VSCODE_OUTPUT.exists() and VSCODE_OUTPUT.read_text(encoding="utf-8") == vscode_generated:
            print(f"OK: {VSCODE_OUTPUT} is up to date ({len(specs)} providers)")
        else:
            print(
                f"STALE: {VSCODE_OUTPUT} differs from backend authority "
                f"({len(specs)} providers). Run scripts/gen-provider-catalog.py"
            )
            ok = False
        return 0 if ok else 1

    GUI_OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    GUI_OUTPUT.write_text(gui_generated, encoding="utf-8")
    VSCODE_OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    VSCODE_OUTPUT.write_text(vscode_generated, encoding="utf-8")
    print(f"Wrote {GUI_OUTPUT} ({len(specs)} providers)")
    print(f"Wrote {VSCODE_OUTPUT} ({len(specs)} providers)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
