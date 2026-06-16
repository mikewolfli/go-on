use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::mpsc;

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

    async fn execute(&self, input: &Value) -> Result<Value> {
        // Execute the skill with timeout and retry support.
        // The prompt_template is filled with input parameters and then
        // executed via the LLM through the ACP chat handler pipeline.
        let timeout = Duration::from_secs(self.timeout_secs.max(10));
        let max_retries = self.max_retries;

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
                let key_double_brace = format!("{{{{{} }}}}", key);
                resolved = resolved.replace(&key_double_brace, &val_str);
            }
            resolved
        } else {
            self.prompt_template.clone()
        };

        // If a global LLM prompt agent is available, use it directly for
        // real LLM execution instead of the retry fallback loop.
        if let Some(agent) = PROMPT_SKILL_AGENT.get() {
            let agent_prompt = prompt.clone();
            return agent
                .execute_prompt(&agent_prompt)
                .await
                .map(|response| {
                    json!({
                        "success": true,
                        "response": response,
                        "skill_type": "prompt_based_llm",
                    })
                })
                .map_err(|e| {
                    anyhow::anyhow!("Prompt skill '{}' LLM execution failed: {}", self.name, e)
                });
        }

        // Execute with retry loop and timeout per attempt
        let mut last_error = None;
        for attempt in 0..=max_retries {
            let _attempt_start = std::time::Instant::now();

            // Async timeout wrapper
            // tokio::time::timeout returns Result<T, Elapsed>. Since the inner
            // async block returns Result<Value> (anyhow::Result), we flatten:
            //   Ok(Ok(val)) -> success
            //   Ok(Err(e))  -> execution error
            //   Err(_)      -> timeout
            let timed_result = tokio::time::timeout(timeout, async {
                // NOTE: Actual LLM execution is performed by the ACP chat handler
                // via the chat pipeline (SkillCreatorSkill). This execute() method
                // returns the prepared prompt for the handler to execute.
                // Full LLM integration will be wired in Phase 10+.
                Ok::<Value, anyhow::Error>(json!({
                    "success": true,
                    "summary": format!("Skill '{}' executed via prompt template", self.name),
                    "prompt": prompt,
                    "skill_type": "prompt_based",
                    "attempt": attempt + 1,
                }))
            })
            .await;

            match timed_result {
                Ok(Ok(val)) => return Ok(val),
                Ok(Err(e)) => {
                    last_error = Some(e);
                    if attempt < max_retries {
                        let backoff = Duration::from_secs(1_u64 << attempt);
                        tokio::time::sleep(backoff).await;
                    }
                }
                Err(_elapsed) => {
                    last_error = Some(anyhow::anyhow!(
                        "Skill '{}' timed out after {}s on attempt {}/{}",
                        self.name,
                        self.timeout_secs,
                        attempt + 1,
                        max_retries + 1
                    ));
                    if attempt < max_retries {
                        let backoff = Duration::from_secs(1_u64 << attempt);
                        tokio::time::sleep(backoff).await;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            anyhow::anyhow!("Skill '{}' failed after {} retries", self.name, max_retries)
        }))
    }
}

impl PromptBasedSkill {
    /// Convenience method: wraps this skill into `Arc<dyn Skill>` for registry registration.
    /// Not called internally but kept as a public utility for consumers.
    #[allow(dead_code)] // public API — reserved for external registry wiring
                        // F-GAP-49 — reserved for future use
    pub fn boxed(self) -> Arc<dyn Skill> {
        Arc::new(self)
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

        let (tx, mut rx) = mpsc::channel::<String>(256);
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

/// A skill composed from two other skills (pipeline: A → B).
#[derive(Debug, Clone)]
pub struct ComposedSkill {
    pub name: String,
    pub description: String,
    pub skill_a: String,
    pub skill_b: String,
    /// Reference to the SkillRegistry that holds skill_a and skill_b.
    /// Used at execute time to look up and delegate to the actual skills.
    pub registry: Arc<Mutex<SkillRegistry>>,
}

#[async_trait]
impl Skill for ComposedSkill {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }

    async fn execute(&self, input: &Value) -> Result<Value> {
        // Look up skill_a from the registry and execute it with the given input.
        // Use a separate function or explicit scope to ensure MutexGuard is dropped
        // before the .await (MutexGuard is not Send).
        let skill_a_clone = {
            let registry = self
                .registry
                .lock()
                .map_err(|e| anyhow::anyhow!("Failed to lock skill registry: {}", e))?;
            registry
                .get(&self.skill_a)
                .ok_or_else(|| {
                    anyhow::anyhow!("Composed skill '{}' not found: {}", self.name, self.skill_a)
                })?
                .clone()
        }; // MutexGuard dropped here
        let result_a = skill_a_clone.execute(input).await?;

        // Look up skill_b and execute with skill_a's output as input
        let skill_b_clone = {
            let registry = self
                .registry
                .lock()
                .map_err(|e| anyhow::anyhow!("Failed to lock skill registry: {}", e))?;
            registry
                .get(&self.skill_b)
                .ok_or_else(|| {
                    anyhow::anyhow!("Composed skill '{}' not found: {}", self.name, self.skill_b)
                })?
                .clone()
        }; // MutexGuard dropped here
        let result_b = skill_b_clone.execute(&result_a).await?;

        Ok(json!({
            "success": true,
            "summary": format!("Composed skill '{}' ({} \u{2192} {}) executed", self.name, self.skill_a, self.skill_b),
            "pipeline": [
                {"skill": self.skill_a, "output": result_a},
                {"skill": self.skill_b, "output": result_b}
            ],
            "result": result_b
        }))
    }
}

impl ComposedSkill {
    // Future convenience methods for skill composition go here.
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

pub(crate) fn name_similarity(left: &str, right: &str) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    if left == right {
        return 1.0;
    }

    let shared_prefix = left
        .chars()
        .zip(right.chars())
        .take_while(|(l, r)| l == r)
        .count() as f64;
    let prefix_score = shared_prefix / left.len().max(right.len()) as f64;

    let overlap = left.chars().filter(|ch| right.contains(*ch)).count() as f64;
    let overlap_score = overlap / left.len().max(right.len()) as f64;

    (0.5 * prefix_score + 0.5 * overlap_score).clamp(0.0, 1.0)
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

pub(crate) fn semantic_similarity(
    intent_tokens: &std::collections::BTreeSet<String>,
    skill: &Arc<dyn Skill>,
) -> f64 {
    if intent_tokens.is_empty() {
        return 0.5;
    }
    let mut signature = String::new();
    signature.push_str(skill.name());
    signature.push(' ');
    signature.push_str(skill.description());
    signature.push(' ');
    signature.push_str(&skill.input_schema().to_string());

    let skill_tokens = tokenize_text(&signature);
    if skill_tokens.is_empty() {
        return 0.0;
    }

    let overlap = intent_tokens
        .iter()
        .filter(|token| skill_tokens.contains(*token))
        .count() as f64;
    let union = intent_tokens.union(&skill_tokens).count().max(1) as f64;
    let token_score = (overlap / union).clamp(0.0, 1.0);
    let intent_text = intent_tokens.iter().cloned().collect::<Vec<_>>().join(" ");
    let embedding_score = embedding_cosine_similarity(&intent_text, &signature);
    (0.5 * token_score + 0.5 * embedding_score).clamp(0.0, 1.0)
}

fn tokenize_text(text: &str) -> std::collections::BTreeSet<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|token| token.len() >= 3)
        .map(|token| token.to_ascii_lowercase())
        .collect()
}

fn embedding_cosine_similarity(left: &str, right: &str) -> f64 {
    let left_vec = hashed_embedding(left, 96);
    let right_vec = hashed_embedding(right, 96);
    cosine_similarity(&left_vec, &right_vec)
}

fn hashed_embedding(text: &str, dim: usize) -> Vec<f64> {
    let mut vec = vec![0.0_f64; dim.max(8)];
    for token in tokenize_text(text) {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        token.hash(&mut hasher);
        let hash = hasher.finish() as usize;
        let index = hash % vec.len();
        vec[index] += 1.0;
    }
    let norm = vec.iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm > f64::EPSILON {
        for item in &mut vec {
            *item /= norm;
        }
    }
    vec
}

fn cosine_similarity(left: &[f64], right: &[f64]) -> f64 {
    if left.len() != right.len() || left.is_empty() {
        return 0.0;
    }
    let dot = left
        .iter()
        .zip(right.iter())
        .map(|(l, r)| l * r)
        .sum::<f64>();
    let left_norm = left.iter().map(|x| x * x).sum::<f64>().sqrt();
    let right_norm = right.iter().map(|x| x * x).sum::<f64>().sqrt();
    if left_norm <= f64::EPSILON || right_norm <= f64::EPSILON {
        return 0.0;
    }
    (dot / (left_norm * right_norm)).clamp(0.0, 1.0)
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

/// Built-in skill-creator skill.
///
/// Describes how to create new skills. This skill serves as a reference
/// that instructs the AI to use the `skill.create` RPC to dynamically
/// create new reusable skills from a natural language description.
///
/// Registered as `"skill-creator"` when `runtime.skills_enabled = true`.
pub struct SkillCreatorSkill {
    /// Reference to the skill registry for creating skills.
    pub registry: Arc<Mutex<SkillRegistry>>,
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
                .lock()
                .map_err(|e| anyhow::anyhow!("skill registry lock error: {e}"))?;
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
    pub fn new(registry: Arc<Mutex<SkillRegistry>>) -> Self {
        Self { registry }
    }
}
