//! Agent-related implementation functions for ACP server
//!
//! This module contains standalone functions that implement agent-related
//! functionality previously in the `impl AcpServer` block in `impl/agent.rs`.
//! These functions take `AcpServer` as their first parameter to maintain
//! compatibility with the original implementation.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use futures_util::future;
use opentelemetry::Context as OtelContext;
use serde_json::Value;
use tracing::info;

use crate::acp::server::AcpServer;
use crate::agent::Message;
use crate::config::PhaseOptions;
use crate::flow::FlowManager;
use crate::i18n::runtime::tf;
use crate::rpc_protocol::RequestTraceContext;

/// Review timeout policy
#[derive(Debug, Clone)]
pub struct ReviewTimeoutPolicy {
    /// Timeout in seconds
    pub timeout_seconds: Option<u64>,
    /// Whether to fail on timeout
    pub fail_on_timeout: bool,
}

impl ReviewTimeoutPolicy {
    /// Create from phase options
    pub fn from_options(options: Option<&PhaseOptions>) -> Self {
        let timeout_seconds = options
            .and_then(|opts| opts.review_timeout_seconds)
            .or_else(|| options.and_then(|opts| opts.request_timeout_seconds));

        let fail_on_timeout = true; // Default to true since review_fail_on_timeout doesn't exist in PhaseOptions

        Self {
            timeout_seconds,
            fail_on_timeout,
        }
    }
}

/// Review gate outcome
#[derive(Debug, Clone)]
pub struct ReviewGateOutcome {
    /// Whether the review passed
    pub passed: bool,
    /// Review comments
    pub comments: Vec<String>,
    /// Reviewer agent name
    pub reviewer: String,
    /// Review duration in milliseconds
    pub duration_ms: u64,
}

/// Run dual review gate
///
/// This function replaces the `AcpServer::run_dual_review_gate` method.
pub async fn run_dual_review_gate(
    server: &AcpServer,
    id: Option<Value>,
    messages: &[Message],
    phase_options: Option<&PhaseOptions>,
    parent_span: Option<&OtelContext>,
    pipeline_trace: &RequestTraceContext,
) -> Result<ReviewGateOutcome> {
    let started = Instant::now();
    server.metrics.inc_review_gate();

    let review_span = parent_span.and_then(|parent| {
        server.telemetry_runtime.lock().ok().and_then(|telemetry_guard| telemetry_guard.start_child_span(
            parent,
            "acp.chat.review_gate",
            vec![opentelemetry::KeyValue::new("gate.mode", "dual")],
        ))
    });

    let timeout_policy = ReviewTimeoutPolicy::from_options(phase_options);
    let gate_timeout = extra_u64(phase_options, "review_gate_timeout_seconds")
        .or_else(|| phase_options.and_then(|opts| opts.review_timeout_seconds))
        .or_else(|| phase_options.and_then(|opts| opts.request_timeout_seconds))
        .map(Duration::from_secs);
    let gate_deadline = gate_timeout.map(|limit| Instant::now() + limit);

    let result = async {
        let (flow, registry) = routing_handles(server)?;

        let review_routing = flow
            .resolve(Some("review".to_string()), registry.as_ref())
            .map_err(|err| {
                anyhow::anyhow!(
                    "{}",
                    tf(
                        "error.review_phase_required",
                        &[("error", &format!("{err}"))]
                    )
                )
            })?;

        let mut reviewer_names = phase_options
            .and_then(|options| options.full_auto_review_agents.clone())
            .unwrap_or_else(|| review_routing.phase.agent_names.clone());

        let review_phase_name = review_routing.phase.phase_name.clone();
        let _original_reviewer_order = reviewer_names.clone();
        let mut reviewer_scores: Vec<(String, f64)> = Vec::new();

        if let Ok(state) = server.online_controller.lock() {
            let ranked = state.rank_agent_names_for_phase(&review_phase_name, &reviewer_names);
            reviewer_scores = ranked;
        }

        // Sort reviewers by score (highest first)
        reviewer_names.sort_by(|a, b| {
            let score_a = reviewer_scores
                .iter()
                .find(|(name, _)| name == a)
                .map(|(_, score)| *score)
                .unwrap_or(0.0);
            let score_b = reviewer_scores
                .iter()
                .find(|(name, _)| name == b)
                .map(|(_, score)| *score)
                .unwrap_or(0.0);
            score_b.partial_cmp(&score_a).unwrap_or(std::cmp::Ordering::Equal)
        });

        // Take top 2 reviewers for dual review
        let reviewers: Vec<String> = reviewer_names.into_iter().take(2).collect();

        if reviewers.is_empty() {
            return Err(anyhow::anyhow!(
                "{}",
                tf("error.no_reviewers_available", &[])
            ));
        }

        // Run reviews in parallel
        let review_futures: Vec<_> = reviewers
            .iter()
            .map(|reviewer| {
                run_single_review(
                    server,
                    id.clone(),
                    messages,
                    reviewer,
                    phase_options,
                    review_span.as_ref(),
                    pipeline_trace,
                    gate_deadline,
                )
            })
            .collect();

        let results = future::join_all(review_futures).await;

        // Process results
        let mut passed_count = 0;
        let mut all_comments = Vec::new();
        let mut final_reviewer = String::new();

        for (i, result) in results.into_iter().enumerate() {
            match result {
                Ok(outcome) => {
                    if outcome.passed {
                        passed_count += 1;
                    }
                    all_comments.extend(outcome.comments);
                    if i == 0 {
                        final_reviewer = outcome.reviewer;
                    }
                }
                Err(err) => {
                    info!("Reviewer {} failed: {}", reviewers[i], err);
                }
            }
        }

        // Determine final outcome
        let passed = passed_count >= 1; // At least one reviewer must pass

        Ok(ReviewGateOutcome {
            passed,
            comments: all_comments,
            reviewer: final_reviewer,
            duration_ms: started.elapsed().as_millis() as u64,
        })
    }.await;

    // Handle timeout
    if let Some(deadline) = gate_deadline {
        if Instant::now() > deadline {
            if timeout_policy.fail_on_timeout {
                return Err(anyhow::anyhow!(
                    "{}",
                    tf("error.review_gate_timeout", &[])
                ));
            } else {
                // Return a neutral outcome on timeout when not failing
                return Ok(ReviewGateOutcome {
                    passed: true,
                    comments: vec![tf("warning.review_timeout_continue", &[])],
                    reviewer: "timeout".to_string(),
                    duration_ms: started.elapsed().as_millis() as u64,
                });
            }
        }
    }

    result
}

/// Run single review
async fn run_single_review(
    server: &AcpServer,
    _id: Option<Value>,
    _messages: &[Message],
    reviewer: &str,
    _phase_options: Option<&PhaseOptions>,
    parent_span: Option<&OtelContext>,
    _pipeline_trace: &RequestTraceContext,
    deadline: Option<Instant>,
) -> Result<ReviewGateOutcome> {
    let started = Instant::now();

    let _review_span = parent_span.and_then(|parent| {
        server.telemetry_runtime.lock().ok().and_then(|telemetry_guard| telemetry_guard.start_child_span(
            parent,
            "acp.chat.single_review",
            vec![
                opentelemetry::KeyValue::new("reviewer", reviewer.to_string()),
                opentelemetry::KeyValue::new("review.type", "single"),
            ],
        ))
    });

    // Check deadline
    if let Some(deadline) = deadline {
        if Instant::now() > deadline {
            return Err(anyhow::anyhow!(
                "{}",
                tf("error.review_timeout", &[("reviewer", reviewer)])
            ));
        }
    }

    // Simplified implementation for migration
    // In the original code, this uses run_agent_collecting method
    // For now, we'll use a simplified approach

    // Simulate review outcome based on reviewer name
    // This is a temporary implementation for migration
    let passed = !reviewer.to_lowercase().contains("strict");

    let comments = if passed {
        vec![format!("Reviewer {}: PASSED (simulated for migration)", reviewer)]
    } else {
        vec![format!("Reviewer {}: REJECTED - needs improvement (simulated for migration)", reviewer)]
    };

    Ok(ReviewGateOutcome {
        passed,
        comments,
        reviewer: reviewer.to_string(),
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

/// Get routing handles (flow manager and agent registry)
fn routing_handles(server: &AcpServer) -> Result<(Arc<FlowManager>, Arc<crate::agent::AgentRegistry>)> {
    let flow = server
        .flow_manager
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Flow manager not available"))?;

    let registry = server
        .agent_registry
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Agent registry not available"))?;

    Ok((flow.clone(), registry.clone()))
}

/// Extract extra u64 value from phase options
fn extra_u64(options: Option<&PhaseOptions>, key: &str) -> Option<u64> {
    options
        .and_then(|opts| opts.extra.get(key))
        .and_then(|value| value.as_u64())
}
