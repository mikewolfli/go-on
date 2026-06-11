//! System detection — asynchronous project context loading
//!
//! Provides the `load()` function that reads README, Cargo.toml, git log,
//! editorconfig, and RULES directory. All file operations are best-effort
//! with graceful fallback and configurable timeouts.

use super::*;
use tokio::time::timeout;
use tracing::debug;

/// Load startup context asynchronously.
///
/// This is safe to call multiple times – the global static prevents redundant
/// work in non-test builds.  All file operations use `tokio::fs` with a
/// configurable timeout.  Missing files, corrupt data, or I/O errors are
/// silently degraded.
pub async fn load(cfg: &StartupContextConfig) -> Result<StartupContext, std::io::Error> {
    if !cfg.enabled {
        let ctx = StartupContext::default();
        #[cfg(not(test))]
        {
            let mut guard = STARTUP_CONTEXT.lock().unwrap_or_else(|poisoned| {
                tracing::warn!("startup_context lock poisoned, recovering");
                poisoned.into_inner()
            });
            *guard = Some(ctx.clone());
        }
        return Ok(ctx);
    }

    // If another task already initialised the global, just return a clone.
    #[cfg(not(test))]
    {
        let guard = match STARTUP_CONTEXT.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::warn!("[B48] STARTUP_CONTEXT lock poisoned, recovering");
                poisoned.into_inner()
            }
        };
        if let Some(cached) = &*guard {
            return Ok(cached.clone());
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
        let mut guard = STARTUP_CONTEXT.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("startup_context lock poisoned, recovering");
            poisoned.into_inner()
        });
        if guard.is_none() {
            *guard = Some(ctx.clone());
        }
    }
    debug!(
        "startup_context: load complete (components: {:?}, files: {}, chars: {})",
        ctx.loaded_components, ctx.file_count, ctx.char_count
    );
    Ok(ctx)
}
