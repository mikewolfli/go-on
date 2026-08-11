#!/usr/bin/env python3
"""Validate key-set consistency of the three i18n catalogs (backend / GUI / vscode).

Each end has its own three-language catalog and the keys are maintained by hand,
so this script is the drift guard: it fails when a language falls out of sync
within one end, or when the vscode `MessageKeys` enum no longer covers the
locale keys it is supposed to mirror.

Checks:
  1. Backend : config/languages/{en-US,zh-CN,zh-TW}.json  — same key set.
  2. GUI     : gui/src/i18n/{en,zh_cn,zh_tw}.rs           — same key set.
  3. VSCode  : vscode-addon/src/locales/{en-US,zh-CN,zh-TW}.json — same key set.
  4. VSCode  : `MessageKeys` enum entries ⊆ locale keys (with a report of
     keys missing from the enum and locale keys never referenced in code).

Run from the repository root:
    python3 scripts/validate-i18n.py            # fail on drift
    python3 scripts/validate-i18n.py --report   # also print dead-key statistics
"""

import argparse
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def load_json_keys(path: Path) -> set[str]:
    """Recursively collect all leaf keys (dotted) from a nested catalog JSON."""
    with open(path, encoding="utf-8") as f:
        data = json.load(f)

    def walk(node, prefix):
        if not isinstance(node, dict):
            return
        for key, value in node.items():
            full = f"{prefix}.{key}" if prefix else key
            if isinstance(value, dict) and value:
                walk(value, full)
            else:
                leaves.add(full)

    leaves: set[str] = set()
    walk(data, "")
    return leaves


def load_gui_rust_keys(path: Path) -> set[str]:
    text = path.read_text(encoding="utf-8")
    return set(re.findall(r'\(\s*"([^"]+)"\s*,\s*"', text))


def load_vscode_message_keys(path: Path) -> tuple[set[str], set[str]]:
    """Return (member names, referenced locale keys) from the MessageKeys object."""
    text = path.read_text(encoding="utf-8")
    m = re.search(r"export const MessageKeys\s*=\s*\{(.*?)\n\} as const;", text, re.S)
    if not m:
        raise SystemExit(f"MessageKeys object not found in {path}")
    pairs = re.findall(r"\n[ \t]*([A-Za-z0-9_]+):\s*\"([^\"]+)\"", m.group(1))
    names = {name for name, _ in pairs}
    keys = {key for _, key in pairs}
    return names, keys


def check_catalog(name: str, groups: list[tuple[str, set[str]]]) -> list[str]:
    """Assert all groups have identical key sets; return drift messages."""
    ref_name, ref = groups[0]
    errors = []
    for other_name, other in groups[1:]:
        missing = sorted(ref - other)
        extra = sorted(other - ref)
        if missing or extra:
            errors.append(
                f"[{name}] {other_name} diverges from {ref_name}: "
                f"missing {len(missing)} keys {missing[:5]}, "
                f"extra {len(extra)} keys {extra[:5]}"
            )
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--report", action="store_true", help="print dead-key statistics"
    )
    args = parser.parse_args()

    errors: list[str] = []

    # 1. Backend JSON catalogs.
    backend_langs = ["en-US", "zh-CN", "zh-TW"]
    backend = [
        (lang, load_json_keys(ROOT / "config" / "languages" / f"{lang}.json"))
        for lang in backend_langs
    ]
    errors += check_catalog("backend", backend)

    # 2. GUI Rust const tables.
    gui_langs = ["en", "zh_cn", "zh_tw"]
    gui = [
        (lang, load_gui_rust_keys(ROOT / "gui" / "src" / "i18n" / f"{lang}.rs"))
        for lang in gui_langs
    ]
    errors += check_catalog("gui", gui)

    # 3. VSCode JSON catalogs.
    vscode_langs = ["en-US", "zh-CN", "zh-TW"]
    vscode = [
        (lang, load_json_keys(ROOT / "vscode-addon" / "src" / "locales" / f"{lang}.json"))
        for lang in vscode_langs
    ]
    errors += check_catalog("vscode", vscode)

    # 4. VSCode MessageKeys object ⊆ locale keys.
    enum_path = ROOT / "vscode-addon" / "src" / "i18n.ts"
    if enum_path.exists():
        _, enum_keys = load_vscode_message_keys(enum_path)
        locale_keys = vscode[0][1]
        not_in_locale = sorted(enum_keys - locale_keys)
        if not_in_locale:
            errors.append(
                f"[vscode] MessageKeys entries not present in locales: "
                f"{not_in_locale[:10]}"
            )
        if args.report:
            unmirrored = sorted(locale_keys - enum_keys)
            print(
                f"[report] vscode: {len(enum_keys)} MessageKeys entries, "
                f"{len(locale_keys)} locale keys, "
                f"{len(unmirrored)} locale keys not mirrored in MessageKeys"
            )

    if args.report:
        print(
            f"[report] backend {len(backend[0][1])} keys | "
            f"gui {len(gui[0][1])} keys | "
            f"vscode {len(vscode[0][1])} keys"
        )
        # GUI dead keys: keys defined but never referenced in gui/src.
        gui_src_text = "\n".join(
            p.read_text(encoding="utf-8", errors="replace")
            for p in (ROOT / "gui" / "src").rglob("*.rs")
        )
        gui_dead = sorted(
            k for k in gui[0][1] if f'"{k}"' not in gui_src_text.replace('i18n/en.rs', '', 1)
        )
        # crude: keys referenced anywhere in the i18n module itself are alive
        gui_dead = [k for k in gui_dead if k not in gui[1][1] and k not in gui[2][1]]
        print(f"[report] gui keys never referenced outside the tables: {len(gui_dead)}")

    if errors:
        print("i18n drift detected:")
        for e in errors:
            print(f"  - {e}")
        return 1
    print("i18n: all three catalogs consistent (backend / gui / vscode).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
