fn resolve_review_policy(
    options: Option<&PhaseOptions>,
    characteristics: Option<&TaskCharacteristics>,
    is_workflow_execute: bool,
    requested_dual_review: bool,
) -> ReviewPolicy {
    let inferred_enhanced = characteristics
        .map(|c| c.complexity >= 4 || c.has_safety_concerns)
        .unwrap_or(false)
        || is_workflow_execute;

    let min_review_level = extra_string(options, "review_min_level").unwrap_or_else(|| {
        if inferred_enhanced {
            "enhanced".to_string()
        } else {
            "standard".to_string()
        }
    });
    let required_reviews = extra_u64(options, "review_required_reviews")
        .map(|v| v.max(1) as usize)
        .unwrap_or_else(|| {
            if min_review_level.eq_ignore_ascii_case("enhanced") {
                2
            } else {
                1
            }
        });
    let required_checks =
        extra_string_list(options, "review_required_checks").unwrap_or_else(|| {
            if is_workflow_execute {
                vec!["qa".to_string(), "retest".to_string(), "final".to_string()]
            } else {
                Vec::new()
            }
        });
    let timeout_policy =
        extra_string(options, "review_timeout_policy").unwrap_or_else(|| "reject".to_string());
    let enforce_dual_review = requested_dual_review
        || required_reviews >= 2
        || min_review_level.eq_ignore_ascii_case("enhanced");
    let enforce_action_gates = !required_checks.is_empty();

    ReviewPolicy {
        min_review_level,
        required_reviews,
        required_checks,
        timeout_policy,
        enforce_dual_review,
        enforce_action_gates,
    }
}

fn action_check_kinds_from_policy(required_checks: &[String]) -> Vec<ActionCheckKind> {
    if required_checks.is_empty() {
        return Vec::new();
    }

    let mut out = Vec::new();
    for name in required_checks {
        if let Some(kind) = ActionCheckKind::parse(name) {
            if !out.contains(&kind) {
                out.push(kind);
            }
        }
    }
    out
}

impl WorkGrade {
    fn parse(raw: Option<&str>) -> Option<Self> {
        let value = raw?.trim().to_ascii_lowercase();
        match value.as_str() {
            "ask" => Some(Self::Ask),
            "edit" => Some(Self::Edit),
            "agent" => Some(Self::Agent),
            "safeguard" => Some(Self::Safeguard),
            "full_auto" | "full-auto" | "auto" => Some(Self::FullAuto),
            _ => None,
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Edit => "edit",
            Self::Agent => "agent",
            Self::Safeguard => "safeguard",
            Self::FullAuto => "full_auto",
        }
    }

    fn rank(&self) -> u8 {
        match self {
            Self::Ask => 0,
            Self::Edit => 1,
            Self::Agent => 2,
            Self::Safeguard => 3,
            Self::FullAuto => 4,
        }
    }
}

#[derive(Debug, Clone)]
struct WorkGradeDecision {
    requested: WorkGrade,
    decided: WorkGrade,
    decision_action: String,
    reasons: Vec<String>,
    risk_score: f64,
}

fn work_grade_action(requested: WorkGrade, decided: WorkGrade) -> String {
    if decided.rank() > requested.rank() {
        "upgraded".to_string()
    } else if decided.rank() < requested.rank() {
        "downgraded".to_string()
    } else {
        "unchanged".to_string()
    }
}

fn decide_work_grade(
    requested_grade: Option<&str>,
    plan: &crate::reinforcement::TaskPlanArtifact,
    is_workflow_execute: bool,
    runtime_healthy: bool,
    force_fail_fast: bool,
) -> WorkGradeDecision {
    let requested = WorkGrade::parse(requested_grade).unwrap_or({
        if is_workflow_execute {
            WorkGrade::FullAuto
        } else {
            WorkGrade::Agent
        }
    });

    let mut decided = requested;
    let mut reasons = Vec::new();

    let risk_score = ((plan.characteristics.complexity.min(5) as f64 / 5.0) * 0.4
        + if plan.characteristics.has_safety_concerns {
            0.25
        } else {
            0.0
        }
        + if plan.characteristics.involves_multiple_modules {
            0.15
        } else {
            0.0
        }
        + ((1.0 - plan.routing.predicted_success_rate as f64).clamp(0.0, 1.0)) * 0.2
        + if runtime_healthy { 0.0 } else { 0.1 })
    .clamp(0.0, 1.0);

    if force_fail_fast || plan.characteristics.has_safety_concerns || risk_score >= 0.75 {
        decided = WorkGrade::Safeguard;
        reasons.push(
            "high-risk posture detected (safety/fail_fast/high risk score), enforce safeguard"
                .to_string(),
        );
    } else if is_workflow_execute && plan.characteristics.complexity >= 3 {
        decided = WorkGrade::FullAuto;
        reasons
            .push("workflow.execute with moderate+ complexity, promote to full_auto".to_string());
    } else if plan.characteristics.complexity >= 3 {
        decided = WorkGrade::Agent;
        reasons.push("multi-step complexity, promote to agent execution".to_string());
    } else if plan.characteristics.complexity <= 1
        && !plan.characteristics.has_safety_concerns
        && plan.routing.predicted_success_rate >= 0.90
    {
        decided = WorkGrade::Edit;
        reasons.push("low-risk simple task, downgrade to edit for efficiency".to_string());
    }

    let decision_action = work_grade_action(requested, decided);
    WorkGradeDecision {
        requested,
        decided,
        decision_action,
        reasons,
        risk_score,
    }
}

#[derive(Debug, Clone, Serialize)]
struct OptimizationPolicyReport {
    auto_attach: bool,
    auto_detach: bool,
    runtime_healthy: bool,
    anomaly_detected: bool,
    requested_modules: Vec<String>,
    attached_modules: Vec<String>,
    detached_modules: Vec<String>,
    reattached_modules: Vec<String>,
    reattach_reasons: Vec<String>,
    detachment_reasons: Vec<String>,
    module_impacts: Vec<String>,
    recovery_conditions: Vec<String>,
    recommendations: Vec<String>,
    phase_parallelism_cap: Option<usize>,
    force_fail_fast: bool,
    risk_assessment: Value,
    resource_budget: Value,
    dynamic_parameters: Value,
    reliability: Value,
    speed: Value,
    cost: Value,
    anomaly: Value,
}

#[derive(Debug, Clone)]
struct OptimizationPolicyOutcome {
    report: OptimizationPolicyReport,
    phase_parallelism_cap: Option<usize>,
    force_fail_fast: bool,
}

const DEFAULT_OPTIMIZATION_MODULES: &[&str] = &[
    "workflow_optimizer",
    "advanced_modules",
    "reliability_optimizer",
    "failure_prevention",
    "speed_optimizer",
    "cost_optimizer",
    "adaptive_selector",
];

fn evaluate_optimization_policy(
    ledger: &ArtifactLedger,
    task: &str,
    plan: &crate::reinforcement::TaskPlanArtifact,
    options: Option<&PhaseOptions>,
    runtime_healthy: bool,
    is_workflow_execute: bool,
) -> OptimizationPolicyOutcome {
    let auto_attach = extra_bool(options, "auto_attach").unwrap_or(is_workflow_execute);
    let auto_detach = extra_bool(options, "auto_detach").unwrap_or(is_workflow_execute);

    let requested_modules = extra_string_list(options, "optimization_modules")
        .map(|modules| {
            modules
                .into_iter()
                .map(|name| name.trim().to_ascii_lowercase())
                .filter(|name| is_supported_optimization_module(name))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let mut attached_modules = if auto_attach {
        if requested_modules.is_empty() {
            DEFAULT_OPTIMIZATION_MODULES
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<_>>()
        } else {
            requested_modules.clone()
        }
    } else {
        Vec::new()
    };

    attached_modules.sort();
    attached_modules.dedup();

    let mut detached_modules = Vec::new();
    let mut reattached_modules = Vec::new();
    let mut reattach_reasons = Vec::new();
    let mut detachment_reasons = Vec::new();
    let mut module_impacts = Vec::new();
    let mut recovery_conditions = Vec::new();
    let mut recommendations = Vec::new();
    let mut phase_parallelism_cap = None;
    let mut force_fail_fast = false;

    let mut risk_assessment = Value::Null;
    let mut resource_budget = Value::Null;
    let mut dynamic_parameters = Value::Null;
    let mut reliability = Value::Null;
    let mut speed = Value::Null;
    let mut cost = Value::Null;
    let mut anomaly = Value::Null;
    let mut anomaly_detected = false;

    if auto_attach && auto_detach {
        let recoverable = recommend_reattach_modules_from_policy_history(ledger, 2, 40);
        for module in recoverable {
            if is_supported_optimization_module(&module)
                && !attached_modules.iter().any(|attached| attached == &module)
            {
                attached_modules.push(module.clone());
                reattached_modules.push(module.clone());
                reattach_reasons.push(format!(
                    "reattached {} after policy history reported two consecutive healthy, anomaly-free executions",
                    module
                ));
                module_impacts.push(format!(
                    "{} reattached to restore optimization depth under healthy runtime conditions",
                    module
                ));
            }
        }
    }

    let has_module = |name: &str| attached_modules.iter().any(|module| module == name);

    if has_module("workflow_optimizer") {
        let risk = PredictiveFailureHandler::assess_risk(
            task,
            plan.characteristics.complexity,
            plan.characteristics.involves_multiple_modules,
            plan.characteristics.has_safety_concerns,
            plan.routing.predicted_success_rate,
        );
        if risk.use_safeguard_mode {
            force_fail_fast = true;
            recommendations.push(
                "workflow_optimizer recommends fail_fast because risk exceeds safeguard threshold"
                    .to_string(),
            );
            module_impacts.push(
                "failure strategy escalated to fail_fast, reducing throughput but limiting blast radius"
                    .to_string(),
            );
            recovery_conditions.push(
                "switch back to tolerant after consecutive low-risk executions with stable gate pass"
                    .to_string(),
            );
        }
        risk_assessment = serde_json::to_value(&risk).unwrap_or(Value::Null);
    }

    if has_module("advanced_modules") {
        let subtask_count = plan.planned_subtasks.len().max(1);
        let budget = ResourceAllocator::allocate_resources(
            "workflow",
            plan.characteristics.complexity,
            subtask_count,
        );
        let tuner = DynamicParameterTuner::new();
        let profile = match plan.characteristics.complexity {
            0 | 1 => "simple",
            2 | 3 => "medium",
            _ => "complex",
        };
        let tuned = tuner.select_parameters(profile, plan.characteristics.complexity);

        phase_parallelism_cap = Some(budget.max_parallel_tasks.max(1));
        recommendations.push(format!(
            "advanced_modules capped subtask parallelism to {} based on resource budget",
            budget.max_parallel_tasks.max(1)
        ));

        resource_budget = serde_json::to_value(&budget).unwrap_or(Value::Null);
        dynamic_parameters = serde_json::to_value(&tuned).unwrap_or(Value::Null);
    }

    if has_module("reliability_optimizer") {
        let optimizer = ReliabilityOptimizer::new();
        let complexity = optimizer.detect_complexity(task);
        let strategy = optimizer.recommend_strategy(complexity);
        let degradation = optimizer.get_degradation_strategy(complexity);
        if complexity >= ReliabilityComplexityLevel::VeryComplex && degradation.is_some() {
            recommendations.push(
                "reliability_optimizer suggests simplified fallback strategy for very complex task"
                    .to_string(),
            );
        }
        reliability = json!({
            "detected_complexity": format!("{:?}", complexity),
            "recommended_strategy": strategy,
            "degradation_strategy": degradation,
        });
    }

    if has_module("speed_optimizer") {
        let mut optimizer = SpeedOptimizer::new();
        optimizer.enable_speculation(SpeculationStrategy::HistoryBased);
        optimizer.set_streaming_mode(StreamingMode::TokenStreaming);
        let estimated = optimizer.estimate_speedup();
        speed = json!({
            "streaming_mode": format!("{:?}", optimizer.streaming_mode()),
            "estimated_speedup": estimated,
        });
        if estimated > 0.1 {
            recommendations.push(
                "speed_optimizer indicates meaningful acceleration potential on this route"
                    .to_string(),
            );
        }
    }

    if has_module("cost_optimizer") {
        let optimizer = CostOptimizer::new();
        let complexity = match plan.characteristics.complexity {
            0 | 1 => CostTaskComplexity::Simple,
            2 => CostTaskComplexity::Moderate,
            3 | 4 => CostTaskComplexity::Complex,
            _ => CostTaskComplexity::VeryComplex,
        };
        let compressed = optimizer.compress_prompt(task);
        let selected_model = optimizer.select_model(complexity, None);
        cost = json!({
            "selected_model": selected_model,
            "compression_ratio": compressed.compression_ratio,
            "original_tokens": compressed.original_tokens,
            "compressed_tokens": compressed.compressed_tokens,
        });
    }

    if has_module("failure_prevention") {
        let prevention = FailurePrevention::new();
        let detected = prevention.detect_anomaly(task, &HashMap::new());
        anomaly_detected = detected.detected;
        if detected.detected {
            force_fail_fast = true;
            recommendations.push(
                "failure_prevention detected anomaly and escalated failure policy to fail_fast"
                    .to_string(),
            );
            if auto_detach {
                for module in ["speed_optimizer", "cost_optimizer"] {
                    if has_module(module) {
                        detached_modules.push(module.to_string());
                        detachment_reasons.push(format!(
                            "detached {} due to anomaly-driven safety escalation",
                            module
                        ));
                        module_impacts.push(format!(
                            "{} detached, prioritizing safety over latency and cost efficiency",
                            module
                        ));
                        recovery_conditions.push(format!(
                            "reattach {} after runtime.health is healthy and no anomaly is detected for two consecutive executions",
                            module
                        ));
                    }
                }
            }
        }
        anomaly = serde_json::to_value(&detected).unwrap_or(Value::Null);
    }

    if auto_detach && plan.characteristics.complexity <= 1 {
        for module in ["reliability_optimizer", "workflow_optimizer"] {
            if has_module(module) {
                detached_modules.push(module.to_string());
                detachment_reasons.push(format!(
                    "detached {} for low-complexity task to reduce control-plane overhead",
                    module
                ));
                module_impacts.push(format!(
                    "{} detached for low-complexity path, reducing analysis depth to improve response speed",
                    module
                ));
                recovery_conditions.push(format!(
                    "reattach {} when task complexity rises above 1 or cross-module risk is detected",
                    module
                ));
            }
        }
    }

    detached_modules.sort();
    detached_modules.dedup();
    reattached_modules.sort();
    reattached_modules.dedup();
    reattach_reasons.sort();
    reattach_reasons.dedup();
    module_impacts.sort();
    module_impacts.dedup();
    recovery_conditions.sort();
    recovery_conditions.dedup();
    attached_modules.retain(|module| !detached_modules.iter().any(|detached| detached == module));

    let report = OptimizationPolicyReport {
        auto_attach,
        auto_detach,
        runtime_healthy,
        anomaly_detected,
        requested_modules,
        attached_modules,
        detached_modules,
        reattached_modules,
        reattach_reasons,
        detachment_reasons,
        module_impacts,
        recovery_conditions,
        recommendations,
        phase_parallelism_cap,
        force_fail_fast,
        risk_assessment,
        resource_budget,
        dynamic_parameters,
        reliability,
        speed,
        cost,
        anomaly,
    };

    OptimizationPolicyOutcome {
        phase_parallelism_cap,
        force_fail_fast,
        report,
    }
}

fn is_supported_optimization_module(name: &str) -> bool {
    matches!(
        name,
        "workflow_optimizer"
            | "adaptive_selector"
            | "advanced_modules"
            | "cost_optimizer"
            | "speed_optimizer"
            | "reliability_optimizer"
            | "failure_prevention"
    )
}

fn role_keywords_for(role: &str) -> Vec<&'static str> {
    match role {
        "planner" => vec!["planner", "plan", "architect"],
        "researcher" => vec!["researcher", "research", "analysis"],
        "coder" => vec!["coder", "code", "implement", "dev"],
        "tester" => vec!["tester", "test", "qa", "verify"],
        "reviewer" => vec!["reviewer", "review", "audit"],
        _ => vec![],
    }
}

fn rank_execution_agents(
    agent_names: &[String],
    desired_role: Option<&str>,
    phase_index: usize,
    task_index: usize,
) -> Vec<ExecutionDecisionCandidate> {
    if agent_names.is_empty() {
        return Vec::new();
    }

    let total = agent_names.len() as f64;
    let mut ranked = agent_names
        .iter()
        .enumerate()
        .map(|(idx, agent_name)| {
            let lower = agent_name.to_ascii_lowercase();
            let history_order_score =
                ((agent_names.len().saturating_sub(idx)) as f64 / total) * 0.55;

            let (role_match_score, role_reason) = if let Some(role) = desired_role {
                let role = role.to_ascii_lowercase();
                let keywords = role_keywords_for(role.as_str());
                if !keywords.is_empty() && keywords.iter().any(|keyword| lower.contains(keyword)) {
                    (0.35f64, format!("role match for {}", role))
                } else {
                    (-0.12f64, format!("no explicit role match for {}", role))
                }
            } else {
                (0.08f64, "no role constraint".to_string())
            };

            let rotation_target = (phase_index + task_index) % agent_names.len();
            let spread_score = if idx == rotation_target { 0.10 } else { 0.02 };
            let score = (history_order_score + role_match_score + spread_score).clamp(0.0, 1.0);

            ExecutionDecisionCandidate {
                agent: agent_name.clone(),
                score,
                reason: format!(
                    "history_order={:.3}, {}, spread_score={:.3}",
                    history_order_score, role_reason, spread_score
                ),
            }
        })
        .collect::<Vec<_>>();

    ranked.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.agent.cmp(&b.agent))
    });
    ranked
}

