import os

for root, dirs, files in os.walk('src'):
    for f in files:
        if f.endswith('.rs'):
            path = os.path.join(root, f)
            with open(path) as fh:
                lines = fh.readlines()
            for i, line in enumerate(lines):
                st = line.strip()
                if st.startswith('mod tests') and '{' in st:
                    prev = lines[i-1].strip() if i>0 else ''
                    if not prev.startswith('#[cfg(test)]'):
                        print(f'{path}:{i+1}')
                        print(f'  prev line: {prev}')
