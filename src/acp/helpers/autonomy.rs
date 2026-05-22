use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::time::Duration;

use crate::acp::helpers::context::run_with_optional_timeout;
use crate::agent::{Agent, Message};
use crate::orchestration::planner_executor::Planner;

fn contains_any(text: &str, terms: &[&str]) -> bool {
    terms.iter().any(|term| text.contains(term))
}

pub(crate) fn planner_guided_tool_preferences(
    task_id: &str,
    phase: &str,
    role: &str,
    objective: &str,
    response_hint: &str,
    max_tools: usize,
) -> Vec<String> {
    fn push_unique_tool(tools: &mut Vec<String>, name: &str) {
        if !tools.iter().any(|tool| tool == name) {
            tools.push(name.to_string());
        }
    }

    let envelope = crate::agent::AgentTaskEnvelope {
        task_id: task_id.to_string(),
        phase: phase.to_string(),
        role: role.to_string(),
        objective: objective.to_string(),
        constraints: None,
        evidence: None,
        input: serde_json::json!({
            "objective": objective,
            "response_hint": response_hint,
        }),
    };

    let plan = Planner::plan(&envelope);
    let joined_text = format!(
        "{}\n{}\n{}",
        objective,
        response_hint,
        plan.steps
            .iter()
            .map(|step| step.description.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    )
    .to_ascii_lowercase();

    let mut tools: Vec<String> = Vec::new();

    let discovery_signal = contains_any(
        &joined_text,
        &["search", "find", "locate", "trace", "inspect", "analyze"],
    ) && contains_any(
        &joined_text,
        &[
            "file",
            "code",
            "workspace",
            "repo",
            "module",
            "bug",
            "error",
        ],
    );
    let review_signal = contains_any(
        &joined_text,
        &["read", "check", "trace", "review", "diagnose", "inspect"],
    ) && contains_any(
        &joined_text,
        &["file", "code", "workspace", "repo", "module", "error"],
    );
    let mutation_signal = contains_any(
        &joined_text,
        &[
            "write",
            "create",
            "update",
            "modify",
            "refactor",
            "fix",
            "implement",
        ],
    ) && contains_any(
        &joined_text,
        &["file", "code", "module", "patch", "change", "refactor"],
    );
    let execution_signal = contains_any(
        &joined_text,
        &["run", "build", "test", "verify", "compile", "benchmark"],
    ) && contains_any(
        &joined_text,
        &[
            "cargo",
            "test",
            "build",
            "compile",
            "benchmark",
            "workspace",
        ],
    );

    if discovery_signal {
        push_unique_tool(&mut tools, "search_files");
    }
    if review_signal {
        push_unique_tool(&mut tools, "read_file");
    }
    if mutation_signal {
        push_unique_tool(&mut tools, "write_file");
    }
    if execution_signal {
        push_unique_tool(&mut tools, "bash");
    }

    tools.truncate(max_tools.max(1));
    tools
}

pub(crate) fn is_execution_like_request(mode: &str, messages: &[Message]) -> bool {
    let mode_lower = mode.trim().to_ascii_lowercase();
    if matches!(
        mode_lower.as_str(),
        "agent" | "edit" | "full_auto" | "workflow" | "execute"
    ) {
        return true;
    }

    const EXECUTION_HINTS: &[&str] = &[
        "fix",
        "modify",
        "update",
        "edit",
        "refactor",
        "implement",
        "create file",
        "run tests",
        "build",
        "compile",
        "verify",
        "apply patch",
        "execute",
        "workflow.execute",
        "task.execute",
        "workflow.generate",
    ];

    messages.iter().any(|message| {
        if message.role != "user" {
            return false;
        }
        let text = message.content.to_ascii_lowercase();
        EXECUTION_HINTS.iter().any(|hint| text.contains(hint))
    })
}

pub(crate) async fn run_followup_after_tool_observation(
    agent: Arc<dyn Agent>,
    messages: Vec<Message>,
    principles: Option<Vec<String>>,
    options: Option<HashMap<String, Value>>,
    timeout_duration: Option<Duration>,
) -> Result<(String, String, Option<String>)> {
    let (sender, mut receiver) = mpsc::channel::<String>(1024);
    let sender = crate::agent::StreamingSender::from(sender);
    let task = tokio::spawn(async move { agent.chat(messages, principles, options, sender).await });

    let collect = async move {
        let mut response = String::new();
        let mut reasoning = String::new();
        let mut selected_model: Option<String> = None;

        while let Some(token) = receiver.recv().await {
            if let Some(model_id) = token.strip_prefix("__model_used__:") {
                selected_model = Some(model_id.trim().to_string());
                continue;
            }

            if token.starts_with("__tool_call__:") {
                // Follow-up round only consumes observations and finalizes output.
                continue;
            }

            if let Some(reasoning_token) = token.strip_prefix("__thinking__") {
                reasoning.push_str(reasoning_token);
            } else {
                response.push_str(&token);
            }
        }

        match task.await {
            Ok(Ok(())) => Ok::<(String, String, Option<String>), anyhow::Error>((
                response,
                reasoning,
                selected_model,
            )),
            Ok(Err(err)) => Err(err.into()),
            Err(join_err) => Err(anyhow::anyhow!("agent follow-up task panicked: {join_err}")),
        }
    };

    run_with_optional_timeout(timeout_duration, collect, |duration| {
        anyhow::anyhow!(
            "agent follow-up timed out after {}s",
            duration.as_secs().max(1)
        )
    })
    .await
}
