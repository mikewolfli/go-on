impl AcpServer {
    async fn handle_chat(
        &self,
        id: Option<Value>,
        params: Option<Value>,
        request_span: Option<OtelContext>,
        parent_trace: Option<RequestTraceContext>,
    ) -> Result<()> {
        let started = Instant::now();
        let pipeline_trace = parent_trace
            .map(|trace| child_trace_context(&trace, "chat.pipeline"))
            .unwrap_or_else(|| chat_trace_context(&id, "chat.pipeline"));
        info!(
            trace_id = %pipeline_trace.trace_id,
            "pipeline entry: chat request received"
        );
        let chat_span = request_span.as_ref().and_then(|parent| {
            self.telemetry.start_child_span(
                parent,
                "acp.chat",
                vec![KeyValue::new("phase.entry", "chat")],
            )
        });
        let result = async {
            if self.lifecycle.is_shutting_down() {
                self.send_error(
                    id,
                    -32031,
                    "server is shutting down".to_string(),
                    Some(serde_json::to_value(self.lifecycle.snapshot())?),
                )
                .await?;
                return Ok(());
            }

            self.metrics.inc_chat_requests();

            let params_value = params.unwrap_or_else(|| json!({}));
            let chat_params: ChatParams = match serde_json::from_value(params_value) {
                Ok(value) => value,
                Err(err) => {
                    self.send_error(
                        id,
                        -32602,
                        crate::i18n::tf("error.invalid_chat_params", &[("error", &format!("{err}"))]),
                        None,
                    )
                    .await?;
                    return Ok(());
                }
            };

            let mode = ChatMode::parse(chat_params.mode.as_deref());
            let mode_name = mode.map(|m| m.as_str()).unwrap_or("default");
            let auto_conv_id = chat_params
                .conversation_id
                .as_deref()
                .and_then(|value| {
                    validate_storage_key(value, "conversation_id", MAX_CONVERSATION_ID_LEN).ok()
                })
                .unwrap_or_else(|| pipeline_trace.trace_id.clone());
            let original_messages = chat_params.messages.clone();
            let (flow, registry) = self.routing_handles()?;
            let effective_phase = self.infer_phase_name_with_flow(
                flow.as_ref(),
                chat_params.phase.as_deref(),
                mode,
            );

            // Mandatory pipeline stage 1: Analyze task intent from conversation input.
            let analyzed_task = TaskRouter::analyze_task(&extract_task_description(&chat_params.messages));
            self.record_trace_event(
                &pipeline_trace,
                "phase.analyze",
                "ok",
                "analyze",
                json!({
                    "task_type": format!("{:?}", analyzed_task.task_type),
                    "complexity": analyzed_task.complexity,
                    "needs_verification": analyzed_task.needs_verification,
                    "has_safety_concerns": analyzed_task.has_safety_concerns,
                    "involves_multiple_modules": analyzed_task.involves_multiple_modules,
                }),
                None,
                0,
            );

            // Mandatory pipeline stage 2: Route into role-based hard gates.
            let pipeline_routing = TaskRouter::route_task(&analyzed_task);
            self.record_trace_event(
                &pipeline_trace,
                "phase.route_hard_gate",
                "ok",
                "route",
                json!({
                    "policy_status": "pass",
                    "roles": pipeline_routing
                        .roles
                        .iter()
                        .map(|role| format!("{:?}", role))
                        .collect::<Vec<_>>(),
                    "success_rate": pipeline_routing.predicted_success_rate,
                    "risk_factors": pipeline_routing.risk_factors.clone(),
                    "mandatory_safeguards": pipeline_routing.pua_enforcement.mandatory_safeguards.clone(),
                }),
                None,
                0,
            );

            let total_chars: usize = chat_params
                .messages
                .iter()
                .map(|m| m.content.chars().count())
                .sum();

            let routing_started = Instant::now();
            let routing = flow
                .resolve(Some(effective_phase.clone()), registry.as_ref())
                .map_err(|err| ProxyError::Internal(err.to_string()))?;
            self.record_trace_event(
                &child_trace_context(&pipeline_trace, "chat.route"),
                "phase.route",
                "ok",
                "route",
                json!({ "phase": routing.phase.phase_name }),
                None,
                routing_started.elapsed().as_millis() as u64,
            );

        if let Some(limit) = extra_u64(routing.phase.options.as_ref(), "max_request_chars") {
            if total_chars > limit as usize {
                self.send_error(
                    id,
                    -32600,
                    format!(
                        "request too large: {} chars exceeds limit {}",
                        total_chars, limit
                    ),
                    None,
                )
                .await?;
                return Ok(());
            }
        }

            if let Some(rpm_limit) = extra_u64(routing.phase.options.as_ref(), "rate_limit_rpm") {
                let burst_capacity = extra_u64(routing.phase.options.as_ref(), "rate_limit_burst").or_else(|| {
                    extra_f64(routing.phase.options.as_ref(), "rate_limit_burst_multiplier")
                        .map(|m| ((rpm_limit as f64) * m.max(0.1)).round() as u64)
                });
            if !self
                .phase_rate_limiter
                    .allow(&routing.phase.phase_name, rpm_limit, burst_capacity)
            {
                self.send_error(
                    id,
                    -32029,
                    format!(
                        "phase '{}' rate limited at {} requests/min",
                        routing.phase.phase_name, rpm_limit
                    ),
                    None,
                )
                .await?;
                return Ok(());
            }
            }

            let phase_max_inflight = extra_u64(routing.phase.options.as_ref(), "phase_max_inflight");
            let global_max_inflight = extra_u64(routing.phase.options.as_ref(), "global_max_inflight");
            let _inflight_guard = match self.inflight_limiter.try_enter(
                &routing.phase.phase_name,
                phase_max_inflight,
                global_max_inflight,
            ) {
                Some(guard) => guard,
                None => {
                    self.send_error(
                        id,
                        -32030,
                        "inflight limit exceeded for this phase or globally".to_string(),
                        None,
                    )
                    .await?;
                    return Ok(());
                }
            };

        let autopilot_complexity = routing
            .phase
            .options
            .as_ref()
            .and_then(|opts| opts.autopilot_complexity.as_deref())
            .and_then(AutopilotComplexity::from_str);

        let mut approval_strategy = mode_to_approval_strategy(mode, autopilot_complexity);
        if matches!(approval_strategy, ApprovalStrategy::AutoPilotSimple)
            && analyzed_task.complexity >= 3
            && self.should_escalate_approval_strategy()
        {
            approval_strategy = ApprovalStrategy::AutoPilotComplex;
            self.record_trace_event(
                &pipeline_trace,
                "phase.route_adapt",
                "ok",
                "route",
                json!({
                    "reason": "online_controller_escalation",
                    "new_strategy": approval_strategy.as_str(),
                }),
                None,
                0,
            );
            self.send_notification(
                "chat.pipeline",
                json!({
                    "id": id.clone(),
                    "event": "strategy_escalated",
                    "strategy": approval_strategy.as_str(),
                }),
            )
            .await?;
        }

        let review_policy = resolve_review_policy(
            routing.phase.options.as_ref(),
            Some(&analyzed_task),
            false,
            approval_strategy.needs_dual_review(),
        );
        if review_policy.enforce_dual_review && !approval_strategy.needs_dual_review() {
            approval_strategy = ApprovalStrategy::AutoPilotComplex;
            self.record_trace_event(
                &pipeline_trace,
                "phase.route_adapt",
                "ok",
                "route",
                json!({
                    "reason": "review_policy_enforced_dual_review",
                    "new_strategy": approval_strategy.as_str(),
                }),
                None,
                0,
            );
        }

        if let Some(reason) = pipeline_gate_violation(&analyzed_task, &pipeline_routing, approval_strategy) {
            self.record_trace_event(
                &pipeline_trace,
                "phase.route_hard_gate",
                "error",
                "route",
                json!({
                    "reason": reason,
                    "policy_status": "blocked",
                }),
                Some(reason.clone()),
                0,
            );
            self.send_error(id, -32603, crate::i18n::tf("error.pipeline_gate_blocked", &[("reason", &reason)]), None)
                .await?;
            return Ok(());
        }

        info!(
            "phase '{}' ({}) selected from flow '{}' with {} candidate agent(s); mode={}, strategy={}",
            routing.phase.phase_name,
            routing.phase.phase_description,
            routing.phase.flow_name,
            routing.agents.len(),
            mode_name,
            approval_strategy.as_str(),
        );

        let review_started = Instant::now();
        let review_decisions = if review_policy.enforce_dual_review {
            match self
                .run_dual_review_gate(
                    id.clone(),
                    &chat_params.messages,
                    routing.phase.options.as_ref(),
                    chat_span.as_ref().or(request_span.as_ref()),
                    &pipeline_trace,
                )
                .await
            {
                Ok(ReviewGateOutcome::Approved(decisions)) => {
                    self.record_trace_event(
                        &child_trace_context(&pipeline_trace, "chat.review"),
                        "phase.review_gate",
                        "ok",
                        "review",
                        json!({
                            "policy_status": "pass",
                            "result": "approved",
                            "review_decisions": decisions.len(),
                        }),
                        None,
                        review_started.elapsed().as_millis() as u64,
                    );
                    Some(decisions)
                }
                Ok(ReviewGateOutcome::Rejected(decisions)) => {
                    self.record_trace_event(
                        &child_trace_context(&pipeline_trace, "chat.review"),
                        "phase.review_gate",
                        "error",
                        "review",
                        json!({
                            "policy_status": "blocked",
                            "result": "rejected",
                            "review_decisions": decisions.len(),
                        }),
                        Some("review gate rejected execution".to_string()),
                        review_started.elapsed().as_millis() as u64,
                    );
                    self.send_error(
                        id,
                        -32603,
                        "review gate rejected execution".to_string(),
                        Some(json!({ "reviews": decisions })),
                    )
                    .await?;
                    return Ok(());
                }
                    Ok(ReviewGateOutcome::Degraded(decisions)) => {
                        self.record_trace_event(
                            &child_trace_context(&pipeline_trace, "chat.review"),
                            "phase.review_gate",
                            "ok",
                            "review",
                            json!({
                                "policy_status": "degraded",
                                "result": "degraded",
                                "review_decisions": decisions.len(),
                            }),
                            None,
                            review_started.elapsed().as_millis() as u64,
                        );
                        self.send_notification(
                            "chat.review",
                            json!({
                                "id": id.clone(),
                                "mode": "degrade_single",
                                "reason": "review gate timeout",
                            }),
                        )
                        .await?;
                        warn!(
                            trace_id = %pipeline_trace.trace_id,
                            "review gate degraded: timeout reached, proceeding with degraded single-reviewer approval"
                        );
                        Some(decisions)
                    }
                Err(err) => {
                    self.record_trace_event(
                        &child_trace_context(&pipeline_trace, "chat.review"),
                        "phase.review_gate",
                        "error",
                        "review",
                        json!({
                            "policy_status": "error",
                            "result": "failed",
                        }),
                        Some(err.to_string()),
                        review_started.elapsed().as_millis() as u64,
                    );
                    self.send_error(id, -32603, crate::i18n::tf("error.review_gate_failed", &[("error", &format!("{err}"))]), None)
                        .await?;
                    return Ok(());
                }
            }
        } else {
            None
        };

        self.record_trace_event(
            &pipeline_trace,
            "phase.verify",
            "ok",
            "verify",
            json!({
                "needs_dual_review": review_policy.enforce_dual_review,
                "review_decisions": review_decisions.as_ref().map(|v| v.len()).unwrap_or(0),
                "review_policy": review_policy,
            }),
            None,
            0,
        );
        let prepared_input = self
            .build_effective_messages(&routing.phase, &chat_params.messages)
            .await?;
        let bypass_cache = matches!(mode, Some(ChatMode::FullAuto));
        let cache_enabled = routing
            .phase
            .options
            .as_ref()
            .and_then(|opts| opts.cache_enabled)
            .unwrap_or(true);

        if !bypass_cache && cache_enabled {
            let cache_ttl = routing
                .phase
                .options
                .as_ref()
                .and_then(|opts| opts.cache_ttl_seconds)
                .unwrap_or(300);

            let cache_key = build_cache_key(
                &routing.phase,
                &prepared_input.messages,
                mode_name,
                approval_strategy.as_str(),
                &routing.phase.agent_names,
            )?;

            if let Some(memory_hit) = self.memory_cache.get(&cache_key) {
                self.metrics.inc_cache_hit();
                let cached_agent = memory_hit
                    .agent_name
                    .clone()
                    .unwrap_or_else(|| "memory-cache".to_string());
                let stream_payload = stream_chunk_notification(
                    &id,
                    &cached_agent,
                    &memory_hit.response_text,
                    1,
                    memory_hit.response_text.chars().count(),
                    Some("memory"),
                    Some(routing.phase.phase_name.as_str()),
                    Some(pipeline_trace.trace_id.as_str()),
                );
                self.send_notification(
                    "chat.stream",
                    stream_payload,
                )
                .await?;
                let done_payload = stream_done_notification(
                    &id,
                    &cached_agent,
                    1,
                    memory_hit.response_text.chars().count(),
                    Some("memory"),
                    Some(routing.phase.phase_name.as_str()),
                    Some(pipeline_trace.trace_id.as_str()),
                    0,
                );
                self.send_notification("chat.stream.done", done_payload).await?;
                self.record_trace_event(
                    &pipeline_trace,
                    "phase.agent",
                    "ok",
                    routing.phase.phase_name.as_str(),
                    json!({
                        "agent": cached_agent,
                        "cache_level": "memory",
                        "source": "memory_cache",
                    }),
                    None,
                    0,
                );

                self.send_result(
                    id,
                    json!({
                        "agent": memory_hit.agent_name,
                        "phase": routing.phase.phase_name,
                        "mode": mode_name,
                        "approval_strategy": approval_strategy.as_str(),
                        "review_policy": review_policy,
                        "cached": true,
                        "cache_level": "memory",
                        "done": true,
                        "reviews": review_decisions,
                        "pipeline": {
                            "analyze": format!("{:?}", analyzed_task.task_type),
                            "route_roles": pipeline_routing
                                .roles
                                .iter()
                                .map(|role| format!("{:?}", role))
                                .collect::<Vec<_>>(),
                        },
                    }),
                )
                .await?;
                self.record_trace_event(
                    &pipeline_trace,
                    "phase.learn",
                    "ok",
                    "learn",
                    json!({"source": "memory_cache"}),
                    None,
                    0,
                );
                return Ok(());
            }

            if let Some(cache) = self.cache_handle() {
                self.metrics.inc_cache_lookup();
                if let Some(hit) = self.cache_get(cache.clone(), cache_key.clone()).await? {
                    self.metrics.inc_cache_hit();
                        let cached_agent =
                            hit.agent_name.clone().unwrap_or_else(|| "cache".to_string());

                    self.memory_cache.put(
                        cache_key,
                        hit.response_text.clone(),
                        hit.agent_name.clone(),
                        cache_ttl,
                    );

                        let stream_payload = stream_chunk_notification(
                            &id,
                            &cached_agent,
                            &hit.response_text,
                            1,
                            hit.response_text.chars().count(),
                            Some("sqlite"),
                            Some(routing.phase.phase_name.as_str()),
                            Some(pipeline_trace.trace_id.as_str()),
                        );
                    self.send_notification(
                        "chat.stream",
                            stream_payload,
                    )
                    .await?;
                        let done_payload = stream_done_notification(
                            &id,
                            &cached_agent,
                            1,
                            hit.response_text.chars().count(),
                            Some("sqlite"),
                            Some(routing.phase.phase_name.as_str()),
                            Some(pipeline_trace.trace_id.as_str()),
                            0,
                        );
                        self.send_notification("chat.stream.done", done_payload).await?;
                    self.record_trace_event(
                        &pipeline_trace,
                        "phase.agent",
                        "ok",
                        routing.phase.phase_name.as_str(),
                        json!({
                            "agent": cached_agent,
                            "cache_level": "sqlite",
                            "source": "sqlite_cache",
                        }),
                        None,
                        0,
                    );

                    self.send_result(
                        id,
                        json!({
                            "agent": hit.agent_name,
                            "phase": routing.phase.phase_name,
                            "mode": mode_name,
                            "approval_strategy": approval_strategy.as_str(),
                            "review_policy": review_policy,
                            "cached": true,
                            "done": true,
                            "reviews": review_decisions,
                            "pipeline": {
                                "analyze": format!("{:?}", analyzed_task.task_type),
                                "route_roles": pipeline_routing
                                    .roles
                                    .iter()
                                    .map(|role| format!("{:?}", role))
                                    .collect::<Vec<_>>(),
                            },
                        }),
                    )
                    .await?;
                    self.record_trace_event(
                        &pipeline_trace,
                        "phase.learn",
                        "ok",
                        "learn",
                        json!({"source": "sqlite_cache"}),
                        None,
                        0,
                    );
                    return Ok(());
                }
                debug!(
                    trace_id = %pipeline_trace.trace_id,
                    phase = %routing.phase.phase_name,
                    "sqlite cache miss — forwarding to live agent"
                );
            }
        }

        let phase_name = routing.phase.phase_name.clone();
        let phase_options = routing.phase.options.clone();
        let phase_agent_options = routing
            .phase
            .options
            .as_ref()
            .and_then(|opts| opts.agent_options());
        let phase_principles = routing.phase.principles.clone();
        let phase_agent_names = routing.phase.agent_names.clone();
        let mut candidate_agents = routing.agents;
        let original_agent_order = candidate_agents
            .iter()
            .map(|(agent_name, _)| agent_name.clone())
            .collect::<Vec<_>>();
        let mut ranked_scores: Vec<(String, f64)> = Vec::new();

        if let Ok(state) = self.online_controller.lock() {
            let ranked = state.rank_agent_names_for_phase(&phase_name, &original_agent_order);
            let rank_index = ranked
                .iter()
                .enumerate()
                .map(|(idx, (name, _))| (name.clone(), idx))
                .collect::<HashMap<_, _>>();
            candidate_agents.sort_by_key(|(agent_name, _)| {
                rank_index
                    .get(agent_name)
                    .copied()
                    .unwrap_or(usize::MAX)
            });
            ranked_scores = ranked;
        }

        let ranked_agent_order = candidate_agents
            .iter()
            .map(|(agent_name, _)| agent_name.clone())
            .collect::<Vec<_>>();
        if original_agent_order != ranked_agent_order {
            self.record_trace_event(
                &pipeline_trace,
                "phase.route_adapt",
                "ok",
                "route",
                json!({
                    "reason": "online_controller_agent_ranking",
                    "original_order": original_agent_order,
                    "ranked_order": ranked_agent_order,
                    "scores": ranked_scores,
                }),
                None,
                0,
            );
        }

        let mut errors: Vec<String> = Vec::new();

            let breaker_failure_threshold = extra_u64(
                routing.phase.options.as_ref(),
                "circuit_breaker_failures",
            )
            .unwrap_or(DEFAULT_BREAKER_FAILURE_THRESHOLD as u64)
                as u32;
            let breaker_open_seconds = extra_u64(
                routing.phase.options.as_ref(),
                "circuit_breaker_open_seconds",
            )
            .unwrap_or(DEFAULT_BREAKER_OPEN_SECONDS as u64)
                as i64;

            for (agent_name, agent) in candidate_agents {
            let agent_started = Instant::now();
            let agent_span = chat_span.as_ref().or(request_span.as_ref()).and_then(|parent| {
                self.telemetry.start_child_span(
                    parent,
                    "acp.chat.agent",
                    vec![
                        KeyValue::new("agent.name", agent_name.clone()),
                        KeyValue::new("phase", phase_name.clone()),
                    ],
                )
            });
            match self.circuit_breakers.allow_request(&agent_name) {
                CircuitBreakerAdmission::Closed => {}
                CircuitBreakerAdmission::HalfOpenProbe => {
                    info!("agent '{}' entering half-open probe", agent_name);
                }
                CircuitBreakerAdmission::Rejected {
                    state,
                    retry_after_seconds,
                } => {
                    warn!(
                        "agent '{}' skipped due to circuit breaker state {}",
                        agent_name, state
                    );
                    errors.push(match retry_after_seconds {
                        Some(seconds) => format!(
                            "{}: skipped by circuit breaker ({}, retry after {}s)",
                            agent_name, state, seconds
                        ),
                        None => format!(
                            "{}: skipped by circuit breaker ({})",
                            agent_name, state
                        ),
                    });
                    if let Some(span) = agent_span {
                        self.telemetry.end_span(
                            span,
                            vec![
                                KeyValue::new("agent.status", "skipped"),
                                KeyValue::new("breaker.state", state.to_string()),
                            ],
                        );
                    }
                    continue;
                }
            }

            // Auto model selection: consult FlowModelSelector and AdaptiveModelSelector
            // to pick the best model for this agent/task, then inject into options.
            let selected_model_id: Option<String>;
            let agent_effective_options: Option<HashMap<String, Value>>;
            {
                let config = flow.config();
                if config.model_selection_mode != "explicit" && agent.supports_model_override() {
                    // Ask AdaptiveModelSelector if there is a preferred candidate
                    let candidates: Vec<String> = agent
                        .available_models()
                        .into_iter()
                        .map(|m| m.id)
                        .collect();
                    let adaptive_winner = self
                        .adaptive_model_selector
                        .lock()
                        .ok()
                        .and_then(|sel| sel.get_best_model(&candidates));

                    let selection = FlowModelSelector::select_model_for_agent(
                        agent.as_ref(),
                        &config,
                        prepared_input.latest_user_query.as_deref(),
                    );
                    // Prefer AdaptiveModelSelector winner (learned from history) when available
                    let chosen_id = adaptive_winner.or_else(|| {
                        selection.selected_model.map(|m| m.id)
                    });
                    if let Some(ref model_id) = chosen_id {
                        let mut opts = phase_agent_options.clone().unwrap_or_default();
                        opts.insert("model".to_string(), serde_json::Value::String(model_id.clone()));
                        agent_effective_options = Some(opts);
                    } else {
                        agent_effective_options = phase_agent_options.clone();
                    }
                    selected_model_id = chosen_id;
                } else {
                    agent_effective_options = phase_agent_options.clone();
                    selected_model_id = None;
                }
            }

            match self
                .run_agent_streaming(
                    id.clone(),
                    agent_name.clone(),
                    agent,
                    prepared_input.messages.clone(),
                    phase_principles.clone(),
                    agent_effective_options,
                    request_timeout(phase_options.as_ref()),
                    Some(phase_name.as_str()),
                    Some(pipeline_trace.trace_id.as_str()),
                )
                .await
            {
                Ok(response_text) => {
                    let agent_duration = agent_started.elapsed();
                    self.record_online_controller_agent_outcome(
                        &phase_name,
                        &agent_name,
                        true,
                        agent_duration,
                    );
                    self.circuit_breakers.record_success(&agent_name);
                    if let Some(ref model_id) = selected_model_id {
                        if let Ok(mut sel) = self.adaptive_model_selector.lock() {
                            sel.record_result(model_id, true);
                        }
                    }
                    if !bypass_cache && cache_enabled {
                        if let Some(cache) = self.cache_handle() {
                            let cache_key = build_cache_key_from_parts(
                                &phase_name,
                                &prepared_input.messages,
                                phase_principles.as_ref(),
                                phase_options.as_ref(),
                                mode_name,
                                approval_strategy.as_str(),
                                &phase_agent_names,
                            )?;
                            let ttl = phase_options
                                .as_ref()
                                .and_then(|opts| opts.cache_ttl_seconds);
                            self.cache_put(
                                cache.clone(),
                                cache_key,
                                response_text.clone(),
                                agent_name.clone(),
                                ttl,
                            )
                            .await?;
                            self.metrics.inc_cache_store();
                        }

                        let ttl = phase_options
                            .as_ref()
                            .and_then(|opts| opts.cache_ttl_seconds)
                            .unwrap_or(300);
                        self.memory_cache.put(
                            build_cache_key_from_parts(
                                &phase_name,
                                &prepared_input.messages,
                                phase_principles.as_ref(),
                                phase_options.as_ref(),
                                mode_name,
                                approval_strategy.as_str(),
                                &phase_agent_names,
                            )?,
                            response_text.clone(),
                            Some(agent_name.clone()),
                            ttl,
                        );
                    }

                    self.persist_memory_updates(
                        &phase_name,
                        phase_options.as_ref(),
                        prepared_input.latest_user_query.as_deref(),
                        &response_text,
                    )
                    .await?;

                    self.send_result(
                        id.clone(),
                        json!({
                            "agent": agent_name,
                            "phase": phase_name,
                            "mode": mode_name,
                            "approval_strategy": approval_strategy.as_str(),
                            "review_policy": review_policy,
                            "cached": false,
                            "done": true,
                            "reviews": review_decisions,
                            "pipeline": {
                                "analyze": format!("{:?}", analyzed_task.task_type),
                                "route_roles": pipeline_routing
                                    .roles
                                    .iter()
                                    .map(|role| format!("{:?}", role))
                                    .collect::<Vec<_>>(),
                                "success_rate": pipeline_routing.predicted_success_rate,
                            },
                        }),
                    )
                    .await?;
                    self.record_trace_event(
                        &pipeline_trace,
                        "phase.evaluate",
                        "ok",
                        "evaluate",
                        json!({
                            "predicted_success_rate": pipeline_routing.predicted_success_rate,
                            "risk_factors": pipeline_routing.risk_factors,
                        }),
                        None,
                        0,
                    );
                    self.record_trace_event(
                        &child_trace_context(&pipeline_trace, &format!("chat.agent.{}", agent_name)),
                        "phase.agent",
                        "ok",
                        &phase_name,
                        json!({ "agent": agent_name.clone() }),
                        None,
                        agent_started.elapsed().as_millis() as u64,
                    );
                    if let Some(span) = agent_span {
                        self.telemetry.end_span(
                            span,
                            vec![
                                KeyValue::new("agent.status", "ok"),
                                KeyValue::new(
                                    "agent.duration_ms",
                                    agent_duration.as_millis() as i64,
                                ),
                            ],
                        );
                    }
                    self.record_trace_event(
                        &pipeline_trace,
                        "phase.learn",
                        "ok",
                        "learn",
                        json!({"source": "agent_output"}),
                        None,
                        0,
                    );
                    // Auto-checkpoint: capture input messages + agent response for recovery
                    let mut cp_messages = original_messages.clone();
                    cp_messages.push(Message {
                        role: "assistant".to_string(),
                        content: response_text.clone(),
                    });
                    let cp_note = format!("{}/{}", phase_name, agent_name);
                    match self.create_conversation_checkpoint(
                        &auto_conv_id,
                        "main",
                        cp_messages,
                        Some(cp_note),
                    ) {
                        Ok(cp) => {
                            let _ = self
                                .send_notification(
                                    "conversation.checkpoint",
                                    json!({
                                        "checkpoint_id": cp.checkpoint_id,
                                        "conversation_id": cp.conversation_id,
                                        "branch_id": cp.branch_id,
                                        "auto": true,
                                    }),
                                )
                                .await;
                        }
                        Err(err) => {
                            warn!("auto-checkpoint skipped: {}", err);
                        }
                    }
                    // Section 7: QA gate — only for FullAuto + high-complexity requests
                    if matches!(mode, Some(ChatMode::FullAuto))
                        && analyzed_task.complexity >= 3
                    {
                        match run_action_check(&self.artifact_ledger(), ActionCheckKind::Qa) {
                            Ok(qa_report) => {
                                if !qa_report.ok {
                                    warn!(
                                        trace_id = %pipeline_trace.trace_id,
                                        phase = %phase_name,
                                        overall_status = ?qa_report.overall_status,
                                        "qa gate: artifacts incomplete — checkpoint and retest before promotion"
                                    );
                                }
                                let _ = self
                                    .send_notification(
                                        "chat.qa_gate",
                                        json!({
                                            "trace_id": pipeline_trace.trace_id,
                                            "ok": qa_report.ok,
                                            "overall_status": format!("{:?}", qa_report.overall_status),
                                            "evidence_refs": qa_report.evidence_refs,
                                        }),
                                    )
                                    .await;
                            }
                            Err(err) => {
                                warn!(
                                    trace_id = %pipeline_trace.trace_id,
                                    error = %err,
                                    "qa gate check skipped: ledger unavailable"
                                );
                            }
                        }
                    }
                    return Ok(());
                }
                Err(err) => {
                    let agent_duration = agent_started.elapsed();
                    self.record_online_controller_agent_outcome(
                        &phase_name,
                        &agent_name,
                        false,
                        agent_duration,
                    );
                    self.metrics.inc_agent_failures();
                    let failure_kind = classify_agent_failure(&err);
                    match failure_kind {
                        "timeout" => self.metrics.inc_agent_timeout_failures(),
                        "panic" => self.metrics.inc_agent_panic_failures(),
                        _ => self.metrics.inc_agent_other_failures(),
                    }
                    self.circuit_breakers.record_failure_with_config(
                        &agent_name,
                        breaker_failure_threshold,
                        breaker_open_seconds,
                    );
                    if let Some(ref model_id) = selected_model_id {
                        if let Ok(mut sel) = self.adaptive_model_selector.lock() {
                            sel.record_result(model_id, false);
                        }
                    }
                    if let Some(span) = agent_span {
                        self.telemetry.end_span(
                            span,
                            vec![
                                KeyValue::new("agent.status", "error"),
                                KeyValue::new("error", err.to_string()),
                                KeyValue::new(
                                    "agent.duration_ms",
                                    agent_duration.as_millis() as i64,
                                ),
                            ],
                        );
                    }
                    self.record_trace_event(
                        &child_trace_context(
                            &pipeline_trace,
                            &format!("chat.agent.{}", agent_name),
                        ),
                        "phase.agent",
                        "error",
                        &phase_name,
                        json!({
                            "agent": agent_name,
                            "failure_kind": failure_kind,
                        }),
                        Some(err.to_string()),
                        agent_duration.as_millis() as u64,
                    );
                    warn!("agent '{}' failed: {err:#}", agent_name);
                    errors.push(format!("{}: {}", agent_name, err));
                }
            }
            }

            self.record_trace_event(
                &pipeline_trace,
                "phase.evaluate",
                "error",
                "evaluate",
                json!({
                    "policy_status": "error",
                    "error_count": errors.len(),
                }),
                Some("all candidate agents failed".to_string()),
                0,
            );
            error!(
                trace_id = %pipeline_trace.trace_id,
                phase = %phase_name,
                error_count = errors.len(),
                "all candidate agents failed: {:?}",
                errors
            );
            self.send_error(
                id,
                -32603,
                crate::i18n::t("error.all_agents_failed"),
                Some(json!({ "errors": errors })),
            )
            .await
        }
        .await;

        if let Some(span) = chat_span {
            self.telemetry.end_span(
                span,
                vec![
                    KeyValue::new("chat.status", if result.is_ok() { "ok" } else { "error" }),
                    KeyValue::new("chat.duration_ms", started.elapsed().as_millis() as i64),
                ],
            );
        }

        if let Ok(mut state) = self.online_controller.lock() {
            state.record(result.is_ok(), started.elapsed().as_millis() as u64);
        }

        self.metrics.observe_chat_latency(started.elapsed());
        result
    }

    fn should_escalate_approval_strategy(&self) -> bool {
        self.online_controller
            .lock()
            .map(|state| state.should_escalate())
            .unwrap_or(false)
    }

}
