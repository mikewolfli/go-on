use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::i18n::runtime::tf;

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
#[cfg_attr(not(test), allow(dead_code))] // public API — reserved for LLM agent wiring
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

/// Records a version change in a skill's evolution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillVersionRecord {
    pub skill_name: String,
    pub version: u32,
    pub change_description: String,
    pub score_at_version: f64,
    pub timestamp_ms: u64,
}

/// A serializable snapshot of a prompt-based skill for disk persistence.
///
/// This record is written to a JSON file whenever a prompt-based skill
/// is created or removed, so that skills survive a backend restart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedPromptSkill {
    pub name: String,
    pub description: String,
    pub prompt_template: String,
    pub input_schema: HashMap<String, String>,
    pub created_at: i64,
}

#[derive(Default)]
pub struct SkillRegistry {
    skills: HashMap<String, Arc<dyn Skill>>,
    stats: HashMap<String, SkillRuntimeStats>,
    /// Skill evolution history keyed by skill name
    pub evolution_history: HashMap<String, Vec<SkillVersionRecord>>,
    /// Optional path to persist prompt-based skills to disk.
    /// When set, skills created via `create_skill_from_prompt` are saved
    /// automatically and reloaded at startup.
    persistence_path: Option<PathBuf>,
    /// Original data for prompt-based skills, keyed by name.
    /// Used to serialize skills back to disk without downcasting.
    prompt_skill_data: HashMap<String, SavedPromptSkill>,
}

impl std::fmt::Debug for SkillRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkillRegistry")
            .field("skills", &self.skills.keys().collect::<Vec<_>>())
            .field("stats", &self.stats)
            .field("evolution_history", &self.evolution_history)
            .field("persistence_path", &self.persistence_path)
            .field("prompt_skill_count", &self.prompt_skill_data.len())
            .finish()
    }
}

pub struct SkillDescriptor {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub score: f64,
    pub total_calls: u64,
    pub success_calls: u64,
    pub failure_calls: u64,
    pub average_latency_ms: f64,
}

#[derive(Clone, Debug, Default)]
struct SkillRuntimeStats {
    total_calls: u64,
    success_calls: u64,
    failure_calls: u64,
    total_latency_ms: u64,
}

impl SkillRuntimeStats {
    fn record(&mut self, success: bool, latency: Duration) {
        self.total_calls += 1;
        if success {
            self.success_calls += 1;
        } else {
            self.failure_calls += 1;
        }
        self.total_latency_ms = self
            .total_latency_ms
            .saturating_add(latency.as_millis() as u64);
    }

    fn average_latency_ms(&self) -> f64 {
        if self.total_calls == 0 {
            0.0
        } else {
            self.total_latency_ms as f64 / self.total_calls as f64
        }
    }

    fn score(&self) -> f64 {
        if self.total_calls == 0 {
            return 0.5;
        }
        let success_rate = self.success_calls as f64 / self.total_calls as f64;
        let latency_penalty = (self.average_latency_ms() / 4000.0).clamp(0.0, 0.4);
        (success_rate - latency_penalty).clamp(0.0, 1.0)
    }
}

impl SkillRegistry {
    /// Register a skill, returning an error if the name is invalid or already registered.
    ///
    /// Name rules:
    /// - 1–64 characters
    /// - Only ASCII: lowercase letters, digits, `.`, `_`, `-`
    /// - Must be unique (duplicates are rejected)
    pub fn register(&mut self, skill: Arc<dyn Skill>) -> Result<()> {
        let name = skill.name().to_string();
        if name.is_empty() || name.len() > 64 {
            anyhow::bail!(
                "{}",
                tf(
                    "error.skill_name_length",
                    &[("name", name.as_str()), ("len", &name.len().to_string())]
                )
            );
        }
        if !name.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-'
        }) {
            anyhow::bail!(
                "{}",
                tf(
                    "error.skill_name_invalid_chars",
                    &[("name", name.as_str()), ("chars", "invalid characters")]
                )
            );
        }
        if self.skills.contains_key(&name) {
            anyhow::bail!(
                "{}",
                tf("error.skill_already_registered", &[("name", name.as_str())])
            );
        }
        match skill.input_schema() {
            serde_json::Value::Object(_) => {}
            other => anyhow::bail!(
                "{}",
                tf(
                    "error.skill_name_invalid_chars",
                    &[
                        ("name", &name),
                        (
                            "chars",
                            &format!("input_schema must be a JSON object, got: {}", other)
                        )
                    ]
                )
            ),
        }
        self.skills.insert(name.clone(), skill);
        self.stats.entry(name).or_default();
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Skill>> {
        self.skills.get(name).cloned()
    }

    pub fn unregister(&mut self, name: &str) -> bool {
        let removed = self.skills.remove(name).is_some();
        if removed {
            self.stats.remove(name);
            self.evolution_history.remove(name); // Clean up history too
            self.prompt_skill_data.remove(name);
        }
        removed
    }

    pub fn list(&self) -> Vec<SkillDescriptor> {
        let mut items = self
            .skills
            .iter()
            .map(|(name, skill)| {
                let stats = self.stats.get(name).cloned().unwrap_or_default();
                SkillDescriptor {
                    name: skill.name().to_string(),
                    description: skill.description().to_string(),
                    input_schema: skill.input_schema(),
                    score: stats.score(),
                    total_calls: stats.total_calls,
                    success_calls: stats.success_calls,
                    failure_calls: stats.failure_calls,
                    average_latency_ms: stats.average_latency_ms(),
                }
            })
            .collect::<Vec<_>>();
        items.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.name.cmp(&b.name))
        });
        items
    }

    pub fn record_outcome(&mut self, name: &str, success: bool, latency: Duration) {
        let stats = self.stats.entry(name.to_string()).or_default();
        stats.record(success, latency);
    }

    pub fn score_of(&self, name: &str) -> Option<f64> {
        self.stats.get(name).map(SkillRuntimeStats::score)
    }

    pub fn best_match(&self, requested: &str) -> Option<String> {
        self.best_match_with_input(requested, &Value::Null)
    }

    pub fn best_match_with_input(&self, requested: &str, input: &Value) -> Option<String> {
        let normalized_requested = normalize_name(requested);
        if normalized_requested.is_empty() {
            return None;
        }

        let intent_tokens = extract_intent_tokens(input);

        self.skills
            .values()
            .map(|skill| {
                let name = skill.name().to_string();
                let normalized_name = normalize_name(&name);
                let name_score = name_similarity(&normalized_requested, &normalized_name);
                let runtime_score = self.score_of(&name).unwrap_or(0.5);
                let semantic_score = semantic_similarity(&intent_tokens, skill);
                let composite = (0.35 * name_score + 0.25 * runtime_score + 0.40 * semantic_score)
                    .clamp(0.0, 1.0);
                (name.clone(), composite)
            })
            .filter(|(_, score)| *score >= 0.55)
            .max_by(|left, right| {
                left.1
                    .partial_cmp(&right.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(name, _)| name)
    }

    /// Create a new skill from a prompt template.
    ///
    /// Generates a `PromptBasedSkill` that wraps the given prompt into a Skill trait.
    /// The skill is automatically persisted to disk if `persistence_path` is set.
    pub fn create_skill_from_prompt(
        &mut self,
        name: &str,
        description: &str,
        prompt_template: &str,
        input_schema: HashMap<String, String>,
    ) -> Result<()> {
        // Validate name uniqueness
        if self.skills.contains_key(name) {
            anyhow::bail!(
                "{}",
                tf("error.skill_already_registered", &[("name", name)])
            );
        }

        let skill = PromptBasedSkill {
            name: name.to_string(),
            description: description.to_string(),
            prompt_template: prompt_template.to_string(),
            input_schema: input_schema.clone(),
            timeout_secs: 120,
            max_retries: 2,
        };

        self.register(Arc::new(skill))?;

        // Store original data for disk persistence
        self.prompt_skill_data.insert(
            name.to_string(),
            SavedPromptSkill {
                name: name.to_string(),
                description: description.to_string(),
                prompt_template: prompt_template.to_string(),
                input_schema,
                created_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64,
            },
        );

        // Persist to disk if persistence path is configured
        if self.persistence_path.is_some() {
            if let Err(e) = self.save_prompt_skills_to_disk() {
                tracing::warn!("Failed to persist prompt skills: {}", e);
            }
        }

        // Record evolution
        self.evolution_history
            .entry(name.to_string())
            .or_default()
            .push(SkillVersionRecord {
                skill_name: name.to_string(),
                version: 1,
                change_description: "Created from prompt template".to_string(),
                score_at_version: 0.5,
                timestamp_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
            });
        // Cap evolution history at 50 records per skill
        if let Some(history) = self.evolution_history.get_mut(name) {
            if history.len() > 50 {
                history.drain(0..history.len() - 50);
            }
        }

        Ok(())
    }

    /// Create a new skill by composing two existing skills.
    ///
    /// The composed skill first executes `skill_a`, then passes its output
    /// as input to `skill_b`.
    pub fn compose_skills(
        &mut self,
        name: &str,
        description: &str,
        skill_a: &str,
        skill_b: &str,
    ) -> Result<()> {
        if self.skills.contains_key(name) {
            anyhow::bail!(
                "{}",
                tf("error.skill_already_registered", &[("name", name)])
            );
        }
        if !self.skills.contains_key(skill_a) {
            anyhow::bail!("{}", tf("error.skill_not_found", &[("name", skill_a)]));
        }
        if !self.skills.contains_key(skill_b) {
            anyhow::bail!("{}", tf("error.skill_not_found", &[("name", skill_b)]));
        }

        // Create a shared registry for ComposedSkill to reference at execution time.
        // This allows the composed skill to look up its sub-skills dynamically.
        let composed_registry = Arc::new(Mutex::new(SkillRegistry::default()));
        // Clone the looked-up skills into the composed registry so they can be
        // resolved at execution time without holding the main registry lock.
        if let Some(sa) = self.skills.get(skill_a) {
            let _ = composed_registry.lock().map(|mut r| r.register(sa.clone()));
        }
        if let Some(sb) = self.skills.get(skill_b) {
            let _ = composed_registry.lock().map(|mut r| r.register(sb.clone()));
        }

        let skill = ComposedSkill {
            name: name.to_string(),
            description: format!("{}: {} \u{2192} {}", description, skill_a, skill_b),
            skill_a: skill_a.to_string(),
            skill_b: skill_b.to_string(),
            registry: composed_registry,
        };

        self.register(Arc::new(skill))?;

        self.evolution_history
            .entry(name.to_string())
            .or_default()
            .push(SkillVersionRecord {
                skill_name: name.to_string(),
                version: 1,
                change_description: format!("Composed from '{}' and '{}'", skill_a, skill_b),
                score_at_version: 0.5,
                timestamp_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64,
            });
        // Cap evolution history at 50 records per skill
        if let Some(history) = self.evolution_history.get_mut(name) {
            if history.len() > 50 {
                history.drain(0..history.len() - 50);
            }
        }

        Ok(())
    }

    /// Get evolution history for a skill.
    pub fn skill_evolution(&self, name: &str) -> Vec<SkillVersionRecord> {
        self.evolution_history
            .get(name)
            .cloned()
            .unwrap_or_default()
    }

    /// List all known skill names (for discovery).
    pub fn list_skills(&self) -> Vec<String> {
        self.skills.keys().cloned().collect()
    }

    // -----------------------------------------------------------------------
    // Disk persistence for prompt-based skills
    // -----------------------------------------------------------------------

    /// Set the persistence path for prompt-based skills.
    /// When set, `create_skill_from_prompt` will automatically persist.
    pub fn set_persistence_path(&mut self, path: PathBuf) {
        self.persistence_path = Some(path);
    }

    /// Load saved prompt skills from disk and register them in the registry.
    ///
    /// This is intended to be called once at startup, after setting
    /// the persistence path. Skills that were previously saved to disk
    /// (e.g. from `create_skill_from_prompt`) are recreated and registered.
    pub fn load_prompt_skills_from_disk(&mut self) -> Result<()> {
        let Some(ref path) = self.persistence_path else {
            return Ok(());
        };
        if !path.exists() {
            return Ok(());
        }
        let content =
            std::fs::read_to_string(path).with_context(|| "failed to read prompt skills file")?;
        let saved: Vec<SavedPromptSkill> =
            serde_json::from_str(&content).context("failed to parse prompt skills file")?;
        for entry in saved {
            let name = entry.name.clone();
            let ps = PromptBasedSkill {
                name: name.clone(),
                description: entry.description.clone(),
                prompt_template: entry.prompt_template.clone(),
                input_schema: entry.input_schema.clone(),
                timeout_secs: 120,
                max_retries: 2,
            };
            self.prompt_skill_data.insert(name.clone(), entry);
            self.skills.insert(name.clone(), Arc::new(ps));
            self.stats.entry(name).or_default();
        }
        Ok(())
    }

    /// Save all prompt-based skills to disk.
    ///
    /// Only skills tracked in `prompt_skill_data` are persisted.
    /// This includes skills created via `create_skill_from_prompt`
    /// and SKILL.md imports.
    pub fn save_prompt_skills_to_disk(&self) -> Result<()> {
        let Some(ref path) = self.persistence_path else {
            return Ok(());
        };
        let saved: Vec<&SavedPromptSkill> = self.prompt_skill_data.values().collect();
        if saved.is_empty() {
            // If the file exists but there are no prompt skills, remove it
            if path.exists() {
                let _ = std::fs::remove_file(path);
            }
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let content =
            serde_json::to_string_pretty(&saved).context("failed to serialize prompt skills")?;
        std::fs::write(path, content)
            .with_context(|| format!("failed to write {}", path.display()))?;
        Ok(())
    }

    /// Remove a prompt-based skill from the registry and persist the change.
    pub fn remove_prompt_skill(&mut self, name: &str) -> Result<()> {
        self.skills.remove(name);
        self.stats.remove(name);
        self.evolution_history.remove(name);
        self.prompt_skill_data.remove(name);
        self.save_prompt_skills_to_disk()
    }
}

// ---------------------------------------------------------------------------
// Skill implementations
// ---------------------------------------------------------------------------

/// A skill created from a prompt template.
#[derive(Debug, Clone)]
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
    #[cfg_attr(not(test), allow(dead_code))] // public API — reserved for external registry wiring
    pub fn boxed(self) -> Arc<dyn Skill> {
        Arc::new(self)
    }
}

/// A skill composed from two other skills (pipeline: A \u2192 B).
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

fn normalize_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>()
}

fn name_similarity(left: &str, right: &str) -> f64 {
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

fn extract_intent_tokens(input: &Value) -> std::collections::BTreeSet<String> {
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

fn semantic_similarity(
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

#[cfg(test)]
mod tests {
    use super::*;

    struct EchoSkill;

    #[async_trait]
    impl Skill for EchoSkill {
        fn name(&self) -> &str {
            "echo_skill"
        }

        fn description(&self) -> &str {
            "Echoes input"
        }

        async fn execute(&self, input: &Value) -> Result<Value> {
            Ok(input.clone())
        }
    }

    #[tokio::test]
    async fn registry_lists_and_executes_skills() {
        let mut registry = SkillRegistry::default();
        registry.register(Arc::new(EchoSkill)).unwrap();

        let listed = registry.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "echo_skill");

        let skill = registry.get("echo_skill").unwrap();
        let result = skill.execute(&json!({"value": 1})).await.unwrap();
        assert_eq!(result["value"], 1);
    }

    #[test]
    fn register_rejects_empty_name() {
        struct BadSkill;
        #[async_trait]
        impl Skill for BadSkill {
            fn name(&self) -> &str {
                ""
            }
            async fn execute(&self, input: &Value) -> Result<Value> {
                Ok(input.clone())
            }
        }
        let mut registry = SkillRegistry::default();
        assert!(registry.register(Arc::new(BadSkill)).is_err());
    }

    #[test]
    fn register_rejects_name_too_long() {
        let long_name = "a".repeat(65);
        struct LongSkill(String);
        #[async_trait]
        impl Skill for LongSkill {
            fn name(&self) -> &str {
                &self.0
            }
            async fn execute(&self, input: &Value) -> Result<Value> {
                Ok(input.clone())
            }
        }
        let mut registry = SkillRegistry::default();
        assert!(registry.register(Arc::new(LongSkill(long_name))).is_err());
    }

    #[test]
    fn register_rejects_invalid_chars() {
        struct BadCharsSkill;
        #[async_trait]
        impl Skill for BadCharsSkill {
            fn name(&self) -> &str {
                "Bad Skill!"
            }
            async fn execute(&self, input: &Value) -> Result<Value> {
                Ok(input.clone())
            }
        }
        let mut registry = SkillRegistry::default();
        assert!(registry.register(Arc::new(BadCharsSkill)).is_err());
    }

    #[test]
    fn register_rejects_duplicate() {
        let mut registry = SkillRegistry::default();
        registry.register(Arc::new(EchoSkill)).unwrap();
        let err = registry.register(Arc::new(EchoSkill)).unwrap_err();
        assert!(err.to_string().contains("error.skill_already_registered"));
    }

    #[test]
    fn register_rejects_non_object_schema() {
        struct BadSchemaSkill;
        #[async_trait]
        impl Skill for BadSchemaSkill {
            fn name(&self) -> &str {
                "bad-schema"
            }
            fn input_schema(&self) -> Value {
                json!("not-an-object")
            }
            async fn execute(&self, input: &Value) -> Result<Value> {
                Ok(input.clone())
            }
        }
        let mut registry = SkillRegistry::default();
        assert!(registry.register(Arc::new(BadSchemaSkill)).is_err());
    }

    #[tokio::test]
    async fn builtin_echo_skill_roundtrips() {
        let skill = super::EchoSkill;
        assert_eq!(skill.name(), "builtin.echo");
        let input = json!({"key": "value", "num": 42});
        let output: Value = skill.execute(&input).await.unwrap();
        assert_eq!(output, input);
    }

    #[test]
    fn unregister_removes_skill_and_stats() {
        let mut registry = SkillRegistry::default();
        registry.register(Arc::new(EchoSkill)).unwrap();
        registry.record_outcome("echo_skill", true, Duration::from_millis(12));

        assert!(registry.unregister("echo_skill"));
        assert!(registry.get("echo_skill").is_none());
        assert!(registry.score_of("echo_skill").is_none());
        assert!(!registry.unregister("echo_skill"));
    }
}
