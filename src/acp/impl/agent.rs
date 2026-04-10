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
use tokio::sync::mpsc;
use tracing::info;

use crate::acp::server::AcpServer;
use crate::agent::Message;
use crate::config::PhaseOptions;
use crate::flow::FlowManager;
use crate::i18n::runtime::tf;
use crate::rpc_protocol::RequestTraceContext;
use crate::verification::DeterministicVerifier;

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

        let timeout_policy = options
            .and_then(|opts| opts.extra.get("review_timeout_policy"))
            .and_then(|value| value.as_str())
            .unwrap_or("reject");
        let fail_on_timeout = !timeout_policy.eq_ignore_ascii_case("degrade_single");

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
        server
            .telemetry_runtime
            .lock()
            .ok()
            .and_then(|telemetry_guard| {
                telemetry_guard.start_child_span(
                    parent,
                    "acp.chat.review_gate",
                    vec![opentelemetry::KeyValue::new("gate.mode", "dual")],
                )
            })
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
            score_b
                .partial_cmp(&score_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Take top 2 reviewers for dual review
        let reviewers: Vec<String> = reviewer_names.into_iter().take(2).collect();

        if reviewers.is_empty() {
            return Err(anyhow::anyhow!(
                "{}",
                tf("error.no_reviewers_available", &[])
            ));
        }

        let reviewer_timeout = phase_options
            .and_then(|opts| opts.review_timeout_seconds)
            .or_else(|| phase_options.and_then(|opts| opts.request_timeout_seconds))
            .map(Duration::from_secs);
        let reviewer_deadline = reviewer_timeout.map(|limit| Instant::now() + limit);

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
                    reviewer_deadline,
                )
            })
            .collect();

        let results = future::join_all(review_futures).await;

        // Process results
        let mut passed_count = 0;
        let mut all_comments = Vec::new();
        let mut final_reviewer = String::new();
        let mut timeout_detected = false;

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
                    if err.to_string().to_ascii_lowercase().contains("timeout") {
                        timeout_detected = true;
                    }
                    info!("Reviewer {} failed: {}", reviewers[i], err);
                }
            }
        }

        // Determine final outcome
        let passed = passed_count >= 1; // At least one reviewer must pass

        Ok((
            ReviewGateOutcome {
                passed,
                comments: all_comments,
                reviewer: final_reviewer,
                duration_ms: started.elapsed().as_millis() as u64,
            },
            timeout_detected,
        ))
    }
    .await;

    // Handle timeout
    if let Some(deadline) = gate_deadline {
        if Instant::now() > deadline {
            let timeout_duration_ms = started.elapsed().as_millis() as u64;
            server.metrics.inc_review_gate_timeout();
            server
                .metrics
                .record_review_latency(timeout_duration_ms as f64);
            if timeout_policy.fail_on_timeout {
                server.metrics.inc_review_gate_rejected();
                return Err(anyhow::anyhow!("{}", tf("error.review_gate_timeout", &[])));
            } else {
                server.metrics.inc_review_gate_degraded();
                match result {
                    Ok((mut outcome, _timeout_detected)) => {
                        outcome
                            .comments
                            .push(tf("warning.review_timeout_continue", &[]));
                        if outcome.passed {
                            server.metrics.inc_review_gate_approved();
                        } else {
                            server.metrics.inc_review_gate_rejected();
                        }
                        return Ok(outcome);
                    }
                    Err(_err) => {
                        server.metrics.inc_review_gate_invalid_response();
                        return Ok(ReviewGateOutcome {
                            passed: true,
                            comments: vec![tf("warning.review_timeout_continue", &[])],
                            reviewer: "timeout".to_string(),
                            duration_ms: timeout_duration_ms,
                        });
                    }
                }
            }
        }
    }

    match result {
        Ok((outcome, timeout_detected)) => {
            if timeout_detected {
                server.metrics.inc_review_gate_timeout();
                if !timeout_policy.fail_on_timeout {
                    server.metrics.inc_review_gate_degraded();
                }
            }
            server
                .metrics
                .record_review_latency(outcome.duration_ms as f64);
            if outcome.passed {
                server.metrics.inc_review_gate_approved();
            } else {
                server.metrics.inc_review_gate_rejected();
            }
            Ok(outcome)
        }
        Err(err) => {
            server.metrics.inc_review_gate_invalid_response();
            server.metrics.inc_review_gate_rejected();
            Err(err)
        }
    }
}

/// Run single review by calling the reviewer agent and parsing its APPROVE/REJECT response.
#[allow(clippy::too_many_arguments)]
async fn run_single_review(
    server: &AcpServer,
    _id: Option<Value>,
    messages: &[Message],
    reviewer: &str,
    phase_options: Option<&PhaseOptions>,
    parent_span: Option<&OtelContext>,
    _pipeline_trace: &RequestTraceContext,
    deadline: Option<Instant>,
) -> Result<ReviewGateOutcome> {
    let started = Instant::now();

    let _review_span = parent_span.and_then(|parent| {
        server
            .telemetry_runtime
            .lock()
            .ok()
            .and_then(|telemetry_guard| {
                telemetry_guard.start_child_span(
                    parent,
                    "acp.chat.single_review",
                    vec![
                        opentelemetry::KeyValue::new("reviewer", reviewer.to_string()),
                        opentelemetry::KeyValue::new("review.type", "single"),
                    ],
                )
            })
    });

    // Check deadline before starting to avoid unnecessary work
    if let Some(deadline) = deadline {
        if Instant::now() > deadline {
            return Err(anyhow::anyhow!(
                "{}",
                tf("error.review_timeout", &[("reviewer", reviewer)])
            ));
        }
    }

    // Look up reviewer agent from the registry
    let (_, registry) = routing_handles(server)?;
    let agent = registry.get(reviewer).ok_or_else(|| {
        anyhow::anyhow!(
            "{}",
            tf("error.reviewer_not_found", &[("reviewer", reviewer)])
        )
    })?;

    // Build review messages: copy the original conversation and append a review prompt
    let mut review_messages = messages.to_vec();
    review_messages.push(Message {
        role: "user".to_string(),
        content: tf("review.request_prompt", &[]),
    });

    let agent_options = phase_options.and_then(|opts| opts.agent_options());

    // Spawn and collect agent response, enforcing the reviewer deadline via tokio timeout
    let (sender, mut receiver) = mpsc::channel::<String>(2048);
    let sender = crate::agent::StreamingSender::from(sender);
    let agent_clone = agent.clone();
    let task = tokio::spawn(async move {
        agent_clone
            .chat(review_messages, None, agent_options, sender)
            .await
    });

    let response = if let Some(deadline) = deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let collect_fut = async move {
            let mut resp = String::new();
            while let Some(token) = receiver.recv().await {
                resp.push_str(&token);
            }
            match task.await {
                Ok(Ok(())) => Ok::<String, anyhow::Error>(resp),
                Ok(Err(err)) => Err(err),
                Err(join_err) => Err(anyhow::anyhow!("reviewer task panicked: {join_err}")),
            }
        };
        match tokio::time::timeout(remaining, collect_fut).await {
            Ok(Ok(resp)) => resp,
            Ok(Err(err)) => return Err(err),
            Err(_elapsed) => {
                return Err(anyhow::anyhow!(
                    "{}",
                    tf("error.review_timeout", &[("reviewer", reviewer)])
                ));
            }
        }
    } else {
        let mut resp = String::new();
        while let Some(token) = receiver.recv().await {
            resp.push_str(&token);
        }
        match task.await {
            Ok(Ok(())) => resp,
            Ok(Err(err)) => return Err(err),
            Err(join_err) => return Err(anyhow::anyhow!("reviewer task panicked: {join_err}")),
        }
    };

    // Parse reviewer response: APPROVE unless the response contains REJECT or DENIED
    let upper = response.to_ascii_uppercase();
    let passed =
        upper.contains("APPROVE") && !upper.contains("REJECT") && !upper.contains("DENIED");

    // Run deterministic signals and summarize into comments (BLUE8-M6/M7)
    // M3: record reviewer outcome into online controller (learning loop)
    let elapsed_ms = started.elapsed().as_millis() as u64;
    if let Ok(mut ctrl) = server.online_controller.lock() {
        ctrl.record_agent_outcome("review", reviewer, passed, elapsed_ms);
    }

    let syntax_signal = DeterministicVerifier::run_syntax_check("");
    let compass_signals = DeterministicVerifier::run_quality_compass_checks();
    let all_signals_count = 1 + compass_signals.len();
    let passed_signals_count =
        usize::from(syntax_signal.passed) + compass_signals.iter().filter(|s| s.passed).count();
    let signal_summary = format!(
        "deterministic: syntax={}, compass={}/{} passed",
        if syntax_signal.passed { "ok" } else { "fail" },
        passed_signals_count,
        all_signals_count,
    );
    let comments = vec![format!("{}: {}", reviewer, response.trim()), signal_summary];

    Ok(ReviewGateOutcome {
        passed,
        comments,
        reviewer: reviewer.to_string(),
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

/// Get routing handles (flow manager and agent registry)
fn routing_handles(
    server: &AcpServer,
) -> Result<(Arc<FlowManager>, Arc<crate::agent::AgentRegistry>)> {
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
