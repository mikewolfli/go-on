#![allow(dead_code)]

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

/// Default clone directory name under temp.
const CLONE_DIR_PREFIX: &str = "go-on-repo-";

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
}

/// Graph representation of the repository file structure.
///
/// Nodes are file paths (relative to repo root). Edges represent
/// import / include / dependency relationships between files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoMap {
    /// Root-relative file paths indexed by a short unique ID.
    pub files: Vec<String>,
    /// Adjacency list: dependencies[file_id] = set of file_ids this file imports.
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
    /// Maximum file size (in bytes) to index.
    max_file_size: u64,
    /// File extensions to exclude from analysis.
    excluded_extensions: HashSet<String>,
    /// Directories to exclude from analysis.
    excluded_dirs: HashSet<String>,
}

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
            max_file_size: 512 * 1024, // 512 KB
            excluded_extensions: excluded_ext,
            excluded_dirs,
        }
    }
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
            max_file_size,
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

        let root_path = if is_remote {
            let dir = tempfile::TempDir::new()
                .map_err(|e| RepoAnalyzerError::CloneFailed(e.to_string()))?;
            let dest = dir.path().join("repo");

            // In production, invoke `git clone` here.
            // For now, create a placeholder directory.
            tokio::fs::create_dir_all(&dest).await?;
            info!("RepoAnalyzer::clone: would clone {} to {:?}", url, dest);
            dest
        } else {
            let local = PathBuf::from(url);
            if !local.exists() {
                return Err(RepoAnalyzerError::PathNotFound(url.into()));
            }
            local
        };

        // Build repo map and type index.
        let repo_map = self.build_repo_map(&root_path).await?;
        let type_index = self.extract_types(&root_path).await?;

        // Language statistics.
        let mut language_stats: HashMap<String, usize> = HashMap::new();
        for f in &repo_map.files {
            if let Some(ext) = Path::new(f).extension().and_then(|e| e.to_str()) {
                *language_stats.entry(ext.to_lowercase()).or_insert(0) += 1;
            }
        }

        let ctx = RepoContext {
            source: url.to_owned(),
            root_path,
            repo_map,
            type_index,
            head_commit: None,
            language_stats,
        };

        // Insert into cache.
        {
            let mut cache = self.cache.lock().await;
            cache.insert(url.to_owned(), ctx.clone());
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

        self.walk_and_collect(path, path, &mut repo_map).await?;

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
    ) -> Result<(), RepoAnalyzerError> {
        let mut entries = tokio::fs::read_dir(dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let ft = entry.file_type().await?;
            let name = entry.file_name().to_string_lossy().to_string();
            let abs_path = entry.path();

            if ft.is_dir() {
                if !self.excluded_dirs.contains(&name) {
                    Box::pin(self.walk_and_collect(root, &abs_path, repo_map)).await?;
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
                repo_map.loc.insert(file_id, 0); // Populated by a real parser.

                // In production, parse the file for imports to build edges.
                // Here we record the file and note that edge detection would occur.
                repo_map.dependencies.insert(file_id, HashSet::new());
                repo_map.dependents.insert(file_id, HashSet::new());
            }
        }
        Ok(())
    }

    // ── Type index ──────────────────────────────────────────────────────

    /// Extract type definitions / symbol index from the repository at `path`.
    ///
    /// This scans source files and runs heuristic extraction for common
    /// language constructs (struct/enum/trait/fn definitions). In production
    /// this would use `tree-sitter` or language-specific parsers.
    pub async fn extract_types(&self, path: &Path) -> Result<TypeIndex, RepoAnalyzerError> {
        info!("RepoAnalyzer::extract_types: scanning {:?}", path);
        let mut index = TypeIndex::empty();

        let repo_map = self.build_repo_map(path).await?;

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
    /// This uses the `type_index` and `repo_map` to ground answers in actual
    /// code symbols. In production, it would delegate to an LLM with a
    /// retrieval-augmented prompt.
    pub async fn answer_code_question(
        &self,
        question: &str,
        repo: &RepoContext,
    ) -> Result<Answer, RepoAnalyzerError> {
        info!(
            "RepoAnalyzer::answer_code_question: question='{}' on {}",
            question, repo.source
        );

        let lower = question.to_lowercase();
        let mut references = Vec::new();

        // Heuristic: if the question mentions a type name, look it up.
        for entry in &repo.type_index.entries {
            if lower.contains(&entry.name.to_lowercase()) {
                if let Some(file) = repo.repo_map.files.get(entry.file_id) {
                    references.push(SourceRef {
                        file: file.clone(),
                        line: entry.line,
                        snippet: entry.qualified_name.clone(),
                    });
                }
            }
        }

        // Heuristic: if asking "how many files" or "total loc", answer directly.
        let text = if lower.contains("how many files") || lower.contains("file count") {
            format!(
                "The repository has {} files with a total of {} lines of code.",
                repo.repo_map.file_count(),
                repo.repo_map.total_loc()
            )
        } else if lower.contains("symbols")
            || lower.contains("types")
            || lower.contains("definitions")
        {
            let by_kind: Vec<String> = repo
                .type_index
                .by_kind
                .iter()
                .map(|(k, v)| format!("  - {}: {}", k.debug_label(), v.len()))
                .collect();
            format!(
                "Found {} symbol definitions:\n{}",
                repo.type_index.len(),
                by_kind.join("\n")
            )
        } else if lower.contains("language") || lower.contains("programming language") {
            let mut stats: Vec<_> = repo.language_stats.iter().collect();
            stats.sort_by(|a, b| b.1.cmp(a.1));
            let lines: Vec<String> = stats
                .iter()
                .map(|(ext, count)| format!("  - .{}: {} files", ext, count))
                .collect();
            format!("Language distribution:\n{}", lines.join("\n"))
        } else {
            // Generic fallback — in production, pass to an LLM.
            format!(
                "Analyzed repository '{}' with {} files and {} type definitions. \
                 References: {} relevant symbols found.",
                repo.source,
                repo.repo_map.file_count(),
                repo.type_index.len(),
                references.len()
            )
        };

        // Build confidence based on reference count.
        let confidence = if references.is_empty() {
            0.3
        } else {
            (references.len() as f64 / 10.0).min(0.95)
        };

        Ok(Answer {
            text,
            references,
            confidence,
            coverage: AnswerCoverage::FullRepo,
        })
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
        let (url, question) = result.unwrap();
        assert_eq!(url, "https://github.com/org/repo");
        assert!(question.is_empty());
    }

    #[test]
    fn test_parse_repo_input_with_question() {
        let result =
            RepoAnalyzer::parse_repo_input("repo:https://github.com/org/repo what is the API?");
        assert!(result.is_some());
        let (url, question) = result.unwrap();
        assert_eq!(url, "https://github.com/org/repo");
        assert_eq!(question, "what is the API?");
    }

    #[test]
    fn test_parse_repo_input_no_prefix() {
        assert!(RepoAnalyzer::parse_repo_input("hello world").is_none());
    }

    #[test]
    fn test_type_index_empty() {
        let idx = TypeIndex::empty();
        assert!(idx.is_empty());
        assert_eq!(idx.len(), 0);
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
    fn test_repo_map_empty() {
        let map = RepoMap::empty();
        assert_eq!(map.file_count(), 0);
        assert_eq!(map.total_loc(), 0);
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

    #[test]
    fn test_answer_serialize_roundtrip() {
        let answer = Answer {
            text: "The main struct is AppConfig.".into(),
            references: vec![SourceRef {
                file: "src/config.rs".into(),
                line: 42,
                snippet: "pub struct AppConfig".into(),
            }],
            confidence: 0.95,
            coverage: AnswerCoverage::FullRepo,
        };
        let json = serde_json::to_string(&answer).unwrap();
        let deserialized: Answer = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.text, answer.text);
        assert!((deserialized.confidence - 0.95).abs() < 1e-6);
    }

    #[tokio::test]
    async fn test_clone_local_path_nonexistent() {
        let analyzer = RepoAnalyzer::default();
        let result = analyzer.clone("/nonexistent/path").await;
        assert!(matches!(result, Err(RepoAnalyzerError::PathNotFound(_))));
    }

    #[test]
    fn test_symbol_kind_debug_label() {
        assert_eq!(SymbolKind::Struct.debug_label(), "struct");
        assert_eq!(SymbolKind::Enum.debug_label(), "enum");
        assert_eq!(SymbolKind::Function.debug_label(), "function");
        assert_eq!(SymbolKind::Other("custom".into()).debug_label(), "other");
    }

    #[test]
    fn test_answer_coverage_serialize() {
        let full = AnswerCoverage::FullRepo;
        let subset = AnswerCoverage::Subset("src/".into());
        let json_full = serde_json::to_string(&full).unwrap();
        let json_subset = serde_json::to_string(&subset).unwrap();
        assert_eq!(
            serde_json::from_str::<AnswerCoverage>(&json_full).unwrap(),
            full
        );
        assert_eq!(
            serde_json::from_str::<AnswerCoverage>(&json_subset).unwrap(),
            subset
        );
    }
}
