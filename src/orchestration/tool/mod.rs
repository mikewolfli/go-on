//! Tool trait and tool runtime for go-on
//!
//! These structures are intentional framework definitions for Phase 0-9 architecture.
//! Tool trait, registry, and implementations will be connected to the execution flow
//! once orchestration logic integrates them.

// ── Sub-modules (moved from orchestration/ for cohesion) ───────────────────
pub mod builtin_tools;
pub mod exec_common;
pub mod executor;
pub mod extended;
pub mod file_walk;
pub mod governance_gate;
pub mod lock;
// pub mod native; — removed: NativeToolBridge was superseded by shared::tool_descriptors
// and all tests were already covered by autonomy_runtime tests.
// pub mod registry_macro; — removed: the builtin_tools! macro had no production callers.
// pub mod events; — removed: the ToolProgress/ProgressSender progress subsystem
// had no production callers (run_with_progress/run_streaming were never invoked).
mod registration;
pub mod types;
use crate::i18n::runtime::tf;
use anyhow::Result;
pub use file_walk::*;
pub use governance_gate::{governance_cache, is_low_risk_tool, ShardedGovernanceCache};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};
pub use types::*;

use crate::orchestration::registration::RegistrationGuard;
use crate::orchestration::skill::SkillRegistry;
use crate::orchestration::tool::lock::ToolLockManager as TLM;

/// Global tool lock manager for file access synchronization.
static TOOL_LOCK_MANAGER: OnceLock<TLM> = OnceLock::new();

fn tool_lock_manager() -> &'static TLM {
    TOOL_LOCK_MANAGER.get_or_init(TLM::new)
}

/// Global skill registry reference for tools that need access to registered skills.
static SKILL_REGISTRY: OnceLock<Arc<RwLock<SkillRegistry>>> = OnceLock::new();

/// Get the global skill registry reference, if set.
pub fn skill_registry() -> Option<&'static Arc<RwLock<SkillRegistry>>> {
    SKILL_REGISTRY.get()
}

/// Set the global skill registry reference used by `SkillListTool` and other
/// registry-aware tools. Call this once during server startup after the skill
/// registry has been initialized.
pub fn set_skill_registry(registry: Arc<RwLock<SkillRegistry>>) {
    if SKILL_REGISTRY.set(registry).is_err() {
        tracing::warn!(
            target: "tool",
            "set_skill_registry called more than once — ignoring duplicate"
        );
    }
}

/// Deduplicate concurrent skill calls: when the model emits several skill
/// calls at once, keep every non-skill tool and only the best-scored skill.
/// Returns the filtered calls plus the name of the winning skill when a
/// dedup actually happened (so callers can surface a user-facing notice).
/// Shared by the CLI chat and ACP agent-runtime paths so behavior stays
/// identical on both sides.
pub fn dedup_skill_calls(
    calls: &[(String, String)],
    registry: &RwLock<SkillRegistry>,
) -> (Vec<(String, String)>, Option<String>) {
    let reg = registry
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let skill_names: Vec<&str> = calls
        .iter()
        .filter(|(name, _)| reg.get(name).is_some())
        .map(|(name, _)| name.as_str())
        .collect();
    if skill_names.len() <= 1 {
        return (calls.to_vec(), None);
    }
    let best = skill_names
        .iter()
        .filter_map(|name| {
            let score = reg
                .score_of(name)
                .unwrap_or(crate::acp::helpers::agent_selector::DEFAULT_REPUTATION_SCORE);
            reg.get(name).map(|_| (name.to_string(), score))
        })
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    match best {
        Some((best_name, _)) => {
            tracing::warn!(
                target: "tool",
                "skill dedup: AI called {} skills ({}), auto-selecting '{}'",
                skill_names.len(),
                skill_names.join(", "),
                best_name
            );
            let deduped = calls
                .iter()
                .filter(|(name, _)| match reg.get(name) {
                    Some(_) => *name == best_name,
                    None => true,
                })
                .cloned()
                .collect();
            (deduped, Some(best_name))
        }
        None => (calls.to_vec(), None),
    }
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let tool_names: Vec<&&str> = self.tools.keys().collect();
        f.debug_struct("ToolRegistry")
            .field("tools", &tool_names)
            .field("profiles", &self.profiles)
            .field("aliases", &self.aliases)
            .finish()
    }
}

impl ToolRegistry {
    /// Create an empty tool registry (no built-in tools registered).
    pub fn new_empty() -> Self {
        Self {
            tools: HashMap::new(),
            profiles: HashMap::new(),
            aliases: HashMap::new(),
            hooks: ToolHookRegistry::default(),
        }
    }

    /// Create a new tool registry and register all built-in tools.
    #[tracing::instrument(level = "info")]
    pub fn new() -> Self {
        let mut registry = Self::new_empty();
        registration::register_all(&mut registry);
        registry
    }

    pub fn register<T: Tool + 'static>(&mut self, tool: T) {
        self.register_with_profile(
            tool,
            ToolCapabilityProfile {
                capability: "custom".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 30_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );
    }

    pub fn register_with_profile<T: Tool + 'static>(
        &mut self,
        tool: T,
        profile: ToolCapabilityProfile,
    ) {
        let name = tool.name();
        // Auto-register with the governance gate so the tool is never
        // rejected as "unknown" — eliminates manual sync burden.
        crate::governance::status::register_tool(name);
        self.profiles.insert(name, profile);
        self.tools.insert(name, Arc::new(tool));
    }

    /// Remove a tool (and its profile) by name, returning `true` if the tool
    /// was registered. Aliases that pointed at the removed tool are dropped
    /// too, so they do not linger in `all_names()` or `profile()` lookups.
    ///
    /// The governance allowlist entry created by `register_with_profile` is
    /// intentionally left in place: `governance::status` exposes no removal
    /// API, and a stale allowlist entry only makes governance slightly more
    /// permissive — it never blocks a tool.
    pub fn unregister(&mut self, name: &str) -> bool {
        let removed = self.tools.remove(name).is_some();
        if removed {
            self.profiles.remove(name);
            self.aliases.retain(|_alias, canonical| *canonical != name);
        }
        removed
    }

    /// Register a tool and return a guard that unregisters it on drop (or
    /// explicit `rollback()`), so a failed plugin setup can never leave a
    /// half-registered tool behind (M1.6 / M4 plugin base).
    ///
    /// The guard's closure captures the tool's `'static` name plus a raw
    /// pointer to this registry. Because the closure must be `'static` (the
    /// guard can outlive the `&mut self` borrow), the caller must uphold the
    /// scoped-guard contract: **the guard must be dropped (or rolled back)
    /// before the registry itself is dropped or moved**. This is intended for
    /// long-lived registries with plugin-scoped guards.
    pub fn register_guarded(&mut self, tool: impl Tool + 'static) -> Result<RegistrationGuard> {
        let name = tool.name();
        self.register(tool);
        let this = std::ptr::from_mut(self);
        Ok(RegistrationGuard::new(move || {
            // SAFETY: per the contract above, the guard is dropped (or rolled
            // back) before the registry is dropped or moved, so `this` still
            // points at a live registry whenever the closure runs.
            unsafe { (&mut *this).unregister(name) };
        }))
    }

    /// Get a tool by name (with alias resolution) — O(1) via HashMap.
    #[tracing::instrument(level = "debug", skip(self))]
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        // Direct HashMap lookup first (O(1))
        if let Some(tool) = self.tools.get(name) {
            return Some(tool.as_ref());
        }
        // Alias resolution: look up the canonical name and find that tool
        self.aliases
            .get(name)
            .and_then(|canonical| self.tools.get(canonical))
            .map(|b| b.as_ref())
    }

    /// Get a tool by name (with alias resolution), returning an `Arc` for async usage — O(1) via HashMap.
    /// The returned `Arc` can be used to call `run_async` on the tool.
    pub fn get_arc(&self, name: &str) -> Option<Arc<dyn Tool>> {
        // Direct HashMap lookup first (O(1))
        if let Some(tool) = self.tools.get(name) {
            return Some(Arc::clone(tool));
        }
        // Alias resolution: look up the canonical name and find that tool
        self.aliases
            .get(name)
            .and_then(|canonical| self.tools.get(canonical))
            .map(Arc::clone)
    }

    pub fn names(&self) -> Vec<&'static str> {
        // Deterministic order: these feed LLM tool lists, MCP resources, and
        // the CLI system prompt — a HashMap iteration order would shuffle the
        // list on every process start.
        let mut names: Vec<&str> = self.tools.keys().copied().collect();
        names.sort_unstable();
        names
    }

    /// Return all tool names including aliases.
    pub fn all_names(&self) -> Vec<&'static str> {
        let mut names: Vec<&str> = self.tools.keys().copied().collect();
        names.extend(self.aliases.keys().copied());
        names.sort_unstable();
        names
    }

    /// Return names of tools with the given exposure level.
    /// Uses the Tool trait's `exposure()` method (which may differ from profile defaults).
    pub fn tools_by_exposure(&self, exposure: ToolExposure) -> Vec<&'static str> {
        let mut names: Vec<&str> = self
            .tools
            .iter()
            .filter_map(|(name, tool)| {
                if tool.exposure() == exposure {
                    Some(*name)
                } else {
                    None
                }
            })
            .collect();
        names.sort_unstable();
        names
    }

    /// Return names of deferred (search-discoverable) tools.
    pub fn deferred_tool_names(&self) -> Vec<&'static str> {
        self.tools_by_exposure(ToolExposure::Deferred)
    }

    /// Register an alias for a tool. When `alias` is looked up via `get()`,
    /// the tool registered under `canonical` name will be returned.
    ///
    /// This enables backward compatibility with legacy tool names that exist
    /// in the governance evaluator allowlist (e.g. "terminal" → "shell_exec").
    pub fn register_alias(&mut self, alias: &'static str, canonical: &'static str) {
        self.aliases.insert(alias, canonical);
    }

    /// Get the profile for a tool by name (with alias resolution).
    /// If `name` is an alias, returns the canonical tool's profile.
    pub fn profile(&self, name: &str) -> Option<&ToolCapabilityProfile> {
        let canonical = self.aliases.get(name).copied().unwrap_or(name);
        self.profiles.get(canonical)
    }

    pub fn capability_matrix(&self) -> serde_json::Value {
        let matrix = self
            .tools
            .iter()
            .filter_map(|(name, tool)| {
                self.profiles.get(name).map(|profile| {
                    serde_json::json!({
                        "name": name,
                        "exposure": tool.exposure(),
                        "capability": profile.capability,
                        "risk_level": profile.risk_level,
                        "timeout_budget_ms": profile.timeout_budget_ms,
                        "retry_policy": profile.retry_policy,
                        "fallback_chain": profile.fallback_chain,
                    })
                })
            })
            .collect::<Vec<_>>();
        serde_json::json!({ "tools": matrix })
    }

    /// Adapt the primary tool's input payload for a fallback tool.
    ///
    /// Most fallback pairs share the same input shape, but several expect a
    /// different field name — forwarding the raw input there makes the
    /// fallback fail with a missing-field error that masks the primary failure
    /// (e.g. read_file→search_files previously passed `{path}` where `pattern`
    /// is required, so the real "file not found" reason was never surfaced).
    fn adapt_fallback_input(primary: &str, fallback: &str, input: &ToolInput) -> ToolInput {
        let mut adapted = input.clone();
        let payload = match adapted.payload.as_object_mut() {
            Some(obj) => obj,
            None => return adapted,
        };
        match (primary, fallback) {
            // read_file {path} → search_files {pattern}: search for the file's
            // base name so a "file not found" failure surfaces candidate paths.
            ("read_file", "search_files") => {
                if let Some(path) = payload.get("path").cloned() {
                    let base = path
                        .as_str()
                        .and_then(|p| std::path::Path::new(p).file_name())
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    payload.remove("path");
                    payload.insert("pattern".to_string(), serde_json::json!(base));
                }
            }
            // code_index_search {query} / go_to_definition|find_references
            // {symbol} → grep {pattern}: the index/LSP backend failed, so fall
            // back to a plain text search for the same identifier.
            ("code_index_search" | "go_to_definition" | "find_references", "grep") => {
                let term = payload
                    .get("query")
                    .or_else(|| payload.get("symbol"))
                    .cloned();
                if let Some(term) = term {
                    payload.insert("pattern".to_string(), term);
                }
            }
            _ => {}
        }
        adapted
    }

    /// Run the fallback chain without governance hooks or metrics recording.
    ///
    /// Shared by [`ToolRegistry::run_with_fallback_async`] (which wraps this
    /// with pre/post hooks and execution metrics) and by
    /// `orchestration::tool::executor::run_tool_with_fallback`, whose caller
    /// (`execute_single_tool`) already runs the pre/post hooks itself — the
    /// former duplicate fallback loop in the executor was deleted to keep a
    /// single implementation.
    pub(crate) async fn run_fallback_chain_async(
        &self,
        name: &str,
        input: &ToolInput,
    ) -> Result<ToolOutput> {
        let Some(primary) = self.get_arc(name) else {
            anyhow::bail!("{}", tf("error.tool_not_found", &[("name", name)]));
        };

        let mut last_result = primary.run_async(input.clone()).await?;
        if last_result.success {
            return Ok(last_result);
        }

        for fb_name in self
            .profile(name)
            .map(|p| p.fallback_chain.clone())
            .unwrap_or_default()
        {
            if let Some(fb) = self.get_arc(&fb_name) {
                let fb_input = Self::adapt_fallback_input(name, &fb_name, input);
                let mut fb_result = fb.run_async(fb_input).await?;
                if fb_result.success {
                    fb_result.audit_log = Some(format!(
                        "primary '{name}' failed, fallback '{fb_name}' succeeded"
                    ));
                    return Ok(fb_result);
                }
                last_result = fb_result;
            }
        }
        Ok(last_result)
    }

    /// Run a tool asynchronously with fallback chain support.
    /// Uses `run_async` directly without `block_in_place` to comply with principle #23.
    ///
    /// This is the only fallback-chain entry point. A synchronous variant was
    /// previously maintained (lines ~1825-1888) but had zero production call
    /// sites and duplicated this async path line-for-line; it has been removed.
    ///
    /// The post-execute hooks run against the final result (primary success or
    /// the last fallback attempt) so they observe the outcome that is actually
    /// returned.
    #[tracing::instrument(level = "debug", skip(self, input), fields(tool = %name, success = false, latency_ms = 0u64, fallback_used = false))]
    pub async fn run_with_fallback_async(
        &self,
        name: &str,
        input: &ToolInput,
    ) -> Result<ToolOutput> {
        let start = std::time::Instant::now();

        let Some(_primary) = self.get_arc(name) else {
            let elapsed = start.elapsed().as_millis() as u64;
            tracing::warn!(target: "tool_execution", tool = %name, latency_ms = elapsed, "tool not found");
            anyhow::bail!("{}", tf("error.tool_not_found", &[("name", name)]));
        };

        // ── Pre-execute hooks (async) ───────
        self.hooks.run_pre_async(name, input).await?;

        let last_result = self.run_fallback_chain_async(name, input).await?;
        let elapsed = start.elapsed().as_millis() as u64;

        // ── Post-execute hooks ─────────────────────────────────────────
        self.hooks.run_post(name, input, &last_result, elapsed);

        record_tool_execution(
            "tool_execution_total",
            name,
            last_result.success,
            elapsed,
            serde_json::to_string(&input.payload).ok().map(|s| s.len()),
        );
        Ok(last_result)
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub use builtin_tools::{
    acquire_tool_write_lock, enforce_write_payload_size, enforce_write_sandbox,
    record_tool_execution, sanitize_path, sanitize_path_for_write, ApplyPatchTool,
    InspectGitDiffTool, ReadFileTool, RunTestsTool, SearchFilesTool, SkillCreateTool,
    SkillExecuteTool, SkillListTool, SkillReloadTool, WriteFileTool,
};

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::process::Command;
    use tempfile::tempdir;
    // Serializes against the startup_context chdir tests (see
    // run_tests_tool_executes_configured_command below).
    use serial_test::serial;

    fn init_git_repo(dir: &Path) {
        run_git(dir, &["init"]);
        run_git(dir, &["config", "user.email", "copilot@example.com"]);
        run_git(dir, &["config", "user.name", "Copilot Test"]);
        // Disable autocrlf to ensure consistent patch format across platforms
        run_git(dir, &["config", "core.autocrlf", "false"]);
    }

    fn run_git(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git command should spawn");
        assert!(
            output.status.success(),
            "git command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).to_string()
    }

    fn tool_input(payload: serde_json::Value) -> ToolInput {
        ToolInput {
            task_id: "test-task".to_string(),
            phase: "test".to_string(),
            agent_role: "tool".to_string(),
            objective: "tool test".to_string(),
            constraints: None,
            evidence: None,
            payload,
            allowed_base_dir: None,
        }
    }

    #[test]
    fn apply_patch_tool_checks_and_applies_patch() {
        let temp = tempdir().expect("tempdir should be created");
        init_git_repo(temp.path());

        let file_path = temp.path().join("sample.txt");
        fs::write(&file_path, "hello\n").expect("initial file should be written");
        run_git(temp.path(), &["add", "sample.txt"]);
        run_git(temp.path(), &["commit", "-m", "init"]);

        fs::write(&file_path, "hello world\n").expect("updated file should be written");
        let patch = run_git(temp.path(), &["diff", "--", "sample.txt"]);
        run_git(temp.path(), &["checkout", "--", "sample.txt"]);

        let tool = ApplyPatchTool;
        let checked = tool
            .run(&tool_input(serde_json::json!({
                "patch": patch,
                "check": true,
                "directory": temp.path().to_string_lossy().to_string(),
            })))
            .expect("patch check should succeed");
        assert!(checked.success);

        let applied = tool
            .run(&tool_input(serde_json::json!({
                "patch": patch,
                "directory": temp.path().to_string_lossy().to_string(),
            })))
            .expect("patch apply should succeed");
        assert!(applied.success);
        let normalized = fs::read_to_string(&file_path)
            .expect("patched file should be readable")
            .replace("\r\n", "\n");
        assert_eq!(normalized, "hello world\n");
    }

    #[test]
    #[serial]
    fn run_tests_tool_executes_configured_command() {
        // First check if git is available — skip if not (sandboxed CI
        // or parallel test execution with PATH changes). `git` inherits the
        // process CWD; the startup_context tests temporarily `set_current_dir`
        // into a temp dir that is deleted on drop, and a concurrent spawn
        // there fails — the shared `serial_test` lock serializes against those
        // chdir tests.
        match std::process::Command::new("git").arg("--version").output() {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("git not found in PATH, skipping test");
                return;
            }
            Err(e) => {
                eprintln!(
                    "git check failed with unexpected error: {}, skipping test",
                    e
                );
                return;
            }
            Ok(o) if !o.status.success() => {
                eprintln!("git --version returned non-zero, skipping test");
                return;
            }
            _ => {}
        }

        let tool = RunTestsTool;
        // The command runs through the bwrap sandbox; under heavy parallel
        // test load a namespace-setup failure can transiently fail an attempt
        // (the production executor retries tool failures — retry_policy
        // `max_retries: 1`). Retry a few times so a transient sandbox hiccup
        // does not flake the test; a persistent failure still fails it (the
        // last attempt's outcome is asserted, never skipped).
        let mut last_outcome: Option<ToolOutput> = None;
        for _attempt in 0..3 {
            match tool.run(&tool_input(serde_json::json!({
                "command": "git",
                "args": ["--version"],
                "directory": ".",
            }))) {
                Ok(r) if r.success => {
                    last_outcome = Some(r);
                    break;
                }
                Ok(r) => {
                    last_outcome = Some(r);
                }
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("No such file or directory") || msg.contains("not found") {
                        eprintln!(
                            "git binary not found during test execution, skipping: {}",
                            msg
                        );
                        return;
                    }
                    eprintln!("run_tests attempt failed (will retry): {}", msg);
                }
            }
        }
        let result = last_outcome.unwrap_or_else(|| {
            panic!("command should execute; all retry attempts errored");
        });
        assert!(result.success);
        let stdout = result.result.expect("result should exist")["stdout"]
            .as_str()
            .expect("stdout should be string")
            .to_string();
        assert!(stdout.contains("git version"));
    }

    #[test]
    fn inspect_git_diff_tool_returns_actual_diff() {
        let temp = tempdir().expect("tempdir should be created");
        init_git_repo(temp.path());

        let file_path = temp.path().join("sample.txt");
        fs::write(&file_path, "hello\n").expect("initial file should be written");
        run_git(temp.path(), &["add", "sample.txt"]);
        run_git(temp.path(), &["commit", "-m", "init"]);
        fs::write(&file_path, "hello world\n").expect("updated file should be written");

        let tool = InspectGitDiffTool;
        let result = tool
            .run(&tool_input(serde_json::json!({
                "directory": temp.path().to_string_lossy().to_string(),
                "files": ["sample.txt"],
            })))
            .expect("git diff should execute");
        assert!(result.success);
        let diff = result.result.expect("result should exist")["diff"]
            .as_str()
            .expect("diff should be string")
            .to_string();
        assert!(diff.contains("hello world"));
    }

    struct AlwaysFailTool;
    impl Tool for AlwaysFailTool {
        fn name(&self) -> &'static str {
            "always_fail"
        }

        fn run(&self, _input: &ToolInput) -> Result<ToolOutput> {
            Ok(ToolOutput {
                success: false,
                result: None,
                error: Some("forced failure".to_string()),
                verification: Some("forced_failure".to_string()),
                audit_log: Some("always_fail executed".to_string()),
                pua_report: None,
            })
        }
    }

    struct AlwaysPassTool;
    impl Tool for AlwaysPassTool {
        fn name(&self) -> &'static str {
            "always_pass"
        }

        fn run(&self, _input: &ToolInput) -> Result<ToolOutput> {
            Ok(ToolOutput {
                success: true,
                result: Some(serde_json::json!({"ok": true})),
                error: None,
                verification: Some("forced_success".to_string()),
                audit_log: Some("always_pass executed".to_string()),
                pua_report: None,
            })
        }
    }

    #[tokio::test]
    async fn tool_registry_runs_fallback_chain_when_primary_fails() {
        let mut registry = ToolRegistry {
            tools: HashMap::new(),
            profiles: HashMap::new(),
            aliases: HashMap::new(),
            hooks: Default::default(),
        };
        registry.register_with_profile(
            AlwaysFailTool,
            ToolCapabilityProfile {
                capability: "primary".to_string(),
                risk_level: ToolRiskLevel::Medium,
                timeout_budget_ms: 1_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: vec!["always_pass".to_string()],
            },
        );
        registry.register_with_profile(
            AlwaysPassTool,
            ToolCapabilityProfile {
                capability: "fallback".to_string(),
                risk_level: ToolRiskLevel::Low,
                timeout_budget_ms: 1_000,
                retry_policy: RetryPolicy {
                    max_retries: 0,
                    retry_on_failure: false,
                },
                fallback_chain: Vec::new(),
            },
        );

        let output = registry
            .run_with_fallback_async("always_fail", &tool_input(serde_json::json!({})))
            .await
            .expect("fallback execution should succeed");
        assert!(output.success);
        let audit_log = output.audit_log.unwrap_or_default();
        assert!(audit_log.contains("fallback"));
    }

    #[test]
    fn test_no_duplicate_tool_names() {
        let registry = ToolRegistry::new();
        let names = registry.names();
        let mut seen = std::collections::HashSet::new();
        for name in &names {
            assert!(
                seen.insert(name),
                "Duplicate tool name: {name}\nAll names: {names:#?}",
            );
        }
    }

    struct DummyGuardedTool;
    impl Tool for DummyGuardedTool {
        fn name(&self) -> &'static str {
            "dummy_guarded"
        }

        fn run(&self, _input: &ToolInput) -> Result<ToolOutput> {
            Ok(ToolOutput {
                success: true,
                result: None,
                error: None,
                verification: None,
                audit_log: None,
                pua_report: None,
            })
        }
    }

    #[test]
    fn register_guarded_removes_tool_on_drop() {
        let mut registry = ToolRegistry::new_empty();
        let guard = registry
            .register_guarded(DummyGuardedTool)
            .expect("registration should succeed");
        assert!(registry.get("dummy_guarded").is_some());
        drop(guard);
        assert!(registry.get("dummy_guarded").is_none());
    }

    #[test]
    fn unregister_returns_false_for_unknown_tool() {
        let mut registry = ToolRegistry::new_empty();
        assert!(!registry.unregister("never_registered_tool"));
    }

    #[test]
    fn unregister_removes_tool_profile_and_aliases() {
        let mut registry = ToolRegistry::new_empty();
        registry.register(DummyGuardedTool);
        registry.register_alias("dummy_alias", "dummy_guarded");
        assert!(registry.get("dummy_alias").is_some());
        assert!(registry.unregister("dummy_guarded"));
        assert!(registry.get("dummy_guarded").is_none());
        assert!(registry.get("dummy_alias").is_none());
        assert!(!registry.all_names().contains(&"dummy_alias"));
        // Second unregister is a no-op.
        assert!(!registry.unregister("dummy_guarded"));
    }

    #[test]
    fn rollback_unregisters_tool_before_drop() {
        let mut registry = ToolRegistry::new_empty();
        let guard = registry
            .register_guarded(DummyGuardedTool)
            .expect("registration should succeed");
        assert!(registry.get("dummy_guarded").is_some());
        guard.rollback();
        // The tool is gone before the guard is dropped; a second unregister
        // returns false, confirming no double-removal.
        assert!(registry.get("dummy_guarded").is_none());
        assert!(!registry.unregister("dummy_guarded"));
    }
}
