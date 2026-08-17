//! M2.2 approval chain layer ① — permission hooks.
//!
//! External-command pre-decision hooks for the tool-approval chain.
//!
//! Layering (from the M2.2 milestone, `docs/log/log-20260814-3.md`):
//!
//!   1. HarnessBus policy evaluation (pre-route, before tool execution)
//!   2. **permission_hooks** (this module) — configured external commands
//!      that return `allow` / `deny` / `override` before the user is asked
//!   3. AutoReviewer (later milestone layer)
//!   4. User prompt (`request_client_permission` on the ACP server)
//!
//! Every executed hook run is recorded in the global audit log
//! (`decision = "permission_hook"`), including no-opinion and failure runs,
//! so all approval decisions enter the audit chain.
//!
//! # Hook contract
//!
//! A hook is executed as a subprocess with the tool name and the tool's
//! argument JSON written to its **stdin** as a single JSON object:
//!
//! ```json
//! { "tool_name": "write_file", "tool_args": { "path": "..." } }
//! ```
//!
//! The **first line of stdout** decides the verdict (case-insensitive):
//!
//! | stdout first line | effect |
//! |---|---|
//! | `allow`    | approve the tool without prompting the user |
//! | `deny`     | reject the tool without prompting the user |
//! | `override` | approve without prompting (treated like `allow` at this layer) |
//! | anything else | no opinion — continue with the next hook / the user prompt |
//!
//! A non-zero exit status also counts as "no opinion". A hook that cannot be
//! spawned or that exceeds the timeout never blocks the approval: it is
//! logged, audited, and skipped (honest fail-open).

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::RuntimeConfig;

/// Default per-hook execution timeout.
const DEFAULT_HOOK_TIMEOUT: Duration = Duration::from_secs(5);

/// Raw config entry for a permission hook.
///
/// Each entry in `[runtime] permission_hooks` may be either:
/// - a command-line string (`"sh scripts/check.sh --strict"`), or
/// - a table with explicit `command` + `args` (`{ command = "python3", args = ["check.py"] }`).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum PermissionHookConfig {
    /// A single command line; split on whitespace (quote-aware).
    CommandLine(String),
    /// Explicit command and argument list.
    Structured {
        command: String,
        #[serde(default)]
        args: Vec<String>,
    },
}

impl PermissionHookConfig {
    /// Normalize into an executable [`PermissionHook`].
    ///
    /// Returns `None` when the entry has no usable command (e.g. an empty
    /// command line) — such entries are skipped at config load time.
    fn to_hook(&self) -> Option<PermissionHook> {
        match self {
            PermissionHookConfig::CommandLine(line) => {
                let mut parts = split_command_line(line);
                if parts.is_empty() {
                    return None;
                }
                let command = parts.remove(0);
                Some(PermissionHook {
                    command,
                    args: parts,
                })
            }
            PermissionHookConfig::Structured { command, args } => {
                if command.trim().is_empty() {
                    return None;
                }
                Some(PermissionHook {
                    command: command.clone(),
                    args: args.clone(),
                })
            }
        }
    }
}

/// A normalized permission hook: an external command executed synchronously.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionHook {
    /// Executable (or script interpreter) to run.
    pub command: String,
    /// Fixed arguments passed to `command`.
    pub args: Vec<String>,
}

/// Verdict a permission hook can express for the tool approval chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookVerdict {
    /// Approve the tool without prompting the user.
    Allow,
    /// Deny the tool without prompting the user.
    Deny,
    /// Override — approve without prompting, exactly like [`HookVerdict::Allow`]
    /// at this layer. Kept as a distinct verdict so downstream layers and the
    /// audit trail can distinguish an explicit override from a plain allow.
    Override,
}

/// Result of running the configured permission hooks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookDecision {
    /// `None` means no hook expressed an opinion — proceed to the user prompt.
    pub verdict: Option<HookVerdict>,
    /// Human-readable reason (the hook's stdout) when a verdict is given.
    pub reason: Option<String>,
}

/// Outcome of running a single hook.
#[derive(Debug, Clone)]
enum HookOutcome {
    /// The hook expressed a verdict, with the reason text it printed.
    Verdict(HookVerdict, Option<String>),
    /// The hook ran but expressed no opinion (garbage output / non-zero exit).
    NoOpinion(Option<i32>),
    /// The hook could not be run (spawn error) or exceeded its timeout.
    Error(String),
}

/// Read and normalize the configured permission hooks from the runtime config.
///
/// Entries that fail to normalize (e.g. empty command lines) are skipped with
/// a warning; a misconfigured hook never blocks approval (fail-open).
pub fn permission_hooks_from_config(cfg: &RuntimeConfig) -> Vec<PermissionHook> {
    cfg.permission_hooks
        .iter()
        .filter_map(|raw| match raw.to_hook() {
            Some(hook) => Some(hook),
            None => {
                tracing::warn!(
                    target: "governance::permission_hooks",
                    "permission hook entry ignored (no usable command)"
                );
                None
            }
        })
        .collect()
}

/// Run all configured permission hooks with the default per-hook timeout
/// ([`DEFAULT_HOOK_TIMEOUT`]).
///
/// Hooks are evaluated in order; the first hook with an opinion decides.
pub fn run_permission_hooks(
    hooks: &[PermissionHook],
    tool_name: &str,
    tool_args: &Value,
) -> HookDecision {
    run_permission_hooks_with_timeout(hooks, tool_name, tool_args, DEFAULT_HOOK_TIMEOUT)
}

/// Run all configured permission hooks with an explicit per-hook timeout.
///
/// The timeout is injectable so tests can exercise the timeout path quickly.
pub fn run_permission_hooks_with_timeout(
    hooks: &[PermissionHook],
    tool_name: &str,
    tool_args: &Value,
    timeout: Duration,
) -> HookDecision {
    for hook in hooks {
        let outcome = run_single_hook(hook, tool_name, tool_args, timeout);
        match &outcome {
            HookOutcome::Verdict(verdict, reason) => {
                record_hook_audit(tool_name, hook, tool_args, &outcome);
                return HookDecision {
                    verdict: Some(*verdict),
                    reason: reason.clone(),
                };
            }
            HookOutcome::NoOpinion(_) => {
                record_hook_audit(tool_name, hook, tool_args, &outcome);
            }
            HookOutcome::Error(err) => {
                // A failing hook must not block the approval: log, audit, and
                // continue to the next hook (honest fail-open).
                tracing::warn!(
                    target: "governance::permission_hooks",
                    command = %hook.command,
                    tool = %tool_name,
                    error = %err,
                    "permission hook failed — skipped (fail-open)"
                );
                record_hook_audit(tool_name, hook, tool_args, &outcome);
            }
        }
    }
    HookDecision {
        verdict: None,
        reason: None,
    }
}

/// Execute a single hook subprocess and interpret its stdout.
///
/// The `{tool_name, tool_args}` payload is written to the hook's stdin; the
/// first stdout line decides the verdict. The child is killed when it exceeds
/// `timeout`. The hook never blocks on stdin and the caller never prompts.
fn run_single_hook(
    hook: &PermissionHook,
    tool_name: &str,
    tool_args: &Value,
    timeout: Duration,
) -> HookOutcome {
    let mut child = match Command::new(&hook.command)
        .args(&hook.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => return HookOutcome::Error(format!("spawn failed: {err}")),
    };

    // Feed the JSON payload on stdin from a dedicated thread so a hook that
    // never reads stdin cannot block the poll loop below (a blocked pipe write
    // would otherwise defeat the timeout). The thread ends as soon as the pipe
    // closes (child exit or kill).
    if let Some(mut stdin) = child.stdin.take() {
        let payload = json!({ "tool_name": tool_name, "tool_args": tool_args }).to_string();
        std::thread::spawn(move || {
            use std::io::Write;
            let _ = stdin.write_all(payload.as_bytes());
            let _ = stdin.flush();
        });
    }

    // Poll for exit with a timeout; kill on timeout instead of blocking.
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return HookOutcome::Error(format!("timed out after {timeout:?}"));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(err) => return HookOutcome::Error(format!("wait failed: {err}")),
        }
    };

    // The process has exited, so reading stdout cannot hang; whatever fit in
    // the pipe buffer is all we need (first line only).
    let mut stdout = String::new();
    if let Some(mut out) = child.stdout.take() {
        use std::io::Read;
        let _ = out.read_to_string(&mut stdout);
    }

    // A non-zero exit status counts as "no opinion" (the hook ran but did not
    // approve or deny).
    if !status.success() {
        return HookOutcome::NoOpinion(status.code());
    }
    match parse_hook_stdout(&stdout) {
        Some(verdict) => HookOutcome::Verdict(verdict, Some(stdout.trim().to_string())),
        None => HookOutcome::NoOpinion(None),
    }
}

/// Interpret the first stdout line of a hook (case-insensitive).
fn parse_hook_stdout(stdout: &str) -> Option<HookVerdict> {
    match stdout.lines().next()?.trim().to_ascii_lowercase().as_str() {
        "allow" => Some(HookVerdict::Allow),
        "deny" => Some(HookVerdict::Deny),
        "override" => Some(HookVerdict::Override),
        _ => None,
    }
}

/// Record one audit entry per executed hook run — verdict, no opinion, or
/// error all enter the audit chain (`decision = "permission_hook"`).
fn record_hook_audit(
    tool_name: &str,
    hook: &PermissionHook,
    tool_args: &Value,
    outcome: &HookOutcome,
) {
    let (outputs, error) = match outcome {
        HookOutcome::Verdict(verdict, reason) => (
            Some(json!({
                "verdict": verdict_label(*verdict),
                "reason": reason,
            })),
            None,
        ),
        HookOutcome::NoOpinion(exit_code) => (
            Some(json!({
                "verdict": "no_opinion",
                "exit_code": exit_code,
            })),
            None,
        ),
        HookOutcome::Error(err) => (None, Some(err.clone())),
    };
    crate::governance::audit::global_audit_log().record(
        crate::governance::audit::AuditLogEntry {
            timestamp: crate::governance::audit::chrono_now(),
            task_id: String::new(),
            phase: "approval".to_string(),
            agent: None,
            tool: Some(tool_name.to_string()),
            decision: "permission_hook".to_string(),
            inputs: json!({
                "command": hook.command,
                "args": hook.args,
                "tool_args": tool_args,
            }),
            outputs,
            error,
            confidence: None,
            data_classification: None,
            compliance_tags: Vec::new(),
            retention_policy: None,
            correlation_id: None,
        },
    );
}

/// Machine-readable label for a [`HookVerdict`] (also used in audit output).
fn verdict_label(verdict: HookVerdict) -> &'static str {
    match verdict {
        HookVerdict::Allow => "allow",
        HookVerdict::Deny => "deny",
        HookVerdict::Override => "override",
    }
}

/// Split a command-line string into tokens, honoring single and double quotes.
///
/// No shell metacharacter expansion is performed — the string form is for
/// simple `command arg arg` lines; anything fancier belongs in `{command, args}`.
fn split_command_line(line: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_token = false;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\'' => {
                in_token = true;
                for inner in chars.by_ref() {
                    if inner == '\'' {
                        break;
                    }
                    current.push(inner);
                }
            }
            '"' => {
                in_token = true;
                for inner in chars.by_ref() {
                    if inner == '"' {
                        break;
                    }
                    current.push(inner);
                }
            }
            ch if ch.is_whitespace() => {
                if in_token {
                    tokens.push(std::mem::take(&mut current));
                    in_token = false;
                }
            }
            ch => {
                in_token = true;
                current.push(ch);
            }
        }
    }
    if in_token {
        tokens.push(current);
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A hook that prints `word` on stdout, portably.
    fn echo_hook(word: &str) -> PermissionHook {
        #[cfg(windows)]
        let (command, args) = (
            "cmd".to_string(),
            vec!["/C".to_string(), format!("echo {word}")],
        );
        #[cfg(not(windows))]
        let (command, args) = (
            "sh".to_string(),
            vec!["-c".to_string(), format!("echo {word}")],
        );
        PermissionHook { command, args }
    }

    /// A hook that runs for ~10s, portably (for the timeout test).
    fn slow_hook() -> PermissionHook {
        #[cfg(windows)]
        let (command, args) = (
            "cmd".to_string(),
            vec!["/C".to_string(), "ping -n 10 127.0.0.1 >nul".to_string()],
        );
        #[cfg(not(windows))]
        let (command, args) = ("sh".to_string(), vec!["-c".to_string(), "sleep 10".to_string()]);
        PermissionHook { command, args }
    }

    fn args() -> Value {
        json!({ "path": "notes.md", "content": "hi" })
    }

    #[test]
    fn empty_hook_list_is_no_opinion() {
        let decision = run_permission_hooks(&[], "write_file", &args());
        assert_eq!(decision, HookDecision { verdict: None, reason: None });
    }

    #[test]
    fn allow_hook_approves() {
        let decision = run_permission_hooks(&[echo_hook("allow")], "write_file", &args());
        assert_eq!(
            decision,
            HookDecision {
                verdict: Some(HookVerdict::Allow),
                reason: Some("allow".to_string()),
            }
        );
    }

    #[test]
    fn deny_hook_rejects() {
        let decision = run_permission_hooks(&[echo_hook("deny")], "write_file", &args());
        assert_eq!(decision.verdict, Some(HookVerdict::Deny));
        assert!(decision.reason.is_some());
    }

    #[test]
    fn override_hook_approves() {
        let decision = run_permission_hooks(&[echo_hook("override")], "write_file", &args());
        assert_eq!(decision.verdict, Some(HookVerdict::Override));
    }

    #[test]
    fn verdict_matching_is_case_insensitive() {
        let decision = run_permission_hooks(&[echo_hook("ALLOW")], "write_file", &args());
        assert_eq!(decision.verdict, Some(HookVerdict::Allow));
        let decision = run_permission_hooks(&[echo_hook("DeNy")], "write_file", &args());
        assert_eq!(decision.verdict, Some(HookVerdict::Deny));
    }

    #[test]
    fn garbage_output_is_no_opinion() {
        let decision = run_permission_hooks(&[echo_hook("maybe")], "write_file", &args());
        assert_eq!(decision.verdict, None);
    }

    #[test]
    fn first_hook_with_opinion_wins() {
        // First hook has no opinion → second hook decides.
        let hooks = vec![echo_hook("maybe"), echo_hook("allow")];
        let decision = run_permission_hooks(&hooks, "write_file", &args());
        assert_eq!(decision.verdict, Some(HookVerdict::Allow));

        // First hook denies → later hooks are not consulted.
        let hooks = vec![echo_hook("deny"), echo_hook("allow")];
        let decision = run_permission_hooks(&hooks, "write_file", &args());
        assert_eq!(decision.verdict, Some(HookVerdict::Deny));
    }

    #[test]
    fn non_zero_exit_is_no_opinion() {
        let hook = PermissionHook {
            #[cfg(windows)]
            command: "cmd".to_string(),
            #[cfg(windows)]
            args: vec!["/C".to_string(), "exit /b 3".to_string()],
            #[cfg(not(windows))]
            command: "sh".to_string(),
            #[cfg(not(windows))]
            args: vec!["-c".to_string(), "exit 3".to_string()],
        };
        let decision = run_permission_hooks(&[hook], "write_file", &args());
        assert_eq!(decision.verdict, None);
    }

    #[test]
    fn missing_command_is_error_and_fail_open() {
        let hook = PermissionHook {
            command: "definitely-not-a-real-binary-xyz".to_string(),
            args: vec![],
        };
        let decision = run_permission_hooks(&[hook], "write_file", &args());
        assert_eq!(decision.verdict, None);
    }

    #[test]
    fn timeout_is_fail_open_and_fast() {
        let start = Instant::now();
        let decision = run_permission_hooks_with_timeout(
            &[slow_hook()],
            "write_file",
            &args(),
            Duration::from_millis(300),
        );
        assert_eq!(decision.verdict, None);
        // Must not wait for the ~10s hook to finish on its own.
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "timeout path took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn hooks_receive_tool_name_and_args_on_stdin() {
        // A hook that echoes back a verdict only when stdin looks sane.
        #[cfg(windows)]
        let hook = PermissionHook {
            command: "cmd".to_string(),
            args: vec![
                "/C".to_string(),
                "findstr /C:tool_name >nul && echo allow".to_string(),
            ],
        };
        #[cfg(not(windows))]
        let hook = PermissionHook {
            command: "sh".to_string(),
            args: vec!["-c".to_string(), "grep -q '\"tool_name\"' && echo allow".to_string()],
        };
        let decision = run_permission_hooks(&[hook], "read_file", &args());
        assert_eq!(decision.verdict, Some(HookVerdict::Allow));
    }

    #[test]
    fn config_normalizes_string_and_structured_entries() {
        let cfg = RuntimeConfig {
            permission_hooks: vec![
                PermissionHookConfig::CommandLine("sh check.sh --strict".to_string()),
                PermissionHookConfig::Structured {
                    command: "python3".to_string(),
                    args: vec!["check.py".to_string()],
                },
            ],
            ..RuntimeConfig::default()
        };
        let hooks = permission_hooks_from_config(&cfg);
        assert_eq!(hooks.len(), 2);
        assert_eq!(hooks[0].command, "sh");
        assert_eq!(hooks[0].args, vec!["check.sh".to_string(), "--strict".to_string()]);
        assert_eq!(hooks[1].command, "python3");
        assert_eq!(hooks[1].args, vec!["check.py".to_string()]);
    }

    #[test]
    fn config_skips_entries_without_a_command() {
        let cfg = RuntimeConfig {
            permission_hooks: vec![
                PermissionHookConfig::CommandLine("   ".to_string()),
                PermissionHookConfig::Structured {
                    command: String::new(),
                    args: vec![],
                },
            ],
            ..RuntimeConfig::default()
        };
        assert!(permission_hooks_from_config(&cfg).is_empty());
    }

    #[test]
    fn config_deserializes_mixed_hook_entries_from_toml() {
        let toml_str = r#"
            permission_hooks = ["sh hook.sh", { command = "python3", args = ["check.py"] }]
        "#;
        let cfg: RuntimeConfig =
            toml::from_str(toml_str).expect("mixed string/table hook entries must parse");
        let hooks = permission_hooks_from_config(&cfg);
        assert_eq!(hooks.len(), 2);
        assert_eq!(hooks[0].command, "sh");
        assert_eq!(hooks[1].args, vec!["check.py".to_string()]);
    }

    #[test]
    fn quote_aware_command_line_splitting() {
        assert_eq!(split_command_line("sh hook.sh --strict"), vec!["sh", "hook.sh", "--strict"]);
        assert_eq!(
            split_command_line("echo \"hello world\" 'a b'"),
            vec!["echo", "hello world", "a b"]
        );
        assert!(split_command_line("   ").is_empty());
        assert!(split_command_line("").is_empty());
    }
}
