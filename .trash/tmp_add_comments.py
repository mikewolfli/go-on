from pathlib import Path
files=['src/vector.rs','src/setup.rs','src/main.rs','src/flow.rs','src/error.rs','src/config.rs','src/cache.rs','src/agent.rs','src/acp.rs']
files += [str(p).replace('\\','/') for p in Path('src/agents').glob('*.rs')]
for p in files:
    fp=Path(p); text=fp.read_text(encoding='utf-8'); s=text.lstrip();
    if s.startswith('//!') or s.startswith('///'): continue
    module=fp.name; header=f'//! {module}\n//! Auto-generated English doc: module overview.\n//!\n'
    fp.write_text(header+text, encoding='utf-8'); print('updated', p)
print('done')
