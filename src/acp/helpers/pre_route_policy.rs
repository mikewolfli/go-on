//! Pre-route policy evaluation pipeline stage
//!
//! This module extracts the HarnessBus policy evaluation, token gate checks,
//! and tenant budget checks from `process_chat_request` into a standalone function.
//! It runs as the first stage in the chat request pipeline.

use anyhow::Result;
use tracing::{debug, info, warn};

use crate::acp::r#impl::chat::ChatParams;
use crate::acp::server::AcpServer;
use crate::rpc_protocol::RequestTraceContext;

/// Result of pre-route policy evaluation.
/// When all policies pass, the function returns `Ok(())`.
/// On failure, the caller should abort the request with the returned error.
#[derive(Debug)]
pub struct PreRoutePolicyResult;

/// Evaluate all pre-route policies in order:
///
/// 1. **HarnessBus policy evaluation** — checks project-level policy gates.
/// 2. **Token gate evaluation** — checks L0-L5 token layer chain (ARCH-04).
/// 3. **Tenant budget check** — checks per-tenant resource quotas (F-GAP-08).
///
/// If any policy denies the request, this function returns an error.
/// Otherwise it returns `Ok(())`.
pub(crate) async fn evaluate_pre_route_policies(
    server: &AcpServer,
    params: &ChatParams,
    trace: &RequestTraceContext,
    tenant_id: &str,
) -> Result<PreRoutePolicyResult> {
    // ── HarnessBus pre-route policy evaluation ─────────────────────────
    // Reset budget clock so long-running backends don't exceed wall clock budget.
    if let Some(ref harness) = server.harness_bus {
        if let Ok(mut budget) = harness.evaluator.budget.lock() {
            budget.reset();
        }
        let task_ctx = crate::governance::pua::TaskContext {
            task_type: crate::governance::pua::TaskType::Other,
            file_count: params.messages.len(),
            risk_score: 0.3,
        };
        let verdict = harness.evaluate(&task_ctx);
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

    // ── HarnessBus token gate evaluation (ARCH-04) ─────────────────────
    // Evaluate the L0-L5 token layer chain to determine the routing tier
    // for this request.  The evaluation updates per-layer counters that are
    // exposed in governance.status as layered_token_trigger_profile.  A
    // Reject verdict from L0 stops processing immediately; other verdicts
    // are informational and do not block execution.
    if let Some(ref harness) = server.harness_bus {
        let input_chars: usize = params.messages.iter().map(|m| m.content.len()).sum();
        let estimated_input = (input_chars / 4).max(1) as u64;
        let gate_ctx = crate::orchestration::token_layers::GateContext {
            request_id: trace.request_id.clone(),
            estimated_input_tokens: estimated_input,
            estimated_output_tokens: estimated_input / 2,
            keywords: vec![],
            has_cache_hit: false,
            confidence_score: 0.8,
            request_text: String::new(),
            max_input_tokens: None,
            max_output_tokens: None,
        };
        let verdict = harness.evaluate_token_gate(&gate_ctx);
        if matches!(
            verdict,
            crate::orchestration::token_layers::TokenGateVerdict::Reject(_)
        ) {
            let reason = match verdict {
                crate::orchestration::token_layers::TokenGateVerdict::Reject(r) => r,
                _ => "token gate rejected".to_string(),
            };
            anyhow::bail!("token gate L0 rejected request: {}", reason);
        }
        debug!("token gate verdict: {:?}", verdict);
    }

    // ── TenantBudgetEnforcer pre-route check (F-GAP-08) ───────────────
    // Check per-tenant resource quotas before allocating compute.
    // Uses the tenant_id resolved from the ChatRequestContext (which comes
    // from the user session when auth is enabled, or falls back to default).
    let tenant_budget_ok = {
        let budget_guard = server.tenant_budget.lock();
        match budget_guard {
            Ok(mut budget) => {
                if server.runtime_config.production_strict {
                    if let Err(e) = budget.check_can_start(tenant_id) {
                        warn!("tenant budget limit reached for {}: {}", tenant_id, e);
                        false
                    } else {
                        budget.start_task(tenant_id);
                        true
                    }
                } else {
                    // Non-strict mode: warn but allow through.
                    if let Err(e) = budget.check_can_start(tenant_id) {
                        warn!(
                            "tenant budget limit reached for {}: {} (non-strict, allowing)",
                            tenant_id, e
                        );
                    }
                    budget.start_task(tenant_id);
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
        anyhow::bail!("tenant '{}' at resource limit", tenant_id,);
    }

    Ok(PreRoutePolicyResult)
}
