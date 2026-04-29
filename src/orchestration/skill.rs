use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

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

#[derive(Default)]
pub struct SkillRegistry {
    skills: HashMap<String, Arc<dyn Skill>>,
    stats: HashMap<String, SkillRuntimeStats>,
    /// Skill evolution history keyed by skill name
    pub evolution_history: HashMap<String, Vec<SkillVersionRecord>>,
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
                "skill name '{}' length {} is outside [1, 64]",
                name,
                name.len()
            );
        }
        if !name.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_' || c == '-'
        }) {
            anyhow::bail!(
                "skill name '{}' contains invalid characters (allowed: a-z 0-9 . _ -)",
                name
            );
        }
        if self.skills.contains_key(&name) {
            anyhow::bail!("skill '{}' is already registered", name);
        }
        match skill.input_schema() {
            serde_json::Value::Object(_) => {}
            other => anyhow::bail!(
                "skill '{}' input_schema must be a JSON object, got: {}",
                name,
                other
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
                let composite = (0.45 * name_score + 0.35 * runtime_score + 0.20 * semantic_score)
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
    pub fn create_skill_from_prompt(
        &mut self,
        name: &str,
        description: &str,
        prompt_template: &str,
        input_schema: HashMap<String, String>,
    ) -> Result<()> {
        // Validate name uniqueness
        if self.skills.contains_key(name) {
            anyhow::bail!("Skill '{}' already exists", name);
        }

        let skill = PromptBasedSkill {
            name: name.to_string(),
            description: description.to_string(),
            prompt_template: prompt_template.to_string(),
            input_schema,
        };

        self.register(Arc::new(skill))?;

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
            anyhow::bail!("Skill '{}' already exists", name);
        }
        if !self.skills.contains_key(skill_a) {
            anyhow::bail!("Source skill '{}' not found", skill_a);
        }
        if !self.skills.contains_key(skill_b) {
            anyhow::bail!("Source skill '{}' not found", skill_b);
        }

        let skill = ComposedSkill {
            name: name.to_string(),
            description: format!("{}: {} \u{2192} {}", description, skill_a, skill_b),
            skill_a: skill_a.to_string(),
            skill_b: skill_b.to_string(),
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

    async fn execute(&self, _input: &Value) -> Result<Value> {
        // In a real implementation, this would execute the prompt through an LLM.
        // For now, return a placeholder outcome indicating the skill exists.
        let template_preview: &str = &self.prompt_template[..self.prompt_template.len().min(100)];
        Ok(json!({
            "success": true,
            "summary": format!("Prompt-based skill '{}' executed (template: {})", self.name, self.prompt_template),
            "details": {
                "skill_type": "prompt_based",
                "template_preview": template_preview,
            }
        }))
    }
}

impl PromptBasedSkill {
    /// Register this skill with a `SkillRegistry`.
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

    async fn execute(&self, _input: &Value) -> Result<Value> {
        Ok(json!({
            "success": true,
            "summary": format!("Composed skill '{}' ({} \u{2192} {})", self.name, self.skill_a, self.skill_b),
            "details": {
                "skill_type": "composed",
                "pipeline": [self.skill_a, self.skill_b],
            }
        }))
    }
}

impl ComposedSkill {
    /// Register this skill with a `SkillRegistry`.
    pub fn boxed(self) -> Arc<dyn Skill> {
        Arc::new(self)
    }
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
        assert!(err.to_string().contains("already registered"));
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
