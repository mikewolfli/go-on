use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
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

#[derive(Default)]
pub struct SkillRegistry {
    skills: HashMap<String, Arc<dyn Skill>>,
    stats: HashMap<String, SkillRuntimeStats>,
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
    pub fn register(&mut self, skill: Arc<dyn Skill>) {
        let name = skill.name().to_string();
        self.skills.insert(name.clone(), skill);
        self.stats.entry(name).or_default();
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Skill>> {
        self.skills.get(name).cloned()
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
}

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
        registry.register(Arc::new(EchoSkill));

        let listed = registry.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "echo_skill");

        let skill = registry.get("echo_skill").unwrap();
        let result = skill.execute(&json!({"value": 1})).await.unwrap();
        assert_eq!(result["value"], 1);
    }
}
