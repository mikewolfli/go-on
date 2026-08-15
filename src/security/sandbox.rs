//! OS-level command sandboxing (bubblewrap) — LAYER 3 isolation.
//!
//! go-on's tool loop previously ran every model-issued command directly inside
//! the server process with full user privileges. The only protection was the
//! text-level blacklist in `exec_common::is_blocked_command`, which can be
//! bypassed by command-text tricks, and which cannot express filesystem or
//! network containment at all.
//!
//! This module closes that gap: when `bwrap` (bubblewrap) is present on Linux,
//! command execution is wrapped in a new user + pid namespace whose root is an
//! EMPTY tmpfs — only the workspace, `$HOME` (workspace-write), the platform
//! runtime dirs and a fresh `/dev` are mounted, so the host filesystem is
//! neither readable nor writable by default. Modes:
//!
//! - `workspace-write` (default when bwrap is available): workspace + `$HOME`
//!   writable, runtime read-only, `/tmp` tmpfs, credential dirs under `$HOME`
//!   masked by empty tmpfs mounts, network enabled. Preserves normal dev
//!   workflows (`cargo`/`npm` caches live under `$HOME`).
//! - `read-only`: workspace and `$HOME` visible read-only, network disabled.
//! - `isolated`: workspace read-only and no `$HOME` at all — no user data is
//!   reachable; network disabled.
//! - `none`: legacy direct execution (policy gates still apply).
//!
//! The empty-root mount order is deliberate: when the host root is bound
//! (`--ro-bind / /`), bwrap's propagation-fixup pass remounts the inherited
//! `/dev` with `MS_NODEV`, making device nodes unusable in unprivileged
//! sandboxes. An empty root hides the host mounts from that pass, so a fresh
//! `--dev /dev` works — and containment is stricter (host paths are absent
//! rather than read-only).
//!
//! Overhead is one `bwrap` spawn per command (~2-4ms); containment is enforced
//! by the kernel namespace/mount machinery, not by pattern matching. When
//! bwrap is missing or its namespaces/devices are unavailable (verified by a
//! real probe), execution degrades to direct with a warning.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// OS-isolation mode for model-issued commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxMode {
    /// No OS sandbox — legacy direct execution.
    None,
    /// Workspace (+ $HOME + configured roots) writable, rest read-only,
    /// network enabled, credential dirs masked.
    WorkspaceWrite,
    /// Everything read-only, `/tmp` tmpfs, network disabled.
    ReadOnly,
    /// Empty tmpfs root, workspace read-only, network disabled.
    Isolated,
}

impl SandboxMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().replace('_', "-").as_str() {
            "none" => Some(SandboxMode::None),
            "workspace-write" | "workspace" | "basic" => Some(SandboxMode::WorkspaceWrite),
            "read-only" | "readonly" | "strict" => Some(SandboxMode::ReadOnly),
            "isolated" => Some(SandboxMode::Isolated),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            SandboxMode::None => "none",
            SandboxMode::WorkspaceWrite => "workspace-write",
            SandboxMode::ReadOnly => "read-only",
            SandboxMode::Isolated => "isolated",
        }
    }
}

/// Resolved sandbox settings (from `[security.command_sandbox]`).
#[derive(Debug, Clone)]
pub struct SandboxConfig {
    pub mode: SandboxMode,
    /// Extra writable roots beyond the command's working directory.
    pub workspace: Option<PathBuf>,
    /// Credential directory names under `$HOME` to mask in workspace-write
    /// mode. Empty means "use defaults".
    pub masked_dirs: Vec<String>,
    /// Environment variable names that are allowed to leak into a sandboxed
    /// command even if they match the credential patterns.
    pub passthrough_env: Vec<String>,
}

/// Default credential directories masked when `$HOME` is writable.
pub fn default_masked_dirs() -> Vec<String> {
    vec![
        ".ssh".to_string(),
        ".aws".to_string(),
        ".gnupg".to_string(),
        ".netrc".to_string(),
        ".git-credentials".to_string(),
        ".npmrc".to_string(),
        ".docker".to_string(),
        ".kube".to_string(),
        ".pgpass".to_string(),
        ".config/gcloud".to_string(),
        ".config/gh".to_string(),
        ".config/rclone".to_string(),
        ".config/op".to_string(),
    ]
}

static COMMAND_SANDBOX: OnceLock<Mutex<Option<SandboxConfig>>> = OnceLock::new();

// ── Observability counters (exposed via `sandbox_counters`) ──────────────
static SANDBOX_APPLIED_TOTAL: AtomicU64 = AtomicU64::new(0);
static SANDBOX_DEGRADED_TOTAL: AtomicU64 = AtomicU64::new(0);
static SANDBOX_SKIPPED_ROOT_TOTAL: AtomicU64 = AtomicU64::new(0);
static SANDBOX_ENV_SCRUBBED_TOTAL: AtomicU64 = AtomicU64::new(0);

pub fn record_sandbox_applied() {
    SANDBOX_APPLIED_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub fn record_sandbox_degraded() {
    SANDBOX_DEGRADED_TOTAL.fetch_add(1, Ordering::Relaxed);
}

pub fn record_sandbox_skipped_root() {
    SANDBOX_SKIPPED_ROOT_TOTAL.fetch_add(1, Ordering::Relaxed);
}

/// Live sandbox counters for the governance/debug surfaces.
pub fn sandbox_counters() -> serde_json::Value {
    serde_json::json!({
        "applied_total": SANDBOX_APPLIED_TOTAL.load(Ordering::Relaxed),
        "degraded_total": SANDBOX_DEGRADED_TOTAL.load(Ordering::Relaxed),
        "skipped_root_total": SANDBOX_SKIPPED_ROOT_TOTAL.load(Ordering::Relaxed),
        "env_vars_scrubbed_total": SANDBOX_ENV_SCRUBBED_TOTAL.load(Ordering::Relaxed),
        "bwrap_available": bwrap_available(),
        "mode": effective_mode().as_str(),
    })
}

/// Inject the sandbox config at server startup (mirrors `init_url_policy`).
/// No-op when called more than once (first writer wins).
pub fn init_command_sandbox(cfg: Option<crate::config::types::CommandSandboxConfig>) {
    let resolved = cfg.map(|c| SandboxConfig {
        mode: {
            let raw = c.mode.as_deref();
            let parsed = raw.and_then(SandboxMode::parse);
            if raw.is_some() && parsed.is_none() {
                tracing::error!(
                    raw = raw.unwrap_or(""),
                    "invalid command_sandbox.mode — falling back to workspace-write (set mode = \"none\" to disable)"
                );
            }
            parsed.unwrap_or(SandboxMode::WorkspaceWrite)
        },
        workspace: c.workspace.map(PathBuf::from),
        masked_dirs: c
            .masked_dirs
            .filter(|m| !m.is_empty())
            .unwrap_or_else(default_masked_dirs),
        passthrough_env: c.passthrough_env.unwrap_or_default(),
    });
    let store = COMMAND_SANDBOX.get_or_init(|| Mutex::new(None));
    let mut guard = store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.is_none() {
        // Log the mode that will actually apply (the default `workspace-write`
        // kicks in when no config is injected). bwrap availability is detected
        // lazily on the first wrapped command — NOT here — so startup stays
        // free of extra subprocess spawning.
        let effective = resolved
            .as_ref()
            .map(|c| c.mode)
            .unwrap_or(SandboxMode::WorkspaceWrite);
        tracing::info!(
            mode = effective.as_str(),
            explicit = resolved.is_some(),
            "command sandbox configured (bwrap detected lazily on first command)"
        );
        *guard = resolved;
    }
}

/// Current sandbox config, if explicitly set.
pub fn sandbox_config() -> Option<SandboxConfig> {
    let store = COMMAND_SANDBOX.get()?;
    let guard = store
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.clone()
}

/// Effective isolation mode. When no config was injected, default to
/// `workspace-write` so unconfigured installs still get containment whenever
/// bwrap is available (matching Codex's default sandbox).
pub fn effective_mode() -> SandboxMode {
    sandbox_config()
        .map(|c| c.mode)
        .unwrap_or(SandboxMode::WorkspaceWrite)
}

/// Whether `bwrap` is available and functional (cached; Linux only).
pub fn bwrap_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        #[cfg(target_os = "linux")]
        {
            Command::new("bwrap")
                .arg("--version")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = Command::new("true").status();
            false
        }
    })
}

/// Environment variable names never passed into a sandboxed command, even
/// when the whole inherited environment is otherwise preserved. Payload-issued
/// env vars in the tool call are still honored explicitly.
pub fn credential_env_names() -> &'static [&'static str] {
    &[
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "DEEPSEEK_API_KEY",
        "GITHUB_TOKEN",
        "GITHUB_COPILOT_TOKEN",
        "HF_TOKEN",
        "HUGGING_FACE_HUB_TOKEN",
        "AWS_ACCESS_KEY_ID",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
        "GOOGLE_API_KEY",
        "AZURE_OPENAI_API_KEY",
        "AZURE_OPENAI_KEY",
    ]
}

/// Substrings (matched case-insensitively) that mark a variable as
/// credential-bearing and therefore blocked from sandboxed commands.
const CREDENTIAL_SUBSTRINGS: &[&str] = &[
    "API_KEY",
    "API_TOKEN",
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "PRIVATE_KEY",
];

/// Whether a single environment variable name is credential-bearing and
/// therefore blocked from sandboxed commands (unless listed in `passthrough`).
/// Pure function — unit-testable without touching the process environment.
pub fn is_credential_env(name: &str, passthrough: &[String]) -> bool {
    if passthrough.iter().any(|p| p.eq_ignore_ascii_case(name)) {
        return false;
    }
    let upper = name.to_ascii_uppercase();
    credential_env_names()
        .iter()
        .any(|d| d.eq_ignore_ascii_case(name))
        || CREDENTIAL_SUBSTRINGS.iter().any(|s| upper.contains(s))
}

/// Filter the inherited environment for sandboxed commands: drops credential
/// variables (unless explicitly listed in `passthrough`) so `printenv` inside
/// the sandbox cannot leak API keys or tokens to a model-issued command.
pub fn sanitized_env(passthrough: &[String]) -> Vec<(String, String)> {
    let vars: Vec<(String, String)> = std::env::vars().collect();
    let kept: Vec<(String, String)> = vars
        .iter()
        .filter(|(k, _)| !is_credential_env(k, passthrough))
        .cloned()
        .collect();
    let dropped = vars.len().saturating_sub(kept.len());
    if dropped > 0 {
        SANDBOX_ENV_SCRUBBED_TOTAL.fetch_add(dropped as u64, Ordering::Relaxed);
    }
    kept
}

/// Whether `bwrap` can actually produce a working sandbox (cached). Stricter
/// than [`bwrap_available`]: it runs a real probe sandbox mirroring the
/// production argv shape and verifies BOTH that the namespaces can be created
/// AND that device nodes (`/dev/null`) are writable inside. The device check
/// matters because on some unprivileged-bwrap setups the propagation-fixup
/// pass remounts an inherited `/dev` with `MS_NODEV`, which would silently
/// break every redirected command in the sandbox. When this probe fails,
/// sandboxing degrades to direct execution with a warning.
pub fn bwrap_probe_works() -> bool {
    static WORKS: OnceLock<bool> = OnceLock::new();
    *WORKS.get_or_init(|| {
        if !bwrap_available() {
            return false;
        }
        // Mirror the real sandbox: empty tmpfs root, fresh /dev, platform
        // read-only binds, then verify a device write works.
        let mut argv: Vec<String> = vec![
            "--unshare-user".into(),
            "--unshare-pid".into(),
            "--die-with-parent".into(),
            "--new-session".into(),
            "--tmpfs".into(),
            "/".into(),
            "--proc".into(),
            "/proc".into(),
            "--dev".into(),
            "/dev".into(),
            "--tmpfs".into(),
            "/tmp".into(),
        ];
        for dir in PLATFORM_RO_BINDS {
            if Path::new(dir).exists() {
                argv.extend(["--ro-bind".into(), (*dir).into(), (*dir).into()]);
            }
        }
        argv.extend(["--chdir".into(), "/".into()]);
        argv.extend(["sh".into(), "-c".into(), "echo probe > /dev/null".into()]);
        Command::new("bwrap")
            .args(&argv)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

/// Host directories re-mounted read-only into the sandbox so the runtime
/// (shells, compilers, package managers, DNS) keeps working. Only these are
/// visible beyond the workspace/`$HOME` — everything else on the host is
/// hidden, which is stricter than a blanket `--ro-bind / /`.
const PLATFORM_RO_BINDS: &[&str] = &[
    "/usr",
    "/lib",
    "/lib64",
    "/bin",
    "/sbin",
    "/etc",
    "/opt",
    "/snap",
    "/nix/store",
];

/// Build the bwrap argv prefix for a given mode. Pure function — unit-testable.
///
/// Mount strategy (security-critical): start from an EMPTY tmpfs root, then
/// mount only what is needed. This is required for two reasons:
/// 1. Containment: nothing from the host is visible unless explicitly mounted
///    (stricter than `--ro-bind / /`, which exposes /home, /var, /root, ...).
/// 2. Correctness: when the host root is bound, bwrap's propagation-fixup pass
///    remounts the inherited `/dev` with `MS_NODEV`, making device nodes
///    (null, urandom, ...) unusable in unprivileged sandboxes. An empty root
///    hides the host mounts from that pass, so a fresh `--dev /dev` works.
fn build_bwrap_argv(
    mode: SandboxMode,
    workspace: &Path,
    home: Option<&Path>,
    extra_writable: &[PathBuf],
    masked_dirs: &[String],
) -> Vec<String> {
    let mut argv: Vec<String> = vec![
        "--unshare-user".into(),
        "--unshare-pid".into(),
        "--die-with-parent".into(),
        "--new-session".into(),
        // Empty root FIRST — see doc comment above.
        "--tmpfs".into(),
        "/".into(),
        "--proc".into(),
        "/proc".into(),
        "--dev".into(),
        "/dev".into(),
        "--tmpfs".into(),
        "/tmp".into(),
    ];
    // Runtime read-only binds (skip anything that does not exist on the host).
    for dir in PLATFORM_RO_BINDS {
        if Path::new(dir).exists() {
            argv.extend(["--ro-bind".into(), (*dir).into(), (*dir).into()]);
        }
    }
    // systemd-resolved keeps /etc/resolv.conf as a symlink into /run; mount
    // that too so DNS works inside the sandbox.
    if Path::new("/run/systemd/resolve").exists() {
        argv.extend([
            "--ro-bind".into(),
            "/run/systemd/resolve".into(),
            "/run/systemd/resolve".into(),
        ]);
    }
    match mode {
        SandboxMode::None => {}
        SandboxMode::WorkspaceWrite => {
            // Re-enable writes per writable root.
            let mut writable: Vec<PathBuf> = vec![workspace.to_path_buf()];
            writable.extend(extra_writable.iter().cloned());
            if let Ok(cwd) = std::env::current_dir() {
                writable.push(cwd);
            }
            // Dedup while preserving order.
            let mut seen = std::collections::HashSet::new();
            for root in writable {
                if seen.insert(root.clone()) {
                    argv.extend([
                        "--bind".into(),
                        root.to_string_lossy().into_owned(),
                        root.to_string_lossy().into_owned(),
                    ]);
                }
            }
            // Writable $HOME for caches, with credential dirs masked on top:
            // each masked dir is replaced by an empty tmpfs, so the model can
            // neither read nor write real keys under it.
            if let Some(home) = home {
                argv.extend([
                    "--bind".into(),
                    home.to_string_lossy().into_owned(),
                    home.to_string_lossy().into_owned(),
                ]);
                for dir in masked_dirs {
                    let p = std::path::Path::new(dir);
                    // Defense in depth (the wrapper also warns): an absolute or
                    // `..`-containing entry would make home.join(dir) escape
                    // $HOME and tmpfs-overlay an unrelated host path. Skip.
                    if p.is_absolute()
                        || p.components()
                            .any(|c| matches!(c, std::path::Component::ParentDir))
                    {
                        continue;
                    }
                    let masked = home.join(dir);
                    if masked.exists() {
                        argv.extend(["--tmpfs".into(), masked.to_string_lossy().into_owned()]);
                    }
                }
            }
        }
        SandboxMode::ReadOnly => {
            // Workspace and $HOME visible read-only; no writable binds.
            argv.extend([
                "--ro-bind".into(),
                workspace.to_string_lossy().into_owned(),
                workspace.to_string_lossy().into_owned(),
            ]);
            if let Some(home) = home {
                argv.extend([
                    "--ro-bind".into(),
                    home.to_string_lossy().into_owned(),
                    home.to_string_lossy().into_owned(),
                ]);
            }
            argv.extend(["--unshare-net".into()]);
        }
        SandboxMode::Isolated => {
            // Runtime + workspace (read-only) only. No $HOME at all, so no
            // user data is reachable; network disabled.
            argv.extend([
                "--ro-bind".into(),
                workspace.to_string_lossy().into_owned(),
                workspace.to_string_lossy().into_owned(),
            ]);
            argv.extend(["--unshare-net".into()]);
        }
    }
    argv.extend(["--chdir".into(), workspace.to_string_lossy().into_owned()]);
    argv
}

/// Result of wrapping a command for OS-level isolation.
pub struct WrappedCommand {
    /// Program to execute (either the original or `bwrap`).
    pub program: String,
    /// Full argv (bwrap prefix included when `applied`).
    pub args: Vec<String>,
    /// Whether OS isolation is active for this invocation.
    pub applied: bool,
    /// The mode that produced this wrapper.
    pub mode: SandboxMode,
}

/// Wrap `program args` in a bwrap sandbox when the effective mode requires it
/// and bwrap is available. Otherwise returns the command unchanged.
pub fn wrap_command(
    mode: SandboxMode,
    workspace: &Path,
    program: &str,
    args: &[String],
) -> WrappedCommand {
    let unavailable = mode != SandboxMode::None && !bwrap_probe_works();
    // Read the config once: it is cloned under a lock, so every access costs
    // a lock + clone of the whole struct.
    let config = sandbox_config();

    // Guard: a writable root of `/` would emit `--bind / /` after
    // `--ro-bind / /`, silently making the whole filesystem writable — i.e. no
    // sandbox at all. This covers every root the wrapper binds: the workspace,
    // the configured extra root, `$HOME`, and the server's working directory.
    let root_writable = mode != SandboxMode::None
        && (workspace == Path::new("/")
            || config
                .as_ref()
                .and_then(|c| c.workspace.as_ref())
                .is_some_and(|w| w == Path::new("/"))
            || std::env::var_os("HOME")
                .map(|h| h == Path::new("/"))
                .unwrap_or(false)
            || std::env::current_dir()
                .map(|c| c == Path::new("/"))
                .unwrap_or(false));

    if mode == SandboxMode::None || unavailable || root_writable {
        if root_writable {
            record_sandbox_skipped_root();
        }
        if unavailable || root_writable {
            static WARNED: OnceLock<()> = OnceLock::new();
            if WARNED.get().is_none() {
                let _ = WARNED.set(());
                if root_writable {
                    tracing::warn!(
                        mode = mode.as_str(),
                        "command sandbox skipped: a writable root resolves to the filesystem root (would disable containment)"
                    );
                } else {
                    tracing::warn!(
                        mode = mode.as_str(),
                        "command sandbox requested but bwrap is unavailable — falling back to direct execution"
                    );
                }
            }
        }
        return WrappedCommand {
            program: program.to_string(),
            args: args.to_vec(),
            applied: false,
            mode,
        };
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let extra_writable: Vec<PathBuf> = config
        .as_ref()
        .and_then(|c| c.workspace.clone())
        .into_iter()
        .collect();
    let masked = config
        .as_ref()
        .map(|c| c.masked_dirs.clone())
        .filter(|m| !m.is_empty())
        .unwrap_or_else(default_masked_dirs);
    // Misconfigured mask entries (absolute paths / `..` segments) are skipped
    // by the argv builder; surface the misconfiguration once instead of
    // silently ignoring it.
    let bad_masks: Vec<&String> = masked
        .iter()
        .filter(|m| {
            let p = Path::new(m);
            p.is_absolute()
                || p.components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
        })
        .collect();
    if !bad_masks.is_empty() {
        static MASK_WARNED: OnceLock<()> = OnceLock::new();
        if MASK_WARNED.get().is_none() {
            let _ = MASK_WARNED.set(());
            tracing::warn!(
                entries = ?bad_masks,
                "command_sandbox.masked_dirs contains absolute/.. paths — those entries are ignored (must be $HOME-relative names)"
            );
        }
    }
    let mut argv = build_bwrap_argv(mode, workspace, home.as_deref(), &extra_writable, &masked);
    // The wrapped program goes first, then its args — bwrap execs the first
    // non-option argument, so omitting `program` here made bwrap exec the
    // first ARG instead (e.g. a timeout value), failing every sandboxed run.
    argv.push(program.to_string());
    argv.extend(args.iter().cloned());
    record_sandbox_applied();
    // Surface the actually-applied mode on first use — with no explicit config
    // the effective mode defaults to workspace-write, and this log makes that
    // implicit default observable instead of silent.
    static APPLIED_INFO: OnceLock<()> = OnceLock::new();
    if APPLIED_INFO.get().is_none() {
        let _ = APPLIED_INFO.set(());
        tracing::info!(
            mode = mode.as_str(),
            workspace = %workspace.display(),
            "command sandbox active for model-issued commands"
        );
    }
    WrappedCommand {
        program: "bwrap".to_string(),
        args: argv,
        applied: true,
        mode,
    }
}

/// Result of preparing a command for sandboxed execution — the program/args a
/// caller should spawn plus how to configure it.
pub struct PreparedCommand {
    /// Program to spawn (either the original or `bwrap`).
    pub program: String,
    /// Full argv (bwrap prefix included when `applied`).
    pub args: Vec<String>,
    /// Whether OS isolation is active for this invocation.
    pub applied: bool,
    /// The mode that produced this wrapper.
    pub mode: SandboxMode,
    /// Whether credential env vars should be scrubbed from the spawned command
    /// (true whenever a sandbox mode is requested, even when bwrap is
    /// unavailable and execution degrades to direct: env leakage is
    /// independent of filesystem containment).
    pub scrub_env: bool,
}

/// Prepare `program args` for sandboxed execution: applies the bwrap wrapper
/// when the effective mode requires it and bwrap works, and decides env
/// scrubbing. Callers build `Command::new(prepared.program)` with
/// `prepared.args` and apply `scrub_env`.
pub fn prepare_command(
    mode: SandboxMode,
    workspace: &Path,
    program: &str,
    args: &[String],
) -> PreparedCommand {
    let wrapped = wrap_command(mode, workspace, program, args);
    PreparedCommand {
        program: wrapped.program,
        args: wrapped.args,
        applied: wrapped.applied,
        mode: wrapped.mode,
        scrub_env: wrapped.mode != SandboxMode::None,
    }
}

/// bwrap "spawned fine but could not set up the namespace" signature: exit
/// status 1 with a `bwrap:` stderr prefix. In that case the inner command
/// never ran, so it is safe (and necessary) to retry without the sandbox.
pub fn is_bwrap_setup_failure(status: Option<i32>, stderr: &str) -> bool {
    status == Some(1)
        && stderr
            .lines()
            .next()
            .is_some_and(|l| l.starts_with("bwrap:"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp_home(mask_dir: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let secret = home.join(mask_dir);
        std::fs::create_dir_all(&secret).unwrap();
        std::fs::write(secret.join("key.pem"), "TOP-SECRET").unwrap();
        (dir, home, secret)
    }

    #[test]
    fn workspace_write_binds_workspace_and_masks_credentials() {
        let (dir, home, secret) = tmp_home(".ssh");
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let argv = build_bwrap_argv(
            SandboxMode::WorkspaceWrite,
            &ws,
            Some(&home),
            &[],
            &[".ssh".to_string()],
        );
        let joined = argv.join(" ");
        // Empty tmpfs root (hides host mounts from the NODEV fixup pass), a
        // fresh /dev, and at least one platform runtime bind must be present.
        assert!(joined.contains("--tmpfs /"));
        assert!(joined.contains("--dev /dev"));
        assert!(joined.contains("--ro-bind /usr /usr"));
        assert!(joined.contains(&format!("--bind {} {}", ws.display(), ws.display())));
        assert!(joined.contains(&format!("--bind {} {}", home.display(), home.display())));
        assert!(joined.contains(&format!("--tmpfs {}", secret.display())));
        assert!(joined.contains("--tmpfs /tmp"));
        assert!(
            !joined.contains("--unshare-net"),
            "workspace-write keeps network"
        );
        assert!(joined.contains(&format!("--chdir {}", ws.display())));
    }

    #[test]
    fn read_only_disables_network_and_writes() {
        let dir = tempfile::TempDir::new().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let argv = build_bwrap_argv(SandboxMode::ReadOnly, &ws, None, &[], &[]);
        let joined = argv.join(" ");
        assert!(joined.contains("--unshare-net"));
        assert!(joined.contains("--tmpfs /"));
        // Workspace is re-exposed read-only.
        assert!(joined.contains(&format!("--ro-bind {} {}", ws.display(), ws.display())));
        assert!(
            !joined.contains("--bind"),
            "read-only has no writable binds"
        );
    }

    #[test]
    fn isolated_masks_host_state_and_readonly_workspace() {
        let dir = tempfile::TempDir::new().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let argv = build_bwrap_argv(SandboxMode::Isolated, &ws, None, &[], &[]);
        let joined = argv.join(" ");
        // Empty root hides the whole host; only runtime + workspace are mounted.
        assert!(joined.contains("--tmpfs /"));
        assert!(joined.contains("--dev /dev"));
        assert!(joined.contains(&format!("--ro-bind {} {}", ws.display(), ws.display())));
        assert!(joined.contains("--unshare-net"));
        // No $HOME exposure at all in isolated mode.
        assert!(!joined.contains("--bind"), "isolated has no writable binds");
        assert!(
            !joined.contains("/home"),
            "isolated must not mount /home (workspace may live elsewhere)"
        );
    }

    #[test]
    fn wrap_command_appends_program_before_args() {
        // Regression: the wrapped program must be the first non-option argv
        // element for bwrap; previously `program` was dropped and bwrap tried
        // to exec the first ARG (e.g. a timeout value) — execvp failure.
        let dir = tempfile::TempDir::new().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let wrapped = wrap_command(
            SandboxMode::WorkspaceWrite,
            &ws,
            "timeout",
            &[
                "10".to_string(),
                "sh".to_string(),
                "-c".to_string(),
                "true".to_string(),
            ],
        );
        // bwrap only exists on Linux; on hosts where the probe fails the
        // wrapper must honestly degrade to direct execution (asserted by
        // `wrap_command_none_mode_never_applies` for None mode). The argv
        // ordering regression below is only observable when the sandbox is
        // actually applied.
        if !wrapped.applied {
            assert!(!bwrap_probe_works());
            return;
        }
        assert_eq!(wrapped.program, "bwrap");
        let chdir_idx = wrapped
            .args
            .windows(2)
            .position(|w| w == ["--chdir", ws.to_string_lossy().as_ref()])
            .expect("--chdir present");
        assert_eq!(
            wrapped.args[chdir_idx + 2],
            "timeout",
            "wrapped program must follow the bwrap options: {:?}",
            wrapped.args
        );
        assert_eq!(wrapped.args[chdir_idx + 3], "10");
    }

    #[test]
    fn wrap_command_none_mode_never_applies() {
        let dir = tempfile::TempDir::new().unwrap();
        let ws = dir.path().join("ws");
        let wrapped = wrap_command(
            SandboxMode::None,
            &ws,
            "sh",
            &["-c".to_string(), "echo hi".to_string()],
        );
        assert!(!wrapped.applied);
        assert_eq!(wrapped.program, "sh");
    }

    #[test]
    fn sanitized_env_drops_credentials_but_keeps_innocuous() {
        // Deny list covers the well-known credential names.
        let deny = credential_env_names();
        assert!(deny
            .iter()
            .any(|d| d.eq_ignore_ascii_case("OPENAI_API_KEY")));
        assert!(deny.iter().any(|d| d.eq_ignore_ascii_case("GITHUB_TOKEN")));
        // Substring rules cover variants not in the exact list, and leave
        // innocuous variables alone.
        for (k, blocked) in [
            ("MY_CUSTOM_API_KEY", true),
            ("DATABASE_PASSWORD", true),
            ("FOO_SECRET_BAR", true),
            ("PATH", false),
            ("CARGO_HOME", false),
        ] {
            assert_eq!(is_credential_env(k, &[]), blocked, "{k}");
        }
        // Passthrough beats deny (case-insensitive).
        assert!(!is_credential_env(
            "MY_CUSTOM_API_KEY",
            &["my_custom_api_key".to_string()]
        ));
        assert!(!is_credential_env(
            "GITHUB_TOKEN",
            &["GITHUB_TOKEN".to_string()]
        ));
        // Case-insensitive deny matching.
        assert!(is_credential_env("github_token", &[]));
    }

    #[test]
    fn wrap_command_skips_root_workspace() {
        // `--bind / /` after `--ro-bind / /` would disable containment — the
        // wrapper must refuse to apply rather than give a false sandbox.
        let wrapped = wrap_command(
            SandboxMode::WorkspaceWrite,
            Path::new("/"),
            "sh",
            &["-c".to_string(), "echo hi".to_string()],
        );
        assert!(
            !wrapped.applied,
            "root workspace must never be sandbox-wrapped"
        );
        assert_eq!(wrapped.program, "sh");
    }

    #[test]
    fn mode_parse_accepts_aliases() {
        assert_eq!(SandboxMode::parse("none"), Some(SandboxMode::None));
        assert_eq!(
            SandboxMode::parse("workspace-write"),
            Some(SandboxMode::WorkspaceWrite)
        );
        assert_eq!(SandboxMode::parse("read-only"), Some(SandboxMode::ReadOnly));
        assert_eq!(SandboxMode::parse("isolated"), Some(SandboxMode::Isolated));
        assert_eq!(SandboxMode::parse("bogus"), None);
    }

    #[test]
    fn argv_mount_order_is_safe() {
        // Mount order is security-critical: the empty tmpfs root must come
        // FIRST so bwrap's propagation-fixup pass cannot remount an inherited
        // /dev with MS_NODEV (breaking device nodes); the fresh /dev and the
        // workspace bind must come after it, and writable binds after --dev.
        let dir = tempfile::TempDir::new().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let argv = build_bwrap_argv(
            SandboxMode::WorkspaceWrite,
            &ws,
            None,
            &[],
            &[".ssh".to_string()],
        );
        let tmpfs_root_idx = argv
            .windows(2)
            .position(|w| w == ["--tmpfs", "/"])
            .expect("empty tmpfs root present");
        let dev_idx = argv.iter().position(|a| a == "--dev").unwrap();
        let first_bind_idx = argv.iter().position(|a| a == "--bind").unwrap();
        assert!(
            tmpfs_root_idx < dev_idx,
            "empty tmpfs root must precede --dev"
        );
        assert!(
            dev_idx < first_bind_idx,
            "--dev must precede writable binds"
        );
        assert!(
            dev_idx < first_bind_idx,
            "--dev must precede writable binds"
        );
    }

    #[test]
    fn mask_entries_that_escape_home_are_ignored() {
        // An absolute or `..`-containing masked_dirs entry would make
        // `home.join(dir)` escape $HOME and tmpfs-overlay an unrelated host
        // path (e.g. `/etc`). The argv builder must skip them. Note: the root
        // `--tmpfs /` IS present by design — it is the empty-root mount that
        // hides the host filesystem, not a mask entry.
        let dir = tempfile::TempDir::new().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::create_dir_all(dir.path().join("etc")).unwrap();
        let argv = build_bwrap_argv(
            SandboxMode::WorkspaceWrite,
            &home,
            Some(&home),
            &[],
            &[
                "/etc".to_string(),
                "/".to_string(),
                "../outside".to_string(),
                ".ssh".to_string(),
            ],
        );
        let joined = argv.join(" ");
        assert!(
            !argv.windows(2).any(|w| w == ["--tmpfs", "/etc"]),
            "absolute mask entries must be ignored, got: {joined}"
        );
        assert!(
            !joined.contains("outside"),
            "parent-dir mask entries must be ignored, got: {joined}"
        );
        // Valid relative entries are still masked.
        assert!(joined.contains(&format!("--tmpfs {}", home.join(".ssh").display())));
    }

    #[test]
    fn token_variants_are_recognized_as_credentials() {
        // Regression: bare `TOKEN` (e.g. SLACK_TOKEN, MYAPP_TOKEN) leaked to
        // sandboxed commands because the substring list only had API_TOKEN.
        for (k, blocked) in [
            ("SLACK_TOKEN", true),
            ("MYAPP_TOKEN", true),
            ("SENTRY_AUTH_TOKEN", true),
            ("PATH", false),
            ("CARGO_HOME", false),
            ("LANG", false),
        ] {
            assert_eq!(is_credential_env(k, &[]), blocked, "{k}");
        }
        // Passthrough still beats the new substring rule.
        assert!(!is_credential_env(
            "SLACK_TOKEN",
            &["slack_token".to_string()]
        ));
    }

    /// Live containment check — skipped with a visible reason when bwrap or
    /// unprivileged user namespaces are unavailable; a real spawn failure when
    /// the probe passed is treated as a test failure (loud), not a silent pass.
    #[cfg(target_os = "linux")]
    fn run_in_sandbox(
        mode: SandboxMode,
        ws: &Path,
        home: Option<&Path>,
        masked: &[String],
        cmd: &str,
    ) -> std::io::Result<std::process::Output> {
        let home_buf;
        let home_ref = match home {
            Some(h) => {
                home_buf = h.to_path_buf();
                Some(home_buf.as_path())
            }
            None => None,
        };
        let mut argv = build_bwrap_argv(mode, ws, home_ref, &[], masked);
        argv.extend(["sh".to_string(), "-c".to_string(), cmd.to_string()]);
        Command::new("bwrap").args(&argv).output()
    }

    #[cfg(target_os = "linux")]
    fn skip_without_sandbox() -> bool {
        if !bwrap_probe_works() {
            eprintln!(
                "skipping live sandbox test — bwrap/unprivileged user namespaces unavailable"
            );
            return true;
        }
        false
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn live_workspace_write_confines_writes_and_masks_creds() {
        if skip_without_sandbox() {
            return;
        }
        let dir = tempfile::TempDir::new().unwrap();
        let ws = dir.path().join("ws");
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let (home_dir, home, secret) = tmp_home(".ssh");

        // Write inside the workspace succeeds.
        let ok = run_in_sandbox(
            SandboxMode::WorkspaceWrite,
            &ws,
            Some(&home),
            &[".ssh".to_string()],
            "touch in_ws && test -f in_ws",
        )
        .expect("bwrap spawn should not fail when the probe passed");
        assert!(
            ok.status.success(),
            "workspace write should succeed: {:?}",
            String::from_utf8_lossy(&ok.stderr)
        );

        // Write outside the workspace is denied by the kernel mount.
        let denied = run_in_sandbox(
            SandboxMode::WorkspaceWrite,
            &ws,
            Some(&home),
            &[".ssh".to_string()],
            &format!("touch {} 2>/dev/null", outside.join("x").display()),
        )
        .expect("bwrap spawn should not fail when the probe passed");
        assert!(
            !denied.status.success(),
            "outside-workspace write must fail, stderr: {:?}",
            String::from_utf8_lossy(&denied.stderr)
        );

        // Credential dir masked: reading the real key must yield nothing.
        let leaked = run_in_sandbox(
            SandboxMode::WorkspaceWrite,
            &ws,
            Some(&home),
            &[".ssh".to_string()],
            &format!("cat {}/key.pem", secret.display()),
        )
        .expect("bwrap spawn should not fail when the probe passed");
        assert!(
            !String::from_utf8_lossy(&leaked.stdout).contains("TOP-SECRET"),
            "masked credential dir must not leak secrets"
        );

        drop(home_dir);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn live_isolated_hides_host_state_and_denies_workspace_write() {
        if skip_without_sandbox() {
            return;
        }
        let dir = tempfile::TempDir::new().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        std::fs::write(ws.join("marker.txt"), "visible").unwrap();
        let out = run_in_sandbox(
            SandboxMode::Isolated,
            &ws,
            None,
            &[],
            &format!(
                "test -f {0}/marker.txt && ! test -e \"$HOME\" && ! touch {0}/nope 2>/dev/null && echo ISOLATED_OK",
                ws.display()
            ),
        )
        .expect("bwrap spawn should not fail when the probe passed");
        assert!(
            out.status.success(),
            "isolated: workspace readable, HOME hidden, write denied — stderr: {:?}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn live_read_only_denies_workspace_write() {
        if skip_without_sandbox() {
            return;
        }
        let dir = tempfile::TempDir::new().unwrap();
        let ws = dir.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let out = run_in_sandbox(SandboxMode::ReadOnly, &ws, None, &[], "touch x 2>/dev/null")
            .expect("bwrap spawn should not fail when the probe passed");
        assert!(!out.status.success(), "read-only sandbox must deny writes");
    }
}
