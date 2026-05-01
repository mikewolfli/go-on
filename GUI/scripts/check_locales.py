import json
import os
import sys

os.chdir(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def flatten_keys(obj, prefix=""):
    keys = set()
    for k, v in obj.items():
        key = prefix + "." + k if prefix else k
        if isinstance(v, dict):
            keys.update(flatten_keys(v, key))
        else:
            keys.add(key)
    return keys


with open("src/locales/en-US.json") as f:
    en = json.load(f)
with open("src/locales/zh-CN.json") as f:
    zh = json.load(f)
with open("src/locales/zh-TW.json") as f:
    tw = json.load(f)

en_keys = flatten_keys(en)
zh_keys = flatten_keys(zh)
tw_keys = flatten_keys(tw)

print("=== Missing in zh-CN (count: {}) ===".format(len(en_keys - zh_keys)))
for k in sorted(en_keys - zh_keys):
    print(k)

print()
print("=== Missing in zh-TW (count: {}) ===".format(len(en_keys - tw_keys)))
for k in sorted(en_keys - tw_keys):
    print(k)

print()
print("=== Extra in zh-CN ===")
for k in sorted(zh_keys - en_keys):
    print(k)

print()
print("=== Extra in zh-TW ===")
for k in sorted(tw_keys - en_keys):
    print(k)

print()
print(
    f"en-US keys: {len(en_keys)}, zh-CN keys: {len(zh_keys)}, zh-TW keys: {len(tw_keys)}"
)
