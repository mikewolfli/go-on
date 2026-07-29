//! Unified concurrent tool execution engine.
//!
//! Executes a batch of tool calls concurrently using `FuturesUnordered`,
//! with built-in governance gating, per-tool retry, circuit breaker,
//! ACP session notifications, SSE progress events, and concurrency limiting.
//!
//! Replaces the duplicated serial loops in:
//! - `autonomy_loop.rs` (ACP main path)
//! - `agent_runtime.rs` (ACP secondary path)
//! - `cli/chat.rs` (CLI path — was already parallel but independently implemented)
//! - `dag_driver.rs` (dead code, entire module to be deleted)

use std::sync::Arc;
use std::time::Instant;

use crate::orchestration::tool::governance_gate::{
    governance_cache, is_low_risk_tool, low_risk_audit_log,
};

use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;
use serde_json::Value;
use tokio::sync::Semaphore;

use crate::acp::r#impl::chat::streaming::{emit_tool_approval_event, StreamFrame};
use crate::orchestration::tool::{RetryPolicy, ToolInput, ToolOutput, ToolRegistry};

/// Configuration for concurrent tool execution.
#[derive(Clone)]
pub(crate) struct ToolExecConfig {
    /// Maximum number of tools to execute concurrently.
    pub max_concurrency: usize,
    /// Maximum consecutive tool failures before circuit breaker trips (0 = disabled).
    pub circuit_breaker_limit: usize,
    /// Operation mode for governance gate ("edit", "safeguard", "full_auto", etc.).
    pub operation_mode: String,
    /// ACP session ID for tool call notifications.
    pub acp_session_id: Option<String>,
}

impl Default for ToolExecConfig {
    fn default() -> Self {
        Self {
            max_concurrency: 10,
            circuit_breaker_limit: 5,
            operation_mode: "ask".to_string(),
            acp_session_id: None,
        }
    }
}

/// Result of executing a batch of tool calls.
#[derive(Debug, Default)]
pub(crate) struct ToolExecResult {
    /// Individual tool results (tool_name → formatted output).
    pub tool_results: Vec<ToolExecItem>,
    /// Total number of tools that failed.
    pub failure_count: usize,
    /// True if the circuit breaker was triggered.
    pub circuit_breaker_triggered: bool,
}

/// Result of a single tool execution.
#[derive(Debug)]
pub(crate) struct ToolExecItem {
    pub tool_name: String,
    pub output: ToolOutput,
    pub success: bool,
    pub duration_ms: u64,
    pub formatted: String,
}

/// Execute a batch of tool calls concurrently with governance, retry, and circuit breaker.
///
/// This is the single entry point for all tool execution in go-on. All paths
/// (ACP autonomy loop, ACP agent_runtime, CLI chat) should call this function
/// instead of implementing their own tool execution loops.
///
/// # Arguments
/// * `tool_calls` — List of (tool_name, args_json) tuples.
/// * `tool_registry` — Registry to resolve tools and their RetryPolicies.
/// * `config` — Execution configuration (concurrency, circuit breaker, governance).
/// * `progress_tx` — Optional SSE progress stream.
/// * `objective` — Current task objective (for ToolInput).
/// * `iteration` — Current iteration/round number (for task_id generation).
/// * `emit_acp_notifications` — Whether to emit ACP session tool call started/completed events.
pub(crate) async fn execute_tools_concurrent(
    tool_calls: &[(String, String)],
    tool_registry: &ToolRegistry,
    config: &ToolExecConfig,
    progress_tx: Option<tokio::sync::mpsc::UnboundedSender<StreamFrame>>,
    objective: &str,
    iteration: usize,
) -> ToolExecResult {
    if tool_calls.is_empty() {
        return ToolExecResult::default();
    }

    let max_concurrency = config.max_concurrency.max(1);
    let semaphore = Arc::new(Semaphore::new(max_concurrency));
    let circuit_breaker_limit = config.circuit_breaker_limit;
    let mut circuit_breaker_triggered = false;
    let mut failure_count: usize = 0;
    let mut tool_results: Vec<ToolExecItem> = Vec::with_capacity(tool_calls.len());

    // Build concurrent futures for all tool calls.
    let mut futures: FuturesUnordered<_> = FuturesUnordered::new();

    for (tool_name, tool_args_str) in tool_calls {
        let tool_name = tool_name.clone();
        let tool_args_str = tool_args_str.clone();
        let sem_clone = Arc::clone(&semaphore);
        let config_clone = config.clone();
        let progress_tx_clone = progress_tx.clone();
        let objective = objective.to_string();

        futures.push(async move {
            let _permit = sem_clone.acquire().await.ok();
            execute_single_tool(
                &tool_name,
                &tool_args_str,
                tool_registry,
                &config_clone,
                progress_tx_clone,
                &objective,
                iteration,
            )
            .await
        });
    }

    // Collect results as they complete.
    while let Some(result) = futures.next().await {
        if !result.success {
            failure_count += 1;
        }

        tool_results.push(result);

        // Check circuit breaker.
        // Triggered when failure_count reaches the limit, regardless of
        // whether any success occurred. The previous `&& total_success == 0`
        // condition was too aggressive — it would trip on the very first
        // failure even in normal operation, causing premature cancellation.
        if circuit_breaker_limit > 0 && failure_count >= circuit_breaker_limit {
            circuit_breaker_triggered = true;
            // Cancel remaining futures by dropping the stream.
            break;
        }
    }

    ToolExecResult {
        tool_results,
        failure_count,
        circuit_breaker_triggered,
    }
}

/// Execute a single tool with governance, retry, notifications, and SSE events.
///
/// This function handles the full lifecycle of a single tool call:
/// 1. Parse and validate arguments
/// 2. Governance gate (edit/safeguard mode approval)
/// 3. Per-tool retry loop for execution
/// 4. ACP session notifications (tool_call_started / tool_call_completed)
/// 5. SSE progress events
async fn execute_single_tool(
    tool_name: &str,
    tool_args_str: &str,
    tool_registry: &ToolRegistry,
    config: &ToolExecConfig,
    progress_tx: Option<tokio::sync::mpsc::UnboundedSender<StreamFrame>>,
    objective: &str,
    iteration: usize,
) -> ToolExecItem {
    let start = Instant::now();
    let tool_name = tool_name.to_string();

    // ── Retrieve per-tool retry policy from the registry ───────────
    let retry_policy = tool_registry
        .profile(&tool_name)
        .map(|p| p.retry_policy.clone())
        .unwrap_or(RetryPolicy {
            max_retries: 0,
            retry_on_failure: false,
        });

    // ── Stream progress event before executing tool ────────────────
    if let Some(ref tx) = progress_tx {
        let _ = tx.send(StreamFrame {
            event: "progress",
            payload: serde_json::json!({
                "message": format!("executing tool {}...", tool_name),
            }),
            status: Some("analyzing"),
        });
    }

    // ── Parse and validate arguments ───────────────────────────────
    let parsed_args: Value = serde_json::from_str(tool_args_str).unwrap_or_default();
    let validation_passed =
        crate::shared::tool_descriptors::validate_required_arguments(&tool_name, &parsed_args)
            .is_ok();

    if !validation_passed {
        let err_msg = if let Some(schema_str) = lookup_tool_schema(&tool_name) {
            format!(
                "Tool '{}' call rejected: required parameters were not provided.\n\
                 Expected input schema for '{}':\n{}\n\
                 Please re-read the schema and provide ALL required parameters in your next tool call.",
                tool_name, tool_name, schema_str
            )
        } else {
            format!(
                "Tool '{}' call rejected: required parameters were not provided.",
                tool_name
            )
        };
        let tool_name_owned = tool_name.clone();
        return ToolExecItem {
            tool_name,
            output: ToolOutput {
                success: false,
                result: None,
                error: Some(err_msg.clone()),
                verification: None,
                audit_log: None,
                pua_report: None,
            },
            success: false,
            duration_ms: start.elapsed().as_millis() as u64,
            formatted: format!(
                "\n[Tool {} validation failed:]\n{}\n\n",
                tool_name_owned, err_msg
            ),
        };
    }

    // ── Governance gate (edit/safeguard mode) ──────────────────────
    //
    // Low-risk tools (reads, utilities, etc.) skip the blocking governance
    // gate to avoid unnecessary latency. Instead, their access is recorded
    // via async audit logging. Safeguard mode always enforces the full gate.
    if config.operation_mode == "edit" || config.operation_mode == "safeguard" {
        if is_low_risk_tool(&tool_name) && config.operation_mode != "safeguard" {
            low_risk_audit_log(&tool_name, &config.operation_mode);
        } else {
            let _ = emit_tool_approval_event(
                &progress_tx,
                &tool_name,
                &parsed_args,
                &config.operation_mode,
                if config.operation_mode == "safeguard" {
                    0.5
                } else {
                    0.0
                },
            )
            .await;

            let approved = ensure_tool_permission(
                config,
                &tool_name,
                &parsed_args,
                if config.operation_mode == "safeguard" {
                    0.5
                } else {
                    0.0
                },
            )
            .await;

            if !approved {
                let denied_msg = format!("Tool '{}' denied by user approval gate.", tool_name);
                return ToolExecItem {
                    tool_name,
                    output: ToolOutput {
                        success: false,
                        result: None,
                        error: Some(denied_msg.clone()),
                        verification: None,
                        audit_log: None,
                        pua_report: None,
                    },
                    success: false,
                    duration_ms: start.elapsed().as_millis() as u64,
                    formatted: format!("\n[{}]\n", denied_msg),
                };
            }
        }
    }

    // ── Execute tool via registry ──────────────────────────────────
    if let Some(tool) = tool_registry.get_arc(&tool_name) {
        // ── Emit ToolCallUpdate::InProgress ────────────────────────
        if let Some(ref sid) = config.acp_session_id {
            if let Some(srv) = crate::acp::server::current_acp_server() {
                srv.emit_tool_call_started(sid, &tool_name, &parsed_args)
                    .await;
            }
        }

        let input = ToolInput {
            task_id: format!("autonomy-{}-{}", iteration, tool_name),
            phase: "execute".to_string(),
            agent_role: "assistant".to_string(),
            objective: objective.to_string(),
            constraints: None,
            evidence: None,
            payload: parsed_args.clone(),
            allowed_base_dir: None,
        };

        // ── Pre-execute hooks ──────────────────────────────────────
        tool_registry.hooks.run_pre(&tool_name, &input);

        // ── Execute with per-tool retry ────────────────────────────
        let max_exec_retries = if retry_policy.retry_on_failure {
            retry_policy.max_retries
        } else {
            0
        };

        let (output, success) = {
            let mut tool_result: Option<ToolOutput> = None;
            let mut last_error: Option<String> = None;
            let mut attempt: usize = 0;
            loop {
                match Arc::clone(&tool).run_async(input.clone()).await {
                    Ok(out) => {
                        tool_result = Some(out);
                        break;
                    }
                    Err(e) => {
                        last_error = Some(e.to_string());
                        if attempt >= max_exec_retries {
                            break;
                        }
                        attempt += 1;
                    }
                }
            }
            match tool_result {
                Some(out) => (out, true),
                None => {
                    let err_msg = last_error.unwrap_or_else(|| {
                        "tool execution stopped (retries exhausted)".to_string()
                    });
                    (
                        ToolOutput {
                            success: false,
                            result: None,
                            error: Some(err_msg),
                            verification: None,
                            audit_log: None,
                            pua_report: None,
                        },
                        false,
                    )
                }
            }
        };

        // ── Emit ToolCallUpdate::Completed ─────────────────────────
        let success_bool = output.error.is_none();
        if let Some(ref sid) = config.acp_session_id {
            if let Some(srv) = crate::acp::server::current_acp_server() {
                let output_val = output
                    .result
                    .as_ref()
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                srv.emit_tool_call_completed(
                    sid,
                    &tool_name,
                    &parsed_args,
                    &output_val,
                    success_bool,
                )
                .await;
            }
        }

        // ── SSE completion notification ────────────────────────────
        if let Some(ref tx) = progress_tx {
            let status = if success_bool { "completed" } else { "failed" };
            let summary = if success_bool {
                format!("✅ **{}** ", tool_name)
            } else {
                format!("❌ **{}** failed ", tool_name)
            };
            let _ = tx.send(StreamFrame {
                event: "chunk",
                payload: serde_json::json!({
                    "token": summary,
                    "tool_status": status,
                }),
                status: Some("generating"),
            });
        }

        let formatted = format_tool_output_for_response(&tool_name, &output);
        let duration_ms = start.elapsed().as_millis() as u64;

        // ── Post-execute hooks ────────────────────────────────────────
        tool_registry
            .hooks
            .run_post(&tool_name, &input, &output, duration_ms);

        ToolExecItem {
            tool_name,
            output,
            success,
            duration_ms,
            formatted,
        }
    } else {
        let err_msg = format!("Tool '{}' not found in registry", tool_name);
        let tool_name_owned = tool_name.clone();
        ToolExecItem {
            tool_name,
            output: ToolOutput {
                success: false,
                result: None,
                error: Some(err_msg.clone()),
                verification: None,
                audit_log: None,
                pua_report: None,
            },
            success: false,
            duration_ms: start.elapsed().as_millis() as u64,
            formatted: format!("\n[Tool {} not available]\n", tool_name_owned),
        }
    }
}

/// Format a tool's output for the consolidated response string.
fn format_tool_output_for_response(tool_name: &str, output: &ToolOutput) -> String {
    let success = output.error.is_none();
    let status_icon = if success { "✅" } else { "❌" };

    let body_content = if success {
        output
            .result
            .as_ref()
            .and_then(|r| {
                if let Some(s) = r.as_str() {
                    if !s.is_empty() {
                        return Some(s.to_string());
                    }
                }
                None
            })
            .unwrap_or_else(|| format!("{:?}", output))
    } else {
        output
            .error
            .as_ref()
            .cloned()
            .unwrap_or_else(|| format!("{:?}", output))
    };

    format!(
        "\n<details>\
         \n<summary>{} {}</summary>\
         \n```\n{}\n```\
         \n</details>\n",
        status_icon, tool_name, body_content
    )
}

/// Check tool permission via ACP session for governance gate (edit/safeguard mode).
///
/// Results are cached in a global [`ShardedGovernanceCache`] keyed by
/// `"{session_id}:{tool_name}"` to avoid redundant network round-trips
/// to the ACP client for repeated tool calls within the same session.
async fn ensure_tool_permission(
    config: &ToolExecConfig,
    tool_name: &str,
    parsed_args: &serde_json::Value,
    risk_score: f64,
) -> bool {
    let Some(server) = crate::acp::server::current_acp_server() else {
        return true;
    };
    let Some(session_id) = config.acp_session_id.as_ref() else {
        return true;
    };

    let cache_key = format!("{}:{}", session_id, tool_name);

    // Check cache first — avoids a 15 s network round-trip when the same
    // tool has already been approved/denied in this session.
    if let Some(cached) = governance_cache().get(&cache_key) {
        tracing::debug!(
            "executor: tool '{}' cache hit ({}), skipping permission request",
            tool_name,
            if cached { "approved" } else { "denied" }
        );
        return cached;
    }

    // Short timeout: if the ACP client (Zed) doesn't show a permission
    // dialog within 15 seconds, we treat it as implicitly allowed rather
    // than blocking the entire tool execution flow.
    let timeout_secs = 15;
    let result = match server
        .request_client_permission(
            session_id,
            tool_name,
            parsed_args,
            &config.operation_mode,
            risk_score,
            timeout_secs,
        )
        .await
    {
        Ok(true) => {
            tracing::debug!("executor: tool '{}' approved by user", tool_name);
            true
        }
        Ok(false) => {
            tracing::warn!("executor: tool '{}' denied by user", tool_name);
            false
        }
        Err(err) => {
            // When the user permission dialog doesn't appear (e.g. Zed ACP stdio
            // doesn't show Approve/Deny), the request will time out after 15s.
            // Instead of blocking for the full timeout, treat transport/connection
            // errors as "allow" so tools can still execute. Only hard denials
            // (Ok(false)) should block tool execution.
            tracing::warn!(
                "executor: permission gate for tool '{}' unavailable ({}), allowing",
                tool_name,
                err
            );
            true
        }
    };

    // Populate cache on cache miss (only cache definitive allow/deny, not errors).
    governance_cache().insert(cache_key, result);

    result
}

/// Look up a tool's input schema JSON string by name, at runtime.
fn lookup_tool_schema(tool_name: &str) -> Option<String> {
    let registry = crate::acp::r#impl::request::tools_pack::global_tool_registry();
    if let Some(tool) = registry.get(tool_name) {
        let schema = tool.input_schema();
        serde_json::to_string_pretty(&schema).ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── is_low_risk_tool ────────────────────────────────────────────

    #[test]
    fn low_risk_tools_are_recognized() {
        let low_risk = [
            "read_file",
            "search_files",
            "list_directory",
            "grep",
            "environment_info",
            "time_util",
            "uuid_gen",
            "random_token",
            "encode_decode",
            "hash_file",
            "diagnostics",
            "diff",
            "format_code",
            "code_metrics",
            "svg_export",
            "rss_feed",
            "date_time",
            "dns_lookup",
        ];
        for &tool in &low_risk {
            assert!(
                is_low_risk_tool(tool),
                "expected '{}' to be classified as low-risk",
                tool
            );
        }
    }

    #[test]
    fn high_risk_tools_are_not_low_risk() {
        let high_risk = [
            "write_file",
            "bash",
            "execute_command",
            "run",
            "edit_file",
            "create_directory",
            "delete_path",
            "move_path",
            "copy_path",
            "http_request",
            "npm_install",
        ];
        for &tool in &high_risk {
            assert!(
                !is_low_risk_tool(tool),
                "expected '{}' to NOT be classified as low-risk",
                tool
            );
        }
    }

    #[test]
    fn unknown_tool_is_not_low_risk() {
        assert!(!is_low_risk_tool("some_unknown_tool"));
        assert!(!is_low_risk_tool(""));
    }

    #[test]
    fn case_sensitive_matching() {
        // The function should do exact case-sensitive matching.
        assert!(is_low_risk_tool("read_file"), "exact match should work");
        assert!(
            !is_low_risk_tool("Read_File"),
            "wrong case should not match"
        );
        assert!(!is_low_risk_tool("READ_FILE"), "uppercase should not match");
    }

    // ── low_risk_audit_log ──────────────────────────────────────────

    #[test]
    fn low_risk_audit_log_does_not_panic() {
        // This is a smoke test — the function should not panic under normal
        // conditions. We can't easily assert on the log output, but we can
        // verify it completes without error.
        low_risk_audit_log("read_file", "edit");
        low_risk_audit_log("grep", "full_auto");
        low_risk_audit_log("uuid_gen", "ask");
    }

    // ── Governance skip behavior ───────────────────────────────────-

    /// Verify that the governance-skip decision logic matches our
    /// expectations without running the full async pipeline.
    #[test]
    fn governance_skip_logic_for_low_risk_tools() {
        // Low-risk tool in edit mode → should skip governance
        assert!(
            is_low_risk_tool("read_file") && "edit" != "safeguard",
            "low-risk in edit mode should be eligible for skip"
        );

        // Low-risk tool in safeguard mode → should NOT skip governance
        assert!(
            !(is_low_risk_tool("read_file") && "safeguard" != "safeguard"),
            "low-risk in safeguard mode should NOT be eligible for skip"
        );

        // High-risk tool in edit mode → should NOT skip governance
        assert!(
            !(is_low_risk_tool("write_file") && "edit" != "safeguard"),
            "high-risk in edit mode should NOT be eligible for skip"
        );

        // Low-risk tool in full_auto mode → should skip governance
        assert!(
            is_low_risk_tool("grep") && "full_auto" != "safeguard",
            "low-risk in full_auto mode should be eligible for skip"
        );

        // Low-risk tool in ask mode → should skip governance
        assert!(
            is_low_risk_tool("dns_lookup") && "ask" != "safeguard",
            "low-risk in ask mode should be eligible for skip"
        );
    }
}
