from pathlib import Path
files=list(Path('src').rglob('*.rs'))
missing=[]
for f in files:
    text=f.read_text(encoding='utf-8').lstrip()
    if not (text.startswith('//!') or text.startswith('///')):
        missing.append(str(f))
print('missing_count', len(missing))
for m in missing:
    print(m)
