//! S5: Startup Repository Context Loader
//!
//! Asynchronously loads project-level context (README, build commands, recent commits,
//! style rules) once per process startup. Uses OnceLock to prevent redundant loads.
//!
//! NOTE: This is an intentional architecture framework (Phase 0-9).
//! Kept as a stable extension point for future startup context integration.

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use tracing::debug;

/// Snapshot of project-level context loaded at startup
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StartupContext {
    pub loaded: bool,
    pub readme_excerpt: String,
    pub readme_chars: usize,
    pub build_commands: Vec<String>,
    pub recent_commits: Vec<String>,
    pub style_rules: Vec<String>,
    /// True when code-repo fingerprint files are detected
    pub has_code_repo: bool,
}

static STARTUP_CONTEXT: OnceLock<StartupContext> = OnceLock::new();

/// Get the globally cached startup context (None until first `load()` call)
pub fn get() -> Option<&'static StartupContext> {
    STARTUP_CONTEXT.get()
}

/// Configuration knobs for startup context loading
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartupContextConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_readme_max_chars")]
    pub readme_max_chars: usize,
    #[serde(default = "default_recent_commits")]
    pub recent_commits: usize,
}

fn default_readme_max_chars() -> usize {
    2000
}
fn default_recent_commits() -> usize {
    5
}

impl Default for StartupContextConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            readme_max_chars: 2000,
            recent_commits: 5,
        }
    }
}

/// Load startup context (non-blocking: call with tokio::spawn)
pub async fn load(cfg: &StartupContextConfig) -> StartupContext {
    if !cfg.enabled {
        return StartupContext::default();
    }

    let mut ctx = StartupContext {
        loaded: true,
        ..Default::default()
    };

    // Detect code-repo fingerprint files
    let fingerprint_files = [
        "Cargo.toml",
        "package.json",
        "go.mod",
        "pom.xml",
        "pyproject.toml",
        "setup.py",
    ];
    let cwd = std::env::current_dir().unwrap_or_default();
    ctx.has_code_repo = fingerprint_files.iter().any(|f| cwd.join(f).exists());

    // README excerpt
    for name in ["README.md", "readme.md", "README.txt", "README"] {
        let path = cwd.join(name);
        if let Ok(content) = tokio::fs::read_to_string(&path).await {
            let excerpt: String = content.chars().take(cfg.readme_max_chars).collect();
            ctx.readme_chars = excerpt.len();
            ctx.readme_excerpt = excerpt;
            break;
        }
    }

    // Build commands from Cargo.toml / package.json
    if cwd.join("Cargo.toml").exists() {
        ctx.build_commands.push("cargo build".to_string());
        ctx.build_commands.push("cargo test".to_string());
    }
    if cwd.join("package.json").exists() {
        ctx.build_commands.push("npm install".to_string());
        ctx.build_commands.push("npm run build".to_string());
    }

    // Recent git commits (best-effort)
    let commit_count = cfg.recent_commits;
    if let Ok(output) = tokio::process::Command::new("git")
        .args(["log", "--oneline", &format!("-{}", commit_count)])
        .output()
        .await
    {
        if output.status.success() {
            let lines = String::from_utf8_lossy(&output.stdout);
            ctx.recent_commits = lines.lines().map(|l| l.to_string()).collect();
        }
    }

    debug!(
        loaded = ctx.loaded,
        has_code_repo = ctx.has_code_repo,
        readme_chars = ctx.readme_chars,
        "startup_context loaded"
    );

    let _ = STARTUP_CONTEXT.set(ctx.clone());
    ctx
}

/// Build a summary string suitable for injection into AgentTaskEnvelope.evidence
pub fn summary_text(ctx: &StartupContext) -> String {
    let mut parts = Vec::new();
    if !ctx.readme_excerpt.is_empty() {
        parts.push(format!(
            "README: {}",
            ctx.readme_excerpt.chars().take(400).collect::<String>()
        ));
    }
    if !ctx.build_commands.is_empty() {
        parts.push(format!("Build: {}", ctx.build_commands.join(", ")));
    }
    if !ctx.recent_commits.is_empty() {
        parts.push(format!("Recent commits: {}", ctx.recent_commits.join("; ")));
    }
    parts.join("\n")
}
