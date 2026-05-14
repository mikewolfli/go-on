//! System initialization and context loader (Phase 2/3)
//!
//! These structures are intentional framework definitions for Phase 0-9 architecture.
//! SystemContext manages memory, audit, and policy layers that will be injected
//! into mode runtimes and agent orchestration once integration is implemented.

use crate::audit::AuditLog;
use crate::memory::memory::{MemoryPolicy, MemoryStore};
use anyhow::Result;

/// Default capacity for the audit log ring buffer.
const DEFAULT_AUDIT_LOG_CAPACITY: usize = 1000;

/// System context for agent execution
pub struct SystemContext {
    pub memory_store: MemoryStore,
    pub audit_log: AuditLog,
    pub memory_policy: MemoryPolicy,
}

impl SystemContext {
    pub fn new() -> Self {
        let policy = MemoryPolicy::default();
        Self {
            memory_store: MemoryStore::new(policy.clone()),
            audit_log: AuditLog::new(DEFAULT_AUDIT_LOG_CAPACITY),
            memory_policy: policy,
        }
    }

    /// Load repository context asynchronously (README, build commands, recent commits, etc.)
    pub fn load_repo_context(&mut self, repo_path: &str) -> Result<()> {
        use std::path::Path;

        let repo = Path::new(repo_path);
        if !repo.exists() {
            // No repo path available; nothing to load
            return Ok(());
        }

        // 1. Load README (try common filenames)
        for name in &["README.md", "README.txt", "README", "readme.md"] {
            let readme_path = repo.join(name);
            if let Ok(content) = std::fs::read_to_string(&readme_path) {
                let excerpt: String = content.chars().take(2000).collect();
                // Store excerpt in memory for later agent context injection
                // (In a full implementation, this would go into memory_store)
                tracing::debug!(repo = %repo_path, chars = excerpt.len(), "loaded README excerpt");
                break;
            }
        }

        // 2. Detect build commands from project files
        let mut build_commands: Vec<String> = Vec::new();
        if repo.join("Cargo.toml").exists() {
            build_commands.push("cargo build".to_string());
            build_commands.push("cargo test".to_string());
        }
        if repo.join("package.json").exists() {
            build_commands.push("npm install".to_string());
            build_commands.push("npm run build".to_string());
            build_commands.push("npm test".to_string());
        }
        if repo.join("go.mod").exists() {
            build_commands.push("go build".to_string());
            build_commands.push("go test".to_string());
        }
        if repo.join("Makefile").exists() || repo.join("makefile").exists() {
            build_commands.push("make".to_string());
        }

        if !build_commands.is_empty() {
            tracing::debug!(repo = %repo_path, commands = ?build_commands, "detected build commands");
        }

        // 3. Load recent git commits (best-effort)
        if repo.join(".git").exists() {
            if let Ok(output) = std::process::Command::new("git")
                .args(["-C", repo_path, "log", "--oneline", "-10"])
                .output()
            {
                if output.status.success() {
                    let lines = String::from_utf8_lossy(&output.stdout);
                    let commits: Vec<String> = lines.lines().map(|l| l.to_string()).collect();
                    tracing::debug!(repo = %repo_path, count = commits.len(), "loaded recent commits");
                }
            }
        }

        // 4. Detect project style rules / editor config
        let style_files = &[
            ".editorconfig",
            ".rustfmt.toml",
            "rustfmt.toml",
            ".prettierrc",
            ".prettierrc.json",
            ".eslintrc.json",
            "tsconfig.json",
        ];
        for name in style_files {
            if repo.join(name).exists() {
                tracing::debug!(repo = %repo_path, file = %name, "detected style configuration");
            }
        }

        tracing::info!(repo = %repo_path, "repository context loaded");
        Ok(())
    }

    /// Cleanup and persistence
    pub fn shutdown(&mut self) -> Result<()> {
        self.memory_store.gc();
        Ok(())
    }
}

impl Default for SystemContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Global context holder (can be made thread-safe with Arc<Mutex<>>)
pub struct GlobalContext {
    pub system: SystemContext,
}

impl GlobalContext {
    pub fn new() -> Self {
        Self {
            system: SystemContext::new(),
        }
    }
}

impl Default for GlobalContext {
    fn default() -> Self {
        Self::new()
    }
}
