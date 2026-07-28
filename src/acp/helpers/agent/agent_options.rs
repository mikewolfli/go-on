//! Agent options assembly and skill injection helpers for ACP chat
//!
//! This module extracts the agent-options assembly logic from
//! `process_chat_request` into a standalone function:
//!
//! 1. **Base options** — seeded from `phase.options.agent_options()`, then
//!    overlaid with per-request options (`params.options.extra`).
//! 2. **Runtime-config flags** — DAG execution, agent reroute, and
//!    metacognitive feedback flags injected from `RuntimeConfig`.
//! 3. **Skill injection** — registered skills are exposed as LLM-callable
//!    function-calling tools with sanitized names.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::acp::server::AcpServer;
use crate::orchestration::flow::ResolvedPhase;
use crate::orchestration::skill::SkillDescriptor;

use super::super::r#impl::chat::ChatParams;

/// Assembles the `base_agent_options` map used throughout the chat-request
/// lifecycle.
///
/// # Steps
///
/// 1. Start with `phase.options.agent_options()` (the `extra` map on the
///    resolved phase's options).
/// 2. Overlay per-request options from `params.options.extra`, flattening
///    any nested `"extra"` keys that legacy clients may produce.
/// 3. Inject runtime-config flags: `enable_dag_execution`,
///    `enable_agent_reroute`, `enable_metacognitive_feedback`.
/// 4. Inject registered skills as `tools` / `tool_choice` entries (with
///    sanitized function names) so the LLM can invoke them during chat.
pub(crate) fn assemble_agent_options(
    server: &AcpServer,
    phase: &ResolvedPhase,
    params: &ChatParams,
) -> HashMap<String, Value> {
    // ── Step 1 & 2: base options from phase + request ──────────────────
    let mut base_agent_options = phase
        .options
        .as_ref()
        .and_then(|opts| opts.agent_options())
        .unwrap_or_default();
    if let Some(request_options) = params.options.as_ref() {
        for (key, value) in &request_options.extra {
            if key == "extra" {
                // Defensive flatten: legacy clients may nest options under "extra" key
                if let Some(obj) = value.as_object() {
                    for (k, v) in obj {
                        base_agent_options.insert(k.clone(), v.clone());
                    }
                }
            } else {
                base_agent_options.insert(key.clone(), value.clone());
            }
        }
    }

    // ── Step 3: Runtime-config flags ──────────────────────────────────
    base_agent_options.insert(
        "enable_dag_execution".to_string(),
        json!(server.runtime_config.enable_dag_execution),
    );
    base_agent_options.insert(
        "enable_agent_reroute".to_string(),
        json!(server.runtime_config.enable_agent_reroute),
    );
    base_agent_options.insert(
        "enable_metacognitive_feedback".to_string(),
        json!(server.runtime_config.enable_metacognitive_feedback),
    );

    // ── Step 4: Inject registered skills as LLM-callable tools ────────
    // When skills are registered in the skill_registry, expose them as
    // function-calling tools to the LLM provider so the AI can invoke them
    // during chat conversations (P0 requirement).
    //
    // NOTE: DeepSeek and some other providers enforce a strict pattern on
    // function names: ^[a-zA-Z0-9_-]+$. We sanitize skill names to match.
    {
        let registry = server
            .orchestration_deps
            .skill_registry
            .read()
            .unwrap_or_else(|poisoned| {
                tracing::warn!("skill_registry lock poisoned during tool injection – recovered");
                poisoned.into_inner()
            });
        let sanitize_fn_name = |name: &str| -> String {
            name.chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect::<String>()
        };
        let skill_tools: Vec<Value> = registry
            .list(false)
            .iter()
            .map(|skill: &SkillDescriptor| {
                let safe_name = sanitize_fn_name(&skill.name);
                let fallback_name = if safe_name.is_empty() {
                    format!("skill-{}", skill.name.len())
                } else {
                    safe_name
                };
                json!({
                    "type": "function",
                    "function": {
                        "name": fallback_name,
                        "description": skill.description,
                        "parameters": skill.input_schema,
                    }
                })
            })
            .collect();
        // Merge built-in tools (http_request, read_file, shell_exec, etc.)
        // from the global ToolRegistry into the same tools array so the LLM
        // can call them directly via OpenAI function-calling, matching the
        // behavior of CLI chat ("/go-on -a").
        let builtin_tools: Vec<Value> = {
            let registry = crate::acp::r#impl::request::tools_pack::global_tool_registry();
            let mut tools = Vec::with_capacity(registry.names().len());
            for name in registry.names() {
                let desc = crate::shared::tool_descriptors::tool_descriptor(name);
                tools.push(json!({
                    "type": "function",
                    "function": {
                        "name": name,
                        "description": desc.description,
                        "parameters": desc.input_schema,
                    }
                }));
            }
            tools
        };
        let all_tools: Vec<Value> = skill_tools.into_iter().chain(builtin_tools).collect();
        if !all_tools.is_empty() {
            base_agent_options.insert("tools".to_string(), json!(all_tools));
            base_agent_options.insert("tool_choice".to_string(), json!("auto"));
        }
    }

    base_agent_options
}
