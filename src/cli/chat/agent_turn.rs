//! Core agent turn pipeline for the terminal chat loop: streaming phase,
//! mode filtering + SafeGuard approval, tool execution, and follow-up.

use std::sync::Arc;

use anyhow::Result;
use tokio::signal;
use tokio::sync::mpsc;
use tokio::time::Duration;
use tracing::{debug, warn};

use crate::acp::helpers::autonomy::terminal_chat_contract_snapshot;
use crate::acp::helpers::context::run_with_optional_timeout;
use crate::acp::r#impl::chat::agent_runtime::{collect_agent_responses, CollectedResponse};
use crate::agents::agent::{Agent, Message};
use crate::cli::markdown_renderer::StreamMarkdownRenderer;
use crate::i18n::runtime::tf;
use crate::orchestration::autonomy_runtime::{
    build_tool_execution_followup_message, build_tool_result_block,
};
use crate::orchestration::mode::{ModeKind, ModeRuntime};
use crate::orchestration::tool::executor::{execute_tools_concurrent, ToolExecConfig};

use super::ansi;
use super::commands::spawn_agent_chat;
use super::tokens::{classify_token, TokenKind};
use super::{
    tool_registry, SafeguardApprovalResult, DEFAULT_FOLLOWUP_TIMEOUT_SECS, MAX_CONCURRENT_TOOLS,
    MAX_TOOLS_IN_FOLLOWUP, MAX_TOOL_RESULT_CHARS,
};

/// Run a single agent turn: agent chat → tool execution → followup.
/// Returns the response text and estimated token usage.
///
/// `principles` is passed in (pre-computed by caller) to avoid rebuilding
/// the tool/skill list twice per round (once for agent chat, once for follow-up).
/// The tool registry is static, so the list never changes between calls.
pub(super) async fn run_agent_with_tools(
    agent: &Arc<dyn Agent>,
    messages: &mut Vec<Message>,
    principles: Vec<String>,
    mode_runtime: Option<&dyn ModeRuntime>,
    stdin_rx: &mut mpsc::Receiver<String>,
) -> Result<(String, usize, usize)> {
    // ── Estimate prompt tokens from existing messages using CJK-aware estimator ──
    let estimated_prompt_tokens: usize = messages
        .iter()
        .map(|m| crate::shared::token_estimator::estimate_tokens(&m.content))
        .sum();

    // ── Phase 1: Agent streaming with Ctrl+C interrupt + reasoning + markdown ──
    let (mut response, tool_calls) =
        run_agent_streaming_phase(agent, messages, &principles).await?;

    // ── Phase 2 (inline): Filter/block tool calls by mode constraints + SafeGuard ──
    let filtered_calls = filter_tool_calls_by_mode(&tool_calls, mode_runtime);
    let (filtered_calls, early_exit) =
        safeguard_approval(&filtered_calls, mode_runtime, stdin_rx).await?;
    if early_exit {
        // Cancelled by SafeGuard: the agent already consumed the prompt tokens.
        let estimated_completion_tokens =
            crate::shared::token_estimator::estimate_tokens(&response);
        return Ok((
            response,
            estimated_prompt_tokens,
            estimated_completion_tokens,
        ));
    }

    // ── Phase 3: Tool execution with FuturesUnordered + semaphore ──
    let (tool_results, has_failure, followup_round_executed) =
        run_tool_execution_phase(filtered_calls).await;

    // ── Phase 4: Send tool results back as follow-up message ──
    if !tool_results.is_empty() {
        response = run_followup_phase(
            agent,
            messages,
            &principles,
            &tool_results,
            has_failure,
            &response,
        )
        .await;
    }

    // ── Append assistant response to history ──
    if !response.is_empty() {
        let last_is_assistant = messages
            .last()
            .map(|m| m.role == "assistant")
            .unwrap_or(false);
        if !last_is_assistant {
            messages.push(Message {
                role: "assistant".to_string(),
                content: response.clone(),
            });
        }
    }

    let autonomy_contract =
        terminal_chat_contract_snapshot(tool_calls.len(), followup_round_executed, &response);
    debug!(
        target: "go_on::cli::chat",
        autonomy_contract = %autonomy_contract,
        "terminal chat turn completed"
    );

    let estimated_completion_tokens = crate::shared::token_estimator::estimate_tokens(&response);
    Ok((
        response,
        estimated_prompt_tokens,
        estimated_completion_tokens,
    ))
}

/// How reasoning content is rendered in the shared streaming loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReasoningRenderStyle {
    /// Primary phase: every reasoning token gets a `💭` prefix.
    PerTokenThinking,
    /// Follow-up phase: ANSI color is toggled on at ReasoningStart and off at
    /// ReasoningEnd (no per-token prefix).
    ColorToggle,
}

/// Shared streaming-token rendering loop used by both the primary agent
/// phase and the follow-up phase: classifies each token (tool calls, reasoning
/// markers, thinking, telemetry), renders content through the markdown
/// renderer, and honours Ctrl+C. When `fwd_tx` is set, every raw token is also
/// forwarded to the shared response collector. Returns `true` when the stream
/// ended normally (receiver closed), `false` when interrupted by Ctrl+C — the
/// caller aborts its own agent task and prints the interrupt message.
async fn render_streaming_tokens(
    rx: &mut mpsc::UnboundedReceiver<String>,
    renderer: &mut StreamMarkdownRenderer,
    fwd_tx: Option<&mpsc::UnboundedSender<String>>,
    style: ReasoningRenderStyle,
) -> bool {
    let mut in_reasoning = false;
    loop {
        // Re-arm Ctrl+C each iteration: signal::ctrl_c() is a one-shot future.
        // Without this, the second Ctrl+C would be ignored.
        let ctrl_c = signal::ctrl_c();
        tokio::pin!(ctrl_c);
        tokio::select! {
            token = rx.recv() => {
                match token {
                    Some(token) => {
                        // Forward ALL tokens to the shared collector when wired.
                        if let Some(tx) = fwd_tx {
                            let _ = tx.send(token.clone());
                        }

                        match classify_token(&token) {
                            // Tool call notification
                            TokenKind::ToolCall(tool_name) => {
                                eprintln!(
                                    "{}🔧 [Tool call: {tool_name}]{}",
                                    ansi!("33"),
                                    ansi!("0")
                                );
                                continue;
                            }
                            // Reasoning content markers
                            TokenKind::ReasoningStart => {
                                in_reasoning = true;
                                if style == ReasoningRenderStyle::ColorToggle {
                                    eprint!("{}", ansi!("90"));
                                }
                                continue;
                            }
                            TokenKind::ReasoningEnd => {
                                in_reasoning = false;
                                if style == ReasoningRenderStyle::ColorToggle {
                                    eprint!("{}", ansi!("0"));
                                    eprintln!();
                                }
                                continue;
                            }
                            // __thinking__ prefixed tokens
                            TokenKind::Thinking(think) => {
                                eprint!("{}💭 {}{}", ansi!("90"), think, ansi!("0"));
                                continue;
                            }
                            // Skip finish_reason and usage telemetry tokens
                            TokenKind::Telemetry => continue,
                            TokenKind::Content => {}
                        }

                        if in_reasoning {
                            if style == ReasoningRenderStyle::PerTokenThinking {
                                eprint!("{}💭 {}{}", ansi!("90"), token, ansi!("0"));
                            } else {
                                eprint!("{}{}{}", ansi!("90"), token, ansi!("0"));
                            }
                        } else {
                            renderer.feed(&token);
                            let (formatted, _) = renderer.flush();
                            if !formatted.is_empty() {
                                eprint!("{}", formatted);
                            }
                        }
                        std::io::Write::flush(&mut std::io::stdout()).ok();
                    }
                    None => return true,
                }
            }
            _ = &mut ctrl_c => return false,
        }
    }
}

/// Phase 1: Stream the agent response with progressive markdown rendering,
/// reasoning markers, tool call notifications, and Ctrl+C interrupt handling.
/// Returns the collected response text and any tool calls emitted by the agent.
async fn run_agent_streaming_phase(
    agent: &Arc<dyn Agent>,
    messages: &[Message],
    principles: &[String],
) -> Result<(String, Vec<(String, String)>)> {
    let initial_principles = if principles.is_empty() {
        None
    } else {
        Some(principles.to_vec())
    };
    let (chat_task, mut rx) =
        spawn_agent_chat(Arc::clone(agent), messages.to_vec(), initial_principles);

    // Use a forwarding channel: progressive display loop sends all tokens
    // to the shared `collect_agent_responses` for final classification.
    let (fwd_tx, fwd_rx) = mpsc::unbounded_channel::<String>();

    let mut renderer = StreamMarkdownRenderer::new();

    // ── Progressive streaming display with interrupt support ──
    let completed = render_streaming_tokens(
        &mut rx,
        &mut renderer,
        Some(&fwd_tx),
        ReasoningRenderStyle::PerTokenThinking,
    )
    .await;
    if !completed {
        eprintln!(
            "\n{}Interrupted agent response. Use /clear to reset.{} ({})",
            ansi!("33"),
            ansi!("0"),
            if chat_task.is_finished() {
                "done"
            } else {
                "aborting"
            }
        );
        chat_task.abort();
    }

    // Drop the forwarding sender so the collector's receiver closes cleanly
    drop(fwd_tx);

    // Await the agent task
    match chat_task.await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => warn!("Agent chat failed: {e}"),
        Err(e) => {
            if e.is_cancelled() {
                debug!("Agent chat cancelled by user");
            } else {
                warn!("Agent chat task panicked: {e}");
            }
        }
    }

    // ── Collect the full response via shared core ──
    let CollectedResponse {
        response,
        reasoning: _reasoning_text,
        tool_calls,
    } = collect_agent_responses(fwd_rx).await.unwrap_or_else(|e| {
        warn!("collect_agent_responses failed: {e}");
        CollectedResponse {
            response: renderer.take_raw_response(),
            reasoning: String::new(),
            tool_calls: Vec::new(),
        }
    });

    // ── Flush remaining renderer output ──
    {
        let (remaining, _) = renderer.flush();
        if !remaining.is_empty() {
            let n = remaining.lines().count();
            for _ in 0..n {
                eprint!("\x1B[F\x1B[K");
            }
            eprintln!("{}", remaining);
        }
    }

    Ok((response, tool_calls))
}

/// Filter tool calls based on mode constraints (allowed tools, max_calls).
/// Delegates the policy decision to the shared `filter_tool_calls_by_policy`
/// (single implementation shared with the ACP chat path); this wrapper keeps
/// the CLI's stderr UX.
fn filter_tool_calls_by_mode(
    tool_calls: &[(String, String)],
    mode_runtime: Option<&dyn ModeRuntime>,
) -> Vec<(String, String)> {
    let kind = mode_runtime.map(|m| m.kind()).unwrap_or(ModeKind::Edit);
    let max_calls = mode_runtime.map(|m| m.max_tool_calls()).unwrap_or(20);
    let (filtered_calls, blocked) =
        crate::orchestration::mode::filter_tool_calls_by_policy(tool_calls, &kind);

    for name in &blocked {
        eprintln!(
            "{}{}{}",
            ansi!("33"),
            tf(
                "cli.chat.tool_blocked_by_mode",
                &[("tool_name", name), ("allowed", "")]
            ),
            ansi!("0")
        );
    }
    if !blocked.is_empty() {
        eprintln!(
            "{}{}{}",
            ansi!("33"),
            tf(
                "cli.chat.tool_call_blocked_by_mode",
                &[
                    ("blocked", &blocked.len().to_string()),
                    ("mode", &format!("{:?}", kind)),
                    ("max", &max_calls.to_string())
                ]
            ),
            ansi!("0")
        );
    }

    filtered_calls
}

/// SafeGuard mode: interactive approval of high-risk operations.
/// Returns `(filtered_calls, early_exit)`; `early_exit` is `true` when the
/// user cancelled execution. The caller reports real token usage in that case
/// (the prompt tokens were already consumed to produce the response).
///
/// Reads the y/N answer from the same stdin channel every other interactive
/// prompt uses: a direct `std::io::stdin().read_line()` here raced with the
/// background stdin task (setup_chat_environment), which could consume the
/// user's answer as a chat line and hang the blocking read on a tokio worker.
async fn safeguard_approval<'a>(
    filtered_calls: &'a [(String, String)],
    mode_runtime: Option<&dyn ModeRuntime>,
    stdin_rx: &mut mpsc::Receiver<String>,
) -> SafeguardApprovalResult<'a> {
    let mode_kind = mode_runtime.map(|m| m.kind());
    let is_safeguard = matches!(mode_kind, Some(ModeKind::SafeGuard));
    let is_high_risk = if is_safeguard {
        mode_runtime
            .map(|m| {
                m.is_high_risk_operation(
                    &filtered_calls
                        .iter()
                        .map(|(n, a)| format!("{}: {}", n, a))
                        .collect::<Vec<_>>()
                        .join(" "),
                )
            })
            .unwrap_or(false)
    } else {
        false
    };

    if is_safeguard && is_high_risk {
        eprintln!(
            "{}🔒 SafeGuard: High-risk operation detected. Review the planned tool calls:{} {}",
            ansi!("31"),
            ansi!("0"),
            filtered_calls
                .iter()
                .map(|(n, a)| format!("  ⚡ {}({})", n, a))
                .collect::<Vec<_>>()
                .join("\n")
        );
        eprint!(
            "{}Proceed with execution? [y/N]{} ",
            ansi!("33"),
            ansi!("0")
        );
        std::io::Write::flush(&mut std::io::stdout()).ok();
        let input = tokio::select! {
            line = stdin_rx.recv() => line.unwrap_or_default(),
            _ = signal::ctrl_c() => {
                eprintln!("\nCancelled.");
                return Ok((filtered_calls, true));
            }
        };
        if !input.trim().eq_ignore_ascii_case("y") {
            eprintln!(
                "{}SafeGuard: Operation cancelled by user.{}",
                ansi!("33"),
                ansi!("0")
            );
            return Ok((filtered_calls, true));
        }
    }

    Ok((filtered_calls, false))
}

/// Phase 2: Execute tools with progressive streaming via FuturesUnordered + Semaphore.
/// Returns (tool_results, has_failure, followup_round_executed).
async fn run_tool_execution_phase(
    filtered_calls: &[(String, String)],
) -> (Vec<String>, bool, bool) {
    let mut followup_round_executed = false;
    if filtered_calls.is_empty() {
        return (Vec::new(), false, followup_round_executed);
    }

    // ── Skill dedup: when AI calls multiple skills simultaneously, auto-select
    //    the one with the highest score and drop the rest. Non-skill tools are
    //    preserved. Shared logic with ACP's run_agent_collecting.
    let tool_calls = match crate::orchestration::tool::skill_registry() {
        Some(registry) => {
            let (deduped, best) =
                crate::orchestration::tool::dedup_skill_calls(filtered_calls, registry);
            if let Some(best_name) = best {
                eprintln!(
                    "  {}skill dedup: auto-selected '{}'{}",
                    ansi!("33"),
                    best_name,
                    ansi!("0")
                );
            }
            deduped
        }
        None => filtered_calls.to_vec(),
    };

    eprintln!("{}── Tool execution ──{}", ansi!("33"), ansi!("0"));

    let exec_result = execute_tools_concurrent(
        &tool_calls,
        tool_registry(),
        &ToolExecConfig {
            max_concurrency: MAX_CONCURRENT_TOOLS,
            circuit_breaker_limit: 0, // CLI handles failures inline
            operation_mode: "ask".to_string(),
            governance_required: false,
            is_safeguard: false,
            acp_session_id: None,
        },
        None, // no SSE progress in CLI
        "",
        0,
    )
    .await;

    let mut tool_results: Vec<String> = Vec::new();
    let mut has_failure = false;

    for item in &exec_result.tool_results {
        let tool_name = &item.tool_name;
        if item.success {
            // The executor returns formatted output; for CLI we need the raw
            // result text for terminal display. Re-extract from ToolOutput.
            let raw_text = item
                .output
                .result
                .as_ref()
                .and_then(|r| {
                    if let Some(s) = r.as_str() {
                        if !s.is_empty() {
                            return Some(s.to_string());
                        }
                    }
                    None
                })
                .unwrap_or_else(|| format!("{:?}", item.output));

            let display = if raw_text.len() > 500 {
                format!(
                    "{}...\n[{} chars truncated]  ({:.1}s)",
                    crate::shared::truncate::truncate_chars(&raw_text, 500, ""),
                    raw_text.len(),
                    item.duration_ms as f32 / 1000.0
                )
            } else {
                format!("{}  ({:.1}s)", raw_text, item.duration_ms as f32 / 1000.0)
            };
            eprintln!("    {}✓{} {}", ansi!("32"), ansi!("0"), display);

            let result_for_llm = if raw_text.len() > MAX_TOOL_RESULT_CHARS {
                tracing::warn!(
                    tool_name = %tool_name,
                    total_chars = raw_text.len(),
                    max_chars = MAX_TOOL_RESULT_CHARS,
                    "Tool result truncated for LLM"
                );
                format!(
                    "{}...\n[truncated: {} total chars, showing first {}]",
                    crate::shared::truncate::truncate_chars(&raw_text, MAX_TOOL_RESULT_CHARS, ""),
                    raw_text.len(),
                    MAX_TOOL_RESULT_CHARS
                )
            } else {
                raw_text.clone()
            };
            tool_results.push(build_tool_result_block(tool_name, &result_for_llm, false));
        } else {
            has_failure = true;
            let err_text = item
                .output
                .error
                .as_deref()
                .unwrap_or("tool execution failed");
            eprintln!(
                "    {}✗ Error: {}{}  ({:.1}s)",
                ansi!("31"),
                err_text,
                ansi!("0"),
                item.duration_ms as f32 / 1000.0
            );
            tool_results.push(build_tool_result_block(tool_name, err_text, true));
        }
    }

    followup_round_executed = true;
    (tool_results, has_failure, followup_round_executed)
}

/// Phase 3: Send tool results back to agent as a follow-up message,
/// stream the follow-up response with markdown rendering + Ctrl+C interrupt.
///
/// Capabilities (matching ACP's `run_followup_after_tool_observation`):
/// - Timeout wrapping via `run_with_optional_timeout` (default 60s)
/// - Tool result count limited to `MAX_TOOLS_IN_FOLLOWUP` (8)
/// - Skill dedup is already handled in Phase 2 (`run_tool_execution_phase`)
/// - Streaming rendering with Ctrl+C interrupt
async fn run_followup_phase(
    agent: &Arc<dyn Agent>,
    messages: &mut Vec<Message>,
    principles: &[String],
    tool_results: &[String],
    has_failure: bool,
    response: &str,
) -> String {
    // ── Limit tool results to prevent message bloat (mirrors ACP max_tools_per_round) ──
    let limited_results: Vec<&String> = tool_results.iter().take(MAX_TOOLS_IN_FOLLOWUP).collect();
    let results_for_message: Vec<String> = limited_results.iter().map(|s| (*s).clone()).collect();
    if tool_results.len() > MAX_TOOLS_IN_FOLLOWUP {
        warn!(
            "Tool results truncated for follow-up: {} total, showing {}",
            tool_results.len(),
            MAX_TOOLS_IN_FOLLOWUP
        );
        eprintln!(
            "  {}⚠  Tool results truncated: {} total, showing first {}{}",
            ansi!("33"),
            tool_results.len(),
            MAX_TOOLS_IN_FOLLOWUP,
            ansi!("0")
        );
    }

    messages.push(Message {
        role: "assistant".to_string(),
        content: response.to_string(),
    });
    messages.push(Message {
        role: "user".to_string(),
        content: build_tool_execution_followup_message(&results_for_message, has_failure),
    });

    eprint!("{}── Agent follow-up ──{}\n🤖 ", ansi!("33"), ansi!("0"));
    std::io::Write::flush(&mut std::io::stdout()).ok();

    // ── Set up streaming channel and timeout ──
    let followup_principles = if principles.is_empty() {
        None
    } else {
        Some(principles.to_vec())
    };
    let (followup_task, mut rx2) =
        spawn_agent_chat(Arc::clone(agent), messages.clone(), followup_principles);

    // ── Collect streaming tokens with timeout ──
    let timeout_duration = Duration::from_secs(DEFAULT_FOLLOWUP_TIMEOUT_SECS);
    let collect = async {
        let mut followup_renderer = StreamMarkdownRenderer::new();
        let completed = render_streaming_tokens(
            &mut rx2,
            &mut followup_renderer,
            None,
            ReasoningRenderStyle::ColorToggle,
        )
        .await;
        if !completed {
            eprintln!(
                "\n{}Interrupted follow-up response.{}  [P3]",
                ansi!("33"),
                ansi!("0")
            );
            followup_task.abort();
        }

        if let Err(e) = followup_task.await {
            warn!("Agent followup task failed: {e}");
        }

        let rendered_final = {
            let (remaining, _) = followup_renderer.flush();
            if !remaining.is_empty() {
                let n = remaining.lines().count();
                for _ in 0..n {
                    eprint!("\x1B[F\x1B[K");
                }
                eprintln!("{}", remaining);
                remaining
            } else {
                followup_renderer.take_raw_response()
            }
        };

        Ok::<String, anyhow::Error>(rendered_final)
    };

    let result = run_with_optional_timeout(Some(timeout_duration), collect, |duration| {
        anyhow::anyhow!(
            "Agent follow-up timed out after {}s",
            duration.as_secs().max(1)
        )
    })
    .await;

    match result {
        Ok(rendered_final) if !rendered_final.trim().is_empty() => {
            crate::acp::helpers::autonomy_metrics::record_tool_followup_success();
            rendered_final
        }
        Ok(_) => {
            crate::acp::helpers::autonomy_metrics::record_tool_followup_fallback();
            response.to_string()
        }
        Err(e) => {
            warn!("Agent follow-up failed or timed out: {e}");
            eprintln!("{}⚠  Follow-up: {}{}  [P3]", ansi!("33"), e, ansi!("0"));
            crate::acp::helpers::autonomy_metrics::record_tool_followup_fallback();
            response.to_string()
        }
    }
}
