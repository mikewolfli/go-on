//! Built-in chat commands, stdin input reading, and user-message dispatch for
//! the terminal chat loop.

use std::sync::Arc;

use tokio::signal;
use tokio::sync::mpsc;
use tracing::warn;

use crate::agents::agent::{Agent, AgentRegistry, Message, StreamingSender};
use crate::i18n::runtime::{t, tf};
use crate::orchestration::mode::{resolve_mode_runtime, ModeKind, ModeRuntime};

use super::agent_turn::run_agent_with_tools;
use super::ansi;
use super::display::{display_context, display_models, display_skills, display_stats};
use super::git::{execute_commit_command, execute_diff_command, execute_review_command};
use super::session::{
    auto_save_turn, check_compact_threshold, handle_load_command, handle_save_command,
};
use super::simple_tool::{build_cli_principles, chat_simple, execute_simple_tool};
use super::tokens::{record_turn_usage, TokenTracker};
use super::{injection_detector, mode_kind_str, tool_registry};

/// Max lines to display for help text.
///
/// Human-readable summary of the chat commands and tool *categories* — shown
/// to the user on `/help`. This is intentionally a concise curated list, NOT
/// the instruction text sent to the model: the model-facing tool inventory is
/// built dynamically from `tool_registry().all_names()` in the system prompt
/// and in `build_cli_principles()` below. Keeping the three lists separate is
/// deliberate — the help text reads well for a human, while the prompt must
/// list every registered tool by its real name.
const HELP_TEXT: &str = "\
Commands:
  /quit        Exit chat
  /clear       Clear conversation history
  /save        Save session to file
  /load        Load session from file
  /help        Show this help
  /agents      List configured agents
  /model       Switch active agent model
  /tools       List available tools
  /skills      List available skills
  /stats       Show conversation stats
  /context     Show context window usage (estimated tokens/characters)
  /compact     Summarize & compact conversation history
  /cost        Show token usage and estimated cost
  /diff        Show git diff (optional path filter)
  /commit      AI-powered git commit (generates message, confirms before committing)
  /review      AI-powered code review of current git diff
  /plan        AI-generated structured execution plan from conversation context
  /find_path   Search for files by name glob
  /models      List available models for current agent
  /retry       Re-send the last user message

The AI agent has access to tools:
  - Read/write files (read_file, write_file, read_file_lines)
  - Search files and directories (search_files, grep, find_path, list_directory)
  - Apply patches (apply_patch)
  - Execute shell commands (shell_exec)
  - Git operations (diff, status, log, commit, review)
  - Cargo commands (cargo_check, cargo_test)
  - Diagnostics (diagnostics)
  - Network tools (http_request, dns_lookup, ping, port_scan)
  - Archive tools (archive_inspect, archive_extract)
  - Compression (compress, decompress)
  - Data tools (jsonl_read, jsonl_write)
  - Environment info (date_time, environment_info)
  - Code search (code_index_search)
  - File comparison (diff)
  - Skills (skill_list, skill_execute, skill_create, skill_reload)
  - Multi-turn conversation with context
  - File operations (copy_path, move_path, delete_path, create_directory)
";

/// Read a line from stdin with Ctrl+C handling. Returns None on EOF.
pub(super) async fn read_user_input(stdin_rx: &mut mpsc::Receiver<String>) -> Option<String> {
    let mut buffer = String::new();
    loop {
        tokio::select! {
            line = stdin_rx.recv() => {
                let line = match line {
                    Some(l) => l,
                    None => return if buffer.is_empty() { None } else { Some(buffer) },
                };
                // If we already have buffered content, this is a continuation line.
                if !buffer.is_empty() {
                    buffer.push('\n');
                    buffer.push_str(line.trim_end());
                } else {
                    // Check for backslash continuation: line ends with \
                    if let Some(trimmed) = line.strip_suffix('\\') {
                        buffer.push_str(trimmed.trim_end());
                        // Continue reading
                        continue;
                    }
                    // Check for whitespace continuation: line starts with whitespace
                    if line.starts_with(' ') || line.starts_with('\t') {
                        buffer.push_str(line.trim_end());
                        continue;
                    }
                    buffer = line.trim_end().to_string();
                }

                // Check for unbalanced braces (multi-line payloads like JSON).
                let open_count = buffer.chars().filter(|c| *c == '{').count();
                let close_count = buffer.chars().filter(|c| *c == '}').count();
                if open_count > close_count {
                    // Braces are unbalanced — continue reading.
                    continue;
                }

                return Some(buffer);
            }
            _ = signal::ctrl_c() => {
                if buffer.is_empty() {
                    eprintln!("\n{}{}{}", ansi!("33"), t("cli.chat.interrupted"), ansi!("0"));
                    return Some(String::new());
                } else {
                    // Return whatever we've buffered so far.
                    return Some(buffer);
                }
            }
        }
    }
}

/// Dispatch a built-in command. Returns `true` if the caller should exit the main loop.
#[allow(clippy::too_many_arguments)]
pub(super) async fn dispatch_builtin_command(
    cmd: &str,
    messages: &mut Vec<Message>,
    current_agent: &mut Arc<dyn Agent>,
    current_agent_name: &mut String,
    current_mode: &mut Box<dyn ModeRuntime>,
    registry: &Arc<AgentRegistry>,
    token_tracker: &mut TokenTracker,
    session_path: &std::path::Path,
    stdin_rx: &mut mpsc::Receiver<String>,
) -> bool {
    match cmd {
        // ── Session commands ──
        "quit" | "exit" | "q" => return true,
        "help" | "h" => {
            eprint!("{}", HELP_TEXT);
        }
        "clear" => {
            messages.clear();
            eprintln!(
                "{}{}{}",
                ansi!("32"),
                t("cli.chat.conversation_cleared"),
                ansi!("0")
            );
        }
        "save" => {
            handle_save_command(messages, current_agent_name, current_mode, session_path).await;
        }
        "load" => {
            handle_load_command(
                session_path,
                messages,
                current_agent,
                current_agent_name,
                current_mode,
                registry,
            )
            .await;
        }
        // ── Agent / tool info commands ──
        "agents" => {
            let names = registry.names();
            for name in &names {
                eprintln!("{}", tf("cli.chat.agents_list", &[("name", name)]));
            }
            eprintln!(
                "{}",
                tf(
                    "cli.chat.switch_agent_hint",
                    &[("name", current_agent_name)]
                )
            );
        }
        "tools" => {
            let reg = tool_registry();
            let names = reg.all_names();
            eprintln!(
                "{}",
                tf(
                    "cli.chat.tools_count",
                    &[("count", &names.len().to_string())]
                )
            );
            for name in names {
                if let Some(profile) = reg.profile(name) {
                    eprintln!(
                        "{}",
                        tf(
                            "cli.chat.tools_list_entry",
                            &[
                                ("name", name),
                                ("capability", &profile.capability.to_string())
                            ]
                        )
                    );
                } else {
                    eprintln!("  {name}");
                }
            }
        }
        "skills" => {
            display_skills();
        }
        // ── Information commands ──
        "stats" => {
            display_stats(messages, token_tracker);
        }
        "cost" => {
            eprint!("{}", token_tracker.display());
        }
        "context" => {
            display_context(messages);
        }
        // ── Compact ──
        "compact" => {
            execute_compact_command(messages, current_agent).await;
        }
        // ── Git commands ──
        cmd if cmd == "diff" || cmd.starts_with("diff ") => {
            execute_diff_command(cmd).await;
        }
        "commit" => {
            execute_commit_command(messages, current_agent, stdin_rx).await;
        }
        "review" => {
            execute_review_command(current_agent).await;
        }
        // ── Plan ──
        "plan" => {
            execute_plan_command(
                messages,
                current_agent,
                registry,
                current_agent_name,
                current_mode,
                stdin_rx,
            )
            .await;
        }
        // ── Find path ──
        find_cmd if find_cmd.starts_with("find_path") || find_cmd.starts_with("find ") => {
            execute_find_path_command(find_cmd).await;
        }
        // ── Mode ──
        mode_cmd if mode_cmd.starts_with("mode") => {
            execute_mode_command(mode_cmd, current_mode, registry, current_agent_name).await;
        }
        // ── Models ──
        "models" => {
            display_models(current_agent, current_agent_name);
        }
        // ── Retry ──
        "retry" => {
            execute_retry_command(
                messages,
                current_agent,
                current_mode,
                token_tracker,
                stdin_rx,
            )
            .await;
        }
        // ── Model (switch agent) ──
        model_cmd if model_cmd.starts_with("model") => {
            execute_switch_agent(
                model_cmd,
                current_agent,
                current_agent_name,
                current_mode,
                registry,
            )
            .await;
        }
        _ => {
            eprintln!("{}", tf("cli.chat.unknown_command", &[("cmd", cmd)]));
        }
    }
    false
}

/// Process a user message through injection detection, run the agent, and auto-save.
#[allow(clippy::too_many_arguments)]
pub(super) async fn process_user_message_and_run_agent(
    line: &str,
    messages: &mut Vec<Message>,
    current_agent: &Arc<dyn Agent>,
    current_agent_name: &str,
    token_tracker: &mut TokenTracker,
    current_mode: &mut Box<dyn ModeRuntime>,
    session_path: &std::path::Path,
    stdin_rx: &mut mpsc::Receiver<String>,
) {
    // ── Prompt injection detection ──
    {
        use crate::security::severity::DetectionSeverity as InjectionSeverity;
        let detector = injection_detector();
        let (sanitized, result) = detector.detect_and_sanitize(line);

        if result.detected {
            for v in &result.violations {
                warn!(
                    target: "cli_injection",
                    category = ?v.category,
                    severity = ?v.base.severity,
                    pattern_id = ?v.pattern_id,
                    description = %v.base.description,
                    "prompt injection detected in user input"
                );
            }

            if detector.should_block(&result, InjectionSeverity::High) {
                let critical: Vec<String> = result
                    .violations
                    .iter()
                    .filter(|v| v.base.severity >= InjectionSeverity::High)
                    .map(|v| format!("{:?}: {}", v.category, v.base.description))
                    .collect();
                eprintln!(
                    "{}{}{}",
                    ansi!("31"),
                    tf(
                        "cli.chat.injection_blocked",
                        &[("violations", &critical.join("; "))]
                    ),
                    ansi!("0")
                );
                return;
            }

            eprintln!(
                "{}{}{}",
                ansi!("33"),
                tf(
                    "cli.chat.injection_warning",
                    &[("score", &format!("{:.2}", result.contamination_score))]
                ),
                ansi!("0")
            );
            messages.push(Message {
                role: "user".to_string(),
                content: sanitized,
            });
        } else {
            messages.push(Message {
                role: "user".to_string(),
                content: line.to_string(),
            });
        }
    }

    eprint!("{}🤖 {}", ansi!("1"), ansi!("0"));
    std::io::Write::flush(&mut std::io::stdout()).ok();

    let principles = build_cli_principles();

    match run_agent_with_tools(
        current_agent,
        messages,
        principles,
        Some(current_mode.as_ref()),
        stdin_rx,
    )
    .await
    {
        Ok((resp, prompt_tokens, completion_tokens)) => {
            record_turn_usage(token_tracker, prompt_tokens, completion_tokens, &resp);
        }
        Err(e) => {
            let err_msg = tf("error.generation_failed", &[("reason", &e.to_string())]);
            eprintln!("\n{}⚠️  {} {}", ansi!("31"), err_msg, ansi!("0"));
            // Clean up the failed assistant message to avoid token waste on retry
            if messages.last().map(|m| m.role.as_str()) == Some("assistant") {
                let last_empty = messages
                    .last()
                    .map(|m| m.content.is_empty())
                    .unwrap_or(false);
                if last_empty {
                    messages.pop();
                }
            }
        }
    }

    // ── Auto-save session every turn ──
    auto_save_turn(messages, current_agent_name, current_mode, session_path);

    // ── Compact prompt threshold check ──
    check_compact_threshold(messages);
}

// ─────────────────────────────────────────────────────────────────────────────
// Command handler helper functions
// ─────────────────────────────────────────────────────────────────────────────

async fn execute_compact_command(messages: &mut Vec<Message>, current_agent: &Arc<dyn Agent>) {
    if messages.len() < 4 {
        eprintln!(
            "{}Conversation too short to compact.{}",
            ansi!("33"),
            ansi!("0")
        );
        return;
    }
    let keep_front = 1.min(messages.len());
    let keep_back = 2.min(messages.len().saturating_sub(keep_front));
    let compact_range = keep_front..(messages.len() - keep_back);
    let compact_count = compact_range.len();
    if compact_count == 0 {
        eprintln!("{}No messages to compact.{}", ansi!("33"), ansi!("0"));
        return;
    }

    eprintln!(
        "{}Summarizing {} messages with LLM...{}",
        ansi!("33"),
        compact_count,
        ansi!("0")
    );

    let to_compact: Vec<Message> = messages[compact_range.clone()].to_vec();
    let summarize_prompt = Message {
        role: "user".to_string(),
        content: format!(
            "Please provide a concise summary of the above conversation. \
             Focus on: what has been accomplished, what decisions were made, \
             what the current state of the project/task is, and what remains to be done. \
             This summary replaces {} conversation turns, so include enough detail \
             (file paths, important findings, key decisions) that the conversation \
             can continue seamlessly without losing context.",
            compact_count
        ),
    };

    let mut summarize_msgs = to_compact;
    summarize_msgs.push(summarize_prompt);

    let (summarize_task, mut summary_rx) =
        spawn_agent_chat(Arc::clone(current_agent), summarize_msgs, None);

    // Bounded collection: the summary is inserted back into the session and
    // re-sent to the model on subsequent requests, so an unbounded stream
    // would inflate every later request's context.
    let summary_text =
        crate::acp::helpers::conversation::drain_channel_capped(&mut summary_rx).await;

    if let Err(e) = summarize_task.await {
        eprintln!(
            "{}{}{}",
            ansi!("31"),
            tf(
                "cli.chat.summarization_failed",
                &[("reason", &e.to_string())]
            ),
            ansi!("0")
        );
        return;
    }

    messages.drain(compact_range);
    messages.insert(
        1,
        Message {
            role: "user".to_string(),
            content: format!(
                "[Conversation compacted: summary of previous {} messages]\n{}",
                compact_count,
                summary_text.trim()
            ),
        },
    );
    eprintln!(
        "{}Compacted {} messages. {} messages remaining.{}",
        ansi!("32"),
        compact_count,
        messages.len(),
        ansi!("0")
    );
}

#[allow(clippy::borrowed_box)]
async fn execute_plan_command(
    messages: &mut Vec<Message>,
    current_agent: &Arc<dyn Agent>,
    registry: &Arc<AgentRegistry>,
    current_agent_name: &str,
    _current_mode: &Box<dyn ModeRuntime>,
    stdin_rx: &mut mpsc::Receiver<String>,
) {
    if messages.is_empty() {
        eprintln!(
            "{}No conversation to derive a plan from.{}",
            ansi!("33"),
            ansi!("0")
        );
        return;
    }

    let plan_runtime = resolve_mode_runtime(
        "plan",
        Some(registry.clone()),
        Some(current_agent_name.to_string()),
    );
    match plan_runtime {
        Ok(plan_mode) => {
            eprint!(
                "{}Generating execution plan with Plan mode constraints...{}",
                ansi!("90"),
                ansi!("0")
            );
            std::io::Write::flush(&mut std::io::stdout()).ok();

            let max_calls = plan_mode.max_tool_calls();
            let allowed = plan_mode.allowed_tools();
            eprintln!(
                "{}[Plan Mode] max_tool_calls={}, allowed_tools={:?}{}",
                ansi!("90"),
                max_calls,
                allowed,
                ansi!("0")
            );

            let plan_principles = build_cli_principles();
            match run_agent_with_tools(
                current_agent,
                messages,
                plan_principles,
                Some(plan_mode.as_ref()),
                stdin_rx,
            )
            .await
            {
                Ok((plan, _, _)) => {
                    eprintln!(
                        "\r{}── Execution Plan (Plan Mode) ──{}",
                        ansi!("1"),
                        ansi!("0")
                    );
                    eprintln!("{}", plan);
                }
                Err(e) => eprintln!(
                    "\r{}Plan generation failed: {}{}",
                    ansi!("31"),
                    e,
                    ansi!("0")
                ),
            }
        }
        Err(e) => {
            eprintln!(
                "{}Failed to create Plan runtime: {}. Falling back to simple chat.{}",
                ansi!("31"),
                e,
                ansi!("0")
            );
            let context: Vec<String> = messages
                .iter()
                .filter(|m| m.role != "system")
                .take(10)
                .map(|m| {
                    format!(
                        "{}: {}",
                        m.role,
                        m.content.chars().take(500).collect::<String>()
                    )
                })
                .collect();
            let context_str = context.join("\n---\n");
            let plan_prompt_msg = Message {
                role: "user".to_string(),
                content: format!(
                    "Based on this conversation, create a structured execution plan.\
                     \nList specific steps with file paths where relevant.\
                     \nFormat as a numbered list.\n\nConversation:\n{}",
                    context_str
                ),
            };
            match chat_simple(current_agent, vec![plan_prompt_msg], vec![]).await {
                Ok(plan) => {
                    eprintln!(
                        "\r{}── Execution Plan (fallback) ──{}",
                        ansi!("1"),
                        ansi!("0")
                    );
                    eprintln!("{}", plan);
                }
                Err(e) => eprintln!(
                    "\r{}Plan generation failed: {}{}",
                    ansi!("31"),
                    e,
                    ansi!("0")
                ),
            }
        }
    }
}

async fn execute_find_path_command(find_cmd: &str) {
    let pattern = find_cmd
        .strip_prefix("find_path ")
        .or_else(|| find_cmd.strip_prefix("find "))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());
    match pattern {
        Some(pattern) => {
            let args = serde_json::json!({"pattern": pattern});
            match execute_simple_tool("search_files", &args).await {
                Ok(result) => eprintln!("{}", result),
                Err(e) => eprintln!("{}Error: {}{}", ansi!("31"), e, ansi!("0")),
            }
        }
        None => eprintln!("{}", t("cli.chat.find_path_usage")),
    }
}

async fn execute_mode_command(
    mode_cmd: &str,
    current_mode: &mut Box<dyn ModeRuntime>,
    registry: &Arc<AgentRegistry>,
    current_agent_name: &str,
) {
    let rest = mode_cmd.strip_prefix("mode").unwrap_or("");
    let name = if rest.is_empty() || rest == " " {
        ""
    } else {
        rest.trim()
    };
    if name.is_empty() {
        eprintln!("{}", t("cli.chat.available_modes"));
        eprintln!(
            "{}",
            tf(
                "cli.chat.current_mode",
                &[("mode", &format!("{:?}", current_mode.kind()))]
            )
        );
        eprintln!("{}", t("cli.chat.usage_mode"));
    } else {
        let canonical = match name.to_lowercase().as_str() {
            "edit" => "edit",
            "ask" => "ask",
            "plan" => "plan",
            "safeguard" | "safe_guard" => "safeguard",
            "full_auto" | "fullauto" => "full_auto",
            _ => {
                eprintln!(
                    "{}{}{}",
                    ansi!("31"),
                    tf("cli.chat.unknown_mode", &[("mode", name)]),
                    ansi!("0")
                );
                return;
            }
        };
        match resolve_mode_runtime(
            canonical,
            Some(registry.clone()),
            Some(current_agent_name.to_string()),
        ) {
            Ok(runtime) => {
                *current_mode = runtime;
                eprintln!(
                    "{}{}{}",
                    ansi!("32"),
                    tf("cli.chat.switched_mode", &[("mode", canonical)]),
                    ansi!("0")
                );
                // Persist mode to config for next session
                let config_path = std::path::Path::new("goon-cli-mode.json");
                if let Ok(content) = std::fs::read_to_string(config_path) {
                    if let Ok(mut state) = serde_json::from_str::<serde_json::Value>(&content) {
                        if let Some(obj) = state.as_object_mut() {
                            obj.insert("mode".to_string(), serde_json::json!(canonical));
                            if let Ok(json) = serde_json::to_string_pretty(&state) {
                                let _ = std::fs::write(config_path, json);
                            }
                        }
                    }
                } else {
                    let state = serde_json::json!({"mode": canonical});
                    if let Ok(json) = serde_json::to_string_pretty(&state) {
                        let _ = std::fs::write(config_path, json);
                    }
                }
                match current_mode.kind() {
                    ModeKind::SafeGuard => eprintln!(
                        "{}{}{}",
                        ansi!("90"),
                        t("cli.chat.mode_safeguard_desc"),
                        ansi!("0")
                    ),
                    ModeKind::FullAuto => eprintln!(
                        "{}{}{}",
                        ansi!("33"),
                        t("cli.chat.mode_full_auto_desc"),
                        ansi!("0")
                    ),
                    ModeKind::Edit => eprintln!(
                        "{}{}{}",
                        ansi!("90"),
                        t("cli.chat.mode_edit_desc"),
                        ansi!("0")
                    ),
                    _ => {}
                }
            }
            Err(e) => eprintln!(
                "{}{}{}",
                ansi!("31"),
                tf("cli.chat.mode_switch_failed", &[("reason", &e.to_string())]),
                ansi!("0")
            ),
        }
    }
}

#[allow(clippy::borrowed_box)]
async fn execute_retry_command(
    messages: &mut Vec<Message>,
    current_agent: &Arc<dyn Agent>,
    current_mode: &Box<dyn ModeRuntime>,
    token_tracker: &mut TokenTracker,
    stdin_rx: &mut mpsc::Receiver<String>,
) {
    if messages.len() < 2 {
        eprintln!(
            "{}{}{}",
            ansi!("33"),
            t("cli.chat.no_messages_retry"),
            ansi!("0")
        );
        return;
    }
    let last_user_idx = messages.iter().rposition(|m| m.role == "user");
    match last_user_idx {
        Some(idx) => {
            let last_user_msg = messages[idx].content.clone();
            messages.truncate(idx + 1);
            let preview: String = last_user_msg.chars().take(60).collect();
            eprintln!(
                "{}{}{}",
                ansi!("33"),
                tf("cli.chat.retrying_message", &[("preview", &preview)]),
                ansi!("0")
            );
            let principles = build_cli_principles();
            match run_agent_with_tools(
                current_agent,
                messages,
                principles,
                Some(current_mode.as_ref()),
                stdin_rx,
            )
            .await
            {
                Ok((resp, prompt_tokens, completion_tokens)) => {
                    record_turn_usage(token_tracker, prompt_tokens, completion_tokens, &resp);
                }
                Err(e) => eprintln!(
                    "\n{}{}{}",
                    ansi!("31"),
                    tf("cli.chat.retry_failed", &[("reason", &e.to_string())]),
                    ansi!("0")
                ),
            }
        }
        None => eprintln!(
            "{}{}{}",
            ansi!("33"),
            t("cli.chat.no_user_message_retry"),
            ansi!("0")
        ),
    }
}

async fn execute_switch_agent(
    model_cmd: &str,
    current_agent: &mut Arc<dyn Agent>,
    current_agent_name: &mut String,
    current_mode: &mut Box<dyn ModeRuntime>,
    registry: &Arc<AgentRegistry>,
) {
    let rest = model_cmd.strip_prefix("model").unwrap_or("");
    let name = if rest.is_empty() || rest == " " {
        ""
    } else {
        rest.trim()
    };
    if name.is_empty() {
        let names = registry.names();
        eprintln!(
            "{}",
            tf("cli.chat.available_agents", &[("names", &names.join(", "))])
        );
        eprintln!(
            "{}",
            tf("cli.chat.current_agent", &[("name", current_agent_name)])
        );
        eprintln!("{}", t("cli.chat.usage_model"));
    } else if let Some(new_agent) = registry.get(name) {
        *current_agent = new_agent;
        *current_agent_name = name.to_string();
        let mode_str = mode_kind_str(current_mode.kind());
        if let Ok(runtime) = resolve_mode_runtime(
            mode_str,
            Some(registry.clone()),
            Some(current_agent_name.clone()),
        ) {
            *current_mode = runtime;
        }
        eprintln!(
            "{}{}{}",
            ansi!("32"),
            tf("cli.chat.switched_agent", &[("name", name)]),
            ansi!("0")
        );
    } else {
        let names = registry.names();
        eprintln!(
            "{}{}{}",
            ansi!("31"),
            tf(
                "cli.chat.agent_not_found",
                &[("name", name), ("names", &names.join(", "))]
            ),
            ansi!("0")
        );
    }
}

/// Spawn `agent.chat` on a background task with a fresh unbounded token
/// channel, returning the task handle and the token receiver the caller
/// drains. Single copy of the channel + `StreamingSender` + `tokio::spawn`
/// scaffolding (previously duplicated across four call sites).
pub(super) fn spawn_agent_chat(
    agent: Arc<dyn Agent>,
    messages: Vec<Message>,
    principles: Option<Vec<String>>,
) -> (
    tokio::task::JoinHandle<crate::core::error::Result<()>>,
    mpsc::UnboundedReceiver<String>,
) {
    let (tx, rx) = mpsc::unbounded_channel::<String>();
    let sender = StreamingSender::from(tx);
    let handle = tokio::spawn(async move { agent.chat(messages, principles, None, sender).await });
    (handle, rx)
}
