//! Code Index & Semantic Search Tool
//!
//! Provides a per-process code index for the local workspace: extracts
//! symbols (functions, structs, enums, traits, impls, modules) using
//! lightweight regex-based parsing, builds an inverted index keyed by symbol
//! name and file path, and supports ranked keyword search across the index.
//!
//! Unlike `grep`, this tool:
//! - Understands code **structure** (symbol names, types, signatures)
//! - Returns ranked results by relevance (exact matches > prefix > fuzzy)
//! - Supports scope-limited search (by directory, by file type)
//!
//! # Lifecycle
//!
//! The index is **process-local** (a static `OnceLock<Mutex<CodeIndex>>`): it
//! is rebuilt from scratch on the first `code_index_search` call and again on
//! every `refresh` (a full rebuild — the mtime map is collected but not yet
//! used to skip unchanged files). It is never persisted to disk, so a new
//! process rebuilds it from the source tree. (The original docs promised
//! msgpack/zstd persistence and incremental mtime-based updates; those were
//! never implemented and the docs now match the code.)
//!
//! # Integration
//!
//! Registered in `ToolRegistry::new()` as `code_index_search` with fallback to `grep`.
//! The pipeline maps it to `"search"` action for sandbox compliance.

use crate::governance::pua::tool_execution_report;
use crate::orchestration::tool::{sanitize_path_for_write, Tool, ToolInput, ToolOutput};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum number of search results to return.
const MAX_RESULTS: usize = 50;

/// Maximum files to index per workspace (safety limit).
const MAX_INDEX_FILES: usize = 10_000;

/// Maximum lines to read per file for symbol extraction.
const MAX_SYMBOL_LINES: usize = 10000;

/// File extensions to index for code symbols.
const CODE_EXTENSIONS: &[&str] = &[
    "rs", "go", "py", "js", "ts", "tsx", "jsx", "rb", "java", "kt", "scala", "swift", "c", "h",
    "cpp", "hpp", "cxx", "hxx", "cc", "hh", "cs", "fs", "fsx", "php", "pl", "pm", "lua", "r", "m",
    "mm", "zig", "nim", "ex", "exs", "toml", "yaml", "yml", "json", "md", "rsx",
];

/// Directories to always skip when indexing.
const SKIP_DIRS: &[&str] = &[
    "target",
    "node_modules",
    ".git",
    ".svn",
    "__pycache__",
    "dist",
    "build",
    ".build",
    ".zig-cache",
    "bin",
    "obj",
    "vendor",
    ".bundle",
    ".cargo",
    ".goon",
    ".vscode",
    ".zed",
    "out",
];

// ---------------------------------------------------------------------------
// Symbol Kind
// ---------------------------------------------------------------------------

/// Kinds of code symbols we extract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SymbolKind {
    Function,
    Method,
    Struct,
    Enum,
    Trait,
    Impl,
    Module,
    Constant,
    TypeAlias,
    Macro,
    Interface,
    Class,
    Property,
    Variable,
    Other(String),
}

impl std::fmt::Display for SymbolKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SymbolKind::Function => write!(f, "function"),
            SymbolKind::Method => write!(f, "method"),
            SymbolKind::Struct => write!(f, "struct"),
            SymbolKind::Enum => write!(f, "enum"),
            SymbolKind::Trait => write!(f, "trait"),
            SymbolKind::Impl => write!(f, "impl"),
            SymbolKind::Module => write!(f, "module"),
            SymbolKind::Constant => write!(f, "constant"),
            SymbolKind::TypeAlias => write!(f, "type_alias"),
            SymbolKind::Macro => write!(f, "macro"),
            SymbolKind::Interface => write!(f, "interface"),
            SymbolKind::Class => write!(f, "class"),
            SymbolKind::Property => write!(f, "property"),
            SymbolKind::Variable => write!(f, "variable"),
            SymbolKind::Other(s) => write!(f, "{}", s),
        }
    }
}

// ---------------------------------------------------------------------------
// CodeSymbol
// ---------------------------------------------------------------------------

/// A single symbol extracted from the codebase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeSymbol {
    /// Symbol name (e.g. "run", "ServerBuilder").
    pub name: String,
    /// Kind of symbol.
    pub kind: SymbolKind,
    /// File path (absolute).
    pub file_path: String,
    /// Line number where the symbol is defined.
    pub line: usize,
    /// The full line of source code for context.
    pub line_text: String,
    /// File extension (e.g. "rs", "go").
    pub extension: String,
    /// Language inferred from extension.
    pub language: String,
}

// ---------------------------------------------------------------------------
// CodeIndex
// ---------------------------------------------------------------------------

/// The in-memory code index, persisted to disk.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CodeIndex {
    /// Symbols by name (lowercased).
    symbols_by_name: HashMap<String, Vec<CodeSymbol>>,
    /// Symbols by file path.
    symbols_by_file: HashMap<String, Vec<CodeSymbol>>,
    /// Number of files indexed.
    pub files_indexed: usize,
    /// Total symbols indexed.
    pub total_symbols: usize,
    /// Timestamp of last index build (epoch ms).
    pub last_built_ms: u64,
    /// Tracked file mtimes for incremental rebuild.
    file_mtimes: HashMap<String, u64>,
}

impl CodeIndex {
    /// Create a new empty index.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build or rebuild the index from `root_dir`.
    pub fn build(&mut self, root_dir: &Path) -> Result<BuildSummary> {
        let start = Instant::now();
        let root = root_dir
            .canonicalize()
            .unwrap_or_else(|_| root_dir.to_path_buf());

        let mut new_symbols_by_name: HashMap<String, Vec<CodeSymbol>> = HashMap::new();
        let mut new_symbols_by_file: HashMap<String, Vec<CodeSymbol>> = HashMap::new();
        let mut new_mtimes: HashMap<String, u64> = HashMap::new();
        let mut files_scanned = 0usize;
        let mut symbols_found = 0usize;

        let mut dir_stack = vec![root.clone()];
        while let Some(dir) = dir_stack.pop() {
            if files_scanned >= MAX_INDEX_FILES {
                break;
            }
            let entries = match fs::read_dir(&dir) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if files_scanned >= MAX_INDEX_FILES {
                    warn!(
                        "code_index: hit MAX_INDEX_FILES ({}) — stopping scan",
                        MAX_INDEX_FILES
                    );
                    break;
                }
                if let Ok(ft) = entry.file_type() {
                    if ft.is_dir() {
                        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                        if !SKIP_DIRS.contains(&name) && !name.starts_with('.') {
                            dir_stack.push(path);
                        }
                    } else if ft.is_file() {
                        let ext = path
                            .extension()
                            .and_then(|e| e.to_str())
                            .unwrap_or("")
                            .to_lowercase();
                        if !CODE_EXTENSIONS.contains(&ext.as_str()) {
                            continue;
                        }
                        let path_str = path.to_string_lossy().to_string();
                        let mtime = file_mtime_ms(&path).unwrap_or(0);
                        new_mtimes.insert(path_str.clone(), mtime);

                        let symbols = extract_symbols(&path, &ext)?;
                        let symbol_count = symbols.len();
                        if !symbols.is_empty() {
                            for sym in &symbols {
                                let key = sym.name.to_lowercase();
                                new_symbols_by_name
                                    .entry(key)
                                    .or_default()
                                    .push(sym.clone());
                            }
                            new_symbols_by_file.insert(path_str, symbols);
                        }
                        files_scanned += 1;
                        symbols_found += symbol_count;
                    }
                }
            }
        }

        self.symbols_by_name = new_symbols_by_name;
        self.symbols_by_file = new_symbols_by_file;
        self.file_mtimes = new_mtimes;
        self.files_indexed = files_scanned;
        self.total_symbols = symbols_found;
        self.last_built_ms = crate::shared::timestamps::now_ts_ms() as u64;

        let elapsed = start.elapsed();
        info!(
            target: "code_index",
            files = files_scanned,
            symbols = symbols_found,
            elapsed_ms = elapsed.as_millis() as u64,
            "code_index: rebuild complete"
        );

        Ok(BuildSummary {
            files_scanned,
            symbols_found,
            elapsed_ms: elapsed.as_millis() as u64,
        })
    }

    /// Rebuild the index. A true mtime-diff incremental update is not
    /// implemented — every refresh is a full rebuild (documented in the
    /// module docs; the mtime map is collected but not yet used to skip
    /// unchanged files).
    pub fn refresh(&mut self, root_dir: &Path) -> Result<BuildSummary> {
        self.build(root_dir)
    }

    /// Search the index by keyword.
    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchResult> {
        let query = query.to_lowercase();
        let limit = limit.min(MAX_RESULTS);
        let query_terms: Vec<&str> = query.split_whitespace().filter(|t| t.len() >= 2).collect();

        if query_terms.is_empty() {
            return Vec::new();
        }

        // Score each matched symbol.
        let mut scored: Vec<(i64, &CodeSymbol)> = Vec::new();

        // Exact match on symbol name.
        if let Some(symbols) = self.symbols_by_name.get(&query) {
            for sym in symbols {
                scored.push((1000, sym));
            }
        }

        // Prefix match: symbol name starts with query.
        for (key, symbols) in &self.symbols_by_name {
            if key.starts_with(&query) && *key != query {
                for sym in symbols {
                    // Prefer exact word boundary.
                    let score = if sym.name.to_lowercase().starts_with(&query) {
                        500
                    } else {
                        200
                    };
                    scored.push((score, sym));
                }
            }
        }

        // Substring match: query is a substring of symbol name.
        for (key, symbols) in &self.symbols_by_name {
            if key.contains(&query) && !key.starts_with(&query) {
                for sym in symbols {
                    scored.push((100, sym));
                }
            }
        }

        // Multi-term search: all terms must match somewhere in the index.
        if query_terms.len() > 1 {
            for (key, symbols) in &self.symbols_by_name {
                let all_match = query_terms.iter().all(|t| key.contains(t));
                if all_match && !key.contains(&query) {
                    for sym in symbols {
                        scored.push((50, sym));
                    }
                }
            }
        }

        // Deduplicate by (file_path, line).
        let mut seen: std::collections::HashSet<(String, usize)> = std::collections::HashSet::new();
        let mut results: Vec<SearchResult> = Vec::new();

        // Sort by score descending, then by name ascending.
        scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));

        for (score, sym) in scored {
            let key = (sym.file_path.clone(), sym.line);
            if seen.contains(&key) {
                continue;
            }
            seen.insert(key.clone());
            results.push(SearchResult {
                score,
                name: sym.name.clone(),
                kind: format!("{}", sym.kind),
                file_path: sym.file_path.clone(),
                line: sym.line,
                line_text: sym.line_text.clone(),
                language: sym.language.clone(),
            });
            if results.len() >= limit {
                break;
            }
        }

        results
    }

    /// Get all symbols in a given file.
    pub fn symbols_in_file(&self, file_path: &str) -> Vec<&CodeSymbol> {
        self.symbols_by_file
            .get(file_path)
            .map(|v| v.iter().collect())
            .unwrap_or_default()
    }

    /// Get all registered symbol names (for tab-completion).
    pub fn all_symbol_names(&self) -> Vec<&str> {
        self.symbols_by_name.keys().map(|s| s.as_str()).collect()
    }
}

// ---------------------------------------------------------------------------
// BuildSummary
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildSummary {
    pub files_scanned: usize,
    pub symbols_found: usize,
    pub elapsed_ms: u64,
}

// ---------------------------------------------------------------------------
// SearchResult
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// Relevance score.
    pub score: i64,
    /// Symbol name.
    pub name: String,
    /// Kind of symbol.
    pub kind: String,
    /// File path.
    pub file_path: String,
    /// Line number.
    pub line: usize,
    /// Line text.
    pub line_text: String,
    /// Language.
    pub language: String,
}

// ---------------------------------------------------------------------------
// Global index cache
// ---------------------------------------------------------------------------

use std::sync::OnceLock;

static CODE_INDEX: OnceLock<Mutex<CodeIndex>> = OnceLock::new();

fn code_index() -> &'static Mutex<CodeIndex> {
    CODE_INDEX.get_or_init(|| Mutex::new(CodeIndex::new()))
}

// ---------------------------------------------------------------------------
// Symbol extraction
// ---------------------------------------------------------------------------

/// Extract symbols from a file based on its extension.
fn extract_symbols(path: &Path, ext: &str) -> Result<Vec<CodeSymbol>> {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return Ok(Vec::new()),
    };
    let reader = BufReader::new(file);
    let mut symbols = Vec::new();
    let path_str = path.to_string_lossy().to_string();
    let language = ext_to_language(ext);

    for (line_no, line_result) in reader.lines().enumerate() {
        if line_no >= MAX_SYMBOL_LINES {
            break;
        }
        let line = match line_result {
            Ok(l) => l,
            Err(_) => break,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with('#') {
            continue;
        }

        match ext {
            "rs" => {
                // Rust symbols
                if let Some(name) = extract_rust_symbol(trimmed) {
                    let kind = rust_symbol_kind(trimmed);
                    symbols.push(CodeSymbol {
                        name,
                        kind,
                        file_path: path_str.clone(),
                        line: line_no + 1,
                        line_text: trimmed.to_string(),
                        extension: ext.to_string(),
                        language: language.clone(),
                    });
                }
            }
            "py" => {
                if let Some(name) = extract_python_symbol(trimmed) {
                    let kind = python_symbol_kind(trimmed);
                    symbols.push(CodeSymbol {
                        name,
                        kind,
                        file_path: path_str.clone(),
                        line: line_no + 1,
                        line_text: trimmed.to_string(),
                        extension: ext.to_string(),
                        language: language.clone(),
                    });
                }
            }
            "go" => {
                if let Some(name) = extract_go_symbol(trimmed) {
                    let kind = go_symbol_kind(trimmed);
                    symbols.push(CodeSymbol {
                        name,
                        kind,
                        file_path: path_str.clone(),
                        line: line_no + 1,
                        line_text: trimmed.to_string(),
                        extension: ext.to_string(),
                        language: language.clone(),
                    });
                }
            }
            "js" | "ts" | "jsx" | "tsx" => {
                if let Some(name) = extract_ts_symbol(trimmed) {
                    let kind = ts_symbol_kind(trimmed);
                    symbols.push(CodeSymbol {
                        name,
                        kind,
                        file_path: path_str.clone(),
                        line: line_no + 1,
                        line_text: trimmed.to_string(),
                        extension: ext.to_string(),
                        language: language.clone(),
                    });
                }
            }
            "java" | "kt" | "scala" => {
                if let Some(name) = extract_java_symbol(trimmed) {
                    let kind = java_symbol_kind(trimmed);
                    symbols.push(CodeSymbol {
                        name,
                        kind,
                        file_path: path_str.clone(),
                        line: line_no + 1,
                        line_text: trimmed.to_string(),
                        extension: ext.to_string(),
                        language: language.clone(),
                    });
                }
            }
            "c" | "h" | "cpp" | "hpp" | "cxx" | "cc" | "hh" => {
                if let Some(name) = extract_cpp_symbol(trimmed) {
                    symbols.push(CodeSymbol {
                        name,
                        kind: cpp_symbol_kind(trimmed),
                        file_path: path_str.clone(),
                        line: line_no + 1,
                        line_text: trimmed.to_string(),
                        extension: ext.to_string(),
                        language: language.clone(),
                    });
                }
            }
            _ => {
                // Generic fallback: search for `fn `, `def `, `func `, `class ` patterns
                if let Some(name) = extract_generic_symbol(trimmed) {
                    symbols.push(CodeSymbol {
                        name,
                        kind: SymbolKind::Other("symbol".to_string()),
                        file_path: path_str.clone(),
                        line: line_no + 1,
                        line_text: trimmed.to_string(),
                        extension: ext.to_string(),
                        language: language.clone(),
                    });
                }
            }
        }
    }

    Ok(symbols)
}

/// Map file extension to a human-readable language name.
fn ext_to_language(ext: &str) -> String {
    match ext {
        "rs" => "Rust".to_string(),
        "py" => "Python".to_string(),
        "go" => "Go".to_string(),
        "js" => "JavaScript".to_string(),
        "ts" | "tsx" => "TypeScript".to_string(),
        "jsx" => "React JSX".to_string(),
        "java" => "Java".to_string(),
        "kt" => "Kotlin".to_string(),
        "scala" => "Scala".to_string(),
        "swift" => "Swift".to_string(),
        "c" | "h" => "C".to_string(),
        "cpp" | "hpp" | "cxx" | "cc" | "hh" => "C++".to_string(),
        "cs" => "C#".to_string(),
        "rb" => "Ruby".to_string(),
        "php" => "PHP".to_string(),
        "pl" | "pm" => "Perl".to_string(),
        "lua" => "Lua".to_string(),
        "r" => "R".to_string(),
        "zig" => "Zig".to_string(),
        "nim" => "Nim".to_string(),
        "ex" | "exs" => "Elixir".to_string(),
        "toml" => "TOML".to_string(),
        "yaml" | "yml" => "YAML".to_string(),
        "json" => "JSON".to_string(),
        "md" => "Markdown".to_string(),
        "rsx" => "Rust/RSX".to_string(),
        _ => ext.to_string(),
    }
}

/// Get file modification time in milliseconds.
fn file_mtime_ms(path: &Path) -> Option<u64> {
    fs::metadata(path).ok().and_then(|m| {
        m.modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
    })
}

// ---------------------------------------------------------------------------
// Language-specific symbol extractors
// ---------------------------------------------------------------------------

fn extract_rust_symbol(line: &str) -> Option<String> {
    // fn name, pub fn name, async fn name, pub async fn name
    let re = regex_lazy!(r"^\s*(?:pub\s+)?(?:async\s+)?fn\s+(\w+)");
    if let Some(caps) = re.captures(line) {
        return caps.get(1).map(|m| m.as_str().to_string());
    }
    // struct name, pub struct name
    let re = regex_lazy!(r"^\s*(?:pub\s+)?struct\s+(\w+)");
    if let Some(caps) = re.captures(line) {
        return caps.get(1).map(|m| m.as_str().to_string());
    }
    // enum name
    let re = regex_lazy!(r"^\s*(?:pub\s+)?enum\s+(\w+)");
    if let Some(caps) = re.captures(line) {
        return caps.get(1).map(|m| m.as_str().to_string());
    }
    // trait name
    let re = regex_lazy!(r"^\s*(?:pub\s+)?trait\s+(\w+)");
    if let Some(caps) = re.captures(line) {
        return caps.get(1).map(|m| m.as_str().to_string());
    }
    // impl ... for ... / impl ...
    let re = regex_lazy!(r"^\s*(?:pub\s+)?impl\s+(?:<\w+>\s*)?(\w+(?:\s*<\w+>)?)");
    if let Some(caps) = re.captures(line) {
        return caps.get(1).map(|m| m.as_str().to_string());
    }
    // mod name
    let re = regex_lazy!(r"^\s*(?:pub\s+)?mod\s+(\w+)");
    if let Some(caps) = re.captures(line) {
        return caps.get(1).map(|m| m.as_str().to_string());
    }
    // const NAME
    let re = regex_lazy!(r"^\s*(?:pub\s+)?const\s+(\w+)");
    if let Some(caps) = re.captures(line) {
        return caps.get(1).map(|m| m.as_str().to_string());
    }
    // type Name
    let re = regex_lazy!(r"^\s*(?:pub\s+)?type\s+(\w+)");
    if let Some(caps) = re.captures(line) {
        return caps.get(1).map(|m| m.as_str().to_string());
    }
    // macro_rules! name
    let re = regex_lazy!(r"^\s*macro_rules!\s*(\w+)");
    if let Some(caps) = re.captures(line) {
        return caps.get(1).map(|m| m.as_str().to_string());
    }
    None
}

fn rust_symbol_kind(line: &str) -> SymbolKind {
    if line.contains("fn ") {
        if line.contains("impl") {
            SymbolKind::Method
        } else {
            SymbolKind::Function
        }
    } else if line.contains("struct") {
        SymbolKind::Struct
    } else if line.contains("enum") {
        SymbolKind::Enum
    } else if line.contains("trait") {
        SymbolKind::Trait
    } else if line.contains("impl") {
        SymbolKind::Impl
    } else if line.contains("mod ") {
        SymbolKind::Module
    } else if line.contains("const ") {
        SymbolKind::Constant
    } else if line.contains("type ") {
        SymbolKind::TypeAlias
    } else if line.contains("macro_rules!") {
        SymbolKind::Macro
    } else {
        SymbolKind::Other("rust_item".to_string())
    }
}

fn extract_python_symbol(line: &str) -> Option<String> {
    // def name
    let re = regex_lazy!(r"^\s*def\s+(\w+)\s*\(");
    if let Some(caps) = re.captures(line) {
        return caps.get(1).map(|m| m.as_str().to_string());
    }
    // async def name
    let re = regex_lazy!(r"^\s*async\s+def\s+(\w+)\s*\(");
    if let Some(caps) = re.captures(line) {
        return caps.get(1).map(|m| m.as_str().to_string());
    }
    // class name
    let re = regex_lazy!(r"^\s*class\s+(\w+)");
    if let Some(caps) = re.captures(line) {
        return caps.get(1).map(|m| m.as_str().to_string());
    }
    None
}

fn python_symbol_kind(line: &str) -> SymbolKind {
    if line.contains("class ") {
        SymbolKind::Class
    } else {
        SymbolKind::Function
    }
}

fn extract_go_symbol(line: &str) -> Option<String> {
    // func name
    let re = regex_lazy!(r"^\s*func\s+(?:\(\w+\s+\*?\w+\)\s+)?(\w+)\s*\(");
    if let Some(caps) = re.captures(line) {
        return caps.get(1).map(|m| m.as_str().to_string());
    }
    // type name struct / type name interface
    let re = regex_lazy!(r"^\s*type\s+(\w+)\s+(?:struct|interface)");
    if let Some(caps) = re.captures(line) {
        return caps.get(1).map(|m| m.as_str().to_string());
    }
    None
}

fn go_symbol_kind(line: &str) -> SymbolKind {
    if line.contains("func ") {
        SymbolKind::Function
    } else {
        SymbolKind::Struct
    }
}

fn extract_ts_symbol(line: &str) -> Option<String> {
    // function name
    let re = regex_lazy!(r"^\s*(?:export\s+)?(?:async\s+)?function\s+(\w+)");
    if let Some(caps) = re.captures(line) {
        return caps.get(1).map(|m| m.as_str().to_string());
    }
    // class name
    let re = regex_lazy!(r"^\s*(?:export\s+)?(?:abstract\s+)?class\s+(\w+)");
    if let Some(caps) = re.captures(line) {
        return caps.get(1).map(|m| m.as_str().to_string());
    }
    // interface name
    let re = regex_lazy!(r"^\s*(?:export\s+)?interface\s+(\w+)");
    if let Some(caps) = re.captures(line) {
        return caps.get(1).map(|m| m.as_str().to_string());
    }
    // enum name
    let re = regex_lazy!(r"^\s*(?:export\s+)?enum\s+(\w+)");
    if let Some(caps) = re.captures(line) {
        return caps.get(1).map(|m| m.as_str().to_string());
    }
    // type name
    let re = regex_lazy!(r"^\s*(?:export\s+)?type\s+(\w+)");
    if let Some(caps) = re.captures(line) {
        return caps.get(1).map(|m| m.as_str().to_string());
    }
    // const/module/var name
    let re = regex_lazy!(r"^\s*(?:export\s+)?(?:const|let|var)\s+(\w+)");
    if let Some(caps) = re.captures(line) {
        return caps.get(1).map(|m| m.as_str().to_string());
    }
    None
}

fn ts_symbol_kind(line: &str) -> SymbolKind {
    if line.contains("function") {
        SymbolKind::Function
    } else if line.contains("class") {
        SymbolKind::Class
    } else if line.contains("interface") {
        SymbolKind::Interface
    } else if line.contains("enum") {
        SymbolKind::Enum
    } else if line.contains("type ") {
        SymbolKind::TypeAlias
    } else {
        SymbolKind::Variable
    }
}

fn extract_java_symbol(line: &str) -> Option<String> {
    // class/interface/enum name
    let re = regex_lazy!(
        r"^\s*(?:public\s+|private\s+|protected\s+)?(?:abstract\s+|final\s+|static\s+)*(?:class|interface|enum|record)\s+(\w+)"
    );
    if let Some(caps) = re.captures(line) {
        return caps.get(1).map(|m| m.as_str().to_string());
    }
    // method
    let re = regex_lazy!(
        r"^\s*(?:public\s+|private\s+|protected\s+)?(?:static\s+|abstract\s+|final\s+|synchronized\s+)*(?:\w+\s+)+(\w+)\s*\("
    );
    if let Some(caps) = re.captures(line) {
        return caps.get(1).map(|m| m.as_str().to_string());
    }
    None
}

fn java_symbol_kind(line: &str) -> SymbolKind {
    if line.contains("class ") {
        SymbolKind::Class
    } else if line.contains("interface ") {
        SymbolKind::Interface
    } else if line.contains("enum ") {
        SymbolKind::Enum
    } else {
        SymbolKind::Method
    }
}

fn extract_cpp_symbol(line: &str) -> Option<String> {
    // class/struct name
    let re = regex_lazy!(r"^\s*(?:class|struct)\s+(\w+)");
    if let Some(caps) = re.captures(line) {
        return caps.get(1).map(|m| m.as_str().to_string());
    }
    // return_type name(...)
    let re = regex_lazy!(r"^\s*(?:\w+(?:\s*[*&])?\s+)+(\w+)\s*\([^)]*\)\s*(?:const\s*)?(?:\{|;)");
    if let Some(caps) = re.captures(line) {
        return caps.get(1).map(|m| m.as_str().to_string());
    }
    None
}

fn cpp_symbol_kind(line: &str) -> SymbolKind {
    if line.contains("class ") {
        SymbolKind::Class
    } else if line.contains("struct ") {
        SymbolKind::Struct
    } else {
        SymbolKind::Function
    }
}

fn extract_generic_symbol(line: &str) -> Option<String> {
    // fn name, def name, func name, class name
    let re = regex_lazy!(r"^\s*(?:fn|def|func|class|sub)\s+(\w+)");
    if let Some(caps) = re.captures(line) {
        return caps.get(1).map(|m| m.as_str().to_string());
    }
    None
}

// ---------------------------------------------------------------------------
// Regex lazy initialization helper
// ---------------------------------------------------------------------------

macro_rules! regex_lazy {
    ($re:literal $(,)?) => {{
        use std::sync::OnceLock;
        static RE: OnceLock<regex::Regex> = OnceLock::new();
        RE.get_or_init(|| regex::Regex::new($re).expect("invalid regex"))
    }};
}

use regex_lazy;

// ---------------------------------------------------------------------------
// CodeIndexTool
// ---------------------------------------------------------------------------

/// Tool that builds and searches a persistent code symbol index.
///
/// Operations:
/// - `build { "directory": "/path" }` — Build index for a directory
/// - `search { "query": "str", "limit": 20 }` — Search the index
/// - `refresh { "directory": "/path" }` — Incremental refresh
/// - `stats {}` — Show index statistics
pub struct CodeIndexTool;

impl Tool for CodeIndexTool {
    fn name(&self) -> &'static str {
        "code_index_search"
    }

    fn run(&self, input: &ToolInput) -> Result<ToolOutput> {
        let payload = &input.payload;
        let operation = payload
            .get("operation")
            .and_then(|v| v.as_str())
            .unwrap_or("search");

        match operation {
            "build" => {
                let directory = payload
                    .get("directory")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        anyhow::anyhow!("code_index_search 'build' requires 'directory'")
                    })?;
                let dir_path = sanitize_path_for_write(input, directory)?;
                let mut index = code_index()
                    .lock()
                    .map_err(|e| anyhow::anyhow!("lock error: {}", e))?;
                let summary = index.build(&dir_path)?;
                Ok(ToolOutput {
                    success: true,
                    result: Some(serde_json::json!({
                        "operation": "build",
                        "files_indexed": summary.files_scanned,
                        "symbols_found": summary.symbols_found,
                        "elapsed_ms": summary.elapsed_ms,
                    })),
                    error: None,
                    verification: Some("code_index_built".to_string()),
                    audit_log: Some(format!(
                        "CodeIndex: built index ({} files, {} symbols)",
                        summary.files_scanned, summary.symbols_found
                    )),
                    pua_report: Some(tool_execution_report("code_index_search", Some("build"))),
                })
            }
            "refresh" => {
                let directory = payload
                    .get("directory")
                    .and_then(|v| v.as_str())
                    .unwrap_or(".");
                let dir_path = PathBuf::from(directory);
                let mut index = code_index()
                    .lock()
                    .map_err(|e| anyhow::anyhow!("lock error: {}", e))?;
                let summary = index.refresh(&dir_path)?;
                Ok(ToolOutput {
                    success: true,
                    result: Some(serde_json::json!({
                        "operation": "refresh",
                        "files_scanned": summary.files_scanned,
                        "symbols_found": summary.symbols_found,
                        "elapsed_ms": summary.elapsed_ms,
                    })),
                    error: None,
                    verification: Some("code_index_refreshed".to_string()),
                    audit_log: Some(format!(
                        "CodeIndex: refreshed ({} files, {} symbols)",
                        summary.files_scanned, summary.symbols_found
                    )),
                    pua_report: Some(tool_execution_report("code_index_search", Some("refresh"))),
                })
            }
            "stats" => {
                let index = code_index()
                    .lock()
                    .map_err(|e| anyhow::anyhow!("lock error: {}", e))?;
                Ok(ToolOutput {
                    success: true,
                    result: Some(serde_json::json!({
                        "operation": "stats",
                        "files_indexed": index.files_indexed,
                        "total_symbols": index.total_symbols,
                        "last_built_ms": index.last_built_ms,
                        "all_symbol_names": index.all_symbol_names(),
                    })),
                    error: None,
                    verification: Some("code_index_stats".to_string()),
                    audit_log: None,
                    pua_report: Some(tool_execution_report("code_index_search", Some("stats"))),
                })
            }
            _ => {
                let query = operation;
                let limit = payload.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
                let mut index = code_index()
                    .lock()
                    .map_err(|e| anyhow::anyhow!("lock error: {}", e))?;
                // Documented lifecycle: the index is built on the first search
                // call (module docs promise a first-call rebuild). Previously
                // search silently returned empty results until an explicit
                // `build`/`refresh` was issued — the docs and the code did not
                // agree. The auto-build is one-time (the index stays warm).
                if index.files_indexed == 0 && index.total_symbols == 0 {
                    let directory = payload
                        .get("directory")
                        .and_then(|v| v.as_str())
                        .unwrap_or(".");
                    match sanitize_path_for_write(input, directory)
                        .and_then(|dir| index.build(&dir))
                    {
                        Ok(summary) => info!(
                            "CodeIndex: auto-built on first search ({} files, {} symbols)",
                            summary.files_scanned, summary.symbols_found
                        ),
                        Err(e) => warn!("CodeIndex: first-search auto-build failed: {}", e),
                    }
                }
                let results = index.search(query, limit);
                Ok(ToolOutput {
                    success: true,
                    result: Some(serde_json::json!({
                        "operation": "search",
                        "query": query,
                        "total": results.len(),
                        "results": results,
                    })),
                    error: None,
                    verification: Some("code_index_searched".to_string()),
                    audit_log: Some(format!(
                        "CodeIndex: searched '{}' ({} results)",
                        query,
                        results.len()
                    )),
                    pua_report: Some(tool_execution_report("code_index_search", Some("search"))),
                })
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_extract_rust_function() {
        assert_eq!(
            extract_rust_symbol("pub fn hello_world() -> Result<()> {"),
            Some("hello_world".to_string())
        );
        assert_eq!(
            extract_rust_symbol("fn run(&self, input: &ToolInput) -> Result<ToolOutput> {"),
            Some("run".to_string())
        );
        assert_eq!(
            extract_rust_symbol("pub async fn handle_request(req: Request) -> Response {"),
            Some("handle_request".to_string())
        );
    }

    #[test]
    fn test_extract_rust_struct() {
        assert_eq!(
            extract_rust_symbol("pub struct ServerBuilder {"),
            Some("ServerBuilder".to_string())
        );
        assert_eq!(
            extract_rust_symbol("struct ToolRegistry {"),
            Some("ToolRegistry".to_string())
        );
    }

    #[test]
    fn test_extract_rust_enum() {
        assert_eq!(
            extract_rust_symbol("pub enum SandboxLevel {"),
            Some("SandboxLevel".to_string())
        );
    }

    #[test]
    fn test_extract_rust_trait() {
        assert_eq!(
            extract_rust_symbol("pub trait Tool: Send + Sync {"),
            Some("Tool".to_string())
        );
    }

    #[test]
    fn test_extract_rust_mod() {
        assert_eq!(
            extract_rust_symbol("pub mod code_index;"),
            Some("code_index".to_string())
        );
    }

    #[test]
    fn test_extract_rust_const() {
        assert_eq!(
            extract_rust_symbol("const MAX_RESULTS: usize = 50;"),
            Some("MAX_RESULTS".to_string())
        );
    }

    #[test]
    fn test_extract_python_def() {
        assert_eq!(
            extract_python_symbol("def hello_world():"),
            Some("hello_world".to_string())
        );
        assert_eq!(
            extract_python_symbol("async def fetch_data(url: str) -> dict:"),
            Some("fetch_data".to_string())
        );
    }

    #[test]
    fn test_extract_python_class() {
        assert_eq!(
            extract_python_symbol("class CodeIndexTool:"),
            Some("CodeIndexTool".to_string())
        );
    }

    #[test]
    fn test_extract_go_func() {
        assert_eq!(
            extract_go_symbol("func HelloWorld() string {"),
            Some("HelloWorld".to_string())
        );
        assert_eq!(
            extract_go_symbol(
                "func (s *Server) ServeHTTP(w http.ResponseWriter, r *http.Request) {"
            ),
            Some("ServeHTTP".to_string())
        );
    }

    #[test]
    fn test_extract_ts_function() {
        assert_eq!(
            extract_ts_symbol("function helloWorld() {"),
            Some("helloWorld".to_string())
        );
        assert_eq!(
            extract_ts_symbol("export async function fetchData(url: string): Promise<Data> {"),
            Some("fetchData".to_string())
        );
    }

    #[test]
    fn test_extract_ts_class() {
        assert_eq!(
            extract_ts_symbol("export class ServerBuilder {"),
            Some("ServerBuilder".to_string())
        );
    }

    #[test]
    fn test_build_and_search() {
        let tmp = TempDir::new().expect("temp dir");
        // Create test files
        std::fs::write(
            tmp.path().join("main.rs"),
            "fn main() { println!(\"hello\"); }\nfn helper() {}\npub fn public_fn() {}",
        )
        .unwrap();
        std::fs::write(
            tmp.path().join("lib.rs"),
            "pub struct MyStruct {}\npub enum MyEnum {}\npub trait MyTrait {}",
        )
        .unwrap();

        let mut index = CodeIndex::new();
        let summary = index.build(tmp.path()).expect("build should succeed");
        assert!(summary.files_scanned >= 2);
        assert!(summary.symbols_found >= 6);

        // Search for "main"
        let results = index.search("main", 10);
        assert!(!results.is_empty());
        assert!(results.iter().any(|r| r.name == "main"));

        // Search for "MyStruct"
        let results = index.search("MyStruct", 10);
        assert!(!results.is_empty());
        assert_eq!(results[0].name, "MyStruct");
    }

    #[test]
    fn test_first_search_auto_builds_index() {
        // Regression: module docs promise a first-search auto-build, but the
        // search branch previously returned empty results until an explicit
        // `build`/`refresh` — docs and code disagreed (principle §18). This
        // exercises the real `run` path: a bare search (no build/refresh)
        // against the empty global index must trigger the one-time auto-build
        // and return symbols. The global singleton is not touched by any other
        // test, so its empty state at this test's start is deterministic.
        let tmp = TempDir::new().expect("temp dir");
        std::fs::write(
            tmp.path().join("main.rs"),
            "fn main() {}\npub fn auto_build_probe() {}",
        )
        .unwrap();

        let input = ToolInput {
            task_id: "code-index-test".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "auto-build".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({
                "operation": "auto_build_probe",
                "directory": tmp.path().to_string_lossy(),
                "limit": 10,
            }),
            allowed_base_dir: None,
        };
        let output = CodeIndexTool.run(&input).expect("run should succeed");
        assert!(output.success);
        let result = output.result.expect("result");
        assert_eq!(result["operation"], "search");
        assert!(
            result["total"].as_u64().unwrap_or(0) > 0,
            "first search must auto-build and return symbols, got: {result}"
        );
        assert!(!result["results"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_build_skips_nonexistent_directory() {
        let mut index = CodeIndex::new();
        let result = index.build(Path::new("/nonexistent-path-12345"));
        assert!(result.is_ok());
        assert_eq!(result.unwrap().files_scanned, 0);
    }

    #[test]
    fn test_search_empty_index() {
        let index = CodeIndex::new();
        let results = index.search("anything", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_symbols_in_file() {
        let mut index = CodeIndex::new();
        let tmp = TempDir::new().expect("temp dir");
        std::fs::write(
            tmp.path().join("test.rs"),
            "pub fn foo() {}\npub struct Bar {}",
        )
        .unwrap();
        index.build(tmp.path()).expect("build");
        let file_path = tmp.path().join("test.rs").to_string_lossy().to_string();
        let symbols = index.symbols_in_file(&file_path);
        assert_eq!(symbols.len(), 2);
    }
}
