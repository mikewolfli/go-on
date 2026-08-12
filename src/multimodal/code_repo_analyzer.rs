//! Code Repository Analyzer (GAP-B52-30)
//!
//! Provides repository cloning, build-graph generation, type-index extraction,
//! and code-question-answering capabilities. Used by the chat system when a
//! `repo:` prefix is detected in the user's input.
//!
//! # Architecture
//!
//! ```text
//! Chat input with `repo:` prefix
//!         │
//!         ▼
//!   RepoAnalyzer::clone(url) ───► RepoContext
//!         │                         │
//!         ├── build_repo_map() ─────┤──► RepoMap (file graph)
//!         └── extract_types() ──────┘──► TypeIndex (symbol index)
//!                │
//!                ▼
//!   answer_code_question(question, repo) ───► Answer
//! ```

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::Mutex;
use tracing::info;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Marker prefix in chat input that triggers repository analysis.
pub const REPO_PREFIX: &str = "repo:";

/// File extensions whose content is scanned for import statements.
const IMPORT_SCAN_EXTENSIONS: &[&str] = &["rs", "py", "js", "jsx", "ts", "tsx", "mjs", "cjs"];

/// Module index files that act as a directory's entry point.
const MODULE_INDEX_FILES: &[&str] = &[
    "mod.rs",
    "__init__.py",
    "index.js",
    "index.jsx",
    "index.ts",
    "index.tsx",
    "index.mjs",
    "index.cjs",
];

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum RepoAnalyzerError {
    #[error("failed to clone repository: {0}")]
    CloneFailed(String),

    #[error("repository path not found: {0}")]
    PathNotFound(String),

    #[error("failed to build repo map: {0}")]
    BuildMapFailed(String),

    #[error("failed to extract type index: {0}")]
    TypeExtractionFailed(String),

    #[error("failed to answer question: {0}")]
    AnswerFailed(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("internal error: {0}")]
    Internal(String),
}

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// Full context for a cloned / loaded repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoContext {
    /// The repository URL or local path it was loaded from.
    pub source: String,
    /// Absolute path to the repository root on disk.
    pub root_path: PathBuf,
    /// The repository map (file graph).
    pub repo_map: RepoMap,
    /// The type index (symbols and definitions).
    pub type_index: TypeIndex,
    /// Git HEAD commit hash (if available).
    pub head_commit: Option<String>,
    /// Language distribution (extension -> file count).
    pub language_stats: HashMap<String, usize>,
    /// Keeps the temporary clone directory alive for the lifetime of the
    /// context. `None` for local paths. Serialization skips it because
    /// `TempDir` is not serializable; the `Arc` keeps the clone directory
    /// alive as long as any clone of this context is alive.
    #[serde(skip)]
    pub temp_dir: Option<std::sync::Arc<tempfile::TempDir>>,
}

/// Graph representation of the repository file structure.
///
/// Nodes are file paths (relative to repo root). Edges represent
/// import / include / dependency relationships between files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoMap {
    /// Root-relative file paths indexed by a short unique ID.
    pub files: Vec<String>,
    /// Adjacency list: `dependencies[file_id]` = set of file_ids this file imports.
    pub dependencies: HashMap<usize, HashSet<usize>>,
    /// Reverse dependency list.
    pub dependents: HashMap<usize, HashSet<usize>>,
    /// Total lines of code per file.
    pub loc: HashMap<usize, usize>,
}

impl RepoMap {
    /// Create an empty repo map.
    pub fn empty() -> Self {
        Self {
            files: Vec::new(),
            dependencies: HashMap::new(),
            dependents: HashMap::new(),
            loc: HashMap::new(),
        }
    }

    /// Number of files in the map.
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// Total lines of code across all files.
    pub fn total_loc(&self) -> usize {
        self.loc.values().sum()
    }
}

/// An indexed type / symbol definition within a repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeEntry {
    /// The fully-qualified name (e.g. `crate::module::StructName`).
    pub qualified_name: String,
    /// The short name.
    pub name: String,
    /// Kind of symbol (struct, enum, trait, function, type alias, etc.).
    pub kind: SymbolKind,
    /// File ID within the repo map.
    pub file_id: usize,
    /// Line number of the definition.
    pub line: usize,
    /// Column number of the definition.
    pub column: usize,
    /// Documentation / comment summary extracted from above the definition.
    pub doc_summary: Option<String>,
    /// Visibility (pub, pub(crate), private, etc.).
    pub visibility: Option<String>,
}

/// Kind of a code symbol.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Struct,
    Enum,
    Trait,
    Function,
    Method,
    TypeAlias,
    Constant,
    Module,
    Macro,
    Interface,
    Class,
    Variable,
    Other(String),
}

/// A type index — a collection of all extracted type/symbol definitions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeIndex {
    /// All entries in the index.
    pub entries: Vec<TypeEntry>,
    /// Quick lookup: qualified_name -> entry.
    pub by_name: HashMap<String, usize>,
    /// Files that contain each symbol kind.
    pub by_kind: HashMap<SymbolKind, Vec<usize>>,
}

impl TypeIndex {
    /// Create an empty type index.
    pub fn empty() -> Self {
        Self {
            entries: Vec::new(),
            by_name: HashMap::new(),
            by_kind: HashMap::new(),
        }
    }

    /// Number of entries in the index.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Return `true` if the index is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Look up an entry by its fully-qualified name.
    pub fn get(&self, name: &str) -> Option<&TypeEntry> {
        self.by_name.get(name).map(|&i| &self.entries[i])
    }

    /// Add an entry to the index.
    pub fn add(&mut self, entry: TypeEntry) {
        let idx = self.entries.len();
        self.by_name.insert(entry.qualified_name.clone(), idx);
        self.by_kind
            .entry(entry.kind.clone())
            .or_default()
            .push(idx);
        self.entries.push(entry);
    }
}

/// An answer produced by the code QA system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Answer {
    /// The natural-language answer text.
    pub text: String,
    /// Source references (file paths + line numbers) supporting the answer.
    pub references: Vec<SourceRef>,
    /// Confidence score (0.0 – 1.0).
    pub confidence: f64,
    /// Whether the answer was based on the full repo or a subset.
    pub coverage: AnswerCoverage,
}

/// A source reference used to support an answer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceRef {
    pub file: String,
    pub line: usize,
    pub snippet: String,
}

/// How much of the repository was consulted to produce an answer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AnswerCoverage {
    FullRepo,
    Subset(String),
}

// ---------------------------------------------------------------------------
// RepoAnalyzer
// ---------------------------------------------------------------------------

/// Analyzer for code repositories. Supports cloning, mapping, type-indexing,
/// and answering natural-language questions about the code.
#[derive(Debug)]
pub struct RepoAnalyzer {
    /// Cache of cloned repos keyed by URL / path.
    cache: Arc<Mutex<HashMap<String, RepoContext>>>,
    /// Insertion order of cache keys for FIFO eviction (bounds disk usage:
    /// each entry pins a full clone in a temp dir for the process lifetime).
    cache_order: Arc<Mutex<std::collections::VecDeque<String>>>,
    /// Maximum file size (in bytes) to index.
    max_file_size: u64,
    /// Maximum number of files indexed per repository.
    max_repo_files: usize,
    /// File extensions to exclude from analysis.
    excluded_extensions: HashSet<String>,
    /// Directories to exclude from analysis.
    excluded_dirs: HashSet<String>,
}

/// Maximum number of cloned repositories retained in the in-memory cache.
const MAX_CACHE_ENTRIES: usize = 16;
/// Maximum number of files indexed per repository (bounds the walk for
/// hostile inputs like `repo:/`).
const MAX_REPO_FILES: usize = 50_000;

impl Default for RepoAnalyzer {
    fn default() -> Self {
        let mut excluded_ext = HashSet::new();
        excluded_ext.insert("png".into());
        excluded_ext.insert("jpg".into());
        excluded_ext.insert("jpeg".into());
        excluded_ext.insert("gif".into());
        excluded_ext.insert("svg".into());
        excluded_ext.insert("ico".into());
        excluded_ext.insert("woff".into());
        excluded_ext.insert("woff2".into());
        excluded_ext.insert("ttf".into());
        excluded_ext.insert("eot".into());
        excluded_ext.insert("o".into());
        excluded_ext.insert("so".into());
        excluded_ext.insert("dll".into());
        excluded_ext.insert("exe".into());
        excluded_ext.insert("bin".into());

        let mut excluded_dirs = HashSet::new();
        excluded_dirs.insert(".git".into());
        excluded_dirs.insert("node_modules".into());
        excluded_dirs.insert("target".into());
        excluded_dirs.insert("build".into());
        excluded_dirs.insert("dist".into());
        excluded_dirs.insert(".venv".into());
        excluded_dirs.insert("__pycache__".into());

        Self {
            cache: Arc::new(Mutex::new(HashMap::new())),
            cache_order: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            max_file_size: 512 * 1024, // 512 KB
            max_repo_files: MAX_REPO_FILES,
            excluded_extensions: excluded_ext,
            excluded_dirs,
        }
    }
}

/// Match result for a code symbol found during RAG context building.
#[derive(Debug, Clone)]
struct SymbolMatch {
    name: String,
    qualified_name: String,
    kind: SymbolKind,
    line: usize,
    files: Vec<String>,
}

/// Context built for retrieval-augmented code question answering.
struct RagContext {
    found_symbols: Vec<SymbolMatch>,
    file_count: usize,
    has_file_intent: bool,
    has_symbol_intent: bool,
    has_lang_intent: bool,
    has_loc_intent: bool,
}

impl RepoAnalyzer {
    /// Create a new `RepoAnalyzer` with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new `RepoAnalyzer` with custom exclusions.
    pub fn with_exclusions(
        max_file_size: u64,
        excluded_extensions: HashSet<String>,
        excluded_dirs: HashSet<String>,
    ) -> Self {
        Self {
            cache: Arc::new(Mutex::new(HashMap::new())),
            cache_order: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            max_file_size,
            max_repo_files: MAX_REPO_FILES,
            excluded_extensions,
            excluded_dirs,
        }
    }

    // ── Cloning ─────────────────────────────────────────────────────────

    /// Clone a remote repository or load a local path.
    ///
    /// For remote URLs (starting with `http://`, `https://`, `git@`, or
    /// `ssh://`), performs a shallow clone. For local paths, copies a
    /// symlink-based reference.
    pub async fn clone(&self, url: &str) -> Result<RepoContext, RepoAnalyzerError> {
        // Check cache first.
        {
            let cache = self.cache.lock().await;
            if let Some(ctx) = cache.get(url) {
                info!("RepoAnalyzer::clone: cache hit for {}", url);
                return Ok(ctx.clone());
            }
        }

        info!("RepoAnalyzer::clone: cloning {}", url);

        // Determine if remote or local.
        let is_remote = url.starts_with("http://")
            || url.starts_with("https://")
            || url.starts_with("git@")
            || url.starts_with("ssh://");

        let (root_path, temp_dir) = if is_remote {
            let dir = tempfile::TempDir::new()
                .map_err(|e| RepoAnalyzerError::CloneFailed(e.to_string()))?;
            let dest = dir.path().join("repo");

            // Attempt git clone for remote URLs
            let clone_result = tokio::process::Command::new("git")
                .args(["clone", "--depth", "1", url])
                .arg(&dest)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .output()
                .await;

            match clone_result {
                Ok(output) if output.status.success() => {
                    info!("RepoAnalyzer::clone: cloned {} to {:?}", url, dest);
                }
                Ok(output) => {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(RepoAnalyzerError::CloneFailed(format!(
                        "git clone failed: {}",
                        stderr.trim()
                    )));
                }
                Err(e) => {
                    return Err(RepoAnalyzerError::CloneFailed(format!(
                        "git not available for remote repo clone: {}. Install git to enable remote repository analysis.",
                        e
                    )));
                }
            }
            // Keep the TempDir alive for the lifetime of the RepoContext;
            // dropping it here would delete the freshly cloned repository.
            (dest, Some(std::sync::Arc::new(dir)))
        } else {
            let local = PathBuf::from(url);
            if !local.exists() {
                return Err(RepoAnalyzerError::PathNotFound(url.into()));
            }
            // Reject walking a filesystem root (e.g. `repo:/`): the walk has
            // no total size bound and would scan the whole tree.
            if local.parent().is_none() {
                return Err(RepoAnalyzerError::CloneFailed(format!(
                    "refusing to analyze filesystem root: {}",
                    local.display()
                )));
            }
            (local, None)
        };

        // Build repo map and type index (single scan — the map is reused
        // for type extraction instead of walking the tree twice).
        let repo_map = self.build_repo_map(&root_path).await?;
        let type_index = self.extract_types(&root_path, &repo_map).await?;

        // Language statistics.
        let mut language_stats: HashMap<String, usize> = HashMap::new();
        for f in &repo_map.files {
            if let Some(ext) = Path::new(f).extension().and_then(|e| e.to_str()) {
                *language_stats.entry(ext.to_lowercase()).or_insert(0) += 1;
            }
        }

        // Git HEAD commit hash (best effort; missing git is not fatal).
        let head_commit = match tokio::process::Command::new("git")
            .args(["-C", &root_path.to_string_lossy(), "rev-parse", "HEAD"])
            .output()
            .await
        {
            Ok(out) if out.status.success() => {
                Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
            }
            _ => None,
        };

        let ctx = RepoContext {
            source: url.to_owned(),
            root_path,
            repo_map,
            type_index,
            head_commit,
            language_stats,
            temp_dir,
        };

        // Insert into cache under a FIFO cap: each entry pins a full clone on
        // disk for the process lifetime, so unbounded caching is a disk leak.
        {
            let mut cache = self.cache.lock().await;
            let mut order = self.cache_order.lock().await;
            cache.insert(url.to_owned(), ctx.clone());
            order.push_back(url.to_owned());
            while order.len() > MAX_CACHE_ENTRIES {
                if let Some(oldest) = order.pop_front() {
                    cache.remove(&oldest);
                }
            }
        }

        Ok(ctx)
    }

    // ── Repo map ────────────────────────────────────────────────────────

    /// Build or refresh the repository map from the given local path.
    ///
    /// Scans all files recursively (excluding configured directories and
    /// extensions), records line counts, and attempts to parse import
    /// statements for dependency edges.
    pub async fn build_repo_map(&self, path: &Path) -> Result<RepoMap, RepoAnalyzerError> {
        info!("RepoAnalyzer::build_repo_map: scanning {:?}", path);
        let mut repo_map = RepoMap::empty();
        let mut contents: Vec<Option<String>> = Vec::new();

        self.walk_and_collect(path, path, &mut repo_map, &mut contents)
            .await?;
        // Resolve import statements between same-repo files into dependency
        // edges. Targets that cannot be mapped to a repo file (external
        // crates, npm packages, stdlib, ...) are skipped without error.
        resolve_dependencies(&mut repo_map, &contents);

        info!(
            "RepoAnalyzer::build_repo_map: {} files, {} edges",
            repo_map.file_count(),
            repo_map
                .dependencies
                .values()
                .map(|s| s.len())
                .sum::<usize>()
        );
        Ok(repo_map)
    }

    async fn walk_and_collect(
        &self,
        root: &Path,
        dir: &Path,
        repo_map: &mut RepoMap,
        contents: &mut Vec<Option<String>>,
    ) -> Result<(), RepoAnalyzerError> {
        // Bound the total walk: without a cap, `repo:/` on a large tree (or a
        // symlinked system dir) would scan unbounded numbers of files.
        if repo_map.files.len() >= self.max_repo_files {
            return Ok(());
        }
        let mut entries = tokio::fs::read_dir(dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let ft = entry.file_type().await?;
            let name = entry.file_name().to_string_lossy().to_string();
            let abs_path = entry.path();

            if ft.is_dir() {
                if !self.excluded_dirs.contains(&name) {
                    Box::pin(self.walk_and_collect(root, &abs_path, repo_map, contents)).await?;
                }
            } else if ft.is_file() {
                let rel_path = abs_path
                    .strip_prefix(root)
                    .unwrap_or(&abs_path)
                    .to_string_lossy()
                    .to_string();

                // Skip excluded extensions.
                if let Some(ext) = Path::new(&rel_path).extension().and_then(|e| e.to_str()) {
                    if self.excluded_extensions.contains(ext) {
                        continue;
                    }
                }

                // Skip oversized files.
                let metadata = tokio::fs::metadata(&abs_path).await?;
                if metadata.len() > self.max_file_size {
                    continue;
                }

                let file_id = repo_map.files.len();
                repo_map.files.push(rel_path.clone());
                // Read the content once: it is used both for the line count
                // (best effort; non-UTF8 or unreadable files count as 0 lines)
                // and later for import-based dependency edge detection.
                let content = tokio::fs::read_to_string(&abs_path).await.ok();
                let line_count = content.as_deref().map(|c| c.lines().count()).unwrap_or(0);
                repo_map.loc.insert(file_id, line_count);
                contents.push(content);

                // Every file starts with an empty edge set; import resolution
                // in `resolve_dependencies` fills them in afterwards.
                repo_map.dependencies.insert(file_id, HashSet::new());
                repo_map.dependents.insert(file_id, HashSet::new());
            }
        }
        Ok(())
    }

    // ── Type index ──────────────────────────────────────────────────────

    /// Extract type definitions / symbol index from the repository at `path`.
    ///
    /// Reuses the already-built [`RepoMap`] so the file tree is scanned only
    /// once per `clone()` call. Heuristic extraction covers common language
    /// constructs (struct/enum/trait/fn definitions).
    pub async fn extract_types(
        &self,
        path: &Path,
        repo_map: &RepoMap,
    ) -> Result<TypeIndex, RepoAnalyzerError> {
        info!("RepoAnalyzer::extract_types: scanning {:?}", path);
        let mut index = TypeIndex::empty();

        for (file_id, rel_path) in repo_map.files.iter().enumerate() {
            let abs_path = path.join(rel_path);
            let content = match tokio::fs::read_to_string(&abs_path).await {
                Ok(c) => c,
                Err(_) => continue, // binary or non-utf8 file
            };

            // Simple line-by-line heuristic for Rust-style definitions.
            for (line_num, line) in content.lines().enumerate() {
                let trimmed = line.trim();
                // Detect `pub struct Name`, `pub enum Name`, `pub trait Name`, `pub fn name`, etc.
                if let Some(name) = self.extract_definition(trimmed) {
                    let kind = self.infer_kind(trimmed);
                    let qualified = format!(
                        "{}::{}",
                        rel_path.replace('/', "::").replace(".rs", ""),
                        name
                    );
                    index.add(TypeEntry {
                        qualified_name: qualified,
                        name,
                        kind,
                        file_id,
                        line: line_num + 1,
                        column: trimmed.find(|c: char| !c.is_whitespace()).unwrap_or(0) + 1,
                        doc_summary: None,
                        visibility: Some(if trimmed.starts_with("pub") {
                            "pub".into()
                        } else {
                            "private".into()
                        }),
                    });
                }
            }
        }

        info!("RepoAnalyzer::extract_types: {} entries", index.len());
        Ok(index)
    }

    /// Simple heuristic: extract an identifier after `struct`, `enum`, `trait`, `fn`, etc.
    fn extract_definition(&self, line: &str) -> Option<String> {
        let keywords = ["struct ", "enum ", "trait ", "fn ", "type ", "impl "];
        for kw in &keywords {
            // Check for `pub struct Name`, `pub(crate) struct Name`, or just `struct Name`.
            if let Some(pos) = line.find(kw) {
                let after_kw = &line[pos + kw.len()..];
                let name = after_kw
                    .split(|c: char| {
                        c.is_whitespace() || c == '(' || c == '<' || c == ';' || c == '{'
                    })
                    .next()
                    .unwrap_or("")
                    .to_string();
                if !name.is_empty() && name.starts_with(|c: char| c.is_alphabetic() || c == '_') {
                    return Some(name);
                }
            }
        }
        None
    }

    fn infer_kind(&self, line: &str) -> SymbolKind {
        if line.contains("struct ") {
            SymbolKind::Struct
        } else if line.contains("enum ") {
            SymbolKind::Enum
        } else if line.contains("trait ") {
            SymbolKind::Trait
        } else if line.contains("fn ") {
            SymbolKind::Function
        } else if line.contains("type ") {
            SymbolKind::TypeAlias
        } else {
            SymbolKind::Other("unknown".into())
        }
    }

    // ── Code QA ─────────────────────────────────────────────────────────

    /// Answer a natural-language question about the repository context.
    ///
    /// Uses the `type_index` and `repo_map` to ground answers in actual
    /// code symbols via retrieval-augmented generation.
    ///
    /// The implementation is heuristic-only: it builds a keyword / symbol
    /// retrieval context (`build_rag_context`) and composes the answer from
    /// the matched symbols and file structure. There is no LLM delegation in
    /// this path.
    pub async fn answer_code_question(
        &self,
        question: &str,
        repo: &RepoContext,
    ) -> Result<Answer, RepoAnalyzerError> {
        info!(
            "RepoAnalyzer::answer_code_question: question='{}' on {}",
            question, repo.source
        );

        // Gather relevant context: type definitions and file structure
        let context = self.build_rag_context(question, repo);

        // Build answer from gathered context
        let (text, references) = if context.found_symbols.is_empty() && context.file_count == 0 {
            (
                format!(
                    "Analyzed repository '{}' with {} files and {} type definitions. No specific matches found for your question.",
                    repo.source,
                    repo.repo_map.file_count(),
                    repo.type_index.len(),
                ),
                Vec::new(),
            )
        } else if context.found_symbols.len() == 1 {
            let sym = &context.found_symbols[0];
            let refs = sym
                .files
                .iter()
                .map(|f| SourceRef {
                    file: f.clone(),
                    line: sym.line,
                    snippet: sym.qualified_name.clone(),
                })
                .collect();
            let kind_label = sym.kind.debug_label();
            (
                format!(
                    "Found {} '{}' defined in {} file(s): {}.\nQualified name: {}",
                    kind_label,
                    sym.name,
                    sym.files.len(),
                    sym.files.join(", "),
                    sym.qualified_name
                ),
                refs,
            )
        } else {
            let refs: Vec<SourceRef> = context
                .found_symbols
                .iter()
                .flat_map(|sym| {
                    sym.files.iter().map(|f| SourceRef {
                        file: f.clone(),
                        line: sym.line,
                        snippet: sym.qualified_name.clone(),
                    })
                })
                .collect();
            let by_kind: Vec<String> = {
                let mut kind_counts: std::collections::HashMap<String, usize> =
                    std::collections::HashMap::new();
                for sym in &context.found_symbols {
                    *kind_counts
                        .entry(sym.kind.debug_label().to_string())
                        .or_insert(0) += 1;
                }
                kind_counts
                    .iter()
                    .map(|(k, v)| format!("  - {}: {}", k, v))
                    .collect()
            };
            (
                format!(
                    "Found {} matching symbols across the repository:\n{}\n\n{} total files, {} total lines of code.",
                    context.found_symbols.len(),
                    by_kind.join("\n"),
                    repo.repo_map.file_count(),
                    repo.repo_map.total_loc()
                ),
                refs,
            )
        };

        // Build confidence based on reference count and context match quality;
        // boost confidence when intent fields match what was found.
        let mut confidence = if context.found_symbols.is_empty() {
            0.3
        } else if context.found_symbols.len() <= 3 {
            0.85
        } else {
            (context.found_symbols.len() as f64 / 15.0 + 0.5).min(0.95)
        };
        // Wire: use intent flags to adjust confidence
        if context.has_file_intent && context.file_count > 0 {
            confidence = (confidence + 0.1).min(0.99);
        }
        if context.has_symbol_intent && !context.found_symbols.is_empty() {
            confidence = (confidence + 0.1).min(0.99);
        }
        if context.has_lang_intent {
            confidence = (confidence + 0.05).min(0.99);
        }
        if context.has_loc_intent {
            // Size/location questions benefit from more context
            confidence = (confidence + 0.03).min(0.99);
        }

        Ok(Answer {
            text,
            references,
            confidence,
            coverage: AnswerCoverage::FullRepo,
        })
    }

    /// Build retrieval-augmented context from repo structures.
    fn build_rag_context(&self, question: &str, repo: &RepoContext) -> RagContext {
        let lower = question.to_lowercase();
        let question_words: std::collections::HashSet<&str> = lower
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .collect();

        let mut found_symbols = Vec::new();
        let mut matched_files: std::collections::HashSet<usize> = std::collections::HashSet::new();

        // Match type index entries by word boundary
        for entry in &repo.type_index.entries {
            let name_lower = entry.name.to_lowercase();
            if question_words.contains(name_lower.as_str()) {
                let files: Vec<String> = repo
                    .repo_map
                    .files
                    .get(entry.file_id)
                    .cloned()
                    .into_iter()
                    .collect();
                found_symbols.push(SymbolMatch {
                    name: entry.name.clone(),
                    qualified_name: entry.qualified_name.clone(),
                    kind: entry.kind.clone(),
                    line: entry.line,
                    files,
                });
                matched_files.insert(entry.file_id);
            }
        }

        // Also match file names in repo_map
        for (file_id, path) in repo.repo_map.files.iter().enumerate() {
            let file_stem = std::path::Path::new(path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();
            if question_words.contains(file_stem.as_str()) {
                matched_files.insert(file_id);
            }
        }

        // Check intent keywords
        let has_file_intent = question_words.contains("files")
            || question_words.contains("how")
            || question_words.contains("many")
            || question_words.contains("count");
        let has_symbol_intent = question_words.contains("symbols")
            || question_words.contains("types")
            || question_words.contains("definitions")
            || question_words.contains("functions");
        let has_lang_intent =
            question_words.contains("language") || question_words.contains("programming");
        let has_loc_intent = question_words.contains("lines")
            || question_words.contains("loc")
            || question_words.contains("size");

        RagContext {
            found_symbols,
            file_count: matched_files.len(),
            has_file_intent,
            has_symbol_intent,
            has_lang_intent,
            has_loc_intent,
        }
    }

    // ── Cache management ────────────────────────────────────────────────

    /// Clear the in-memory clone cache.
    pub async fn clear_cache(&self) {
        let mut cache = self.cache.lock().await;
        cache.clear();
        info!("RepoAnalyzer::clear_cache: cache cleared");
    }

    /// Remove a specific entry from the cache.
    pub async fn remove_from_cache(&self, url: &str) {
        let mut cache = self.cache.lock().await;
        cache.remove(url);
    }

    /// Check if a string starts with the `repo:` prefix.
    pub fn has_repo_prefix(input: &str) -> bool {
        input.trim().starts_with(REPO_PREFIX)
    }

    /// Extract the repo URL and the actual question from prefixed input.
    ///
    /// Returns `(repo_url, question)` if the prefix is present.
    pub fn parse_repo_input(input: &str) -> Option<(String, String)> {
        let trimmed = input.trim();
        if let Some(rest) = trimmed.strip_prefix(REPO_PREFIX) {
            let rest = rest.trim();
            // Format: "repo:https://github.com/org/repo what is the main struct?"
            // Split on first space to get URL and question.
            if let Some((url, question)) = rest.split_once(char::is_whitespace) {
                return Some((url.to_owned(), question.trim().to_owned()));
            }
            // If only URL is provided, question is empty.
            return Some((rest.to_owned(), String::new()));
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Dependency edge detection
// ---------------------------------------------------------------------------

/// Resolve import statements between same-repo files into dependency edges.
///
/// Each file's content is parsed for imports (Rust `use`, Python `import` /
/// `from ... import`, JS/TS `import ... from` / `require`). Targets that
/// resolve to another file in the repository produce a forward
/// `dependencies` edge and a reverse `dependents` edge; targets that cannot
/// be mapped to any repo file (external crates, npm packages, stdlib, ...)
/// are skipped and are not treated as errors.
fn resolve_dependencies(repo_map: &mut RepoMap, contents: &[Option<String>]) {
    let lookup = build_dependency_lookup(repo_map);
    let files: Vec<(usize, String)> = repo_map
        .files
        .iter()
        .enumerate()
        .map(|(id, path)| (id, path.clone()))
        .collect();
    for (file_id, rel_path) in files {
        let Some(Some(content)) = contents.get(file_id) else {
            continue; // binary / unreadable file — nothing to parse
        };
        for target in parse_imports(&rel_path, content) {
            if let Some(dep_id) = resolve_dependency(&rel_path, &target, &lookup) {
                if dep_id == file_id {
                    continue; // self-import (e.g. `from . import x`)
                }
                repo_map
                    .dependencies
                    .entry(file_id)
                    .or_default()
                    .insert(dep_id);
                repo_map
                    .dependents
                    .entry(dep_id)
                    .or_default()
                    .insert(file_id);
            }
        }
    }
}

/// Build a lookup of module-path keys -> file IDs used to resolve import
/// targets. Every file is registered under several canonical forms: its raw
/// relative path, its extension-less path, its module-index directory form
/// (e.g. `src/util/mod.rs` -> `src/util`), a Rust `::`-joined form, and
/// crate-root-relative variants with a leading `src/` stripped, so that
/// `use crate::...` and top-level module paths resolve.
fn build_dependency_lookup(repo_map: &RepoMap) -> HashMap<String, usize> {
    let mut lookup: HashMap<String, usize> = HashMap::new();
    for (file_id, rel) in repo_map.files.iter().enumerate() {
        for key in module_path_keys(rel) {
            lookup.entry(key).or_insert(file_id);
        }
    }
    lookup
}

/// All module-path keys a file is addressable under.
fn module_path_keys(rel: &str) -> Vec<String> {
    let no_ext = strip_module_extension(rel);
    let mut keys = vec![rel.to_string(), no_ext.clone()];
    if let Some(dir) = index_module_dir(rel) {
        if !dir.is_empty() {
            keys.push(dir);
        }
    }
    keys.push(no_ext.replace('/', "::"));
    if let Some(stripped) = no_ext.strip_prefix("src/") {
        keys.push(stripped.to_string());
        keys.push(stripped.replace('/', "::"));
    }
    keys
}

/// Strip the file extension from a relative path (e.g. `src/util/helper.rs`
/// -> `src/util/helper`).
fn strip_module_extension(rel: &str) -> String {
    Path::new(rel)
        .with_extension("")
        .to_string_lossy()
        .to_string()
}

/// If `rel` names a module index file (`mod.rs`, `__init__.py`, `index.*`),
/// return the directory it represents; otherwise `None`.
fn index_module_dir(rel: &str) -> Option<String> {
    let path = Path::new(rel);
    let file_name = path.file_name()?.to_str()?;
    let is_index = file_name == "mod.rs"
        || file_name == "__init__.py"
        || matches!(
            file_name,
            "index.js" | "index.jsx" | "index.ts" | "index.tsx" | "index.mjs" | "index.cjs"
        );
    if is_index {
        path.parent().map(|p| p.to_string_lossy().to_string())
    } else {
        None
    }
}

/// Parse import statements from `content`, returning normalized module
/// targets.
///
/// Targets keep their relative (`./`, `../`) or absolute style; Rust
/// `crate::` / `self::` / `super::` prefixes are normalized into the
/// corresponding absolute or relative form. Supported languages:
///
/// - Rust: `use crate::a::b;` / `use self::a;` / `use super::a;` / `use a::b;`
/// - Python: `import a.b` / `from a.b import c` / `from .a import b`
/// - JS/TS: `import ... from 'x'` / `import 'x'` / `require('x')`
fn parse_imports(rel_path: &str, content: &str) -> Vec<String> {
    let ext = Path::new(rel_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let mut targets = Vec::new();
    match ext {
        "rs" => {
            for line in content.lines() {
                let path = line
                    .trim()
                    .strip_prefix("use ")
                    .and_then(|rest| rest.split(';').next())
                    .map(str::trim);
                if let Some(path) = path.filter(|p| !p.is_empty()) {
                    if let Some(target) = normalize_rust_use_path(path) {
                        targets.push(target);
                    }
                }
            }
        }
        "py" => {
            for line in content.lines() {
                let trimmed = line.trim();
                if let Some(rest) = trimmed.strip_prefix("import ") {
                    let module = rest.split_whitespace().next().unwrap_or("");
                    let module = module.trim_end_matches(',');
                    if !module.is_empty() {
                        targets.push(module.replace('.', "/"));
                    }
                } else if let Some(rest) = trimmed.strip_prefix("from ") {
                    let (module_part, names_part) =
                        rest.split_once(" import ").unwrap_or((rest, ""));
                    let (dots, dotted_path) = split_leading_dots(module_part.trim());
                    if dotted_path.is_empty() {
                        // `from . import x` — package self-import, no edge.
                        continue;
                    }
                    let module = dotted_path.replace('.', "/");
                    let prefix = if dots > 0 {
                        format!("./{}", "../".repeat(dots.saturating_sub(2)))
                    } else {
                        String::new()
                    };
                    targets.push(format!("{prefix}{module}"));
                    if let Some(first_name) = names_part.trim().split(',').next() {
                        let name = first_name.split_whitespace().next().unwrap_or("");
                        if !name.is_empty() && !name.starts_with('(') {
                            // `from a.b import c` may also bind submodule `a.b.c`.
                            targets.push(format!("{prefix}{module}/{name}"));
                        }
                    }
                }
            }
        }
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => {
            for line in content.lines() {
                if let Some(spec) = extract_js_specifier(line) {
                    let spec = spec.trim().trim_end_matches(';');
                    if !spec.is_empty() {
                        targets.push(spec.to_string());
                    }
                }
            }
        }
        _ => {}
    }
    targets
}

/// Normalize a Rust `use` path (the part after the `use` keyword, before the
/// terminating `;`) into a module target. `crate::` and `self::` resolve to
/// the crate root / importer directory; `super::` segments map to `../`
/// prefixes; `use foo::{a, b};` groups keep their base module path.
fn normalize_rust_use_path(path: &str) -> Option<String> {
    let path = path.split(" as ").next().unwrap_or(path);
    let path = path.split("::{").next().unwrap_or(path).trim();
    let path = path.trim_end_matches("::*").trim();
    if path.is_empty() {
        return None;
    }
    if let Some(rest) = path.strip_prefix("crate::") {
        return (!rest.is_empty()).then(|| rest.replace("::", "/"));
    }
    if let Some(rest) = path.strip_prefix("self::") {
        return Some(format!("./{}", rest.replace("::", "/")));
    }
    let mut ups = 0;
    let mut remainder = path;
    while let Some(rest) = remainder.strip_prefix("super::") {
        ups += 1;
        remainder = rest;
    }
    if ups > 0 {
        Some(format!(
            "{}{}",
            "../".repeat(ups),
            remainder.replace("::", "/")
        ))
    } else {
        Some(path.replace("::", "/"))
    }
}

/// Split a Python module name into a leading-dot count and the remaining
/// dotted path (e.g. `..foo.bar` -> `(2, "foo.bar")`).
fn split_leading_dots(module: &str) -> (usize, &str) {
    let dots = module.bytes().take_while(|b| *b == b'.').count();
    (dots, &module[dots..])
}

/// Extract the module specifier from a JS/TS import/require line: the first
/// quoted string after `from`, after `require(`, or in a bare `import 'x'`.
fn extract_js_specifier(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let is_import_statement =
        trimmed.contains("from") || trimmed.contains("require(") || trimmed.starts_with("import ");
    if !is_import_statement {
        return None;
    }
    for quote in ['\'', '"'] {
        if let Some(start) = line.find(quote) {
            if let Some(rel_end) = line[start + 1..].find(quote) {
                return Some(line[start + 1..start + 1 + rel_end].to_string());
            }
        }
    }
    None
}

/// Resolve a normalized import target to a file ID via `lookup`, or `None`
/// when the target does not map to any file inside the repository.
///
/// Relative targets (`./`, `../`) are resolved against the importing file's
/// directory; absolute targets are tried as-is and relative to a
/// conventional `src/` crate root.
fn resolve_dependency(
    importer_rel: &str,
    target: &str,
    lookup: &HashMap<String, usize>,
) -> Option<usize> {
    let base = if target.starts_with("./") || target.starts_with("../") {
        let parent = Path::new(importer_rel)
            .parent()
            .unwrap_or_else(|| Path::new(""));
        normalize_slashes(&parent.join(target).to_string_lossy())
    } else {
        normalize_slashes(target)
    };
    let base = base.trim_end_matches('/').to_string();
    if base.is_empty() {
        return None;
    }

    let mut keys = vec![base.clone()];
    for ext in IMPORT_SCAN_EXTENSIONS {
        keys.push(format!("{base}.{ext}"));
    }
    for index in MODULE_INDEX_FILES {
        keys.push(format!("{base}/{index}"));
    }
    keys.push(base.replace('/', "::"));
    if !target.starts_with("./") && !target.starts_with("../") {
        keys.push(format!("src/{base}"));
        keys.push(format!("src/{}", base.replace('/', "::")));
    }
    keys.into_iter().find_map(|key| lookup.get(&key).copied())
}

/// Normalize a slash-separated relative path, resolving `.` and `..`
/// segments (best effort — `..` above the repo root is dropped).
fn normalize_slashes(path: &str) -> String {
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    segments.join("/")
}

impl SymbolKind {
    /// Human-readable label suitable for debugging / display.
    pub fn debug_label(&self) -> &'static str {
        match self {
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Trait => "trait",
            Self::Function => "function",
            Self::Method => "method",
            Self::TypeAlias => "type_alias",
            Self::Constant => "constant",
            Self::Module => "module",
            Self::Macro => "macro",
            Self::Interface => "interface",
            Self::Class => "class",
            Self::Variable => "variable",
            Self::Other(_) => "other",
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_repo_prefix_detection() {
        assert!(RepoAnalyzer::has_repo_prefix("repo:https://github.com/a/b"));
        assert!(RepoAnalyzer::has_repo_prefix(
            "  repo:https://github.com/a/b  "
        ));
        assert!(!RepoAnalyzer::has_repo_prefix("https://github.com/a/b"));
    }

    #[test]
    fn test_parse_repo_input_url_only() {
        let result = RepoAnalyzer::parse_repo_input("repo:https://github.com/org/repo");
        assert!(result.is_some());
        let (url, question) = result.expect("parse_repo_input should return Some");
        assert_eq!(url, "https://github.com/org/repo");
        assert!(question.is_empty());
    }

    #[test]
    fn test_parse_repo_input_with_question() {
        let result =
            RepoAnalyzer::parse_repo_input("repo:https://github.com/org/repo what is the API?");
        assert!(result.is_some());
        let (url, question) = result.expect("parse_repo_input should return Some");
        assert_eq!(url, "https://github.com/org/repo");
        assert_eq!(question, "what is the API?");
    }

    #[test]
    fn test_parse_repo_input_no_prefix() {
        assert!(RepoAnalyzer::parse_repo_input("hello world").is_none());
    }

    #[test]
    fn test_type_index_add_and_get() {
        let mut idx = TypeIndex::empty();
        idx.add(TypeEntry {
            qualified_name: "crate::MyStruct".into(),
            name: "MyStruct".into(),
            kind: SymbolKind::Struct,
            file_id: 0,
            line: 10,
            column: 1,
            doc_summary: None,
            visibility: Some("pub".into()),
        });
        assert_eq!(idx.len(), 1);
        assert!(idx.get("crate::MyStruct").is_some());
        assert!(idx.get("nonexistent").is_none());
    }

    #[test]
    fn test_extract_definition() {
        let analyzer = RepoAnalyzer::default();
        assert_eq!(
            analyzer.extract_definition("pub struct Foo {").as_deref(),
            Some("Foo")
        );
        assert_eq!(
            analyzer.extract_definition("fn bar() -> bool").as_deref(),
            Some("bar")
        );
        assert_eq!(
            analyzer
                .extract_definition("pub(crate) enum Color {")
                .as_deref(),
            Some("Color")
        );
        assert_eq!(analyzer.extract_definition("let x = 5;"), None);
    }

    #[test]
    fn test_infer_kind() {
        let analyzer = RepoAnalyzer::default();
        assert_eq!(analyzer.infer_kind("struct Foo"), SymbolKind::Struct);
        assert_eq!(analyzer.infer_kind("enum Bar"), SymbolKind::Enum);
        assert_eq!(analyzer.infer_kind("trait Baz"), SymbolKind::Trait);
        assert_eq!(analyzer.infer_kind("fn qux"), SymbolKind::Function);
    }

    #[tokio::test]
    async fn test_clone_local_path_nonexistent() {
        let analyzer = RepoAnalyzer::default();
        let result = analyzer.clone("/nonexistent/path").await;
        assert!(matches!(result, Err(RepoAnalyzerError::PathNotFound(_))));
    }

    /// Dependency edges must be populated for same-repo Rust imports: a file
    /// that `use crate::util::helper;` depends on `src/util/helper.rs`, and
    /// the reverse `dependents` edge must exist. External `std` imports must
    /// not create edges.
    #[tokio::test]
    async fn test_build_repo_map_resolves_rust_dependency_edges() {
        let tmp = std::env::temp_dir().join("go-on-repo-map-rust");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("src/util")).unwrap();
        std::fs::write(
            tmp.join("src/lib.rs"),
            "pub mod util;\nuse crate::util::helper;\nuse std::collections::HashMap;\n",
        )
        .unwrap();
        std::fs::write(tmp.join("src/util/mod.rs"), "pub mod helper;\n").unwrap();
        std::fs::write(
            tmp.join("src/util/helper.rs"),
            "pub fn helper() -> u32 { 42 }\n",
        )
        .unwrap();

        let analyzer = RepoAnalyzer::default();
        let map = analyzer.build_repo_map(&tmp).await.unwrap();
        let lib = map
            .files
            .iter()
            .position(|f| f == "src/lib.rs")
            .expect("lib.rs indexed");
        let helper = map
            .files
            .iter()
            .position(|f| f == "src/util/helper.rs")
            .expect("helper.rs indexed");

        // `use crate::util::helper;` must produce a dependency edge.
        assert!(
            map.dependencies[&lib].contains(&helper),
            "lib.rs should depend on src/util/helper.rs, got {:?}",
            map.dependencies[&lib]
        );
        // Reverse edge in dependents.
        assert!(
            map.dependents[&helper].contains(&lib),
            "src/util/helper.rs should list lib.rs as dependent, got {:?}",
            map.dependents[&helper]
        );
        // The external stdlib import must not create an edge.
        assert_eq!(
            map.dependencies[&lib].len(),
            1,
            "only the same-repo edge should be recorded, got {:?}",
            map.dependencies[&lib]
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Dependency edges must be populated for Python and JS/TS imports too:
    /// `import pkg.b` / `from pkg import b`, relative `./util/helper.js`
    /// imports, and `require('../pkg/b')` walking up the directory tree.
    #[tokio::test]
    async fn test_build_repo_map_resolves_python_and_js_dependency_edges() {
        let tmp = std::env::temp_dir().join("go-on-repo-map-py-js");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("pkg")).unwrap();
        std::fs::create_dir_all(tmp.join("web/util")).unwrap();
        std::fs::write(tmp.join("pkg/__init__.py"), "").unwrap();
        std::fs::write(tmp.join("pkg/a.py"), "import pkg.b\nfrom pkg import b\n").unwrap();
        std::fs::write(tmp.join("pkg/b.py"), "VALUE = 1\n").unwrap();
        std::fs::write(
            tmp.join("web/app.js"),
            "import { foo } from './util/helper.js';\nconst other = require('../pkg/b');\n",
        )
        .unwrap();
        std::fs::write(tmp.join("web/util/helper.js"), "export const foo = 1;\n").unwrap();

        let analyzer = RepoAnalyzer::default();
        let map = analyzer.build_repo_map(&tmp).await.unwrap();
        let a = map
            .files
            .iter()
            .position(|f| f == "pkg/a.py")
            .expect("pkg/a.py indexed");
        let b = map
            .files
            .iter()
            .position(|f| f == "pkg/b.py")
            .expect("pkg/b.py indexed");
        let app = map
            .files
            .iter()
            .position(|f| f == "web/app.js")
            .expect("web/app.js indexed");
        let helper = map
            .files
            .iter()
            .position(|f| f == "web/util/helper.js")
            .expect("web/util/helper.js indexed");

        // Python: `import pkg.b` / `from pkg import b` both resolve to pkg/b.py.
        assert!(
            map.dependencies[&a].contains(&b),
            "pkg/a.py should depend on pkg/b.py, got {:?}",
            map.dependencies[&a]
        );
        // JS: relative import resolves within the importer's directory tree.
        assert!(
            map.dependencies[&app].contains(&helper),
            "web/app.js should depend on web/util/helper.js, got {:?}",
            map.dependencies[&app]
        );
        // JS: `require('../pkg/b')` walks up to the repo root.
        assert!(
            map.dependencies[&app].contains(&b),
            "web/app.js should depend on pkg/b.py via require('../pkg/b'), got {:?}",
            map.dependencies[&app]
        );
        // Reverse edges are populated.
        assert!(
            map.dependents[&helper].contains(&app),
            "web/util/helper.js should list web/app.js as dependent, got {:?}",
            map.dependents[&helper]
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
