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

    fn create_conversation_checkpoint(
        &self,
        conversation_id: &str,
        branch_id: &str,
        messages: Vec<Message>,
        note: Option<String>,
    ) -> std::result::Result<ConversationCheckpoint, String> {
        if checkpoint_message_chars(&messages) > MAX_CHECKPOINT_MESSAGE_CHARS {
            return Err(format!(
                "checkpoint messages exceed max chars {}",
                MAX_CHECKPOINT_MESSAGE_CHARS
            ));
        }

        let checkpoint = {
            let mut store = self
                .conversation_store
                .lock()
                .map_err(|_| "conversation store lock poisoned".to_string())?;

            if !store.contains_key(conversation_id) && store.len() >= MAX_CONVERSATIONS_TRACKED {
                if let Some(evicted) =
                    evict_oldest_conversation(&mut store, &self.conversation_touch_order)
                {
                    warn!(
                        "conversation store reached limit ({}), evicted oldest conversation '{}'",
                        MAX_CONVERSATIONS_TRACKED, evicted
                    );
                }
            }

            let touched_at = now_ts();
            let state = store
                .entry(conversation_id.to_string())
                .or_insert_with(ConversationState::default);
            state.last_touched_at = touched_at;

            enforce_checkpoint_capacity(state, 1, None);

            let parent_checkpoint_id = state.branch_heads.get(branch_id).cloned();
            let checkpoint = ConversationCheckpoint {
                checkpoint_id: format!("cp-{}", CHECKPOINT_COUNTER.fetch_add(1, Ordering::Relaxed)),
                conversation_id: conversation_id.to_string(),
                branch_id: branch_id.to_string(),
                parent_checkpoint_id,
                created_at: now_ts(),
                note,
                messages,
            };

            state
                .branch_heads
                .insert(branch_id.to_string(), checkpoint.checkpoint_id.clone());
            state.checkpoints.push(checkpoint.clone());
            touch_conversation_order(&self.conversation_touch_order, conversation_id);
            checkpoint
        };

        self.persist_checkpoint_summary(&checkpoint);
        Ok(checkpoint)
    }

    fn list_conversation_checkpoints(
        &self,
        conversation_id: &str,
        branch_id: Option<&str>,
        limit: usize,
    ) -> std::result::Result<Vec<ConversationCheckpoint>, String> {
        let store = self
            .conversation_store
            .lock()
            .map_err(|_| "conversation store lock poisoned".to_string())?;
        let Some(state) = store.get(conversation_id) else {
            return Ok(Vec::new());
        };

        Ok(state
            .checkpoints
            .iter()
            .rev()
            .filter(|checkpoint| {
                branch_id
                    .map(|target| checkpoint.branch_id == target)
                    .unwrap_or(true)
            })
            .take(limit.max(1))
            .cloned()
            .collect::<Vec<_>>())
    }

    fn rollback_conversation_checkpoint(
        &self,
        conversation_id: &str,
        checkpoint_id: &str,
        target_branch: Option<&str>,
    ) -> Option<ConversationCheckpoint> {
        let restored = {
            let mut store = match self.conversation_store.lock() {
                Ok(guard) => guard,
                Err(_) => {
                    warn!(
                        "conversation rollback failed because conversation store lock is poisoned"
                    );
                    return None;
                }
            };
            let state = store.get_mut(conversation_id)?;
            state.last_touched_at = now_ts();
            let checkpoint = state
                .checkpoints
                .iter()
                .find(|candidate| candidate.checkpoint_id == checkpoint_id)
                .cloned()?;

            let branch = target_branch
                .unwrap_or(checkpoint.branch_id.as_str())
                .to_string();
            let restored = ConversationCheckpoint {
                checkpoint_id: format!("cp-{}", CHECKPOINT_COUNTER.fetch_add(1, Ordering::Relaxed)),
                conversation_id: conversation_id.to_string(),
                branch_id: branch.clone(),
                parent_checkpoint_id: Some(checkpoint.checkpoint_id.clone()),
                created_at: now_ts(),
                note: Some(format!("rollback:{}", checkpoint.checkpoint_id)),
                messages: checkpoint.messages.clone(),
            };

            enforce_checkpoint_capacity(state, 1, Some(checkpoint_id));
            state.checkpoints.push(restored.clone());
            state
                .branch_heads
                .insert(branch, restored.checkpoint_id.clone());
            touch_conversation_order(&self.conversation_touch_order, conversation_id);
            restored
        };

        self.persist_checkpoint_summary(&restored);
        Some(restored)
    }

    fn prune_conversation_checkpoints(
        &self,
        conversation_id: &str,
        branch_id: Option<&str>,
        keep: usize,
    ) -> ConversationPruneResult {
        let Ok(mut store) = self.conversation_store.lock() else {
            warn!("conversation prune skipped because conversation store lock is poisoned");
            return ConversationPruneResult::default();
        };
        let Some(state) = store.get_mut(conversation_id) else {
            return ConversationPruneResult::default();
        };
        state.last_touched_at = now_ts();

        let original_len = state.checkpoints.len();
        if let Some(target_branch) = branch_id {
            let mut branch_checkpoints: Vec<String> = state
                .checkpoints
                .iter()
                .filter(|cp| cp.branch_id == target_branch)
                .map(|cp| cp.checkpoint_id.clone())
                .collect();

            if branch_checkpoints.len() <= keep {
                return ConversationPruneResult::default();
            }

            let to_remove_count = branch_checkpoints.len() - keep;
            let to_remove: HashSet<String> = branch_checkpoints.drain(0..to_remove_count).collect();
            state
                .checkpoints
                .retain(|cp| !to_remove.contains(&cp.checkpoint_id));
        } else {
            // Prune globally: keep most recent `keep` checkpoints across all branches
            if state.checkpoints.len() <= keep {
                return ConversationPruneResult::default();
            }
            let drain_to = state.checkpoints.len() - keep;
            state.checkpoints.drain(0..drain_to);
        }

        let before_heads = state.branch_heads.clone();
        repair_conversation_branch_heads(state);
        let (repaired_heads, dropped_heads) =
            branch_head_adjustment_counts(&before_heads, &state.branch_heads);
        touch_conversation_order(&self.conversation_touch_order, conversation_id);

        ConversationPruneResult {
            removed: original_len - state.checkpoints.len(),
            repaired_heads,
            dropped_heads,
        }
    }

    fn record_online_controller_agent_outcome(
        &self,
        phase_name: &str,
        agent_name: &str,
        success: bool,
        duration: Duration,
    ) {
        if let Ok(mut state) = self.online_controller.lock() {
            state.record_agent_outcome(
                phase_name,
                agent_name,
                success,
                duration.as_millis() as u64,
            );
        }
    }

    fn infer_phase_name_with_flow(
        &self,
        flow: &FlowManager,
        explicit_phase: Option<&str>,
        mode: Option<ChatMode>,
    ) -> String {
        if let Some(phase) = explicit_phase {
            return phase.to_string();
        }

        match mode {
            Some(ChatMode::Ask) if flow.has_phase("review") => "review".to_string(),
            Some(ChatMode::Edit) | Some(ChatMode::Agent) | Some(ChatMode::FullAuto)
                if flow.has_phase("coding") =>
            {
                "coding".to_string()
            }
            _ => flow.default_phase().to_string(),
        }
    }

    async fn build_effective_messages(
        &self,
        phase: &ResolvedPhase,
        messages: &[Message],
    ) -> Result<PreparedChatInput> {
        let vector_config_snapshot = self.vector_config_snapshot();
        let optimized_messages = optimize_messages(messages, phase.options.as_ref());
        let latest_query = latest_user_query(&optimized_messages);
        let mut prepared_messages: Vec<Message> = Vec::new();

        if let Some(vector_store) = self.vector_store_handle() {
            let tuned_state = if let Some(autotune) = self.autotune_handle() {
                Some(autotune_state_snapshot(&autotune).await)
            } else {
                None
            };

            let summary_enabled =
                effective_summary_enabled(phase.options.as_ref(), vector_config_snapshot.as_ref());
            let summary_trigger = effective_summary_trigger_messages(
                phase.options.as_ref(),
                vector_config_snapshot.as_ref(),
            );

            if summary_enabled && optimized_messages.len() >= summary_trigger {
                self.metrics.inc_summary_read();
                if let Some(summary) = self
                    .vector_get_phase_summary(vector_store.clone(), phase.phase_name.clone())
                    .await?
                {
                    self.metrics.inc_summary_hit();
                    prepared_messages.push(Message {
                        role: "user".to_string(),
                        content: format!("Conversation summary for this phase:\n{}", summary),
                    });
                }
            }

            let vector_enabled =
                effective_vector_enabled(phase.options.as_ref(), vector_config_snapshot.as_ref());
            if vector_enabled {
                let vector_auto =
                    effective_vector_auto(phase.options.as_ref(), vector_config_snapshot.as_ref());
                let min_query_chars = effective_vector_min_query_chars(
                    phase.options.as_ref(),
                    vector_config_snapshot.as_ref(),
                    tuned_state.as_ref(),
                );

                if let Some(query) = latest_query.as_ref() {
                    let should_search = if vector_auto {
                        query.chars().count() >= min_query_chars
                    } else {
                        !query.trim().is_empty()
                    };

                    if should_search {
                        self.metrics.inc_vector_search();
                        let top_k = effective_vector_top_k(
                            phase.options.as_ref(),
                            vector_config_snapshot.as_ref(),
                            tuned_state.as_ref(),
                        );
                        let min_similarity = effective_vector_min_similarity(
                            phase.options.as_ref(),
                            vector_config_snapshot.as_ref(),
                        );
                        let max_snippet_chars = effective_vector_max_snippet_chars(
                            phase.options.as_ref(),
                            vector_config_snapshot.as_ref(),
                        );

                        let (hits, feedback) = self
                            .vector_search(
                                vector_store.clone(),
                                phase.phase_name.clone(),
                                query.clone(),
                                top_k,
                                min_similarity,
                                max_snippet_chars,
                            )
                            .await?;

                        // Record precision feedback for autotune if enabled
                        if let Some(autotune) = self.autotune_handle() {
                            if let Some(config) = self.autotune_config_snapshot() {
                                let state_to_persist = {
                                    let mut state = autotune.lock().await;
                                    state.record_vector_search(feedback.avg_similarity, &config);

                                    let mut mutated = false;
                                    if state.advance_cooldown_window(&config) {
                                        mutated = true;
                                    } else if state.should_evaluate(&config) {
                                        state.evaluate_and_adjust(&config);
                                        mutated = true;
                                    }

                                    if mutated {
                                        Some(state.clone())
                                    } else {
                                        None
                                    }
                                };

                                if let Some(state) = state_to_persist {
                                    if let Some(path) = self.autotune_state_path_snapshot() {
                                        if let Err(e) = state.save(path.as_str()) {
                                            warn!(
                                                "{}",
                                                crate::i18n::tf(
                                                    "warning.failed_persist_autotune",
                                                    &[("error", &format!("{}", e))]
                                                )
                                            );
                                        }
                                    } else {
                                        warn!("autotune update skipped persistence because no resolved state path is available");
                                    }
                                }
                            }
                        }

                        if !hits.is_empty() {
                            self.metrics.inc_vector_hit();
                            prepared_messages.push(Message {
                                role: "user".to_string(),
                                content: build_vector_context_message(&hits),
                            });
                        }
                    }
                }
            }
        }

        prepared_messages.extend(optimized_messages);

        Ok(PreparedChatInput {
            messages: prepared_messages,
            latest_user_query: latest_query,
        })
    }

    async fn persist_memory_updates(
        &self,
        phase_name: &str,
        options: Option<&PhaseOptions>,
        latest_user_query: Option<&str>,
        response_text: &str,
    ) -> Result<()> {
        let vector_config_snapshot = self.vector_config_snapshot();
        let Some(vector_store) = self.vector_store_handle() else {
            return Ok(());
        };

        if let Some(query) = latest_user_query {
            self.vector_upsert(
                vector_store.clone(),
                phase_name.to_string(),
                query.to_string(),
                response_text.to_string(),
            )
            .await?;
            self.metrics.inc_vector_store();
        }

        let summary_enabled = effective_summary_enabled(options, vector_config_snapshot.as_ref());
        if !summary_enabled {
            return Ok(());
        }

        self.metrics.inc_summary_read();
        let existing_summary = self
            .vector_get_phase_summary(vector_store.clone(), phase_name.to_string())
            .await?;
        if existing_summary.is_some() {
            self.metrics.inc_summary_hit();
        }

        let summary_max_chars =
            effective_summary_max_chars(options, vector_config_snapshot.as_ref());
        let new_summary = append_recent_summary(
            existing_summary.as_deref(),
            latest_user_query,
            response_text,
            summary_max_chars,
        );

        self.vector_upsert_phase_summary(vector_store.clone(), phase_name.to_string(), new_summary)
            .await?;
        self.metrics.inc_summary_store();
        Ok(())
    }

    async fn cache_get(
        &self,
        cache: Arc<ResponseCache>,
        cache_key: String,
    ) -> Result<Option<crate::cache::CachedResponse>> {
        spawn_blocking(move || cache.get(&cache_key))
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf(
                        "error.task_join",
                        &[("task", "cache_get"), ("error", &format!("{}", e))]
                    )
                )
            })?
    }

    async fn cache_put(
        &self,
        cache: Arc<ResponseCache>,
        cache_key: String,
        response_text: String,
        agent_name: String,
        ttl: Option<u64>,
    ) -> Result<()> {
        spawn_blocking(move || cache.put(&cache_key, &response_text, &agent_name, ttl))
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf(
                        "error.task_join",
                        &[("task", "cache_put"), ("error", &format!("{}", e))]
                    )
                )
            })?
    }

    async fn cache_entry_count(&self, cache: Arc<ResponseCache>) -> Result<u64> {
        spawn_blocking(move || cache.entry_count())
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf(
                        "error.task_join",
                        &[("task", "cache_entry_count"), ("error", &format!("{}", e))]
                    )
                )
            })?
    }

    async fn cache_clear(&self, cache: Arc<ResponseCache>) -> Result<usize> {
        spawn_blocking(move || cache.clear_all())
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf(
                        "error.task_join",
                        &[("task", "cache_clear"), ("error", &format!("{}", e))]
                    )
                )
            })?
    }

    async fn vector_search(
        &self,
        vector_store: Arc<VectorStore>,
        phase: String,
        query: String,
        top_k: usize,
        min_similarity: f32,
        max_snippet_chars: usize,
    ) -> Result<(Vec<VectorHit>, crate::vector::VectorPrecisionFeedback)> {
        spawn_blocking(move || {
            vector_store.search(&phase, &query, top_k, min_similarity, max_snippet_chars)
        })
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "{}",
                crate::i18n::tf(
                    "error.task_join",
                    &[("task", "vector_search"), ("error", &format!("{}", e))]
                )
            )
        })?
    }

    async fn vector_get_phase_summary(
        &self,
        vector_store: Arc<VectorStore>,
        phase: String,
    ) -> Result<Option<String>> {
        spawn_blocking(move || vector_store.get_phase_summary(&phase))
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf(
                        "error.task_join",
                        &[
                            ("task", "vector_get_phase_summary"),
                            ("error", &format!("{}", e))
                        ]
                    )
                )
            })?
    }

    async fn vector_upsert(
        &self,
        vector_store: Arc<VectorStore>,
        phase: String,
        query: String,
        response_text: String,
    ) -> Result<()> {
        spawn_blocking(move || vector_store.upsert(&phase, &query, &response_text))
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf(
                        "error.task_join",
                        &[("task", "vector_upsert"), ("error", &format!("{}", e))]
                    )
                )
            })?
    }

    async fn vector_entry_counts(&self, vector_store: Arc<VectorStore>) -> Result<(u64, u64)> {
        spawn_blocking(move || {
            let memory = vector_store.memory_entry_count()?;
            let summaries = vector_store.summary_entry_count()?;
            Ok::<(u64, u64), anyhow::Error>((memory, summaries))
        })
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "{}",
                crate::i18n::tf(
                    "error.task_join",
                    &[
                        ("task", "vector_entry_counts"),
                        ("error", &format!("{}", e))
                    ]
                )
            )
        })?
    }

    async fn vector_clear(&self, vector_store: Arc<VectorStore>) -> Result<(usize, usize)> {
        spawn_blocking(move || vector_store.clear_all())
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf(
                        "error.task_join",
                        &[("task", "vector_clear"), ("error", &format!("{}", e))]
                    )
                )
            })?
    }

    async fn vector_upsert_phase_summary(
        &self,
        vector_store: Arc<VectorStore>,
        phase: String,
        summary: String,
    ) -> Result<()> {
        spawn_blocking(move || vector_store.upsert_phase_summary(&phase, &summary))
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf(
                        "error.task_join",
                        &[
                            ("task", "vector_upsert_phase_summary"),
                            ("error", &format!("{}", e))
                        ]
                    )
                )
            })?
    }

    async fn run_dual_review_gate(
        &self,
        id: Option<Value>,
        messages: &[Message],
        phase_options: Option<&PhaseOptions>,
        parent_span: Option<&OtelContext>,
        pipeline_trace: &RequestTraceContext,
    ) -> Result<ReviewGateOutcome> {
        let started = Instant::now();
        self.metrics.inc_review_gate();
        let review_span = parent_span.and_then(|parent| {
            self.telemetry.start_child_span(
                parent,
                "acp.chat.review_gate",
                vec![KeyValue::new("gate.mode", "dual")],
            )
        });

        let timeout_policy = ReviewTimeoutPolicy::from_options(phase_options);
        let gate_timeout = extra_u64(phase_options, "review_gate_timeout_seconds")
            .or_else(|| phase_options.and_then(|opts| opts.review_timeout_seconds))
            .or_else(|| phase_options.and_then(|opts| opts.request_timeout_seconds))
            .map(Duration::from_secs);
        let gate_deadline = gate_timeout.map(|limit| Instant::now() + limit);

        let result = async {
            let (flow, registry) = self.routing_handles()?;

            let review_routing = flow
                .resolve(Some("review".to_string()), registry.as_ref())
                .map_err(|err| {
                    anyhow::anyhow!(
                        "{}",
                        crate::i18n::tf(
                            "error.review_phase_required",
                            &[("error", &format!("{err}"))]
                        )
                    )
                })?;

            let mut reviewer_names = phase_options
                .and_then(|options| options.full_auto_review_agents.clone())
                .unwrap_or_else(|| review_routing.phase.agent_names.clone());

            let review_phase_name = review_routing.phase.phase_name.clone();
            let original_reviewer_order = reviewer_names.clone();
            let mut reviewer_scores: Vec<(String, f64)> = Vec::new();
            if let Ok(state) = self.online_controller.lock() {
                let ranked = state.rank_agent_names_for_phase(&review_phase_name, &reviewer_names);
                let rank_index = ranked
                    .iter()
                    .enumerate()
                    .map(|(idx, (name, _))| (name.clone(), idx))
                    .collect::<HashMap<_, _>>();
                reviewer_names
                    .sort_by_key(|name| rank_index.get(name).copied().unwrap_or(usize::MAX));
                reviewer_scores = ranked;
            }

            if reviewer_names != original_reviewer_order {
                self.record_trace_event(
                    &child_trace_context(pipeline_trace, "chat.review.route_adapt"),
                    "phase.review_route_adapt",
                    "ok",
                    "review",
                    json!({
                        "reason": "online_controller_reviewer_ranking",
                        "original_order": original_reviewer_order,
                        "ranked_order": reviewer_names,
                        "scores": reviewer_scores,
                    }),
                    None,
                    0,
                );
            }

            if reviewer_names.len() > 2 {
                reviewer_names.truncate(2);
            }

            let min_reviewers = extra_u64(phase_options, "min_reviewers").unwrap_or(2) as usize;
            let required_approvals = extra_u64(phase_options, "required_approvals")
                .unwrap_or(min_reviewers as u64)
                .max(1) as usize;

            if reviewer_names.len() < min_reviewers {
                anyhow::bail!(
                    "complex full_auto mode requires at least {} review agents",
                    min_reviewers
                );
            }

            let mut prepared_review = self
                .build_effective_messages(&review_routing.phase, messages)
                .await?;
            prepared_review.messages.push(Message {
                role: "user".to_string(),
                content: review_gate_prompt(),
            });

            let mut decisions = Vec::new();
            let mut approved_count = 0usize;
            let min_review_chars =
                extra_u64(phase_options, "review_min_response_chars").unwrap_or(8) as usize;
            let total_reviewers = reviewer_names.len();
            for (idx, reviewer) in reviewer_names.into_iter().enumerate() {
                let reviewer_started = Instant::now();
                let reviewer_span = review_span.as_ref().and_then(|parent| {
                    self.telemetry.start_child_span(
                        parent,
                        "acp.chat.reviewer",
                        vec![KeyValue::new("reviewer", reviewer.clone())],
                    )
                });
                if let Some(deadline) = gate_deadline {
                    let now = Instant::now();
                    if now >= deadline {
                        let err = anyhow::anyhow!(
                            "review gate timed out after {}s",
                            gate_timeout.map(|d| d.as_secs()).unwrap_or(0)
                        );
                        self.metrics.inc_review_gate_timeout();
                        record_agent_failure_metrics(self.metrics.as_ref(), &err);

                        return match timeout_policy {
                            ReviewTimeoutPolicy::Reject => {
                                self.metrics.inc_review_gate_rejected();
                                Ok(ReviewGateOutcome::Rejected(decisions))
                            }
                            ReviewTimeoutPolicy::DegradeSingle => {
                                if approved_count >= 1 {
                                    self.metrics.inc_review_gate_degraded();
                                    self.metrics.inc_review_gate_approved();
                                    Ok(ReviewGateOutcome::Degraded(decisions))
                                } else {
                                    self.metrics.inc_review_gate_rejected();
                                    Ok(ReviewGateOutcome::Rejected(decisions))
                                }
                            }
                        };
                    }
                }

                let agent = registry.get(&reviewer).ok_or_else(|| {
                    anyhow::anyhow!(
                        "{}",
                        crate::i18n::tf("error.review_agent_not_available", &[("name", &reviewer)])
                    )
                })?;

                let reviewer_timeout = if let Some(deadline) = gate_deadline {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    let configured =
                        review_timeout(review_routing.phase.options.as_ref(), phase_options);
                    match configured {
                        Some(configured_limit) => Some(std::cmp::min(configured_limit, remaining)),
                        None => Some(remaining),
                    }
                } else {
                    review_timeout(review_routing.phase.options.as_ref(), phase_options)
                };

                let response = match self
                    .run_agent_collecting(
                        reviewer.clone(),
                        agent,
                        prepared_review.messages.clone(),
                        review_routing.phase.principles.clone(),
                        review_routing
                            .phase
                            .options
                            .as_ref()
                            .and_then(|opts| opts.agent_options()),
                        reviewer_timeout,
                    )
                    .await
                {
                    Ok(response) => response,
                    Err(err) => {
                        self.record_online_controller_agent_outcome(
                            &review_phase_name,
                            &reviewer,
                            false,
                            reviewer_started.elapsed(),
                        );
                        if let Some(span) = reviewer_span {
                            self.telemetry.end_span(
                                span,
                                vec![
                                    KeyValue::new("review.status", "error"),
                                    KeyValue::new("error", err.to_string()),
                                ],
                            );
                        }
                        record_agent_failure_metrics(self.metrics.as_ref(), &err);
                        let err_message = err.to_string();
                        if classify_agent_failure(&err) == "timeout" {
                            self.metrics.inc_review_gate_timeout();
                            return match timeout_policy {
                                ReviewTimeoutPolicy::Reject => {
                                    self.metrics.inc_review_gate_rejected();
                                    Ok(ReviewGateOutcome::Rejected(decisions))
                                }
                                ReviewTimeoutPolicy::DegradeSingle => {
                                    if approved_count >= 1 {
                                        self.metrics.inc_review_gate_degraded();
                                        self.metrics.inc_review_gate_approved();
                                        Ok(ReviewGateOutcome::Degraded(decisions))
                                    } else {
                                        self.metrics.inc_review_gate_rejected();
                                        Ok(ReviewGateOutcome::Rejected(decisions))
                                    }
                                }
                            };
                        }
                        return Err(anyhow::anyhow!(err_message));
                    }
                };

                let verdict = review_verdict(&response, min_review_chars);
                self.record_online_controller_agent_outcome(
                    &review_phase_name,
                    &reviewer,
                    verdict != ReviewVerdict::Invalid,
                    reviewer_started.elapsed(),
                );
                if verdict == ReviewVerdict::Invalid {
                    self.metrics.inc_review_gate_invalid_response();
                }
                let decision = ReviewDecision {
                    reviewer: reviewer.clone(),
                    verdict: verdict.as_str().to_string(),
                    response: response.clone(),
                };

                self.send_notification(
                    "chat.review",
                    json!({
                        "id": id.clone(),
                        "reviewer": reviewer,
                        "verdict": decision.verdict,
                    }),
                )
                .await?;

                decisions.push(decision);
                if let Some(span) = reviewer_span {
                    self.telemetry.end_span(
                        span,
                        vec![
                            KeyValue::new("review.status", verdict.as_str().to_string()),
                            KeyValue::new(
                                "review.duration_ms",
                                reviewer_started.elapsed().as_millis() as i64,
                            ),
                        ],
                    );
                }

                if verdict.is_approved() {
                    approved_count += 1;
                    if approved_count >= required_approvals {
                        self.metrics.inc_review_gate_approved();
                        return Ok(ReviewGateOutcome::Approved(decisions));
                    }
                }

                let remaining = total_reviewers - (idx + 1);
                if approved_count + remaining < required_approvals {
                    self.metrics.inc_review_gate_rejected();
                    return Ok(ReviewGateOutcome::Rejected(decisions));
                }
            }

            if approved_count >= required_approvals {
                self.metrics.inc_review_gate_approved();
                Ok(ReviewGateOutcome::Approved(decisions))
            } else {
                self.metrics.inc_review_gate_rejected();
                Ok(ReviewGateOutcome::Rejected(decisions))
            }
        };

        let output = result.await;
        if let Some(span) = review_span {
            self.telemetry.end_span(
                span,
                vec![
                    KeyValue::new("gate.status", if output.is_ok() { "ok" } else { "error" }),
                    KeyValue::new("gate.duration_ms", started.elapsed().as_millis() as i64),
                ],
            );
        }
        self.metrics.observe_review_latency(started.elapsed());
        output
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_agent_streaming(
        &self,
        id: Option<Value>,
        agent_name: String,
        agent: Arc<dyn Agent>,
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: Option<HashMap<String, Value>>,
        timeout_limit: Option<Duration>,
        phase_name: Option<&str>,
        trace_id: Option<&str>,
    ) -> Result<String> {
        let started = Instant::now();
        let (sender, mut receiver) = mpsc::unbounded_channel::<String>();
        let agent_task =
            tokio::spawn(async move { agent.chat(messages, principles, options, sender).await });

        let mut response_text = String::new();
        let mut stream_chunks: usize = 0;
        let mut streamed_chars: usize = 0;
        let collect_stream = async {
            while let Some(token) = receiver.recv().await {
                let token_chars = token.chars().count();
                let projected_chunks = stream_chunks.saturating_add(1);
                let projected_chars = streamed_chars.saturating_add(token_chars);
                if stream_would_exceed_limits(stream_chunks, streamed_chars, token_chars) {
                    return Err(anyhow::anyhow!(
                        "agent '{}' stream exceeded limits (chunks={}, chars={})",
                        agent_name,
                        projected_chunks,
                        projected_chars
                    ));
                }
                response_text.push_str(&token);
                stream_chunks = projected_chunks;
                streamed_chars = projected_chars;
                let payload = stream_chunk_notification(
                    &id,
                    &agent_name,
                    &token,
                    stream_chunks,
                    streamed_chars,
                    None,
                    phase_name,
                    trace_id,
                );
                self.send_notification("chat.stream", payload).await?;
            }

            Ok::<(), anyhow::Error>(())
        };

        if let Some(limit) = timeout_limit {
            if timeout(limit, collect_stream).await.is_err() {
                agent_task.abort();
                return Err(anyhow::anyhow!(
                    "agent '{}' timed out after {}s",
                    agent_name,
                    limit.as_secs()
                ));
            }
        } else {
            collect_stream.await?;
        }

        let result = match agent_task.await {
            Ok(Ok(())) => {
                let done_payload = stream_done_notification(
                    &id,
                    &agent_name,
                    stream_chunks,
                    streamed_chars,
                    None,
                    phase_name,
                    trace_id,
                    started.elapsed().as_millis() as u64,
                );
                self.send_notification("chat.stream.done", done_payload)
                    .await?;
                Ok(response_text)
            }
            Ok(Err(err)) => Err(err),
            Err(join_err) => Err(anyhow::anyhow!(
                "agent '{}' panic: {}",
                agent_name,
                join_err
            )),
        };

        self.metrics.observe_agent_latency(started.elapsed());
        result
    }

    async fn run_agent_collecting(
        &self,
        agent_name: String,
        agent: Arc<dyn Agent>,
        messages: Vec<Message>,
        principles: Option<Vec<String>>,
        options: Option<HashMap<String, Value>>,
        timeout_limit: Option<Duration>,
    ) -> Result<String> {
        let started = Instant::now();
        let (sender, mut receiver) = mpsc::unbounded_channel::<String>();
        let agent_task =
            tokio::spawn(async move { agent.chat(messages, principles, options, sender).await });

        let mut response_text = String::new();
        let mut stream_chunks: usize = 0;
        let mut streamed_chars: usize = 0;
        let collect_stream = async {
            while let Some(token) = receiver.recv().await {
                let token_chars = token.chars().count();
                let projected_chunks = stream_chunks.saturating_add(1);
                let projected_chars = streamed_chars.saturating_add(token_chars);
                if stream_would_exceed_limits(stream_chunks, streamed_chars, token_chars) {
                    return Err(anyhow::anyhow!(
                        "agent '{}' stream exceeded limits (chunks={}, chars={})",
                        agent_name,
                        projected_chunks,
                        projected_chars
                    ));
                }
                response_text.push_str(&token);
                stream_chunks = projected_chunks;
                streamed_chars = projected_chars;
            }

            Ok::<(), anyhow::Error>(())
        };

        if let Some(limit) = timeout_limit {
            if timeout(limit, collect_stream).await.is_err() {
                agent_task.abort();
                return Err(anyhow::anyhow!(
                    "agent '{}' timed out after {}s",
                    agent_name,
                    limit.as_secs()
                ));
            }
        } else {
            collect_stream.await?;
        }

        let result = match agent_task.await {
            Ok(Ok(())) => Ok(response_text),
            Ok(Err(err)) => Err(err),
            Err(join_err) => Err(anyhow::anyhow!(
                "agent '{}' panic: {}",
                agent_name,
                join_err
            )),
        };

        self.metrics.observe_review_latency(started.elapsed());
        result
    }

    async fn reload_runtime_config(&self) -> Result<Value> {
        let config_path = self
            .config_path
            .as_ref()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf(
                        "error.config_reload_unavailable",
                        &[("reason", "config path not set")]
                    )
                )
            })?
            .clone();
        let client = self.http_client.clone().ok_or_else(|| {
            anyhow::anyhow!(
                "{}",
                crate::i18n::tf(
                    "error.config_reload_unavailable",
                    &[("reason", "http client not set")]
                )
            )
        })?;

        let new_config = AppConfig::load(&config_path)?;
        let health_report =
            validate_runtime_readiness(&config_path, &new_config).map_err(|err| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf(
                        "error.config_reload_failed",
                        &[("error", &format!("{err}"))]
                    )
                )
            })?;
        for warning in &health_report.warnings {
            let severity = match warning.severity {
                crate::config::ConfigWarningSeverity::Critical => "critical",
                crate::config::ConfigWarningSeverity::Warn => "warn",
                crate::config::ConfigWarningSeverity::Info => "info",
            };
            warn!(
                "config reload warning [{}:{}] {}",
                severity, warning.code, warning.message
            );
        }

        let config_arc = Arc::new(new_config);
        let new_registry = Arc::new(AgentRegistry::from_config(Arc::clone(&config_arc), client)?);
        let new_flow = Arc::new(FlowManager::new(
            Arc::clone(&config_arc),
            self.forced_phase.clone(),
        ));

        let new_cache = match &config_arc.cache {
            Some(cache_cfg) if cache_cfg.enabled => {
                let cache_path = if PathBuf::from(&cache_cfg.path).is_absolute() {
                    PathBuf::from(&cache_cfg.path)
                } else {
                    config_path
                        .parent()
                        .unwrap_or_else(|| std::path::Path::new("."))
                        .join(&cache_cfg.path)
                };
                Some(Arc::new(ResponseCache::new(
                    &cache_path,
                    cache_cfg.default_ttl_seconds,
                    cache_cfg.max_entries,
                )?))
            }
            _ => None,
        };

        let new_vector_store = match &config_arc.vector {
            Some(vector_cfg) if vector_cfg.enabled => {
                let vector_path = if PathBuf::from(&vector_cfg.path).is_absolute() {
                    PathBuf::from(&vector_cfg.path)
                } else {
                    config_path
                        .parent()
                        .unwrap_or_else(|| std::path::Path::new("."))
                        .join(&vector_cfg.path)
                };
                Some(Arc::new(VectorStore::new(
                    &vector_path,
                    vector_cfg.dimensions,
                    vector_cfg.max_entries,
                )?))
            }
            _ => None,
        };

        let new_autotune_state_path = config_arc.autotune.as_ref().and_then(|autotune_cfg| {
            if !autotune_cfg.enabled {
                return None;
            }
            Some(
                if PathBuf::from(&autotune_cfg.state_path).is_absolute() {
                    PathBuf::from(&autotune_cfg.state_path)
                } else {
                    config_path
                        .parent()
                        .unwrap_or_else(|| std::path::Path::new("."))
                        .join(&autotune_cfg.state_path)
                }
                .to_string_lossy()
                .to_string(),
            )
        });

        let (new_autotune, new_autotune_config) = match config_arc.autotune.as_ref() {
            Some(autotune_cfg) if autotune_cfg.enabled => {
                let state_path = new_autotune_state_path
                    .clone()
                    .unwrap_or_else(|| "acp_autotune_state.json".to_string());
                let state = AutoTuneState::load_or_default(&state_path, autotune_cfg);
                (
                    Some(Arc::new(Mutex::new(state))),
                    Some(autotune_cfg.clone()),
                )
            }
            _ => (None, None),
        };

        let new_runtime_config = config_arc.runtime.clone().unwrap_or_default();

        {
            let mut flow_guard = self.flow.lock().map_err(|_| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf("error.mutex_poisoned", &[("name", "flow")])
                )
            })?;
            *flow_guard = new_flow;
        }
        {
            let mut registry_guard = self.registry.lock().map_err(|_| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf("error.mutex_poisoned", &[("name", "registry")])
                )
            })?;
            *registry_guard = new_registry;
        }
        {
            let mut cache_guard = self.cache.lock().map_err(|_| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf("error.mutex_poisoned", &[("name", "cache")])
                )
            })?;
            *cache_guard = new_cache;
        }
        {
            let mut vector_guard = self.vector_store.lock().map_err(|_| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf("error.mutex_poisoned", &[("name", "vector")])
                )
            })?;
            *vector_guard = new_vector_store;
        }
        {
            let mut vector_cfg_guard = self
                .vector_config
                .lock()
                .map_err(|_| anyhow::anyhow!("vector_config mutex poisoned"))?;
            *vector_cfg_guard = config_arc.vector.clone();
        }
        {
            let mut autotune_guard = self.autotune.lock().map_err(|_| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf("error.mutex_poisoned", &[("name", "autotune")])
                )
            })?;
            *autotune_guard = new_autotune;
        }
        {
            let mut autotune_cfg_guard = self.autotune_config.lock().map_err(|_| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf("error.mutex_poisoned", &[("name", "autotune_config")])
                )
            })?;
            *autotune_cfg_guard = new_autotune_config;
        }
        {
            let mut autotune_path_guard = self
                .autotune_state_path
                .lock()
                .map_err(|_| anyhow::anyhow!("autotune_state_path mutex poisoned"))?;
            *autotune_path_guard = new_autotune_state_path;
        }
        {
            let mut runtime_guard = self.runtime_config.lock().map_err(|_| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf("error.mutex_poisoned", &[("name", "runtime_config")])
                )
            })?;
            *runtime_guard = new_runtime_config;
        }

        // Clear dynamic guardrails to avoid stale state after topology changes.
        if let Ok(mut g) = self.circuit_breakers.inner.lock() {
            g.clear();
        }
        if let Ok(mut g) = self.phase_rate_limiter.inner.lock() {
            g.clear();
        }
        self.inflight_limiter.clear();

        Ok(json!({
            "ok": true,
            "note": crate::i18n::t("info.resources_reloaded"),
            "path": config_path,
            "warning_count": health_report.total,
            "warnings": health_report.warning_messages(),
            "profile_recommendation": health_report.profile_recommendation,
            "recommendations": health_report.recommendations,
            "health": health_report,
        }))
    }

    async fn send_result(&self, id: Option<Value>, result: Value) -> Result<()> {
        self.write_response(JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        })
        .await
    }

    async fn send_error(
        &self,
        id: Option<Value>,
        code: i64,
        message: String,
        data: Option<Value>,
    ) -> Result<()> {
        self.write_response(JsonRpcResponse {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message,
                data,
            }),
        })
        .await
    }

    async fn send_notification(&self, method: &str, params: Value) -> Result<()> {
        let payload = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.write_json_line(&payload).await
    }

    async fn write_response(&self, response: JsonRpcResponse) -> Result<()> {
        let value = serde_json::to_value(response)?;
        self.write_json_line(&value).await
    }

    async fn write_json_line(&self, value: &Value) -> Result<()> {
        let mut stdout = self.output.lock().await;
        let mut encoded = serde_json::to_vec(value)?;
        encoded.push(b'\n');
        stdout.write_all(&encoded).await?;
        stdout.flush().await?;
        Ok(())
    }
}
