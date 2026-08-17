//! Terminal display helpers for the chat loop: startup banner, info commands
//! (`/skills`, `/stats`, `/context`, `/models`), diff highlighting, and
//! tool-output formatting.

use std::sync::Arc;

use anyhow::Result;

use crate::agents::agent::{Agent, Message};
use crate::i18n::runtime::{t, tf};
use crate::orchestration::mode::ModeKind;
use crate::orchestration::session_compressor::DEFAULT_TOKEN_WINDOW;

use super::ansi;
use super::tokens::TokenTracker;
use super::{mode_kind_str, COMPACT_PROMPT_THRESHOLD};

/// Print the startup banner for the terminal chat session.
pub(super) fn print_chat_banner(current_agent_name: &str, mode: ModeKind) {
    let mode_name = mode_kind_str(mode);
    let version = env!("CARGO_PKG_VERSION");
    eprintln!("╔════════════════════════════════════════════════════════════════╗");
    eprintln!("║            go-on terminal chat v{:<46} ║", version);
    eprintln!("╠════════════════════════════════════════════════════════════════╣");
    eprintln!("║  Agent: {:<60} ║", current_agent_name);
    eprintln!("║  Mode:  {:<60} ║", mode_name);
    eprintln!("║  Commands: /help /quit /clear /save /load /cost /compact    ║");
    eprintln!("║   /diff /commit /plan /model /models /retry /context /tools /skills   ║");
    eprintln!("║   /mode /stats /find_path  ║");
    eprintln!("╚════════════════════════════════════════════════════════════════╝");
    eprintln!();
}

pub(super) fn display_skills() {
    let descriptor_list = crate::orchestration::tool::skill_registry()
        .and_then(|r| r.read().ok())
        .map(|guard| guard.list(false))
        .unwrap_or_default();
    if descriptor_list.is_empty() {
        eprintln!("{}", t("cli.chat.no_skills"));
    } else {
        eprintln!(
            "{}",
            tf(
                "cli.chat.skills_count",
                &[("count", &descriptor_list.len().to_string())]
            )
        );
        for s in &descriptor_list {
            eprintln!("  {:<25} score: {:.2}", s.name, s.score);
        }
    }
}

pub(super) fn display_stats(messages: &[Message], token_tracker: &TokenTracker) {
    let agent_msgs = messages.iter().filter(|m| m.role == "assistant").count();
    let user_msgs = messages.iter().filter(|m| m.role == "user").count();
    let total_chars: usize = messages.iter().map(|m| m.content.len()).sum();
    eprintln!("{}", t("cli.chat.stats_header"));
    eprintln!(
        "{}",
        tf(
            "cli.chat.stats_messages",
            &[
                ("total", &messages.len().to_string()),
                ("user", &user_msgs.to_string()),
                ("assistant", &agent_msgs.to_string()),
            ]
        )
    );
    eprintln!(
        "{}",
        tf(
            "cli.chat.stats_total_chars",
            &[("count", &total_chars.to_string())]
        )
    );
    eprintln!(
        "{}",
        tf(
            "cli.chat.stats_avg_length",
            &[(
                "count",
                &(if !messages.is_empty() {
                    total_chars / messages.len()
                } else {
                    0
                })
                .to_string()
            )]
        )
    );
    eprint!("{}", token_tracker.display());
}

pub(super) fn display_context(messages: &[Message]) {
    let total_chars: usize = messages.iter().map(|m| m.content.len()).sum();
    let est_tokens: usize = messages
        .iter()
        .map(|m| crate::shared::token_estimator::estimate_tokens(&m.content))
        .sum();
    let system_msgs = messages.iter().filter(|m| m.role == "system").count();
    eprintln!("{}", t("cli.chat.context_header"));
    eprintln!(
        "{}",
        tf(
            "cli.chat.context_messages",
            &[
                ("total", &messages.len().to_string()),
                ("system", &system_msgs.to_string()),
                (
                    "user",
                    &messages
                        .iter()
                        .filter(|m| m.role == "user")
                        .count()
                        .to_string()
                ),
                (
                    "assistant",
                    &messages
                        .iter()
                        .filter(|m| m.role == "assistant")
                        .count()
                        .to_string()
                ),
            ]
        )
    );
    eprintln!(
        "{}",
        tf(
            "cli.chat.context_chars",
            &[
                ("count", &total_chars.to_string()),
                ("tokens", &est_tokens.to_string()),
            ]
        )
    );
    eprintln!(
        "{}",
        tf(
            "cli.chat.context_used_pct",
            &[(
                "pct",
                &format!(
                    "{:.1}",
                    (est_tokens as f64 / DEFAULT_TOKEN_WINDOW as f64 * 100.0).min(100.0)
                )
            )]
        )
    );
    if messages.len() >= COMPACT_PROMPT_THRESHOLD {
        eprintln!(
            "{}",
            tf(
                "cli.chat.context_compact_tip",
                &[
                    ("open", ansi!("33")),
                    ("close", ansi!("0")),
                    ("current", &messages.len().to_string()),
                    ("threshold", &COMPACT_PROMPT_THRESHOLD.to_string()),
                ]
            )
        );
    }
}

pub(super) fn display_models(current_agent: &Arc<dyn Agent>, current_agent_name: &str) {
    let models = current_agent.available_models();
    if models.is_empty() {
        eprintln!(
            "{}{}{}",
            ansi!("33"),
            tf("cli.chat.no_models", &[("agent", current_agent_name)]),
            ansi!("0")
        );
    } else {
        eprintln!(
            "{}{}{}:",
            ansi!("1"),
            tf("cli.chat.models_header", &[("agent", current_agent_name)]),
            ansi!("0")
        );
        for m in &models {
            let default_flag = if m.is_default { " (default)" } else { "" };
            eprintln!("  {:<30} {} {}{}", m.id, m.name, ansi!("90"), default_flag);
        }
    }
}

/// Display a git diff with ANSI color highlighting, optionally limited to `max_lines`.
pub(super) fn display_diff(diff: &str, max_lines: Option<usize>) {
    let iter: Box<dyn Iterator<Item = &str>> = match max_lines {
        Some(n) => Box::new(diff.lines().take(n)),
        None => Box::new(diff.lines()),
    };
    for line in iter {
        if line.starts_with('+') && !line.starts_with("+++") {
            eprintln!("{}{}{}", ansi!("32"), line, ansi!("0"));
        } else if line.starts_with('-') && !line.starts_with("---") {
            eprintln!("{}{}{}", ansi!("31"), line, ansi!("0"));
        } else if line.starts_with("@@") {
            eprintln!("{}{}{}", ansi!("36"), line, ansi!("0"));
        } else {
            eprintln!("{}", line);
        }
    }
}

/// Append stdout + stderr to a buffer, separated by newline.
fn append_stdouterr(buf: &mut String, r: &serde_json::Value) {
    if let Some(stdout) = r["stdout"].as_str() {
        if !stdout.is_empty() {
            if !buf.is_empty() {
                buf.push('\n');
            }
            buf.push_str(stdout);
        }
    }
    if let Some(stderr) = r["stderr"].as_str() {
        if !stderr.is_empty() {
            if !buf.is_empty() {
                buf.push('\n');
            }
            buf.push_str(stderr);
        }
    }
}

/// Append stdout, stderr, and exit code to a buffer.
pub(super) fn append_cmd_result(buf: &mut String, r: &serde_json::Value) {
    use std::fmt::Write;
    append_stdouterr(buf, r);
    if let Some(code) = r["exit_code"].as_i64() {
        if !buf.is_empty() {
            buf.push('\n');
        }
        let _ = write!(buf, "exit code: {code}");
    }
}

/// Format a command execution output (stdout + stderr + exit code) into a string.
pub(super) fn format_cmd_output(r: &serde_json::Value) -> Result<String> {
    let mut buf = String::new();
    append_cmd_result(&mut buf, r);
    Ok(buf)
}

/// Format the output of a `run_tests` tool call.
pub(super) fn format_run_tests_output(r: &serde_json::Value) -> Result<String> {
    use std::fmt::Write;
    let mut buf = match r["filter"].as_str() {
        Some(f) if !f.is_empty() => format!("filter: {f}"),
        _ => String::new(),
    };
    append_stdouterr(&mut buf, r);
    if let Some(code) = r["exit_code"].as_i64() {
        if !buf.is_empty() {
            buf.push('\n');
        }
        let _ = write!(buf, "exit code: {code}");
    }
    if let Some(cmd) = r["command"].as_str() {
        let _ = write!(buf, "\ncommand: {cmd}");
    }
    Ok(buf)
}

/// Format the output of an `inspect_git_diff` tool call.
pub(super) fn format_inspect_git_diff_output(r: &serde_json::Value) -> Result<String> {
    let diff = r["diff"].as_str().unwrap_or("");
    let staged = r["staged"].as_bool().unwrap_or(false);
    let mut buf = if staged {
        "(staged diff)".to_string()
    } else {
        "(unstaged diff)".to_string()
    };
    if !diff.is_empty() {
        buf.push('\n');
        buf.push_str(diff);
    }
    if let Some(stderr) = r["stderr"].as_str() {
        if !stderr.is_empty() {
            buf.push('\n');
            buf.push_str(stderr);
        }
    }
    Ok(buf)
}

/// Format the output of a `cargo_check` tool call.
pub(super) fn format_cargo_check_output(r: &serde_json::Value) -> Result<String> {
    use std::fmt::Write;
    let error_count = r["error_count"].as_u64().unwrap_or(0);
    let warning_count = r["warning_count"].as_u64().unwrap_or(0);
    let mut buf = format!("cargo check: {error_count} errors, {warning_count} warnings\n");
    if let Some(errors) = r["errors"].as_array() {
        for e in errors {
            if let Some(rendered) = e["rendered"].as_str() {
                let _ = write!(buf, "\n── ERROR ──\n{rendered}");
            }
        }
    }
    if let Some(warnings) = r["warnings"].as_array() {
        for w in warnings {
            if let Some(rendered) = w["rendered"].as_str() {
                let _ = write!(buf, "\n── WARNING ──\n{rendered}");
            }
        }
    }
    if let Some(code) = r["exit_code"].as_i64() {
        let _ = write!(buf, "\nexit code: {code}");
    }
    Ok(buf)
}
