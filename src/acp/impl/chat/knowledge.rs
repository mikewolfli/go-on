//! Knowledge persistence and session distillation for ACP chat
//!
//! Contains the knowledge persistence, vector memory, session distillation,
//! and supporting helper functions. These were extracted from the parent
//! `chat.rs` to reduce the monolithic file size.

use serde_json::{json, Value};
use tracing::warn;

use crate::acp::server::AcpServer;
use crate::config::PhaseOptions;
use crate::memory_module::{MemoryClass, MemoryEntry};
use crate::orchestration::task_router::TaskRouter;
use crate::reinforcement::{
    persist_knowledge_insight_event, persist_workflow_learning_event, KnowledgeInsightArtifact,
    WorkflowLearningEvent,
};

use crate::acp::r#impl::chat::{
    build_phase_summary, effective_vector_settings, extract_task_description,
    generate_phase_summary_text, latest_user_message, ChatParams,
};

/// Persist knowledge from a completed chat interaction into the memory store
/// and vector store for future retrieval.
pub(crate) async fn persist_chat_knowledge(
    server: &AcpServer,
    conversation_id: &str,
    branch_id: &str,
    phase_name: &str,
    agent_name: &str,
    params: &ChatParams,
    response_text: &str,
) -> Value {
    let task = extract_task_description(&params.messages);
    let request_excerpt = truncate_chars(&task, 240);
    let response_excerpt = truncate_chars(response_text, 320);
    let reusable_insights = derive_reusable_insights(response_text);
    let verification_steps = derive_verification_steps(response_text);
    let confidence = derive_knowledge_confidence(&reusable_insights, &verification_steps);

    let artifact = KnowledgeInsightArtifact {
        generated_at: crate::acp::prelude::now_ts(),
        conversation_id: conversation_id.to_string(),
        branch_id: branch_id.to_string(),
        phase: phase_name.to_string(),
        task: truncate_chars(&task, 200),
        agent: agent_name.to_string(),
        source: "chat".to_string(),
        request_excerpt: request_excerpt.clone(),
        response_excerpt: response_excerpt.clone(),
        reusable_insights: reusable_insights.clone(),
        verification_steps: verification_steps.clone(),
        confidence,
    };

    let memory_class = if confidence >= 0.9 && reusable_insights.len() >= 2 {
        MemoryClass::Semantic
    } else {
        MemoryClass::Episodic
    };
    let memory_class_name = format!("{:?}", memory_class);

    let memory_content = json!({
        "phase": phase_name,
        "conversation_id": conversation_id,
        "branch_id": branch_id,
        "task": artifact.task,
        "reusable_insights": artifact.reusable_insights,
        "verification_steps": artifact.verification_steps,
        "response_excerpt": artifact.response_excerpt,
    })
    .to_string();

    let (promoted_count, retained_entries) = {
        let mut store = server
            .persistence
            .memory_store
            .lock()
            .unwrap_or_else(|poisoned| {
                tracing::warn!("memory store mutex poisoned during persist_chat_knowledge");
                poisoned.into_inner()
            });
        store.store(MemoryEntry {
            id: format!(
                "knowledge-{}-{}",
                crate::shared::timestamps::now_ts_ms(),
                branch_id
            ),
            class: memory_class,
            content: memory_content,
            timestamp: crate::acp::prelude::now_ts().to_string(),
            usefulness: confidence as f32,
            staleness: 0,
            user_id: None,
            session_id: None,
        });
        store.gc();
        let promotion = store.promote();
        let promoted_count = promotion.promoted_count;
        let retained_entries = store.retrieve(MemoryClass::Observation, 256).len()
            + store.retrieve(MemoryClass::Episodic, 256).len()
            + store.retrieve(MemoryClass::Semantic, 256).len()
            + store.retrieve(MemoryClass::ProjectState, 256).len();
        (promoted_count, retained_entries)
    };

    let mut vector_memory_written = false;
    if let Some(vector_store) = server.cache_deps.cache.vector_store.clone() {
        let vector_payload = format!(
            "Task: {}\nInsights:\n{}\nVerification:\n{}\nAnswer:\n{}",
            request_excerpt,
            reusable_insights.join("\n"),
            verification_steps.join("\n"),
            response_excerpt,
        );
        if vector_store
            .upsert(
                phase_name,
                &format!("knowledge:{}:{}", phase_name, request_excerpt),
                &vector_payload,
            )
            .await
            .is_ok()
        {
            server.observability.metrics.record_vector_store();
            vector_memory_written = true;
        }
    }

    let ledger = crate::acp::r#impl::runtime::artifact_ledger(server);
    let artifact_path = persist_knowledge_insight_event(&ledger, artifact, 256)
        .ok()
        .map(|path| path.display().to_string());

    json!({
        "memory_class": memory_class_name,
        "confidence": confidence,
        "reusable_insights": reusable_insights,
        "verification_steps": verification_steps,
        "artifact_path": artifact_path,
        "retained_entries": &retained_entries,
        "promoted_count": &promoted_count,
        "vector_memory_written": vector_memory_written,
    })
}

/// Persist vector memory from a chat interaction.
///
/// Stores the query-response pair in the vector store and, if summary
/// conditions are met, generates and persists an abstractive phase summary.
pub(crate) async fn persist_vector_memory(
    server: &AcpServer,
    phase_name: &str,
    phase_options: Option<&PhaseOptions>,
    params: &ChatParams,
    response_text: &str,
    selected_agent: &str,
) {
    let Some(settings) = effective_vector_settings(server, phase_options).await else {
        return;
    };
    let Some(store) = server.cache_deps.cache.vector_store.clone() else {
        return;
    };
    let Some(query_text) = latest_user_message(&params.messages) else {
        return;
    };

    if let Err(err) = store
        .clone()
        .upsert(phase_name, query_text, response_text)
        .await
    {
        warn!(phase = phase_name, error = %err, "vector upsert failed");
    } else {
        server.observability.metrics.record_vector_store();
    }

    if settings.summary_enabled && params.messages.len() >= settings.summary_trigger_messages {
        let summary_text = generate_phase_summary_text(
            server,
            phase_name,
            phase_options,
            selected_agent,
            &params.messages,
            response_text,
            settings.summary_max_chars,
        )
        .await
        .unwrap_or_else(|| {
            build_phase_summary(&params.messages, response_text, settings.summary_max_chars)
        });
        if !summary_text.is_empty() {
            if let Err(err) = store.upsert_phase_summary(phase_name, &summary_text).await {
                warn!(phase = phase_name, error = %err, "phase summary upsert failed");
            } else {
                server.observability.metrics.record_summary_store();
            }
        }
    }
}

fn derive_reusable_insights(response_text: &str) -> Vec<String> {
    response_text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| line.len() >= 24)
        .filter(|line| !line.starts_with("```") && !line.starts_with('#'))
        .take(4)
        .map(|line| truncate_chars(line, 180))
        .collect()
}

fn derive_verification_steps(response_text: &str) -> Vec<String> {
    response_text
        .lines()
        .map(str::trim)
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            lower.contains("test")
                || lower.contains("verify")
                || lower.contains("clippy")
                || lower.contains("check")
                || lower.contains("build")
        })
        .take(4)
        .map(|line| truncate_chars(line, 160))
        .collect()
}

fn derive_knowledge_confidence(reusable_insights: &[String], verification_steps: &[String]) -> f64 {
    let base = if reusable_insights.is_empty() {
        0.72
    } else {
        0.82
    };
    let verification_bonus = (verification_steps.len().min(3) as f64) * 0.05;
    let insight_bonus = (reusable_insights.len().min(3) as f64) * 0.03;
    (base + verification_bonus + insight_bonus).clamp(0.0, 0.98)
}

/// Truncate by characters with a hard output budget: the input is trimmed
/// and the `...` ellipsis replaces the last 3 characters of the budget, so
/// the result is at most `max_chars` characters. This differs from
/// [`crate::shared::truncate::truncate_chars`] (append-style, no trim) and is
/// kept local to preserve the exact knowledge-artifact excerpt semantics.
pub(crate) fn truncate_chars(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }

    let trimmed = text.trim();
    let mut result = trimmed.chars().take(max_chars).collect::<String>();
    if trimmed.chars().count() > max_chars && max_chars > 1 {
        let keep = max_chars.saturating_sub(3);
        result = trimmed.chars().take(keep).collect::<String>();
        result.push_str("...");
    }
    result
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn persist_session_distillation(
    server: &AcpServer,
    conversation_id: &str,
    branch_id: &str,
    phase_name: &str,
    params: &ChatParams,
    selected_agent: &str,
    candidate_agents: &[String],
    agent_attempts: &[Value],
    response_text: &str,
) -> Value {
    let task = extract_task_description(&params.messages);
    let success_count = agent_attempts
        .iter()
        .filter(|attempt| attempt.get("ok").and_then(Value::as_bool) == Some(true))
        .count();
    let failure_count = agent_attempts.len().saturating_sub(success_count);
    let success_rate = if agent_attempts.is_empty() {
        1.0
    } else {
        success_count as f64 / agent_attempts.len() as f64
    };
    let merged_agents = if candidate_agents.is_empty() {
        vec![selected_agent.to_string()]
    } else {
        candidate_agents.to_vec()
    };
    let distill_params = json!({
        "learning_mode": "adaptive",
        "memory_scope": "task_and_repo",
        "repair_iterations": failure_count,
        "distill_scope": "task_repo_runtime",
        "evolution_mode": "continuous",
    });
    let mut learning_profile = crate::acp::r#impl::request::build_learning_profile(
        "session.distill",
        &task,
        &distill_params,
    );
    if let Some(obj) = learning_profile.as_object_mut() {
        obj.insert(
            "session".to_string(),
            json!({
                "conversation_id": conversation_id,
                "branch_id": branch_id,
                "phase": phase_name,
                "selected_agent": selected_agent,
                "agents_considered": merged_agents,
                "agent_attempts_total": agent_attempts.len(),
                "success_rate": round_metric(success_rate),
            }),
        );
    }

    let mut knowledge_refinement = crate::acp::r#impl::request::build_knowledge_refinement_profile(
        "session.distill",
        &task,
        &distill_params,
        &learning_profile,
    );
    if let Some(obj) = knowledge_refinement.as_object_mut() {
        obj.insert(
            "merge".to_string(),
            json!({
                "selected_agent": selected_agent,
                "agents_considered": candidate_agents,
                "agents_succeeded": success_count,
                "agents_failed": failure_count,
                "shared_epistemic_base_updated": true,
            }),
        );
    }

    let artifact = json!({
        "generated_at": crate::acp::prelude::now_ts(),
        "conversation_id": conversation_id,
        "branch_id": branch_id,
        "phase": phase_name,
        "task": truncate_chars(&task, 200),
        "selected_agent": selected_agent,
        "merged_agents": merged_agents,
        "learning_profile": learning_profile,
        "knowledge_refinement": knowledge_refinement,
        "response_excerpt": truncate_chars(response_text, 240),
    });

    let ledger = crate::acp::r#impl::runtime::artifact_ledger(server);
    let artifact_path = ledger
        .write_json("spec", "latest-session-distillation.json", &artifact)
        .ok()
        .map(|path| path.display().to_string());

    let insight = KnowledgeInsightArtifact {
        generated_at: crate::acp::prelude::now_ts(),
        conversation_id: conversation_id.to_string(),
        branch_id: branch_id.to_string(),
        phase: phase_name.to_string(),
        task: truncate_chars(&task, 200),
        agent: "multi-agent.merge".to_string(),
        source: "session_distillation".to_string(),
        request_excerpt: truncate_chars(&task, 200),
        response_excerpt: truncate_chars(response_text, 240),
        reusable_insights: vec![format!(
            "Selected agent '{}' after considering {} agent(s)",
            selected_agent,
            candidate_agents.len().max(1)
        )],
        verification_steps: agent_attempts
            .iter()
            .filter_map(|attempt| attempt.get("error").and_then(Value::as_str))
            .take(3)
            .map(|error| truncate_chars(error, 120))
            .collect(),
        confidence: round_metric((0.70 + success_rate * 0.25).clamp(0.0, 0.98)),
    };
    let knowledge_artifact_path = persist_knowledge_insight_event(&ledger, insight, 256)
        .ok()
        .map(|path| path.display().to_string());

    let analyzed = TaskRouter::analyze_task(&task);
    // Derive clarification metrics from the task's latest requirement contract
    // (same source as the task.execute path). Reports zero when no contract
    // exists for this task yet — honest "no data", never a fake score.
    let clarification_metrics =
        crate::acp::helpers::requirement::resolve_learning_clarification_metrics(
            &ledger,
            &task,
            &distill_params,
        );
    let learning_event = WorkflowLearningEvent {
        generated_at: crate::acp::prelude::now_ts(),
        task: truncate_chars(&task, 200),
        complexity: analyzed.complexity,
        predicted_success_rate: success_rate as f32,
        subtasks_total: agent_attempts.len().max(1),
        subtasks_completed: success_count,
        subtasks_failed: failure_count,
        subtasks_skipped: 0,
        // The chat distillation path has no per-attempt timing data, so
        // serial/critical-path durations are not available (0 = no data).
        serial_work_ms: 0,
        critical_path_ms: 0,
        parallel_speedup: if agent_attempts.len() > 1 { 1.0 } else { 0.0 },
        parallel_efficiency: if agent_attempts.len() > 1 {
            round_metric(success_rate)
        } else {
            1.0
        },
        executor: selected_agent.to_string(),
        source: "session_distillation".to_string(),
        runtime_healthy: server.get_status().lifecycle.is_healthy,
        gates_ok: true,
        work_grade: if failure_count == 0 {
            "A".to_string()
        } else if success_count > 0 {
            "B".to_string()
        } else {
            "C".to_string()
        },
        risk_score: round_metric((1.0 - success_rate).clamp(0.0, 1.0)),
        clarification_rounds: clarification_metrics.rounds,
        clarification_quality_score: clarification_metrics.quality_score,
        requirement_change_count: clarification_metrics.requirement_change_count,
        review_reject_root_cause: String::new(),
        primary_stability_score: round_metric(success_rate),
        secondary_utilization_rate: if agent_attempts.len() > 1 {
            round_metric(
                (agent_attempts.len().saturating_sub(1)) as f64 / agent_attempts.len() as f64,
            )
        } else {
            0.0
        },
        failover_count: failure_count as u32,
        failover_root_cause: agent_attempts
            .iter()
            .filter_map(|attempt| attempt.get("error").and_then(Value::as_str))
            .next()
            .unwrap_or_default()
            .to_string(),
    };
    let learning_artifact_path = persist_workflow_learning_event(&ledger, learning_event, 256)
        .ok()
        .map(|path| path.display().to_string());

    json!({
        "artifact_path": artifact_path,
        "knowledge_artifact_path": knowledge_artifact_path,
        "learning_artifact_path": learning_artifact_path,
        "shared_epistemic_base_updated": true,
        "merged_agents": candidate_agents,
        "success_rate": round_metric(success_rate),
        "learning_profile": artifact["learning_profile"].clone(),
        "knowledge_refinement": artifact["knowledge_refinement"].clone(),
    })
}

pub(crate) fn round_metric(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}
