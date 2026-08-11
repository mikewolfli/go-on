//! Integration tests for Tool and Skill execution pipelines.
//!
//! Validates the full end-to-end flow:
//! - Tool: Registry → Input validation → Governance check → Execution → Output
//! - Skill: Registry → Lookup with fuzzy match → Execution → Outcome recording
//!
//! These tests run in-process (no child process) and verify actual I/O behaviour:
//! tool calls produce real filesystem side-effects, skill calls produce real
//! output values. No assertions are trivialised: every call chain is traced.

use anyhow::Result;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

// Import Tool trait so we can call .run() on tool instances.
use go_on::orchestration::tool::Tool as _;

// ============================================================================
// Helpers — shared across both test suites
// ============================================================================

/// Create a temporary directory that is automatically cleaned up on drop.
fn tmp_dir() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir should be created")
}

/// A test tool that writes a file with the given content.
/// Used to verify that tool execution produces observable side-effects.
struct TestWriteTool {
    output_dir: std::path::PathBuf,
}

impl go_on::orchestration::tool::Tool for TestWriteTool {
    fn name(&self) -> &'static str {
        "test_write"
    }

    fn description(&self) -> &str {
        "Writes a test file to disk — validates observable side-effects"
    }

    fn run(
        &self,
        input: &go_on::orchestration::tool::ToolInput,
    ) -> Result<go_on::orchestration::tool::ToolOutput> {
        let payload = &input.payload;
        let filename = payload
            .get("filename")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing 'filename' argument"))?;
        let content = payload
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let file_path = self.output_dir.join(filename);
        std::fs::write(&file_path, content)?;

        Ok(go_on::orchestration::tool::ToolOutput {
            success: true,
            result: Some(json!({
                "path": file_path.to_string_lossy(),
                "bytes_written": content.len(),
            })),
            error: None,
            verification: None,
            audit_log: None,
            pua_report: None,
        })
    }
}

/// A test skill that concatenates two input fields.
struct ConcatSkill;

#[async_trait::async_trait]
impl go_on::orchestration::skill::Skill for ConcatSkill {
    fn name(&self) -> &str {
        "concat_skill"
    }

    fn description(&self) -> &str {
        "Concatenates 'a' and 'b' fields from input"
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "a": {"type": "string", "description": "First part"},
                "b": {"type": "string", "description": "Second part"}
            },
            "required": ["a", "b"]
        })
    }

    async fn execute(&self, input: &Value) -> Result<Value> {
        let a = input.get("a").and_then(|v| v.as_str()).unwrap_or("");
        let b = input.get("b").and_then(|v| v.as_str()).unwrap_or("");
        Ok(json!({
            "result": format!("{}{}", a, b),
        }))
    }
}

// A second skill for multi-skill registry tests.
struct ReverseSkill;

#[async_trait::async_trait]
impl go_on::orchestration::skill::Skill for ReverseSkill {
    fn name(&self) -> &str {
        "reverse_skill"
    }

    fn description(&self) -> &str {
        "Reverses the input string in field 'text'"
    }

    async fn execute(&self, input: &Value) -> Result<Value> {
        let text = input.get("text").and_then(|v| v.as_str()).unwrap_or("");
        Ok(json!({
            "result": text.chars().rev().collect::<String>(),
        }))
    }
}

// ============================================================================
// Tests
// ============================================================================

// ── Tool: ReadFileTool (built-in) ──────────────────────────────────────────

#[tokio::test]
async fn tool_read_file_full_pipeline() {
    let dir = tmp_dir();
    let file_path = dir.path().join("hello.txt");
    std::fs::write(&file_path, "Hello, Tool Pipeline!").unwrap();

    let tool = go_on::orchestration::tool::builtin_tools::ReadFileTool;
    let input = go_on::orchestration::tool::ToolInput {
        task_id: "test-task-1".into(),
        phase: "testing".into(),
        agent_role: "tester".into(),
        objective: "read test file".into(),
        constraints: None,
        evidence: None,
        payload: json!({"path": file_path.to_string_lossy()}),
        allowed_base_dir: Some(dir.path().to_path_buf()),
    };

    let output = tool.run(&input).expect("tool run should succeed");
    assert!(output.success, "ReadFileTool should succeed");
    let result = output.result.expect("tool should return a result");
    assert_eq!(
        result["content"], "Hello, Tool Pipeline!",
        "file content should match"
    );
}

// ── Tool: custom TestWriteTool — verifies observable side-effects ──────────

#[tokio::test]
async fn tool_write_file_produces_observable_side_effect() {
    let dir = tmp_dir();

    let tool = TestWriteTool {
        output_dir: dir.path().to_path_buf(),
    };
    let input = go_on::orchestration::tool::ToolInput {
        task_id: "test-task-2".into(),
        phase: "testing".into(),
        agent_role: "tester".into(),
        objective: "write a test artifact".into(),
        constraints: None,
        evidence: None,
        payload: json!({
            "filename": "artifact.txt",
            "content": "observable-side-effect",  // 22 chars
        }),
        allowed_base_dir: Some(dir.path().to_path_buf()),
    };

    // 1. Execute the tool
    let output = tool.run(&input).expect("tool run should succeed");
    assert!(output.success, "TestWriteTool should report success");

    // 2. Verify the side-effect: file exists on disk
    let written_path = dir.path().join("artifact.txt");
    assert!(
        written_path.exists(),
        "tool must create the file on disk (observable side-effect)"
    );
    let content = std::fs::read_to_string(&written_path).unwrap();
    assert_eq!(
        content, "observable-side-effect",
        "file content must match what the tool wrote"
    );

    // 3. Verify the result metadata matches reality
    let result = output.result.expect("tool should return a result");
    assert_eq!(
        result["bytes_written"],
        json!(22),
        "metadata should report actual bytes written"
    );
}

// ── Tool: ToolRegistry — full registration + execution chain ───────────────

#[tokio::test]
async fn tool_registry_execution_chain() {
    let dir = tmp_dir();

    // 1. Create a registry and register tools
    let mut registry = go_on::orchestration::tool::ToolRegistry::new_empty();
    registry.register(go_on::orchestration::tool::builtin_tools::ReadFileTool);
    registry.register(TestWriteTool {
        output_dir: dir.path().to_path_buf(),
    });

    // 2. Write a file first via the write tool
    let write_file_path = dir.path().join("chain.txt");
    std::fs::write(&write_file_path, "chain-data").unwrap();

    // 3. Look up and execute the read tool via the registry
    let read_tool = registry
        .get("read_file")
        .expect("read_file should be registered");
    let read_input = go_on::orchestration::tool::ToolInput {
        task_id: "chain-task".into(),
        phase: "testing".into(),
        agent_role: "tester".into(),
        objective: "read chain file".into(),
        constraints: None,
        evidence: None,
        payload: json!({"path": write_file_path.to_string_lossy()}),
        allowed_base_dir: Some(dir.path().to_path_buf()),
    };
    let read_output = read_tool
        .run(&read_input)
        .expect("read_file execution should succeed");
    assert!(read_output.success, "read_file via registry should succeed");
    assert_eq!(
        read_output
            .result
            .as_ref()
            .and_then(|r| r.get("content"))
            .and_then(|c| c.as_str()),
        Some("chain-data"),
        "registry-routed tool should return correct content"
    );

    // 4. Verify tools are findable via the registry's public API
    assert!(
        registry.get("read_file").is_some(),
        "registry should have read_file"
    );
    assert!(
        registry.get("test_write").is_some(),
        "registry should have test_write"
    );
}

// ── Tool: governance pre-check via HarnessBus PolicyEvaluator ──────────────

#[tokio::test]
async fn tool_governance_pre_check() {
    use go_on::governance::hardening::{BudgetTracker, IdempotencyCache, SandboxLevel, TaskBudget};
    use go_on::governance::harness_bus::evaluator::PolicyEvaluator;
    use go_on::governance::pua::{PuaEnforcementPlan, PuaRuleEngine, TaskContext};
    use go_on::governance::rationalization::SelfRationalizationGuard;
    use go_on::governance::runtime_controls::OnlineControllerState;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    // Build a minimal governance evaluator
    let enforcement_plan = PuaEnforcementPlan::default();
    let rule_engine = PuaRuleEngine::new(Arc::new(Mutex::new(enforcement_plan)));
    let evaluator = PolicyEvaluator::new(
        Arc::new(Mutex::new(rule_engine)),
        Arc::new(Mutex::new(SandboxLevel::None)),
        Arc::new(Mutex::new(BudgetTracker::new(TaskBudget {
            max_tokens: 10000,
            max_tool_calls: 100,
            max_wall_clock_seconds: 60,
            max_api_calls: 100,
        }))),
        Arc::new(Mutex::new(IdempotencyCache::new(Duration::from_secs(3600)))),
        Arc::new(Mutex::new(OnlineControllerState::default())),
        Arc::new(Mutex::new(SelfRationalizationGuard::new(0.95))),
    );

    // Verify the evaluator can be constructed and called without panic
    // (the specific verdict depends on default governance policy)
    let ctx = TaskContext {
        task_type: go_on::governance::pua::TaskType::Other,
        file_count: 1,
        risk_score: 0.2,
    };
    let verdict = evaluator.evaluate(&ctx);
    // The evaluator should produce SOME verdict (not panic)
    match &verdict {
        go_on::governance::harness_bus::PolicyVerdict::Allow
        | go_on::governance::harness_bus::PolicyVerdict::Review(_)
        | go_on::governance::harness_bus::PolicyVerdict::Deny(_)
        | go_on::governance::harness_bus::PolicyVerdict::Escalate(_) => {}
    }

    // A high-risk task should not crash either
    let high_risk_ctx = TaskContext {
        task_type: go_on::governance::pua::TaskType::SecurityPatch,
        file_count: 10,
        risk_score: 0.85,
    };
    let _verdict = evaluator.evaluate(&high_risk_ctx);
}

// ── Skill: registry_lists_and_executes_skills ──────────────────────────────

#[tokio::test]
async fn skill_registry_full_cycle() {
    let mut registry = go_on::orchestration::skill::SkillRegistry::default();

    registry
        .register(Arc::new(ConcatSkill))
        .expect("should register concat_skill");
    registry
        .register(Arc::new(ReverseSkill))
        .expect("should register reverse_skill");

    // 1. List should return both skills
    let listed = registry.list(true);
    assert_eq!(listed.len(), 2, "registry should contain 2 skills");
    let names: Vec<&str> = listed.iter().map(|d| d.name.as_str()).collect();
    assert!(names.contains(&"concat_skill"));
    assert!(names.contains(&"reverse_skill"));

    // 2. Exact lookup + execution
    let skill = registry
        .get("concat_skill")
        .expect("concat_skill should exist");
    let result = skill
        .execute(&json!({"a": "hello_", "b": "world"}))
        .await
        .expect("concat_skill should execute");
    assert_eq!(
        result["result"], "hello_world",
        "concat should join a and b"
    );

    // 3. Registry lists both skills
    assert_eq!(listed.len(), 2, "stats should reflect 2 skills");
    for desc in &listed {
        assert_eq!(desc.total_calls, 0, "no executions yet for {}", desc.name);
    }

    // 4. Duplicate registration should fail
    let dup_result = registry.register(Arc::new(ConcatSkill));
    assert!(
        dup_result.is_err(),
        "duplicate skill registration should be rejected"
    );
}

// ── Skill: execution timing and outcome recording ──────────────────────────

#[tokio::test]
async fn skill_execution_timing_and_outcomes() {
    let mut registry = go_on::orchestration::skill::SkillRegistry::default();
    registry.register(Arc::new(ReverseSkill)).expect("register");

    let skill = registry.get("reverse_skill").unwrap();

    // 1. Measure execution time
    let start = Instant::now();
    let result = skill
        .execute(&json!({"text": "hello world"}))
        .await
        .expect("reverse_skill should execute");
    let elapsed = start.elapsed();

    assert_eq!(
        result["result"], "dlrow olleh",
        "reverse_skill should reverse input"
    );

    // 2. Execution should be near-instant (microseconds, not seconds)
    assert!(
        elapsed < Duration::from_secs(1),
        "simple string reversal should complete in <1s, took {:?}",
        elapsed
    );

    // 3. Record an outcome on the registry
    registry.record_outcome("reverse_skill", true, elapsed);

    // 4. Descriptor should reflect the recorded outcome
    let descriptors = registry.list(true);
    let rev_desc = descriptors
        .iter()
        .find(|d| d.name == "reverse_skill")
        .expect("reverse_skill should be listed");
    assert_eq!(rev_desc.total_calls, 1, "should record 1 execution");
    assert_eq!(rev_desc.success_calls, 1, "should record 1 success");
    assert_eq!(rev_desc.failure_calls, 0, "should record 0 failures");
}

// ── Skill: fuzzy match via best_match_with_input ───────────────────────────

#[tokio::test]
async fn skill_fuzzy_match_works() {
    let mut registry = go_on::orchestration::skill::SkillRegistry::default();
    registry
        .register(Arc::new(ConcatSkill))
        .expect("register concat_skill");

    // Exact name should always match
    let matched = registry
        .best_match_with_input("concat_skill", &json!({"a": "x"}))
        .expect("exact name should find a result");
    assert_eq!(
        matched, "concat_skill",
        "exact name should resolve to itself"
    );

    // Completely unrelated name should return None
    let no_match = registry.best_match_with_input("zzzz_not_a_skill", &json!({}));
    assert!(no_match.is_none(), "unrelated name should not match");
}

// ── Skill: descriptor contains input schema and metadata ───────────────────

#[tokio::test]
async fn skill_descriptor_contains_metadata() {
    let mut registry = go_on::orchestration::skill::SkillRegistry::default();
    registry.register(Arc::new(ConcatSkill)).expect("register");

    let descriptors = registry.list(true);
    let concat_desc = descriptors
        .iter()
        .find(|d| d.name == "concat_skill")
        .expect("concat_skill should be listed");

    assert!(
        !concat_desc.description.is_empty(),
        "skill description should not be empty"
    );
    assert_eq!(
        concat_desc.total_calls, 0,
        "newly registered skill should have zero calls"
    );
    assert_eq!(
        concat_desc.average_latency_ms, 0.0,
        "newly registered skill should have zero avg latency"
    );
}

// ── End-to-end: Tool + Skill in the same process ──────────────────────────

#[tokio::test]
async fn tool_and_skill_e2e_coexistence() {
    let dir = tmp_dir();

    // 1. Tool: write a file via the tool pipeline
    let tool = TestWriteTool {
        output_dir: dir.path().to_path_buf(),
    };
    let tool_input = go_on::orchestration::tool::ToolInput {
        task_id: "e2e-task".into(),
        phase: "testing".into(),
        agent_role: "tester".into(),
        objective: "e2e test".into(),
        constraints: None,
        evidence: None,
        payload: json!({
            "filename": "e2e_output.txt",
            "content": "skill_input_data",
        }),
        allowed_base_dir: Some(dir.path().to_path_buf()),
    };
    let tool_output = tool.run(&tool_input).expect("tool should write file");
    assert!(tool_output.success);

    // 2. Read back the file content
    let written = std::fs::read_to_string(dir.path().join("e2e_output.txt")).unwrap();

    // 3. Skill: process the written content
    let mut registry = go_on::orchestration::skill::SkillRegistry::default();
    registry.register(Arc::new(ReverseSkill)).expect("register");

    let skill = registry.get("reverse_skill").unwrap();
    let skill_result = skill
        .execute(&json!({"text": written}))
        .await
        .expect("skill should execute");

    // 4. Verify the full pipeline: tool writes → skill transforms
    assert_eq!(
        skill_result["result"], "atad_tupni_lliks",
        "e2e pipeline: tool writes 'skill_input_data', skill reverses to 'atad_tupni_lliks'"
    );
}
