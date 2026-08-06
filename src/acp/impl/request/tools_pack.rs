use std::sync::{Arc, Mutex, OnceLock};
use tracing::Instrument;

use super::config_handlers::build_trace_payload;
use super::prompts_pack::{build_prompts_get_tool, build_prompts_list_tool};
use super::*;
use crate::acp::helpers::tool_governance::{
    record_tool_allowed, record_tool_budget_denied, record_tool_harness_sandbox_denied,
    record_tool_rbac_denied,
};
use crate::governance::hardening::{task_budget_for_target, BudgetTracker, GovernanceAction};
use crate::orchestration::skill::discovery_cache::SkillDiscovery;
use crate::orchestration::skill::Skill as _;
use crate::orchestration::skill_import::{SkillImportPolicy, SkillImportRequest, SkillImportStore};

/// Global `SkillDiscovery` engine, lazily initialized on first `skill-finder` call.
static SKILL_DISCOVERY: OnceLock<Mutex<SkillDiscovery>> = OnceLock::new();

static GLOBAL_TOOL_REGISTRY: OnceLock<Arc<ToolRegistry>> = OnceLock::new();

/// Get or create the global `ToolRegistry` instance (lazily initialized once).
///
/// Returns `&'static Arc` so callers can cheaply `Arc::clone` the shared
/// registry (e.g. for the per-server field) instead of building a second
/// full registration of every built-in tool.
pub(crate) fn global_tool_registry() -> &'static Arc<ToolRegistry> {
    GLOBAL_TOOL_REGISTRY.get_or_init(|| Arc::new(ToolRegistry::new()))
}

/// Get or create the global `SkillDiscovery` instance.
pub(crate) fn skill_discovery() -> &'static Mutex<SkillDiscovery> {
    SKILL_DISCOVERY.get_or_init(|| Mutex::new(SkillDiscovery::new()))
}

pub(super) fn skill_import_policy(server: &AcpServer) -> SkillImportPolicy {
    SkillImportPolicy::from_runtime(&server.runtime_config)
}

pub(super) fn open_skill_import_store(server: &AcpServer) -> Result<SkillImportStore> {
    SkillImportStore::load(
        skill_import_policy(server),
        server.orchestration_deps.skill_registry.clone(),
    )
}

pub(crate) fn build_mcp_tool_descriptors(server: Option<&AcpServer>) -> Vec<Value> {
    let mut tools = vec![
        json!({
            "name": "acp_trace_get",
            "description": "Get ACP trace events",
            "input_schema": {"type": "object"}
        }),
        json!({
            "name": "acp_debug_panel_get",
            "description": "Get ACP debug panel snapshot",
            "input_schema": {"type": "object"}
        }),
        json!({
            "name": "goon_workflow_run_list",
            "description": "List workflow runs with pagination and status filter",
            "input_schema": {"type": "object"}
        }),
        json!({
            "name": "goon_workflow_run_get",
            "description": "Get workflow run details by run_id",
            "input_schema": {"type": "object", "required": ["run_id"]}
        }),
        json!({
            "name": "goon_workflow_run_cancel",
            "description": "Cancel workflow run by run_id",
            "input_schema": {"type": "object", "required": ["run_id"]}
        }),
        json!({
            "name": "goon_workflow_run_pause",
            "description": "Pause workflow run by run_id",
            "input_schema": {"type": "object", "required": ["run_id"]}
        }),
        json!({
            "name": "goon_workflow_run_resume",
            "description": "Resume workflow run by run_id",
            "input_schema": {"type": "object", "required": ["run_id"]}
        }),
        json!({
            "name": "goon_provider_test_connection",
            "description": "Validate provider connectivity and key readiness",
            "input_schema": {"type": "object", "required": ["provider"]}
        }),
        json!({
            "name": "goon_provider_test_completion",
            "description": "Validate provider/model completion route",
            "input_schema": {"type": "object", "required": ["provider"]}
        }),
        json!({
            "name": "goon_provider_capabilities",
            "description": "Query provider model capabilities metadata",
            "input_schema": {"type": "object", "required": ["provider"]}
        }),
        json!({
            "name": "goon_metrics_window_query",
            "description": "Query metrics time-window series (1m/5m/1h)",
            "input_schema": {"type": "object"}
        }),
        json!({
            "name": "goon_metrics_errors_summary",
            "description": "Query grouped errors and sample failures",
            "input_schema": {"type": "object"}
        }),
        json!({
            "name": "goon_skill_update",
            "description": "Update imported skill manifest fields",
            "input_schema": {"type": "object", "required": ["name"]}
        }),
        json!({
            "name": "goon_skill_version_list",
            "description": "List imported skill version snapshots",
            "input_schema": {"type": "object", "required": ["name"]}
        }),
        json!({
            "name": "goon_skill_version_rollback",
            "description": "Rollback imported skill to a specified version",
            "input_schema": {"type": "object", "required": ["name", "version"]}
        }),
        json!({
            "name": "prompts_list",
            "description": "List all available prompt templates organized by category. Returns categories with their templates including id, title, description, content, and tags.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "lang": {
                        "type": "string",
                        "description": "Language code: 'en', 'zh-CN', or 'zh-TW' (default: 'en')",
                        "default": "en"
                    }
                }
            }
        }),
        json!({
            "name": "prompts_get",
            "description": "Get a single prompt template by its id. Returns the full template content and metadata.",
            "input_schema": {
                "type": "object",
                "required": ["id"],
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The template id to retrieve"
                    },
                    "lang": {
                        "type": "string",
                        "description": "Language code: 'en', 'zh-CN', or 'zh-TW' (default: 'en')",
                        "default": "en"
                    }
                }
            }
        }),
        json!({
            "name": "skill-finder",
            "description": "Search for registered skills by description or intent. Returns matching skills with their names, descriptions, performance scores, and input schemas. The AI can use this tool to discover which skills are available for a given task before invoking them.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Natural language description of what the user wants to accomplish"
                    },
                    "top_k": {
                        "type": "integer",
                        "description": "Maximum number of matching skills to return (default 5, max 20)",
                        "default": 5
                    }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "echo_skill",
            "description": "Echo back structured input for skill pipeline diagnostics.",
            "input_schema": {
                "type": "object"
            }
        }),
        json!({
            "name": "skill-creator",
            "description": "Create or update prompt skills from structured definitions.",
            "input_schema": {
                "type": "object"
            }
        }),
        json!({
            "name": "builtin.echo",
            "description": "Echo tool payload for connectivity and contract diagnostics.",
            "input_schema": {
                "type": "object"
            }
        }),
        json!({
            "name": "http_request",
            "description": "Make HTTP requests (GET/POST/PUT/DELETE) to external APIs. Only http:// and https:// URLs are allowed. Private/internal IPs are blocked for security.",
            "input_schema": {
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The full HTTP/HTTPS URL to request"
                    },
                    "method": {
                        "type": "string",
                        "enum": ["GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS"],
                        "description": "HTTP method (default: GET)"
                    },
                    "headers": {
                        "type": "object",
                        "description": "Optional HTTP headers as key-value pairs",
                        "additionalProperties": {"type": "string"}
                    },
                    "body": {
                        "type": "string",
                        "description": "Request body for POST/PUT/PATCH"
                    },
                    "auth": {
                        "type": "object",
                        "properties": {
                            "bearer": {
                                "type": "string",
                                "description": "Bearer token for Authorization header"
                            }
                        }
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "description": "Request timeout in milliseconds (default: 15000)"
                    }
                },
                "required": ["url"]
            }
        }),
        json!({
            "name": "workflow_execute",
            "description": "Execute a workflow task with optional phase. The AI creates and runs an autonomous multi-step plan using available skills to accomplish the given task.",
            "input_schema": {
                "type": "object",
                "required": ["task"],
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "The task to execute as a workflow"
                    },
                    "phase": {
                        "type": "string",
                        "description": "Optional phase for multi-phase workflows"
                    }
                }
            }
        }),
        json!({
            "name": "workflow_ask",
            "description": "Ask the AI to complete a task using its full reasoning and available skills. The AI will autonomously determine the best approach using workflow orchestration.",
            "input_schema": {
                "type": "object",
                "required": ["task"],
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "The task or question to process"
                    },
                    "auto_create_skills": {
                        "type": "boolean",
                        "description": "Whether to auto-create skills if needed (default: true)"
                    }
                }
            }
        }),
        json!({
            "name": "workflow_generate",
            "description": "Generate a workflow plan for a given task without executing it. Returns the structured plan for review.",
            "input_schema": {
                "type": "object",
                "required": ["task"],
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "The task to generate a workflow plan for"
                    }
                }
            }
        }),
        json!({
            "name": "import_skill",
            "description": "Import a skill from a remote source (GitHub, URL, etc.). Returns the imported skill record with name, version, and metadata.",
            "input_schema": {
                "type": "object",
                "required": ["source"],
                "properties": {
                    "source": {
                        "type": "object",
                        "description": "Source configuration for the skill import"
                    }
                }
            }
        }),
        json!({
            "name": "github_search_skills",
            "description": "Search GitHub for go-on compatible skills by query. Returns matching repositories with stars, description, and topics.",
            "input_schema": {
                "type": "object",
                "required": ["query"],
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query to find skills on GitHub"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of results to return (1-20, default 10)",
                        "default": 10
                    }
                }
            }
        }),
    ];

    let registry = global_tool_registry();
    let mut builtins = registry.names();
    builtins.sort_unstable();
    tools.extend(builtins.into_iter().map(|name| {
        // Prefer the actual Tool trait's description/input_schema over the
        // shared tool_descriptors.rs match arms. This ensures feature-gated
        // tools (docx, pdf, excel, cad, image, etc.) also get their proper
        // schema exposed in tools/list, not just a generic fallback.
        if let Some(tool) = registry.get(name) {
            // Prefer the Tool trait's description, but fall back to the shared
            // tool_descriptors table when the trait returns an empty string
            // (several core built-in tools do not override description()).
            let description: String = {
                let own = tool.description();
                if own.is_empty() {
                    crate::shared::tool_descriptors::tool_descriptor_value(name)
                        .get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Registered MCP tool")
                        .to_string()
                } else {
                    own.to_string()
                }
            };
            json!({
                "name": name,
                "description": description,
                "input_schema": tool.input_schema(),
            })
        } else {
            serde_json::to_value(local_tool_descriptor(name)).unwrap_or_else(|_| {
                json!({
                    "name": name,
                    "description": "Registered MCP tool",
                    "input_schema": {"type": "object"}
                })
            })
        }
    }));

    if let Some(server) = server {
        let registry = server
            .orchestration_deps
            .skill_registry
            .read()
            .unwrap_or_else(|poisoned| {
                tracing::warn!("lock poisoned, recovering");
                poisoned.into_inner()
            });
        tools.extend(registry.list(false).into_iter().map(|skill| {
            json!({
                "name": skill.name,
                "description": skill.description,
                "input_schema": skill.input_schema,
                "x_runtime": {
                    "score": skill.score,
                    "total_calls": skill.total_calls,
                    "success_calls": skill.success_calls,
                    "failure_calls": skill.failure_calls,
                    "average_latency_ms": skill.average_latency_ms,
                }
            })
        }));

        if let Ok(store) = open_skill_import_store(server) {
            for record in store.list().into_iter().filter(|record| record.enabled) {
                let (description, input_schema) = load_imported_skill_manifest(&record)
                    .map(|manifest| {
                        let description = if manifest.description.trim().is_empty() {
                            format!(
                                "Imported skill manifest {}@{}",
                                manifest.name, manifest.version
                            )
                        } else {
                            manifest.description
                        };
                        (description, manifest.input_schema)
                    })
                    .unwrap_or_else(|| {
                        (
                            format!("Imported skill manifest {}@{}", record.name, record.version),
                            json!({"type": "object"}),
                        )
                    });

                tools.push(json!({
                    "name": record.name,
                    "description": description,
                    "input_schema": input_schema,
                    "x_import": {
                        "source": record.source,
                        "source_ref": record.source_ref,
                        "sha256": record.sha256,
                        "version": record.version,
                        "manifest_path": record.manifest_path,
                    }
                }));
            }
        }
    }

    // Keep descriptor names unique across baseline/runtime/imported sources.
    // This avoids count drift between ACP and MCP routes when the same skill
    // is exposed by both static baseline and dynamic registries.
    let mut seen = std::collections::HashSet::new();
    tools.retain(|tool| {
        let Some(name) = tool.get("name").and_then(Value::as_str) else {
            return true;
        };
        seen.insert(name.to_string())
    });

    tools
}

pub(crate) async fn execute_mcp_tool_call(
    server: &AcpServer,
    name: &str,
    arguments: &Value,
) -> Result<Value> {
    // OTel span: captures tool name, execution duration, and outcome.
    // Instrument the body so the span is Send-safe across await points.
    let span = tracing::info_span!("tool_execute", tool = %name);
    let start = std::time::Instant::now();
    let tool_name = name.to_string();

    let result = async {
    if let Some(harness_bus) = server.governance_deps.harness_bus.as_ref() {
        let verdict = harness_bus.evaluator.check_tool_call(&tool_name, arguments);
        // Skip require_review for tools that are registered in the
        // tool_registry — they are explicitly registered and trusted.
        let is_registered = server.tool_registry.get(name).is_some();
        if verdict.require_review && !is_registered {
            // Unknown tool — not in sandbox whitelist. The AI should ask
            // the user for approval before using this tool (inline
            // confirmation pattern, like zed chat / copilot chat).
            record_tool_harness_sandbox_denied();
            anyhow::bail!(
                "tool '{}' is not in sandbox whitelist and requires user confirmation. \
                 Ask the user for approval before using this tool. \
                 Tools can be added to the allowlist via governance policy configuration.",
                name,
            );
        } else if !verdict.allowed && !verdict.require_review {
            record_tool_harness_sandbox_denied();
            anyhow::bail!(
                "tool '{}' denied by harness sandbox policy (sandbox_allowed={}). \
                 The sandbox level or governance policy is blocking this tool. \
                 Try adjusting the sandbox level or providing a policy override.",
                name,
                verdict.allowed
            );
        }
        if !verdict.budget_ok {
            record_tool_budget_denied();
            anyhow::bail!("tool '{}' denied by harness budget gate", name);
        }
        if !verdict.permitted {
            record_tool_rbac_denied();
            anyhow::bail!("tool '{}' denied by harness RBAC permission gate", name);
        }
    } else {
        // No HarnessBus — apply default tool governance policy
        // to prevent "default allow all" blind spot (AUTON-05).
        let classification =
            crate::acp::helpers::tool_governance_defaults::evaluate_default_tool_policy(
                name,
                false,
                false,
                server.runtime_config.deployment_target.as_deref(),
            );
        if !classification.allowed {
            anyhow::bail!(
                "tool '{}' blocked by default governance policy: {} (risk_class={:?})",
                name,
                classification.reason,
                classification.risk_class,
            );
        }
    }

    // BLUE56-C05: ChaosEngine fault injection check (only when chaos-testing feature enabled)
    #[cfg(feature = "chaos-testing")]
    {
        static CHAOS: LazyLock<crate::resilience::chaos::ChaosEngine> = LazyLock::new(|| {
            let engine = crate::resilience::chaos::ChaosEngine::new();
            engine.set_enabled(std::env::var("GO_ON_CHAOS_ENABLED").as_deref() == Ok("1"));
            engine
        });
        if let Some(fault_type) = CHAOS.check_fault(name) {
            tracing::warn!(
                target: "chaos",
                tool = %name,
                fault = ?fault_type,
                "chaos engine would inject fault"
            );
        }
    }

    let budget_scope = budget_scope_key(name, arguments);
    let estimated_tokens = estimate_argument_tokens(arguments);
    let pua_engine = PuaRuleEngine::new(server.governance_deps.pua_enforcement_plan.clone());
    let remaining_tokens = {
        let mut trackers = tool_budget_trackers().lock().await;
        let tracker = trackers.entry(budget_scope.clone()).or_insert_with(|| {
            BudgetTracker::new(task_budget_for_target(
                server.runtime_config.deployment_target.as_deref(),
            ))
        });
        tracker.check_wall_clock().map_err(|err| {
            record_tool_budget_denied();
            anyhow::anyhow!("budget denied tool '{name}' in scope '{budget_scope}': {err}")
        })?;
        tracker.record_tool_call().map_err(|err| {
            record_tool_budget_denied();
            anyhow::anyhow!("budget denied tool '{name}' in scope '{budget_scope}': {err}")
        })?;
        tracker
            .consume_with_pua(estimated_tokens, &pua_engine)
            .map_err(|err| {
                record_tool_budget_denied();
                anyhow::anyhow!("budget denied tool '{name}' in scope '{budget_scope}': {err}")
            })?;
        tracker.remaining_tokens()
    };

    // ── Redundant-hardening check removed ─────────────────────────────
    // The HarnessBus sandbox check above (or the default governance fallback)
    // already classifies every tool and calls SandboxPolicy::can_execute_*().
    // The old `governance_action_for_tool()` + `enforce_action()` was a
    // second, name-heuristic-based classification that duplicated the same
    // SandboxPolicy calls — and could diverge from the authoritative
    // per-tool mapping in evaluator.rs.  Removing this layer eliminates the
    // redundant check and the potential for inconsistent classification.
    //
    // Budget tracking (wall-clock, tool-call-count, PUA token consumption)
    // remains above and applies to all tools regardless of governance path.

    record_tool_allowed();
    info!(
        "hardening allow tool={} budget_scope={} estimated_tokens={} remaining_tokens={}",
        name,
        budget_scope,
        estimated_tokens,
        remaining_tokens
    );

    match name {
        "acp_trace_get" => {
            let trace = build_trace_payload(arguments);
            Ok(json!({
                "ok": true,
                "events": trace.get("events").cloned().unwrap_or_else(|| json!([])),
                "total": trace.get("total").cloned().unwrap_or_else(|| json!(0)),
                "limit": trace.get("limit").cloned().unwrap_or_else(|| json!(100)),
            }))
        }
        "acp_debug_panel_get" => Ok(build_debug_panel_payload(server).await),
        "goon_workflow_run_list" => workflow_run_list_payload(arguments),
        "goon_workflow_run_get" => workflow_run_get_payload(arguments),
        "goon_workflow_run_cancel" => workflow_run_transition_payload(arguments, "cancelled"),
        "goon_workflow_run_pause" => workflow_run_transition_payload(arguments, "paused"),
        "goon_workflow_run_resume" => workflow_run_transition_payload(arguments, "running"),
        "goon_provider_test_connection" => provider_test_connection_payload(server, arguments).await,
        "goon_provider_test_completion" => provider_test_completion_payload(server, arguments),
        "goon_provider_capabilities" => provider_capabilities_payload(server, arguments),
        "goon_metrics_window_query" => Ok(metrics_window_query_payload(server, arguments)),
        "goon_metrics_errors_summary" => Ok(metrics_errors_summary_payload(server, arguments)),
        "goon_skill_update" => skill_update_payload(server, arguments),
        "goon_skill_version_list" => skill_version_list_payload(server, arguments),
        "goon_skill_version_rollback" => skill_version_rollback_payload(server, arguments),
        "prompts_list" => {
            let lang = arguments
                .get("lang")
                .and_then(|v| v.as_str())
                .unwrap_or("en");
            Ok(build_prompts_list_tool(&server.prompt_manager, lang))
        }
        "prompts_get" => {
            let id = arguments
                .get("id")
                .and_then(|v| v.as_str())
                .context("missing required field: id")?;
            let lang = arguments
                .get("lang")
                .and_then(|v| v.as_str())
                .unwrap_or("en");
            Ok(build_prompts_get_tool(&server.prompt_manager, lang, id))
        }
        "skill-finder" => {
            let query = arguments
                .get("query")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let top_k = arguments
                .get("top_k")
                .and_then(|v| v.as_u64())
                .unwrap_or(5)
                .min(20) as usize;

            let results: Vec<Value> = {
                // Use SkillDiscovery with SkillIndex for token-based similarity search.
                let mut discovery = skill_discovery().lock().unwrap_or_else(|e| e.into_inner());
                let registry = server
                    .orchestration_deps
                    .skill_registry
                    .read()
                    .unwrap_or_else(|poisoned| {
                        tracing::warn!("lock poisoned, recovering");
                        poisoned.into_inner()
                    });
                // No ThresholdLearner available here — use default 0.40 threshold.
                let scored = discovery.discover(query, top_k, &registry, None);
                drop(registry);
                scored
                    .into_iter()
                    .map(|s| json!({
                        "name": s.name,
                        "description": s.description,
                        "score": s.score,
                        "input_schema": s.input_schema,
                        "total_calls": s.total_calls,
                        "success_calls": s.success_calls,
                        "failure_calls": s.failure_calls,
                        "average_latency_ms": s.average_latency_ms,
                    }))
                    .collect()
            };
            Ok(json!({
                "ok": true,
                "query": query,
                "results": results,
                "total": results.len(),
            }))
        }
        "import_skill" => {
            let request: SkillImportRequest = serde_json::from_value(arguments.clone())
                .context("invalid params for import_skill: expected { source: { ... } }")?;
            let policy = skill_import_policy(server);
            if !policy.enabled {
                anyhow::bail!("skill import is disabled by security policy");
            }
            let mut store = SkillImportStore::load(
                policy,
                server.orchestration_deps.skill_registry.clone(),
            )?;
            match store.import_skill(request).await {
                Ok(record) => {
                    store.save()?;
                    Ok(json!({
                        "ok": true,
                        "action": "import",
                        "name": record.name,
                        "version": record.version,
                        "description": record.description,
                        "source": record.source,
                        "source_ref": record.source_ref,
                        "enabled": record.enabled,
                    }))
                }
                Err(e) => {
                    anyhow::bail!("skill import failed: {e}")
                }
            }
        }
        "github_search_skills" => {
            let query = arguments
                .get("query")
                .and_then(|v| v.as_str())
                .context("missing required field: query")?;
            let max_results = arguments
                .get("max_results")
                .and_then(|v| v.as_u64())
                .unwrap_or(10)
                .clamp(1, 20) as usize;

            // Try GitHub API first, with a short timeout. The process-global
            // client (shared/http_client.rs) is used so all HTTP callers share
            // one connection pool.
            let client = crate::shared::http_client::http_client()
                .map_err(|e| anyhow::anyhow!("shared HTTP client unavailable: {e}"))?;

            let encoded_query = query.replace(" ", "+");

            // Search GitHub repos with go-on-skill topic
            let url = format!(
                "https://api.github.com/search/repositories?q={encoded_query}+topic:go-on-skill&sort=stars&order=desc&per_page={max_results}"
            );

            let resp = client.get(&url).send().await;
            let items = match resp {
                Ok(r) if r.status().is_success() => {
                    let body: serde_json::Value = r.json().await.unwrap_or_default();
                    body.get("items")
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default()
                }
                _ => {
                    // GitHub API rate-limited or unavailable — search via `go-on` topic
                    // as fallback
                    let fallback_url = format!(
                        "https://api.github.com/search/repositories?q={encoded_query}+topic:go-on&sort=stars&order=desc&per_page={max_results}"
                    );
                    match client.get(&fallback_url).send().await {
                        Ok(r) if r.status().is_success() => {
                            let body: serde_json::Value = r.json().await.unwrap_or_default();
                            body.get("items")
                                .and_then(|v| v.as_array())
                                .cloned()
                                .unwrap_or_default()
                        }
                        _ => Vec::new(),
                    }
                }
            };

            let results: Vec<serde_json::Value> = items.iter().map(|item| {
                json!({
                    "repo": item.get("full_name").and_then(|v| v.as_str()).unwrap_or(""),
                    "description": item.get("description").and_then(|v| v.as_str()).unwrap_or(""),
                    "stars": item.get("stargazers_count").and_then(|v| v.as_u64()).unwrap_or(0),
                    "url": item.get("html_url").and_then(|v| v.as_str()).unwrap_or(""),
                    "language": item.get("language").and_then(|v| v.as_str()).unwrap_or(""),
                    "topics": item.get("topics").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|t| t.as_str().map(String::from)).collect::<Vec<_>>()).unwrap_or_default(),
                })
            }).collect();

            Ok(json!({
                "ok": true,
                "query": query,
                "total": results.len(),
                "results": results,
            }))
        }
        _ => {
            let registry = global_tool_registry();
            if let Some(tool) = registry.get(name) {
                crate::shared::tool_descriptors::validate_required_arguments(name, arguments)?;
                // Use spawn_blocking to avoid blocking the tokio runtime thread
                // with synchronous tool execution (e.g. shell_exec may take 60s).
                let input = ToolInput {
                    task_id: format!("mcp-tool-{name}"),
                    phase: "mcp".to_string(),
                    agent_role: "tool".to_string(),
                    objective: format!("Execute MCP tool '{name}'"),
                    constraints: None,
                    evidence: None,
                    payload: arguments.clone(),
                    allowed_base_dir: None,
                };
                let result = tokio::task::spawn_blocking(move || tool.run(&input))
                    .await
                    .map_err(|e| anyhow::anyhow!("tool execution panicked: {e}"))??;
                return Ok(serde_json::to_value(result)?);
            }

            let resolved_skill_name = {
                let registry = server
                    .orchestration_deps
                    .skill_registry
                    .read()
                    .unwrap_or_else(|poisoned| {
                        tracing::warn!("lock poisoned, recovering");
                        poisoned.into_inner()
                    });
                if registry.get(name).is_some() {
                    Some(name.to_string())
                } else {
                    registry.best_match_with_input(name, arguments)
                }
            };
            let skill = resolved_skill_name.as_ref().and_then(|resolved| {
                let registry = server
                    .orchestration_deps
                    .skill_registry
                    .read()
                    .unwrap_or_else(|poisoned| {
                        tracing::warn!("lock poisoned, recovering");
                        poisoned.into_inner()
                    });
                registry.get(resolved)
            });
            match skill {
                Some(skill) => {
                    let started = Instant::now();
                    let outcome = skill.execute(arguments).await;
                    let skill_name = resolved_skill_name.as_deref().unwrap_or(name);
                    let mut registry = server
                        .orchestration_deps
                        .skill_registry
                        .write()
                        .unwrap_or_else(|poisoned| {
                            tracing::warn!("lock poisoned, recovering");
                            poisoned.into_inner()
                        });
                    registry.record_outcome(skill_name, outcome.is_ok(), started.elapsed());
                    outcome
                }
                None => {
                    if let Some(imported) = find_enabled_imported_skill(server, name)? {
                        if let Some(manifest) = load_imported_skill_manifest(&imported) {
                            // Real execution: prompt-based imported skills run through the
                            // same LLM executor as registry prompt skills (PromptBasedSkill).
                            // If no LLM agent is wired, execute() returns a clear error.
                            if let Some(prompt_template) = &manifest.prompt_template {
                                // Convert the manifest's JSON-schema `properties`
                                // map into the `{name -> type}` shape that
                                // PromptBasedSkill expects.
                                let input_schema: std::collections::HashMap<String, String> =
                                    manifest
                                        .input_schema
                                        .get("properties")
                                        .and_then(Value::as_object)
                                        .map(|props| {
                                            props
                                                .iter()
                                                .filter_map(|(k, v)| {
                                                    v.get("type")
                                                        .and_then(Value::as_str)
                                                        .map(|t| (k.clone(), t.to_string()))
                                                })
                                                .collect()
                                        })
                                        .unwrap_or_default();
                                let skill =
                                    crate::orchestration::skill::PromptBasedSkill {
                                        name: manifest.name.clone(),
                                        description: manifest.description.clone(),
                                        prompt_template: prompt_template.clone(),
                                        input_schema,
                                        timeout_secs: 120,
                                        max_retries: 2,
                                        disable_model_invocation: manifest.disable_model_invocation,
                                        policy: None,
                                    };
                                let result = skill.execute(arguments).await?;
                                return Ok(json!({
                                    "ok": true,
                                    "executed": true,
                                    "mode": "imported_manifest",
                                    "name": manifest.name,
                                    "version": manifest.version,
                                    "source": imported.source,
                                    "source_ref": imported.source_ref,
                                    "sha256": imported.sha256,
                                    "result": result,
                                }));
                            }
                            anyhow::bail!(
                                "imported skill '{}' has no executable content (no \
                                 prompt_template in manifest {}); cannot execute",
                                manifest.name,
                                imported.manifest_path
                            );
                        }
                        anyhow::bail!(
                            "imported skill '{}' manifest is unavailable at {}; cannot execute",
                            imported.name,
                            imported.manifest_path
                        );
                    }
                    anyhow::bail!("unknown tool or skill: {name}")
                }
            }
        }
    }
    }.instrument(span).await;

    // Record tool execution metrics.
    let latency_ms = start.elapsed().as_secs_f64() * 1000.0;
    crate::observability::performance::record_global_operation(result.is_ok(), latency_ms);
    if let Err(e) = &result {
        tracing::warn!(
            tool = %tool_name,
            latency_ms = %latency_ms,
            error = %e,
            "tool execution failed"
        );
    }
    result
}

pub(super) fn find_enabled_imported_skill(
    server: &AcpServer,
    name: &str,
) -> Result<Option<ImportedSkillRecord>> {
    let store = open_skill_import_store(server)?;
    Ok(store
        .list()
        .into_iter()
        .find(|record| record.enabled && record.name == name))
}

pub(super) fn load_imported_skill_manifest(
    record: &ImportedSkillRecord,
) -> Option<SkillImportManifest> {
    let raw = fs::read_to_string(&record.manifest_path).ok()?;
    serde_json::from_str::<SkillImportManifest>(&raw).ok()
}

pub(crate) fn budget_scope_key(name: &str, arguments: &Value) -> String {
    if let Some(task_id) = arguments.get("task_id").and_then(Value::as_str) {
        return format!("task:{task_id}");
    }
    if let Some(conversation_id) = arguments.get("conversation_id").and_then(Value::as_str) {
        return format!("conversation:{conversation_id}");
    }
    format!("tool:{name}")
}

pub(crate) fn estimate_argument_tokens(arguments: &Value) -> usize {
    // Lightweight approximation keeps budget enforcement deterministic without model calls.
    serde_json::to_string(arguments)
        .map(|payload| (payload.len() / 4).max(1))
        .unwrap_or(1)
}

pub(super) fn governance_action_for_tool(name: &str) -> GovernanceAction {
    crate::governance::tool_capability::ToolCapabilityRegistry::action(name)
}

pub(super) fn local_tool_descriptor(name: &'static str) -> Value {
    crate::shared::tool_descriptors::tool_descriptor_value(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── build_mcp_tool_descriptors baseline ────────────────────────────

    #[test]
    fn build_mcp_tool_descriptors_returns_baseline_tools() {
        let tools = build_mcp_tool_descriptors(None);
        assert!(!tools.is_empty(), "must return at least baseline tools");

        // All registered tools should have proper descriptors, not generic fallback.
        let generic_count = tools
            .iter()
            .filter(|t| {
                t.get("description")
                    .and_then(Value::as_str)
                    .map(|d| d == "Registered MCP tool")
                    .unwrap_or(false)
            })
            .count();
        assert!(
            generic_count < 5,
            "at most 4 tools should have generic description, got {generic_count}"
        );

        // Should include core tools
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|t| t.get("name").and_then(Value::as_str))
            .collect();
        assert!(names.contains(&"acp_trace_get"));
        assert!(names.contains(&"acp_debug_panel_get"));
        assert!(names.contains(&"goon_workflow_run_list"));
        assert!(names.contains(&"prompts_list"));
        assert!(names.contains(&"prompts_get"));
        assert!(names.contains(&"builtin.echo"));
    }

    #[test]
    fn build_mcp_tool_descriptors_all_have_input_schema() {
        let tools = build_mcp_tool_descriptors(None);
        for tool in &tools {
            let name = tool.get("name").and_then(Value::as_str).unwrap_or("?");
            assert!(
                tool.get("input_schema").is_some() || tool.get("inputSchema").is_some(),
                "tool '{}' missing input_schema",
                name
            );
        }
    }

    #[test]
    fn build_mcp_tool_descriptors_no_duplicate_names() {
        let tools = build_mcp_tool_descriptors(None);
        let mut seen = std::collections::HashSet::new();
        for tool in &tools {
            let name = tool.get("name").and_then(Value::as_str).unwrap_or("?");
            assert!(
                seen.insert(name.to_string()),
                "duplicate tool name: {}",
                name
            );
        }
    }

    // ── budget_scope_key ──────────────────────────────────────────────

    #[test]
    fn budget_scope_key_uses_task_id() {
        let key = budget_scope_key("tool_x", &json!({"task_id": "abc"}));
        assert_eq!(key, "task:abc");
    }

    #[test]
    fn budget_scope_key_uses_conversation_id() {
        let key = budget_scope_key("tool_x", &json!({"conversation_id": "conv-1"}));
        assert_eq!(key, "conversation:conv-1");
    }

    #[test]
    fn budget_scope_key_falls_back_to_tool_name() {
        let key = budget_scope_key("my_tool", &json!({}));
        assert_eq!(key, "tool:my_tool");
    }

    // ── estimate_argument_tokens ───────────────────────────────────────

    #[test]
    fn estimate_argument_tokens_returns_at_least_one() {
        let tokens = estimate_argument_tokens(&Value::Null);
        assert!(tokens >= 1);
    }

    #[test]
    fn estimate_argument_tokens_scales_with_payload_size() {
        let small = estimate_argument_tokens(&json!("a"));
        let large = estimate_argument_tokens(&json!("a".repeat(400)));
        assert!(large > small);
    }

    // ── governance_action_for_tool ─────────────────────────────────────

    #[test]
    fn governance_action_for_tool_shell() {
        assert_eq!(
            governance_action_for_tool("shell_exec"),
            crate::governance::hardening::GovernanceAction::Shell
        );
    }

    #[test]
    fn governance_action_for_tool_write() {
        assert_eq!(
            governance_action_for_tool("write_file"),
            crate::governance::hardening::GovernanceAction::Write
        );
    }

    #[test]
    fn governance_action_for_tool_search() {
        assert_eq!(
            governance_action_for_tool("search_files"),
            crate::governance::hardening::GovernanceAction::Search
        );
    }

    #[test]
    fn governance_action_for_tool_default_read() {
        assert_eq!(
            governance_action_for_tool("read_file"),
            crate::governance::hardening::GovernanceAction::Read
        );
    }
}
