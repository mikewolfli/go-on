//! Session persistence for the terminal chat loop: manual save/load, per-turn
//! auto-save, auto-compaction, and save-on-exit.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::agents::agent::{Agent, AgentRegistry, Message};
use crate::i18n::runtime::{t, tf};
use crate::orchestration::mode::{resolve_mode_runtime, ModeRuntime};

use super::ansi;
use super::{
    mode_kind_str, save_notify, AutoSaveGuard, AUTO_COMPACT_KEEP, AUTO_COMPACT_THRESHOLD,
    COMPACT_PROMPT_THRESHOLD, SAVE_IN_FLIGHT,
};

/// Session data for persistence.
#[derive(Serialize, Deserialize)]
struct ChatSession {
    messages: Vec<Message>,
    agent_name: String,
    #[serde(default)]
    mode: String,
}

/// Build the persisted `ChatSession` snapshot for the current conversation.
///
/// Shared by the three session-persistence paths (manual `/save`,
/// per-turn auto-save, save-on-exit) so the serialized shape cannot drift
/// between them. Each caller keeps its own JSON serializer (`to_string_pretty`
/// for the user-facing `/save`, compact `to_string` for background saves).
#[allow(clippy::borrowed_box)]
fn serialize_session(
    messages: &[Message],
    current_agent_name: &str,
    current_mode: &Box<dyn ModeRuntime>,
) -> ChatSession {
    ChatSession {
        messages: messages.to_vec(),
        agent_name: current_agent_name.to_string(),
        mode: mode_kind_str(current_mode.kind()).to_string(),
    }
}

#[allow(clippy::borrowed_box)]
pub(super) async fn handle_save_command(
    messages: &[Message],
    current_agent_name: &str,
    current_mode: &Box<dyn ModeRuntime>,
    session_path: &std::path::Path,
) {
    let session = serialize_session(messages, current_agent_name, current_mode);
    match serde_json::to_string_pretty(&session) {
        Ok(json) => match tokio::fs::write(session_path, &json).await {
            Ok(()) => eprintln!(
                "{}{}{}",
                ansi!("32"),
                tf(
                    "cli.chat.session_saved",
                    &[("path", &session_path.display().to_string())]
                ),
                ansi!("0")
            ),
            Err(e) => eprintln!(
                "{}{}{}",
                ansi!("31"),
                tf("cli.chat.session_save_failed", &[("error", &e.to_string())]),
                ansi!("0")
            ),
        },
        Err(e) => eprintln!(
            "{}{}{}",
            ansi!("31"),
            tf(
                "cli.chat.session_serialize_failed",
                &[("error", &e.to_string())]
            ),
            ansi!("0")
        ),
    }
}

pub(super) async fn handle_load_command(
    session_path: &std::path::Path,
    messages: &mut Vec<Message>,
    current_agent: &mut Arc<dyn Agent>,
    current_agent_name: &mut String,
    current_mode: &mut Box<dyn ModeRuntime>,
    registry: &Arc<AgentRegistry>,
) {
    match tokio::fs::read_to_string(session_path).await {
        Ok(json) => match serde_json::from_str::<ChatSession>(&json) {
            Ok(session) => {
                let agent_valid = registry.get(&session.agent_name).is_some();
                if !agent_valid {
                    eprintln!(
                        "{}{}{}",
                        ansi!("33"),
                        tf(
                            "cli.chat.session_load_agent_warn",
                            &[("agent", &session.agent_name)]
                        ),
                        ansi!("0")
                    );
                }
                *messages = session.messages;
                if agent_valid && session.agent_name != *current_agent_name {
                    if let Some(new_agent) = registry.get(&session.agent_name) {
                        *current_agent = new_agent;
                        *current_agent_name = session.agent_name.clone();
                        let mode_str = mode_kind_str(current_mode.kind());
                        if let Ok(runtime) = resolve_mode_runtime(
                            mode_str,
                            Some(registry.clone()),
                            Some(current_agent_name.clone()),
                        ) {
                            *current_mode = runtime;
                        }
                    }
                }
                if !session.mode.is_empty() {
                    let canonical = session.mode.to_lowercase();
                    if let Ok(runtime) = resolve_mode_runtime(
                        &canonical,
                        Some(registry.clone()),
                        Some(current_agent_name.clone()),
                    ) {
                        *current_mode = runtime;
                        eprintln!(
                            "{}{}{}",
                            ansi!("32"),
                            tf("cli.chat.restored_mode", &[("mode", &canonical)]),
                            ansi!("0")
                        );
                    }
                }
                eprintln!(
                    "{}{}{}",
                    ansi!("32"),
                    tf(
                        "cli.chat.session_loaded",
                        &[
                            ("count", &messages.len().to_string()),
                            ("agent", current_agent_name),
                            ("mode", &format!("{:?}", current_mode.kind())),
                        ]
                    ),
                    ansi!("0")
                );
            }
            Err(e) => eprintln!(
                "{}{}{}",
                ansi!("31"),
                tf(
                    "cli.chat.session_parse_failed",
                    &[("error", &e.to_string())]
                ),
                ansi!("0")
            ),
        },
        Err(_) => eprintln!(
            "{}{}{}",
            ansi!("33"),
            tf(
                "cli.chat.session_not_found",
                &[("path", &session_path.display().to_string())]
            ),
            ansi!("0")
        ),
    }
}

/// Auto-save the session after each turn.
#[allow(clippy::borrowed_box)]
pub(super) fn auto_save_turn(
    messages: &[Message],
    current_agent_name: &str,
    current_mode: &Box<dyn ModeRuntime>,
    session_path: &std::path::Path,
) {
    if messages.is_empty() || SAVE_IN_FLIGHT.load(Ordering::Acquire) {
        return;
    }
    SAVE_IN_FLIGHT.store(true, Ordering::Release);
    let json = serde_json::to_string(&serialize_session(
        messages,
        current_agent_name,
        current_mode,
    ))
    .unwrap_or_default();
    let path = session_path.to_path_buf();
    let guard = AutoSaveGuard;
    tokio::spawn(async move {
        if let Err(e) = tokio::fs::write(&path, &json).await {
            tracing::warn!("Failed to auto-save session: {e}");
        }
        drop(guard);
    });
}

/// Check conversation length and auto-compact if needed (SlidingWindow).
/// Keeps the last AUTO_COMPACT_KEEP messages when threshold is exceeded.
pub(super) fn check_compact_threshold(messages: &mut Vec<Message>) {
    let msg_count = messages.len();
    if (COMPACT_PROMPT_THRESHOLD..AUTO_COMPACT_THRESHOLD).contains(&msg_count) {
        eprintln!("{}{}{}", ansi!("33"), t("cli.chat.tip_compact"), ansi!("0"));
    }
    if msg_count >= AUTO_COMPACT_THRESHOLD {
        let keep = AUTO_COMPACT_KEEP;
        let remove_count = msg_count.saturating_sub(keep);
        // Preserve the system message at index 0 (tool inventory + __tool_call__
        // protocol framing) — the manual /compact path keeps messages[0] too,
        // but this path previously drained from index 0, silently deleting the
        // system framing after ~30 turns and degrading tool-calling.
        let drain_start = 1.min(messages.len().saturating_sub(keep));
        messages.drain(drain_start..remove_count);
        eprintln!(
            "{}{}{}",
            ansi!("32"),
            tf(
                "cli.chat.conversation_auto_compacted",
                &[
                    ("removed", &remove_count.to_string()),
                    ("remaining", &messages.len().to_string()),
                ]
            ),
            ansi!("0")
        );
    }
}

/// Save the session on exit.
#[allow(clippy::borrowed_box)]
pub(super) async fn save_session_on_exit(
    messages: &[Message],
    current_agent_name: &str,
    current_mode: &Box<dyn ModeRuntime>,
    session_path: &std::path::Path,
) {
    if messages.is_empty() {
        return;
    }
    if SAVE_IN_FLIGHT.load(Ordering::Acquire) {
        tokio::select! {
            _ = save_notify().notified() => {}
            _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
        }
    }
    let json = serde_json::to_string(&serialize_session(
        messages,
        current_agent_name,
        current_mode,
    ))
    .unwrap_or_default();
    if let Err(e) = tokio::fs::write(session_path, &json).await {
        tracing::warn!("Failed to save session on exit: {e}");
    } else {
        eprintln!("Session auto-saved");
    }
}
