# Code Index — Semantic Code Search

## Overview

The `code_index_search` tool provides workspace-wide code symbol indexing and ranked keyword search. Unlike `grep` which does plain-text search, the code index understands code structure: it extracts function names, structs, enums, traits, classes, interfaces, and other symbols, then returns ranked results based on match quality.

## Supported Languages

| Language | Extensions | Extracted Symbols |
|----------|-----------|-------------------|
| Rust | `.rs` | fn, struct, enum, trait, impl, mod, const, type, macro_rules! |
| Python | `.py` | def, class, async def |
| TypeScript/JS | `.ts/.tsx/.js/.jsx` | function, class, interface, enum, type, const/let/var |
| Go | `.go` | func, type struct, type interface |
| Java/Kotlin | `.java/.kt` | class, interface, enum, method |
| C/C++ | `.c/.h/.cpp/.hpp` | function, class, struct |
| Ruby | `.rb` | def, class |
| Rust/RSX | `.rsx` | fn, struct |

## Usage

### Build Index
```json
{
  "operation": "build",
  "directory": "/path/to/workspace"
}
```

### Search
```json
{
  "operation": "search",
  "query": "ToolRegistry",
  "limit": 20
}
```

### Refresh (rebuild)
```json
{
  "operation": "refresh",
  "directory": "/path/to/workspace"
}
```

### Stats
```json
{
  "operation": "stats"
}
```

## VS Code Integration

The `go-on.semanticSearch` command in the VS Code extension provides a GUI interface:
1. Opens an input box for the search query
2. Automatically builds the workspace index
3. Runs the search and opens results in a JSON viewer

## Scored Results

Results include a relevance score:
- **1000**: Exact symbol name match
- **500**: Prefix match (word boundary)
- **200**: Prefix match (substring)
- **100**: Substring match
- **50**: Multi-term partial match

## Performance

- Skips `target/`, `node_modules/`, `.git/` and other build artifact directories
- Max 10,000 files per workspace (configurable in source)
- Index is memory-only (JSON persistence planned for a future release)
