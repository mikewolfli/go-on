use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock, RwLock};

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tracing::warn;

use crate::agent::{Agent, Message, StreamingSender};

use super::registry::SkillRegistry;

/// Trait for providing LLM-based prompt execution to PromptBasedSkill.
#[async_trait]
pub trait PromptSkillAgent: Send + Sync {
    /// Execute a prompt and return the LLM response as a string.
    async fn execute_prompt(&self, prompt: &str) -> Result<String>;
}

/// Global LLM agent provider for PromptBasedSkill execution.
/// Set during server startup to enable real LLM-based skill execution.
static PROMPT_SKILL_AGENT: OnceLock<Arc<dyn PromptSkillAgent>> = OnceLock::new();

/// Policy for controlling how a skill is invoked and exposed.
/// Mirrors codex's `SkillPolicy` concept.
#[derive(Debug, Clone, Default)]
pub struct SkillPolicy {
    /// If false (default), the skill can be implicitly invoked when the
    /// user's intent matches its description. Set to true to require
    /// explicit invocation only.
    pub allow_implicit_invocation: Option<bool>,
    /// Optional product restriction — empty means available to all products.
    pub products: Vec<String>,
}

/// Set the global prompt skill agent for LLM execution.
/// Must be called before any PromptBasedSkill.execute() invocations that
/// require real LLM execution.
pub fn set_prompt_skill_agent(agent: Arc<dyn PromptSkillAgent>) {
    let _ = PROMPT_SKILL_AGENT.set(agent);
}

#[async_trait]
pub trait Skill: Send + Sync {
    fn name(&self) -> &str;

    fn description(&self) -> &str {
        "Registered MCP skill"
    }

    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }

    /// Whether this skill should be hidden from model-facing discovery
    /// (e.g., the `skill_list` tool and semantic skill index).
    /// Hidden skills are still invocable via explicit name lookup.
    fn disable_model_invocation(&self) -> bool {
        false
    }

    /// Optional policy controlling how this skill can be invoked.
    /// Mirrors codex's `SkillPolicy` — enables implicit invocation detection
    /// and product-based gating.
    fn policy(&self) -> Option<&SkillPolicy> {
        None
    }

    async fn execute(&self, input: &Value) -> Result<Value>;
}

pub struct PromptBasedSkill {
    pub name: String,
    pub description: String,
    pub prompt_template: String,
    pub input_schema: HashMap<String, String>,
    /// Maximum execution time in seconds before the skill times out.
    /// Default: 120 seconds.
    pub timeout_secs: u64,
    /// Maximum number of retries on transient failure.
    /// Default: 2 retries.
    pub max_retries: u32,
    /// When true, this skill is excluded from model-facing listings
    /// (skill_list tool, semantic index) but remains invocable via
    /// explicit name lookup or `/` command.
    pub disable_model_invocation: bool,
    /// Optional policy for controlling implicit invocation and product gating.
    pub policy: Option<SkillPolicy>,
}

#[async_trait]
impl Skill for PromptBasedSkill {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> Value {
        let mut properties = serde_json::Map::new();
        for (k, v) in &self.input_schema {
            properties.insert(k.clone(), json!({"type": v}));
        }
        json!({
            "type": "object",
            "properties": properties,
        })
    }

    fn disable_model_invocation(&self) -> bool {
        self.disable_model_invocation
    }

    fn policy(&self) -> Option<&SkillPolicy> {
        self.policy.as_ref()
    }

    async fn execute(&self, input: &Value) -> Result<Value> {
        // Execute the prompt-based skill via the configured LLM agent.
        // The prompt_template is filled with input parameters and then
        // executed via the global PROMPT_SKILL_AGENT.

        // Build the prompt by substituting input parameters into the template
        let prompt = if let Some(obj) = input.as_object() {
            let mut resolved = self.prompt_template.clone();
            for (key, value) in obj {
                let key_brace = format!("{{{}}}", key);
                let val_str = match value {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                resolved = resolved.replace(&key_brace, &val_str);
                // Also support {{key}} format for template engines
                let key_double_brace = format!("{{{{{}}}}}", key);
                resolved = resolved.replace(&key_double_brace, &val_str);
            }
            resolved
        } else {
            self.prompt_template.clone()
        };

        // If a global LLM prompt agent is available, use it with
        // configurable timeout and retry logic.
        if let Some(agent) = PROMPT_SKILL_AGENT.get() {
            let agent_prompt = prompt.clone();
            let timeout_duration = std::time::Duration::from_secs(self.timeout_secs);
            let max_attempts = (self.max_retries + 1) as usize;
            // Cumulative deadline: total wall-clock timeout for all retries combined.
            // Worst case: timeout_secs * (max_retries + 1) + total backoff.
            // Using 2x as a safety margin to cover backoff and retry overhead.
            let overall_deadline = std::time::Duration::from_secs(self.timeout_secs * 2)
                .max(timeout_duration.saturating_mul(max_attempts as u32));
            let deadline = tokio::time::Instant::now() + overall_deadline;
            let mut last_error: Option<anyhow::Error> = None;

            for attempt in 1..=max_attempts {
                // Check cumulative deadline before each attempt
                if tokio::time::Instant::now() >= deadline {
                    return Err(last_error.unwrap_or_else(|| {
                        anyhow::anyhow!(
                        "Prompt skill '{}' overall deadline of {:?} exceeded after {} attempt(s)",
                        self.name, overall_deadline, attempt - 1
                    )
                    }));
                }
                match tokio::time::timeout(timeout_duration, agent.execute_prompt(&agent_prompt))
                    .await
                {
                    Ok(Ok(response)) => {
                        return Ok(json!({
                            "success": true,
                            "response": response,
                            "skill_type": "prompt_based_llm",
                            "attempts": attempt,
                        }));
                    }
                    Ok(Err(e)) => {
                        let err_str = e.to_string();
                        // Detect rate limiting (HTTP 429, "rate limit", "too many requests")
                        // via the shared canonical detector.
                        let is_rate_limit = crate::agents::agent::is_rate_limit_error(&err_str);

                        if is_rate_limit {
                            // Exponential backoff for rate limits: 1s, 2s, 4s, ...
                            // using the shared canonical schedule (capped at 10s,
                            // matching the declared max_delay_ms: 10_000 contract).
                            let backoff_secs = crate::agents::agent::retry_backoff_secs(
                                attempt.saturating_sub(1) as u32,
                            );
                            let backoff = std::time::Duration::from_secs(backoff_secs);
                            warn!(
                                "Prompt skill '{}' rate limited on attempt {}/{}, \
                                 backing off {}s: {}",
                                self.name, attempt, max_attempts, backoff_secs, err_str
                            );
                            tokio::time::sleep(backoff).await;
                            last_error = Some(anyhow::anyhow!(
                                "Prompt skill '{}' rate limited after {} attempt(s): {}",
                                self.name,
                                attempt,
                                err_str
                            ));
                        } else {
                            last_error = Some(anyhow::anyhow!(
                                "Prompt skill '{}' LLM execution failed: {}",
                                self.name,
                                err_str
                            ));
                            if attempt < max_attempts {
                                // Same canonical exponential schedule as the
                                // rate-limit branch above (1s, 2s, 4s, … capped
                                // at 10s).
                                let backoff_secs = crate::agents::agent::retry_backoff_secs(
                                    attempt.saturating_sub(1) as u32,
                                );
                                let backoff = std::time::Duration::from_secs(backoff_secs);
                                warn!(
                                    "Prompt skill '{}' attempt {}/{} failed, retrying in {}s: {}",
                                    self.name, attempt, max_attempts, backoff_secs, err_str
                                );
                                tokio::time::sleep(backoff).await;
                            }
                        }
                    }
                    Err(_elapsed) => {
                        last_error = Some(anyhow::anyhow!(
                            "Prompt skill '{}' timed out after {}s (timeout_secs={}). \
                             Consider increasing timeout_secs or reducing prompt complexity.",
                            self.name,
                            self.timeout_secs,
                            self.timeout_secs
                        ));
                        if attempt < max_attempts {
                            warn!(
                                "Prompt skill '{}' attempt {}/{} timed out after {}s, retrying",
                                self.name, attempt, max_attempts, self.timeout_secs
                            );
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        }
                    }
                }
            }

            return Err(last_error.unwrap_or_else(|| {
                anyhow::anyhow!(
                    "Prompt skill '{}' execution failed after {} attempts",
                    self.name,
                    max_attempts
                )
            }));
        }

        // No global LLM agent configured — return a clear actionable error.
        // Prompt-based skills cannot execute without an LLM provider (API key).
        // Check for common causes:
        //   - OPENAI_API_KEY / ANTHROPIC_API_KEY not set in environment
        //   - set_prompt_skill_agent() not called during server startup
        //   - LLM provider configuration missing from config file
        anyhow::bail!(
            "Prompt skill '{}' cannot execute: no LLM agent configured. \
             This usually means one of the following:\n\
             1. No API key is set — check that OPENAI_API_KEY, ANTHROPIC_API_KEY, \
                or your provider's key is set in the environment or config.\n\
             2. set_prompt_skill_agent() was not called during server startup — \
                ensure the server wired a PromptSkillAgent.\n\
             3. The LLM provider configuration is missing from the config file.\n\
             Prompt template (first 120 chars): {}..",
            self.name,
            &prompt.chars().take(120).collect::<String>()
        )
    }
}

/// A `PromptSkillAgent` that delegates prompt execution to an `Agent` in the
/// registry. This bridges the skill system with the configured LLM provider
/// to enable real LLM-based skill execution.
pub struct ChatBasedSkillAgent {
    agent: Arc<dyn Agent>,
}

impl ChatBasedSkillAgent {
    /// Create a new skill agent wrapping the given provider agent.
    pub fn new(agent: Arc<dyn Agent>) -> Self {
        Self { agent }
    }
}

#[async_trait]
impl PromptSkillAgent for ChatBasedSkillAgent {
    async fn execute_prompt(&self, prompt: &str) -> Result<String> {
        let messages = vec![Message {
            role: "user".to_string(),
            content: prompt.to_string(),
        }];

        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let sender = StreamingSender::new(tx);

        self.agent
            .chat(messages, None, None, sender)
            .await
            .map_err(|e| anyhow::anyhow!("LLM agent chat failed: {}", e))?;

        let mut response = String::new();
        while let Some(token) = rx.recv().await {
            response.push_str(&token);
        }

        if response.is_empty() {
            anyhow::bail!("LLM agent returned empty response");
        }

        Ok(response)
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

pub(crate) fn normalize_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>()
}

pub(crate) fn extract_intent_tokens(input: &Value) -> std::collections::BTreeSet<String> {
    let mut chunks = Vec::new();
    if let Some(object) = input.as_object() {
        for key in ["objective", "task", "query", "prompt", "content", "input"] {
            if let Some(value) = object.get(key) {
                if let Some(text) = value.as_str() {
                    chunks.push(text.to_string());
                }
            }
        }
    }
    tokenize_text(&chunks.join(" "))
}

/// Shared bag-of-words tokenizer for the skill module tree.
///
/// Splits `text` on non-alphanumeric ASCII characters, keeps tokens whose
/// raw length is at least `min_len` and which are not present in `stopwords`
/// (an empty set disables stop-word filtering), then lowercases the
/// survivors. Both `registry::tokenize` (with the common stop-word list) and
/// `tokenize_text` (without one) delegate here so the two scoring paths share
/// a single tokenization rule (B3).
pub(crate) fn tokenize_with_stopwords(
    text: &str,
    min_len: usize,
    stopwords: &HashSet<&str>,
) -> Vec<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|token| token.len() >= min_len && !stopwords.contains(token))
        .map(|token| token.to_ascii_lowercase())
        .collect()
}

fn tokenize_text(text: &str) -> std::collections::BTreeSet<String> {
    tokenize_with_stopwords(text, 3, &HashSet::new())
        .into_iter()
        .collect()
}

/// Built-in echo skill.
///
/// Returns the input value unchanged. Useful for smoke-testing the skill
/// pipeline and as a reference implementation.
///
/// Registered as `"builtin.echo"` when `runtime.skills_enabled = true`.
pub struct EchoSkill;

#[async_trait]
impl Skill for EchoSkill {
    fn name(&self) -> &str {
        "builtin.echo"
    }

    fn description(&self) -> &str {
        "Returns the input value unchanged (builtin smoke-test skill)"
    }

    async fn execute(&self, input: &Value) -> Result<Value> {
        Ok(input.clone())
    }
}

/// Legacy alias of [`EchoSkill`], registered under the name advertised in
/// `tools.list` (`"echo_skill"`) so that the advertised name is
/// deterministically executable — previously it was only reachable via
/// fuzzy skill matching, which made the advertised-but-unregistered name
/// non-deterministic.
pub struct EchoSkillAlias;

#[async_trait]
impl Skill for EchoSkillAlias {
    fn name(&self) -> &str {
        "echo_skill"
    }

    fn description(&self) -> &str {
        "Echo back structured input for skill pipeline diagnostics."
    }

    async fn execute(&self, input: &Value) -> Result<Value> {
        Ok(input.clone())
    }
}

/// Built-in skill-creator skill.
///
/// Describes how to create new skills. This skill serves as a reference
/// that instructs the AI to use the `skill.create` RPC to dynamically
/// create new reusable skills from a natural language description.
///
/// Registered as `"skill-creator"` when `runtime.skills_enabled = true`.
pub struct SkillCreatorSkill {
    /// Reference to the skill registry for creating skills.
    pub registry: Arc<RwLock<SkillRegistry>>,
}

#[async_trait]
impl Skill for SkillCreatorSkill {
    fn name(&self) -> &str {
        "skill-creator"
    }

    fn description(&self) -> &str {
        "Creates a new reusable skill from a natural language description"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "description": {"type": "string"},
                "prompt_template": {"type": "string"},
                "input_schema": {"type": "object"}
            },
            "required": ["name", "description", "prompt_template"]
        })
    }

    async fn execute(&self, input: &Value) -> Result<Value> {
        let name = input.get("name").and_then(Value::as_str).unwrap_or("");
        let description = input
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("");
        let prompt_template = input
            .get("prompt_template")
            .and_then(Value::as_str)
            .unwrap_or("");
        let input_schema: HashMap<String, String> = input
            .get("input_schema")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        if name.is_empty() || description.is_empty() || prompt_template.is_empty() {
            anyhow::bail!("Missing required fields: name, description, prompt_template");
        }

        {
            let mut registry = self
                .registry
                .write()
                .map_err(|e| anyhow::anyhow!("skill registry write-lock error: {e}"))?;
            registry.create_skill_from_prompt(name, description, prompt_template, input_schema)?;
        }
        // Lock is dropped before Ok()

        Ok(json!({
            "success": true,
            "summary": format!("Skill '{}' created successfully", name),
            "name": name,
            "description": description,
        }))
    }
}

impl SkillCreatorSkill {
    /// Create a new SkillCreatorSkill with a reference to the skill registry.
    pub fn new(registry: Arc<RwLock<SkillRegistry>>) -> Self {
        Self { registry }
    }
}
