//! System detection — asynchronous project context loading
//!
//! Provides the `load()` function that reads README, Cargo.toml, git log,
//! editorconfig, and RULES directory. All file operations are best-effort
//! with graceful fallback and configurable timeouts.

use super::*;
use tokio::time::timeout;
use tracing::debug;

/// Partial result of one parallel load phase in [`load`].
/// Each phase owns disjoint fields; results are merged in fixed order after
/// `tokio::join!`, so `loaded_components` stays deterministic.
#[derive(Default)]
struct PhaseResult {
    /// Number of I/O attempts made by this phase.
    attempts: usize,
    /// Whether the phase produced a usable result.
    loaded: bool,
    /// Component label for `loaded_components` (None when not loaded).
    component: Option<String>,
    /// README excerpt + char count (phase: README).
    readme_excerpt: String,
    readme_chars: usize,
    /// Cargo.toml metadata (phase: Cargo.toml).
    package_name: String,
    package_version: String,
    deps_count: usize,
    build_commands: Vec<String>,
    /// git log commits (phase: git log).
    recent_commits: Vec<String>,
    /// Style rules from .editorconfig and RULES/ (phases: editorconfig + RULES).
    style_rules: Vec<String>,
    rules_files: Vec<String>,
    /// Accounting: one file per successful phase, plus extracted chars.
    file_count: usize,
    char_count: usize,
}

/// Merge one parallel phase's result into the shared `StartupContext`.
fn merge_phase(ctx: &mut StartupContext, phase: PhaseResult) {
    ctx.attempted_file_count += phase.attempts;
    ctx.char_count += phase.char_count;
    if phase.loaded {
        ctx.file_count += 1;
        if let Some(component) = phase.component {
            ctx.loaded_components.push(component);
        }
    }
    if !phase.readme_excerpt.is_empty() {
        ctx.readme_excerpt = phase.readme_excerpt;
        ctx.readme_chars = phase.readme_chars;
    }
    if !phase.package_name.is_empty() {
        ctx.package_name = phase.package_name;
    }
    if !phase.package_version.is_empty() {
        ctx.package_version = phase.package_version;
    }
    if phase.deps_count > 0 {
        ctx.deps_count = phase.deps_count;
    }
    ctx.build_commands.extend(phase.build_commands);
    ctx.recent_commits.extend(phase.recent_commits);
    ctx.style_rules.extend(phase.style_rules);
    ctx.rules_files.extend(phase.rules_files);
}

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
    let readme_max_chars = cfg.readme_max_chars;
    let recent_commits = cfg.recent_commits;

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

    // ── Parallel load phases ────────────────────────────────────────────
    // README, Cargo.toml, git log, .editorconfig and RULES/ are mutually
    // independent (git log spawns a subprocess; the rest are file reads), so
    // they run concurrently via `tokio::join!` instead of five serialized
    // timeouts. Every phase keeps its own `io_timeout_ms` timeout, and each
    // partial result is merged in fixed order afterwards.

    // Phase 2: README excerpt (fallback order preserved within the phase).
    let readme_fut = async {
        let mut attempts = 0usize;
        let readme_candidates = ["README.md", "README", "README.zh-CN.md"];
        for name in &readme_candidates {
            let path = cwd.join(name);
            attempts += 1;
            if let Ok(result) = timeout(io_timeout, tokio::fs::read_to_string(&path)).await {
                match result {
                    Ok(text) => {
                        let excerpt: String = text.chars().take(readme_max_chars).collect();
                        let chars = excerpt.chars().count();
                        debug!("startup_context: loaded README from {}", name);
                        return PhaseResult {
                            attempts,
                            loaded: true,
                            component: Some(format!("readme:{}", name)),
                            readme_excerpt: excerpt,
                            readme_chars: chars,
                            file_count: 1,
                            char_count: chars,
                            ..PhaseResult::default()
                        };
                    }
                    Err(e) => {
                        debug!("startup_context: failed to read {}: {}", name, e);
                    }
                }
            } else {
                debug!("startup_context: timeout reading {}", name);
            }
        }
        PhaseResult {
            attempts,
            ..PhaseResult::default()
        }
    };

    // Phase 3: Cargo.toml (package name, version, deps count).
    let cargo_fut = async {
        let mut result = PhaseResult {
            attempts: 1,
            ..PhaseResult::default()
        };
        let cargo_path = cwd.join("Cargo.toml");
        if let Ok(read) = timeout(io_timeout, tokio::fs::read_to_string(&cargo_path)).await {
            match read {
                Ok(text) => {
                    result.loaded = true;
                    result.component = Some("cargo".to_string());
                    result.file_count = 1;
                    result.char_count = text.len();

                    // Extract package name
                    if let Some(line) = text.lines().find(|l| l.trim().starts_with("name")) {
                        let name_val = line.split('=').nth(1).unwrap_or("").trim();
                        result.package_name =
                            name_val.trim_matches('"').trim_matches('\'').to_string();
                    } else {
                        result.package_name = "(unnamed)".to_string();
                    }
                    // Extract package version
                    if let Some(line) = text.lines().find(|l| l.trim().starts_with("version")) {
                        let ver_val = line.split('=').nth(1).unwrap_or("").trim();
                        result.package_version =
                            ver_val.trim_matches('"').trim_matches('\'').to_string();
                    } else {
                        result.package_version = "0.0.0".to_string();
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
                    result.deps_count = count;
                    // Add build commands from Cargo.toml
                    result.build_commands.push("cargo build".to_string());
                    result.build_commands.push("cargo test".to_string());
                    if text.contains("[workspace]") {
                        result
                            .build_commands
                            .push("cargo build --workspace".to_string());
                    }
                    // Check for supplementary build tools
                    if cwd.join("Makefile").exists() {
                        result.build_commands.push("make".to_string());
                    }
                    if cwd.join("Justfile").exists() || cwd.join("justfile").exists() {
                        result.build_commands.push("just".to_string());
                    }

                    debug!(
                        "startup_context: loaded Cargo.toml (name={}, deps={})",
                        result.package_name, count
                    );
                }
                Err(e) => {
                    debug!("startup_context: failed to read Cargo.toml: {}", e);
                }
            }
        } else {
            debug!("startup_context: timeout reading Cargo.toml");
        }
        result
    };

    // Phase 4: git commit messages.
    let git_fut = async {
        let mut result = PhaseResult {
            attempts: 1,
            ..PhaseResult::default()
        };
        let git_cmd = format!("git log --oneline -{}", recent_commits);
        if let Ok(out) = timeout(
            io_timeout,
            tokio::process::Command::new("sh")
                .arg("-c")
                .arg(&git_cmd)
                .current_dir(&cwd)
                .output(),
        )
        .await
        {
            match out {
                Ok(output) if output.status.success() => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    for line in stdout.lines() {
                        let line = line.trim();
                        if !line.is_empty() {
                            result.recent_commits.push(line.to_string());
                        }
                    }
                    if !result.recent_commits.is_empty() {
                        result.loaded = true;
                        result.component = Some("git_log".to_string());
                        result.file_count = 1;
                        result.char_count =
                            result.recent_commits.iter().map(|s| s.len()).sum::<usize>();
                        debug!(
                            "startup_context: loaded {} recent commits",
                            result.recent_commits.len()
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
        result
    };

    // Phase 5: .editorconfig style rules.
    let editorconfig_fut = async {
        let mut result = PhaseResult {
            attempts: 1,
            ..PhaseResult::default()
        };
        let editorconfig_path = cwd.join(".editorconfig");
        if let Ok(read) = timeout(io_timeout, tokio::fs::read_to_string(&editorconfig_path)).await {
            match read {
                Ok(text) => {
                    result.loaded = true;
                    result.component = Some("editorconfig".to_string());
                    result.file_count = 1;
                    result.char_count = text.len();
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
                            result.style_rules.push(trimmed.to_string());
                        }
                    }
                    debug!(
                        "startup_context: loaded .editorconfig ({} rules)",
                        result.style_rules.len()
                    );
                }
                Err(e) => {
                    debug!("startup_context: failed to read .editorconfig: {}", e);
                }
            }
        } else {
            debug!("startup_context: timeout reading .editorconfig");
        }
        result
    };

    // Phase 6: RULES/ directory (limited to 10 entries).
    let rules_fut = async {
        let rules_path = cwd.join("RULES");
        if !rules_path.is_dir() {
            debug!("startup_context: RULES/ directory not found, skipping");
            return PhaseResult::default();
        }
        let mut result = PhaseResult {
            attempts: 1,
            ..PhaseResult::default()
        };
        if let Ok(scan) = timeout(io_timeout, async {
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
            match scan {
                Ok((names, file_contents)) => {
                    result.loaded = true;
                    result.component = Some("RULES".to_string());
                    result.file_count = 1;
                    result.rules_files = names;
                    result.style_rules.extend(file_contents);
                    result.char_count = result.rules_files.iter().map(|s| s.len()).sum::<usize>();
                    debug!(
                        "startup_context: loaded RULES/ directory ({} entries)",
                        result.rules_files.len()
                    );
                }
                Err(e) => {
                    debug!("startup_context: failed to read RULES/ directory: {}", e);
                }
            }
        } else {
            debug!("startup_context: timeout reading RULES/ directory");
        }
        result
    };

    // ── Merge phase results (fixed order keeps loaded_components stable) ──
    let (readme, cargo, git, editorconfig, rules) =
        tokio::join!(readme_fut, cargo_fut, git_fut, editorconfig_fut, rules_fut);
    for phase in [readme, cargo, git, editorconfig, rules] {
        merge_phase(&mut ctx, phase);
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
