#!/usr/bin/env python3
"""Cross-client i18n consistency check.

Checks that each client's translation files are internally consistent
(same key set across languages):

  1. Backend  — config/languages/{en-US,zh-CN,zh-TW}.json (flat dot-keys)
  2. VSCode   — vscode-addon/src/locales/{en-US,zh-CN,zh-TW}.json (nested)

The GUI keeps its translations in Rust (`gui/src/i18n/*.rs`); its
cross-language key consistency is enforced by the unit test
`test_i18n_all_keys_have_all_languages` in `gui/src/tests.rs`.

Exits non-zero when any client has a missing/extra key so CI can gate on it.
"""

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent


def flatten(d, prefix=""):
    """Flatten a nested dict into a set of dot-path keys."""
    keys = set()
    for k, v in d.items():
        path = f"{prefix}.{k}" if prefix else k
        if isinstance(v, dict):
            keys |= flatten(v, path)
        else:
            keys.add(path)
    return keys


def check_group(name, files):
    """Verify all files in `files` expose the same key set."""
    print(f"=== {name} ===")
    loaded = {}
    for f in files:
        with open(f, encoding="utf-8") as fh:
            data = json.load(fh)
        loaded[f.name] = flatten(data)

    reference_name = next(iter(loaded))
    reference = loaded[reference_name]
    print(f"{reference_name}: {len(reference)} keys")

    ok = True
    for fname, keys in loaded.items():
        if fname == reference_name:
            continue
        missing = sorted(reference - keys)
        extra = sorted(keys - reference)
        print(f"{fname}: {len(keys)} keys")
        if missing:
            ok = False
            print(f"  MISSING ({len(missing)}): {', '.join(missing[:10])}")
        if extra:
            ok = False
            print(f"  EXTRA   ({len(extra)}): {', '.join(extra[:10])}")
        if not missing and not extra:
            print("  OK — key set identical")
    return ok


def main():
    backend = check_group(
        "Backend (config/languages)",
        [
            ROOT / "config" / "languages" / "en-US.json",
            ROOT / "config" / "languages" / "zh-CN.json",
            ROOT / "config" / "languages" / "zh-TW.json",
        ],
    )
    vscode = check_group(
        "VSCode (vscode-addon/src/locales)",
        [
            ROOT / "vscode-addon" / "src" / "locales" / "en-US.json",
            ROOT / "vscode-addon" / "src" / "locales" / "zh-CN.json",
            ROOT / "vscode-addon" / "src" / "locales" / "zh-TW.json",
        ],
    )
    print("GUI: enforced by gui/src/tests.rs::test_i18n_all_keys_have_all_languages")

    if not (backend and vscode):
        print("\nFAIL: i18n key sets are not consistent across languages")
        sys.exit(1)
    print("\nOK: all i18n files internally consistent")


if __name__ == "__main__":
    main()
