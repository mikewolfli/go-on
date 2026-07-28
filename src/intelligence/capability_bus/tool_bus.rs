//! ToolBus — Unified tool & skill sub-bus (BLUE38 §1, ARCH-13 multi-bus architecture)
//!
//! The ToolBus wraps the existing `ToolRegistry` and `SkillRegistry` into a
//! single sub-bus that the `CapabilityBus` can query for agent-aware tool
//! assignment, execution, and statistics.
//!
//! # Integration
//!
//! ```text
//!  CapabilityBus
//!      │
//!      ├── WorkflowLearningBus
//!      ├── KnowledgeBus
//!      ├── ReputationStore
//!      ├── CapabilityGraph
//!      ├── ...
//!      └── ToolBus  ←  this module
//!              │
//!              ├── ToolRegistry  (orchestration::tool)
//!              └── SkillRegistry (orchestration::skill)
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use anyhow::Result;

use crate::orchestration::skill::{Skill, SkillRegistry};
use crate::orchestration::tool::{ToolInput, ToolOutput, ToolRegistry, ToolRiskLevel};

// ---------------------------------------------------------------------------
// Descriptor – one item in the combined capability matrix
// ---------------------------------------------------------------------------

/// A unified descriptor for both tools and skills.
///
/// Returned by `capability_matrix()` so callers see a homogeneous list.
#[derive(Debug, Clone)]
pub struct ToolDescriptor {
    pub name: String,
    pub capability: String,
    pub risk_level: String,
    pub timeout_ms: u64,
    pub fallback_chain: Vec<String>,
    pub is_skill: bool,
}

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

/// Per-tool usage statistics tracked by the ToolBus.
#[derive(Debug, Clone, Default)]
pub struct ToolStats {
    pub total_calls: u64,
    pub success_calls: u64,
    pub failure_calls: u64,
    pub avg_duration_ms: f64,
}

impl ToolStats {
    fn record(&mut self, success: bool, duration_ms: u64) {
        self.total_calls += 1;
        if success {
            self.success_calls += 1;
        } else {
            self.failure_calls += 1;
        }
        // Exponential moving average to avoid unbounded accumulation.
        if self.total_calls == 1 {
            self.avg_duration_ms = duration_ms as f64;
        } else {
            self.avg_duration_ms +=
                (duration_ms as f64 - self.avg_duration_ms) / self.total_calls as f64;
        }
    }
}

// ---------------------------------------------------------------------------
// ToolBus profile
// ---------------------------------------------------------------------------

/// High-level health / status snapshot of the ToolBus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolBusProfile {
    pub enabled: bool,
    pub total_tools: u32,
    pub total_skills: u32,
    pub total_calls: u64,
    pub success_rate: f64,
}

// ---------------------------------------------------------------------------
// ToolBus
// ---------------------------------------------------------------------------

/// Mutable inner state protected by a single `Mutex`.
///
/// Kept separate so `ToolBus` methods can take `&self` (required by the
/// `CapabilityBus` interface) while still mutating statistics.
struct ToolBusInner {
    stats: HashMap<String, ToolStats>,
    total_calls: u64,
    total_success_calls: u64,
    enabled: bool,
}

/// Unified sub-bus that exposes tools and skills through a common interface.
///
/// The `CapabilityBus` holds one instance of `ToolBus` and delegates all
/// capability-lookup, agent-tool-matching, execution, and statistics-gathering
/// to it.
pub struct ToolBus {
    tool_registry: &'static ToolRegistry,
    skill_registry: Arc<RwLock<SkillRegistry>>,
    inner: Mutex<ToolBusInner>,
}

impl ToolBus {
    // -----------------------------------------------------------------------
    // Construction
    // -----------------------------------------------------------------------

    /// Create a new `ToolBus` wrapping the given registries.
    pub fn new(
        tool_registry: &'static ToolRegistry,
        skill_registry: Arc<RwLock<SkillRegistry>>,
    ) -> Self {
        let mut stats = HashMap::new();

        // Pre-populate stats entries for every known tool.
        for name in tool_registry.names() {
            stats.entry(name.to_string()).or_default();
        }
        // Pre-populate stats entries for every known skill.
        {
            let reg = match skill_registry.read() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    tracing::warn!("[B48] skill_registry lock poisoned, recovering");
                    poisoned.into_inner()
                }
            };
            for desc in reg.list(false) {
                stats.entry(desc.name.clone()).or_default();
            }
        }

        Self {
            tool_registry,
            skill_registry,
            inner: Mutex::new(ToolBusInner {
                stats,
                total_calls: 0,
                total_success_calls: 0,
                enabled: true,
            }),
        }
    }

    // -----------------------------------------------------------------------
    // Capability matrix
    // -----------------------------------------------------------------------

    /// Return a combined list of all tools and skills with their capability
    /// profiles.  Skills are always listed with a risk level of `"medium"` and
    /// an empty fallback chain because those concepts are not part of the
    /// `Skill` trait.
    pub fn capability_matrix(&self) -> Vec<ToolDescriptor> {
        let mut descriptors: Vec<ToolDescriptor> = Vec::new();

        // Tools
        for name in self.tool_registry.names() {
            let profile = self.tool_registry.profile(name);
            descriptors.push(ToolDescriptor {
                name: name.to_string(),
                capability: profile
                    .map(|p| p.capability.clone())
                    .unwrap_or_else(|| "unknown".to_string()),
                risk_level: profile
                    .map(|p| match p.risk_level {
                        ToolRiskLevel::Low => "low",
                        ToolRiskLevel::Medium => "medium",
                        ToolRiskLevel::High => "high",
                    })
                    .unwrap_or("medium")
                    .to_string(),
                timeout_ms: profile.map(|p| p.timeout_budget_ms).unwrap_or(30_000),
                fallback_chain: profile
                    .map(|p| p.fallback_chain.clone())
                    .unwrap_or_default(),
                is_skill: false,
            });
        }

        // Skills
        let reg = self.skill_registry.read().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        for desc in reg.list(false) {
            descriptors.push(ToolDescriptor {
                name: desc.name.clone(),
                capability: format!("skill:{}", desc.name),
                risk_level: "medium".to_string(),
                timeout_ms: 30_000,
                fallback_chain: Vec::new(),
                is_skill: true,
            });
        }

        descriptors
    }

    // -----------------------------------------------------------------------
    // Agent-tool matching
    // -----------------------------------------------------------------------

    /// Return names of tools (and skills) that are appropriate for the given
    /// `agent_role` and `task_type`.
    ///
    /// The matching heuristic is deliberately simple:
    ///
    /// * **Tools** – the tool's `capability` field is compared against both
    ///   `agent_role` and `task_type` via substring / prefix matching.  When the
    ///   agent is `"coder"` the tool `"filesystem_write"` is considered a match
    ///   because `"filesystem"` overlaps with common coding tasks.
    /// * **Skills** – the skill's name and description are matched against
    ///   `task_type` using the same `SkillRegistry::best_match_with_input`
    ///   semantics.
    ///
    /// This method will be refined as the RL feedback loop matures.
    pub fn agent_tool_match(&self, agent_role: &str, task_type: &str) -> Vec<String> {
        let mut matches: Vec<String> = Vec::new();

        let role_lower = agent_role.to_lowercase();
        let task_lower = task_type.to_lowercase();

        // Match tools by capability field.
        let reg = self.tool_registry;
        for name in reg.names() {
            let profile = reg.profile(name);
            if let Some(prof) = profile {
                let cap_lower = prof.capability.to_lowercase();
                // A tool matches if its capability overlaps with the agent
                // role or the task type.
                if cap_lower.contains(&role_lower)
                    || role_lower.contains(&cap_lower)
                    || cap_lower.contains(&task_lower)
                    || task_lower.contains(&cap_lower)
                {
                    matches.push(name.to_string());
                }
            }
        }

        // Match skills via the skill-registry's best-match logic.
        let reg = self.skill_registry.read().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        if let Some(best) = reg.best_match_with_input(
            task_type,
            &serde_json::json!({"task": task_type, "objective": task_type}),
        ) {
            if !matches.contains(&best) {
                matches.push(best);
            }
        }

        matches
    }

    // -----------------------------------------------------------------------
    // Execution with HarnessBus-compatible validation
    // -----------------------------------------------------------------------

    /// Execute a tool by name with HarnessBus-compatible validation.
    ///
    /// Returns an error if the tool name is unknown or execution fails.
    pub async fn execute_tool(&self, tool_name: &str, input: &ToolInput) -> Result<ToolOutput> {
        // ── Lifetime / start ──────────────────────────────────────────
        let start = std::time::Instant::now();

        let result = self.dispatch_tool(tool_name, input).await;

        // ── Record statistics ────────────────────────────────────────
        let duration_ms = start.elapsed().as_millis() as u64;
        let success = result
            .as_ref()
            .map(|output| output.success)
            .unwrap_or(false);
        // Use an immutable snapshot of the name for the record call.
        // (record_tool_call needs &str, not the moved tool_name if we
        //  consumed it — but we keep a copy.)
        self.record_tool_call(tool_name, success, duration_ms);

        result
    }

    /// Look up a skill by name, returning an owned Arc so the caller can
    /// drop the registry lock before .await.
    fn lookup_skill(&self, name: &str) -> Option<Arc<dyn Skill>> {
        match self.skill_registry.read() {
            Ok(guard) => {
                // Clone is required to drop MutexGuard before .await in caller.
                #[allow(clippy::map_clone)]
                let skill = guard.get(name).map(|s| s.clone());
                skill
            }
            Err(poisoned) => {
                tracing::warn!("lock poisoned, recovering");
                let guard = poisoned.into_inner();
                // Clone is required to drop MutexGuard before .await in caller.
                #[allow(clippy::map_clone)]
                let skill = guard.get(name).map(|s| s.clone());
                skill
            }
        }
    }

    /// Inner dispatch — separate from the stats-recording wrapper.
    async fn dispatch_tool(&self, tool_name: &str, input: &ToolInput) -> Result<ToolOutput> {
        // Check if it is a built-in tool.
        let has_tool = self.tool_registry.get(tool_name).is_some();
        if has_tool {
            // Offload to blocking pool to avoid blocking the tokio worker thread.
            let reg: &'static ToolRegistry = self.tool_registry;
            let input = input.clone();
            let tool_name = tool_name.to_string();
            return tokio::task::spawn_blocking(move || reg.run_with_fallback(&tool_name, &input))
                .await?;
        }

        // Check if it is a registered skill.
        // Lock scope: dropped before any await point.
        let skill_name = tool_name.to_string();
        let skill_input = serde_json::json!({
            "task_id": input.task_id,
            "phase": input.phase,
            "agent_role": input.agent_role,
            "objective": input.objective,
            "constraints": input.constraints,
            "evidence": input.evidence,
            "payload": input.payload,
        });

        // Lookup skill while holding the lock, then drop the lock before .await.
        let skill = self.lookup_skill(&skill_name);
        if let Some(skill) = skill {
            let output_value = skill.execute(&skill_input).await?;

            return Ok(ToolOutput {
                success: true,
                result: Some(output_value),
                error: None,
                verification: None,
                audit_log: Some(format!("executed skill '{}'", tool_name)),
                pua_report: None,
            });
        }

        anyhow::bail!("ToolBus: tool or skill '{}' not found", tool_name)
    }

    // -----------------------------------------------------------------------
    // Statistics
    // -----------------------------------------------------------------------

    /// Return a snapshot of per-tool usage statistics.
    pub fn tool_stats(&self) -> HashMap<String, ToolStats> {
        self.inner
            .lock()
            .ok()
            .map(|inner| inner.stats.clone())
            .unwrap_or_default()
    }

    /// Record a tool call outcome for statistics tracking.
    pub fn record_tool_call(&self, tool_name: &str, success: bool, duration_ms: u64) {
        let mut inner = self.inner.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });

        let entry = inner.stats.entry(tool_name.to_string()).or_default();
        entry.record(success, duration_ms);

        inner.total_calls += 1;
        if success {
            inner.total_success_calls += 1;
        }
    }

    // -----------------------------------------------------------------------
    // Profile
    // -----------------------------------------------------------------------

    /// Access the inner SkillRegistry for profiling / evolution tracking.
    pub fn skill_registry_ref(&self) -> &Arc<RwLock<SkillRegistry>> {
        &self.skill_registry
    }

    /// Produce a high-level profile snapshot of the ToolBus.
    pub fn profile(&self) -> ToolBusProfile {
        let total_tools = self.tool_registry.names().len() as u32;

        let total_skills = self
            .skill_registry
            .read()
            .map(|reg| reg.list(false).len() as u32)
            .unwrap_or(0);

        let inner = self.inner.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("lock poisoned, recovering");
            poisoned.into_inner()
        });
        let total_calls = inner.total_calls;
        let total_success_calls = inner.total_success_calls;
        let enabled = inner.enabled;

        let success_rate = if total_calls == 0 {
            1.0
        } else {
            total_success_calls as f64 / total_calls as f64
        };

        ToolBusProfile {
            enabled,
            total_tools,
            total_skills,
            total_calls,
            success_rate,
        }
    }
}

// ---------------------------------------------------------------------------
// Feature-gated remote-skill import (multi-users-server only)
// ---------------------------------------------------------------------------

/// Register a remote skill from a remote MCP endpoint.
///
/// Only available under the `multi-users-server` feature flag.
#[cfg(feature = "multi-users-server")]
pub fn import_remote_skill(tool_bus: &ToolBus, endpoint: &str, skill_name: &str) -> Result<()> {
    use crate::orchestration::skill_import::RemoteSkill;

    let remote = RemoteSkill::new(endpoint, skill_name, None, None)?;

    let skill: Arc<dyn crate::orchestration::skill::Skill> = Arc::new(remote);
    tool_bus
        .skill_registry
        .write()
        .map_err(|e| anyhow::anyhow!("SkillRegistry lock poisoned: {}", e))?
        .register(skill)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestration::skill::EchoSkill;
    use crate::orchestration::tool::Tool;

    struct LogicalFailureTool;

    impl Tool for LogicalFailureTool {
        fn name(&self) -> &'static str {
            "logical_failure"
        }

        fn run(&self, _input: &ToolInput) -> Result<ToolOutput> {
            Ok(ToolOutput {
                success: false,
                result: None,
                error: Some("simulated logical failure".to_string()),
                verification: None,
                audit_log: None,
                pua_report: None,
            })
        }
    }

    fn make_bus_with_registry(reg: &'static ToolRegistry) -> ToolBus {
        let skill_registry = Arc::new(RwLock::new(SkillRegistry::default()));
        ToolBus::new(reg, skill_registry)
    }

    fn make_bus_with_skill(reg: &'static ToolRegistry) -> ToolBus {
        let skill_registry = Arc::new(RwLock::new(SkillRegistry::default()));
        // Register the builtin echo skill for testing.
        {
            let mut skill_guard = skill_registry.write().unwrap_or_else(|poisoned| {
                tracing::warn!("lock poisoned, recovering");
                poisoned.into_inner()
            });
            let _ = skill_guard.register(Arc::new(EchoSkill));
        }
        ToolBus::new(reg, skill_registry)
    }

    fn make_bus() -> ToolBus {
        let tool_registry = ToolRegistry::new();
        let tool_registry: &'static ToolRegistry = Box::leak(Box::new(tool_registry));
        make_bus_with_skill(tool_registry)
    }

    #[test]
    fn capability_matrix_includes_tools_and_skills() {
        let bus = make_bus();
        let matrix = bus.capability_matrix();

        // At least the 6 built-in tools.
        assert!(
            matrix.len() >= 6,
            "expected at least 6 tools, got {}",
            matrix.len()
        );

        let tool_names: Vec<&str> = matrix.iter().map(|d| d.name.as_str()).collect();
        assert!(tool_names.contains(&"read_file"), "read_file missing");
        assert!(tool_names.contains(&"write_file"), "write_file missing");
        assert!(tool_names.contains(&"search_files"), "search_files missing");
        assert!(tool_names.contains(&"apply_patch"), "apply_patch missing");
        assert!(tool_names.contains(&"run_tests"), "run_tests missing");
        assert!(
            tool_names.contains(&"inspect_git_diff"),
            "inspect_git_diff missing"
        );

        // Also includes the echo skill.
        assert!(tool_names.contains(&"builtin.echo"), "builtin.echo missing");

        // Non-skills are marked correctly.
        for desc in &matrix {
            if desc.name == "read_file" {
                assert!(!desc.is_skill, "read_file should not be a skill");
                assert_eq!(desc.risk_level, "low");
            }
        }
    }

    #[test]
    fn agent_tool_match_returns_relevant_tools() {
        let bus = make_bus();
        let matched = bus.agent_tool_match("coder", "filesystem_read");

        // At minimum "read_file" and "search_files" should match.
        assert!(
            matched.contains(&"read_file".to_string()),
            "expected read_file in matches for coder/filesystem_read, got {:?}",
            matched
        );
    }

    #[tokio::test]
    async fn execute_known_tool_succeeds() {
        let bus = make_bus();
        let input = ToolInput {
            task_id: "test-001".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "read a file".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({"path": "Cargo.toml"}),
            allowed_base_dir: None,
        };

        // read_file should succeed (the file exists in the workspace).
        let result = bus.execute_tool("read_file", &input).await;
        assert!(result.is_ok(), "read_file failed: {:?}", result.err());

        let output = result.expect("expected read_file to succeed");
        assert!(output.success, "read_file returned success=false");

        // Statistics should have been recorded.
        let stats = bus.tool_stats();
        let read_file_stats = stats.get("read_file");
        assert!(read_file_stats.is_some(), "no stats for read_file");
        assert_eq!(
            read_file_stats
                .expect("expected stats for read_file")
                .total_calls,
            1
        );
        assert_eq!(
            read_file_stats
                .expect("expected stats for read_file")
                .success_calls,
            1
        );
    }

    #[tokio::test]
    async fn execute_unknown_tool_returns_error() {
        let bus = make_bus();
        let input = ToolInput {
            task_id: "test-002".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "do something".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({}),
            allowed_base_dir: None,
        };

        let result = bus.execute_tool("nonexistent_tool_xyz", &input).await;
        assert!(result.is_err(), "expected error for unknown tool");
        assert!(
            result.unwrap_err().to_string().contains("not found"),
            "error should mention 'not found'"
        );
    }

    #[tokio::test]
    async fn execute_tool_ok_but_logical_failure_tracks_failure_stats() {
        let mut reg = ToolRegistry::new();
        reg.register(LogicalFailureTool);
        let reg: &'static ToolRegistry = Box::leak(Box::new(reg));
        let bus = make_bus_with_registry(reg);

        let input = ToolInput {
            task_id: "test-003".to_string(),
            phase: "act".to_string(),
            agent_role: "coder".to_string(),
            objective: "trigger logical failure".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({}),
            allowed_base_dir: None,
        };

        let output = bus
            .execute_tool("logical_failure", &input)
            .await
            .expect("tool should return logical failure output");
        assert!(
            !output.success,
            "logical failure output should be unsuccessful"
        );

        let stats = bus.tool_stats();
        let lf_stats = stats
            .get("logical_failure")
            .expect("expected stats for logical_failure");
        assert_eq!(lf_stats.total_calls, 1);
        assert_eq!(lf_stats.success_calls, 0);
        assert_eq!(lf_stats.failure_calls, 1);
    }

    #[test]
    fn tool_stats_tracks_success_and_failure() {
        let bus = make_bus();

        // Record some calls manually.
        bus.record_tool_call("read_file", true, 12);
        bus.record_tool_call("read_file", true, 8);
        bus.record_tool_call("read_file", false, 30);

        let stats = bus.tool_stats();
        let rf_stats = stats
            .get("read_file")
            .expect("expected stats for read_file");

        assert_eq!(rf_stats.total_calls, 3);
        assert_eq!(rf_stats.success_calls, 2);
        assert_eq!(rf_stats.failure_calls, 1);
        assert!(
            (rf_stats.avg_duration_ms - ((12.0 + 8.0 + 30.0) / 3.0)).abs() < 0.001,
            "avg_duration_ms expected ~16.67, got {}",
            rf_stats.avg_duration_ms
        );
    }

    #[test]
    fn profile_reflects_state() {
        let bus = make_bus();

        let prof = bus.profile();
        assert!(prof.enabled);
        assert!(prof.total_tools >= 6);
        assert!(prof.total_skills >= 1);
        assert_eq!(prof.total_calls, 0);
        assert!((prof.success_rate - 1.0).abs() < 0.001);
    }

    #[tokio::test]
    async fn execute_skill_echo_roundtrips() {
        let bus = make_bus();
        let input = ToolInput {
            task_id: "skill-test".to_string(),
            phase: "act".to_string(),
            agent_role: "tester".to_string(),
            objective: "echo test".to_string(),
            constraints: None,
            evidence: None,
            payload: serde_json::json!({"hello": "world"}),
            allowed_base_dir: None,
        };

        let result = bus.execute_tool("builtin.echo", &input).await;
        assert!(result.is_ok(), "echo skill failed: {:?}", result.err());

        let output = result.expect("expected echo skill to succeed");
        assert!(output.success);
        assert_eq!(
            output.result,
            Some(serde_json::json!({
                "task_id": "skill-test",
                "phase": "act",
                "agent_role": "tester",
                "objective": "echo test",
                "constraints": serde_json::Value::Null,
                "evidence": serde_json::Value::Null,
                "payload": {"hello": "world"}
            }))
        );
    }
}
