//! Git integration commands for the terminal chat loop: `/diff`, `/commit`,
//! and `/review`.

use std::sync::Arc;

use tokio::signal;
use tokio::sync::mpsc;

use crate::agents::agent::{Agent, Message};
use crate::i18n::runtime::tf;

use super::ansi;
use super::display::display_diff;
use super::simple_tool::chat_simple;

pub(super) async fn execute_diff_command(cmd: &str) {
    // cmd is the part after '/', e.g. "diff" or "diff src/"
    let path_filter = cmd
        .strip_prefix("diff ")
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    let mut git_cmd = tokio::process::Command::new("git");
    git_cmd.arg("diff");
    if let Some(filter) = path_filter {
        git_cmd.arg("--").arg(filter);
    }
    match git_cmd.output().await {
        Ok(out) => {
            let diff = String::from_utf8_lossy(&out.stdout);
            if diff.trim().is_empty() {
                eprintln!(
                    "{}No changes to display.{} (stderr: {})",
                    ansi!("33"),
                    ansi!("0"),
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            } else {
                display_diff(&diff, None);
            }
        }
        Err(e) => eprintln!(
            "{}{}{}",
            ansi!("31"),
            tf("cli.chat.git_diff_failed", &[("reason", &e.to_string())]),
            ansi!("0")
        ),
    }
}

pub(super) async fn execute_commit_command(
    _messages: &[Message],
    current_agent: &Arc<dyn Agent>,
    stdin_rx: &mut mpsc::Receiver<String>,
) {
    let (diff_output, full_diff) = match collect_git_diffs().await {
        Some(pair) => pair,
        None => return,
    };

    eprintln!(
        "{}Changes:{} {}",
        ansi!("1"),
        ansi!("0"),
        diff_output.trim()
    );

    let suggested_msg = generate_commit_message(current_agent, &diff_output, &full_diff).await;

    eprintln!(
        "\r{}✓ Message generated{} {}",
        ansi!("32"),
        ansi!("0"),
        suggested_msg
    );
    eprint!(
        "  {}Press Enter to commit, type a custom message, or n/N to cancel: {} ",
        ansi!("90"),
        ansi!("0")
    );
    std::io::Write::flush(&mut std::io::stdout()).ok();

    let user_line = tokio::select! {
        line = stdin_rx.recv() => line.unwrap_or_default(),
        _ = signal::ctrl_c() => { eprintln!("\nCancelled."); return; }
    };
    let trimmed = user_line.trim().to_string();

    if trimmed.eq_ignore_ascii_case("n") || trimmed.eq_ignore_ascii_case("no") {
        eprintln!("{}Commit cancelled.{}", ansi!("33"), ansi!("0"));
        return;
    }

    let final_msg = if trimmed.is_empty() {
        suggested_msg
    } else {
        trimmed
    };

    stage_and_commit(&final_msg).await;
}

/// Run `git diff --stat` and `git diff` to collect change information.
/// Returns `None` if the diff failed or there is nothing to commit.
/// The two git commands are independent and run concurrently.
async fn collect_git_diffs() -> Option<(String, String)> {
    let (stat_result, full_result) = tokio::join!(
        tokio::process::Command::new("git")
            .args(["diff", "--stat"])
            .output(),
        tokio::process::Command::new("git").arg("diff").output()
    );

    let diff_output = match stat_result {
        Ok(out) => String::from_utf8_lossy(&out.stdout).to_string(),
        Err(e) => {
            eprintln!(
                "{}{}{}",
                ansi!("31"),
                tf("cli.chat.git_diff_failed", &[("reason", &e.to_string())]),
                ansi!("0")
            );
            return None;
        }
    };
    if diff_output.trim().is_empty() {
        eprintln!("{}Nothing to commit.{}", ansi!("33"), ansi!("0"));
        return None;
    }

    let full_diff = match full_result {
        Ok(out) => {
            let s = String::from_utf8_lossy(&out.stdout).to_string();
            if s.len() > 8000 {
                // char-safe truncation: a naive `&s[..8000]` byte slice can
                // panic when a multi-byte UTF-8 code point straddles 8000.
                format!(
                    "{}...\n[truncated]",
                    crate::shared::truncate::truncate_chars(&s, 8000, "")
                )
            } else {
                s
            }
        }
        Err(_) => String::new(),
    };

    Some((diff_output, full_diff))
}

/// Generate a commit message using the agent or a fallback heuristic.
async fn generate_commit_message(
    agent: &Arc<dyn Agent>,
    diff_output: &str,
    full_diff: &str,
) -> String {
    eprint!("{}Generating commit message...{}", ansi!("90"), ansi!("0"));
    std::io::Write::flush(&mut std::io::stdout()).ok();

    let prompt_msg = Message {
        role: "user".to_string(),
        content: format!(
            "Generate a single-line conventional commit message for these changes.\
             \nFormat: <type>(<scope>): <description>\
             \nExamples:\
             \n  feat(api): add user authentication endpoint\
             \n  fix(cache): resolve TTL race condition\
             \n  refactor(cli): simplify command dispatch\
             \n  docs(readme): update installation steps\
             \n\nReturn ONLY the commit message, nothing else.\n\n{}",
            if full_diff.is_empty() {
                diff_output
            } else {
                full_diff
            }
        ),
    };

    match chat_simple(agent, vec![prompt_msg], vec![]).await {
        Ok(m) => m,
        Err(e) => {
            eprintln!(
                "\r{}AI commit message failed: {} — using fallback{}",
                ansi!("31"),
                e,
                ansi!("0")
            );
            format!(
                "feat: {}",
                diff_output.lines().filter(|l| l.contains('|')).count()
            )
        }
    }
}

/// Stage all changes and commit with the given message.
async fn stage_and_commit(msg: &str) {
    let stage_ok = tokio::process::Command::new("git")
        .args(["add", "-A"])
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !stage_ok {
        eprintln!("{}Failed to stage changes.{}", ansi!("31"), ansi!("0"));
        return;
    }

    match tokio::process::Command::new("git")
        .args(["commit", "-m", msg])
        .output()
        .await
    {
        Ok(out) if out.status.success() => {
            eprintln!(
                "{}✓ Committed{}{}",
                ansi!("32"),
                ansi!("0"),
                String::from_utf8_lossy(&out.stdout).trim()
            );
        }
        Ok(out) => eprintln!(
            "{}Commit failed: {}{}",
            ansi!("31"),
            String::from_utf8_lossy(&out.stderr).trim(),
            ansi!("0")
        ),
        Err(e) => eprintln!(
            "{}{}{}",
            ansi!("31"),
            tf("cli.chat.git_diff_failed", &[("reason", &e.to_string())]),
            ansi!("0")
        ),
    }
}

pub(super) async fn execute_review_command(current_agent: &Arc<dyn Agent>) {
    let detailed = match tokio::process::Command::new("git")
        .args(["diff"])
        .output()
        .await
    {
        Ok(out) => String::from_utf8_lossy(&out.stdout).to_string(),
        Err(_) => String::new(),
    };
    if detailed.trim().is_empty() {
        eprintln!("{}No changes to review.{}", ansi!("33"), ansi!("0"));
        return;
    }

    let stat_lines: Vec<&str> = detailed
        .lines()
        .filter(|l| {
            l.contains('|')
                && !l.starts_with("diff ")
                && !l.starts_with("index ")
                && !l.starts_with("---")
                && !l.starts_with("+++")
                && !l.starts_with("@@")
        })
        .collect();
    if !stat_lines.is_empty() {
        eprintln!(
            "{}── Changes ({} file(s)) ──{}",
            ansi!("1"),
            stat_lines.len(),
            ansi!("0")
        );
        for line in &stat_lines {
            eprintln!("  {}", line);
        }
        eprintln!();
    }

    eprint!("{}Reviewing changes with AI...{}", ansi!("90"), ansi!("0"));
    std::io::Write::flush(&mut std::io::stdout()).ok();

    let truncated_diff = if detailed.len() > 12000 {
        format!(
            "{}...\n[truncated: {} total bytes]",
            crate::shared::truncate::truncate_chars(&detailed, 12000, ""),
            detailed.len()
        )
    } else {
        detailed.clone()
    };

    let review_prompt = Message {
        role: "user".to_string(),
        content: format!(
            "Review this git diff for bugs, security issues, code quality, and improvement suggestions.\
             \nBe concise but specific. Point to exact lines where issues exist.\
             \nIf the code looks good, say so briefly.\n\n```diff\n{}\n```",
            truncated_diff
        ),
    };

    match chat_simple(current_agent, vec![review_prompt], vec![]).await {
        Ok(review) => {
            eprintln!("\r{}── AI Code Review ──{}", ansi!("1"), ansi!("0"));
            eprintln!("{}", review);
        }
        Err(e) => {
            eprintln!(
                "\r{}{}{}",
                ansi!("31"),
                tf("cli.chat.ai_review_failed", &[("reason", &e.to_string())]),
                ansi!("0")
            );
            display_diff(&detailed, Some(60));
        }
    }
}
