//! Pre-route policy evaluation pipeline stage
//!
//! This module extracts the HarnessBus policy evaluation and tenant budget
//! checks from `process_chat_request` into a standalone function.
//! It runs as the first stage in the chat request pipeline.

use anyhow::Result;
use regex::Regex;
use tracing::warn;

use crate::acp::r#impl::chat::ChatParams;
use crate::acp::server::AcpServer;
use crate::agent::Message;
use crate::orchestration::mode::ModeKind;

// Pre-compiled regex patterns for mode-capability validation (compiled once).
static RE_URL: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"https?://[^\s)\]\]]+").expect("valid URL regex"));
static RE_FILE_REF: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(
        r"(?i)\b[a-zA-Z0-9_\-./]+\.(rs|py|js|ts|go|rb|c|cpp|h|hpp|java|kt|swift|md|txt|json|yaml|toml|xml|html|css|sql|sh|bash|zsh|ps1|bat)\b",
    )
    .expect("valid file extension regex")
});
static RE_EXEC_KEYWORD: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"(?i)\b(create|build|write|implement|delete|remove|deploy|connect)\b")
        .expect("valid execution keyword regex")
});
static RE_SEC_KEYWORD: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(password|secret|token|credential|ssh|private\.key|api\.key|certificate|authorization|authentication)\b",
    )
    .expect("valid security keyword regex")
});
static RE_PLAN_KEYWORD: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(plan|design|architect|architecture|diagram|flowchart|blueprint|schema|proposal)\b",
    )
    .expect("valid planning keyword regex")
});

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Evaluate all pre-route policies in order:
///
/// 1. **Mode-capability cross-validation** — checks message content vs mode.
/// 2. **HarnessBus policy evaluation** — checks project-level policy gates.
/// 3. **Tenant budget check** — checks per-tenant resource quotas (F-GAP-08).
///
/// If any policy denies the request, this function returns an error.
/// Otherwise it returns `Ok(())`.
pub(crate) async fn evaluate_pre_route_policies(
    server: &AcpServer,
    params: &ChatParams,
    _tenant_id: &str,
) -> Result<()> {
    // ── Mode-capability cross-validation ────────────────────────────────
    // Check message content against the current mode's capabilities before
    // proceeding to the HarnessBus and budget stages.
    let current_mode = ModeKind::from(params.mode.as_str());
    validate_mode_capability(&current_mode, &params.messages)?;

    // ── HarnessBus pre-route budget clock reset ────────────────────────
    // Reset the wall-clock budget so long-running backends don't exceed their
    // budget. NOTE: the full `harness.evaluate()` was previously run here with
    // a FAKE context (task_type=Other, risk_score=0.3 hardcoded) — the real
    // compliance evaluation runs in `CapabilityBus::decide()` with the actual
    // task context, so this duplicate gate was removed.
    if let Some(ref harness) = server.governance_deps.harness_bus {
        // The budget lock is taken and released in a separate scope so the
        // !Send MutexGuard is dropped before any .await below.
        {
            let mut budget = match harness.evaluator.budget.lock() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    tracing::warn!("pre_route_policy: budget mutex was poisoned, recovering");
                    poisoned.into_inner()
                }
            };
            budget.reset();
            // MutexGuard dropped here
        }
    }

    // ── TenantBudgetEnforcer pre-route check (activated) ───────────────
    // Check per-tenant resource quotas before allocating compute.
    // Uses the _tenant_id resolved from the ChatRequestContext (which comes
    // from the user session when auth is enabled, or falls back to default).
    // In strict mode (production_strict=true) the request is rejected when
    // quota is exceeded; in non-strict mode the request is allowed with a
    // warning log.
    #[cfg(feature = "multi-users-server")]
    {
        let tenant_budget_ok = {
            let budget_guard = server.rate_limiting.tenant_budget.lock();
            match budget_guard {
                Ok(mut budget) => {
                    // check_and_start_task atomically validates and consumes the
                    // concurrent-task slot (avoids the TOCTOU race of the old
                    // separate check_can_start + start_task pair).
                    if server.runtime_config.production_strict {
                        if let Err(e) = budget.check_and_start_task(_tenant_id) {
                            warn!("tenant budget limit reached for {}: {}", _tenant_id, e);
                            false
                        } else {
                            true
                        }
                    } else {
                        // Non-strict mode: warn but allow through.
                        if let Err(e) = budget.check_and_start_task(_tenant_id) {
                            warn!(
                                "tenant budget limit reached for {}: {} (non-strict, allowing)",
                                _tenant_id, e
                            );
                        }
                        true
                    }
                }
                Err(e) => {
                    warn!("tenant_budget lock poisoned: {e}");
                    // Continue without budget enforcement — degraded mode
                    true
                }
            }
        };

        if !tenant_budget_ok {
            anyhow::bail!("tenant '{}' at resource limit", _tenant_id,);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Mode-capability cross-validation
// ---------------------------------------------------------------------------

/// Cross-validate message content against the current mode's capabilities.
///
/// Concatenates user messages into a task description, detects patterns
/// that suggest certain execution needs (URLs, code blocks, execution
/// keywords, planning keywords, etc.), maps them to the minimum required
/// [`ModeKind`], and returns an error with a user-friendly recommendation
/// if the current mode is insufficient.
fn validate_mode_capability(mode: &ModeKind, messages: &[Message]) -> Result<()> {
    // Only check the LAST user message to avoid cross-message contamination.
    // Previous assistant responses may contain security-sensitive content
    // (API keys, tokens, secrets) that would incorrectly flag benign follow-
    // up requests as requiring higher privilege modes.
    let task_description: String = messages
        .iter()
        .rfind(|m| m.role == "user")
        .map(|m| m.content.as_str())
        .unwrap_or("")
        .to_string();

    if task_description.is_empty() {
        return Ok(());
    }

    // ── Pattern detection (using pre-compiled LazyLock regexes) ────────────
    let has_url = RE_URL.is_match(&task_description);
    let has_code_block = task_description.contains("```");
    let has_file_ref = RE_FILE_REF.is_match(&task_description);
    let has_execution_keyword = RE_EXEC_KEYWORD.is_match(&task_description);
    let has_security_keyword = RE_SEC_KEYWORD.is_match(&task_description);
    let has_planning_keyword = RE_PLAN_KEYWORD.is_match(&task_description);

    let is_short_question = task_description.len() < 50
        && !has_url
        && !has_code_block
        && !has_file_ref
        && !has_execution_keyword
        && !has_security_keyword
        && !has_planning_keyword;

    // ── Determine minimum required mode ───────────────────────────────────
    let required_mode = if is_short_question {
        ModeKind::Ask
    } else if has_url && has_execution_keyword {
        // URLs combined with execution keywords strongly suggest autonomous
        // operations such as fetching data and processing it.
        ModeKind::FullAuto
    } else if has_url || has_security_keyword {
        // URL fetching and security-sensitive operations require SafeGuard
        // for approval at high-risk nodes.
        ModeKind::SafeGuard
    } else if has_planning_keyword {
        ModeKind::Plan
    } else if has_code_block || has_file_ref || has_execution_keyword {
        // Code blocks, file references, or execution keywords indicate
        // code and file editing needs.
        ModeKind::Edit
    } else {
        // No specific pattern detected — Ask is sufficient.
        ModeKind::Ask
    };

    // ── Capability check ──────────────────────────────────────────────────
    if capability_level(mode) < capability_level(&required_mode) {
        let mode_name = display_name(&required_mode);
        warn!(
            "pre_route_policy: mode '{}' insufficient for detected task, recommending '{}'",
            display_name(mode),
            mode_name,
        );
        anyhow::bail!(
            "The current mode '{}' is not suitable for this request. \
             Please switch to '{}' mode for better results.",
            display_name(mode),
            mode_name,
        );
    }

    Ok(())
}

/// Numeric capability level for mode comparison.
///
/// Higher values represent greater autonomy and execution capability:
///
/// | Level | Mode       | Description                     |
/// |-------|------------|---------------------------------|
/// | 0     | Ask        | Simple Q&A, no execution        |
/// | 1     | Plan       | Planning/design, no execution   |
/// | 2     | Edit       | Code and file editing           |
/// | 3     | SafeGuard  | High-risk execution w/ approval |
/// | 4     | FullAuto   | Full autonomous execution       |
fn capability_level(mode: &ModeKind) -> u8 {
    match mode {
        ModeKind::Ask => 0,
        ModeKind::Plan => 1,
        ModeKind::Edit => 2,
        ModeKind::SafeGuard => 3,
        ModeKind::FullAuto => 4,
    }
}

/// Return the human-readable mode name for user-facing messages.
fn display_name(mode: &ModeKind) -> &'static str {
    match mode {
        ModeKind::Ask => "ask",
        ModeKind::Plan => "plan",
        ModeKind::Edit => "edit",
        ModeKind::FullAuto => "full_auto",
        ModeKind::SafeGuard => "safeguard",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::server::ServerBuilder;
    use crate::agent::Message;

    fn make_chat_params() -> ChatParams {
        ChatParams {
            mode: "edit".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: "Hello, how can you help?".to_string(),
            }],
            conversation_id: None,
            branch_id: None,
            phase: None,
            options: None,
            vector_hits: None,
            plan_output: None,
            model: None,
            temperature: None,
            max_tokens: None,
        }
    }

    // -----------------------------------------------------------------------
    // validate_mode_capability tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_validate_simple_question_ask_mode_ok() {
        let mode = ModeKind::Ask;
        let msgs = vec![Message {
            role: "user".to_string(),
            content: "What is the weather?".to_string(),
        }];
        assert!(validate_mode_capability(&mode, &msgs).is_ok());
    }

    #[test]
    fn test_validate_short_simple_question_in_plan_mode_ok() {
        let mode = ModeKind::Plan;
        let msgs = vec![Message {
            role: "user".to_string(),
            content: "What is the capital of France?".to_string(),
        }];
        assert!(validate_mode_capability(&mode, &msgs).is_ok());
    }

    #[test]
    fn test_validate_code_block_in_ask_mode_fails() {
        let mode = ModeKind::Ask;
        let msgs = vec![Message {
            role: "user".to_string(),
            content: "Please fix this code:\n```rust\nlet x = 1;\n```".to_string(),
        }];
        let result = validate_mode_capability(&mode, &msgs);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("edit"), "error should recommend edit");
    }

    #[test]
    fn test_validate_code_block_in_edit_mode_ok() {
        let mode = ModeKind::Edit;
        let msgs = vec![Message {
            role: "user".to_string(),
            content: "Fix:\n```rust\nlet x = 1;\n```".to_string(),
        }];
        assert!(validate_mode_capability(&mode, &msgs).is_ok());
    }

    #[test]
    fn test_validate_file_ref_in_ask_mode_fails() {
        let mode = ModeKind::Ask;
        let msgs = vec![Message {
            role: "user".to_string(),
            content: "Update src/main.rs to add a new route.".to_string(),
        }];
        let result = validate_mode_capability(&mode, &msgs);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("edit"), "error should recommend edit");
    }

    #[test]
    fn test_validate_url_in_ask_mode_fails() {
        let mode = ModeKind::Ask;
        let msgs = vec![Message {
            role: "user".to_string(),
            content: "Fetch https://api.example.com/data and parse it.".to_string(),
        }];
        let result = validate_mode_capability(&mode, &msgs);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("safeguard"),
            "error should recommend safeguard"
        );
    }

    #[test]
    fn test_validate_url_in_safeguard_mode_ok() {
        let mode = ModeKind::SafeGuard;
        let msgs = vec![Message {
            role: "user".to_string(),
            content: "Fetch https://api.example.com/data.".to_string(),
        }];
        assert!(validate_mode_capability(&mode, &msgs).is_ok());
    }

    #[test]
    fn test_validate_execution_keyword_in_edit_mode_ok() {
        let mode = ModeKind::Edit;
        let msgs = vec![Message {
            role: "user".to_string(),
            content: "Create a new function that validates email addresses.".to_string(),
        }];
        assert!(validate_mode_capability(&mode, &msgs).is_ok());
    }

    #[test]
    fn test_validate_execution_keyword_in_ask_mode_fails() {
        let mode = ModeKind::Ask;
        let msgs = vec![Message {
            role: "user".to_string(),
            content: "Implement a sorting algorithm in Rust.".to_string(),
        }];
        let result = validate_mode_capability(&mode, &msgs);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("edit"), "error should recommend edit");
    }

    #[test]
    fn test_validate_planning_keyword_in_ask_mode_fails() {
        let mode = ModeKind::Ask;
        let msgs = vec![Message {
            role: "user".to_string(),
            content: "Design a microservices architecture for our platform.".to_string(),
        }];
        let result = validate_mode_capability(&mode, &msgs);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("plan"), "error should recommend plan");
    }

    #[test]
    fn test_validate_planning_keyword_in_plan_mode_ok() {
        let mode = ModeKind::Plan;
        let msgs = vec![Message {
            role: "user".to_string(),
            content: "Design a database schema for the new feature.".to_string(),
        }];
        assert!(validate_mode_capability(&mode, &msgs).is_ok());
    }

    #[test]
    fn test_validate_url_plus_execution_suggests_full_auto() {
        let mode = ModeKind::Edit;
        let msgs = vec![Message {
            role: "user".to_string(),
            content: "Fetch https://api.github.com/repos and create a summary.".to_string(),
        }];
        let result = validate_mode_capability(&mode, &msgs);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("full_auto"),
            "error should recommend full_auto, got: {err}"
        );
    }

    #[test]
    fn test_validate_full_auto_handles_url_plus_execution_ok() {
        let mode = ModeKind::FullAuto;
        let msgs = vec![Message {
            role: "user".to_string(),
            content: "Fetch https://api.github.com/repos and create a summary.".to_string(),
        }];
        assert!(validate_mode_capability(&mode, &msgs).is_ok());
    }

    #[test]
    fn test_validate_security_keyword_in_edit_mode_fails() {
        let mode = ModeKind::Edit;
        let msgs = vec![Message {
            role: "user".to_string(),
            content: "Rotate the API token for the production service.".to_string(),
        }];
        let result = validate_mode_capability(&mode, &msgs);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("safeguard"),
            "error should recommend safeguard, got: {err}"
        );
    }

    #[test]
    fn test_validate_security_keyword_in_safeguard_mode_ok() {
        let mode = ModeKind::SafeGuard;
        let msgs = vec![Message {
            role: "user".to_string(),
            content: "Update the SSH key for the deployment server.".to_string(),
        }];
        assert!(validate_mode_capability(&mode, &msgs).is_ok());
    }

    #[test]
    fn test_validate_empty_messages_always_ok() {
        let mode = ModeKind::Ask;
        assert!(validate_mode_capability(&mode, &[]).is_ok());
        assert!(validate_mode_capability(&ModeKind::Edit, &[]).is_ok());
        assert!(validate_mode_capability(&ModeKind::SafeGuard, &[]).is_ok());
        assert!(validate_mode_capability(&ModeKind::FullAuto, &[]).is_ok());
        assert!(validate_mode_capability(&ModeKind::Plan, &[]).is_ok());
    }

    #[test]
    fn test_validate_only_assistant_messages_ok() {
        let mode = ModeKind::Ask;
        let msgs = vec![Message {
            role: "assistant".to_string(),
            content: "Here is some code:\n```rust\nlet x = 1;\n```".to_string(),
        }];
        // Only user messages are concatenated; assistant messages are ignored.
        assert!(validate_mode_capability(&mode, &msgs).is_ok());
    }

    #[test]
    fn test_validate_long_complex_question_no_pattern_ok() {
        // Long but without specific patterns — Ask is sufficient.
        let mode = ModeKind::Ask;
        let msgs = vec![Message {
            role: "user".to_string(),
            content: "I was wondering if you could tell me more about the history of \
                       the Roman Empire and its impact on modern European culture \
                       and legal systems."
                .to_string(),
        }];
        assert!(validate_mode_capability(&mode, &msgs).is_ok());
    }

    // -----------------------------------------------------------------------
    // Integration: validate_mode_capability is called at the START of
    // evaluate_pre_route_policies. The tests below verify that mode-mode
    // mismatches are caught BEFORE the HarnessBus stage.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_evaluate_pre_route_policies_with_empty_server() {
        let server = ServerBuilder::new().build();
        let params = make_chat_params();

        let result = evaluate_pre_route_policies(&server, &params, "test-tenant").await;

        // With no harness_bus configured, only the mode-capability check
        // and tenant budget check run. The default "ask" query should pass.
        assert!(
            result.is_ok(),
            "pre-route policies should pass with empty server config"
        );
        result.unwrap();
    }

    #[tokio::test]
    async fn test_evaluate_pre_route_policies_different_tenant() {
        let server = ServerBuilder::new().build();
        let params = make_chat_params();

        let result = evaluate_pre_route_policies(&server, &params, "tenant-42").await;
        assert!(result.is_ok(), "should work for any tenant id");
        result.unwrap();
    }

    #[tokio::test]
    async fn test_evaluate_pre_route_policies_with_multiple_messages() {
        let server = ServerBuilder::new().build();
        let params = ChatParams {
            messages: vec![
                Message {
                    role: "user".to_string(),
                    content: "Fix bug #123".to_string(),
                },
                Message {
                    role: "assistant".to_string(),
                    content: "Looking into bug #123...".to_string(),
                },
            ],
            ..make_chat_params()
        };

        let result = evaluate_pre_route_policies(&server, &params, "test-tenant").await;
        assert!(result.is_ok(), "should handle multiple messages");
        result.unwrap();
    }

    #[tokio::test]
    async fn test_pre_route_rejects_mode_mismatch() {
        let server = ServerBuilder::new().build();
        let params = ChatParams {
            mode: "ask".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: "Build a new feature in src/feature.rs".to_string(),
            }],
            ..make_chat_params()
        };

        let result = evaluate_pre_route_policies(&server, &params, "test-tenant").await;
        assert!(
            result.is_err(),
            "should reject ask-mode request with build keyword and file ref"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("edit"),
            "error should mention edit mode, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_pre_route_passes_with_correct_mode() {
        let server = ServerBuilder::new().build();
        let params = ChatParams {
            mode: "edit".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: "Build a new feature in src/feature.rs".to_string(),
            }],
            ..make_chat_params()
        };

        let result = evaluate_pre_route_policies(&server, &params, "test-tenant").await;
        assert!(
            result.is_ok(),
            "should pass edit-mode request with build keyword and file ref"
        );
        result.unwrap();
    }

    #[tokio::test]
    async fn test_pre_route_blocked_by_safeguard_mismatch() {
        let server = ServerBuilder::new().build();
        let params = ChatParams {
            mode: "edit".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: "Fetch https://api.example.com/data and parse the response.".to_string(),
            }],
            ..make_chat_params()
        };

        let result = evaluate_pre_route_policies(&server, &params, "test-tenant").await;
        assert!(
            result.is_err(),
            "should reject edit-mode request with URL fetching"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("safeguard"),
            "error should recommend safeguard, got: {err}"
        );
    }

    // -----------------------------------------------------------------------
    // Unit tests for helper functions
    // -----------------------------------------------------------------------

    #[test]
    fn test_capability_level_ordering() {
        assert!(capability_level(&ModeKind::Ask) < capability_level(&ModeKind::Plan));
        assert!(capability_level(&ModeKind::Plan) < capability_level(&ModeKind::Edit));
        assert!(capability_level(&ModeKind::Edit) < capability_level(&ModeKind::SafeGuard));
        assert!(capability_level(&ModeKind::SafeGuard) < capability_level(&ModeKind::FullAuto));
    }

    #[test]
    fn test_display_name_all_variants() {
        assert_eq!(display_name(&ModeKind::Ask), "ask");
        assert_eq!(display_name(&ModeKind::Plan), "plan");
        assert_eq!(display_name(&ModeKind::Edit), "edit");
        assert_eq!(display_name(&ModeKind::FullAuto), "full_auto");
        assert_eq!(display_name(&ModeKind::SafeGuard), "safeguard");
    }
}
