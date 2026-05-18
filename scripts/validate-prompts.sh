#!/usr/bin/env bash
set -euo pipefail

# Validate prompt template files in prompts/.
# Usage:
#   scripts/validate-prompts.sh [--strict-i18n]

STRICT_I18N=0
if [[ "${1:-}" == "--strict-i18n" ]]; then
  STRICT_I18N=1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
PROMPTS_DIR="$ROOT_DIR/prompts"

if [[ ! -d "$PROMPTS_DIR" ]]; then
  echo "❌ prompts directory not found: $PROMPTS_DIR" >&2
  exit 1
fi

python3 - "$PROMPTS_DIR" "$STRICT_I18N" <<'PY'
import json
import os
import sys
from pathlib import Path

prompts_dir = Path(sys.argv[1])
strict_i18n = sys.argv[2] == "1"

main_files = ["en.json", "zh-CN.json", "zh-TW.json"]
errors = []
warnings = []
infos = []

def fail(msg: str):
    errors.append(msg)

def warn(msg: str):
    warnings.append(msg)

def info(msg: str):
    infos.append(msg)

def parse_main(path: Path):
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except Exception as e:
        fail(f"{path}: invalid JSON ({e})")
        return []

    if isinstance(data, dict):
        categories = data.get("categories")
        if not isinstance(categories, list):
            fail(f"{path}: object format must contain array field 'categories'")
            return []
        return categories

    if isinstance(data, list):
        return data

    fail(f"{path}: root must be object or array")
    return []

def validate_template(path: Path, cat_id: str, t, idx: int):
    if not isinstance(t, dict):
        fail(f"{path}: template[{cat_id}#{idx}] must be an object")
        return None

    required = ["id", "title", "description", "content", "tags"]
    for key in required:
        if key not in t:
            fail(f"{path}: template[{cat_id}#{idx}] missing '{key}'")
            return None

    for key in ["id", "title", "description", "content"]:
        if not isinstance(t[key], str) or not t[key].strip():
            fail(f"{path}: template[{cat_id}#{idx}] field '{key}' must be non-empty string")
            return None

    if not isinstance(t["tags"], list) or any(not isinstance(x, str) for x in t["tags"]):
        fail(f"{path}: template[{cat_id}#{idx}] field 'tags' must be string array")
        return None

    return t

def validate_categories(path: Path, categories):
    cat_ids = set()
    scoped_ids = set()
    short_ids = {}

    normalized = []

    for i, c in enumerate(categories):
        if not isinstance(c, dict):
            fail(f"{path}: category[{i}] must be an object")
            continue

        for key in ["id", "name", "icon", "templates"]:
            if key not in c:
                fail(f"{path}: category[{i}] missing '{key}'")
                continue

        cid = c.get("id")
        name = c.get("name")
        icon = c.get("icon")
        templates = c.get("templates")

        if not isinstance(cid, str) or not cid.strip():
            fail(f"{path}: category[{i}] id must be non-empty string")
            continue
        if not isinstance(name, str) or not name.strip():
            fail(f"{path}: category[{i}] name must be non-empty string")
        if not isinstance(icon, str) or not icon.strip():
            fail(f"{path}: category[{i}] icon must be non-empty string")
        if not isinstance(templates, list):
            fail(f"{path}: category[{i}] templates must be array")
            templates = []

        if cid in cat_ids:
            fail(f"{path}: duplicate category id '{cid}'")
        cat_ids.add(cid)

        valid_templates = []
        local_tpl_ids = set()
        for j, t in enumerate(templates):
            t_obj = validate_template(path, cid, t, j)
            if t_obj is None:
                continue
            tid = t_obj["id"]
            if tid in local_tpl_ids:
                fail(f"{path}: duplicate template id '{tid}' in category '{cid}'")
                continue
            local_tpl_ids.add(tid)

            scoped = f"{cid}.{tid}"
            if scoped in scoped_ids:
                fail(f"{path}: duplicate scoped template id '{scoped}'")
            scoped_ids.add(scoped)

            short_ids[tid] = short_ids.get(tid, 0) + 1
            valid_templates.append(t_obj)

        normalized.append({
            "id": cid,
            "name": name,
            "icon": icon,
            "templates": valid_templates,
        })

    collisions = sorted([k for k, n in short_ids.items() if n > 1])
    if collisions:
        info(f"{path}: short template id collisions ({len(collisions)}): {', '.join(collisions[:20])} (handled by scoped commands)")

    return normalized, scoped_ids

def validate_custom(path: Path):
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except Exception as e:
        fail(f"{path}: invalid JSON ({e})")
        return

    if not isinstance(data, list):
        fail(f"{path}: custom file root must be array")
        return

    ids = set()
    for i, t in enumerate(data):
        if not isinstance(t, dict):
            fail(f"{path}: custom template[{i}] must be object")
            continue
        for key in ["id", "title", "description", "content", "tags"]:
            if key not in t:
                fail(f"{path}: custom template[{i}] missing '{key}'")
                continue
        tid = t.get("id")
        if not isinstance(tid, str) or not tid.strip():
            fail(f"{path}: custom template[{i}] id must be non-empty string")
            continue
        if tid in ids:
            fail(f"{path}: duplicate custom template id '{tid}'")
        ids.add(tid)

all_sets = {}
for name in main_files:
    path = prompts_dir / name
    if not path.exists():
        fail(f"missing required prompts file: {path}")
        continue
    categories = parse_main(path)
    _, scoped = validate_categories(path, categories)
    all_sets[name] = scoped

custom_dir = prompts_dir / "custom"
if custom_dir.exists() and custom_dir.is_dir():
    for custom in sorted(custom_dir.glob("*.json")):
        validate_custom(custom)

if "en.json" in all_sets:
    en_set = all_sets["en.json"]
    for name in ["zh-CN.json", "zh-TW.json"]:
        if name not in all_sets:
            continue
        missing = sorted(list(en_set - all_sets[name]))
        extra = sorted(list(all_sets[name] - en_set))
        if missing:
            msg = f"{name}: missing {len(missing)} entries from en baseline"
            if strict_i18n:
                fail(msg)
            else:
                warn(msg)
        if extra:
            info(f"{name}: has {len(extra)} locale-specific entries not in en baseline")

print("=== Prompt Validation Summary ===")
if infos:
    print(f"Info: {len(infos)}")
    for i in infos:
        print(f"  - {i}")
else:
    print("Info: 0")

if warnings:
    print(f"Warnings: {len(warnings)}")
    for w in warnings:
        print(f"  - {w}")
else:
    print("Warnings: 0")

if errors:
    print(f"Errors: {len(errors)}")
    for e in errors:
        print(f"  - {e}")
    sys.exit(1)

print("Errors: 0")
print("✅ Prompt templates validation passed")
PY
