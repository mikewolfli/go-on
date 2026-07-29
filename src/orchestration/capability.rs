//! Unified capability scheduler — wraps Tool and Skill into a single trait.
//!
//! This module provides:
//! - [`CapabilityType`] — distinguishes Tool vs Skill
//! - [`Capability`] trait — unified interface for executable capabilities
//! - [`ToolCapabilityAdapter`] — wraps `Arc<dyn Tool>` as a capability
//! - [`SkillCapabilityAdapter`] — wraps `Arc<dyn Skill>` as a capability
//! - [`CapabilityScheduler`] — registry + dispatch for unified capabilities
//! - [`CapabilityInfo`] — descriptor returned by [`CapabilityScheduler::list`]
//!
//! # Status: Integration scaffold
//! These types are defined and tested but not yet wired into the production
//! tool dispatch path. Wiring will happen when FullAutoFlow or CapabilityBus
//! adopts the unified scheduler. Until then, dead_code is suppressed.
//! See: docs/blueprints/principle.md — "有完整目的，但部分实现，为接入"

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

use crate::orchestration::skill::{Skill, SkillRegistry};
use crate::orchestration::tool::{Tool, ToolInput, ToolRegistry};

// ---------------------------------------------------------------------------
// CapabilityType
// ---------------------------------------------------------------------------

/// Distinguishes the source of a capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(dead_code)] // Future unified Tool/Skill dispatch interface — wiring pending in FullAutoFlow
pub enum CapabilityType {
    /// A capability backed by a [`Tool`].
    Tool,
    /// A capability backed by a [`Skill`].
    Skill,
}

#[allow(dead_code)] // Part of CapabilityType — wiring pending
impl CapabilityType {
    /// Returns a human-readable label for this capability type.
    pub fn as_str(&self) -> &'static str {
        match self {
            CapabilityType::Tool => "tool",
            CapabilityType::Skill => "skill",
        }
    }
}

// ---------------------------------------------------------------------------
// Capability trait
// ---------------------------------------------------------------------------

/// Unified trait representing an executable capability.
///
/// Both tools and skills can be adapted to this interface, allowing
/// the [`CapabilityScheduler`] to dispatch execution uniformly.
#[async_trait]
#[allow(dead_code)] // Future unified Tool/Skill dispatch interface — wiring pending
pub trait Capability: Send + Sync {
    /// Returns the unique name of this capability.
    fn name(&self) -> &str;

    /// Returns a human-readable description of what this capability does.
    fn description(&self) -> &str;

    /// Returns the JSON Schema for this capability's input parameters.
    fn input_schema(&self) -> Value;

    /// Returns whether this is a Tool or Skill capability.
    fn capability_type(&self) -> CapabilityType;

    /// Execute the capability with the given input and return the output.
    async fn execute(&self, input: Value) -> Result<Value>;
}

// ---------------------------------------------------------------------------
// CapabilityInfo
// ---------------------------------------------------------------------------

/// A descriptor for a registered capability, returned by
/// [`CapabilityScheduler::list`].
#[derive(Debug, Clone)]
#[allow(dead_code)] // Returned by CapabilityScheduler::list — wiring pending
pub struct CapabilityInfo {
    /// The unique name of the capability.
    pub name: String,
    /// A human-readable description.
    pub description: String,
    /// The JSON Schema for the input parameters.
    pub input_schema: Value,
    /// Whether this is a Tool or Skill capability.
    pub capability_type: CapabilityType,
}

// ---------------------------------------------------------------------------
// ToolCapabilityAdapter
// ---------------------------------------------------------------------------

/// Adapter wrapping an `Arc<dyn Tool>` as a [`Capability`].
///
/// The adapter bridges the [`Tool`] trait interface to the [`Capability`]
/// trait, constructing a [`ToolInput`] envelope from the raw JSON input
/// and calling [`Tool::run_async`].
#[allow(dead_code)] // Adapter for CapabilityScheduler — wiring pending
pub struct ToolCapabilityAdapter {
    inner: Arc<dyn Tool>,
}

#[allow(dead_code)] // Adapter methods — wiring pending
impl ToolCapabilityAdapter {
    /// Create a new adapter wrapping the given tool.
    pub fn new(tool: Arc<dyn Tool>) -> Self {
        Self { inner: tool }
    }

    /// Return a reference to the inner tool.
    pub fn inner(&self) -> &Arc<dyn Tool> {
        &self.inner
    }

    /// Consume the adapter and return the inner tool.
    pub fn into_inner(self) -> Arc<dyn Tool> {
        self.inner
    }
}

#[async_trait]
impl Capability for ToolCapabilityAdapter {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> &str {
        self.inner.description()
    }

    fn input_schema(&self) -> Value {
        self.inner.input_schema()
    }

    fn capability_type(&self) -> CapabilityType {
        CapabilityType::Tool
    }

    async fn execute(&self, input: Value) -> Result<Value> {
        let tool_input = ToolInput {
            task_id: "capability-scheduler".to_string(),
            phase: "execution".to_string(),
            agent_role: "system".to_string(),
            objective: self.inner.description().to_string(),
            constraints: None,
            evidence: None,
            payload: input,
            allowed_base_dir: None,
        };
        let tool = self.inner.clone();
        let output = tool.run_async(tool_input).await?;
        Ok(json!({
            "success": output.success,
            "result": output.result,
            "error": output.error,
        }))
    }
}

// ---------------------------------------------------------------------------
// SkillCapabilityAdapter
// ---------------------------------------------------------------------------

/// Adapter wrapping an `Arc<dyn Skill>` as a [`Capability`].
///
/// The adapter bridges the [`Skill`] trait interface to the [`Capability`]
/// trait, delegating directly to [`Skill::execute`].
#[allow(dead_code)] // Adapter for CapabilityScheduler — wiring pending
pub struct SkillCapabilityAdapter {
    inner: Arc<dyn Skill>,
}

#[allow(dead_code)] // Adapter methods — wiring pending
impl SkillCapabilityAdapter {
    /// Create a new adapter wrapping the given skill.
    pub fn new(skill: Arc<dyn Skill>) -> Self {
        Self { inner: skill }
    }

    /// Return a reference to the inner skill.
    pub fn inner(&self) -> &Arc<dyn Skill> {
        &self.inner
    }

    /// Consume the adapter and return the inner skill.
    pub fn into_inner(self) -> Arc<dyn Skill> {
        self.inner
    }
}

#[async_trait]
impl Capability for SkillCapabilityAdapter {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> &str {
        self.inner.description()
    }

    fn input_schema(&self) -> Value {
        self.inner.input_schema()
    }

    fn capability_type(&self) -> CapabilityType {
        CapabilityType::Skill
    }

    async fn execute(&self, input: Value) -> Result<Value> {
        self.inner.execute(&input).await
    }
}

// ---------------------------------------------------------------------------
// CapabilityScheduler
// ---------------------------------------------------------------------------

/// A registry and scheduler for unified capabilities.
///
/// Wraps both [`Tool`] and [`Skill`] instances under a single [`Capability`]
/// interface, allowing callers to register, discover, and dispatch any
/// executable capability by name.
///
/// # Examples
///
/// ```ignore
/// use crate::orchestration::capability::*;
///
/// let mut scheduler = CapabilityScheduler::new();
///
/// // Register individual tools and skills
/// scheduler.register_tool(Arc::new(my_tool));
/// scheduler.register_skill(Arc::new(my_skill));
///
/// // Or bulk-import from existing registries
/// scheduler.register_all_tools(&tool_registry);
/// scheduler.register_all_skills(&skill_registry);
///
/// // Look up and execute
/// let info = scheduler.list();
/// let result = scheduler.execute("my_tool", json!({...})).await?;
/// ```
pub struct CapabilityScheduler {
    capabilities: HashMap<String, Arc<dyn Capability>>,
}

#[allow(dead_code)] // Scheduler methods — wiring pending in FullAutoFlow
impl CapabilityScheduler {
    /// Create a new empty scheduler.
    pub fn new() -> Self {
        Self {
            capabilities: HashMap::new(),
        }
    }

    /// Register a tool as a capability via its adapter.
    ///
    /// If a capability with the same name already exists, it is overwritten.
    pub fn register_tool(&mut self, tool: Arc<dyn Tool>) {
        let adapter = Arc::new(ToolCapabilityAdapter::new(tool));
        let name = adapter.name().to_string();
        self.capabilities.insert(name, adapter);
    }

    /// Register a skill as a capability via its adapter.
    ///
    /// If a capability with the same name already exists, it is overwritten.
    pub fn register_skill(&mut self, skill: Arc<dyn Skill>) {
        let adapter = Arc::new(SkillCapabilityAdapter::new(skill));
        let name = adapter.name().to_string();
        self.capabilities.insert(name, adapter);
    }

    /// Register all tools from a [`ToolRegistry`] as capabilities.
    ///
    /// Existing capabilities with the same names will be overwritten.
    pub fn register_all_tools(&mut self, registry: &ToolRegistry) {
        for name in registry.names() {
            if let Some(tool) = registry.get_arc(name) {
                self.register_tool(tool);
            }
        }
    }

    /// Register all skills from a [`SkillRegistry`] as capabilities.
    ///
    /// Existing capabilities with the same names will be overwritten.
    /// This includes hidden skills — the scheduler treats all capabilities
    /// uniformly.
    pub fn register_all_skills(&mut self, registry: &SkillRegistry) {
        for desc in registry.list(true) {
            if let Some(skill) = registry.get(&desc.name) {
                self.register_skill(skill);
            }
        }
    }

    /// Look up a capability by name.
    ///
    /// Returns `None` if no capability with the given name is registered.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Capability>> {
        self.capabilities.get(name).cloned()
    }

    /// Return a list of all registered capabilities, sorted by name.
    pub fn list(&self) -> Vec<CapabilityInfo> {
        let mut items: Vec<CapabilityInfo> = self
            .capabilities
            .values()
            .map(|cap| CapabilityInfo {
                name: cap.name().to_string(),
                description: cap.description().to_string(),
                input_schema: cap.input_schema(),
                capability_type: cap.capability_type(),
            })
            .collect();
        items.sort_by(|a, b| a.name.cmp(&b.name));
        items
    }

    /// Execute a capability by name with the given JSON input.
    ///
    /// Returns an error if the capability is not found or execution fails.
    pub async fn execute(&self, name: &str, input: Value) -> Result<Value> {
        let cap = self
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("capability '{}' not found", name))?;
        cap.execute(input).await
    }

    /// Returns the number of registered capabilities.
    pub fn len(&self) -> usize {
        self.capabilities.len()
    }

    /// Returns `true` if no capabilities are registered.
    pub fn is_empty(&self) -> bool {
        self.capabilities.is_empty()
    }

    /// Returns `true` if a capability with the given name is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.capabilities.contains_key(name)
    }
}

impl Default for CapabilityScheduler {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::skill::Skill as SkillTrait;
    use crate::orchestration::tool::{Tool as ToolTrait, ToolInput, ToolOutput};

    // ── Helpers ─────────────────────────────────────────────────────

    struct TestTool;

    impl ToolTrait for TestTool {
        fn name(&self) -> &'static str {
            "test_tool"
        }

        fn description(&self) -> &str {
            "A test tool"
        }

        fn run(&self, _input: &ToolInput) -> Result<ToolOutput> {
            Ok(ToolOutput {
                success: true,
                result: Some(json!({"message": "hello from tool"})),
                error: None,
                verification: None,
                audit_log: None,
                pua_report: None,
            })
        }
    }

    struct TestSkill;

    #[async_trait]
    impl SkillTrait for TestSkill {
        fn name(&self) -> &str {
            "test_skill"
        }

        fn description(&self) -> &str {
            "A test skill"
        }

        async fn execute(&self, input: &Value) -> Result<Value> {
            Ok(json!({
                "echo": input,
                "from": "skill",
            }))
        }
    }

    // ── Adapter Tests ──────────────────────────────────────────────

    #[tokio::test]
    async fn test_tool_capability_adapter() {
        let tool: Arc<dyn ToolTrait> = Arc::new(TestTool);
        let adapter = ToolCapabilityAdapter::new(tool);

        assert_eq!(adapter.name(), "test_tool");
        assert_eq!(adapter.description(), "A test tool");
        assert_eq!(adapter.capability_type(), CapabilityType::Tool);

        let result = adapter.execute(json!({"foo": "bar"})).await.unwrap();
        assert_eq!(result["success"], true);
        assert_eq!(result["result"]["message"], "hello from tool");
    }

    #[tokio::test]
    async fn test_skill_capability_adapter() {
        let skill: Arc<dyn SkillTrait> = Arc::new(TestSkill);
        let adapter = SkillCapabilityAdapter::new(skill);

        assert_eq!(adapter.name(), "test_skill");
        assert_eq!(adapter.description(), "A test skill");
        assert_eq!(adapter.capability_type(), CapabilityType::Skill);

        let result = adapter.execute(json!({"hello": "world"})).await.unwrap();
        assert_eq!(result["echo"]["hello"], "world");
        assert_eq!(result["from"], "skill");
    }

    // ── Scheduler Tests ────────────────────────────────────────────

    #[test]
    fn test_scheduler_empty() {
        let scheduler = CapabilityScheduler::new();
        assert!(scheduler.is_empty());
        assert_eq!(scheduler.len(), 0);
        assert!(!scheduler.contains("anything"));
    }

    #[test]
    fn test_scheduler_register_and_list() {
        let mut scheduler = CapabilityScheduler::new();

        let tool: Arc<dyn ToolTrait> = Arc::new(TestTool);
        let skill: Arc<dyn SkillTrait> = Arc::new(TestSkill);

        scheduler.register_tool(tool);
        scheduler.register_skill(skill);

        assert!(!scheduler.is_empty());
        assert_eq!(scheduler.len(), 2);

        let list = scheduler.list();
        assert_eq!(list.len(), 2);

        let tool_info = list.iter().find(|i| i.name == "test_tool").unwrap();
        assert_eq!(tool_info.capability_type, CapabilityType::Tool);

        let skill_info = list.iter().find(|i| i.name == "test_skill").unwrap();
        assert_eq!(skill_info.capability_type, CapabilityType::Skill);
    }

    #[tokio::test]
    async fn test_scheduler_execute() {
        let mut scheduler = CapabilityScheduler::new();

        let tool: Arc<dyn ToolTrait> = Arc::new(TestTool);
        let skill: Arc<dyn SkillTrait> = Arc::new(TestSkill);

        scheduler.register_tool(tool);
        scheduler.register_skill(skill);

        // Execute tool
        let result = scheduler
            .execute("test_tool", json!({"input": "data"}))
            .await
            .unwrap();
        assert_eq!(result["success"], true);

        // Execute skill
        let result = scheduler
            .execute("test_skill", json!({"key": "value"}))
            .await
            .unwrap();
        assert_eq!(result["echo"]["key"], "value");

        // Unknown capability
        let err = scheduler
            .execute("nonexistent", json!({}))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_scheduler_get_returns_cloneable_arc() {
        let mut scheduler = CapabilityScheduler::new();
        let tool: Arc<dyn ToolTrait> = Arc::new(TestTool);
        scheduler.register_tool(tool);

        let cap = scheduler.get("test_tool").expect("tool should exist");
        let result = cap.execute(json!({})).await.unwrap();
        assert_eq!(result["success"], true);

        // The Arc should be cloneable and still work
        let cap2 = cap.clone();
        let result2 = cap2.execute(json!({})).await.unwrap();
        assert_eq!(result2["success"], true);
    }

    #[tokio::test]
    async fn test_scheduler_register_overwrites_duplicate() {
        let mut scheduler = CapabilityScheduler::new();

        let skill: Arc<dyn SkillTrait> = Arc::new(TestSkill);
        scheduler.register_skill(skill);

        // Register a tool with the same name — should overwrite
        let tool: Arc<dyn ToolTrait> = Arc::new(TestTool);
        scheduler.register_tool(tool);

        let cap = scheduler.get("test_tool").unwrap();
        assert_eq!(cap.capability_type(), CapabilityType::Tool);
    }
}
