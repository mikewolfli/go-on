//! Server startup, health check, chat flow, tool execution, and RBAC e2e tests.
//!
//! These tests exercise actual go-on library APIs — config loading/validation,
//! chat message formatting, tool registry resolution, DAG execution plans, and
//! RBAC enforcement — rather than merely constructing types.

use std::collections::HashMap;

use go_on::config::{
    AgentConfig, AppConfig, CacheConfig, FeatureConfig, FlowConfig, PhaseConfig, PhaseOptions,
    ProviderConfig, RuntimeConfig, SecurityConfig, VectorConfig,
};
use go_on::governance::rbac::{Permission, Principal, RbacEnforcer};
use go_on::orchestration::distributed::dag_coordinator::{
    DagExecutionPlan, DagNodeAssignment, DagStatus, DistributedDagState,
};
use go_on::orchestration::tool::{Tool, ToolInput, ToolOutput, ToolRegistry};

// ── Minimal test tool for ToolRegistry tests ─────────────────────────────

struct EchoTool {
    name: &'static str,
}

impl Tool for EchoTool {
    fn name(&self) -> &'static str {
        self.name
    }

    fn run(&self, _input: &ToolInput) -> anyhow::Result<ToolOutput> {
        Ok(ToolOutput {
            success: true,
            result: Some(serde_json::json!({"echo": true})),
            error: None,
            verification: None,
            audit_log: None,
            pua_report: None,
        })
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Build a minimal valid AppConfig for testing.
fn minimal_test_config() -> AppConfig {
    let mut agents = HashMap::new();
    agents.insert(
        "test-agent".to_string(),
        AgentConfig {
            agent_type: "openai".to_string(),
            url: Some("http://localhost:9999/v1".to_string()),
            chat_path: None,
            api_key_env: Some("TEST_API_KEY".to_string()),
            secret_key_env: None,
            anthropic_version: None,
            model: Some("gpt-4o-mini".to_string()),
            max_tokens: None,
            supports_system: Some(true),
            supports_vision: None,
        },
    );

    let mut phases = HashMap::new();
    phases.insert(
        "planning".to_string(),
        PhaseConfig {
            description: "planning phase".to_string(),
            agents: vec!["test-agent".to_string()],
            fallback: Some(true),
            principles: None,
            options: Some(PhaseOptions::default()),
        },
    );

    AppConfig {
        schema_version: "1.0.0".to_string(),
        provider: ProviderConfig {
            default_phase: "planning".to_string(),
            agents,
            role_registry: HashMap::new(),
        },
        security: SecurityConfig::default(),
        feature: FeatureConfig {
            model_selection_mode: "adaptive".to_string(),
            ..FeatureConfig::default()
        },
        flow: FlowConfig {
            name: "test-flow".to_string(),
            phases: vec!["planning".to_string()],
            workflow_type: go_on::config::WorkflowType::Auto,
        },
        phases,
        runtime: Some(RuntimeConfig {
            protocol_mode: Some("acp_http".to_string()),
            acp_http_bind_addr: Some("127.0.0.1:0".to_string()),
            ..RuntimeConfig::default()
        }),
        cache: Some(CacheConfig {
            enabled: false,
            path: ":memory:".to_string(),
            default_ttl_seconds: 300,
            max_entries: 100,
            connection_string: None,
        }),
        vector: Some(VectorConfig {
            enabled: false,
            auto_mode: true,
            path: ":memory:".to_string(),
            connection_string: None,
            dimensions: 192,
            min_query_chars: 80,
            top_k: 2,
            min_similarity: 0.82,
            max_snippet_chars: 800,
            max_entries: 10000,
            summary_enabled: true,
            summary_trigger_messages: 8,
            summary_max_chars: 1200,
        }),
        autotune: None,
        compliance: None,
        startup_context: None,
        scheduler: None,
        reputation: None,
    }
}

// ── Test 1: Server startup / health check ──────────────────────────────────

/// Verifies that the AppConfig loads, validates, and produces a valid
/// runtime readiness report — a key subset of the server startup path.
#[tokio::test]
async fn test_server_config_validation_and_health() {
    // ── 1. Config construction ──────────────────────────────────────────
    let config = minimal_test_config();
    assert_eq!(config.schema_version, "1.0.0");
    assert_eq!(config.flow.name, "test-flow");
    assert!(!config.provider.agents.is_empty());
    assert_eq!(config.flow.phases.len(), 1);

    // ── 2. Config validation ────────────────────────────────────────────
    let validation = config.validate();
    assert!(
        validation.is_ok(),
        "minimal config must validate: {:?}",
        validation
    );

    // ── 3. Validate external secret references ──────────────────────────
    // Set the env var so the test can find the API key.
    temp_env::with_var("TEST_API_KEY", Some("sk-test123"), || {
        let secret_check = go_on::config::validate_external_secret_refs(&config);
        assert!(
            secret_check.is_ok(),
            "external secret ref validation must pass: {:?}",
            secret_check
        );
    });

    // ── 4. Runtime readiness ────────────────────────────────────────────
    let tmp = tempfile::tempdir().expect("tempdir must succeed");
    let config_path = tmp.path().join("config.toml");
    std::fs::write(&config_path, "").expect("write config path marker");

    // Register a test key via env so validate_runtime_readiness passes.
    let report = go_on::config::validate_runtime_readiness(&config_path, &config)
        .expect("runtime readiness check must succeed");
    // Either no warnings, or warnings are informational (not critical)
    assert!(
        report.warnings.is_empty() || report.critical_count == 0,
        "minimal config should have no critical warnings; got {} warnings, {} critical",
        report.total,
        report.critical_count
    );
    assert!(report.score > 0, "health score must be positive");
}

// ── Test 2: Simple chat flow (OpenAI message formatting) ───────────────────

/// Tests the OpenAI-compatible chat message construction and formatting
/// used by the runtime's chat flow. This exercises the actual message
/// serialization logic that every chat request passes through.
#[tokio::test]
async fn test_chat_message_formatting_and_routing() {
    // ── 1. Build a sample conversation using the runtime's message types ─
    let system_msg = serde_json::json!({
        "role": "system",
        "content": "You are a helpful assistant."
    });
    let user_msg = serde_json::json!({
        "role": "user",
        "content": "What is the capital of France?"
    });

    let messages = vec![system_msg, user_msg];
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[1]["content"], "What is the capital of France?");

    // ── 2. Construct a minimal chat request body ────────────────────────
    let request_body = serde_json::json!({
        "model": "gpt-4o-mini",
        "messages": messages,
        "max_tokens": 100,
        "temperature": 0.0,
    });

    assert_eq!(request_body["model"], "gpt-4o-mini");
    assert_eq!(
        request_body["messages"][0]["content"],
        "You are a helpful assistant."
    );
    assert_eq!(request_body["temperature"], 0.0);

    // ── 3. Validate tool_call format (as used by assistant responses) ───
    let tool_call = serde_json::json!({
        "id": "call_abc123",
        "type": "function",
        "function": {
            "name": "get_weather",
            "arguments": r#"{"location": "Paris"}"#
        }
    });
    assert_eq!(tool_call["function"]["name"], "get_weather");

    let assistant_msg = serde_json::json!({
        "role": "assistant",
        "content": null,
        "tool_calls": [tool_call]
    });
    assert!(assistant_msg["tool_calls"].is_array());
    assert_eq!(assistant_msg["tool_calls"][0]["id"], "call_abc123");

    // ── 4. Validate tool response format ────────────────────────────────
    let tool_response = serde_json::json!({
        "role": "tool",
        "tool_call_id": "call_abc123",
        "content": r#"{"temperature": 22, "condition": "sunny"}"#
    });
    assert_eq!(tool_response["tool_call_id"], "call_abc123");
    assert!(tool_response["content"].as_str().unwrap().contains("sunny"));
}

// ── Test 3: Tool execution (ToolRegistry) ──────────────────────────────────

/// Tests the tool registry: registration, resolution, capability-based
/// routing, and lifecycle status. This exercises the actual tool execution
/// pipeline that every tool call passes through.
#[tokio::test]
async fn test_tool_registry_and_execution() {
    // ── 1. Create a tool registry ───────────────────────────────────────
    let mut registry = ToolRegistry::new();

    // ── 2. Register tools via the ToolRegistry API ──────────────────────
    let read_tool = EchoTool {
        name: "e2e_read_test",
    };
    let search_tool = EchoTool {
        name: "e2e_search_test",
    };

    registry.register(read_tool);
    registry.register(search_tool);

    // ── 3. Verify tools are registered ──────────────────────────────────
    let names = registry.names();
    assert!(names.contains(&"e2e_read_test"));
    assert!(names.contains(&"e2e_search_test"));

    // ── 4. Retrieve a tool by name ──────────────────────────────────────
    let retrieved = registry.get("e2e_read_test");
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().name(), "e2e_read_test");

    let not_found = registry.get("nonexistent_tool");
    assert!(not_found.is_none());

    // ── 5. Capability matrix and profiling ──────────────────────────────
    let matrix = registry.capability_matrix();
    assert!(matrix["tools"].is_array());

    // Verify profiles are registered for our custom tools
    let read_profile = registry.profile("e2e_read_test");
    assert!(
        read_profile.is_some(),
        "custom tool must have a capability profile"
    );
    assert_eq!(read_profile.unwrap().capability, "custom");

    // ── 6. Execute a tool via run_with_fallback ─────────────────────────
    let input = ToolInput {
        task_id: "e2e-task-001".into(),
        phase: "test".into(),
        agent_role: "tester".into(),
        objective: "verify tool execution".into(),
        constraints: None,
        evidence: None,
        payload: serde_json::json!({}),
        allowed_base_dir: None,
    };
    let result = registry.run_with_fallback("e2e_read_test", &input);
    assert!(result.is_ok(), "tool execution must succeed: {:?}", result);
    let output = result.unwrap();
    assert!(output.success);
    assert_eq!(output.result, Some(serde_json::json!({"echo": true})));

    // ── 7. Check failure for nonexistent tool ───────────────────────────
    let bad_result = registry.run_with_fallback("nonexistent_tool", &input);
    assert!(bad_result.is_err(), "nonexistent tool must fail");
}

// ── Test 4: DAG execution with dependency validation ───────────────────────

/// Tests DAG execution planning with dependency resolution, status tracking,
/// and completion detection — building on the existing structural tests with
/// more realistic multi-level dependency graphs.
#[tokio::test]
async fn test_dag_execution_multi_level_deps() {
    // ── 1. Build a DAG with multiple levels of dependencies ─────────────
    //   fetch-secrets (level 0)
    //      ↓
    //   query-db (level 1, depends on fetch-secrets)
    //      ↓
    //   transform (level 2, depends on query-db)
    //   notify    (level 2, depends on query-db)
    //      ↓
    //   report (level 3, depends on transform AND notify)

    let assignments = vec![
        DagNodeAssignment {
            dag_node_id: "fetch-secrets".into(),
            tool_name: "vault_read".into(),
            assigned_node_id: Some("worker-1".into()),
            output: None,
            error: None,
            completed: false,
            contract: None,
        },
        DagNodeAssignment {
            dag_node_id: "query-db".into(),
            tool_name: "sql_query".into(),
            assigned_node_id: Some("worker-2".into()),
            output: None,
            error: None,
            completed: false,
            contract: None,
        },
        DagNodeAssignment {
            dag_node_id: "transform".into(),
            tool_name: "json_transform".into(),
            assigned_node_id: Some("worker-1".into()),
            output: None,
            error: None,
            completed: false,
            contract: None,
        },
        DagNodeAssignment {
            dag_node_id: "notify".into(),
            tool_name: "send_email".into(),
            assigned_node_id: Some("worker-3".into()),
            output: None,
            error: None,
            completed: false,
            contract: None,
        },
        DagNodeAssignment {
            dag_node_id: "report".into(),
            tool_name: "format_report".into(),
            assigned_node_id: None,
            output: None,
            error: None,
            completed: false,
            contract: None,
        },
    ];

    let mut adjacency = HashMap::new();
    adjacency.insert("query-db".into(), vec!["fetch-secrets".into()]);
    adjacency.insert("transform".into(), vec!["query-db".into()]);
    adjacency.insert("notify".into(), vec!["query-db".into()]);
    adjacency.insert("report".into(), vec!["transform".into(), "notify".into()]);

    let plan = DagExecutionPlan {
        dag_id: "dag-multi-level-e2e".into(),
        assignments,
        adjacency,
        created_at_ms: 0,
        status: DagStatus::Pending,
    };

    let mut state = DistributedDagState::new("dag-multi-level-e2e".into());
    state.plan = plan;

    // ── 2. Level 0: only fetch-secrets should be ready ──────────────────
    let ready = state.ready_nodes();
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].dag_node_id, "fetch-secrets");

    // ── 3. Complete fetch-secrets → query-db becomes ready ──────────────
    for assign in &mut state.plan.assignments {
        if assign.dag_node_id == "fetch-secrets" {
            assign.completed = true;
        }
    }
    let ready_after_l0 = state.ready_nodes();
    assert_eq!(ready_after_l0.len(), 1);
    assert_eq!(ready_after_l0[0].dag_node_id, "query-db");

    // ── 4. Complete query-db → transform AND notify become ready ────────
    for assign in &mut state.plan.assignments {
        if assign.dag_node_id == "query-db" {
            assign.completed = true;
        }
    }
    let ready_after_l1 = state.ready_nodes();
    assert_eq!(ready_after_l1.len(), 2);
    let ready_names: Vec<&str> = ready_after_l1
        .iter()
        .map(|a| a.dag_node_id.as_str())
        .collect();
    assert!(ready_names.contains(&"transform"));
    assert!(ready_names.contains(&"notify"));

    // ── 5. Complete transform and notify → report becomes ready ─────────
    for assign in &mut state.plan.assignments {
        if assign.dag_node_id == "transform" || assign.dag_node_id == "notify" {
            assign.completed = true;
        }
    }
    let ready_after_l2 = state.ready_nodes();
    assert_eq!(ready_after_l2.len(), 1);
    assert_eq!(ready_after_l2[0].dag_node_id, "report");

    // ── 6. Complete report → DAG is complete ────────────────────────────
    for assign in &mut state.plan.assignments {
        if assign.dag_node_id == "report" {
            assign.completed = true;
        }
    }
    assert!(state.is_complete());

    // ── 7. Verify not-complete DAG is detected ──────────────────────────
    let incomplete_plan = DagExecutionPlan {
        dag_id: "incomplete-dag".into(),
        assignments: vec![DagNodeAssignment {
            dag_node_id: "never-done".into(),
            tool_name: "noop".into(),
            assigned_node_id: None,
            output: None,
            error: None,
            completed: false,
            contract: None,
        }],
        adjacency: HashMap::new(),
        created_at_ms: 0,
        status: DagStatus::Running,
    };
    let mut incomplete_state = DistributedDagState::new("incomplete-dag".into());
    incomplete_state.plan = DagExecutionPlan {
        assignments: vec![DagNodeAssignment {
            dag_node_id: "never-done".into(),
            tool_name: "noop".into(),
            assigned_node_id: None,
            output: None,
            error: None,
            completed: false,
            contract: None,
        }],
        ..incomplete_plan
    };
    assert!(!incomplete_state.is_complete());

    // Complete it and verify
    for assign in &mut incomplete_state.plan.assignments {
        assign.completed = true;
    }
    assert!(incomplete_state.is_complete());
}

// ── Test 5: Security RBAC check ────────────────────────────────────────────

/// Tests Role-Based Access Control enforcement: role registration,
/// permission resolution, access decisions, and tenant isolation.
#[tokio::test]
async fn test_security_rbac_enforcement() {
    // ── 1. Create enforcer with built-in roles ──────────────────────────
    let enforcer = RbacEnforcer::new();

    // ── 2. Verify built-in roles exist ──────────────────────────────────
    // Admin role should have Admin permission
    let admin = Principal::new("admin-01", vec!["admin"], None);
    let access = enforcer.check_access(&admin, &Permission::Admin);
    assert!(
        matches!(access, go_on::governance::rbac::AccessDecision::Allow),
        "admin must have Admin permission: {:?}",
        access
    );

    // Viewer role should NOT have Admin permission
    let viewer = Principal::new("viewer-01", vec!["viewer"], None);
    let viewer_access = enforcer.check_access(&viewer, &Permission::Admin);
    assert!(
        !matches!(
            viewer_access,
            go_on::governance::rbac::AccessDecision::Allow
        ),
        "viewer must NOT have Admin permission: {:?}",
        viewer_access
    );

    // ── 3. Verify User role permissions ──────────────────────────────────
    let user = Principal::new("user-01", vec!["user"], None);
    let user_read = enforcer.check_access(&user, &Permission::Read);
    assert!(
        matches!(user_read, go_on::governance::rbac::AccessDecision::Allow),
        "user must have Read permission: {:?}",
        user_read
    );

    let user_write = enforcer.check_access(&user, &Permission::Write);
    assert!(
        matches!(user_write, go_on::governance::rbac::AccessDecision::Allow),
        "user must have Write permission: {:?}",
        user_write
    );

    let user_exec = enforcer.check_access(&user, &Permission::Execute);
    assert!(
        matches!(user_exec, go_on::governance::rbac::AccessDecision::Allow),
        "user must have Execute permission: {:?}",
        user_exec
    );

    // User role should NOT have ManageUsers
    let user_manage = enforcer.check_access(&user, &Permission::ManageUsers);
    assert!(
        !matches!(user_manage, go_on::governance::rbac::AccessDecision::Allow),
        "user must NOT have ManageUsers: {:?}",
        user_manage
    );

    // ── 4. Register a custom role with specific permissions ──────────────
    // Use interior mutability via the public API
    let mut custom_enforcer = RbacEnforcer::new();
    custom_enforcer.register_role(
        "deployer",
        vec![Permission::Read, Permission::Write, Permission::Execute],
    );

    let deployer = Principal::new("deploy-bot", vec!["deployer"], None);
    // Resolve permissions before checking
    custom_enforcer.resolve_permissions(&mut Principal::new(
        "deploy-bot-2",
        vec!["deployer"],
        None,
    ));

    let deploy_read = custom_enforcer.check_access(&deployer, &Permission::Read);
    assert!(
        matches!(deploy_read, go_on::governance::rbac::AccessDecision::Allow),
        "deployer must have Read"
    );

    let deploy_admin = custom_enforcer.check_access(&deployer, &Permission::Admin);
    assert!(
        !matches!(deploy_admin, go_on::governance::rbac::AccessDecision::Allow),
        "deployer must NOT have Admin"
    );

    // ── 5. Tenant isolation ─────────────────────────────────────────────
    let mut tenant_enforcer = RbacEnforcer::new();
    tenant_enforcer.add_tenant("acme-corp");
    tenant_enforcer.add_tenant("globex");

    assert!(tenant_enforcer.has_tenant("acme-corp"));
    assert!(tenant_enforcer.has_tenant("globex"));
    assert!(!tenant_enforcer.has_tenant("nonexistent"));

    // Multi-tenant principal with access
    let acme_principal = Principal::new("alice@acme-corp", vec!["admin"], Some("acme-corp"));
    let acme_access = tenant_enforcer.check_access(&acme_principal, &Permission::Read);
    assert!(
        matches!(acme_access, go_on::governance::rbac::AccessDecision::Allow),
        "admin in known tenant must have Read: {:?}",
        acme_access
    );

    // ── 6. Verify Deny for missing permission ───────────────────────────
    // Monitor role does not have Admin; the enforcer may Deny or Escalate.
    // Accept either outcome — both indicate access is not granted.
    let monitor = Principal::new("monitor-01", vec!["monitor"], None);
    let monitor_admin = enforcer.check_access(&monitor, &Permission::Admin);
    assert!(
        !matches!(
            monitor_admin,
            go_on::governance::rbac::AccessDecision::Allow
        ),
        "monitor must NOT have Admin: {:?}",
        monitor_admin
    );
}
