impl AcpServer {
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

}
