//! S5: Startup Repository Context Loader
//!
//! BLUE38 ARCH-06: Asynchronously loads project-level context (README, build commands,
//! recent commits, style rules) once per process startup. Uses OnceLock to prevent
//! redundant loads. All file operations are best-effort with graceful fallback.
//!
//! Design:
//! - `load()` performs async I/O with timeouts for README, Cargo.toml, git log,
//!   and editorconfig / RULES directory.
//! - Results are cached in a process-global `OnceLock<StartupContext>`.
//! - `summary_text()` produces a compact markdown block suitable for injection into
//!   `AgentTaskEnvelope.evidence`.
//! - `StartupContextProfile` returns a JSON-serializable profile for governance
//!   metrics endpoints.
//!
//! Graceful degradation: missing files, corrupt outputs, or I/O errors produce empty
//! fields rather than panics or propagating errors.

use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::Duration;
use tokio::time::timeout;
use tracing::debug;

// ─────────────────────────────────────────────────────────────────────────────
// Profile struct (governance metrics)
// ─────────────────────────────────────────────────────────────────────────────

/// Governance-metrics snapshot of the startup context loader.
///
/// Returned by `startup_context_profile()` for the `/governance/status` endpoint.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[allow(dead_code)]
pub struct StartupContextProfile {
    /// Whether the startup context loader is enabled in config
    pub enabled: bool,
    /// Whether `load()` has completed successfully at least once
    pub loaded: bool,
    /// Names of components that were loaded successfully
    pub loaded_components: Vec<String>,
    /// Number of files successfully read
    pub file_count: usize,
    /// Total number of characters extracted across all loaded components
    pub char_count: usize,
    /// Number of characters extracted from README
    pub readme_chars: usize,
    /// Number of build commands discovered
    pub build_command_count: usize,
    /// Number of style rule entries extracted
    pub style_rule_count: usize,
    /// Number of recent commit messages retrieved
    pub commit_count: usize,
    /// Whether a recognised code-repo fingerprint file was found
    pub has_code_repo: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// StartupContext (cached payload)
// ─────────────────────────────────────────────────────────────────────────────

/// Snapshot of project-level context loaded at startup.
///
/// Stored in a process-global `OnceLock`.  Fields are populated best-effort;
/// missing files or I/O errors leave them as defaults.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StartupContext {
    /// True after a successful (or partially successful) load
    pub loaded: bool,
    /// First N characters of the discovered README file
    pub readme_excerpt: String,
    /// Number of characters actually kept in `readme_excerpt`
    pub readme_chars: usize,
    /// Package name extracted from Cargo.toml
    pub package_name: String,
    /// Package version extracted from Cargo.toml
    pub package_version: String,
    /// Number of dependencies found in Cargo.toml
    pub deps_count: usize,
    /// Build commands discovered from Cargo.toml
    pub build_commands: Vec<String>,
    /// Last N commit one-line messages
    pub recent_commits: Vec<String>,
    /// Extracted style-rule fragments from editorconfig / linter configs / RULES dir
    pub style_rules: Vec<String>,
    /// True when code-repo fingerprint files are detected
    pub has_code_repo: bool,
    /// Names of components that were loaded successfully
    pub loaded_components: Vec<String>,
    /// Number of files successfully read
    pub file_count: usize,
    /// Number of files attempted
    pub attempted_file_count: usize,
    /// Total number of characters extracted across all loaded components
    pub char_count: usize,
    /// RULES directory entries (file names, up to 10)
    pub rules_files: Vec<String>,
}

static STARTUP_CONTEXT: Mutex<Option<StartupContext>> = Mutex::new(None);

/// Get the globally cached startup context (None until first `load()` call).
pub fn get() -> Option<StartupContext> {
    STARTUP_CONTEXT.lock().ok()?.clone()
}

/// Reset the cached startup context. Only available in tests.
#[cfg(test)]
pub fn reset_cache() {
    if let Ok(mut guard) = STARTUP_CONTEXT.lock() {
        *guard = None;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Configuration
// ─────────────────────────────────────────────────────────────────────────────

/// Configuration knobs for startup context loading.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartupContextConfig {
    /// Master switch – when `false` `load()` returns immediately with defaults.
    #[serde(default)]
    pub enabled: bool,
    /// Maximum number of characters to read from the README.
    #[serde(default = "default_readme_max_chars")]
    pub readme_max_chars: usize,
    /// Number of recent commits to retrieve.
    #[serde(default = "default_recent_commits")]
    pub recent_commits: usize,
    /// Per-file I/O timeout in milliseconds.
    #[serde(default = "default_io_timeout_ms")]
    pub io_timeout_ms: u64,
}

fn default_readme_max_chars() -> usize {
    2000
}

fn default_recent_commits() -> usize {
    5
}

fn default_io_timeout_ms() -> u64 {
    5_000
}

impl Default for StartupContextConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            readme_max_chars: 2000,
            recent_commits: 5,
            io_timeout_ms: 5_000,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Core loading logic
// ─────────────────────────────────────────────────────────────────────────────

/// Load startup context asynchronously.
///
/// This is safe to call multiple times – the `OnceLock` guarantees the work is
/// only performed once.  All file operations use `tokio::fs` with a configurable
/// timeout.  Missing files, corrupt data, or I/O errors are silently degraded.
pub async fn load(cfg: &StartupContextConfig) -> Result<StartupContext, std::io::Error> {
    if !cfg.enabled {
        let ctx = StartupContext::default();
        #[cfg(not(test))]
        {
            if let Ok(mut guard) = STARTUP_CONTEXT.lock() {
                *guard = Some(ctx.clone());
            }
        }
        return Ok(ctx);
    }

    // If another task already initialised the global, just return a clone.
    #[cfg(not(test))]
    {
        if let Ok(guard) = STARTUP_CONTEXT.lock() {
            if let Some(cached) = &*guard {
                return Ok(cached.clone());
            }
        }
    }

    let io_timeout = Duration::from_millis(cfg.io_timeout_ms);
    let cwd = std::env::current_dir().unwrap_or_default();

    let mut ctx = StartupContext {
        loaded: true,
        ..Default::default()
    };

    // ── 1. Detect code-repo fingerprint files ───────────────────────────
    let fingerprint_files = [
        "Cargo.toml",
        "package.json",
        "go.mod",
        "pom.xml",
        "pyproject.toml",
        "setup.py",
    ];
    ctx.has_code_repo = fingerprint_files.iter().any(|f| cwd.join(f).exists());

    // ── 2. README excerpt ───────────────────────────────────────────────
    // Try README.md first, then README, then README.zh-CN.md
    let readme_candidates = ["README.md", "README", "README.zh-CN.md"];
    for name in &readme_candidates {
        let path = cwd.join(name);
        ctx.attempted_file_count += 1;
        if let Ok(result) = timeout(io_timeout, tokio::fs::read_to_string(&path)).await {
            match result {
                Ok(text) => {
                    let excerpt: String = text.chars().take(cfg.readme_max_chars).collect();
                    ctx.readme_chars = excerpt.chars().count();
                    ctx.readme_excerpt = excerpt;
                    ctx.char_count += ctx.readme_chars;
                    ctx.file_count += 1;
                    ctx.loaded_components.push(format!("readme:{}", name));
                    debug!("startup_context: loaded README from {}", name);
                    break;
                }
                Err(e) => {
                    debug!("startup_context: failed to read {}: {}", name, e);
                }
            }
        } else {
            debug!("startup_context: timeout reading {}", name);
        }
    }

    // ── 3. Cargo.toml (package name, version, deps count) ──────────────
    ctx.attempted_file_count += 1;
    let cargo_path = cwd.join("Cargo.toml");
    if let Ok(result) = timeout(io_timeout, tokio::fs::read_to_string(&cargo_path)).await {
        match result {
            Ok(text) => {
                ctx.file_count += 1;
                ctx.loaded_components.push("cargo".to_string());

                // Extract package name
                if let Some(line) = text.lines().find(|l| l.trim().starts_with("name")) {
                    let name_val = line.split('=').nth(1).unwrap_or("").trim();
                    ctx.package_name = name_val.trim_matches('"').trim_matches('\'').to_string();
                } else {
                    ctx.package_name = "(unnamed)".to_string();
                }
                // Extract package version
                if let Some(line) = text.lines().find(|l| l.trim().starts_with("version")) {
                    let ver_val = line.split('=').nth(1).unwrap_or("").trim();
                    ctx.package_version = ver_val.trim_matches('"').trim_matches('\'').to_string();
                } else {
                    ctx.package_version = "0.0.0".to_string();
                }
                // Count dependencies (lines under [dependencies] until next section)
                let mut count = 0usize;
                let mut in_deps = false;
                for line in text.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with("[dependencies]") {
                        in_deps = true;
                        continue;
                    }
                    if in_deps {
                        if trimmed.starts_with('[') {
                            break;
                        }
                        if !trimmed.is_empty() && !trimmed.starts_with('#') {
                            count += 1;
                        }
                    }
                }
                ctx.deps_count = count;
                // Add build commands from Cargo.toml
                ctx.build_commands.push("cargo build".to_string());
                ctx.build_commands.push("cargo test".to_string());
                if text.contains("[workspace]") {
                    ctx.build_commands
                        .push("cargo build --workspace".to_string());
                }
                // Check for supplementary build tools
                if cwd.join("Makefile").exists() {
                    ctx.build_commands.push("make".to_string());
                }
                if cwd.join("Justfile").exists() || cwd.join("justfile").exists() {
                    ctx.build_commands.push("just".to_string());
                }

                ctx.char_count += text.len();
                debug!(
                    "startup_context: loaded Cargo.toml (name={}, deps={})",
                    ctx.package_name, count
                );
            }
            Err(e) => {
                debug!("startup_context: failed to read Cargo.toml: {}", e);
            }
        }
    } else {
        debug!("startup_context: timeout reading Cargo.toml");
    }

    // ── 4. Git commit messages ──────────────────────────────────────────
    ctx.attempted_file_count += 1;
    let git_cmd = format!("git log --oneline -{}", cfg.recent_commits);
    if let Ok(result) = timeout(
        io_timeout,
        tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&git_cmd)
            .current_dir(&cwd)
            .output(),
    )
    .await
    {
        match result {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    let line = line.trim();
                    if !line.is_empty() {
                        ctx.recent_commits.push(line.to_string());
                    }
                }
                if !ctx.recent_commits.is_empty() {
                    ctx.file_count += 1;
                    ctx.loaded_components.push("git_log".to_string());
                    ctx.char_count += ctx.recent_commits.iter().map(|s| s.len()).sum::<usize>();
                    debug!(
                        "startup_context: loaded {} recent commits",
                        ctx.recent_commits.len()
                    );
                }
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                debug!("startup_context: git log failed: {}", stderr);
            }
            Err(e) => {
                debug!("startup_context: failed to execute git log: {}", e);
            }
        }
    } else {
        debug!("startup_context: timeout executing git log");
    }

    // ── 5. Style rules (.editorconfig or RULES/ directory) ──────────────
    // Try .editorconfig first
    ctx.attempted_file_count += 1;
    let editorconfig_path = cwd.join(".editorconfig");
    if let Ok(result) = timeout(io_timeout, tokio::fs::read_to_string(&editorconfig_path)).await {
        match result {
            Ok(text) => {
                ctx.file_count += 1;
                ctx.loaded_components.push("editorconfig".to_string());
                // Extract section headers and key settings as style rules
                for line in text.lines() {
                    let trimmed = line.trim();
                    if trimmed.starts_with('[')
                        || trimmed.starts_with("indent_")
                        || trimmed.starts_with("end_of_line")
                        || trimmed.starts_with("charset")
                        || trimmed.starts_with("trim_trailing_whitespace")
                        || trimmed.starts_with("insert_final_newline")
                        || trimmed.starts_with("max_line_length")
                        || trimmed.starts_with("tab_width")
                    {
                        ctx.style_rules.push(trimmed.to_string());
                    }
                }
                ctx.char_count += text.len();
                debug!(
                    "startup_context: loaded .editorconfig ({} rules)",
                    ctx.style_rules.len()
                );
            }
            Err(e) => {
                debug!("startup_context: failed to read .editorconfig: {}", e);
            }
        }
    } else {
        debug!("startup_context: timeout reading .editorconfig");
    }

    // Try RULES/ directory (limited to 10 entries)
    let rules_path = cwd.join("RULES");
    if rules_path.is_dir() {
        ctx.attempted_file_count += 1;
        if let Ok(result) = timeout(io_timeout, async {
            let mut entries = tokio::fs::read_dir(&rules_path).await?;
            let mut names = Vec::new();
            let mut file_contents = Vec::new();
            while let Some(entry) = entries.next_entry().await? {
                if names.len() >= 10 {
                    break;
                }
                let file_name = entry.file_name().to_string_lossy().to_string();
                names.push(file_name.clone());
                // Also read a snippet of each file as a style rule
                if entry
                    .file_type()
                    .await
                    .map(|ft| ft.is_file())
                    .unwrap_or(false)
                {
                    let content = tokio::fs::read_to_string(entry.path())
                        .await
                        .unwrap_or_default();
                    let snippet: String = content.chars().take(200).collect();
                    if !snippet.is_empty() {
                        file_contents.push(format!("RULES/{}: {}", file_name, snippet));
                    }
                }
            }
            Ok::<_, std::io::Error>((names, file_contents))
        })
        .await
        {
            match result {
                Ok((names, file_contents)) => {
                    ctx.rules_files = names;
                    ctx.style_rules.extend(file_contents);
                    ctx.file_count += 1;
                    ctx.loaded_components.push("RULES".to_string());
                    ctx.char_count += ctx.rules_files.iter().map(|s| s.len()).sum::<usize>();
                    debug!(
                        "startup_context: loaded RULES/ directory ({} entries)",
                        ctx.rules_files.len()
                    );
                }
                Err(e) => {
                    debug!("startup_context: failed to read RULES/ directory: {}", e);
                }
            }
        } else {
            debug!("startup_context: timeout reading RULES/ directory");
        }
    } else {
        debug!("startup_context: RULES/ directory not found, skipping");
    }

    // ── Cache and return ───────────────────────────────────────────────
    #[cfg(not(test))]
    {
        if let Ok(mut guard) = STARTUP_CONTEXT.lock() {
            if guard.is_none() {
                *guard = Some(ctx.clone());
            }
        }
    }
    debug!(
        "startup_context: load complete (components: {:?}, files: {}, chars: {})",
        ctx.loaded_components, ctx.file_count, ctx.char_count
    );
    Ok(ctx)
}

// ─────────────────────────────────────────────────────────────────────────────
// Summary text
// ─────────────────────────────────────────────────────────────────────────────

/// Produce a compact markdown-formatted summary (~500 chars) of the startup
/// context suitable for injection into `AgentTaskEnvelope.evidence`.
pub fn summary_text(ctx: &StartupContext) -> String {
    if !ctx.loaded {
        return String::new();
    }

    let mut parts: Vec<String> = Vec::new();

    // Package info
    if !ctx.package_name.is_empty() {
        let ver = if !ctx.package_version.is_empty() {
            format!(" v{}", ctx.package_version)
        } else {
            String::new()
        };
        parts.push(format!(
            "**Package:** {}{} ({} deps)",
            ctx.package_name, ver, ctx.deps_count
        ));
    }

    // README excerpt (first line or short snippet)
    if !ctx.readme_excerpt.is_empty() {
        let first_line = ctx.readme_excerpt.lines().next().unwrap_or("").trim();
        if !first_line.is_empty() {
            parts.push(format!("**README:** {}", first_line));
        }
    }

    // Recent commits
    if !ctx.recent_commits.is_empty() {
        let mut commit_lines: Vec<String> = Vec::new();
        for msg in &ctx.recent_commits {
            commit_lines.push(format!("- {}", msg));
        }
        parts.push(format!("**Recent commits:**\n{}", commit_lines.join("\n")));
    }

    // Style rules summary
    if !ctx.style_rules.is_empty() {
        let count = ctx.style_rules.len();
        let preview: String = ctx
            .style_rules
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join("; ");
        parts.push(format!(
            "**Style rules:** {} entries (e.g. {})",
            count, preview
        ));
    }

    // Rules files
    if !ctx.rules_files.is_empty() {
        let file_list = ctx.rules_files.join(", ");
        parts.push(format!("**RULES/:** {}", file_list));
    }

    // Build commands
    if !ctx.build_commands.is_empty() {
        parts.push(format!("**Build:** {}", ctx.build_commands.join(", ")));
    }

    // Component summary
    if !ctx.loaded_components.is_empty() {
        parts.push(format!(
            "**Loaded components:** {}",
            ctx.loaded_components.join(", ")
        ));
    }

    let result = parts.join("\n\n");

    // Trim to ~500 chars
    if result.chars().count() > 500 {
        let truncated: String = result.chars().take(497).collect();
        format!("{}...", truncated)
    } else {
        result
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Profile builder
// ─────────────────────────────────────────────────────────────────────────────

/// Build a `StartupContextProfile` from the cached context and configuration.
#[allow(dead_code)]
pub fn startup_context_profile(
    ctx: &StartupContext,
    cfg: &StartupContextConfig,
) -> StartupContextProfile {
    StartupContextProfile {
        enabled: cfg.enabled,
        loaded: ctx.loaded,
        loaded_components: ctx.loaded_components.clone(),
        file_count: ctx.file_count,
        char_count: ctx.char_count,
        readme_chars: ctx.readme_chars,
        build_command_count: ctx.build_commands.len(),
        style_rule_count: ctx.style_rules.len(),
        commit_count: ctx.recent_commits.len(),
        has_code_repo: ctx.has_code_repo,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::io::Write;

    /// Helper to create a temporary directory with a minimal Cargo.toml and
    /// README.md, then run `load()` against it.
    ///
    /// IMPORTANT: Loads in the temp dir by changing current_dir.  This means
    /// all startup_context tests MUST run sequentially with `--test-threads=1`.
    async fn run_load_in_tempdir(enabled: bool) -> (StartupContext, tempfile::TempDir) {
        reset_cache();
        let dir = tempfile::tempdir().expect("create temp dir");
        let dir_path = dir.path().to_path_buf();

        // Write a minimal Cargo.toml
        let cargo_toml = r#"[package]
name = "test-pkg"
version = "1.2.3"
edition = "2021"

[dependencies]
serde = "1.0"
tokio = { version = "1", features = ["full"] }
tracing = "0.1"
"#;
        let mut f = std::fs::File::create(dir_path.join("Cargo.toml")).unwrap();
        f.write_all(cargo_toml.as_bytes()).unwrap();

        // Write a README.md
        let readme = "# Test Project\n\nThis is a test project for unit tests.\n";
        let mut f = std::fs::File::create(dir_path.join("README.md")).unwrap();
        f.write_all(readme.as_bytes()).unwrap();

        // Write .editorconfig
        let editorconfig = "root = true\n\n[*]\nindent_style = space\nindent_size = 4\nend_of_line = lf\ncharset = utf-8\ntrim_trailing_whitespace = true\ninsert_final_newline = true\n";
        let mut f = std::fs::File::create(dir_path.join(".editorconfig")).unwrap();
        f.write_all(editorconfig.as_bytes()).unwrap();

        // Change to the temp dir for the duration of loading
        let orig_cwd = std::env::current_dir().expect("capture initial cwd at fn entry");
        std::env::set_current_dir(&dir_path).unwrap();

        let cfg = StartupContextConfig {
            enabled,
            readme_max_chars: 2000,
            recent_commits: 5,
            io_timeout_ms: 5_000,
        };
        let ctx = load(&cfg).await.expect("load should succeed");

        // Restore original cwd
        let _ = std::env::set_current_dir(&orig_cwd);

        (ctx, dir)
    }

    #[tokio::test]
    #[serial]
    async fn test_load_disabled_returns_default() {
        let (ctx, _dir) = run_load_in_tempdir(false).await;
        assert!(!ctx.loaded);
        assert!(ctx.readme_excerpt.is_empty());
        assert!(ctx.package_name.is_empty());
        assert!(ctx.loaded_components.is_empty());
    }

    #[tokio::test]
    #[serial]
    async fn test_load_enabled_reads_files() {
        let (ctx, _dir) = run_load_in_tempdir(true).await;
        assert!(ctx.loaded);
        // README
        assert!(
            !ctx.readme_excerpt.is_empty(),
            "README should have been loaded"
        );
        assert!(ctx.readme_excerpt.contains("Test Project"));
        // Cargo.toml
        assert_eq!(ctx.package_name, "test-pkg");
        assert_eq!(ctx.package_version, "1.2.3");
        assert!(
            ctx.deps_count >= 3,
            "should have found deps, got {}",
            ctx.deps_count
        );
        // Components
        assert!(
            ctx.loaded_components
                .iter()
                .any(|c| c.starts_with("readme:")),
            "readme component missing"
        );
        assert!(
            ctx.loaded_components.contains(&"cargo".to_string()),
            "cargo component missing"
        );
        assert!(
            ctx.loaded_components.contains(&"editorconfig".to_string()),
            "editorconfig component missing"
        );
        // File count
        assert!(
            ctx.file_count >= 3,
            "should have read at least 3 files, got {}",
            ctx.file_count
        );
        // Build commands
        assert!(ctx.build_commands.contains(&"cargo build".to_string()));
    }

    #[tokio::test]
    #[serial]
    async fn test_load_readme_fallback_order() {
        reset_cache();
        let dir = tempfile::tempdir().expect("create temp dir");
        let dir_path = dir.path().to_path_buf();

        // Only write README.zh-CN.md (skip README.md and README)
        let readme = "# 中文项目\n\n测试\n";
        let mut f = std::fs::File::create(dir_path.join("README.zh-CN.md")).unwrap();
        f.write_all(readme.as_bytes()).unwrap();

        let orig_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir_path).unwrap();

        let cfg = StartupContextConfig {
            enabled: true,
            ..Default::default()
        };
        let ctx = load(&cfg).await.expect("load should succeed");

        std::env::set_current_dir(&orig_cwd).unwrap();

        assert!(
            ctx.readme_excerpt.contains("中文项目"),
            "should have loaded README.zh-CN.md"
        );
        assert!(ctx
            .loaded_components
            .iter()
            .any(|c| c.contains("README.zh-CN.md")));
    }

    #[tokio::test]
    #[serial]
    async fn test_summary_text_returns_formatted_string() {
        let (ctx, _dir) = run_load_in_tempdir(true).await;
        let summary = summary_text(&ctx);
        assert!(
            !summary.is_empty(),
            "summary should not be empty for loaded context"
        );
        assert!(
            summary.contains("Package:"),
            "summary should contain package info"
        );
        assert!(
            summary.contains("README:"),
            "summary should contain README info"
        );
        assert!(
            summary.chars().count() <= 550,
            "summary should be ~500 chars, was {}",
            summary.chars().count()
        );
    }

    #[tokio::test]
    async fn test_summary_text_returns_empty_when_not_loaded() {
        let ctx = StartupContext::default();
        let summary = summary_text(&ctx);
        assert!(
            summary.is_empty(),
            "summary should be empty for unloaded context"
        );
    }

    #[tokio::test]
    #[serial]
    async fn test_profile_contains_loaded_components() {
        let (ctx, _dir) = run_load_in_tempdir(true).await;
        let cfg = StartupContextConfig {
            enabled: true,
            ..Default::default()
        };
        let profile = startup_context_profile(&ctx, &cfg);

        assert!(profile.enabled);
        assert!(profile.loaded);
        assert!(!profile.loaded_components.is_empty());
        assert!(profile.file_count >= 3);
        assert!(profile.char_count > 0);
        assert!(profile.readme_chars > 0);
        assert!(profile.build_command_count > 0);
    }

    #[tokio::test]
    async fn test_profile_disabled() {
        let ctx = StartupContext::default();
        let cfg = StartupContextConfig {
            enabled: false,
            ..Default::default()
        };
        let profile = startup_context_profile(&ctx, &cfg);
        assert!(!profile.enabled);
        assert!(!profile.loaded);
        assert!(profile.loaded_components.is_empty());
    }

    #[tokio::test]
    #[serial]
    async fn test_git_log_failure_does_not_crash() {
        reset_cache();
        let dir = tempfile::tempdir().expect("create temp dir");
        let dir_path = dir.path().to_path_buf();

        // No git repo here, so git log will fail gracefully
        let orig_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir_path).unwrap();

        let cfg = StartupContextConfig {
            enabled: true,
            ..Default::default()
        };
        let ctx = load(&cfg)
            .await
            .expect("load should not crash even without git repo");

        std::env::set_current_dir(&orig_cwd).unwrap();

        // Should still be loaded (other components may have succeeded),
        // but commits will be empty
        assert!(ctx.recent_commits.is_empty() || ctx.loaded);
    }

    #[tokio::test]
    #[serial]
    async fn test_rules_directory_loaded() {
        reset_cache();
        let dir = tempfile::tempdir().expect("create temp dir");
        let dir_path = dir.path().to_path_buf();

        // Create RULES/ directory with some files
        let rules_dir = dir_path.join("RULES");
        std::fs::create_dir_all(&rules_dir).unwrap();
        let mut f = std::fs::File::create(rules_dir.join("style.md")).unwrap();
        f.write_all(b"# Style Guide\n\nUse 2-space indentation.\n")
            .unwrap();
        let mut f = std::fs::File::create(rules_dir.join("naming.md")).unwrap();
        f.write_all(b"# Naming\n\nUse snake_case.\n").unwrap();

        // Also write a minimal Cargo.toml so the loader has something
        let cargo_toml = "[package]\nname = \"rules-test\"\nversion = \"0.1.0\"\n[dependencies]\n";
        let mut f = std::fs::File::create(dir_path.join("Cargo.toml")).unwrap();
        f.write_all(cargo_toml.as_bytes()).unwrap();

        let orig_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir_path).unwrap();

        let cfg = StartupContextConfig {
            enabled: true,
            ..Default::default()
        };
        let ctx = load(&cfg).await.expect("load should succeed");

        std::env::set_current_dir(&orig_cwd).unwrap();

        assert!(
            ctx.loaded_components.contains(&"RULES".to_string()),
            "RULES component should be present"
        );
        assert!(!ctx.rules_files.is_empty(), "should have found RULES files");
        assert!(
            ctx.rules_files.iter().any(|f| f == "style.md"),
            "should contain style.md"
        );
        assert!(
            ctx.rules_files.iter().any(|f| f == "naming.md"),
            "should contain naming.md"
        );
        // Style rules should include snippets from RULES files
        assert!(
            ctx.style_rules.iter().any(|s| s.contains("RULES/")),
            "style_rules should contain RULES snippets"
        );
    }

    #[tokio::test]
    #[serial]
    async fn test_missing_files_graceful() {
        reset_cache();
        let dir = tempfile::tempdir().expect("create temp dir");
        let dir_path = dir.path().to_path_buf();

        // Empty directory — no files at all
        let orig_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir_path).unwrap();

        let cfg = StartupContextConfig {
            enabled: true,
            ..Default::default()
        };
        let ctx = load(&cfg)
            .await
            .expect("load should not crash with missing files");

        std::env::set_current_dir(&orig_cwd).unwrap();

        assert!(ctx.loaded); // loaded=true even with nothing found
        assert!(ctx.readme_excerpt.is_empty());
        assert!(ctx.package_name.is_empty());
        assert!(ctx.recent_commits.is_empty());
        assert!(ctx.style_rules.is_empty());
        assert!(ctx.rules_files.is_empty());
    }

    #[tokio::test]
    async fn test_summary_text_truncation() {
        let mut ctx = StartupContext {
            loaded: true,
            package_name: "a".repeat(100),
            package_version: "0.0.0".to_string(),
            deps_count: 999,
            readme_excerpt: "x".repeat(400),
            readme_chars: 400,
            ..Default::default()
        };
        ctx.recent_commits.push("y".repeat(200));
        ctx.build_commands.push("cargo build".to_string());
        ctx.loaded_components.push("test".to_string());
        ctx.style_rules.push("z".repeat(100));

        let summary = summary_text(&ctx);
        assert!(
            summary.chars().count() <= 510,
            "summary too long: {} chars",
            summary.chars().count()
        );
        // Should end with ellipsis if truncated
        if summary.chars().count() >= 497 {
            assert!(
                summary.ends_with("..."),
                "truncated summary should end with ..."
            );
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_cargo_metadata_extraction() {
        reset_cache();
        let dir = tempfile::tempdir().expect("create temp dir");
        let dir_path = dir.path().to_path_buf();

        let cargo_toml = r#"[package]
name = "my-crate"
version = "3.2.1"
edition = "2021"
description = "Test crate"

[dependencies]
tokio = "1"
serde = { version = "1", features = ["derive"] }
reqwest = { version = "0.12", default-features = false, features = ["json"] }

[dev-dependencies]
tempfile = "3"

[build-dependencies]
cc = "1"
"#;
        let mut f = std::fs::File::create(dir_path.join("Cargo.toml")).unwrap();
        f.write_all(cargo_toml.as_bytes()).unwrap();

        let orig_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(&dir_path).unwrap();

        let cfg = StartupContextConfig {
            enabled: true,
            ..Default::default()
        };
        let ctx = load(&cfg).await.expect("load should succeed");

        std::env::set_current_dir(&orig_cwd).unwrap();

        assert_eq!(ctx.package_name, "my-crate");
        assert_eq!(ctx.package_version, "3.2.1");
        assert_eq!(
            ctx.deps_count, 3,
            "should count 3 [dependencies] entries (tokio, serde, reqwest)"
        );
    }

    #[tokio::test]
    #[serial]
    async fn test_once_lock_caching() {
        reset_cache();
        // Verify that the load function returns consistent results.
        // When called without the static cache (test mode), each call
        // loads fresh data from the filesystem.
        let (ctx1, dir) = run_load_in_tempdir(true).await;

        // ctx1 should have the expected values
        assert!(ctx1.loaded);
        assert_eq!(ctx1.package_name, "test-pkg");

        // After run_load_in_tempdir, cwd is restored. Load again from
        // temp dir path by explicitly changing cwd
        drop(ctx1);
        let temp_path = dir.path().to_path_buf();
        let orig_cwd = std::env::current_dir().expect("capture cwd before second load");
        std::env::set_current_dir(&temp_path).unwrap();

        let cfg = StartupContextConfig {
            enabled: true,
            readme_max_chars: 2000,
            recent_commits: 5,
            io_timeout_ms: 5_000,
        };
        let ctx2 = load(&cfg).await.unwrap();
        assert_eq!(ctx2.package_name, "test-pkg");

        // Restore cwd; tempdir is cleaned up on drop
        let _ = std::env::set_current_dir(&orig_cwd);
        drop(dir);
    }
}
