#[derive(Debug, Default, Clone, Copy)]
struct MaintenanceCycleResult {
    memory_expired_removed: usize,
    sqlite_expired_removed: usize,
    cache_vacuumed: bool,
    vector_vacuumed: bool,
}

#[allow(clippy::too_many_arguments)]
async fn run_background_maintenance_loop(
    runtime_config: Arc<StdMutex<RuntimeConfig>>,
    memory_cache: Arc<MemoryResponseCache>,
    cache: Arc<StdMutex<Option<Arc<ResponseCache>>>>,
    vector_store: Arc<StdMutex<Option<Arc<VectorStore>>>>,
    maintenance: Arc<MaintenanceTracker>,
    lifecycle: Arc<LifecycleState>,
    circuit_breakers: Arc<CircuitBreakerRegistry>,
    phase_rate_limiter: Arc<PhaseRateLimiter>,
    inflight_limiter: Arc<InflightLimiter>,
    shutdown_notify: Arc<Notify>,
) {
    let config = runtime_config
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();
    let mut maintenance_interval = tokio::time::interval(Duration::from_secs(
        config.maintenance_interval_seconds.max(1),
    ));
    let mut health_interval =
        tokio::time::interval(Duration::from_secs(config.health_interval_seconds.max(1)));
    maintenance_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    health_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = shutdown_notify.notified() => break,
            _ = maintenance_interval.tick() => {
                if lifecycle.is_shutting_down() {
                    break;
                }

                if let Err(err) = perform_maintenance_cycle(
                    Arc::clone(&memory_cache),
                    Arc::clone(&cache),
                    Arc::clone(&vector_store),
                    Arc::clone(&runtime_config),
                    Arc::clone(&maintenance),
                    "background",
                ).await {
                    warn!("background maintenance cycle failed: {}", err);
                }
            }
            _ = health_interval.tick() => {
                if lifecycle.is_shutting_down() {
                    break;
                }

                log_background_health(
                    Arc::clone(&memory_cache),
                    Arc::clone(&cache),
                    Arc::clone(&vector_store),
                    Arc::clone(&circuit_breakers),
                    Arc::clone(&phase_rate_limiter),
                    Arc::clone(&inflight_limiter),
                    Arc::clone(&lifecycle),
                    Arc::clone(&maintenance),
                ).await;
            }
        }
    }
}

async fn perform_maintenance_cycle(
    memory_cache: Arc<MemoryResponseCache>,
    cache: Arc<StdMutex<Option<Arc<ResponseCache>>>>,
    vector_store: Arc<StdMutex<Option<Arc<VectorStore>>>>,
    runtime_config: Arc<StdMutex<RuntimeConfig>>,
    maintenance: Arc<MaintenanceTracker>,
    source: &str,
) -> Result<MaintenanceCycleResult> {
    maintenance.note_started();
    let vacuum_interval_cycles = runtime_config
        .lock()
        .map(|guard| guard.sqlite_vacuum_interval_cycles.max(1))
        .unwrap_or(60);
    let current_cycle = maintenance.snapshot().cycles_total;
    let should_vacuum = current_cycle.is_multiple_of(vacuum_interval_cycles);

    let memory_expired_removed = memory_cache.purge_expired();
    let cache_handle = cache.lock().ok().and_then(|guard| guard.clone());
    let sqlite_expired_removed_result = if let Some(cache) = cache_handle.clone() {
        spawn_blocking(move || cache.purge_expired())
            .await
            .map_err(|e| anyhow::anyhow!("cache purge task join error: {}", e))?
    } else {
        Ok(0)
    };
    let sqlite_expired_removed = match sqlite_expired_removed_result {
        Ok(value) => value,
        Err(err) => {
            maintenance.note_failed(&err.to_string());
            return Err(err);
        }
    };

    let cache_vacuumed = if should_vacuum {
        if let Some(cache) = cache_handle.clone() {
            spawn_blocking(move || cache.vacuum())
                .await
                .map_err(|e| anyhow::anyhow!("cache vacuum task join error: {}", e))??;
            true
        } else {
            false
        }
    } else {
        false
    };

    let vector_vacuumed = if should_vacuum {
        if let Some(store) = vector_store.lock().ok().and_then(|guard| guard.clone()) {
            spawn_blocking(move || store.vacuum())
                .await
                .map_err(|e| anyhow::anyhow!("vector vacuum task join error: {}", e))??;
            true
        } else {
            false
        }
    } else {
        false
    };

    let result = MaintenanceCycleResult {
        memory_expired_removed,
        sqlite_expired_removed,
        cache_vacuumed,
        vector_vacuumed,
    };

    maintenance.note_completed(
        memory_expired_removed,
        sqlite_expired_removed,
        cache_vacuumed,
        vector_vacuumed,
    );
    info!(
        "maintenance cycle '{}' completed (memory_removed={}, sqlite_removed={}, cache_vacuumed={}, vector_vacuumed={})",
        source,
        result.memory_expired_removed,
        result.sqlite_expired_removed,
        result.cache_vacuumed,
        result.vector_vacuumed
    );
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
async fn log_background_health(
    memory_cache: Arc<MemoryResponseCache>,
    cache: Arc<StdMutex<Option<Arc<ResponseCache>>>>,
    vector_store: Arc<StdMutex<Option<Arc<VectorStore>>>>,
    circuit_breakers: Arc<CircuitBreakerRegistry>,
    phase_rate_limiter: Arc<PhaseRateLimiter>,
    inflight_limiter: Arc<InflightLimiter>,
    lifecycle: Arc<LifecycleState>,
    maintenance: Arc<MaintenanceTracker>,
) {
    let sqlite_cache_entries =
        if let Some(cache) = cache.lock().ok().and_then(|guard| guard.clone()) {
            match spawn_blocking(move || cache.entry_count()).await {
                Ok(Ok(count)) => Some(count),
                Ok(Err(err)) => {
                    warn!(
                        "background health failed to read sqlite cache entries: {}",
                        err
                    );
                    None
                }
                Err(err) => {
                    warn!("background health cache count task failed: {}", err);
                    None
                }
            }
        } else {
            None
        };

    let vector_counts =
        if let Some(store) = vector_store.lock().ok().and_then(|guard| guard.clone()) {
            match spawn_blocking(move || {
                Ok::<(u64, u64), anyhow::Error>((
                    store.memory_entry_count()?,
                    store.summary_entry_count()?,
                ))
            })
            .await
            {
                Ok(Ok(counts)) => Some(counts),
                Ok(Err(err)) => {
                    warn!("background health failed to read vector counts: {}", err);
                    None
                }
                Err(err) => {
                    warn!("background health vector count task failed: {}", err);
                    None
                }
            }
        } else {
            None
        };

    let (global_inflight, phase_inflight) = inflight_limiter.snapshot();
    let lifecycle_snapshot = lifecycle.snapshot();
    let maintenance_snapshot = maintenance.snapshot();

    info!(
        "runtime health: shutting_down={}, inflight_global={}, inflight_phases={}, memory_cache_entries={}, sqlite_cache_entries={:?}, vector_counts={:?}, breaker_open={}, breaker_half_open={}, rate_limiter_tracked={}, maintenance_running={}, maintenance_cycles={}",
        lifecycle_snapshot.shutting_down,
        global_inflight,
        phase_inflight.len(),
        memory_cache.active_entries(),
        sqlite_cache_entries,
        vector_counts,
        circuit_breakers.open_count(),
        circuit_breakers.half_open_count(),
        phase_rate_limiter.tracked_phases(),
        maintenance_snapshot.running,
        maintenance_snapshot.cycles_total,
    );
}

fn request_timeout(options: Option<&PhaseOptions>) -> Option<Duration> {
    options
        .and_then(|opts| opts.request_timeout_seconds)
        .map(Duration::from_secs)
}

async fn autotune_state_snapshot(autotune: &Arc<Mutex<AutoTuneState>>) -> AutoTuneState {
    autotune.lock().await.clone()
}

fn effective_vector_enabled(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
) -> bool {
    options
        .and_then(|opts| opts.vector_enabled)
        .or_else(|| vector_config.map(|cfg| cfg.enabled))
        .unwrap_or(true)
}

fn effective_vector_auto(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
) -> bool {
    options
        .and_then(|opts| opts.vector_auto)
        .or_else(|| vector_config.map(|cfg| cfg.auto_mode))
        .unwrap_or(true)
}

fn effective_vector_min_query_chars(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
    autotune_state: Option<&AutoTuneState>,
) -> usize {
    autotune_state
        .map(|state| state.current_min_query_chars)
        .or_else(|| options.and_then(|opts| opts.vector_min_query_chars))
        .or_else(|| vector_config.map(|cfg| cfg.min_query_chars))
        .unwrap_or(DEFAULT_VECTOR_MIN_QUERY_CHARS)
}

fn effective_vector_top_k(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
    autotune_state: Option<&AutoTuneState>,
) -> usize {
    autotune_state
        .map(|state| state.current_top_k)
        .or_else(|| options.and_then(|opts| opts.vector_top_k))
        .or_else(|| vector_config.map(|cfg| cfg.top_k))
        .unwrap_or(DEFAULT_VECTOR_TOP_K)
}

fn effective_vector_min_similarity(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
) -> f32 {
    options
        .and_then(|opts| opts.vector_min_similarity)
        .or_else(|| vector_config.map(|cfg| cfg.min_similarity))
        .unwrap_or(DEFAULT_VECTOR_MIN_SIMILARITY)
}

fn effective_vector_max_snippet_chars(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
) -> usize {
    options
        .and_then(|opts| opts.vector_max_snippet_chars)
        .or_else(|| vector_config.map(|cfg| cfg.max_snippet_chars))
        .unwrap_or(DEFAULT_VECTOR_MAX_SNIPPET_CHARS)
}

fn effective_summary_enabled(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
) -> bool {
    options
        .and_then(|opts| opts.summary_enabled)
        .or_else(|| vector_config.map(|cfg| cfg.summary_enabled))
        .unwrap_or(true)
}

fn effective_summary_trigger_messages(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
) -> usize {
    options
        .and_then(|opts| opts.summary_trigger_messages)
        .or_else(|| vector_config.map(|cfg| cfg.summary_trigger_messages))
        .unwrap_or(DEFAULT_SUMMARY_TRIGGER_MESSAGES)
}

fn effective_summary_max_chars(
    options: Option<&PhaseOptions>,
    vector_config: Option<&VectorConfig>,
) -> usize {
    options
        .and_then(|opts| opts.summary_max_chars)
        .or_else(|| vector_config.map(|cfg| cfg.summary_max_chars))
        .unwrap_or(DEFAULT_SUMMARY_MAX_CHARS)
}

fn optimize_messages(messages: &[Message], options: Option<&PhaseOptions>) -> Vec<Message> {
    let mut trimmed = messages.to_vec();

    if let Some(max_messages) = options.and_then(|opts| opts.max_history_messages) {
        if trimmed.len() > max_messages {
            trimmed = trimmed[trimmed.len() - max_messages..].to_vec();
        }
    }

    if let Some(max_chars) = options.and_then(|opts| opts.max_history_chars) {
        let mut kept_reversed = Vec::new();
        let mut total_chars = 0usize;

        for message in trimmed.iter().rev() {
            let message_chars = message.content.chars().count();
            if !kept_reversed.is_empty() && total_chars + message_chars > max_chars {
                break;
            }

            kept_reversed.push(message.clone());
            total_chars += message_chars;
        }

        kept_reversed.reverse();
        trimmed = kept_reversed;
    }

    trimmed
}

fn latest_user_query(messages: &[Message]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|message| message.role.eq_ignore_ascii_case("user"))
        .map(|message| message.content.trim().to_string())
        .filter(|content| !content.is_empty())
}

fn build_vector_context_message(hits: &[VectorHit]) -> String {
    let normalized = dedupe_vector_hits(hits);
    let mut content = String::from("Relevant prior context from similar requests:\n");
    for (index, hit) in normalized.iter().enumerate() {
        content.push_str(&format!(
            "{}. [similarity {:.2}] {}\n",
            index + 1,
            hit.similarity,
            hit.response_snippet
        ));
    }
    content
}

fn append_recent_summary(
    existing_summary: Option<&str>,
    latest_user_query: Option<&str>,
    response_text: &str,
    max_chars: usize,
) -> String {
    let mut segments: Vec<String> = Vec::new();
    if let Some(existing) = existing_summary {
        if !existing.trim().is_empty() {
            segments.push(existing.trim().to_string());
        }
    }
    if let Some(query) = latest_user_query {
        segments.push(format!("User focus: {}", query.trim()));
    }
    if !response_text.trim().is_empty() {
        segments.push(format!("Latest response: {}", response_text.trim()));
    }

    trim_to_tail_chars(&segments.join("\n\n"), max_chars)
}

fn trim_to_tail_chars(input: &str, max_chars: usize) -> String {
    let chars: Vec<char> = input.chars().collect();
    if chars.len() <= max_chars {
        return input.to_string();
    }

    chars[chars.len() - max_chars..].iter().collect()
}

fn build_cache_key(
    phase: &ResolvedPhase,
    messages: &[Message],
    mode_name: &str,
    approval_strategy: &str,
    agent_names: &[String],
) -> Result<String> {
    build_cache_key_from_parts(
        &phase.phase_name,
        messages,
        phase.principles.as_ref(),
        phase.options.as_ref(),
        mode_name,
        approval_strategy,
        agent_names,
    )
}

fn build_cache_key_from_parts(
    phase_name: &str,
    messages: &[Message],
    principles: Option<&Vec<String>>,
    options: Option<&PhaseOptions>,
    mode_name: &str,
    approval_strategy: &str,
    agent_names: &[String],
) -> Result<String> {
    let payload = json!({
        "phase": phase_name,
        "messages": messages,
        "principles": principles,
        "options": options,
        "mode": mode_name,
        "approval_strategy": approval_strategy,
        "agents": agent_names,
    });

    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_vec(&payload)?);
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn dedupe_vector_hits(hits: &[VectorHit]) -> Vec<VectorHit> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for hit in hits {
        let key = hit
            .response_snippet
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase();
        if seen.insert(key) {
            out.push(hit.clone());
        }
    }
    out
}

fn filter_env_ready_agents(config_path: Option<&PathBuf>, candidates: &[String]) -> Vec<String> {
    let Some(path) = config_path else {
        return candidates.to_vec();
    };
    let config = match load_app_config_lazy(path) {
        Some(cfg) => cfg,
        None => return candidates.to_vec(),
    };

    candidates
        .iter()
        .filter(|agent| is_agent_env_ready(config.as_ref(), agent))
        .cloned()
        .collect()
}

fn capability_max_complexity(ready_agents: usize) -> u8 {
    match ready_agents {
        0 => 0,
        1 => 2,
        2 => 4,
        _ => 5,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkGrade {
    Ask,
    Edit,
    Agent,
    Safeguard,
    FullAuto,
}

#[derive(Debug, Clone, Serialize)]
struct ReviewPolicy {
    min_review_level: String,
    required_reviews: usize,
    required_checks: Vec<String>,
    timeout_policy: String,
    enforce_dual_review: bool,
    enforce_action_gates: bool,
}

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

fn extra_u64(options: Option<&PhaseOptions>, key: &str) -> Option<u64> {
    options
        .and_then(|opts| opts.extra.get(key))
        .and_then(|v| v.as_u64())
}

fn extra_f64(options: Option<&PhaseOptions>, key: &str) -> Option<f64> {
    options
        .and_then(|opts| opts.extra.get(key))
        .and_then(|v| v.as_f64())
}

fn extra_string(options: Option<&PhaseOptions>, key: &str) -> Option<String> {
    options
        .and_then(|opts| opts.extra.get(key))
        .and_then(|v| v.as_str())
        .map(|v| v.to_string())
}

fn extra_bool(options: Option<&PhaseOptions>, key: &str) -> Option<bool> {
    options
        .and_then(|opts| opts.extra.get(key))
        .and_then(|v| v.as_bool())
}

fn extra_string_list(options: Option<&PhaseOptions>, key: &str) -> Option<Vec<String>> {
    options
        .and_then(|opts| opts.extra.get(key))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.as_str())
                .map(|item| item.to_string())
                .collect::<Vec<_>>()
        })
}

fn percentile(samples: &[u64], percentile: f64) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    let clamped = percentile.clamp(0.0, 100.0);
    let rank = ((clamped / 100.0) * ((samples.len() - 1) as f64)).round() as usize;
    samples[rank]
}

#[derive(Debug, Clone)]
struct RequirementGateDecision {
    blocked: bool,
    reason: Option<String>,
    missing_fields: Vec<String>,
    clarification_artifact_path: Option<PathBuf>,
    governance_artifact_path: PathBuf,
}

#[derive(Debug, Clone, Copy)]
struct LearningClarificationMetrics {
    rounds: u32,
    quality_score: f64,
    requirement_change_count: u32,
}

fn parse_string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.as_str())
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default()
}

fn parse_requirement_contract_from_params(
    params: &Value,
    task: &str,
) -> Option<RequirementContractArtifact> {
    let contract = params.get("requirement_contract")?;
    let goal = contract
        .get("goal")
        .and_then(|v| v.as_str())
        .map(|v| v.trim().to_string())
        .unwrap_or_default();
    let scope = contract
        .get("scope")
        .and_then(|v| v.as_str())
        .map(|v| v.trim().to_string())
        .unwrap_or_default();

    Some(RequirementContractArtifact {
        generated_at: now_ts(),
        task: task.to_string(),
        source: "request.params.requirement_contract".to_string(),
        goal,
        scope,
        non_goals: parse_string_list(contract.get("non_goals")),
        acceptance_criteria: parse_string_list(contract.get("acceptance_criteria")),
        constraints: parse_string_list(contract.get("constraints")),
        open_questions: parse_string_list(contract.get("open_questions")),
        ambiguity_score: contract
            .get("ambiguity_score")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            .min(5) as u8,
        user_confirmed: contract
            .get("user_confirmed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    })
}

fn default_requirement_contract(task: &str, source: &str) -> RequirementContractArtifact {
    RequirementContractArtifact {
        generated_at: now_ts(),
        task: task.to_string(),
        source: source.to_string(),
        goal: String::new(),
        scope: String::new(),
        non_goals: Vec::new(),
        acceptance_criteria: Vec::new(),
        constraints: Vec::new(),
        open_questions: Vec::new(),
        ambiguity_score: 0,
        user_confirmed: false,
    }
}

fn requirement_missing_fields(contract: &RequirementContractArtifact) -> Vec<String> {
    let mut missing = Vec::new();
    if contract.goal.trim().is_empty() {
        missing.push("goal".to_string());
    }
    if contract.scope.trim().is_empty() {
        missing.push("scope".to_string());
    }
    if contract.acceptance_criteria.is_empty() {
        missing.push("acceptance_criteria".to_string());
    }
    if contract.constraints.is_empty() {
        missing.push("constraints".to_string());
    }
    missing
}

fn requirement_questions_from_missing(missing_fields: &[String]) -> Vec<String> {
    missing_fields
        .iter()
        .map(|field| match field.as_str() {
            "goal" => "这个任务最终想达成的业务目标是什么？".to_string(),
            "scope" => "本次改动边界是什么？哪些模块必须包含？".to_string(),
            "acceptance_criteria" => "验收标准是什么？如何证明完成？".to_string(),
            "constraints" => "有哪些硬约束（时间、兼容性、性能、安全）？".to_string(),
            other => format!("请补充字段: {}", other),
        })
        .collect::<Vec<_>>()
}

fn estimate_requirement_ambiguity(task: &str, contract: &RequirementContractArtifact) -> u8 {
    let characteristics = TaskRouter::analyze_task(task);
    let mut score = characteristics.complexity.min(5);
    let missing = requirement_missing_fields(contract).len() as u8;
    score = score.saturating_add(missing.min(2));
    score.min(5)
}

fn load_latest_requirement_contract(
    ledger: &ArtifactLedger,
    task: &str,
) -> Option<RequirementContractArtifact> {
    let artifact = load_latest_requirement_contract_lazy(ledger)?;
    if artifact.task.trim() == task.trim() {
        Some(artifact)
    } else {
        None
    }
}

fn evaluate_requirement_gate(
    ledger: &ArtifactLedger,
    task: &str,
    params: &Value,
    source: &str,
) -> Result<RequirementGateDecision> {
    let characteristics = TaskRouter::analyze_task(task);
    let clarification_required = characteristics.complexity >= 3
        || characteristics.involves_multiple_modules
        || characteristics.needs_verification
        || characteristics.has_safety_concerns;

    let mut contract = parse_requirement_contract_from_params(params, task)
        .or_else(|| load_latest_requirement_contract(ledger, task))
        .unwrap_or_else(|| default_requirement_contract(task, source));
    contract.generated_at = now_ts();
    contract.source = source.to_string();
    contract.ambiguity_score = estimate_requirement_ambiguity(task, &contract);
    if let Some(v) = params
        .get("requirement_confirmed")
        .and_then(|v| v.as_bool())
    {
        contract.user_confirmed = v;
    }

    let missing_fields = requirement_missing_fields(&contract);
    let confirmed = contract.user_confirmed && missing_fields.is_empty();
    let blocked = clarification_required && !confirmed;

    let clarification_artifact_path =
        if parse_requirement_contract_from_params(params, task).is_some() {
            Some(persist_requirement_contract(ledger, &contract)?)
        } else {
            None
        };

    let reason = if blocked {
        Some(
            "requirement clarification/confirmation is required before planning or execution"
                .to_string(),
        )
    } else {
        None
    };
    let governance = GovernancePolicyArtifact {
        generated_at: now_ts(),
        task: task.to_string(),
        source: source.to_string(),
        clarification_required,
        confirmed,
        blocked,
        reason: reason.clone(),
        next_step: if blocked {
            json!({
                "method": "workflow.clarify",
                "task": task,
                "missing_fields": missing_fields,
                "suggested_followup": "call workflow.confirm with completed requirement_contract and user_confirmed=true"
            })
        } else {
            json!({"status": "confirmed"})
        },
    };
    let governance_artifact_path = persist_governance_policy(ledger, &governance)?;

    Ok(RequirementGateDecision {
        blocked,
        reason,
        missing_fields,
        clarification_artifact_path,
        governance_artifact_path,
    })
}

fn derive_clarification_quality_score(contract: &RequirementContractArtifact) -> f64 {
    let missing_count = requirement_missing_fields(contract).len() as f64;
    let completeness_score = ((4.0 - missing_count).max(0.0) / 4.0).clamp(0.0, 1.0);
    let ambiguity_penalty = (contract.ambiguity_score as f64 / 5.0).clamp(0.0, 1.0);
    let quality = 0.7 * completeness_score + 0.3 * (1.0 - ambiguity_penalty);
    quality.clamp(0.0, 1.0)
}

fn resolve_learning_clarification_metrics(
    ledger: &ArtifactLedger,
    task: &str,
    params: &Value,
) -> LearningClarificationMetrics {
    let provided_contract = parse_requirement_contract_from_params(params, task);
    let latest_contract = load_latest_requirement_contract(ledger, task);
    let active_contract = provided_contract.as_ref().or(latest_contract.as_ref());

    let rounds = params
        .get("clarification_rounds")
        .and_then(|v| v.as_u64())
        .map(|v| v.min(64) as u32)
        .unwrap_or_else(|| {
            if let Some(contract) = active_contract {
                let has_questions = !contract.open_questions.is_empty();
                let base_rounds = if has_questions { 1 } else { 0 };
                let confirm_round = if contract.user_confirmed { 1 } else { 0 };
                (base_rounds + confirm_round).max(1)
            } else {
                0
            }
        });

    let quality_score = params
        .get("clarification_quality_score")
        .and_then(|v| v.as_f64())
        .map(|v| v.clamp(0.0, 1.0))
        .unwrap_or_else(|| {
            active_contract
                .map(derive_clarification_quality_score)
                .unwrap_or(0.0)
        });

    let requirement_change_count = params
        .get("requirement_change_count")
        .and_then(|v| v.as_u64())
        .map(|v| v.min(4096) as u32)
        .or_else(|| {
            params
                .get("requirement_contract_revision")
                .and_then(|v| v.as_u64())
                .map(|revision| revision.saturating_sub(1).min(4096) as u32)
        })
        .unwrap_or_else(|| {
            if let (Some(current), Some(previous)) =
                (provided_contract.as_ref(), latest_contract.as_ref())
            {
                let changed = current.goal != previous.goal
                    || current.scope != previous.scope
                    || current.non_goals != previous.non_goals
                    || current.acceptance_criteria != previous.acceptance_criteria
                    || current.constraints != previous.constraints;
                if changed {
                    1
                } else {
                    0
                }
            } else if provided_contract.is_some() {
                1
            } else {
                0
            }
        });

    LearningClarificationMetrics {
        rounds,
        quality_score,
        requirement_change_count,
    }
}

fn observe_latency_histogram(
    duration: Duration,
    count: &mut u64,
    sum_seconds: &mut f64,
    buckets: &mut [u64; HISTOGRAM_BUCKETS_SECONDS.len() + 1],
) {
    let value = duration.as_secs_f64();
    *count += 1;
    *sum_seconds += value;
    let mut idx = HISTOGRAM_BUCKETS_SECONDS.len();
    for (i, bound) in HISTOGRAM_BUCKETS_SECONDS.iter().enumerate() {
        if value <= *bound {
            idx = i;
            break;
        }
    }
    buckets[idx] = buckets[idx].saturating_add(1);
}

fn extract_task_description(messages: &[Message]) -> String {
    messages
        .iter()
        .rev()
        .find(|message| {
            message.role.eq_ignore_ascii_case("user") && !message.content.trim().is_empty()
        })
        .map(|message| message.content.clone())
        .or_else(|| messages.last().map(|message| message.content.clone()))
        .unwrap_or_else(|| "general task".to_string())
}

fn pipeline_gate_violation(
    analyzed_task: &TaskCharacteristics,
    routing: &RoutingDecision,
    approval_strategy: ApprovalStrategy,
) -> Option<String> {
    let non_trivial = analyzed_task.complexity >= 3
        || analyzed_task.needs_verification
        || analyzed_task.involves_multiple_modules
        || analyzed_task.has_safety_concerns;

    if non_trivial && routing.roles.is_empty() {
        return Some("routing produced no roles for a non-trivial task".to_string());
    }

    let reviewer_required = routing.roles.contains(&AgentRole::Reviewer)
        || routing
            .pua_enforcement
            .mandatory_roles
            .contains(&AgentRole::Reviewer);
    if reviewer_required && !approval_strategy.needs_dual_review() {
        return Some(
            "reviewer role required by pipeline routing, but current mode does not enable dual review gate"
                .to_string(),
        );
    }

    if non_trivial && routing.pua_enforcement.mandatory_safeguards.is_empty() {
        return Some("PUA safeguards missing for non-trivial task".to_string());
    }

    None
}

fn touch_conversation_order(order: &StdMutex<Vec<String>>, conversation_id: &str) {
    if let Ok(mut guard) = order.lock() {
        if let Some(position) = guard.iter().position(|item| item == conversation_id) {
            guard.remove(position);
        }
        guard.push(conversation_id.to_string());
    }
}

fn evict_oldest_conversation(
    store: &mut HashMap<String, ConversationState>,
    order: &StdMutex<Vec<String>>,
) -> Option<String> {
    if let Ok(mut guard) = order.lock() {
        while let Some(candidate) = guard.first().cloned() {
            guard.remove(0);
            if store.remove(&candidate).is_some() {
                return Some(candidate);
            }
        }
        return None;
    }

    let oldest = store
        .iter()
        .min_by_key(|(_, state)| state.last_touched_at)
        .map(|(id, _)| id.clone());

    oldest.and_then(|id| store.remove(&id).map(|_| id))
}

fn enforce_checkpoint_capacity(
    state: &mut ConversationState,
    incoming: usize,
    protected_checkpoint_id: Option<&str>,
) {
    let total_after_insert = state.checkpoints.len().saturating_add(incoming);
    if total_after_insert <= MAX_CHECKPOINTS_PER_CONVERSATION {
        return;
    }

    let mut overflow = total_after_insert - MAX_CHECKPOINTS_PER_CONVERSATION;
    let mut cursor = 0usize;

    // Prefer removing oldest checkpoints, but keep the rollback target when requested.
    while overflow > 0 && cursor < state.checkpoints.len() {
        let can_remove = protected_checkpoint_id
            .map(|protected| state.checkpoints[cursor].checkpoint_id != protected)
            .unwrap_or(true);
        if can_remove {
            state.checkpoints.remove(cursor);
            overflow -= 1;
        } else {
            cursor += 1;
        }
    }

    if overflow > 0 {
        let drain_to = overflow.min(state.checkpoints.len());
        state.checkpoints.drain(0..drain_to);
    }

    repair_conversation_branch_heads(state);
}

fn stream_would_exceed_limits(
    current_chunks: usize,
    current_chars: usize,
    next_token_chars: usize,
) -> bool {
    current_chunks.saturating_add(1) > MAX_STREAM_CHUNKS
        || current_chars.saturating_add(next_token_chars) > MAX_STREAM_CHARS
}

fn validate_storage_key(
    value: &str,
    field: &str,
    max_len: usize,
) -> std::result::Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(crate::i18n::tf(
            "error.storage_key_empty",
            &[("field", field)],
        ));
    }
    if trimmed.len() > max_len {
        return Err(crate::i18n::tf(
            "error.storage_key_too_long",
            &[("field", field), ("max_len", &max_len.to_string())],
        ));
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | ':' | '/'))
    {
        return Err(format!(
            "{field} contains invalid characters; allowed: [A-Za-z0-9_.:/-]"
        ));
    }

    Ok(trimmed.to_string())
}

fn checkpoint_message_chars(messages: &[Message]) -> usize {
    messages.iter().map(|msg| msg.content.chars().count()).sum()
}

fn repair_conversation_branch_heads(state: &mut ConversationState) {
    let existing_ids = state
        .checkpoints
        .iter()
        .map(|checkpoint| checkpoint.checkpoint_id.clone())
        .collect::<HashSet<_>>();
    let mut repaired_heads: HashMap<String, String> = HashMap::new();
    for (branch, head_id) in state.branch_heads.clone() {
        if existing_ids.contains(&head_id) {
            repaired_heads.insert(branch, head_id);
            continue;
        }

        if let Some(fallback) = state
            .checkpoints
            .iter()
            .rev()
            .find(|checkpoint| checkpoint.branch_id == branch)
            .map(|checkpoint| checkpoint.checkpoint_id.clone())
        {
            repaired_heads.insert(branch, fallback);
        }
    }
    state.branch_heads = repaired_heads;
}

fn branch_head_adjustment_counts(
    before: &HashMap<String, String>,
    after: &HashMap<String, String>,
) -> (usize, usize) {
    let mut repaired = 0usize;
    let mut dropped = 0usize;
    for (branch, old_head) in before {
        match after.get(branch) {
            Some(new_head) if new_head != old_head => repaired = repaired.saturating_add(1),
            Some(_) => {}
            None => dropped = dropped.saturating_add(1),
        }
    }

    (repaired, dropped)
}

fn infer_pua_stage(event_type: &str, phase: &str) -> Option<String> {
    if event_type.starts_with("phase.") {
        return Some(phase.to_string());
    }
    None
}

fn normalize_trace_attributes(event_type: &str, phase: &str, status: &str, inputs: Value) -> Value {
    let mut attrs = match inputs {
        Value::Object(map) => map,
        other => {
            let mut map = serde_json::Map::new();
            map.insert("payload".to_string(), other);
            map
        }
    };

    attrs
        .entry("event_type".to_string())
        .or_insert_with(|| Value::String(event_type.to_string()));
    attrs
        .entry("phase".to_string())
        .or_insert_with(|| Value::String(phase.to_string()));
    attrs
        .entry("stage".to_string())
        .or_insert_with(|| Value::String(phase.to_string()));
    attrs.entry("policy_status".to_string()).or_insert_with(|| {
        Value::String(
            match status {
                "ok" => "pass",
                "error" => "error",
                _ => "unknown",
            }
            .to_string(),
        )
    });

    Value::Object(attrs)
}

#[allow(clippy::too_many_arguments)]
fn stream_chunk_notification(
    id: &Option<Value>,
    agent: &str,
    token: &str,
    chunk_index: usize,
    total_chars: usize,
    cache_level: Option<&str>,
    phase: Option<&str>,
    trace_id: Option<&str>,
) -> Value {
    let mut payload = serde_json::Map::new();
    payload.insert("id".to_string(), id.clone().unwrap_or(Value::Null));
    payload.insert("agent".to_string(), Value::String(agent.to_string()));
    payload.insert("token".to_string(), Value::String(token.to_string()));
    payload.insert("chunk_index".to_string(), json!(chunk_index));
    payload.insert("total_chars".to_string(), json!(total_chars));

    if let Some(level) = cache_level {
        payload.insert("cached".to_string(), Value::Bool(true));
        payload.insert("cache_level".to_string(), Value::String(level.to_string()));
    }
    if let Some(phase_name) = phase {
        payload.insert("phase".to_string(), Value::String(phase_name.to_string()));
    }
    if let Some(trace) = trace_id {
        payload.insert("trace_id".to_string(), Value::String(trace.to_string()));
    }

    Value::Object(payload)
}

#[allow(clippy::too_many_arguments)]
fn stream_done_notification(
    id: &Option<Value>,
    agent: &str,
    chunks: usize,
    total_chars: usize,
    cache_level: Option<&str>,
    phase: Option<&str>,
    trace_id: Option<&str>,
    duration_ms: u64,
) -> Value {
    let mut payload = serde_json::Map::new();
    payload.insert("id".to_string(), id.clone().unwrap_or(Value::Null));
    payload.insert("agent".to_string(), Value::String(agent.to_string()));
    payload.insert("done".to_string(), Value::Bool(true));
    payload.insert("chunks".to_string(), json!(chunks));
    payload.insert("total_chars".to_string(), json!(total_chars));
    payload.insert("duration_ms".to_string(), json!(duration_ms));

    if let Some(level) = cache_level {
        payload.insert("cached".to_string(), Value::Bool(true));
        payload.insert("cache_level".to_string(), Value::String(level.to_string()));
    }
    if let Some(phase_name) = phase {
        payload.insert("phase".to_string(), Value::String(phase_name.to_string()));
    }
    if let Some(trace) = trace_id {
        payload.insert("trace_id".to_string(), Value::String(trace.to_string()));
    }

    Value::Object(payload)
}

fn histogram_prometheus_lines(
    name: &str,
    count: u64,
    sum_seconds: f64,
    buckets: &[u64; HISTOGRAM_BUCKETS_SECONDS.len() + 1],
) -> Vec<String> {
    let mut lines = Vec::new();
    push_metric_header(
        &mut lines,
        name,
        "histogram",
        "ACP latency distribution in seconds",
    );
    let mut cumulative = 0_u64;
    for (idx, le) in HISTOGRAM_BUCKETS_SECONDS.iter().enumerate() {
        cumulative = cumulative.saturating_add(buckets[idx]);
        lines.push(format!("{}_bucket{{le=\"{}\"}} {}", name, le, cumulative));
    }
    cumulative = cumulative.saturating_add(buckets[HISTOGRAM_BUCKETS_SECONDS.len()]);
    lines.push(format!("{}_bucket{{le=\"+Inf\"}} {}", name, cumulative));
    lines.push(format!("{}_sum {}", name, sum_seconds));
    lines.push(format!("{}_count {}", name, count));
    lines
}

fn classify_agent_failure(err: &anyhow::Error) -> &'static str {
    let msg = err.to_string().to_ascii_lowercase();
    if msg.contains("timed out") || msg.contains("timeout") {
        return "timeout";
    }
    if msg.contains("panic") {
        return "panic";
    }
    "other"
}

fn record_agent_failure_metrics(metrics: &RuntimeMetrics, err: &anyhow::Error) {
    metrics.inc_agent_failures();
    match classify_agent_failure(err) {
        "timeout" => metrics.inc_agent_timeout_failures(),
        "panic" => metrics.inc_agent_panic_failures(),
        _ => metrics.inc_agent_other_failures(),
    }
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn hash_hex(input: &str, hex_len: usize) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let digest = hasher.finalize();
    let full = digest
        .iter()
        .map(|byte| format!("{:02x}", byte))
        .collect::<String>();
    full.chars().take(hex_len).collect()
}

fn escape_prometheus_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

fn build_prometheus_metrics(
    snapshot: &MetricsSnapshot,
    gauges: &RuntimeGaugeSnapshot,
    breaker_snapshot: &HashMap<String, CircuitBreakerSnapshot>,
    phase_limiter_snapshot: &HashMap<String, (f64, f64)>,
    inflight_snapshot: &(usize, HashMap<String, usize>),
    lifecycle: &LifecycleSnapshot,
    maintenance: &MaintenanceSnapshot,
) -> String {
    let mut lines = Vec::new();
    push_scalar_metric(
        &mut lines,
        "acp_chat_requests_total",
        "counter",
        "Total ACP chat requests handled",
        snapshot.chat_requests_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_cache_lookup_total",
        "counter",
        "Total cache lookups performed",
        snapshot.cache_lookup_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_cache_hit_total",
        "counter",
        "Total cache hits served",
        snapshot.cache_hit_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_cache_store_total",
        "counter",
        "Total cache writes performed",
        snapshot.cache_store_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_vector_search_total",
        "counter",
        "Total vector searches performed",
        snapshot.vector_search_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_vector_hit_total",
        "counter",
        "Total vector retrieval hits",
        snapshot.vector_hit_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_vector_store_total",
        "counter",
        "Total vector memory writes",
        snapshot.vector_store_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_summary_read_total",
        "counter",
        "Total summary memory reads",
        snapshot.summary_read_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_summary_hit_total",
        "counter",
        "Total summary memory hits",
        snapshot.summary_hit_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_summary_store_total",
        "counter",
        "Total summary memory writes",
        snapshot.summary_store_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_agent_failures_total",
        "counter",
        "Total agent execution failures",
        snapshot.agent_failures_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_agent_timeout_failures_total",
        "counter",
        "Total agent timeout failures",
        snapshot.agent_timeout_failures_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_agent_panic_failures_total",
        "counter",
        "Total agent panic failures",
        snapshot.agent_panic_failures_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_agent_other_failures_total",
        "counter",
        "Total uncategorized agent failures",
        snapshot.agent_other_failures_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_review_gate_total",
        "counter",
        "Total review gate evaluations",
        snapshot.review_gate_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_review_gate_approved_total",
        "counter",
        "Total review gate approvals",
        snapshot.review_gate_approved_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_review_gate_rejected_total",
        "counter",
        "Total review gate rejections",
        snapshot.review_gate_rejected_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_review_gate_timeout_total",
        "counter",
        "Total review gate deadline timeouts",
        snapshot.review_gate_timeout_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_review_gate_degraded_total",
        "counter",
        "Total review gate approvals degraded after timeout",
        snapshot.review_gate_degraded_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_review_gate_invalid_response_total",
        "counter",
        "Total invalid review gate responses",
        snapshot.review_gate_invalid_response_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_lazy_blue5_doc_lookup_total",
        "counter",
        "Total BLUE5 document lazy-load lookups",
        snapshot.lazy_blue5_doc_lookup_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_lazy_blue5_doc_hit_total",
        "counter",
        "Total BLUE5 document lazy-load cache hits",
        snapshot.lazy_blue5_doc_hit_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_lazy_blue5_doc_reload_total",
        "counter",
        "Total BLUE5 document lazy-load reloads",
        snapshot.lazy_blue5_doc_reload_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_lazy_app_config_lookup_total",
        "counter",
        "Total app config lazy-load lookups",
        snapshot.lazy_app_config_lookup_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_lazy_app_config_hit_total",
        "counter",
        "Total app config lazy-load cache hits",
        snapshot.lazy_app_config_hit_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_lazy_app_config_reload_total",
        "counter",
        "Total app config lazy-load reloads",
        snapshot.lazy_app_config_reload_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_lazy_clarification_lookup_total",
        "counter",
        "Total clarification artifact lazy-load lookups",
        snapshot.lazy_clarification_lookup_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_lazy_clarification_hit_total",
        "counter",
        "Total clarification artifact lazy-load cache hits",
        snapshot.lazy_clarification_hit_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_lazy_clarification_reload_total",
        "counter",
        "Total clarification artifact lazy-load reloads",
        snapshot.lazy_clarification_reload_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_memory_cache_entries",
        "gauge",
        "Current in-memory cache entries",
        gauges.memory_cache_entries,
    );
    push_scalar_metric(
        &mut lines,
        "acp_sqlite_cache_entries",
        "gauge",
        "Current SQLite cache entries",
        gauges.sqlite_cache_entries,
    );
    push_scalar_metric(
        &mut lines,
        "acp_vector_memory_entries",
        "gauge",
        "Current vector memory entries",
        gauges.vector_memory_entries,
    );
    push_scalar_metric(
        &mut lines,
        "acp_vector_summary_entries",
        "gauge",
        "Current vector summary entries",
        gauges.vector_summary_entries,
    );
    push_scalar_metric(
        &mut lines,
        "acp_circuit_open_agents",
        "gauge",
        "Current open circuit breaker agents",
        gauges.circuit_open_agents,
    );
    push_scalar_metric(
        &mut lines,
        "acp_circuit_half_open_agents",
        "gauge",
        "Current half-open circuit breaker agents",
        gauges.circuit_half_open_agents,
    );
    push_scalar_metric(
        &mut lines,
        "acp_circuit_tracked_agents",
        "gauge",
        "Current tracked circuit breaker agents",
        gauges.circuit_tracked_agents,
    );
    push_scalar_metric(
        &mut lines,
        "acp_rate_limiter_tracked_phases",
        "gauge",
        "Current tracked phases with rate limiter state",
        gauges.rate_limiter_tracked_phases,
    );
    push_scalar_metric(
        &mut lines,
        "acp_lifecycle_shutting_down",
        "gauge",
        "Whether the ACP server is shutting down",
        if lifecycle.shutting_down { 1 } else { 0 },
    );
    push_scalar_metric(
        &mut lines,
        "acp_maintenance_cycles_total",
        "counter",
        "Total maintenance cycles executed",
        maintenance.cycles_total,
    );
    push_scalar_metric(
        &mut lines,
        "acp_maintenance_running",
        "gauge",
        "Whether a maintenance cycle is currently running",
        if maintenance.running { 1 } else { 0 },
    );

    push_metric_header(
        &mut lines,
        "acp_inflight_requests",
        "gauge",
        "Current in-flight request count by scope",
    );
    lines.push(format!(
        "acp_inflight_requests{{scope=\"global\"}} {}",
        inflight_snapshot.0
    ));
    for (phase, count) in inflight_snapshot.1.iter() {
        lines.push(format!(
            "acp_inflight_requests{{scope=\"phase\",phase=\"{}\"}} {}",
            escape_prometheus_label(phase),
            count
        ));
    }

    push_metric_header(
        &mut lines,
        "acp_phase_rate_limiter_tokens",
        "gauge",
        "Current token bucket tokens by phase",
    );
    push_metric_header(
        &mut lines,
        "acp_phase_rate_limiter_capacity",
        "gauge",
        "Current token bucket capacity by phase",
    );
    for (phase, (tokens, capacity)) in phase_limiter_snapshot.iter() {
        let phase = escape_prometheus_label(phase);
        lines.push(format!(
            "acp_phase_rate_limiter_tokens{{phase=\"{}\"}} {:.3}",
            phase, tokens
        ));
        lines.push(format!(
            "acp_phase_rate_limiter_capacity{{phase=\"{}\"}} {:.3}",
            phase, capacity
        ));
    }

    push_metric_header(
        &mut lines,
        "acp_circuit_breaker_state",
        "gauge",
        "Current circuit breaker state per agent",
    );
    push_metric_header(
        &mut lines,
        "acp_circuit_breaker_failures",
        "gauge",
        "Current consecutive failures per agent",
    );
    for (agent, state) in breaker_snapshot.iter() {
        let agent = escape_prometheus_label(agent);
        for stage in ["closed", "open", "half_open", "half_open_ready"] {
            let value = if state.state == stage { 1 } else { 0 };
            lines.push(format!(
                "acp_circuit_breaker_state{{agent=\"{}\",state=\"{}\"}} {}",
                agent, stage, value
            ));
        }
        lines.push(format!(
            "acp_circuit_breaker_failures{{agent=\"{}\"}} {}",
            agent, state.consecutive_failures
        ));
    }

    lines.extend(histogram_prometheus_lines(
        "acp_chat_latency_seconds",
        snapshot.chat_latency_count,
        snapshot.chat_latency_sum_seconds,
        &snapshot.chat_latency_bucket_counts,
    ));
    lines.extend(histogram_prometheus_lines(
        "acp_agent_latency_seconds",
        snapshot.agent_latency_count,
        snapshot.agent_latency_sum_seconds,
        &snapshot.agent_latency_bucket_counts,
    ));
    lines.extend(histogram_prometheus_lines(
        "acp_review_latency_seconds",
        snapshot.review_latency_count,
        snapshot.review_latency_sum_seconds,
        &snapshot.review_latency_bucket_counts,
    ));

    lines.join("\n")
}

