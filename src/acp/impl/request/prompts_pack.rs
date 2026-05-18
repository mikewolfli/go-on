//! Prompt templates system
//!
//! Manages prompt template files organized by language:
//!   ./prompts/{lang}.json      — built-in prompts for each language
//!   ./prompts/custom/{lang}.json — user-created prompts
//!
//! Exposed via:
//!   - RPC method `prompts.list` — list all categories and templates for current language
//!   - RPC method `prompts.search` — search templates by keyword
//!   - RPC method `prompts.get` — get template by id
//!   - RPC method `prompts.create` — create custom template
//!   - RPC method `prompts.update` — update custom template
//!   - RPC method `prompts.delete` — delete custom template
//!   - MCP tool `prompts_list`, `prompts_get` — for LLM skill discovery

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// A single prompt template
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptTemplate {
    pub id: String,
    pub title: String,
    pub description: String,
    pub content: String,
    pub tags: Vec<String>,
}

/// A category of prompt templates
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptCategory {
    pub id: String,
    pub name: String,
    pub icon: String,
    pub templates: Vec<PromptTemplate>,
}

/// Root structure for prompt JSON files
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptCollection {
    pub categories: Vec<PromptCategory>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
enum PromptCollectionFile {
    Wrapped { categories: Vec<PromptCategory> },
    Flat(Vec<PromptCategory>),
}

/// Prompt manager — loads and serves prompt templates by language
pub struct PromptManager {
    /// Base path to the prompts directory (default: "./prompts")
    base_path: PathBuf,
    /// In-memory cache: language_code -> PromptCollection
    cache: Mutex<HashMap<String, PromptCollection>>,
    /// Custom prompts cache: language_code -> Vec<PromptTemplate>
    custom_cache: Mutex<HashMap<String, Vec<PromptTemplate>>>,
}

impl PromptManager {
    fn parse_collection(content: &str) -> Result<PromptCollection> {
        let parsed: PromptCollectionFile = serde_json::from_str(content)
            .context("failed to parse prompts JSON: expected {'categories': [...]} or [...]")?;

        let categories = match parsed {
            PromptCollectionFile::Wrapped { categories } => categories,
            PromptCollectionFile::Flat(categories) => categories,
        };

        Ok(PromptCollection { categories })
    }

    fn merge_with_fallback(
        mut primary: PromptCollection,
        fallback: PromptCollection,
    ) -> PromptCollection {
        for fallback_cat in fallback.categories {
            if let Some(primary_cat) = primary
                .categories
                .iter_mut()
                .find(|c| c.id == fallback_cat.id)
            {
                for fallback_tpl in fallback_cat.templates {
                    if primary_cat
                        .templates
                        .iter()
                        .all(|t| t.id != fallback_tpl.id)
                    {
                        primary_cat.templates.push(fallback_tpl);
                    }
                }
            } else {
                primary.categories.push(fallback_cat);
            }
        }
        primary
    }

    /// Create a new prompt manager with the given base path.
    /// The base path should point to the `prompts/` directory.
    pub fn new(base_path: PathBuf) -> Self {
        let resolved_base_path = if base_path.exists() {
            base_path
        } else if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                let candidate = parent.join("prompts");
                if candidate.exists() {
                    candidate
                } else {
                    PathBuf::from("prompts")
                }
            } else {
                PathBuf::from("prompts")
            }
        } else {
            PathBuf::from("prompts")
        };

        Self {
            base_path: resolved_base_path,
            cache: Mutex::new(HashMap::new()),
            custom_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Get the language code from config language string.
    /// Maps: "zh-CN" -> "zh-CN", "zh_TW" -> "zh-TW", "en" or anything else -> "en"
    fn normalize_lang(lang: &str) -> &str {
        match lang {
            "zh-CN" | "zh_CN" | "zh-cn" => "zh-CN",
            "zh-TW" | "zh_TW" | "zh-tw" => "zh-TW",
            _ => "en",
        }
    }

    /// Load prompts for a given language from disk.
    fn load_from_disk(&self, lang: &str) -> Result<PromptCollection> {
        let filename = format!("{}.json", lang);
        let path = self.base_path.join(&filename);
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read prompts file: {}", path.display()))?;
        let collection = Self::parse_collection(&content)
            .with_context(|| format!("failed to parse prompts file: {}", path.display()))?;
        Ok(collection)
    }

    /// Load custom prompts for a given language from disk.
    fn load_custom_from_disk(&self, lang: &str) -> Result<Vec<PromptTemplate>> {
        let custom_dir = self.base_path.join("custom");
        let filename = format!("{}.json", lang);
        let path = custom_dir.join(&filename);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read custom prompts file: {}", path.display()))?;
        let templates: Vec<PromptTemplate> = serde_json::from_str(&content)
            .with_context(|| format!("failed to parse custom prompts file: {}", path.display()))?;
        Ok(templates)
    }

    /// Save custom prompts for a given language to disk.
    fn save_custom_to_disk(&self, lang: &str, templates: &[PromptTemplate]) -> Result<()> {
        let custom_dir = self.base_path.join("custom");
        std::fs::create_dir_all(&custom_dir)?;
        let filename = format!("{}.json", lang);
        let path = custom_dir.join(&filename);
        let content = serde_json::to_string_pretty(templates)?;
        std::fs::write(&path, &content)?;
        Ok(())
    }

    /// Get the prompt collection for a language (cached).
    pub fn get_collection(&self, lang: &str) -> Result<PromptCollection> {
        let lang = Self::normalize_lang(lang);
        {
            let cache = self
                .cache
                .lock()
                .map_err(|e| anyhow::anyhow!("mutex poisoned: {}", e))?;
            if let Some(collection) = cache.get(lang) {
                return Ok(collection.clone());
            }
        }
        let mut collection = self.load_from_disk(lang)?;
        if lang != "en" {
            if let Ok(fallback) = self.load_from_disk("en") {
                collection = Self::merge_with_fallback(collection, fallback);
            }
        }
        {
            let mut cache = self
                .cache
                .lock()
                .map_err(|e| anyhow::anyhow!("mutex poisoned: {}", e))?;
            cache.insert(lang.to_string(), collection.clone());
        }
        Ok(collection)
    }

    /// Get all templates (built-in + custom) for a language, merged by category.
    pub fn get_all_templates(&self, lang: &str) -> Result<PromptCollection> {
        let mut collection = self.get_collection(lang)?;
        let lang = Self::normalize_lang(lang);
        let custom = {
            let cache = self
                .custom_cache
                .lock()
                .map_err(|e| anyhow::anyhow!("mutex poisoned: {}", e))?;
            if let Some(custom) = cache.get(lang) {
                custom.clone()
            } else {
                drop(cache);
                let custom = self.load_custom_from_disk(lang).unwrap_or_default();
                let mut cache = self
                    .custom_cache
                    .lock()
                    .map_err(|e| anyhow::anyhow!("mutex poisoned: {}", e))?;
                cache.insert(lang.to_string(), custom.clone());
                custom
            }
        };

        // Add custom templates to the "custom" category
        if !custom.is_empty() {
            if let Some(custom_cat) = collection.categories.iter_mut().find(|c| c.id == "custom") {
                for ct in custom {
                    if let Some(existing) = custom_cat.templates.iter_mut().find(|t| t.id == ct.id)
                    {
                        *existing = ct;
                    } else {
                        custom_cat.templates.push(ct);
                    }
                }
            } else {
                collection.categories.push(PromptCategory {
                    id: "custom".to_string(),
                    name: match lang {
                        "zh-CN" => "自定义".to_string(),
                        "zh-TW" => "自訂".to_string(),
                        _ => "Custom".to_string(),
                    },
                    icon: "⭐".to_string(),
                    templates: custom,
                });
            }
        }

        Ok(collection)
    }

    /// Search templates by keyword across all categories.
    pub fn search_templates(
        &self,
        lang: &str,
        query: &str,
    ) -> Vec<(String, String, PromptTemplate)> {
        let collection = match self.get_all_templates(lang) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        let query_lower = query.to_lowercase();
        let mut results = Vec::new();
        for cat in &collection.categories {
            for tpl in &cat.templates {
                let title_lower = tpl.title.to_lowercase();
                let desc_lower = tpl.description.to_lowercase();
                let content_lower = tpl.content.to_lowercase();
                let tag_match = tpl
                    .tags
                    .iter()
                    .any(|t| t.to_lowercase().contains(&query_lower));
                if title_lower.contains(&query_lower)
                    || desc_lower.contains(&query_lower)
                    || content_lower.contains(&query_lower)
                    || tag_match
                {
                    results.push((cat.id.clone(), cat.name.clone(), tpl.clone()));
                }
            }
        }
        results
    }

    /// Get a single template by id across all categories.
    pub fn get_template(
        &self,
        lang: &str,
        template_id: &str,
    ) -> Option<(String, String, PromptTemplate)> {
        let normalized = template_id.trim().trim_start_matches('/');
        let collection = self.get_all_templates(lang).ok()?;

        // Support scoped ids: "category.template"
        if let Some((cat_id, tpl_id)) = normalized.split_once('.') {
            for cat in &collection.categories {
                if cat.id != cat_id {
                    continue;
                }
                for tpl in &cat.templates {
                    if tpl.id == tpl_id {
                        return Some((cat.id.clone(), cat.name.clone(), tpl.clone()));
                    }
                }
            }
        }

        for cat in &collection.categories {
            for tpl in &cat.templates {
                if tpl.id == normalized {
                    return Some((cat.id.clone(), cat.name.clone(), tpl.clone()));
                }
            }
        }
        None
    }

    /// Create a custom template.
    pub fn create_template(&self, lang: &str, template: PromptTemplate) -> Result<()> {
        let lang = Self::normalize_lang(lang);
        let mut custom = self.load_custom_from_disk(lang).unwrap_or_default();
        // Check for duplicate id
        if custom.iter().any(|t| t.id == template.id) {
            anyhow::bail!("template with id '{}' already exists", template.id);
        }
        custom.push(template);
        self.save_custom_to_disk(lang, &custom)?;
        // Invalidate cache
        self.custom_cache
            .lock()
            .map_err(|e| anyhow::anyhow!("mutex poisoned: {}", e))?
            .insert(lang.to_string(), custom);
        Ok(())
    }

    /// Update a custom template.
    pub fn update_template(&self, lang: &str, template: PromptTemplate) -> Result<()> {
        let lang = Self::normalize_lang(lang);
        let mut custom = self.load_custom_from_disk(lang).unwrap_or_default();
        if let Some(pos) = custom.iter().position(|t| t.id == template.id) {
            custom[pos] = template;
        } else {
            anyhow::bail!("template with id '{}' not found", template.id);
        }
        self.save_custom_to_disk(lang, &custom)?;
        self.custom_cache
            .lock()
            .map_err(|e| anyhow::anyhow!("mutex poisoned: {}", e))?
            .insert(lang.to_string(), custom);
        Ok(())
    }

    /// Delete a custom template.
    pub fn delete_template(&self, lang: &str, template_id: &str) -> Result<()> {
        let lang = Self::normalize_lang(lang);
        let mut custom = self.load_custom_from_disk(lang).unwrap_or_default();
        if let Some(pos) = custom.iter().position(|t| t.id == template_id) {
            custom.remove(pos);
        } else {
            anyhow::bail!("template with id '{}' not found", template_id);
        }
        self.save_custom_to_disk(lang, &custom)?;
        self.custom_cache
            .lock()
            .map_err(|e| anyhow::anyhow!("mutex poisoned: {}", e))?
            .insert(lang.to_string(), custom);
        Ok(())
    }

    /// Invalidate all caches (e.g. when language changes).
    pub fn invalidate_cache(&self) -> Result<()> {
        self.cache
            .lock()
            .map_err(|e| anyhow::anyhow!("mutex poisoned: {}", e))?
            .clear();
        self.custom_cache
            .lock()
            .map_err(|e| anyhow::anyhow!("mutex poisoned: {}", e))?
            .clear();
        Ok(())
    }
}

// ── RPC Handlers ─────────────────────────────────────────────────────

/// Handle `prompts.list` — returns all categories and templates for current language.
pub fn handle_prompts_list(pm: &PromptManager, lang: &str) -> Result<Value> {
    let collection = pm.get_all_templates(lang)?;
    Ok(json!(collection))
}

/// Handle `prompts.search` — search templates by keyword.
pub fn handle_prompts_search(pm: &PromptManager, lang: &str, query: &str) -> Result<Value> {
    let results = pm.search_templates(lang, query);
    let items: Vec<Value> = results
        .into_iter()
        .map(|(cat_id, cat_name, tpl)| {
            json!({
                "category_id": cat_id,
                "category_name": cat_name,
                "template": tpl,
            })
        })
        .collect();
    Ok(json!({
        "query": query,
        "results": items,
        "total": items.len(),
    }))
}

/// Handle `prompts.get` — get a single template by id.
pub fn handle_prompts_get(pm: &PromptManager, lang: &str, template_id: &str) -> Result<Value> {
    match pm.get_template(lang, template_id) {
        Some((cat_id, cat_name, tpl)) => Ok(json!({
            "category_id": cat_id,
            "category_name": cat_name,
            "template": tpl,
        })),
        None => anyhow::bail!("template '{}' not found", template_id),
    }
}

/// Handle `prompts.create` — create a custom template.
pub fn handle_prompts_create(pm: &PromptManager, lang: &str, params: &Value) -> Result<Value> {
    let template: PromptTemplate =
        serde_json::from_value(params.clone()).context("invalid template data")?;
    pm.create_template(lang, template)?;
    Ok(json!({"ok": true}))
}

/// Handle `prompts.update` — update a custom template.
pub fn handle_prompts_update(pm: &PromptManager, lang: &str, params: &Value) -> Result<Value> {
    let template: PromptTemplate =
        serde_json::from_value(params.clone()).context("invalid template data")?;
    pm.update_template(lang, template)?;
    Ok(json!({"ok": true}))
}

/// Handle `prompts.delete` — delete a custom template.
pub fn handle_prompts_delete(pm: &PromptManager, lang: &str, params: &Value) -> Result<Value> {
    let template_id = params
        .get("id")
        .and_then(|v| v.as_str())
        .context("missing required field: id")?;
    pm.delete_template(lang, template_id)?;
    Ok(json!({"ok": true}))
}

// ── MCP Tool Helpers ────────────────────────────────────────────────

/// Build response for MCP tool `prompts_list`.
/// Returns all categories and templates as a JSON array.
pub fn build_prompts_list_tool(pm: &PromptManager, lang: &str) -> Value {
    match pm.get_all_templates(lang) {
        Ok(collection) => json!({
            "ok": true,
            "categories": collection.categories,
            "total_categories": collection.categories.len(),
        }),
        Err(e) => json!({
            "ok": false,
            "error": format!("{}", e),
        }),
    }
}

/// Build response for MCP tool `prompts_get`.
/// Returns a single template by id.
pub fn build_prompts_get_tool(pm: &PromptManager, lang: &str, id: &str) -> Value {
    match pm.get_template(lang, id) {
        Some((cat_id, cat_name, tpl)) => json!({
            "ok": true,
            "category_id": cat_id,
            "category_name": cat_name,
            "template": tpl,
        }),
        None => json!({
            "ok": false,
            "error": format!("template '{}' not found", id),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_collection_supports_wrapped_and_flat_formats() {
        let wrapped = r#"{
            "categories": [
                {
                    "id": "software_dev",
                    "name": "Software Development",
                    "icon": "💻",
                    "templates": []
                }
            ]
        }"#;
        let flat = r#"[
            {
                "id": "software_dev",
                "name": "软件开发",
                "icon": "💻",
                "templates": []
            }
        ]"#;

        let wrapped_parsed = PromptManager::parse_collection(wrapped).expect("wrapped parse");
        let flat_parsed = PromptManager::parse_collection(flat).expect("flat parse");

        assert_eq!(wrapped_parsed.categories.len(), 1);
        assert_eq!(flat_parsed.categories.len(), 1);
        assert_eq!(wrapped_parsed.categories[0].id, "software_dev");
        assert_eq!(flat_parsed.categories[0].id, "software_dev");
    }

    #[test]
    fn scoped_template_id_lookup_is_supported() {
        let tmp = std::env::temp_dir().join(format!(
            "go_on_prompts_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");

        let content = r#"{
            "categories": [
                {
                    "id": "business",
                    "name": "Business",
                    "icon": "📊",
                    "templates": [
                        {
                            "id": "risk_assessment",
                            "title": "Risk A",
                            "description": "A",
                            "content": "A",
                            "tags": []
                        }
                    ]
                },
                {
                    "id": "finance",
                    "name": "Finance",
                    "icon": "💰",
                    "templates": [
                        {
                            "id": "risk_assessment",
                            "title": "Risk B",
                            "description": "B",
                            "content": "B",
                            "tags": []
                        }
                    ]
                }
            ]
        }"#;
        std::fs::write(tmp.join("en.json"), content).expect("write prompts file");

        let pm = PromptManager::new(tmp.clone());
        let scoped = pm
            .get_template("en", "finance.risk_assessment")
            .expect("scoped template");
        assert_eq!(scoped.0, "finance");
        assert_eq!(scoped.2.title, "Risk B");

        let _ = std::fs::remove_file(tmp.join("en.json"));
        let _ = std::fs::remove_dir_all(tmp);
    }

    #[test]
    fn non_en_collection_merges_missing_templates_from_en() {
        let tmp = std::env::temp_dir().join(format!(
            "go_on_prompts_merge_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");

        let en = r#"{
            "categories": [
                {
                    "id": "software_dev",
                    "name": "Software Development",
                    "icon": "💻",
                    "templates": [
                        {
                            "id": "architecture_design",
                            "title": "Architecture Design",
                            "description": "D",
                            "content": "D",
                            "tags": []
                        }
                    ]
                }
            ]
        }"#;
        let zh_cn = r#"[
            {
                "id": "software_dev",
                "name": "软件开发",
                "icon": "💻",
                "templates": [
                    {
                        "id": "explain_code",
                        "title": "解释代码",
                        "description": "D",
                        "content": "D",
                        "tags": []
                    }
                ]
            }
        ]"#;

        std::fs::write(tmp.join("en.json"), en).expect("write en prompts");
        std::fs::write(tmp.join("zh-CN.json"), zh_cn).expect("write zh-CN prompts");

        let pm = PromptManager::new(tmp.clone());
        let merged = pm.get_collection("zh-CN").expect("merged collection");
        let cat = merged
            .categories
            .iter()
            .find(|c| c.id == "software_dev")
            .expect("software_dev category");

        assert!(cat.templates.iter().any(|t| t.id == "explain_code"));
        assert!(cat.templates.iter().any(|t| t.id == "architecture_design"));

        let _ = std::fs::remove_file(tmp.join("en.json"));
        let _ = std::fs::remove_file(tmp.join("zh-CN.json"));
        let _ = std::fs::remove_dir_all(tmp);
    }
}
