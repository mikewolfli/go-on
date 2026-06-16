//! Pre-route policy evaluation pipeline stage
//!
//! This module extracts the HarnessBus policy evaluation and tenant budget
//! checks from `process_chat_request` into a standalone function.
//! It runs as the first stage in the chat request pipeline.

use anyhow::Result;
use tracing::{info, warn};

use crate::acp::r#impl::chat::ChatParams;
use crate::acp::server::AcpServer;

/// Evaluate all pre-route policies in order:
///
/// 1. **HarnessBus policy evaluation** — checks project-level policy gates.
/// 2. **Tenant budget check** — checks per-tenant resource quotas (F-GAP-08).
///
/// If any policy denies the request, this function returns an error.
/// Otherwise it returns `Ok(())`.
pub(crate) async fn evaluate_pre_route_policies(
    server: &AcpServer,
    params: &ChatParams,
    _tenant_id: &str,
) -> Result<()> {
    // ── HarnessBus pre-route policy evaluation ─────────────────────────
    // Reset budget clock so long-running backends don't exceed wall clock budget.
    if let Some(ref harness) = server.governance_deps.harness_bus {
        let task_ctx = crate::governance::pua::TaskContext {
            task_type: crate::governance::pua::TaskType::Other,
            file_count: params.messages.len(),
            risk_score: 0.3,
        };
        // Reset budget before evaluation.
        // The budget lock is taken and released in a separate scope so the
        // !Send MutexGuard is dropped BEFORE the .await below.
        {
            let mut budget = match harness.evaluator.budget.lock() {
                Ok(guard) => guard,
                Err(poisoned) => {
                    tracing::warn!("pre_route_policy: budget mutex was poisoned, recovering");
                    poisoned.into_inner()
                }
            };
            budget.reset();
            // MutexGuard dropped here, before .await
        }
        let verdict = harness.evaluate(&task_ctx).await;
        match &verdict {
            crate::governance::harness_bus::PolicyVerdict::Deny(v) => {
                anyhow::bail!("harness policy denied: {}", v.detail);
            }
            crate::governance::harness_bus::PolicyVerdict::Escalate(r) => {
                warn!("harness policy escalation: {}", r.reason);
                // Continue with degraded mode — the runtime will apply
                // stricter constraints via AgentExecutionPolicy later.
            }
            crate::governance::harness_bus::PolicyVerdict::Review(r) => {
                info!("harness policy flagged for review: {}", r.reason);
            }
            _ => {
                warn!("unexpected PolicyVerdict variant in gate evaluation");
            }
        }
    }

    // ── RateLimitMiddleware tenant-level rate limit (F-GAP-49) ───────-
    // Check per-tenant token bucket rate limits alongside phase-level rate
    // limiting (PhaseRateLimiter). Returns 429 with retry-after when exceeded.
    #[cfg(feature = "multi-users-server")]
    {
        if let Some(ref limiter) = server.rate_limiting.rate_limit_middleware {
            if let Err(retry_after) = limiter.check(_tenant_id).await {
                anyhow::bail!(
                    "rate limited for tenant '{}': retry after {}s",
                    _tenant_id,
                    retry_after,
                );
            }
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
                    if server.runtime_config.production_strict {
                        if let Err(e) = budget.check_can_start(_tenant_id) {
                            warn!("tenant budget limit reached for {}: {}", _tenant_id, e);
                            false
                        } else {
                            budget.start_task(_tenant_id);
                            true
                        }
                    } else {
                        // Non-strict mode: warn but allow through.
                        if let Err(e) = budget.check_can_start(_tenant_id) {
                            warn!(
                                "tenant budget limit reached for {}: {} (non-strict, allowing)",
                                _tenant_id, e
                            );
                        }
                        budget.start_task(_tenant_id);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::server::ServerBuilder;
    use crate::agent::Message;

    fn make_chat_params() -> ChatParams {
        ChatParams {
            mode: "ask".to_string(),
            messages: vec![Message {
                role: "user".to_string(),
                content: "Hello, how can you help?".to_string(),
            }],
            conversation_id: None,
            branch_id: None,
            phase: None,
            options: None,
            requirement_contract: None,
            plan: None,
            vector_hits: None,
            execution_decision_candidate: None,
        }
    }

    #[tokio::test]
    async fn test_evaluate_pre_route_policies_with_empty_server() {
        let server = ServerBuilder::new().build();
        let params = make_chat_params();

        let result = evaluate_pre_route_policies(&server, &params, "test-tenant").await;

        // With no harness_bus configured, only the tenant budget check runs.
        // In non-strict mode (default), tenant budget should pass.
        assert!(
            result.is_ok(),
            "pre-route policies should pass with empty server config"
        );
    }

    #[tokio::test]
    async fn test_evaluate_pre_route_policies_different_tenant() {
        let server = ServerBuilder::new().build();
        let params = make_chat_params();

        let result = evaluate_pre_route_policies(&server, &params, "tenant-42").await;
        assert!(result.is_ok(), "should work for any tenant id");
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
    }
}
