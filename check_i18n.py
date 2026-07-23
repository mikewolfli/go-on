import json

with open('config/languages/en-US.json') as f:
    en = json.load(f)
with open('config/languages/zh-CN.json') as f:
    cn = json.load(f)
with open('config/languages/zh-TW.json') as f:
    tw = json.load(f)

en_keys = set(en['messages'].keys())
cn_keys = set(cn['messages'].keys())
tw_keys = set(tw['messages'].keys())

print("en-US: {} keys".format(len(en_keys)))
print("zh-CN: {} keys".format(len(cn_keys)))
print("zh-TW: {} keys".format(len(tw_keys)))
print()

missing_cn = sorted(en_keys - cn_keys)
missing_tw = sorted(en_keys - tw_keys)
extra_cn = sorted(cn_keys - en_keys)
extra_tw = sorted(tw_keys - en_keys)

if missing_cn:
    print("=== Keys in en-US but NOT in zh-CN ({} missing) ===".format(len(missing_cn)))
    for k in missing_cn:
        print("  " + k)
else:
    print("=== zh-CN: no missing keys ===")
print()

if missing_tw:
    print("=== Keys in en-US but NOT in zh-TW ({} missing) ===".format(len(missing_tw)))
    for k in missing_tw:
        print("  " + k)
else:
    print("=== zh-TW: no missing keys ===")
print()

if extra_cn:
    print("=== Keys in zh-CN but NOT in en-US ===")
    for k in extra_cn:
        print("  " + k)

if extra_tw:
    print("=== Keys in zh-TW but NOT in en-US ===")
    for k in extra_tw:
        print("  " + k)
