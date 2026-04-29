use super::*;

#[derive(Clone, Debug, Serialize, Default)]
pub(super) struct HardnessDimensions {
    pub context_scale: f64,
    pub cross_file_span: f64,
    pub tool_dependency: f64,
    pub recovery_complexity: f64,
}

#[derive(Clone, Debug, Serialize, Default)]
pub(super) struct HardnessBudgetProfile {
    #[allow(dead_code)]
    pub timeout_seconds: u64,
    pub parallelism_cap: usize,
    pub required_reviews: usize,
    pub recommended_mode: String,
}

#[derive(Clone, Debug, Serialize, Default)]
pub(super) struct HardnessProfile {
    pub score: f64,
    pub normalized: f64,
    pub level: String,
    pub dimensions: HardnessDimensions,
    pub budget: HardnessBudgetProfile,
    pub reasons: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Default)]
pub(super) struct TokenBudgetProfile {
    pub phase: String,
    pub hardness_level: String,
    pub input_tokens_estimate: u64,
    pub output_tokens_budget: u64,
    pub total_tokens_budget: u64,
    pub budget_class: String,
}

#[derive(Clone, Debug, Serialize, Default)]
pub(super) struct TokenCompressionProfile {
    pub enabled: bool,
    pub triggered: bool,
    pub reason: String,
    pub strategy: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Default)]
pub(super) struct CostRoutingProfile {
    pub preferred_model_tier: String,
    pub high_cost_model_allowed: bool,
    pub cooldown_seconds: u64,
    pub degrade_strategies: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Default)]
pub(super) struct CostTelemetryProfile {
    pub estimated_unit_cost: f64,
    pub estimated_total_cost: f64,
    pub total_requests: u64,
    pub failed_requests: u64,
    pub agent_timeout_failures_total: u64,
    pub review_gate_timeout_total: u64,
    pub runtime_probe_timeout_total: u64,
}

#[derive(Clone, Debug, Serialize, Default)]
pub(super) struct TokenCostGovernanceProfile {
    pub policy_version: String,
    pub hardness: HardnessProfile,
    pub budget: TokenBudgetProfile,
    pub compression: TokenCompressionProfile,
    pub routing: CostRoutingProfile,
    pub telemetry: CostTelemetryProfile,
}

pub(super) fn clamp01(value: f64) -> f64 {
    value.clamp(0.0, 1.0)
}

pub(super) fn scale_to_unit(value: f64, max: f64) -> f64 {
    if max <= 0.0 {
        return 0.0;
    }
    clamp01(value / max)
}

pub(super) fn mode_rank(mode: &str) -> u8 {
    match mode.to_ascii_lowercase().as_str() {
        "ask" => 0,
        "edit" => 1,
        "agent" => 2,
        "safeguard" => 3,
        "full_auto" | "full-auto" | "auto" => 4,
        _ => 2,
    }
}

pub(super) fn stricter_execution_mode(primary: &str, fallback: &str) -> String {
    if mode_rank(primary) >= mode_rank(fallback) {
        primary.to_string()
    } else {
        fallback.to_string()
    }
}

pub(super) fn hardness_level_from_score(score: f64) -> &'static str {
    if score >= 76.0 {
        "extreme"
    } else if score >= 56.0 {
        "high"
    } else if score >= 31.0 {
        "medium"
    } else {
        "low"
    }
}

pub(super) fn hardness_budget_for_level(level: &str) -> HardnessBudgetProfile {
    match level {
        "extreme" => HardnessBudgetProfile {
            timeout_seconds: 210,
            parallelism_cap: 1,
            required_reviews: 2,
            recommended_mode: "safeguard".to_string(),
        },
        "high" => HardnessBudgetProfile {
            timeout_seconds: 150,
            parallelism_cap: 2,
            required_reviews: 2,
            recommended_mode: "safeguard".to_string(),
        },
        "medium" => HardnessBudgetProfile {
            timeout_seconds: 90,
            parallelism_cap: 3,
            required_reviews: 1,
            recommended_mode: "agent".to_string(),
        },
        _ => HardnessBudgetProfile {
            timeout_seconds: 45,
            parallelism_cap: 4,
            required_reviews: 1,
            recommended_mode: "edit".to_string(),
        },
    }
}

pub(super) fn hardness_to_complexity(normalized: f64) -> u8 {
    if normalized >= 0.85 {
        5
    } else if normalized >= 0.65 {
        4
    } else if normalized >= 0.45 {
        3
    } else if normalized >= 0.25 {
        2
    } else {
        1
    }
}

pub(super) fn summarize_hardness(task: &str, params: &Value) -> HardnessProfile {
    let task_chars = task.chars().count() as f64;
    let payload_size = serde_json::to_string(params)
        .map(|raw| raw.len() as f64)
        .unwrap_or(0.0);
    let context_scale =
        scale_to_unit(task_chars, 1200.0) * 0.6 + scale_to_unit(payload_size, 6000.0) * 0.4;

    let changed_files = params
        .get("changed_files")
        .and_then(Value::as_array)
        .map(|items| items.len())
        .or_else(|| {
            params
                .get("files")
                .and_then(Value::as_array)
                .map(|items| items.len())
        })
        .unwrap_or(0) as f64;
    let cross_file_span = scale_to_unit(changed_files, 12.0);

    let tool_dependencies = params
        .get("tool_dependencies")
        .and_then(Value::as_array)
        .map(|items| items.len())
        .unwrap_or(0) as f64;
    let requested_tool_loop = params
        .get("lazy_tool_loop")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let tool_dependency = clamp01(
        scale_to_unit(tool_dependencies, 8.0)
            + if requested_tool_loop { 0.15 } else { 0.0 }
            + if task.to_ascii_lowercase().contains("tool") {
                0.1
            } else {
                0.0
            },
    );

    let retry_count = params
        .get("retry_count")
        .and_then(Value::as_u64)
        .unwrap_or(0) as f64;
    let failover_required = params
        .get("requires_failover")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let dual_review_required = params
        .get("dual_review_required")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let recovery_complexity = clamp01(
        scale_to_unit(retry_count, 4.0)
            + if failover_required { 0.25 } else { 0.0 }
            + if dual_review_required { 0.2 } else { 0.0 },
    );

    let normalized = clamp01(
        context_scale * 0.35
            + cross_file_span * 0.25
            + tool_dependency * 0.2
            + recovery_complexity * 0.2,
    );
    let score = normalized * 100.0;
    let level = hardness_level_from_score(score).to_string();
    let budget = hardness_budget_for_level(&level);

    let mut reasons = Vec::new();
    if context_scale >= 0.5 {
        reasons.push("large_context_or_payload".to_string());
    }
    if cross_file_span >= 0.5 {
        reasons.push("cross_file_span_high".to_string());
    }
    if tool_dependency >= 0.5 {
        reasons.push("tool_dependency_high".to_string());
    }
    if recovery_complexity >= 0.5 {
        reasons.push("recovery_complexity_high".to_string());
    }
    if reasons.is_empty() {
        reasons.push("baseline".to_string());
    }

    HardnessProfile {
        score,
        normalized,
        level,
        dimensions: HardnessDimensions {
            context_scale,
            cross_file_span,
            tool_dependency,
            recovery_complexity,
        },
        budget,
        reasons,
    }
}

pub(super) fn estimate_tokens_from_text(raw: &str) -> u64 {
    if raw.trim().is_empty() {
        return 0;
    }
    ((raw.chars().count() as f64) / 3.8).ceil() as u64
}

pub(super) fn resolve_cost_tier(
    level: &str,
    compression_triggered: bool,
) -> (&'static str, f64, f64) {
    if compression_triggered {
        return ("economy", 0.0008, 0.0014);
    }
    match level {
        "extreme" => ("high", 0.0024, 0.0048),
        "high" => ("standard", 0.0015, 0.003),
        "medium" => ("standard", 0.0012, 0.0022),
        _ => ("economy", 0.0008, 0.0014),
    }
}

pub(super) fn summarize_token_cost_governance(
    task: &str,
    params: &Value,
    hardness: HardnessProfile,
    metrics: &crate::acp::prelude::MetricsSnapshot,
) -> TokenCostGovernanceProfile {
    let payload_raw = serde_json::to_string(params).unwrap_or_else(|_| "{}".to_string());
    let task_tokens = estimate_tokens_from_text(task);
    let payload_tokens = estimate_tokens_from_text(payload_raw.as_str());

    let retrieval_fragments = params
        .get("retrieval_fragments")
        .or_else(|| params.get("evidence_fragments"))
        .or_else(|| params.get("chunks"))
        .and_then(Value::as_array)
        .map(|items| items.len() as u64)
        .unwrap_or(0);
    let input_tokens_estimate = task_tokens
        .saturating_add(payload_tokens)
        .saturating_add(retrieval_fragments.saturating_mul(120));

    let phase = params
        .get("phase")
        .and_then(Value::as_str)
        .unwrap_or("execute")
        .to_string();

    let phase_bonus = if matches!(phase.as_str(), "plan" | "research" | "consult") {
        300
    } else {
        0
    };
    let base_output_budget = match hardness.level.as_str() {
        "extreme" => 3400,
        "high" => 2500,
        "medium" => 1600,
        _ => 900,
    } + phase_bonus;

    let explicit_output_budget = params
        .get("max_output_tokens")
        .or_else(|| params.get("output_token_budget"))
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(8000);
    let output_tokens_budget = if explicit_output_budget > 0 {
        explicit_output_budget.max(base_output_budget as u64 / 2)
    } else {
        base_output_budget as u64
    };
    let total_tokens_budget = input_tokens_estimate.saturating_add(output_tokens_budget);
    let budget_class = if total_tokens_budget >= 5200 {
        "critical"
    } else if total_tokens_budget >= 3600 {
        "high"
    } else if total_tokens_budget >= 2000 {
        "medium"
    } else {
        "low"
    }
    .to_string();

    let force_compress = params
        .get("force_compress")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let compression_triggered = force_compress
        || input_tokens_estimate > output_tokens_budget.saturating_mul(2)
        || total_tokens_budget > 4000;
    let compression_reason = if force_compress {
        "forced_by_request"
    } else if input_tokens_estimate > output_tokens_budget.saturating_mul(2) {
        "input_over_output_budget"
    } else if total_tokens_budget > 4000 {
        "total_budget_exceeds_threshold"
    } else {
        "within_budget"
    }
    .to_string();

    let (tier, input_rate, output_rate) =
        resolve_cost_tier(hardness.level.as_str(), compression_triggered);
    let high_cost_model_allowed =
        matches!(hardness.level.as_str(), "high" | "extreme") && !compression_triggered;
    let cooldown_seconds = match hardness.level.as_str() {
        "extreme" => 180,
        "high" => 120,
        "medium" => 60,
        _ => 30,
    };

    let estimated_unit_cost = (input_tokens_estimate as f64 / 1000.0) * input_rate
        + (output_tokens_budget as f64 / 1000.0) * output_rate;
    let estimated_total_cost = estimated_unit_cost.max(0.0);

    TokenCostGovernanceProfile {
        policy_version: "x6-token-cost-v1".to_string(),
        hardness: hardness.clone(),
        budget: TokenBudgetProfile {
            phase,
            hardness_level: hardness.level.clone(),
            input_tokens_estimate,
            output_tokens_budget,
            total_tokens_budget,
            budget_class,
        },
        compression: TokenCompressionProfile {
            enabled: true,
            triggered: compression_triggered,
            reason: compression_reason,
            strategy: vec![
                "rolling_summary".to_string(),
                "dedupe_evidence".to_string(),
                "adaptive_retrieval_window".to_string(),
            ],
        },
        routing: CostRoutingProfile {
            preferred_model_tier: tier.to_string(),
            high_cost_model_allowed,
            cooldown_seconds,
            degrade_strategies: vec![
                "trim_context_first".to_string(),
                "downgrade_model_tier".to_string(),
                "limit_tool_roundtrips".to_string(),
            ],
        },
        telemetry: CostTelemetryProfile {
            estimated_unit_cost,
            estimated_total_cost,
            total_requests: metrics.total_requests,
            failed_requests: metrics.failed_requests,
            agent_timeout_failures_total: metrics.agent_timeout_failures_total,
            review_gate_timeout_total: metrics.review_gate_timeout_total,
            runtime_probe_timeout_total: metrics.runtime_probe_timeout_total,
        },
    }
}
