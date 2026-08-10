//! S5: Startup Repository Context Loader
//!
//! BLUE38 ARCH-06: Asynchronously loads project-level context (README, build commands,
//! recent commits, style rules) once per process startup. Uses OnceLock to prevent
//! redundant loads. All file operations are best-effort with graceful fallback.
//!
//! Design:
//! - `load()` performs async I/O with timeouts for README, Cargo.toml, git log,
//!   and editorconfig / RULES directory.
//! - Results are cached in a process-global `Mutex<Option<StartupContext>>`.
//! - `summary_text()` produces a compact markdown block suitable for injection into
//!   `AgentTaskEnvelope.evidence`.
//! - `StartupContextProfile` returns a JSON-serializable profile for governance
//!   metrics endpoints.
//!
//! Graceful degradation: missing files, corrupt outputs, or I/O errors produce empty
//! fields rather than panics or propagating errors.

use serde::{Deserialize, Serialize};
use std::sync::Mutex;

use crate::config::StartupContextConfig;
use std::time::Duration;

pub mod detection;
pub mod profile;

pub use detection::load;

// ─────────────────────────────────────────────────────────────────────────────
// Profile struct (governance metrics)
// ─────────────────────────────────────────────────────────────────────────────

/// Governance-metrics snapshot of the startup context loader.
///
/// Returned by `startup_context_profile()` for the `/governance/status` endpoint.
/// Exported via all 5 protocol modes (auto/acp-stdio/acp-http/mcp-stdio/mcp-http).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StartupContextProfile {
    /// Whether the startup context loader is enabled in config
    pub enabled: bool,
    /// Whether `load()` has completed successfully at least once
    pub loaded: bool,
    /// Names of components that were loaded successfully
    pub loaded_components: Vec<String>,
    /// Number of files successfully read
    pub file_count: usize,
    /// Number of files/operations attempted (successful or not)
    pub attempted_file_count: usize,
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
/// Stored in a process-global `Mutex<Option<StartupContext>>`.  Fields are
/// populated best-effort; missing files or I/O errors leave them as defaults.
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
    STARTUP_CONTEXT
        .lock()
        .unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned, recovering");
            poisoned.into_inner()
        })
        .clone()
}

/// Reset the cached startup context. Only available in tests.
#[cfg(test)]
fn reset_cache() {
    let mut guard = STARTUP_CONTEXT.lock().unwrap_or_else(|poisoned| {
        tracing::warn!("startup_context reset_cache lock poisoned, recovering");
        poisoned.into_inner()
    });
    *guard = None;
}

// ─────────────────────────────────────────────────────────────────────────────
// Configuration
// ─────────────────────────────────────────────────────────────────────────────

// StartupContextConfig is defined in core/config/types.rs (re-exported as
// crate::config::StartupContextConfig) and shared with the config loader; the
// io_timeout_ms field was merged into that type to eliminate the duplicate
// definition that used to live here.

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
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::startup_context::profile::startup_context_profile;
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
