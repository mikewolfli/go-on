//! Fallback agent execution for ACP chat
//!
//! Contains the fallback agent execution logic and related types.
//! Extracted from the parent `chat.rs` to reduce the monolithic file size.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::warn;

use crate::acp::helpers::cache_strategy::store_async;
use crate::acp::helpers::context::request_timeout;
use crate::acp::server::{AcpServer, OutcomeEvent};
use crate::agent::Message;
use crate::i18n::runtime::{t, tf};

use crate::acp::r#impl::chat::{
    run_agent_collecting, select_strong_model_id, StreamNotificationContext, StreamObserver,
};
use crate::rpc_protocol::RequestTraceContext;

pub(crate) fn is_quota_or_token_limit_error(error_text: &str) -> bool {
    let text = error_text.to_ascii_lowercase();
    // HTTP 429 rate limit / quota errors
    text.contains("429")
        || text.contains("rate limit")
        || text.contains("quota")
        || text.contains("insufficient_quota")
        // Token limit/exhaustion
        || text.contains("token") && text.contains("limit")
        || text.contains("token") && text.contains("exhaust")
        || text.contains("token") && text.contains("expired")
        || text.contains("token") && text.contains("invalid")
        // Copilot-specific: token refresh failures (401) or GitHub token access issues
        || text.contains("token refresh failed")
        || text.contains("copilot token") && text.contains("401")
        || text.contains("copilot token") && text.contains("403")
        // Billing / credit errors
        || text.contains("billing")
        || text.contains("credit") && text.contains("insufficient")
        // Generic auth errors for API access failures (key issues often masquerade as 401/403)
        || text.contains("unauthorized")
        || text.contains("forbidden") && text.contains("token")
}

/// A job representing a high-risk multi-agent vote task.
type HighRiskVoteJob = (
    String,
    Arc<dyn crate::agent::Agent>,
    HashMap<String, Value>,
    Option<String>,
);

/// Result of executing fallback agents.
pub(crate) struct FallbackExecutionResult {
    pub selected_agent: String,
    pub response_text: String,
    pub reasoning_text: String,
    pub selected_model_name: Option<String>,
    pub last_err: Option<anyhow::Error>,
    pub agent_attempts: Vec<Value>,
    pub quota_failed_agents: Vec<String>,
    pub _cache_hit: bool,
    pub high_risk_vote_jobs: Vec<HighRiskVoteJob>,
}

/// Execute fallback agents using parallel execution with a concurrency limit.
///
/// Iterates over resolved agents and calls each concurrently using `tokio::spawn`
/// with a `Semaphore` limiting concurrency to 5. Returns the first successful
/// response or collects all errors for fallback handling.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn execute_fallback_agents(
    server: &AcpServer,
    agent_list: Vec<(String, Arc<dyn crate::agent::Agent>)>,
    phase: &crate::orchestration::flow::ResolvedPhase,
    phase_name: &str,
    agent_messages: Vec<Message>,
    base_agent_options: HashMap<String, Value>,
    stream_observer: Option<StreamObserver>,
    trace: &RequestTraceContext,
    unhealthy_fallback_agent: Option<String>,
    enable_high_risk_multi_agent_vote: bool,
    max_vote_agents: usize,
    tenant_id: &str,
) -> FallbackExecutionResult {
    let mut selected_agent = String::new();
    let mut response_text = String::new();
    let mut reasoning_text = String::new();
    let mut selected_model_name: Option<String> = None;
    let mut last_err: Option<anyhow::Error> = None;
    let mut agent_attempts: Vec<Value> = Vec::with_capacity(agent_list.len() + 2);
    let mut quota_failed_agents: Vec<String> = Vec::with_capacity(agent_list.len());
    let mut high_risk_vote_jobs: Vec<HighRiskVoteJob> = Vec::with_capacity(max_vote_agents);

    use futures_util::future::join_all;
    let semaphore = Arc::new(tokio::sync::Semaphore::new(5));
    let mut futures = Vec::with_capacity(agent_list.len());

    for (agent_name, agent) in agent_list.into_iter() {
        // High-risk multi-agent vote collection: skip regular execution
        if enable_high_risk_multi_agent_vote {
            let strong_model = if agent.supports_model_override() {
                select_strong_model_id(agent.as_ref())
            } else {
                None
            };
            let mut vote_options = base_agent_options.clone();
            if let Some(model_id) = strong_model.clone() {
                vote_options.insert("model".to_string(), Value::String(model_id));
            }
            high_risk_vote_jobs.push((
                agent_name.clone(),
                Arc::clone(&agent),
                vote_options,
                strong_model,
            ));
            if high_risk_vote_jobs.len() >= max_vote_agents {
                continue;
            }
            continue;
        }

        let is_unhealthy = server
            .governance_deps
            .capability_bus
            .as_ref()
            .is_some_and(|cb| {
                let unhealthy = !cb.is_agent_healthy(&agent_name);
                if unhealthy && unhealthy_fallback_agent.as_deref() != Some(agent_name.as_str()) {
                    warn!(
                        phase = %phase_name,
                        agent = %agent_name,
                        "skipping unhealthy agent"
                    );
                }
                unhealthy
            });

        if is_unhealthy && unhealthy_fallback_agent.as_deref() != Some(agent_name.as_str()) {
            agent_attempts.push(json!({
                "agent": agent_name,
                "ok": false,
                "skipped_unhealthy": true,
                "duration_ms": 0u64,
                "error": t("error.chat.agent_unhealthy")
            }));
            continue;
        }

        let sem_clone = Arc::clone(&semaphore);
        let agent_name_owned = agent_name.clone();
        let per_attempt_options = base_agent_options.clone();
        let stream_obs = stream_observer.clone();
        let msg_clone = agent_messages.clone();
        let principles = phase.principles.clone();
        let timeout = request_timeout(phase.options.as_ref());
        let _tenant_id_owned = tenant_id.to_string();

        let fut = async move {
            let permit_timeout = std::time::Duration::from_secs(30);
            let _permit = tokio::time::timeout(permit_timeout, sem_clone.acquire())
                .await
                .map_err(|_| {
                    tracing::warn!(
                        "semaphore acquire timed out after {}s",
                        permit_timeout.as_secs()
                    );
                })
                .and_then(|r| {
                    r.map_err(|_| {
                        tracing::warn!("semaphore closed during agent execution — task skipped");
                    })
                });
            let _permit = match _permit {
                Ok(p) => p,
                Err(()) => {
                    return (
                        agent_name_owned,
                        std::time::Instant::now(),
                        Err(anyhow::anyhow!("semaphore closed")),
                    )
                }
            };
            let attempt_started = std::time::Instant::now();
            let stream_ctx = StreamNotificationContext {
                stream_observer: stream_obs,
                agent_name: &agent_name_owned,
                phase_name,
                trace_id: &trace.trace_id,
            };

            // ── Tenant budget check before LLM call (B54-075) ─────────
            #[cfg(feature = "multi-users-server")]
            {
                let budget_guard =
                    server
                        .rate_limiting
                        .tenant_budget
                        .lock()
                        .unwrap_or_else(|poisoned| {
                            warn!("execute_fallback_agents: tenant_budget poisoned, recovering");
                            poisoned.into_inner()
                        });
                if let Err(e) = budget_guard.check_can_start(&_tenant_id_owned) {
                    return (
                        agent_name_owned,
                        attempt_started,
                        Err(anyhow::anyhow!(
                            "tenant '{}' token budget exceeded: {}",
                            _tenant_id_owned,
                            e
                        )),
                    );
                }
            }

            let result = run_agent_collecting(
                server,
                stream_ctx,
                agent,
                msg_clone,
                principles,
                Some(per_attempt_options),
                timeout,
            )
            .await;
            (agent_name_owned, attempt_started, result)
        };
        futures.push(fut);
    }

    // Collect results using JoinAll
    let results = join_all(futures).await;

    for (agent_name, attempt_started, agent_result) in results {
        match agent_result {
            Ok((output_text, reasoning_output, agent_selected_model)) => {
                if output_text.trim().is_empty() {
                    agent_attempts.push(json!({
                        "agent": agent_name,
                        "ok": false,
                        "duration_ms": attempt_started.elapsed().as_millis() as u64,
                        "error": "empty_response",
                    }));
                    let _ = server
                        .resilience
                        .outcome_tx
                        .send(OutcomeEvent::AgentOutcome {
                            phase_name: phase_name.to_string(),
                            agent_name: agent_name.to_string(),
                            success: false,
                            duration_ms: attempt_started.elapsed().as_millis() as u64,
                        });

                    // BLUE56-GAP-B06/B07: Record consciousness metric and self-model on failure
                    if let Some(ref cb) = server.governance_deps.capability_bus {
                        use crate::intelligence::consciousness::AwarenessMetricType;
                        let _ = cb.consciousness.record_metric(
                            AwarenessMetricType::SelfAwareness,
                            0.0,
                            0.8,
                        );
                        cb.self_model.record_execution_result(
                            &agent_name,
                            false,
                            attempt_started.elapsed().as_millis() as u64,
                        );
                        // BLUE56-B08: Record agent failure event in WorldModel
                        let mut payload = std::collections::HashMap::new();
                        payload.insert("status".to_string(), "failure".to_string());
                        payload.insert("phase".to_string(), phase_name.to_string());
                        payload.insert(
                            "duration_ms".to_string(),
                            attempt_started.elapsed().as_millis().to_string(),
                        );
                        let _ =
                            cb.world_model
                                .record_event("agent_execution", &agent_name, payload);
                        // BLUE56-B09: Run TripleFusion fusion cycle after execution
                        // Uses the shared global singleton so fusion_cycles accumulate across requests.
                        let fusion_bridge =
                            crate::intelligence::triple_fusion::global_triple_fusion_bridge();
                        let triggers = fusion_bridge
                            .lock()
                            .await
                            .run_fusion_cycle(&cb.metacognitive, &cb.consciousness);
                        crate::intelligence::fusion_evolution_bridge::send_triggers_to_evolution(
                            triggers,
                        );
                    }
                    // BLUE56-GAP-C04: Record failure in HyperResilienceEngine
                    let _ = server
                        .resilience
                        .hyper_resilience
                        .record_failure_with_mode(
                            &agent_name,
                            crate::resilience::hyper_resilience::FailureMode::ResourceExhaustion,
                        )
                        .await;
                    // BLUE56-B05: Record failure in HotFailover (global singleton)
                    {
                        use crate::intelligence::hot_failover::HOT_FAILOVER_INSTANCE;
                        if let Ok(failover) = HOT_FAILOVER_INSTANCE.lock() {
                            failover.record_failure(&agent_name);
                        }
                    }

                    continue;
                }

                let _ = server
                    .resilience
                    .outcome_tx
                    .send(OutcomeEvent::AgentOutcome {
                        phase_name: phase_name.to_string(),
                        agent_name: agent_name.to_string(),
                        success: true,
                        duration_ms: attempt_started.elapsed().as_millis() as u64,
                    });

                // BLUE56-GAP-B06/B07: Record consciousness metric and self-model on success
                if let Some(ref cb) = server.governance_deps.capability_bus {
                    use crate::intelligence::consciousness::AwarenessMetricType;
                    let _ = cb.consciousness.record_metric(
                        AwarenessMetricType::SelfAwareness,
                        1.0,
                        0.9,
                    );
                    cb.self_model.record_execution_result(
                        &agent_name,
                        true,
                        attempt_started.elapsed().as_millis() as u64,
                    );
                    // BLUE56-B08: Record agent success event in WorldModel
                    let mut payload = std::collections::HashMap::new();
                    payload.insert("status".to_string(), "success".to_string());
                    payload.insert("phase".to_string(), phase_name.to_string());
                    payload.insert(
                        "duration_ms".to_string(),
                        attempt_started.elapsed().as_millis().to_string(),
                    );
                    let _ = cb
                        .world_model
                        .record_event("agent_execution", &agent_name, payload);
                    // BLUE56-B09: Run TripleFusion fusion cycle after execution
                    // Uses the shared global singleton so fusion_cycles accumulate across requests.
                    let fusion_bridge =
                        crate::intelligence::triple_fusion::global_triple_fusion_bridge();
                    let triggers = fusion_bridge
                        .lock()
                        .await
                        .run_fusion_cycle(&cb.metacognitive, &cb.consciousness);
                    crate::intelligence::fusion_evolution_bridge::send_triggers_to_evolution(
                        triggers,
                    );
                }
                // BLUE56-GAP-C04: Record success in HyperResilienceEngine
                let _ = server
                    .resilience
                    .hyper_resilience
                    .record_success(&agent_name)
                    .await;

                agent_attempts.push(json!({
                    "agent": agent_name,
                    "ok": true,
                    "duration_ms": attempt_started.elapsed().as_millis() as u64,
                    "model": agent_selected_model,
                }));
                selected_agent = agent_name.clone();
                response_text = output_text.clone();
                reasoning_text = reasoning_output.clone();
                if let Some(ref m) = agent_selected_model {
                    selected_model_name = Some(m.clone());
                }

                // Store in token cache
                let input_text =
                    crate::intelligence::token_cache::messages_to_text(&agent_messages);
                let token_count =
                    crate::intelligence::token_cache::estimate_token_count(&output_text);
                let cache = server.cache_deps.cache.token_cache.clone();
                let model_name = base_agent_options
                    .get("model")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                store_async(
                    cache,
                    input_text,
                    output_text.clone(),
                    token_count,
                    Some(agent_name.clone()),
                    model_name,
                );

                last_err = None;
                break; // First success wins
            }
            Err(err) => {
                let err_text = err.to_string();
                let quota_limited = is_quota_or_token_limit_error(&err_text);
                if quota_limited {
                    quota_failed_agents.push(agent_name.clone());
                }
                agent_attempts.push(json!({
                    "agent": agent_name,
                    "ok": false,
                    "quota_limited": quota_limited,
                    "duration_ms": attempt_started.elapsed().as_millis() as u64,
                    "error": err_text
                }));
                let _ = server
                    .resilience
                    .outcome_tx
                    .send(OutcomeEvent::AgentOutcome {
                        phase_name: phase_name.to_string(),
                        agent_name: agent_name.to_string(),
                        success: false,
                        duration_ms: attempt_started.elapsed().as_millis() as u64,
                    });
                let agent_label = agent_name.clone();
                let enriched_err = anyhow::anyhow!(tf(
                    "error.chat.agent_error_prefix",
                    &[("agent", &agent_label), ("error", &err.to_string())]
                ));
                last_err = Some(enriched_err);
            }
        }
    }

    FallbackExecutionResult {
        selected_agent,
        response_text,
        reasoning_text,
        selected_model_name,
        last_err,
        agent_attempts,
        quota_failed_agents,
        _cache_hit: false,
        high_risk_vote_jobs,
    }
}
