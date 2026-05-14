//! Agent factory implementation — F-GAP-13 (FUTURE4.M4 / BLUE38 §6.8).
//!
//! The `AgentFactory` manages sub-agent lifecycle: template registration,
//! dynamic instance creation with config overrides, lookup by capability,
//! expiration pruning, and runtime metrics.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

// ─── Data structures ─────────────────────────────────────────────────────────

/// A reusable template for creating sub-agent instances.
///
/// Templates are registered with the factory and used as blueprints when
/// creating new agents. Each template carries capability tags that allow
/// capability-based agent discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTemplate {
    /// Unique name for this template (used as the identifier).
    pub name: String,
    /// Base agent type (e.g. "openai", "anthropic", "deepseek").
    pub base_type: String,
    /// Default configuration values applied to every new instance.
    pub default_config: HashMap<String, String>,
    /// Tags describing capabilities of agents created from this template.
    pub capability_tags: Vec<String>,
    /// Operating mode for agents created from this template.
    pub mode: String,
    /// Programming principles agents should follow.
    pub principles: Vec<String>,
}

/// A running sub-agent instance created by the factory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentInstance {
    /// Unique identifier for this instance.
    pub id: String,
    /// Name of the template this instance was created from.
    pub template_name: String,
    /// Unix timestamp (milliseconds) when this instance was created.
    pub created_ms: u64,
    /// Configuration overrides applied on top of the template's defaults.
    pub config_overrides: HashMap<String, String>,
    /// Current status of the agent instance.
    #[allow(dead_code)] // F-GAP-12 — reserved for future metrics exposure
    pub status: String,
    /// Runtime metrics for this instance.
    #[allow(dead_code)] // F-GAP-12 — reserved for future metrics exposure
    pub metrics: HashMap<String, u64>,
}

/// Configuration for the agent factory itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentFactoryConfig {
    /// Maximum number of concurrently active agent instances (0 = unlimited).
    pub max_instances: u32,
    /// Whether users may register custom templates at runtime.
    pub allow_custom_templates: bool,
    /// Default time-to-live in milliseconds for new agent instances.
    /// Instances older than this may be pruned by `prune_expired()`.
    pub default_ttl_ms: u64,
}

impl Default for AgentFactoryConfig {
    fn default() -> Self {
        Self {
            max_instances: 50,
            allow_custom_templates: true,
            default_ttl_ms: 86_400_000, // 24 hours
        }
    }
}

/// Snapshot of factory runtime metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactoryProfile {
    /// Total number of registered templates.
    pub total_templates: usize,
    /// Number of currently active agent instances.
    pub active_instances: usize,
    /// Total number of agent instances ever created.
    pub created_count: u64,
    /// Total number of agent instances ever destroyed.
    pub destroyed_count: u64,
    /// Templates grouped by category (base_type).
    pub templates_by_category: HashMap<String, usize>,
}

/// Request payload for creating a new sub-agent instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateAgentRequest {
    /// Name of the template to use.
    pub template_name: String,
    /// Configuration overrides to apply on top of the template's defaults.
    pub config_overrides: HashMap<String, String>,
    /// Optional time-to-live in milliseconds for this specific instance.
    /// If `None`, the factory's `default_ttl_ms` is used.
    pub ttl_ms: Option<u64>,
    /// Additional tags to attach to the instance for discovery.
    pub tags: Vec<String>,
}

// ─── Main factory ────────────────────────────────────────────────────────────

/// Agent factory — creates, configures, and manages sub-agent instances.
///
/// The factory is thread-safe and uses interior mutability (`Arc<Mutex<>>`)
/// so it can be shared across threads. All fallible operations return
/// `anyhow::Result`.
#[derive(Debug)]
pub struct AgentFactory {
    /// Factory configuration.
    config: AgentFactoryConfig,
    /// Registered agent templates (name → template).
    templates: Arc<Mutex<HashMap<String, AgentTemplate>>>,
    /// Active agent instances (id → instance).
    instances: Arc<Mutex<HashMap<String, SubAgentInstance>>>,
    /// Expiration timestamps for instances (id → expiry_ms).
    expirations: Arc<Mutex<HashMap<String, u64>>>,
    /// Monotonically increasing counter for instance IDs.
    id_counter: AtomicU64,
    /// Total number of instances ever created.
    created_count: AtomicU64,
    /// Total number of instances ever destroyed.
    destroyed_count: AtomicU64,
}

impl AgentFactory {
    /// Create a new `AgentFactory` with the given configuration.
    ///
    /// The factory starts with no templates and no active instances.
    pub fn new(config: AgentFactoryConfig) -> Self {
        Self {
            config,
            templates: Arc::new(Mutex::new(HashMap::new())),
            instances: Arc::new(Mutex::new(HashMap::new())),
            expirations: Arc::new(Mutex::new(HashMap::new())),
            id_counter: AtomicU64::new(1),
            created_count: AtomicU64::new(0),
            destroyed_count: AtomicU64::new(0),
        }
    }

    /// Register a new agent template in the factory.
    ///
    /// If a template with the same name already exists, it is overwritten.
    /// Registration may be denied if `allow_custom_templates` is `false`.
    pub fn register_template(&self, template: AgentTemplate) -> Result<()> {
        if !self.config.allow_custom_templates {
            return Err(anyhow!(
                "Custom template registration is disabled by factory configuration"
            ));
        }

        let mut templates = self
            .templates
            .lock()
            .map_err(|e| anyhow!("Failed to acquire lock on templates: {}", e))?;
        templates.insert(template.name.clone(), template);
        Ok(())
    }

    /// Create a sub-agent instance from a registered template.
    ///
    /// Returns the newly created `SubAgentInstance` on success. The instance
    /// is assigned a unique ID and its status is set to "running" by default.
    ///
    /// Returns an error if:
    /// - The template does not exist.
    /// - The maximum number of instances has been reached.
    pub fn create_agent(&self, request: CreateAgentRequest) -> Result<SubAgentInstance> {
        // Look up the template.
        let template = {
            let templates = self
                .templates
                .lock()
                .map_err(|e| anyhow!("Failed to acquire lock on templates: {}", e))?;
            templates
                .get(&request.template_name)
                .cloned()
                .ok_or_else(|| anyhow!("Template '{}' not found", request.template_name))?
        };

        // Check max instances limit.
        {
            let instances = self
                .instances
                .lock()
                .map_err(|e| anyhow!("Failed to acquire lock on instances: {}", e))?;
            let max = self.config.max_instances;
            if max > 0 && instances.len() as u32 >= max {
                return Err(anyhow!(
                    "Maximum number of agent instances ({}) reached",
                    max
                ));
            }
        }

        let now_ms = now_epoch_ms();
        let instance_id = format!("agent-{}", self.id_counter.fetch_add(1, Ordering::AcqRel));

        // Merge default config with overrides (overrides take precedence).
        let mut merged_config = template.default_config.clone();
        for (key, value) in &request.config_overrides {
            merged_config.insert(key.clone(), value.clone());
        }

        // Merge capability tags.
        let mut merged_tags = template.capability_tags.clone();
        merged_tags.extend(request.tags.iter().cloned());

        let instance = SubAgentInstance {
            id: instance_id.clone(),
            template_name: template.name,
            created_ms: now_ms,
            config_overrides: merged_config,
            status: "running".to_string(),
            metrics: HashMap::new(),
        };

        // Compute expiration.
        let ttl = request.ttl_ms.unwrap_or(self.config.default_ttl_ms);
        let expiry_ms = now_ms + ttl;

        {
            let mut instances = self
                .instances
                .lock()
                .map_err(|e| anyhow!("Failed to acquire lock on instances: {}", e))?;
            instances.insert(instance_id.clone(), instance.clone());
        }

        {
            let mut expirations = self
                .expirations
                .lock()
                .map_err(|e| anyhow!("Failed to acquire lock on expirations: {}", e))?;
            expirations.insert(instance_id, expiry_ms);
        }

        self.created_count.fetch_add(1, Ordering::Release);

        Ok(instance)
    }

    /// Destroy (remove) a sub-agent instance by its ID.
    ///
    /// This is idempotent — destroying a non-existent or already-destroyed
    /// instance is a no-op.
    pub fn destroy_agent(&self, instance_id: &str) {
        {
            let mut instances = match self.instances.lock() {
                Ok(guard) => guard,
                Err(_) => return,
            };
            if instances.remove(instance_id).is_none() {
                return; // Not found — no-op.
            }
        }

        {
            let mut expirations = match self.expirations.lock() {
                Ok(guard) => guard,
                Err(_) => return,
            };
            expirations.remove(instance_id);
        }

        self.destroyed_count.fetch_add(1, Ordering::Release);
    }

    /// Get details of a specific agent instance by its ID.
    pub fn get_agent(&self, instance_id: &str) -> Option<SubAgentInstance> {
        self.instances
            .lock()
            .ok()
            .and_then(|guard| guard.get(instance_id).cloned())
    }

    /// List all currently active agent instances.
    pub fn list_agents(&self) -> Vec<SubAgentInstance> {
        self.instances
            .lock()
            .ok()
            .map(|guard| guard.values().cloned().collect())
            .unwrap_or_default()
    }

    /// List all registered templates.
    pub fn list_templates(&self) -> Vec<AgentTemplate> {
        self.templates
            .lock()
            .ok()
            .map(|guard| guard.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Find active agent instances whose template matches the given capability tag.
    ///
    /// An instance matches if the tag appears in its template's `capability_tags`.
    /// Since instances do not store their own tags after creation, this method
    /// looks up the template that created each instance and checks its tags.
    pub fn find_agents_by_capability(&self, tag: &str) -> Vec<SubAgentInstance> {
        let templates = match self.templates.lock() {
            Ok(guard) => guard,
            Err(_) => return Vec::new(),
        };

        let instances = match self.instances.lock() {
            Ok(guard) => guard,
            Err(_) => return Vec::new(),
        };

        instances
            .values()
            .filter(|inst| {
                templates
                    .get(&inst.template_name)
                    .map(|t| t.capability_tags.iter().any(|t| t == tag))
                    .unwrap_or(false)
            })
            .cloned()
            .collect()
    }

    /// Return a snapshot of the factory's current runtime metrics.
    pub fn profile(&self) -> FactoryProfile {
        let templates = self
            .templates
            .lock()
            .ok()
            .map(|guard| guard.clone())
            .unwrap_or_default();

        let active_instances = self
            .instances
            .lock()
            .ok()
            .map(|guard| guard.len())
            .unwrap_or(0);

        let total_templates = templates.len();

        let mut templates_by_category: HashMap<String, usize> = HashMap::new();
        for template in templates.values() {
            *templates_by_category
                .entry(template.base_type.clone())
                .or_insert(0) += 1;
        }

        FactoryProfile {
            total_templates,
            active_instances,
            created_count: self.created_count.load(Ordering::Acquire),
            destroyed_count: self.destroyed_count.load(Ordering::Acquire),
            templates_by_category,
        }
    }

    /// Remove expired agent instances.
    ///
    /// An instance is considered expired if its recorded expiration timestamp
    /// (set at creation time) is in the past relative to the current clock.
    ///
    /// Returns the number of instances that were pruned.
    pub fn prune_expired(&self) -> usize {
        let now_ms = now_epoch_ms();

        // Collect expired instance IDs while holding only the expirations lock.
        let expired_ids: Vec<String> = self
            .expirations
            .lock()
            .ok()
            .map(|guard| {
                guard
                    .iter()
                    .filter(|(_, &expiry)| now_ms > expiry)
                    .map(|(id, _)| id.clone())
                    .collect()
            })
            .unwrap_or_default();

        let count = expired_ids.len();
        for id in &expired_ids {
            // Inline removal with consistent lock order (instances → expirations)
            // instead of calling destroy_agent, to avoid potential deadlock
            // with find_agents_by_capability (which locks templates → instances).
            {
                let mut instances = match self.instances.lock() {
                    Ok(guard) => guard,
                    Err(_) => continue,
                };
                if instances.remove(id).is_none() {
                    continue;
                }
            }
            {
                let mut expirations = match self.expirations.lock() {
                    Ok(guard) => guard,
                    Err(_) => continue,
                };
                expirations.remove(id);
            }
            self.destroyed_count.fetch_add(1, Ordering::Release);
        }
        count
    }
}

impl Default for AgentFactory {
    fn default() -> Self {
        Self::new(AgentFactoryConfig::default())
    }
}

// ─── Private helpers ─────────────────────────────────────────────────────────

/// Return the current Unix time in milliseconds.
fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_template(name: &str, base_type: &str, tags: &[&str]) -> AgentTemplate {
        AgentTemplate {
            name: name.to_string(),
            base_type: base_type.to_string(),
            default_config: HashMap::from([
                ("temperature".to_string(), "0.7".to_string()),
                ("max_tokens".to_string(), "2048".to_string()),
            ]),
            capability_tags: tags.iter().map(|s| s.to_string()).collect(),
            mode: "standard".to_string(),
            principles: vec!["Be concise".to_string(), "Be accurate".to_string()],
        }
    }

    fn sample_request(template_name: &str) -> CreateAgentRequest {
        CreateAgentRequest {
            template_name: template_name.to_string(),
            config_overrides: HashMap::new(),
            ttl_ms: None,
            tags: Vec::new(),
        }
    }

    // ── test_new_factory_empty ───────────────────────────────────────────────

    #[test]
    fn test_new_factory_empty() {
        let factory = AgentFactory::new(AgentFactoryConfig::default());
        assert!(factory.list_templates().is_empty());
        assert!(factory.list_agents().is_empty());
        let profile = factory.profile();
        assert_eq!(profile.total_templates, 0);
        assert_eq!(profile.active_instances, 0);
        assert_eq!(profile.created_count, 0);
        assert_eq!(profile.destroyed_count, 0);
        assert!(profile.templates_by_category.is_empty());
    }

    // ── test_register_and_list_templates ─────────────────────────────────────

    #[test]
    fn test_register_and_list_templates() {
        let factory = AgentFactory::default();
        let tmpl = sample_template("code-review", "openai", &["code", "review"]);
        factory
            .register_template(tmpl)
            .expect("Should register template");

        let templates = factory.list_templates();
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].name, "code-review");
        assert_eq!(templates[0].base_type, "openai");

        let profile = factory.profile();
        assert_eq!(profile.total_templates, 1);
    }

    // ── test_create_agent_from_template ──────────────────────────────────────

    #[test]
    fn test_create_agent_from_template() {
        let factory = AgentFactory::default();
        factory
            .register_template(sample_template("writer", "anthropic", &["text", "writing"]))
            .unwrap();

        let instance = factory
            .create_agent(sample_request("writer"))
            .expect("Should create agent");

        assert!(instance.id.starts_with("agent-"));
        assert_eq!(instance.template_name, "writer");
        assert_eq!(instance.status, "running");
        assert!(instance.created_ms > 0);

        let agents = factory.list_agents();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].id, instance.id);

        let profile = factory.profile();
        assert_eq!(profile.active_instances, 1);
        assert_eq!(profile.created_count, 1);
    }

    // ── test_create_agent_applies_overrides ──────────────────────────────────

    #[test]
    fn test_create_agent_applies_overrides() {
        let factory = AgentFactory::default();
        let mut tmpl = sample_template("tuner", "openai", &["config"]);
        tmpl.default_config = HashMap::from([
            ("temperature".to_string(), "0.7".to_string()),
            ("model".to_string(), "gpt-4".to_string()),
        ]);
        factory.register_template(tmpl).unwrap();

        let mut overrides = HashMap::new();
        overrides.insert("temperature".to_string(), "0.2".to_string());
        overrides.insert("top_p".to_string(), "0.9".to_string());

        let request = CreateAgentRequest {
            template_name: "tuner".to_string(),
            config_overrides: overrides,
            ttl_ms: None,
            tags: Vec::new(),
        };

        let instance = factory
            .create_agent(request)
            .expect("Should create agent with overrides");

        // Override should win for "temperature".
        assert_eq!(
            instance.config_overrides.get("temperature"),
            Some(&"0.2".to_string())
        );
        // Original default should still be present for "model".
        assert_eq!(
            instance.config_overrides.get("model"),
            Some(&"gpt-4".to_string())
        );
        // New key from overrides should be present.
        assert_eq!(
            instance.config_overrides.get("top_p"),
            Some(&"0.9".to_string())
        );
    }

    // ── test_destroy_agent ──────────────────────────────────────────────────

    #[test]
    fn test_destroy_agent() {
        let factory = AgentFactory::default();
        factory
            .register_template(sample_template("helper", "deepseek", &["tool"]))
            .unwrap();

        let instance = factory.create_agent(sample_request("helper")).unwrap();
        assert_eq!(factory.list_agents().len(), 1);

        factory.destroy_agent(&instance.id);
        assert!(factory.list_agents().is_empty());
        assert!(factory.get_agent(&instance.id).is_none());

        let profile = factory.profile();
        assert_eq!(profile.active_instances, 0);
        assert_eq!(profile.destroyed_count, 1);
    }

    // ── test_destroy_unknown_agent_is_noop ───────────────────────────────────

    #[test]
    fn test_destroy_unknown_agent_is_noop() {
        let factory = AgentFactory::default();
        factory
            .register_template(sample_template("noop", "mistral", &["test"]))
            .unwrap();

        // Create one instance.
        let _instance = factory.create_agent(sample_request("noop")).unwrap();
        let before = factory.profile();

        // Destroy a non-existent ID — should be a no-op.
        factory.destroy_agent("nonexistent-agent-id");

        let after = factory.profile();
        assert_eq!(after.active_instances, before.active_instances);
        assert_eq!(after.created_count, before.created_count);
        assert_eq!(after.destroyed_count, before.destroyed_count);
    }

    // ── test_list_agents ─────────────────────────────────────────────────────

    #[test]
    fn test_list_agents() {
        let factory = AgentFactory::default();
        factory
            .register_template(sample_template("alpha", "openai", &["a"]))
            .unwrap();
        factory
            .register_template(sample_template("beta", "anthropic", &["b"]))
            .unwrap();

        let a1 = factory.create_agent(sample_request("alpha")).unwrap();
        let a2 = factory.create_agent(sample_request("alpha")).unwrap();
        let b1 = factory.create_agent(sample_request("beta")).unwrap();

        let agents = factory.list_agents();
        assert_eq!(agents.len(), 3);

        let ids: Vec<String> = agents.iter().map(|a| a.id.clone()).collect();
        assert!(ids.contains(&a1.id));
        assert!(ids.contains(&a2.id));
        assert!(ids.contains(&b1.id));
    }

    // ── test_find_agents_by_capability ──────────────────────────────────────

    #[test]
    fn test_find_agents_by_capability() {
        let factory = AgentFactory::default();
        factory
            .register_template(sample_template("coder", "openai", &["code", "python"]))
            .unwrap();
        factory
            .register_template(sample_template("writer", "anthropic", &["text", "writing"]))
            .unwrap();
        factory
            .register_template(sample_template("debugger", "deepseek", &["code", "debug"]))
            .unwrap();

        let _c1 = factory.create_agent(sample_request("coder")).unwrap();
        let _c2 = factory.create_agent(sample_request("coder")).unwrap();
        let _w1 = factory.create_agent(sample_request("writer")).unwrap();
        let _d1 = factory.create_agent(sample_request("debugger")).unwrap();

        let code_agents = factory.find_agents_by_capability("code");
        assert_eq!(code_agents.len(), 3); // coder (2) + debugger (1)

        let text_agents = factory.find_agents_by_capability("text");
        assert_eq!(text_agents.len(), 1);

        let unknown_agents = factory.find_agents_by_capability("nonexistent");
        assert!(unknown_agents.is_empty());
    }

    // ── test_prune_expired ───────────────────────────────────────────────────

    #[test]
    fn test_prune_expired() {
        let factory = AgentFactory::default();
        factory
            .register_template(sample_template("ephemeral", "gemini", &["short"]))
            .unwrap();

        // Create an instance with an extremely short TTL so it expires immediately.
        let request = CreateAgentRequest {
            template_name: "ephemeral".to_string(),
            config_overrides: HashMap::new(),
            ttl_ms: Some(1), // 1 ms
            tags: Vec::new(),
        };

        let _instance = factory.create_agent(request).unwrap();
        assert_eq!(factory.list_agents().len(), 1);

        // Small sleep to let the expiry pass.
        std::thread::sleep(std::time::Duration::from_millis(10));

        let pruned = factory.prune_expired();
        assert_eq!(pruned, 1);
        assert!(factory.list_agents().is_empty());
    }

    // ── test_profile_reflects_state ──────────────────────────────────────────

    #[test]
    fn test_profile_reflects_state() {
        let factory = AgentFactory::default();

        factory
            .register_template(sample_template("svc-a", "openai", &["svc"]))
            .unwrap();
        factory
            .register_template(sample_template("svc-b", "openai", &["svc"]))
            .unwrap();
        factory
            .register_template(sample_template("svc-c", "anthropic", &["svc"]))
            .unwrap();

        let _a1 = factory.create_agent(sample_request("svc-a")).unwrap();
        let _a2 = factory.create_agent(sample_request("svc-a")).unwrap();
        let _b1 = factory.create_agent(sample_request("svc-b")).unwrap();
        let _c1 = factory.create_agent(sample_request("svc-c")).unwrap();

        // Destroy one instance.
        factory.destroy_agent(&_a1.id);

        let profile = factory.profile();
        assert_eq!(profile.total_templates, 3);
        assert_eq!(profile.active_instances, 3);
        assert_eq!(profile.created_count, 4);
        assert_eq!(profile.destroyed_count, 1);

        // Two templates are "openai", one is "anthropic".
        assert_eq!(profile.templates_by_category.get("openai"), Some(&2));
        assert_eq!(profile.templates_by_category.get("anthropic"), Some(&1));
    }

    // ── test_max_instances_enforced ──────────────────────────────────────────

    #[test]
    fn test_max_instances_enforced() {
        let config = AgentFactoryConfig {
            max_instances: 2,
            allow_custom_templates: true,
            default_ttl_ms: 86_400_000,
        };
        let factory = AgentFactory::new(config);

        factory
            .register_template(sample_template("limited", "openai", &["test"]))
            .unwrap();

        // First two creations should succeed.
        let _first = factory
            .create_agent(sample_request("limited"))
            .expect("First creation should succeed");
        let _second = factory
            .create_agent(sample_request("limited"))
            .expect("Second creation should succeed");

        // Third creation should fail.
        let result = factory.create_agent(sample_request("limited"));
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Maximum number of agent instances") || err_msg.contains("2"));

        // After destroying one, we should be able to create again.
        factory.destroy_agent(&_first.id);
        let _third = factory
            .create_agent(sample_request("limited"))
            .expect("Third creation should succeed after destroy");
    }

    // ── test_create_agent_unknown_template ───────────────────────────────────

    #[test]
    fn test_create_agent_unknown_template() {
        let factory = AgentFactory::default();
        let result = factory.create_agent(sample_request("does-not-exist"));
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Template 'does-not-exist' not found"));
    }

    // ── test_register_template_disabled ──────────────────────────────────────

    #[test]
    fn test_register_template_disabled() {
        let config = AgentFactoryConfig {
            max_instances: 10,
            allow_custom_templates: false,
            default_ttl_ms: 86_400_000,
        };
        let factory = AgentFactory::new(config);

        let tmpl = sample_template("blocked", "openai", &["blocked"]);
        let result = factory.register_template(tmpl);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("is disabled"));
    }

    // ── test_list_templates_empty ────────────────────────────────────────────

    #[test]
    fn test_list_templates_empty() {
        let factory = AgentFactory::default();
        assert!(factory.list_templates().is_empty());
    }

    // ── test_get_agent_returns_none_for_unknown ──────────────────────────────

    #[test]
    fn test_get_agent_returns_none_for_unknown() {
        let factory = AgentFactory::default();
        assert!(factory.get_agent("nonexistent").is_none());
    }

    // ── test_prune_expired_no_expired ────────────────────────────────────────

    #[test]
    fn test_prune_expired_no_expired() {
        let factory = AgentFactory::default();
        factory
            .register_template(sample_template("persistent", "openai", &["keep"]))
            .unwrap();

        let _inst = factory.create_agent(sample_request("persistent")).unwrap();
        assert_eq!(factory.list_agents().len(), 1);

        // No instances should be expired (long default TTL).
        let pruned = factory.prune_expired();
        assert_eq!(pruned, 0);
        assert_eq!(factory.list_agents().len(), 1);
    }

    // ── test_factory_default ─────────────────────────────────────────────────

    #[test]
    fn test_factory_default() {
        let factory = AgentFactory::default();
        assert_eq!(factory.config.max_instances, 50);
        assert!(factory.config.allow_custom_templates);
        assert_eq!(factory.config.default_ttl_ms, 86_400_000);
    }
}
