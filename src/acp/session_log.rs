//! Append-only session log — the factual event source for a conversation (M1.4).
//!
//! A conversation's *session log* is the authoritative, append-only record of
//! the events a conversation produced. The milestone invariant is
//! **"模型可见 ⇒ 日志可重建"** ("model-visible ⇒ log-rebuildable"): any message
//! history exposed to the model must be reconstructible from the log alone via
//! [`SessionLog::derive_messages`], the canonical projection.
//!
//! # Event model
//!
//! Every event is one of:
//! - [`SessionEvent::UserMessage`] — a user turn (projects to role `user`).
//! - [`SessionEvent::AssistantMessage`] — an assistant turn's visible text (role `assistant`).
//! - [`SessionEvent::ThoughtChunk`] — a slice of assistant reasoning.
//! - [`SessionEvent::ToolCall`] — a tool invocation that accompanied an assistant turn.
//! - [`SessionEvent::ToolResult`] — the outcome of a tool invocation.
//!
//! # Projection rules (`derive_messages`)
//!
//! The projection is a single left-to-right pass over the events:
//! - `UserMessage` closes the current assistant turn (if any) and emits a
//!   `user` message with the event's content.
//! - `AssistantMessage` closes the previous assistant turn and opens a new one.
//!   A pending *thought-only* turn (reasoning streamed before any visible
//!   text) is folded into the new message as its prefix rather than emitted
//!   separately, so a turn's reasoning and its text stay one message.
//! - `ThoughtChunk` folds into assistant content with a `\n\n` separator:
//!   before the first `AssistantMessage` of a turn it becomes the prefix of
//!   that turn's content; after text it becomes a suffix. A trailing thought
//!   chunk with no following text still projects as an `assistant` message
//!   (it was model-visible reasoning).
//! - `ToolCall` is **not** projected as a message: it accompanies the
//!   assistant turn that decided to invoke the tool (the open turn, or the
//!   most recently emitted assistant message). The tool name remains
//!   recoverable from the paired `ToolResult` block. A `ToolCall` with no
//!   preceding assistant turn (a log that starts mid-turn) is skipped, and the
//!   following `ToolResult` then also finds no assistant context and is
//!   skipped, keeping the projection well-formed.
//! - `ToolResult` is appended to the assistant turn's content as a block in
//!   the exact shape the runtime uses for model-visible tool results
//!   ([`crate::orchestration::autonomy_runtime::build_tool_result_block`]):
//!   `[Tool result: <name>]\n<output>\n[/Tool result]` (or `[Tool error: ...]`
//!   when `ok == false`). When the assistant message that *follows* the result
//!   already carries the identical block (some pipelines bake tool results
//!   into the next assistant text), the block is not appended a second time.
//!
//! # Boundaries (this round)
//!
//! Only the ACP `session/prompt` path appends events today — a
//! `UserMessage` + `AssistantMessage` pair per completed turn (see
//! `protocol_pack/session.rs`). Tool-call events will be logged by the
//! autonomy-loop integration in a follow-up. Fork/resume/replay are **not**
//! yet migrated to derive exclusively from the log; this module establishes
//! the authoritative append-only record, its projection, and the debug
//! invariant, and that migration is the next step of M1.4.

use serde::{Deserialize, Serialize};

use crate::agent::Message;

/// A single append-only event recorded for a conversation.
///
/// Serialization support lets the log be persisted or shipped inside snapshots
/// (e.g. debug dumps / audit replay) later.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionEvent {
    /// A user turn. Projects to a `user` message.
    UserMessage {
        /// The user-visible prompt text.
        content: String,
    },
    /// An assistant turn's visible text. Projects to an `assistant` message.
    AssistantMessage {
        /// The assistant's reply text.
        content: String,
    },
    /// A slice of assistant reasoning, folded into the assistant content of
    /// the turn it belongs to (see the module docs for the fold rule).
    ThoughtChunk {
        /// The reasoning text.
        content: String,
    },
    /// A tool invocation that accompanied the preceding assistant turn.
    /// Never projected as its own message.
    ToolCall {
        /// The invoked tool's name.
        name: String,
        /// The tool arguments as given by the assistant.
        arguments: serde_json::Value,
    },
    /// The outcome of a tool invocation; projected as a tool-result block on
    /// the assistant turn's content (see the module docs).
    ToolResult {
        /// The invoked tool's name.
        name: String,
        /// Whether the invocation succeeded (`false` → `[Tool error: ...]`).
        ok: bool,
        /// The tool's textual output (or error message).
        output: String,
    },
}

/// Append-only event log for one `(conversation_id, branch_id)` pair.
///
/// Events are only ever appended ([`SessionLog::append`]); nothing in the
/// runtime mutates or removes recorded events, which is what makes the log a
/// trustworthy factual source for replay and history derivation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionLog {
    events: Vec<SessionEvent>,
}

/// Assistant turn currently being assembled but not yet flushed into the
/// projected message list.
struct OpenTurn {
    content: String,
    /// True once any assistant text or tool-result block has been folded in.
    /// A thought-only open turn (false) is a prefix of the assistant turn that
    /// follows it and is merged into that message instead of being emitted on
    /// its own.
    material: bool,
}

impl SessionLog {
    /// Append one event to the log (append-only by construction).
    pub fn append(&mut self, event: SessionEvent) {
        self.events.push(event);
    }

    /// Number of recorded events.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether the log holds no events yet.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Iterate over the recorded events in append order.
    pub fn iter(&self) -> impl Iterator<Item = &SessionEvent> {
        self.events.iter()
    }

    /// The recorded events as a slice, in append order.
    pub fn as_slice(&self) -> &[SessionEvent] {
        &self.events
    }

    /// Canonical projection of the log into model-visible [`Message`] history.
    ///
    /// See the module docs for the exact per-event rules. The projection is
    /// deterministic and order-preserving: messages appear in the same order
    /// the underlying events were appended.
    pub fn derive_messages(&self) -> Vec<Message> {
        let mut out: Vec<Message> = Vec::new();
        // Assistant turn currently being assembled but not yet flushed into
        // `out`. Thought chunks and tool-result blocks fold into it.
        let mut pending: Option<OpenTurn> = None;

        let events = self.events.as_slice();
        for (i, event) in events.iter().enumerate() {
            match event {
                SessionEvent::UserMessage { content } => {
                    flush_assistant(&mut out, &mut pending);
                    out.push(Message {
                        role: "user".to_string(),
                        content: content.clone(),
                    });
                }
                SessionEvent::AssistantMessage { content } => {
                    // A new AssistantMessage closes the previous assistant turn
                    // and opens the next one — unless the previous turn is
                    // thought-only, in which case the thought is this message's
                    // reasoning prefix and folds in instead of being emitted
                    // separately.
                    match pending.take() {
                        Some(open) if !open.material => {
                            let merged = if open.content.is_empty() {
                                content.clone()
                            } else {
                                format!("{}\n\n{}", open.content, content)
                            };
                            pending = Some(OpenTurn {
                                content: merged,
                                material: true,
                            });
                        }
                        Some(open) => {
                            out.push(Message {
                                role: "assistant".to_string(),
                                content: open.content,
                            });
                            pending = Some(OpenTurn {
                                content: content.clone(),
                                material: true,
                            });
                        }
                        None => {
                            pending = Some(OpenTurn {
                                content: content.clone(),
                                material: true,
                            });
                        }
                    }
                }
                SessionEvent::ThoughtChunk { content } => match pending.as_mut() {
                    Some(open) => {
                        // Suffix fold: "text\n\nthought".
                        if !open.content.is_empty() {
                            open.content.push_str("\n\n");
                        }
                        open.content.push_str(content);
                    }
                    None => {
                        // Prefix fold: seeds the assistant turn; the following
                        // AssistantMessage merges with it (see above).
                        pending = Some(OpenTurn {
                            content: content.clone(),
                            material: false,
                        });
                    }
                },
                SessionEvent::ToolCall { .. } => {
                    // Not projected as a message: a tool call accompanies the
                    // assistant turn that decided to invoke the tool (the open
                    // pending turn, or the most recently emitted assistant
                    // message). A ToolCall with no preceding assistant turn has
                    // nothing to attach to and is skipped — the paired
                    // ToolResult then also finds no assistant context and is
                    // skipped, keeping the projection well-formed.
                }
                SessionEvent::ToolResult { name, ok, output } => {
                    if pending.is_none() && !matches!(out.last(), Some(m) if m.role == "assistant")
                    {
                        // No assistant turn to attach the result to: skip.
                        continue;
                    }
                    let block = crate::orchestration::autonomy_runtime::build_tool_result_block(
                        name, output, !ok,
                    );
                    // Dedup guard: when the assistant message that follows this
                    // result already carries the identical block (some pipelines
                    // bake tool results into the next assistant text), do not
                    // append it a second time. The scan stops at the first
                    // assistant message or user boundary after the result.
                    let carried_by_following_assistant = events[i + 1..]
                        .iter()
                        .find_map(|next| match next {
                            SessionEvent::ThoughtChunk { .. } => None,
                            SessionEvent::AssistantMessage { content } => Some(content.as_str()),
                            SessionEvent::UserMessage { .. } => Some(""),
                            _ => None,
                        })
                        .is_some_and(|content| content.contains(&block));
                    if !carried_by_following_assistant {
                        append_tool_block(&mut out, &mut pending, block);
                    }
                }
            }
        }
        flush_assistant(&mut out, &mut pending);
        out
    }
}

/// Flush the in-progress assistant turn (if any) into `out` as a message.
fn flush_assistant(out: &mut Vec<Message>, pending: &mut Option<OpenTurn>) {
    if let Some(open) = pending.take() {
        out.push(Message {
            role: "assistant".to_string(),
            content: open.content,
        });
    }
}

/// Append a tool-result block to the open assistant turn, or to the most
/// recently emitted assistant message when no turn is open.
fn append_tool_block(out: &mut [Message], pending: &mut Option<OpenTurn>, block: String) {
    match pending.as_mut() {
        Some(open) => {
            if !open.content.is_empty() {
                open.content.push('\n');
            }
            open.content.push_str(&block);
            open.material = true;
        }
        None => {
            if let Some(last) = out.last_mut() {
                if last.role == "assistant" {
                    if !last.content.is_empty() {
                        last.content.push('\n');
                    }
                    last.content.push_str(&block);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn empty_log_projects_empty_messages() {
        let log = SessionLog::default();
        assert_eq!(log.len(), 0);
        assert!(log.is_empty());
        assert!(log.derive_messages().is_empty());
        assert!(log.iter().next().is_none());
        assert!(log.as_slice().is_empty());
    }

    #[test]
    fn append_and_derive_round_trip_for_realistic_turn() {
        // user → assistant (thought + text) → tool_call → tool_result → assistant
        let mut log = SessionLog::default();
        log.append(SessionEvent::UserMessage {
            content: "list the files".to_string(),
        });
        log.append(SessionEvent::ThoughtChunk {
            content: "I need to inspect the workspace".to_string(),
        });
        log.append(SessionEvent::AssistantMessage {
            content: "Let me list the files.".to_string(),
        });
        log.append(SessionEvent::ToolCall {
            name: "list_directory".to_string(),
            arguments: json!({ "path": "src" }),
        });
        log.append(SessionEvent::ToolResult {
            name: "list_directory".to_string(),
            ok: true,
            output: "src/acp\nsrc/agent".to_string(),
        });
        log.append(SessionEvent::AssistantMessage {
            content: "Here is the layout: src/acp, src/agent".to_string(),
        });

        assert_eq!(log.len(), 6);
        let derived = log.derive_messages();
        assert_eq!(derived.len(), 3);

        // user turn
        assert_eq!(derived[0].role, "user");
        assert_eq!(derived[0].content, "list the files");

        // assistant turn that made the call: thought (folded with "\n\n") +
        // text + the tool-result block in the runtime's exact shape
        assert_eq!(derived[1].role, "assistant");
        assert_eq!(
            derived[1].content,
            "I need to inspect the workspace\n\nLet me list the files.\n\
             [Tool result: list_directory]\nsrc/acp\nsrc/agent\n[/Tool result]"
        );

        // final assistant turn
        assert_eq!(derived[2].role, "assistant");
        assert_eq!(derived[2].content, "Here is the layout: src/acp, src/agent");
    }

    #[test]
    fn ordering_preserved_across_multiple_turns() {
        let mut log = SessionLog::default();
        log.append(SessionEvent::UserMessage {
            content: "first prompt".to_string(),
        });
        log.append(SessionEvent::AssistantMessage {
            content: "first response".to_string(),
        });
        log.append(SessionEvent::UserMessage {
            content: "second prompt".to_string(),
        });
        log.append(SessionEvent::AssistantMessage {
            content: "second response".to_string(),
        });

        let derived = log.derive_messages();
        let roles = derived
            .iter()
            .map(|m| (m.role.as_str(), m.content.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            roles,
            vec![
                ("user", "first prompt"),
                ("assistant", "first response"),
                ("user", "second prompt"),
                ("assistant", "second response"),
            ]
        );
    }

    #[test]
    fn thought_chunk_folds_into_following_assistant_turn_as_prefix() {
        let mut log = SessionLog::default();
        log.append(SessionEvent::UserMessage {
            content: "hi".to_string(),
        });
        log.append(SessionEvent::ThoughtChunk {
            content: "reasoning".to_string(),
        });
        log.append(SessionEvent::AssistantMessage {
            content: "answer".to_string(),
        });

        let derived = log.derive_messages();
        assert_eq!(derived.len(), 2);
        assert_eq!(derived[0].role, "user");
        assert_eq!(derived[1].role, "assistant");
        assert_eq!(derived[1].content, "reasoning\n\nanswer");
    }

    #[test]
    fn thought_chunk_after_text_folds_as_suffix() {
        let mut log = SessionLog::default();
        log.append(SessionEvent::AssistantMessage {
            content: "text".to_string(),
        });
        log.append(SessionEvent::ThoughtChunk {
            content: "more reasoning".to_string(),
        });
        log.append(SessionEvent::AssistantMessage {
            content: "final".to_string(),
        });

        let derived = log.derive_messages();
        assert_eq!(derived.len(), 2);
        assert_eq!(derived[0].content, "text\n\nmore reasoning");
        assert_eq!(derived[1].content, "final");
    }

    #[test]
    fn trailing_thought_chunk_projects_as_assistant_message() {
        let mut log = SessionLog::default();
        log.append(SessionEvent::UserMessage {
            content: "question".to_string(),
        });
        log.append(SessionEvent::ThoughtChunk {
            content: "thinking without a reply yet".to_string(),
        });

        let derived = log.derive_messages();
        assert_eq!(derived.len(), 2);
        assert_eq!(derived[0].role, "user");
        assert_eq!(derived[1].role, "assistant");
        assert_eq!(derived[1].content, "thinking without a reply yet");
    }

    #[test]
    fn tool_call_without_preceding_assistant_is_skipped() {
        // A log that starts mid-turn: the tool call has no assistant message
        // to accompany, so neither the call nor the result is projected.
        let mut log = SessionLog::default();
        log.append(SessionEvent::ToolCall {
            name: "read_file".to_string(),
            arguments: json!({ "path": "src/main.rs" }),
        });
        log.append(SessionEvent::ToolResult {
            name: "read_file".to_string(),
            ok: true,
            output: "fn main() {}".to_string(),
        });

        assert!(log.derive_messages().is_empty());
    }

    #[test]
    fn tool_result_error_projects_error_block_shape() {
        let mut log = SessionLog::default();
        log.append(SessionEvent::UserMessage {
            content: "run it".to_string(),
        });
        log.append(SessionEvent::AssistantMessage {
            content: "Running now.".to_string(),
        });
        log.append(SessionEvent::ToolCall {
            name: "shell_exec".to_string(),
            arguments: json!({ "command": "cargo check" }),
        });
        log.append(SessionEvent::ToolResult {
            name: "shell_exec".to_string(),
            ok: false,
            output: "error: unresolved import".to_string(),
        });

        let derived = log.derive_messages();
        assert_eq!(derived.len(), 2);
        assert_eq!(
            derived[1].content,
            "Running now.\n[Tool error: shell_exec]\nerror: unresolved import\n[/Tool error]"
        );
    }

    #[test]
    fn tool_result_not_duplicated_when_following_assistant_carries_block() {
        // The runtime can bake the tool result into the next assistant text;
        // the projection must not append the block a second time.
        let block = "[Tool result: grep]\nfound it\n[/Tool result]";
        let mut log = SessionLog::default();
        log.append(SessionEvent::UserMessage {
            content: "search".to_string(),
        });
        log.append(SessionEvent::AssistantMessage {
            content: "Checking.".to_string(),
        });
        log.append(SessionEvent::ToolCall {
            name: "grep".to_string(),
            arguments: json!({ "pattern": "todo" }),
        });
        log.append(SessionEvent::ToolResult {
            name: "grep".to_string(),
            ok: true,
            output: "found it".to_string(),
        });
        log.append(SessionEvent::AssistantMessage {
            content: format!("Search done.\n{}", block),
        });

        let derived = log.derive_messages();
        assert_eq!(derived.len(), 3);
        // The tool-calling turn keeps only its own text — no block appended.
        assert_eq!(derived[1].content, "Checking.");
        // The final assistant message already carries the block, verbatim.
        assert_eq!(derived[2].content, format!("Search done.\n{}", block));
    }
}
