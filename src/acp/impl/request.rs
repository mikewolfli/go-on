impl AcpServer {
    async fn handle_request(&self, request: JsonRpcRequest) -> Result<()> {
        let trace = self.new_request_trace(&request);
        let request_span = self.telemetry.start_root_span(
            "acp.request",
            &format!("{}:{}", trace.method, trace.request_id),
            vec![
                KeyValue::new("rpc.method", trace.method.clone()),
                KeyValue::new("rpc.request_id", trace.request_id.clone()),
                KeyValue::new("trace.id", trace.trace_id.clone()),
            ],
        );
        self.record_trace_event(
            &trace,
            "request.start",
            "ok",
            "rpc",
            json!({
                "method": trace.method,
                "request_id": trace.request_id,
            }),
            None,
            0,
        );

        // Enhanced telemetry logging
        telemetry_enhanced::log::request_start("rpc", &trace.method, &trace.request_id);

        let method = request.method.clone();
        let request_id = request.id.clone();
        let started = Instant::now();
        let result = async {
            if self.lifecycle.is_shutting_down() && method != "shutdown" {
                return self
                    .send_error(
                        request_id,
                        -32031,
                        "server is shutting down".to_string(),
                        Some(serde_json::to_value(self.lifecycle.snapshot())?),
                    )
                    .await;
            }

            match method.as_str() {
            "initialize" => {
                // Measure initialization performance
                let (result, duration) = performance::utils::measure_time(|| {
                    json!({
                        "name": "go-on",
                        "protocol": "acp",
                        "capabilities": {
                            "chat": true,
                            "streaming": true,
                            "phase": true,
                            "metrics": true,
                            "debug_panel": true,
                            "mcp_adapter": true,
                            "conversation_control": true,
                            "autotune": self.autotune_config_snapshot().map(|cfg| cfg.enabled).unwrap_or(false),
                        }
                    })
                });

                // Log performance metrics
                debug!("initialize request handled in {:?}", duration);
                self.send_result(request_id, result).await
            }
            "mcp.initialize" => {
                self.send_result(
                    request_id,
                    json!({
                        "protocolVersion": crate::mcp::MCP_VERSION,
                        "capabilities": {
                            "tools": {},
                        },
                        "serverInfo": {
                            "name": "go-on",
                            "version": env!("CARGO_PKG_VERSION"),
                        }
                    }),
                )
                .await
            }
            "mcp.tools.list" => {
                self.send_result(
                    request_id,
                    json!({
                        "tools": [
                            {
                                "name": "acp_debug_panel_get",
                                "description": "Get runtime debug panel snapshot",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "limit": {"type": "number"}
                                    }
                                }
                            },
                            {
                                "name": "acp_trace_get",
                                "description": "Get recent trace events",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "limit": {"type": "number"}
                                    }
                                }
                            },
                            {
                                "name": "acp_runtime_health",
                                "description": "Get runtime health summary",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {}
                                }
                            },
                            {
                                "name": "acp_task_plan",
                                "description": "Build and persist a controlled task plan",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "task": {"type": "string"}
                                    },
                                    "required": ["task"]
                                }
                            },
                            {
                                "name": "acp_action_check",
                                "description": "Run BLUE2 action checks against .goon artifacts",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "kind": {"type": "string"}
                                    }
                                }
                            },
                            {
                                "name": "acp_conversation_checkpoint_list",
                                "description": "List conversation checkpoints",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "conversation_id": {"type": "string"},
                                        "branch_id": {"type": "string"},
                                        "limit": {"type": "number"}
                                    }
                                }
                            }
                        ]
                    }),
                )
                .await
            }
            "mcp.tools.call" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let tool_name = match params.get("name").and_then(|v| v.as_str()) {
                    Some(value) => value,
                    None => {
                        return self
                            .send_error(
                                request_id,
                                -32602,
                                "name is required for mcp.tools.call".to_string(),
                                None,
                            )
                            .await;
                    }
                };
                let args = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));

                let tool_result = match tool_name {
                    "acp_debug_panel_get" => {
                        let limit = args
                            .get("limit")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(100)
                            .min(500) as usize;
                        let events = self.trace_snapshot(limit);
                        json!({
                            "ok": true,
                            "count": events.len(),
                            "events": events,
                            "trace_metrics": self.trace_metrics_snapshot(),
                        })
                    }
                    "acp_trace_get" => {
                        let limit = args
                            .get("limit")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(100)
                            .min(1000) as usize;
                        let events = self.trace_snapshot(limit);
                        json!({
                            "ok": true,
                            "count": events.len(),
                            "events": events,
                        })
                    }
                    "acp_runtime_health" => {
                        let report = self.runtime_healthcheck_report()?;
                        let artifact_path =
                            persist_runtime_healthcheck(&self.artifact_ledger(), &report)?;
                        let runtime_details = report
                            .components
                            .iter()
                            .find(|component| component.name == "runtime")
                            .map(|component| component.details.clone())
                            .unwrap_or(Value::Null);
                        let sqlite_cache_entries = report
                            .components
                            .iter()
                            .find(|component| component.name == "cache")
                            .and_then(|component| component.details.get("entries"))
                            .cloned()
                            .unwrap_or(Value::Null);
                        let vector = report
                            .components
                            .iter()
                            .find(|component| component.name == "vector")
                            .map(|component| component.details.clone())
                            .unwrap_or(Value::Null);
                        json!({
                            "ok": report.overall_status != CheckStatus::Error,
                            "report": report,
                            "artifact_path": artifact_path.display().to_string(),
                            "memory_cache_entries": self.memory_cache.active_entries(),
                            "sqlite_cache_entries": sqlite_cache_entries,
                            "lazy_load_cache": runtime_details.get("lazy_load_cache").cloned().unwrap_or(Value::Null),
                            "circuit_breaker": runtime_details.get("circuit_breaker").cloned().unwrap_or(Value::Null),
                            "rate_limiter": runtime_details.get("rate_limiter").cloned().unwrap_or(Value::Null),
                            "inflight": runtime_details.get("inflight").cloned().unwrap_or(Value::Null),
                            "vector": vector,
                            "lifecycle": runtime_details.get("lifecycle").cloned().unwrap_or(Value::Null),
                            "maintenance": runtime_details.get("maintenance").cloned().unwrap_or(Value::Null),
                            "review_gate": runtime_details.get("review_gate").cloned().unwrap_or(Value::Null),
                            "telemetry": runtime_details.get("telemetry").cloned().unwrap_or(Value::Null),
                        })
                    }
                    "acp_task_plan" => {
                        let task = match args.get("task").and_then(|v| v.as_str()) {
                            Some(value) if !value.trim().is_empty() => value,
                            _ => {
                                return self
                                    .send_error(
                                        request_id,
                                        -32602,
                                        "task is required for acp_task_plan".to_string(),
                                        None,
                                    )
                                    .await;
                            }
                        };
                        let plan = build_task_plan(task);
                        let artifact_path = persist_task_plan(&self.artifact_ledger(), &plan)?;
                        json!({
                            "ok": true,
                            "plan": plan,
                            "artifact_path": artifact_path.display().to_string(),
                        })
                    }
                    "acp_action_check" => {
                        let kind = args
                            .get("kind")
                            .and_then(|v| v.as_str())
                            .and_then(ActionCheckKind::parse)
                            .unwrap_or(ActionCheckKind::All);
                        let report = run_action_check(&self.artifact_ledger(), kind)?;
                        json!({
                            "ok": report.ok,
                            "report": report,
                        })
                    }
                    "acp_conversation_checkpoint_list" => {
                        let conversation_id = args
                            .get("conversation_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("default");
                        let branch_id = args.get("branch_id").and_then(|v| v.as_str());
                        let limit = args
                            .get("limit")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(50)
                            .min(500) as usize;
                        match self
                            .list_conversation_checkpoints(conversation_id, branch_id, limit)
                        {
                            Ok(checkpoints) => json!({
                                "ok": true,
                                "count": checkpoints.len(),
                                "checkpoints": checkpoints,
                            }),
                            Err(message) => json!({
                                "ok": false,
                                "error": message,
                            }),
                        }
                    }
                    _ => {
                        return self
                            .send_error(
                                request_id,
                                -32602,
                                crate::i18n::tf("error.unknown_mcp_adapter_tool", &[("tool_name", tool_name)]),
                                None,
                            )
                            .await;
                    }
                };

                self.send_result(
                    request_id,
                    json!({
                        "content": [{
                            "type": "text",
                            "text": tool_result.to_string(),
                        }],
                        "structuredContent": tool_result,
                    }),
                )
                .await
            }
            "chat" => {
                // Measure chat handling performance
                let (result, duration) = performance::utils::measure_time(|| {
                    self.handle_chat(
                        request_id,
                        request.params,
                        request_span.clone(),
                        Some(trace.clone()),
                    )
                });

                // Log performance metrics
                debug!("chat request handled in {:?}", duration);
                result.await
            }
            "metrics.get" => {
                // Measure metrics retrieval performance
                let (result, duration) = performance::utils::measure_time(|| {
                    serde_json::to_value(self.metrics.snapshot())
                });

                // Log performance metrics
                debug!("metrics.get request handled in {:?}", duration);

                self.send_result(request_id, result?).await
            }
            "metrics.prometheus" => {
                // Measure Prometheus metrics generation performance
                let (result, duration) = performance::utils::measure_time_async(|| async {
                    let sqlite_cache_entries = if let Some(cache) = self.cache_handle() {
                        self.cache_entry_count(cache.clone()).await.unwrap_or(0)
                    } else {
                        0
                    };
                    let (vector_memory_entries, vector_summary_entries) =
                        if let Some(store) = self.vector_store_handle() {
                            self.vector_entry_counts(store.clone())
                                .await
                                .unwrap_or((0, 0))
                        } else {
                            (0, 0)
                        };

                    let gauges = RuntimeGaugeSnapshot {
                        memory_cache_entries: self.memory_cache.active_entries() as u64,
                        sqlite_cache_entries,
                        vector_memory_entries,
                        vector_summary_entries,
                        circuit_open_agents: self.circuit_breakers.open_count() as u64,
                        circuit_half_open_agents: self.circuit_breakers.half_open_count() as u64,
                        circuit_tracked_agents: self.circuit_breakers.tracked_agents() as u64,
                        rate_limiter_tracked_phases: self.phase_rate_limiter.tracked_phases() as u64,
                    };
                    let breaker_snapshot = self.circuit_breakers.snapshot();
                    let phase_limiter_snapshot = self.phase_rate_limiter.snapshot();
                    let inflight_snapshot = self.inflight_limiter.snapshot();
                    let lifecycle = self.lifecycle.snapshot();
                    let maintenance = self.maintenance.snapshot();

                    json!({
                        "text": build_prometheus_metrics(
                            &self.metrics.snapshot(),
                            &gauges,
                            &breaker_snapshot,
                            &phase_limiter_snapshot,
                            &inflight_snapshot,
                            &lifecycle,
                            &maintenance,
                        )
                    })
                }).await;

                // Log performance metrics
                debug!("metrics.prometheus request handled in {:?}", duration);

                self.send_result(request_id, result).await
            }
            "metrics.reset" => {
                self.metrics.reset();
                self.send_result(request_id, json!({"ok": true})).await
            }
            "trace.metrics" => {
                let result = self.trace_metrics_snapshot();
                self.send_result(request_id, result).await
            }
            "trace.get" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let limit = params
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(100)
                    .min(1000) as usize;
                let events = self.trace_snapshot(limit);
                self.send_result(
                    request_id,
                    json!({
                        "ok": true,
                        "count": events.len(),
                        "events": events,
                    }),
                )
                .await
            }
            "debug.panel.get" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let limit = params
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(100)
                    .min(500) as usize;
                let recent_events = self.trace_snapshot(limit);

                let stage_transitions = recent_events
                    .iter()
                    .filter(|event| event.event_type.starts_with("phase."))
                    .map(|event| {
                        json!({
                            "timestamp": event.timestamp,
                            "event_type": event.event_type,
                            "phase": event.phase,
                            "status": event.status,
                            "duration_ms": event.duration_ms,
                            "task_id": event.task_id,
                            "pua_stage": event.pua_stage,
                        })
                    })
                    .collect::<Vec<_>>();

                let review_outcomes = recent_events
                    .iter()
                    .filter(|event| event.event_type == "phase.review_gate")
                    .map(|event| {
                        let attrs = event.inputs.get("attributes").cloned().unwrap_or_else(|| json!({}));
                        json!({
                            "timestamp": event.timestamp,
                            "status": event.status,
                            "phase": event.phase,
                            "attributes": attrs,
                            "error": event.error,
                        })
                    })
                    .collect::<Vec<_>>();

                let mut selected_agents: Vec<String> = Vec::new();
                let mut seen_agents: HashSet<String> = HashSet::new();
                for event in &recent_events {
                    if event.event_type != "phase.agent" {
                        continue;
                    }
                    let maybe_agent = event
                        .inputs
                        .get("attributes")
                        .and_then(|attrs| attrs.get("agent"))
                        .and_then(|v| v.as_str())
                        .map(|v| v.to_string());
                    if let Some(agent) = maybe_agent {
                        if seen_agents.insert(agent.clone()) {
                            selected_agents.push(agent);
                        }
                    }
                }

                let (conversation_count, checkpoint_count, branch_head_count) = self
                    .conversation_store
                    .lock()
                    .map(|store| {
                        let conversation_count = store.len();
                        let checkpoint_count = store
                            .values()
                            .map(|state| state.checkpoints.len())
                            .sum::<usize>();
                        let branch_head_count = store
                            .values()
                            .map(|state| state.branch_heads.len())
                            .sum::<usize>();
                        (conversation_count, checkpoint_count, branch_head_count)
                    })
                    .unwrap_or((0, 0, 0));

                let ledger = self.artifact_ledger();
                let artifacts = json!({
                    "root": ledger.root().display().to_string(),
                    "spec_plan": ledger.latest_path("spec", "latest-plan.json").exists(),
                    "healthcheck": ledger.latest_path("qa", "latest-healthcheck.json").exists(),
                    "retest": ledger.latest_path("retest", "latest-action-check.json").exists(),
                    "final_summary": ledger.latest_path("final", "latest-summary.json").exists(),
                });

                self.send_result(
                    request_id,
                    json!({
                        "ok": true,
                        "panel": {
                            "trace": {
                                "count": recent_events.len(),
                                "stage_transitions": stage_transitions,
                            },
                            "selected_agents": selected_agents,
                            "review_outcomes": review_outcomes,
                            "runtime_health": {
                                "memory_cache_entries": self.memory_cache.active_entries(),
                                "lazy_load_cache": lazy_load_cache_snapshot(),
                                "circuit_breaker": {
                                    "open_agents": self.circuit_breakers.open_count(),
                                    "half_open_agents": self.circuit_breakers.half_open_count(),
                                    "tracked_agents": self.circuit_breakers.tracked_agents(),
                                },
                                "lifecycle": self.lifecycle.snapshot(),
                            },
                            "conversations": {
                                "count": conversation_count,
                                "checkpoints": checkpoint_count,
                                "branch_heads": branch_head_count,
                            },
                            "artifacts": artifacts,
                            "review_gate": {
                                "total": self.metrics.snapshot().review_gate_total,
                                "approved": self.metrics.snapshot().review_gate_approved_total,
                                "rejected": self.metrics.snapshot().review_gate_rejected_total,
                                "timeout": self.metrics.snapshot().review_gate_timeout_total,
                                "degraded": self.metrics.snapshot().review_gate_degraded_total,
                                "invalid_response": self.metrics.snapshot().review_gate_invalid_response_total,
                            },
                        }
                    }),
                )
                .await
            }
            "workflow.clarify" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let task = match params.get("task").and_then(|v| v.as_str()) {
                    Some(value) if !value.trim().is_empty() => value.to_string(),
                    _ => {
                        return self
                            .send_error(
                                request_id,
                                -32602,
                                "task is required for workflow.clarify".to_string(),
                                None,
                            )
                            .await;
                    }
                };

                let ledger = self.artifact_ledger();
                let base_contract = parse_requirement_contract_from_params(&params, &task)
                    .unwrap_or_else(|| default_requirement_contract(&task, "workflow.clarify"));
                let mut contract = base_contract.clone();
                let missing_fields = requirement_missing_fields(&contract);
                contract.open_questions = requirement_questions_from_missing(&missing_fields);
                contract.ambiguity_score = estimate_requirement_ambiguity(&task, &contract);
                contract.user_confirmed = false;
                let blue5_doc = load_blue5_doc_lazy(self.config_path.as_ref());
                let blue5_auto = evaluate_blue5_for_clarify(
                    &blue5_doc,
                    &contract,
                    &missing_fields,
                    &params,
                );

                let previous_session = fs::read_to_string(
                    ledger.latest_path("spec", "latest-clarification-session.json"),
                )
                .ok()
                .and_then(|raw| serde_json::from_str::<ClarificationSessionArtifact>(&raw).ok())
                .filter(|session| session.task.trim() == task.trim());

                let round_index = params
                    .get("round_index")
                    .and_then(|v| v.as_u64())
                    .map(|v| v.max(1) as u32)
                    .unwrap_or_else(|| {
                        previous_session
                            .as_ref()
                            .map(|session| session.round_index.saturating_add(1))
                            .unwrap_or(1)
                    });
                let session_id = params
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty())
                    .or_else(|| previous_session.as_ref().map(|session| session.session_id.clone()))
                    .unwrap_or_else(|| format!("clarify-{}", now_ts()));
                let user_feedback = params
                    .get("user_feedback")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let resolved_points = parse_string_list(params.get("resolved_points"));
                let ready_to_confirm = params
                    .get("ready_to_confirm")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(missing_fields.is_empty());
                let collaboration_mode = params
                    .get("clarify_collaboration_mode")
                    .and_then(|v| v.as_str())
                    .map(|v| v.trim().to_ascii_lowercase())
                    .filter(|v| v == "single_ai" || v == "multi_ai")
                    .unwrap_or_else(|| {
                        if blue5_auto.should_multi_ai_clarify {
                            "multi_ai".to_string()
                        } else {
                            "single_ai".to_string()
                        }
                    });

                let mut lead_clarifier = "none".to_string();
                let mut assistant_clarifiers: Vec<String> = Vec::new();
                if let Ok((flow, registry)) = self.routing_handles() {
                    let phase_hint = params
                        .get("phase")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let routing = flow
                        .resolve(phase_hint, registry.as_ref())
                        .unwrap_or_else(|_| {
                            flow.resolve(None, registry.as_ref())
                                .expect("default phase must always resolve")
                        });
                    let env_ready_agents =
                        filter_env_ready_agents(self.config_path.as_ref(), &routing.phase.agent_names);
                    if let Some(first) = env_ready_agents.first() {
                        lead_clarifier = first.clone();
                        if collaboration_mode == "multi_ai" {
                            assistant_clarifiers = env_ready_agents.iter().skip(1).take(2).cloned().collect();
                        }
                    }
                }

                let clarification_session = ClarificationSessionArtifact {
                    generated_at: now_ts(),
                    task: task.clone(),
                    source: "workflow.clarify".to_string(),
                    session_id,
                    round_index,
                    lead_clarifier,
                    assistant_clarifiers,
                    user_feedback,
                    resolved_points,
                    open_points: missing_fields.clone(),
                    next_questions: contract.open_questions.clone(),
                    ready_to_confirm,
                };

                let clarification_path = persist_requirement_contract(&ledger, &contract)?;
                let clarification_session_path =
                    persist_clarification_session_artifact(&ledger, &clarification_session)?;
                let governance = GovernancePolicyArtifact {
                    generated_at: now_ts(),
                    task: task.clone(),
                    source: "workflow.clarify".to_string(),
                    clarification_required: true,
                    confirmed: false,
                    blocked: true,
                    reason: Some("requirement clarification required before planning/execution".to_string()),
                    next_step: json!({
                        "method": "workflow.confirm",
                        "task": task,
                        "ready_to_confirm": clarification_session.ready_to_confirm,
                        "round_index": clarification_session.round_index,
                        "requirement_contract": {
                            "goal": contract.goal,
                            "scope": contract.scope,
                            "non_goals": contract.non_goals,
                            "acceptance_criteria": contract.acceptance_criteria,
                            "constraints": contract.constraints,
                            "user_confirmed": true
                        }
                    }),
                };
                let governance_path = persist_governance_policy(&ledger, &governance)?;

                self.send_result(
                    request_id,
                    json!({
                        "ok": true,
                        "clarification_required": true,
                        "missing_fields": missing_fields,
                        "open_questions": contract.open_questions,
                        "requirement_contract": contract,
                        "blue5": {
                            "doc": blue5_doc,
                            "auto": blue5_auto,
                        },
                        "clarification_session": clarification_session,
                        "clarify_collaboration_mode": collaboration_mode,
                        "clarification_artifact_path": clarification_path.display().to_string(),
                        "clarification_session_artifact_path": clarification_session_path
                            .display()
                            .to_string(),
                        "governance_artifact_path": governance_path.display().to_string(),
                    }),
                )
                .await
            }
            "workflow.confirm" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let task = match params.get("task").and_then(|v| v.as_str()) {
                    Some(value) if !value.trim().is_empty() => value.to_string(),
                    _ => {
                        return self
                            .send_error(
                                request_id,
                                -32602,
                                "task is required for workflow.confirm".to_string(),
                                None,
                            )
                            .await;
                    }
                };

                let ledger = self.artifact_ledger();
                let mut contract = parse_requirement_contract_from_params(&params, &task)
                    .or_else(|| load_latest_requirement_contract(&ledger, &task))
                    .unwrap_or_else(|| default_requirement_contract(&task, "workflow.confirm"));
                contract.generated_at = now_ts();
                contract.source = "workflow.confirm".to_string();
                contract.user_confirmed = params
                    .get("user_confirmed")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                contract.ambiguity_score = estimate_requirement_ambiguity(&task, &contract);

                let missing_fields = requirement_missing_fields(&contract);
                if !missing_fields.is_empty() {
                    return self
                        .send_error(
                            request_id,
                            -32602,
                            "workflow.confirm requires complete requirement_contract (goal/scope/acceptance_criteria/constraints)"
                                .to_string(),
                            Some(json!({
                                "missing_fields": missing_fields,
                                "next_step": "fill requirement_contract and retry workflow.confirm"
                            })),
                        )
                        .await;
                }

                let latest_session = fs::read_to_string(
                    ledger.latest_path("spec", "latest-clarification-session.json"),
                )
                .ok()
                .and_then(|raw| serde_json::from_str::<ClarificationSessionArtifact>(&raw).ok())
                .filter(|session| session.task.trim() == task.trim());

                let ready_to_confirm = params
                    .get("ready_to_confirm")
                    .and_then(|v| v.as_bool())
                    .or_else(|| latest_session.as_ref().map(|session| session.ready_to_confirm))
                    .unwrap_or(false);
                if !ready_to_confirm {
                    return self
                        .send_error(
                            request_id,
                            -32006,
                            "workflow.confirm blocked: clarification session is not ready_to_confirm"
                                .to_string(),
                            Some(json!({
                                "kind": "clarification_session",
                                "task": task,
                                "next_step": {
                                    "method": "workflow.clarify",
                                    "task": task,
                                    "round_index": latest_session
                                        .as_ref()
                                        .map(|s| s.round_index.saturating_add(1))
                                        .unwrap_or(1)
                                }
                            })),
                        )
                        .await;
                }

                let clarification_path = persist_requirement_contract(&ledger, &contract)?;
                let confirm_session = ClarificationSessionArtifact {
                    generated_at: now_ts(),
                    task: task.clone(),
                    source: "workflow.confirm".to_string(),
                    session_id: latest_session
                        .as_ref()
                        .map(|s| s.session_id.clone())
                        .unwrap_or_else(|| format!("clarify-{}", now_ts())),
                    round_index: latest_session
                        .as_ref()
                        .map(|s| s.round_index)
                        .unwrap_or(1),
                    lead_clarifier: latest_session
                        .as_ref()
                        .map(|s| s.lead_clarifier.clone())
                        .unwrap_or_else(|| "none".to_string()),
                    assistant_clarifiers: latest_session
                        .as_ref()
                        .map(|s| s.assistant_clarifiers.clone())
                        .unwrap_or_default(),
                    user_feedback: params
                        .get("user_feedback")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    resolved_points: vec![
                        "goal".to_string(),
                        "scope".to_string(),
                        "acceptance_criteria".to_string(),
                        "constraints".to_string(),
                    ],
                    open_points: Vec::new(),
                    next_questions: Vec::new(),
                    ready_to_confirm,
                };
                let clarification_session_path =
                    persist_clarification_session_artifact(&ledger, &confirm_session)?;
                let governance = GovernancePolicyArtifact {
                    generated_at: now_ts(),
                    task: task.clone(),
                    source: "workflow.confirm".to_string(),
                    clarification_required: true,
                    confirmed: contract.user_confirmed,
                    blocked: !contract.user_confirmed,
                    reason: if contract.user_confirmed {
                        None
                    } else {
                        Some("user_confirmed=false".to_string())
                    },
                    next_step: json!({
                        "confirmed": contract.user_confirmed,
                        "next_method": if contract.user_confirmed { "task.plan" } else { "workflow.confirm" }
                    }),
                };
                let governance_path = persist_governance_policy(&ledger, &governance)?;

                self.send_result(
                    request_id,
                    json!({
                        "ok": contract.user_confirmed,
                        "confirmed": contract.user_confirmed,
                        "requirement_contract": contract,
                        "clarification_session": confirm_session,
                        "clarification_artifact_path": clarification_path.display().to_string(),
                        "clarification_session_artifact_path": clarification_session_path
                            .display()
                            .to_string(),
                        "governance_artifact_path": governance_path.display().to_string(),
                    }),
                )
                .await
            }
            "task.plan" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let task = match params.get("task").and_then(|v| v.as_str()) {
                    Some(value) if !value.trim().is_empty() => value,
                    _ => {
                        return self
                            .send_error(
                                request_id,
                                -32602,
                                "task is required for task.plan".to_string(),
                                None,
                            )
                            .await;
                    }
                };

                let ledger = self.artifact_ledger();
                let requirement_gate = evaluate_requirement_gate(&ledger, task, &params, "task.plan")?;
                if requirement_gate.blocked {
                    return self
                        .send_error(
                            request_id,
                            -32006,
                            requirement_gate
                                .reason
                                .clone()
                                .unwrap_or_else(|| "requirement confirmation is required".to_string()),
                            Some(json!({
                                "kind": "requirement_contract",
                                "task": task,
                                "missing_fields": requirement_gate.missing_fields,
                                "next_step": {
                                    "method": "workflow.clarify",
                                    "task": task
                                },
                                "governance_artifact_path": requirement_gate
                                    .governance_artifact_path
                                    .display()
                                    .to_string(),
                            })),
                        )
                        .await;
                }

                let plan = build_task_plan(task);
                let artifact_path = persist_task_plan(&ledger, &plan)?;
                self.record_trace_event(
                    &trace,
                    "phase.plan",
                    "ok",
                    "plan",
                    json!({
                        "task": task,
                        "sub_agent_recommended": plan.sub_agent_recommended,
                        "planned_subtasks": plan.planned_subtasks.len(),
                    }),
                    None,
                    started.elapsed().as_millis() as u64,
                );
                self.send_result(
                    request_id,
                    json!({
                        "ok": true,
                        "plan": plan,
                        "artifact_path": artifact_path.display().to_string(),
                        "requirement_gate": {
                            "confirmed": true,
                            "governance_artifact_path": requirement_gate.governance_artifact_path.display().to_string(),
                            "clarification_artifact_path": requirement_gate
                                .clarification_artifact_path
                                .as_ref()
                                .map(|p| p.display().to_string()),
                        }
                    }),
                )
                .await
            }
            "workflow.generate" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let task = match params.get("task").and_then(|v| v.as_str()) {
                    Some(value) if !value.trim().is_empty() => value,
                    _ => {
                        return self
                            .send_error(
                                request_id,
                                -32602,
                                "task is required for workflow.generate".to_string(),
                                None,
                            )
                            .await;
                    }
                };

                let ledger = self.artifact_ledger();
                let requirement_gate =
                    evaluate_requirement_gate(&ledger, task, &params, "workflow.generate")?;
                if requirement_gate.blocked {
                    return self
                        .send_error(
                            request_id,
                            -32006,
                            requirement_gate
                                .reason
                                .clone()
                                .unwrap_or_else(|| "requirement confirmation is required".to_string()),
                            Some(json!({
                                "kind": "requirement_contract",
                                "task": task,
                                "missing_fields": requirement_gate.missing_fields,
                                "next_step": {
                                    "method": "workflow.clarify",
                                    "task": task
                                },
                                "governance_artifact_path": requirement_gate
                                    .governance_artifact_path
                                    .display()
                                    .to_string(),
                            })),
                        )
                        .await;
                }

                let plan = build_task_plan(task);
                let plan_artifact_path = persist_task_plan(&ledger, &plan)?;
                let workflow = build_workflow_generated_artifact(&plan);
                let workflow_artifact_path = persist_workflow_generated(&ledger, &workflow)?;

                self.record_trace_event(
                    &trace,
                    "phase.plan",
                    "ok",
                    "workflow",
                    json!({
                        "task": task,
                        "nodes": workflow.nodes.len(),
                        "edges": workflow.edges.len(),
                        "execution_phases": workflow.execution_order.len(),
                    }),
                    None,
                    started.elapsed().as_millis() as u64,
                );
                self.send_result(
                    request_id,
                    json!({
                        "ok": true,
                        "plan": plan,
                        "workflow": workflow,
                        "plan_artifact_path": plan_artifact_path.display().to_string(),
                        "workflow_artifact_path": workflow_artifact_path.display().to_string(),
                        "requirement_gate": {
                            "confirmed": true,
                            "governance_artifact_path": requirement_gate.governance_artifact_path.display().to_string(),
                            "clarification_artifact_path": requirement_gate
                                .clarification_artifact_path
                                .as_ref()
                                .map(|p| p.display().to_string()),
                        }
                    }),
                )
                .await
            }
            "workflow.research" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let task = match params.get("task").and_then(|v| v.as_str()) {
                    Some(value) if !value.trim().is_empty() => value.to_string(),
                    _ => {
                        return self
                            .send_error(
                                request_id,
                                -32602,
                                "task is required for workflow.research".to_string(),
                                None,
                            )
                            .await;
                    }
                };

                let phase_hint = params
                    .get("phase")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                let (flow, registry) = self.routing_handles()?;
                let routing = flow
                    .resolve(phase_hint, registry.as_ref())
                    .unwrap_or_else(|_| {
                        flow.resolve(None, registry.as_ref())
                            .expect("default phase must always resolve")
                    });
                let env_ready_phase_agents =
                    filter_env_ready_agents(self.config_path.as_ref(), &routing.phase.agent_names);
                if env_ready_phase_agents.is_empty() {
                    return self
                        .send_error(
                            request_id,
                            -32005,
                            "workflow.research has no env-ready agents; configure at least one key or switch phase"
                                .to_string(),
                            Some(json!({
                                "kind": "capability_ceiling",
                                "phase": routing.phase.phase_name,
                                "configured_agents": routing.phase.agent_names,
                                "next_step": {
                                    "configure_agent_key": true,
                                    "or_switch_phase": true
                                }
                            })),
                        )
                        .await;
                }

                let planner_agent_name = match env_ready_phase_agents.first().cloned() {
                    Some(name) => name,
                    None => {
                        return self
                            .send_error(
                                request_id,
                                -32603,
                                "workflow.research requires at least one routable agent".to_string(),
                                None,
                            )
                            .await;
                    }
                };
                let researcher_agent_name = env_ready_phase_agents
                    .get(1)
                    .cloned()
                    .unwrap_or_else(|| planner_agent_name.clone());
                let reviewer_agent_name = env_ready_phase_agents
                    .get(2)
                    .cloned()
                    .unwrap_or_else(|| researcher_agent_name.clone());

                let planner_agent = match registry.get(&planner_agent_name) {
                    Some(a) => a,
                    None => {
                        return self
                            .send_error(
                                request_id,
                                -32603,
                                format!(
                                    "workflow.research planner agent '{}' not found",
                                    planner_agent_name
                                ),
                                None,
                            )
                            .await;
                    }
                };
                let researcher_agent = match registry.get(&researcher_agent_name) {
                    Some(a) => a,
                    None => {
                        return self
                            .send_error(
                                request_id,
                                -32603,
                                format!(
                                    "workflow.research researcher agent '{}' not found",
                                    researcher_agent_name
                                ),
                                None,
                            )
                            .await;
                    }
                };
                let reviewer_agent = match registry.get(&reviewer_agent_name) {
                    Some(a) => a,
                    None => {
                        return self
                            .send_error(
                                request_id,
                                -32603,
                                format!(
                                    "workflow.research reviewer agent '{}' not found",
                                    reviewer_agent_name
                                ),
                                None,
                            )
                            .await;
                    }
                };

                let planner_prompt = format!(
                    "Task: {}\n\nAs Planner: produce a concise problem tree and acceptance criteria.",
                    task
                );
                let researcher_prompt = format!(
                    "Task: {}\n\nAs Researcher: propose 3 candidate solutions with risk matrix and tradeoffs.",
                    task
                );
                let reviewer_prompt = format!(
                    "Task: {}\n\nAs Reviewer: select one recommended plan from candidates with rationale and risks.",
                    task
                );

                let planner_output = self
                    .run_agent_collecting(
                        planner_agent_name.clone(),
                        planner_agent,
                        vec![Message {
                            role: "user".to_string(),
                            content: planner_prompt,
                        }],
                        None,
                        None,
                        Some(Duration::from_secs(120)),
                    )
                    .await?;
                let researcher_output = self
                    .run_agent_collecting(
                        researcher_agent_name.clone(),
                        researcher_agent,
                        vec![Message {
                            role: "user".to_string(),
                            content: researcher_prompt,
                        }],
                        None,
                        None,
                        Some(Duration::from_secs(120)),
                    )
                    .await?;
                let reviewer_output = self
                    .run_agent_collecting(
                        reviewer_agent_name.clone(),
                        reviewer_agent,
                        vec![Message {
                            role: "user".to_string(),
                            content: reviewer_prompt,
                        }],
                        None,
                        None,
                        Some(Duration::from_secs(120)),
                    )
                    .await?;

                let artifact = WorkflowResearchArtifact {
                    generated_at: now_ts(),
                    task: task.clone(),
                    planner_output,
                    researcher_output,
                    recommended_plan: reviewer_output.chars().take(500).collect(),
                    reviewer_output,
                };
                let artifact_path = persist_workflow_research(&self.artifact_ledger(), &artifact)?;

                self.record_trace_event(
                    &trace,
                    "phase.research",
                    "ok",
                    "research",
                    json!({
                        "task": task,
                        "planner_agent": planner_agent_name,
                        "researcher_agent": researcher_agent_name,
                        "reviewer_agent": reviewer_agent_name,
                    }),
                    None,
                    started.elapsed().as_millis() as u64,
                );

                self.send_result(
                    request_id,
                    json!({
                        "ok": true,
                        "artifact": artifact,
                        "artifact_path": artifact_path.display().to_string(),
                    }),
                )
                .await
            }
            "workflow.consult" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let task = match params.get("task").and_then(|v| v.as_str()) {
                    Some(value) if !value.trim().is_empty() => value.to_string(),
                    _ => {
                        return self
                            .send_error(
                                request_id,
                                -32602,
                                "task is required for workflow.consult".to_string(),
                                None,
                            )
                            .await;
                    }
                };

                let phase_hint = params
                    .get("phase")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                let (flow, registry) = self.routing_handles()?;
                let routing = flow
                    .resolve(phase_hint, registry.as_ref())
                    .unwrap_or_else(|_| {
                        flow.resolve(None, registry.as_ref())
                            .expect("default phase must always resolve")
                    });
                let env_ready_phase_agents =
                    filter_env_ready_agents(self.config_path.as_ref(), &routing.phase.agent_names);
                if env_ready_phase_agents.is_empty() {
                    return self
                        .send_error(
                            request_id,
                            -32005,
                            "workflow.consult has no env-ready agents; configure at least one key or switch phase"
                                .to_string(),
                            None,
                        )
                        .await;
                }

                let policy = resolve_primary_secondary_policy(
                    &env_ready_phase_agents,
                    &params,
                    routing.phase.options.as_ref(),
                )?;
                let trigger_reason = params
                    .get("trigger_reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("explicit workflow.consult request")
                    .to_string();
                let threshold = params
                    .get("consultation_confidence_threshold")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.65)
                    .clamp(0.0, 1.0);

                let (artifact, consensus_achieved) = run_consultation_workflow(
                    self,
                    registry.as_ref(),
                    &task,
                    "workflow.consult",
                    &trigger_reason,
                    &policy,
                    threshold,
                )
                .await?;
                let artifact_path =
                    persist_consultation_artifact(&self.artifact_ledger(), &artifact)?;

                self.send_result(
                    request_id,
                    json!({
                        "ok": true,
                        "consensus_achieved": consensus_achieved,
                        "artifact": artifact,
                        "artifact_path": artifact_path.display().to_string(),
                        "primary_secondary_policy": policy,
                    }),
                )
                .await
            }
            // Section 6 (sub-agent orchestration) + Section 5 (lifecycle tracking)
            method @ ("task.execute" | "workflow.execute") => {
                let is_workflow_execute = method == "workflow.execute";
                let params = request.params.unwrap_or_else(|| json!({}));
                let task_str = match params.get("task").and_then(|v| v.as_str()) {
                    Some(t) if !t.trim().is_empty() => t.to_string(),
                    _ => {
                        return self
                            .send_error(
                                request_id,
                                -32602,
                                "task is required for task.execute".to_string(),
                                None,
                            )
                            .await;
                    }
                };

                let phase_hint = params
                    .get("phase")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                let mut plan = build_task_plan(&task_str);
                let ledger = self.artifact_ledger();
                let requirement_gate =
                    evaluate_requirement_gate(&ledger, &task_str, &params, method)?;
                if requirement_gate.blocked {
                    return self
                        .send_error(
                            request_id,
                            -32006,
                            requirement_gate
                                .reason
                                .clone()
                                .unwrap_or_else(|| "requirement confirmation is required".to_string()),
                            Some(json!({
                                "kind": "requirement_contract",
                                "task": task_str,
                                "missing_fields": requirement_gate.missing_fields,
                                "next_step": {
                                    "method": "workflow.clarify",
                                    "task": task_str
                                },
                                "governance_artifact_path": requirement_gate
                                    .governance_artifact_path
                                    .display()
                                    .to_string(),
                            })),
                        )
                        .await;
                }

                let (flow, registry) = self.routing_handles()?;
                let routing = flow
                    .resolve(phase_hint, registry.as_ref())
                    .unwrap_or_else(|_| {
                        flow.resolve(None, registry.as_ref())
                            .expect("default phase must always resolve")
                    });
                let adaptive_routing = params
                    .get("adaptive_routing")
                    .and_then(|v| v.as_bool())
                    .or_else(|| extra_bool(routing.phase.options.as_ref(), "adaptive_routing"))
                    .unwrap_or(true);
                let predicted_success_rate_base = plan.routing.predicted_success_rate;
                if adaptive_routing {
                    plan.routing.predicted_success_rate = recommend_predicted_success_rate_from_learning(
                        &ledger,
                        plan.routing.predicted_success_rate,
                        plan.characteristics.complexity,
                    );
                }
                let predicted_success_rate_tuned =
                    (plan.routing.predicted_success_rate - predicted_success_rate_base).abs()
                        > f32::EPSILON;
                let env_ready_phase_agents =
                    filter_env_ready_agents(self.config_path.as_ref(), &routing.phase.agent_names);
                if env_ready_phase_agents.is_empty() {
                    return self
                        .send_error(
                            request_id,
                            -32005,
                            "no env-ready agents are available for this phase; provide at least one agent key or switch phase"
                                .to_string(),
                            Some(json!({
                                "kind": "capability_ceiling",
                                "phase": routing.phase.phase_name,
                                "configured_agents": routing.phase.agent_names,
                                "suggestions": [
                                    "configure at least one agent credential",
                                    "switch to a phase with an env-ready agent"
                                ]
                            })),
                        )
                        .await;
                }
                let adaptive_agent_order = params
                    .get("adaptive_agent_order")
                    .and_then(|v| v.as_bool())
                    .or_else(|| {
                        extra_bool(routing.phase.options.as_ref(), "adaptive_agent_order")
                    })
                    .unwrap_or(is_workflow_execute);
                let phase_agent_names = if adaptive_agent_order {
                    recommend_agent_order_from_execution_history(
                        &ledger,
                        &env_ready_phase_agents,
                        40,
                    )
                } else {
                    env_ready_phase_agents.clone()
                };
                let agent_order_tuned = phase_agent_names != env_ready_phase_agents;
                let primary_secondary_policy = match resolve_primary_secondary_policy(
                    &phase_agent_names,
                    &params,
                    routing.phase.options.as_ref(),
                ) {
                    Ok(policy) => policy,
                    Err(err) => {
                        return self
                            .send_error(
                                request_id,
                                -32602,
                                err.to_string(),
                                Some(json!({
                                    "kind": "primary_secondary_policy_invalid",
                                    "env_ready_phase_agents": phase_agent_names,
                                    "supported_failover_policy": [
                                        "first_secondary",
                                        "score_based_secondary",
                                        "abort"
                                    ],
                                })),
                            )
                            .await;
                    }
                };
                let blue5_doc = load_blue5_doc_lazy(self.config_path.as_ref());
                let mut blue5_auto =
                    evaluate_blue5_for_execute(&blue5_doc, &plan, &phase_agent_names, &params);
                blue5_auto.primary_agent = Some(primary_secondary_policy.primary_agent.clone());
                blue5_auto.secondary_agents = primary_secondary_policy.secondary_agents.clone();

                let mut consultation_artifact_path: Option<PathBuf> = None;
                let mut consultation_summary: Option<String> = None;
                let consultation_confidence_threshold = params
                    .get("consultation_confidence_threshold")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.65)
                    .clamp(0.0, 1.0);
                if blue5_auto.should_consultation {
                    let trigger_reason = if blue5_auto.reasons.is_empty() {
                        "blue5 auto consultation gate triggered".to_string()
                    } else {
                        blue5_auto.reasons.join("; ")
                    };
                    let (consultation_artifact, consensus_achieved) = run_consultation_workflow(
                        self,
                        registry.as_ref(),
                        &task_str,
                        method,
                        &trigger_reason,
                        &primary_secondary_policy,
                        consultation_confidence_threshold,
                    )
                    .await?;
                    let artifact_path = persist_consultation_artifact(&ledger, &consultation_artifact)?;
                    consultation_summary = Some(
                        consultation_artifact
                            .consensus_plan
                            .chars()
                            .take(240)
                            .collect::<String>(),
                    );
                    consultation_artifact_path = Some(artifact_path.clone());

                    if !consensus_achieved {
                        return self
                            .send_error(
                                request_id,
                                -32007,
                                "consultation did not reach executable consensus; clarify requirements before execution"
                                    .to_string(),
                                Some(json!({
                                    "kind": "consultation_blocked",
                                    "task": task_str,
                                    "trigger_reason": trigger_reason,
                                    "consultation_confidence_threshold": consultation_confidence_threshold,
                                    "consultation_artifact_path": artifact_path.display().to_string(),
                                    "next_step": {
                                        "method": "workflow.clarify",
                                        "task": task_str
                                    }
                                })),
                            )
                            .await;
                    }

                    let consensus_prefix = consultation_artifact
                        .consensus_plan
                        .chars()
                        .take(360)
                        .collect::<String>();
                    for subtask in plan.planned_subtasks.iter_mut() {
                        subtask.description = format!(
                            "Consultation consensus:\n{}\n\nSubtask:\n{}",
                            consensus_prefix, subtask.description
                        );
                    }
                }

                // M8: persist primary-secondary policy artifact immediately after resolution
                let _ps_policy_artifact_path = persist_primary_secondary_policy_artifact(
                    &ledger,
                    &PrimarySecondaryPolicyArtifact {
                        generated_at: now_ts(),
                        task: task_str.clone(),
                        source: method.to_string(),
                        primary_agent: primary_secondary_policy.primary_agent.clone(),
                        secondary_agents: primary_secondary_policy.secondary_agents.clone(),
                        policy_version: primary_secondary_policy.policy_version.clone(),
                        failover_policy: primary_secondary_policy.failover_policy.clone(),
                        secondary_max_count: primary_secondary_policy.secondary_max_count,
                    },
                )?;

                let capability_ready_agents = phase_agent_names.len();
                let capability_max = capability_max_complexity(capability_ready_agents);
                let capability_exceeded = plan.characteristics.complexity > capability_max;
                let enforce_capability_ceiling = params
                    .get("enforce_capability_ceiling")
                    .and_then(|v| v.as_bool())
                    .or_else(|| {
                        extra_bool(
                            routing.phase.options.as_ref(),
                            "enforce_capability_ceiling",
                        )
                    })
                    .unwrap_or(true);
                let capability_decision = params
                    .get("capability_decision")
                    .and_then(|v| v.as_str())
                    .map(|value| value.to_ascii_lowercase());
                let capability_confirm = params
                    .get("capability_confirm")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let mut capability_forced_degrade = false;
                let capability_decision_effective = if capability_exceeded && enforce_capability_ceiling {
                    match (capability_decision.as_deref(), capability_confirm) {
                        (Some("degrade"), true) => {
                            capability_forced_degrade = true;
                            "degrade"
                        }
                        (Some("multi_ai"), true) => {
                            if capability_ready_agents < 2 {
                                return self
                                    .send_error(
                                        request_id,
                                        -32005,
                                        "capability ceiling exceeded and multi_ai requires at least two env-ready agents"
                                            .to_string(),
                                        Some(json!({
                                            "kind": "capability_ceiling",
                                            "task_complexity": plan.characteristics.complexity,
                                            "capability_max_complexity": capability_max,
                                            "ready_agents": capability_ready_agents,
                                            "decision": "multi_ai",
                                            "suggestions": [
                                                "configure one more env-ready agent",
                                                "or choose capability_decision=degrade with capability_confirm=true"
                                            ]
                                        })),
                                    )
                                    .await;
                            }
                            "multi_ai"
                        }
                        _ => {
                            return self
                                .send_error(
                                    request_id,
                                    -32005,
                                    "task complexity exceeds current capability ceiling; choose capability_decision=multi_ai or capability_decision=degrade and set capability_confirm=true"
                                        .to_string(),
                                    Some(json!({
                                        "kind": "capability_ceiling",
                                        "task_complexity": plan.characteristics.complexity,
                                        "capability_max_complexity": capability_max,
                                        "ready_agents": capability_ready_agents,
                                        "configured_phase_agents": routing.phase.agent_names,
                                        "env_ready_phase_agents": env_ready_phase_agents,
                                        "next_step": {
                                            "degrade": {
                                                "capability_decision": "degrade",
                                                "capability_confirm": true
                                            },
                                            "multi_ai": {
                                                "capability_decision": "multi_ai",
                                                "capability_confirm": true
                                            }
                                        }
                                    })),
                                )
                                .await;
                        }
                    }
                } else if capability_exceeded {
                    warn!(
                        task = %task_str,
                        complexity = plan.characteristics.complexity,
                        capability_max = capability_max,
                        ready_agents = capability_ready_agents,
                        "capability ceiling exceeded but enforcement is disabled; continuing in warn-only mode"
                    );
                    "warn_only"
                } else {
                    "not_required"
                };
                let capability_governance = json!({
                    "ready_agents": capability_ready_agents,
                    "capability_max_complexity": capability_max,
                    "task_complexity": plan.characteristics.complexity,
                    "exceeded": capability_exceeded,
                    "enforced": enforce_capability_ceiling,
                    "decision": capability_decision_effective,
                    "forced_degrade": capability_forced_degrade,
                });
                let primary_agent_name = Some(primary_secondary_policy.primary_agent.clone());
                let executor_label = if phase_agent_names.len() > 1 {
                    "multi-agent-auto-assigned".to_string()
                } else {
                    primary_agent_name
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string())
                };
                let mut workflow_artifact_path: Option<PathBuf> = None;
                let mut workflow_meta: Option<Value> = None;

                let auto_research = params
                    .get("auto_research")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(is_workflow_execute);
                let mut research_artifact_path: Option<PathBuf> = None;
                let mut research_summary: Option<String> = None;
                if auto_research {
                    let planner_agent_name = match phase_agent_names.first().cloned() {
                        Some(name) => name,
                        None => {
                            return self
                                .send_error(
                                    request_id,
                                    -32603,
                                    "workflow.execute auto_research requires at least one routable agent"
                                        .to_string(),
                                    None,
                                )
                                .await;
                        }
                    };
                    let researcher_agent_name = phase_agent_names
                        .get(1)
                        .cloned()
                        .unwrap_or_else(|| planner_agent_name.clone());
                    let reviewer_agent_name = phase_agent_names
                        .get(2)
                        .cloned()
                        .unwrap_or_else(|| researcher_agent_name.clone());

                    let planner_agent = match registry.get(&planner_agent_name) {
                        Some(agent) => agent,
                        None => {
                            return self
                                .send_error(
                                    request_id,
                                    -32603,
                                    format!(
                                        "workflow.execute auto_research planner agent '{}' not found",
                                        planner_agent_name
                                    ),
                                    None,
                                )
                                .await;
                        }
                    };
                    let researcher_agent = match registry.get(&researcher_agent_name) {
                        Some(agent) => agent,
                        None => {
                            return self
                                .send_error(
                                    request_id,
                                    -32603,
                                    format!(
                                        "workflow.execute auto_research researcher agent '{}' not found",
                                        researcher_agent_name
                                    ),
                                    None,
                                )
                                .await;
                        }
                    };
                    let reviewer_agent = match registry.get(&reviewer_agent_name) {
                        Some(agent) => agent,
                        None => {
                            return self
                                .send_error(
                                    request_id,
                                    -32603,
                                    format!(
                                        "workflow.execute auto_research reviewer agent '{}' not found",
                                        reviewer_agent_name
                                    ),
                                    None,
                                )
                                .await;
                        }
                    };

                    let planner_prompt = format!(
                        "Task: {}\n\nAs Planner: produce a concise problem tree and acceptance criteria.",
                        task_str
                    );
                    let researcher_prompt = format!(
                        "Task: {}\n\nAs Researcher: propose 3 candidate solutions with risk matrix and tradeoffs.",
                        task_str
                    );
                    let reviewer_prompt = format!(
                        "Task: {}\n\nAs Reviewer: select one recommended plan from candidates with rationale and risks.",
                        task_str
                    );

                    let planner_output = self
                        .run_agent_collecting(
                            planner_agent_name.clone(),
                            planner_agent,
                            vec![Message {
                                role: "user".to_string(),
                                content: planner_prompt,
                            }],
                            None,
                            None,
                            Some(Duration::from_secs(120)),
                        )
                        .await?;
                    let researcher_output = self
                        .run_agent_collecting(
                            researcher_agent_name.clone(),
                            researcher_agent,
                            vec![Message {
                                role: "user".to_string(),
                                content: researcher_prompt,
                            }],
                            None,
                            None,
                            Some(Duration::from_secs(120)),
                        )
                        .await?;
                    let reviewer_output = self
                        .run_agent_collecting(
                            reviewer_agent_name.clone(),
                            reviewer_agent,
                            vec![Message {
                                role: "user".to_string(),
                                content: reviewer_prompt,
                            }],
                            None,
                            None,
                            Some(Duration::from_secs(120)),
                        )
                        .await?;

                    let recommended_plan = reviewer_output.chars().take(500).collect::<String>();
                    let artifact = WorkflowResearchArtifact {
                        generated_at: now_ts(),
                        task: task_str.clone(),
                        planner_output,
                        researcher_output,
                        recommended_plan: recommended_plan.clone(),
                        reviewer_output,
                    };
                    let artifact_path = persist_workflow_research(&ledger, &artifact)?;
                    let summary = recommended_plan.chars().take(240).collect::<String>();

                    // Inject the research consensus into subtask prompts so execution follows the selected plan.
                    for subtask in plan.planned_subtasks.iter_mut() {
                        subtask.description = format!(
                            "Research consensus:\n{}\n\nSubtask:\n{}",
                            summary, subtask.description
                        );
                    }

                    research_summary = Some(summary);
                    research_artifact_path = Some(artifact_path);
                }

                let plan_artifact_path = persist_task_plan(&ledger, &plan)?;
                if is_workflow_execute {
                    let workflow = build_workflow_generated_artifact(&plan);
                    workflow_meta = Some(json!({
                        "nodes": workflow.nodes.len(),
                        "edges": workflow.edges.len(),
                        "execution_phases": workflow.execution_order.len(),
                    }));
                    workflow_artifact_path = Some(persist_workflow_generated(&ledger, &workflow)?);
                }

                let exec_started_ts = now_ts();
                let runtime_healthy =
                    !self.lifecycle.is_shutting_down() && self.circuit_breakers.open_count() == 0;

                let optimization_outcome = evaluate_optimization_policy(
                    &ledger,
                    &task_str,
                    &plan,
                    routing.phase.options.as_ref(),
                    runtime_healthy,
                    is_workflow_execute,
                );
                let requested_grade = params
                    .get("work_grade")
                    .and_then(|v| v.as_str())
                    .or_else(|| params.get("mode").and_then(|v| v.as_str()));
                let mut work_grade_decision = decide_work_grade(
                    requested_grade,
                    &plan,
                    is_workflow_execute,
                    runtime_healthy,
                    optimization_outcome.force_fail_fast,
                );
                let adaptive_work_grade = params
                    .get("adaptive_work_grade")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(is_workflow_execute);
                if adaptive_work_grade {
                    let recommended = recommend_work_grade_from_learning(
                        &ledger,
                        work_grade_decision.decided.as_str(),
                    );
                    if let Some(recommended_grade) = WorkGrade::parse(Some(&recommended)) {
                        if recommended_grade != work_grade_decision.decided {
                            work_grade_decision.reasons.push(format!(
                                "LearningBus tuned work grade from {} to {} based on recent cross-task outcomes",
                                work_grade_decision.decided.as_str(),
                                recommended_grade.as_str()
                            ));
                            work_grade_decision.decided = recommended_grade;
                            work_grade_decision.decision_action = work_grade_action(
                                work_grade_decision.requested,
                                work_grade_decision.decided,
                            );
                        }
                    }
                }
                if capability_forced_degrade && work_grade_decision.decided != WorkGrade::Safeguard {
                    work_grade_decision.decided = WorkGrade::Safeguard;
                    work_grade_decision.reasons.push(
                        "capability ceiling exceeded and user selected degrade; force safeguard work grade"
                            .to_string(),
                    );
                    work_grade_decision.decision_action = work_grade_action(
                        work_grade_decision.requested,
                        work_grade_decision.decided,
                    );
                }

                let mut completed = 0usize;
                let mut failed = 0usize;
                let mut skipped = 0usize;

                let phase_parallelism_base = extra_u64(routing.phase.options.as_ref(), "phase_max_inflight")
                    .or_else(|| extra_u64(routing.phase.options.as_ref(), "subtask_parallelism"))
                    .map(|value| value.max(1) as usize)
                    .unwrap_or(4);
                let mut phase_parallelism_base = optimization_outcome
                    .phase_parallelism_cap
                    .map(|cap| phase_parallelism_base.min(cap.max(1)))
                    .unwrap_or(phase_parallelism_base);
                if capability_forced_degrade {
                    phase_parallelism_base = 1;
                }
                let adaptive_parallelism = params
                    .get("adaptive_parallelism")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(is_workflow_execute);
                let phase_parallelism = if adaptive_parallelism {
                    recommend_parallelism_from_learning(&ledger, phase_parallelism_base, 1, 16)
                } else {
                    phase_parallelism_base
                };
                let phase_parallelism = if capability_forced_degrade {
                    1
                } else {
                    phase_parallelism
                };
                let parallelism_tuned = phase_parallelism != phase_parallelism_base;

                let role_aware_assignment = params
                    .get("role_aware_assignment")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(is_workflow_execute);
                let assignment_workflow = build_workflow_generated_artifact(&plan);
                let role_map: HashMap<String, String> = if role_aware_assignment {
                    assignment_workflow
                        .nodes
                        .iter()
                        .map(|node| (node.id.clone(), node.role.clone()))
                        .collect()
                } else {
                    HashMap::new()
                };
                let dependency_count_map: HashMap<String, usize> = assignment_workflow
                    .nodes
                    .iter()
                    .map(|node| (node.id.clone(), node.dependencies.len()))
                    .collect();

                let fail_fast_base = params
                    .get("fail_fast")
                    .and_then(|v| v.as_bool())
                    .or_else(|| {
                        extra_string(routing.phase.options.as_ref(), "subtask_failure_strategy")
                            .map(|v| v.eq_ignore_ascii_case("fail_fast"))
                    })
                    .unwrap_or(false);
                let fail_fast_base =
                    fail_fast_base || optimization_outcome.force_fail_fast || capability_forced_degrade;
                let adaptive_failure_strategy = params
                    .get("adaptive_failure_strategy")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(is_workflow_execute);
                let fail_fast = if adaptive_failure_strategy {
                    recommend_failure_strategy_from_learning(
                        &ledger,
                        if fail_fast_base { "fail_fast" } else { "tolerant" },
                    )
                    .eq_ignore_ascii_case("fail_fast")
                } else {
                    fail_fast_base
                };
                let failure_strategy = if fail_fast { "fail_fast" } else { "tolerant" };
                let failure_strategy_tuned = fail_fast != fail_fast_base;
                let review_policy = resolve_review_policy(
                    routing.phase.options.as_ref(),
                    Some(&plan.characteristics),
                    is_workflow_execute,
                    false,
                );
                let review_started = Instant::now();
                let review_decisions = if review_policy.enforce_dual_review {
                    let execute_review_messages = vec![Message {
                        role: "user".to_string(),
                        content: task_str.clone(),
                    }];
                    match self
                        .run_dual_review_gate(
                            request_id.clone(),
                            &execute_review_messages,
                            routing.phase.options.as_ref(),
                            None,
                            &trace,
                        )
                        .await
                    {
                        Ok(ReviewGateOutcome::Approved(decisions)) => {
                            self.record_trace_event(
                                &child_trace_context(&trace, "execute.review"),
                                "phase.review_gate",
                                "ok",
                                "review",
                                json!({
                                    "policy_status": "pass",
                                    "result": "approved",
                                    "review_decisions": decisions.len(),
                                    "method": method,
                                }),
                                None,
                                review_started.elapsed().as_millis() as u64,
                            );
                            Some(decisions)
                        }
                        Ok(ReviewGateOutcome::Rejected(decisions)) => {
                            self.record_trace_event(
                                &child_trace_context(&trace, "execute.review"),
                                "phase.review_gate",
                                "error",
                                "review",
                                json!({
                                    "policy_status": "blocked",
                                    "result": "rejected",
                                    "review_decisions": decisions.len(),
                                    "method": method,
                                }),
                                Some("review gate rejected execution".to_string()),
                                review_started.elapsed().as_millis() as u64,
                            );
                            return self
                                .send_error(
                                    request_id,
                                    -32603,
                                    "review gate rejected execution".to_string(),
                                    Some(json!({
                                        "kind": "review_gate",
                                        "method": method,
                                        "reviews": decisions,
                                    })),
                                )
                                .await;
                        }
                        Ok(ReviewGateOutcome::Degraded(decisions)) => {
                            self.record_trace_event(
                                &child_trace_context(&trace, "execute.review"),
                                "phase.review_gate",
                                "ok",
                                "review",
                                json!({
                                    "policy_status": "degraded",
                                    "result": "degraded",
                                    "review_decisions": decisions.len(),
                                    "method": method,
                                }),
                                None,
                                review_started.elapsed().as_millis() as u64,
                            );
                            self.send_notification(
                                "workflow.review",
                                json!({
                                    "id": request_id.clone(),
                                    "mode": "degrade_single",
                                    "reason": "review gate timeout",
                                    "method": method,
                                }),
                            )
                            .await?;
                            Some(decisions)
                        }
                        Err(err) => {
                            self.record_trace_event(
                                &child_trace_context(&trace, "execute.review"),
                                "phase.review_gate",
                                "error",
                                "review",
                                json!({
                                    "policy_status": "error",
                                    "method": method,
                                }),
                                Some(err.to_string()),
                                review_started.elapsed().as_millis() as u64,
                            );
                            return self
                                .send_error(
                                    request_id,
                                    -32603,
                                    crate::i18n::tf("error.review_gate_failed", &[("error", &format!("{err}"))]),
                                    Some(json!({
                                        "kind": "review_gate",
                                        "method": method,
                                    })),
                                )
                                .await;
                        }
                    }
                } else {
                    None
                };

                let mut serial_work_ms: u64 = 0;
                let mut critical_path_ms: u64 = 0;
                let mut phases_executed: usize = 0;
                let mut halted_early = false;
                let mut phase_parallel_utilization_sum: f64 = 0.0;
                let mut serial_degradation_count: usize = 0;
                let mut parallel_failure_rollback_count: usize = 0;
                let mut assignment_audit_records: Vec<ExecutionAssignmentRecord> = Vec::new();
                let mut parallel_phase_decisions: Vec<ParallelPhaseDecisionRecord> = Vec::new();
                let mut selected_agents_audit: HashSet<String> = HashSet::new();
                let learning_clarification =
                    resolve_learning_clarification_metrics(&ledger, &task_str, &params);

                // M5/M6/M7: pre-compute failover secondaries once for this execution
                let failover_policy_str = primary_secondary_policy.failover_policy.clone();
                let failover_secondary_runs: Vec<(String, std::sync::Arc<dyn crate::agent::Agent>)> =
                    primary_secondary_policy
                        .secondary_agents
                        .iter()
                        .filter_map(|name| registry.get(name).map(|a| (name.clone(), a)))
                        .collect();
                let mut total_failover_count: u32 = 0;

                let mut phase_records: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
                for (index, record) in plan.planned_subtasks.iter().enumerate() {
                    phase_records.entry(record.phase_index).or_default().push(index);
                }

                for (phase_index, indexes) in phase_records {
                    let mut phase_failed = false;
                    let mut phase_sum_duration_ms: u64 = 0;
                    let mut phase_max_duration_ms: u64 = 0;
                    let phase_width = indexes.len();
                    let has_phase_dependencies = indexes.iter().any(|idx| {
                        plan.planned_subtasks
                            .get(*idx)
                            .and_then(|record| dependency_count_map.get(&record.id))
                            .copied()
                            .unwrap_or(0)
                            > 0
                    });
                    let phase_parallelism_effective = if has_phase_dependencies {
                        1
                    } else {
                        phase_parallelism.max(1)
                    };
                    let phase_capacity = phase_parallelism_effective.max(1);
                    let phase_utilization =
                        (phase_width.min(phase_capacity) as f64 / phase_capacity as f64)
                            .clamp(0.0, 1.0);
                    phase_parallel_utilization_sum += phase_utilization;
                    if phase_width <= 1 || phase_capacity <= 1 {
                        serial_degradation_count = serial_degradation_count.saturating_add(1);
                    }

                    let mut phase_assignment_lookup: HashMap<usize, Option<String>> = HashMap::new();
                    for idx in &indexes {
                        let Some(record) = plan.planned_subtasks.get(*idx) else {
                            continue;
                        };
                        let subtask_id = record.id.clone();
                        let desired_role = role_map.get(&subtask_id).cloned();
                        let dependency_blocked = dependency_count_map
                            .get(&subtask_id)
                            .copied()
                            .unwrap_or(0)
                            > 0;
                        let ranked_candidates = rank_execution_agents(
                            &phase_agent_names,
                            desired_role.as_deref(),
                            phase_index,
                            *idx,
                        );
                        let selected_agent = ranked_candidates.first().map(|candidate| {
                            selected_agents_audit.insert(candidate.agent.clone());
                            candidate.agent.clone()
                        });
                        let selection_reason = ranked_candidates
                            .first()
                            .map(|candidate| candidate.reason.clone())
                            .unwrap_or_else(|| {
                                "no candidate agent available for this subtask".to_string()
                            });

                        phase_assignment_lookup.insert(*idx, selected_agent.clone());
                        assignment_audit_records.push(ExecutionAssignmentRecord {
                            subtask_id,
                            phase_index,
                            task_index: *idx,
                            desired_role,
                            selected_agent: selected_agent.clone(),
                            selection_reason,
                            candidate_scores: ranked_candidates,
                            dependency_blocked,
                            node_primary_agent: selected_agent,
                            node_secondary_agents: primary_secondary_policy.secondary_agents.clone(),
                            effective_executor: None,
                            failover_applied: false,
                            failover_reason: None,
                        });
                    }

                    parallel_phase_decisions.push(ParallelPhaseDecisionRecord {
                        phase_index,
                        subtask_count: phase_width,
                        parallelism_limit: phase_parallelism_effective,
                        utilization_target: phase_utilization,
                        has_dependencies: has_phase_dependencies,
                        execution_mode: if phase_parallelism_effective > 1 {
                            "parallel".to_string()
                        } else {
                            "serial".to_string()
                        },
                        reason: if has_phase_dependencies {
                            "phase contains dependency edges; enforce serial execution for safety"
                                .to_string()
                        } else {
                            "phase subtasks are independent; allow bounded parallel execution"
                                .to_string()
                        },
                    });

                    let tasks = indexes.iter().map(|index| {
                        let idx = *index;
                        let description = plan.planned_subtasks[idx].description.clone();
                        let subtask_id = plan.planned_subtasks[idx].id.clone();
                        let assigned_agent = phase_assignment_lookup
                            .get(&idx)
                            .cloned()
                            .unwrap_or(None);
                        let run_agent = assigned_agent
                            .as_ref()
                            .and_then(|name| registry.get(name));

                        // M5/M6/M7: capture failover context per closure
                        let fallback_runs = failover_secondary_runs.clone();
                        let failover_policy = failover_policy_str.clone();

                        async move {
                            let subtask_wall = Instant::now();
                            let subtask_start_ts = now_ts();

                            let Some(run_agent_name) = assigned_agent else {
                                let subtask_stop_ts = now_ts();
                                return (
                                    idx,
                                    subtask_id,
                                    "none".to_string(),
                                    subtask_start_ts,
                                    subtask_stop_ts,
                                    0u64,
                                    Ok(None),
                                    "none".to_string(),
                                    false,
                                    None::<String>,
                                );
                            };
                            let Some(run_agent) = run_agent else {
                                let subtask_stop_ts = now_ts();
                                return (
                                    idx,
                                    subtask_id,
                                    run_agent_name.clone(),
                                    subtask_start_ts,
                                    subtask_stop_ts,
                                    0u64,
                                    Ok(None),
                                    run_agent_name,
                                    false,
                                    None::<String>,
                                );
                            };

                            let description_for_failover = description.clone();
                            let messages = vec![Message {
                                role: "user".to_string(),
                                content: description,
                            }];

                            let primary_result = self
                                .run_agent_collecting(
                                    run_agent_name.clone(),
                                    run_agent,
                                    messages,
                                    None,
                                    None,
                                    Some(Duration::from_secs(120)),
                                )
                                .await
                                .map(Some);

                            // Failover chain: apply policy when primary fails
                            let (effective_executor, failover_applied, failover_reason, sub_result) =
                                if primary_result.is_err() {
                                    match failover_policy.as_str() {
                                        "abort" => {
                                            let reason = format!(
                                                "primary agent '{}' failed; failover_policy=abort",
                                                run_agent_name
                                            );
                                            (run_agent_name.clone(), false, Some(reason), primary_result)
                                        }
                                        "score_based_secondary" => {
                                            let mut found = false;
                                            let mut fb_executor = run_agent_name.clone();
                                            let mut fb_reason = Some(format!(
                                                "primary '{}' failed; score_based_secondary: no eligible secondary succeeded",
                                                run_agent_name
                                            ));
                                            let mut fb_result = primary_result;
                                            for (fb_name, fb_agent) in &fallback_runs {
                                                let fb_msgs = vec![Message {
                                                    role: "user".to_string(),
                                                    content: description_for_failover.clone(),
                                                }];
                                                let attempt = self
                                                    .run_agent_collecting(
                                                        fb_name.clone(),
                                                        fb_agent.clone(),
                                                        fb_msgs,
                                                        None,
                                                        None,
                                                        Some(Duration::from_secs(120)),
                                                    )
                                                    .await
                                                    .map(Some);
                                                if attempt.is_ok() {
                                                    fb_executor = fb_name.clone();
                                                    fb_reason = Some(format!(
                                                        "primary '{}' failed; score_based_secondary '{}' took over",
                                                        run_agent_name, fb_name
                                                    ));
                                                    fb_result = attempt;
                                                    found = true;
                                                    break;
                                                }
                                            }
                                            (fb_executor, found, fb_reason, fb_result)
                                        }
                                        _ => {
                                            // default / "first_secondary"
                                            if let Some((fb_name, fb_agent)) = fallback_runs.first() {
                                                let fb_msgs = vec![Message {
                                                    role: "user".to_string(),
                                                    content: description_for_failover.clone(),
                                                }];
                                                let attempt = self
                                                    .run_agent_collecting(
                                                        fb_name.clone(),
                                                        fb_agent.clone(),
                                                        fb_msgs,
                                                        None,
                                                        None,
                                                        Some(Duration::from_secs(120)),
                                                    )
                                                    .await
                                                    .map(Some);
                                                if attempt.is_ok() {
                                                    let reason = format!(
                                                        "primary '{}' failed; first_secondary '{}' took over",
                                                        run_agent_name, fb_name
                                                    );
                                                    (fb_name.clone(), true, Some(reason), attempt)
                                                } else {
                                                    let reason = format!(
                                                        "primary '{}' and first_secondary '{}' both failed",
                                                        run_agent_name, fb_name
                                                    );
                                                    (run_agent_name.clone(), false, Some(reason), primary_result)
                                                }
                                            } else {
                                                (
                                                    run_agent_name.clone(),
                                                    false,
                                                    Some("no secondary agents available for failover".to_string()),
                                                    primary_result,
                                                )
                                            }
                                        }
                                    }
                                } else {
                                    (run_agent_name.clone(), false, None, primary_result)
                                };

                            let duration_ms = subtask_wall.elapsed().as_millis() as u64;
                            let subtask_stop_ts = now_ts();
                            (
                                idx,
                                subtask_id,
                                run_agent_name,
                                subtask_start_ts,
                                subtask_stop_ts,
                                duration_ms,
                                sub_result,
                                effective_executor,
                                failover_applied,
                                failover_reason,
                            )
                        }
                    });

                    let results = stream::iter(tasks)
                        .buffer_unordered(phase_parallelism_effective)
                        .collect::<Vec<_>>()
                        .await;

                    phases_executed += 1;
                    for (
                        idx,
                        subtask_id,
                        run_agent_name,
                        subtask_start_ts,
                        subtask_stop_ts,
                        duration_ms,
                        sub_result,
                        effective_executor,
                        failover_applied,
                        failover_reason,
                    ) in results
                    {
                        // Update assignment audit record with node execution outcome
                        if let Some(audit) = assignment_audit_records
                            .iter_mut()
                            .find(|r| r.subtask_id == subtask_id)
                        {
                            audit.effective_executor = Some(effective_executor.clone());
                            audit.failover_applied = failover_applied;
                            audit.failover_reason = failover_reason.clone();
                        }
                        if failover_applied {
                            total_failover_count = total_failover_count.saturating_add(1);
                        }

                        let Some(record) = plan.planned_subtasks.get_mut(idx) else {
                            continue;
                        };

                        phase_sum_duration_ms = phase_sum_duration_ms.saturating_add(duration_ms);
                        phase_max_duration_ms = phase_max_duration_ms.max(duration_ms);

                        match sub_result {
                            Ok(Some(_response)) => {
                                record.mark_executed(
                                    subtask_start_ts,
                                    subtask_stop_ts,
                                    duration_ms,
                                    "completed",
                                    &effective_executor,
                                );
                                completed += 1;
                                info!(
                                    subtask_id = %subtask_id,
                                    executor = %effective_executor,
                                    failover_applied,
                                    duration_ms,
                                    "subtask completed"
                                );
                            }
                            Ok(None) => {
                                record.mark_executed(
                                    subtask_start_ts,
                                    subtask_stop_ts,
                                    0,
                                    "skipped",
                                    &effective_executor,
                                );
                                skipped += 1;
                            }
                            Err(err) => {
                                record.mark_executed(
                                    subtask_start_ts,
                                    subtask_stop_ts,
                                    duration_ms,
                                    "failed",
                                    &run_agent_name,
                                );
                                failed += 1;
                                warn!(
                                    subtask_id = %subtask_id,
                                    executor = %run_agent_name,
                                    error = %err,
                                    "subtask failed"
                                );
                                phase_failed = true;
                            }
                        }
                    }

                    serial_work_ms = serial_work_ms.saturating_add(phase_sum_duration_ms);
                    critical_path_ms = critical_path_ms.saturating_add(phase_max_duration_ms);

                    if fail_fast && phase_failed {
                        halted_early = true;
                        break;
                    }
                }

                if halted_early {
                    for record in plan.planned_subtasks.iter_mut() {
                        if record.status == "planned" {
                            let ts = now_ts();
                            record.mark_executed(ts, ts, 0, "skipped", "none");
                            skipped += 1;
                            parallel_failure_rollback_count =
                                parallel_failure_rollback_count.saturating_add(1);
                        }
                    }
                }

                let parallel_utilization = if phases_executed == 0 {
                    0.0
                } else {
                    (phase_parallel_utilization_sum / phases_executed as f64).clamp(0.0, 1.0)
                };

                let exec_stop_ts = now_ts();
                let parallel_efficiency = if serial_work_ms == 0 {
                    1.0
                } else {
                    (critical_path_ms as f64 / serial_work_ms as f64).clamp(0.0, 1.0)
                };
                let parallel_speedup = if critical_path_ms == 0 {
                    1.0
                } else {
                    serial_work_ms as f64 / critical_path_ms as f64
                };
                let summary = TaskExecutionSummary {
                    generated_at: exec_stop_ts,
                    task: task_str.clone(),
                    subtasks_total: plan.planned_subtasks.len(),
                    subtasks_completed: completed,
                    subtasks_failed: failed,
                    subtasks_skipped: skipped,
                    executor: executor_label.clone(),
                    records: plan.planned_subtasks.clone(),
                    execution_metrics: Some(TaskExecutionMetrics {
                        subtask_parallelism: phase_parallelism,
                        failure_strategy: failure_strategy.to_string(),
                        phases_executed,
                        halted_early,
                        parallel_utilization,
                        serial_degradation_count,
                        parallel_failure_rollback_count,
                        serial_work_ms,
                        critical_path_ms,
                        parallel_efficiency,
                        parallel_speedup,
                    }),
                    artifact_path: None,
                };
                let artifact_path = persist_task_execution_summary(&ledger, &summary)?;
                // Extract failover root cause before assignment_audit_records is moved into the artifact
                let failover_root_cause_str = assignment_audit_records
                    .iter()
                    .filter(|r| r.failover_applied)
                    .filter_map(|r| r.failover_reason.as_deref())
                    .next()
                    .unwrap_or("")
                    .to_string();
                let mut selected_agents = selected_agents_audit.into_iter().collect::<Vec<_>>();
                selected_agents.sort();
                let execution_decision_artifact = ExecutionDecisionArtifact {
                    generated_at: exec_stop_ts,
                    task: task_str.clone(),
                    source: method.to_string(),
                    selected_agents,
                    assignment_reason: format!(
                        "adaptive_agent_order={}, role_aware_assignment={}, capability_decision={}, env_ready_agents={}",
                        adaptive_agent_order,
                        role_aware_assignment,
                        capability_decision_effective,
                        phase_agent_names.len()
                    ),
                    subtask_assignments: assignment_audit_records,
                    parallel_phase_decisions,
                    parallelism: phase_parallelism,
                    failure_strategy: failure_strategy.to_string(),
                    degrade_policy: capability_decision_effective.to_string(),
                };
                let primary_failover_reports = execution_decision_artifact
                    .subtask_assignments
                    .iter()
                    .map(|record| PrimaryFailoverReportItem {
                        subtask_id: record.subtask_id.clone(),
                        phase_index: record.phase_index,
                        selected_primary_agent: record.node_primary_agent.clone(),
                        effective_executor: record.effective_executor.clone(),
                        failover_applied: record.failover_applied,
                        failover_reason: record.failover_reason.clone(),
                    })
                    .collect::<Vec<_>>();
                let execution_decision_artifact_path =
                    persist_execution_decision(&ledger, &execution_decision_artifact)?;
                let primary_failover_count = primary_failover_reports
                    .iter()
                    .filter(|report| report.failover_applied)
                    .count();
                let primary_failover_artifact_path = persist_primary_secondary_failover_artifact(
                    &ledger,
                    &PrimarySecondaryFailoverArtifact {
                        generated_at: exec_stop_ts,
                        task: task_str.clone(),
                        source: method.to_string(),
                        primary_agent: primary_secondary_policy.primary_agent.clone(),
                        secondary_agents: primary_secondary_policy.secondary_agents.clone(),
                        failover_policy: primary_secondary_policy.failover_policy.clone(),
                        total_subtasks: primary_failover_reports.len(),
                        failover_count: primary_failover_count,
                        reports: primary_failover_reports.clone(),
                    },
                )?;
                let optimization_artifact_path = persist_workflow_optimization_policy(
                    &ledger,
                    &WorkflowOptimizationPolicyArtifact {
                        generated_at: exec_stop_ts,
                        task: task_str.clone(),
                        source: method.to_string(),
                        policy_report: serde_json::to_value(&optimization_outcome.report)
                            .unwrap_or(Value::Null),
                        phase_parallelism_cap: optimization_outcome
                            .phase_parallelism_cap
                            .map(|value| value as u64),
                        force_fail_fast: optimization_outcome.force_fail_fast,
                        runtime_healthy,
                        anomaly_detected: optimization_outcome.report.anomaly_detected,
                        detached_modules: optimization_outcome.report.detached_modules.clone(),
                        reattached_modules: optimization_outcome.report.reattached_modules.clone(),
                    },
                )?;

                let auto_gates = params
                    .get("auto_gates")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(is_workflow_execute);
                let auto_gates = if review_policy.enforce_action_gates {
                    true
                } else {
                    auto_gates
                };
                let mut gate_reports = Vec::new();
                let mut gates_ok = true;
                if auto_gates {
                    let policy_gates = action_check_kinds_from_policy(&review_policy.required_checks);
                    let gates = if policy_gates.is_empty() {
                        vec![
                            ActionCheckKind::Qa,
                            ActionCheckKind::Retest,
                            ActionCheckKind::Final,
                        ]
                    } else {
                        policy_gates
                    };
                    for gate in gates {
                        let report = run_action_check(&ledger, gate)?;
                        if !report.ok {
                            gates_ok = false;
                        }
                        gate_reports.push(report);
                    }
                }
                let final_gate_report = gate_reports.iter().find(|report| report.kind == "final");
                let review_reject_root_cause = if failed > 0 {
                    "subtask_failed".to_string()
                } else if auto_gates && !gates_ok {
                    gate_reports
                        .iter()
                        .find(|report| !report.ok)
                        .map(|report| format!("action_check:{}", report.kind))
                        .unwrap_or_else(|| "action_check_failed".to_string())
                } else {
                    String::new()
                };
                let final_conclusion = json!({
                    "status": if failed == 0 && (!auto_gates || gates_ok) {
                        "approved"
                    } else {
                        "needs_attention"
                    },
                    "summary": if failed == 0 && (!auto_gates || gates_ok) {
                        "workflow execution and gates passed"
                    } else if failed > 0 {
                        "workflow execution contains failed subtasks"
                    } else {
                        "workflow execution completed but gate checks failed"
                    },
                    "evidence_refs": final_gate_report
                        .map(|report| report.evidence_refs.clone())
                        .unwrap_or_default(),
                    "final_summary_path": final_gate_report
                        .and_then(|report| report.final_summary_path.clone()),
                    "retest_report_path": final_gate_report
                        .and_then(|report| report.retest_report_path.clone()),
                });

                if (failed > 0 || !gates_ok) && work_grade_decision.decided != WorkGrade::Safeguard {
                    work_grade_decision.decided = WorkGrade::Safeguard;
                    work_grade_decision.reasons.push(
                        "execution produced failures or gate rejection, escalated to safeguard"
                            .to_string(),
                    );
                    work_grade_decision.decision_action = work_grade_action(
                        work_grade_decision.requested,
                        work_grade_decision.decided,
                    );
                }

                let work_grade_artifact_path = persist_workflow_work_grade(
                    &ledger,
                    &WorkflowWorkGradeArtifact {
                        generated_at: exec_stop_ts,
                        task: task_str.clone(),
                        source: method.to_string(),
                        requested_grade: work_grade_decision.requested.as_str().to_string(),
                        decided_grade: work_grade_decision.decided.as_str().to_string(),
                        decision_action: work_grade_decision.decision_action.clone(),
                        reasons: work_grade_decision.reasons.clone(),
                        risk_score: work_grade_decision.risk_score,
                    },
                )?;
                let learning_artifact_path = persist_workflow_learning_event(
                    &ledger,
                    WorkflowLearningEvent {
                        generated_at: exec_stop_ts,
                        task: task_str.clone(),
                        complexity: plan.characteristics.complexity,
                        predicted_success_rate: plan.routing.predicted_success_rate,
                        subtasks_total: summary.subtasks_total,
                        subtasks_completed: completed,
                        subtasks_failed: failed,
                        subtasks_skipped: skipped,
                        serial_work_ms,
                        critical_path_ms,
                        parallel_speedup,
                        parallel_efficiency,
                        executor: executor_label.clone(),
                        source: method.to_string(),
                        runtime_healthy,
                        gates_ok,
                        work_grade: work_grade_decision.decided.as_str().to_string(),
                        risk_score: work_grade_decision.risk_score,
                        clarification_rounds: learning_clarification.rounds,
                        clarification_quality_score: learning_clarification.quality_score,
                        requirement_change_count: learning_clarification.requirement_change_count,
                        review_reject_root_cause: review_reject_root_cause.clone(),
                        primary_stability_score: if summary.subtasks_total == 0 {
                            1.0
                        } else {
                            1.0 - (total_failover_count as f64 / summary.subtasks_total as f64)
                        },
                        secondary_utilization_rate: if summary.subtasks_total == 0 {
                            0.0
                        } else {
                            total_failover_count as f64 / summary.subtasks_total as f64
                        },
                        failover_count: total_failover_count,
                        failover_root_cause: failover_root_cause_str,
                    },
                    200,
                )?;
                let pipeline_metrics_artifact_path = persist_pipeline_unified_metrics(
                    &ledger,
                    &PipelineUnifiedMetricsArtifact {
                        generated_at: exec_stop_ts,
                        task: task_str.clone(),
                        source: method.to_string(),
                        predicted_success_rate: plan.routing.predicted_success_rate as f64,
                        risk_score: work_grade_decision.risk_score,
                        runtime_healthy,
                        gates_ok,
                        subtasks_total: summary.subtasks_total,
                        subtasks_completed: completed,
                        subtasks_failed: failed,
                        subtasks_skipped: skipped,
                        parallelism: phase_parallelism,
                        parallel_utilization,
                        serial_degradation_count,
                        parallel_failure_rollback_count,
                        failure_strategy: failure_strategy.to_string(),
                        work_grade: work_grade_decision.decided.as_str().to_string(),
                        optimization_policy: serde_json::to_value(&optimization_outcome.report)
                            .unwrap_or(Value::Null),
                    },
                )?;

                self.record_trace_event(
                    &trace,
                    "phase.execute",
                    if failed == 0 { "ok" } else { "warn" },
                    "execute",
                    json!({
                        "task": task_str,
                        "subtasks_total": summary.subtasks_total,
                        "subtasks_completed": completed,
                        "subtasks_failed": failed,
                        "subtask_parallelism": phase_parallelism,
                        "adaptive_routing": adaptive_routing,
                        "predicted_success_rate_tuned": predicted_success_rate_tuned,
                        "adaptive_agent_order": adaptive_agent_order,
                        "agent_order_tuned": agent_order_tuned,
                        "capability_governance": capability_governance.clone(),
                        "blue5": {
                            "doc": blue5_doc.clone(),
                            "auto": blue5_auto.clone(),
                            "primary_secondary_policy": primary_secondary_policy.clone(),
                        },
                        "failure_strategy": failure_strategy,
                        "parallel_utilization": parallel_utilization,
                        "serial_degradation_count": serial_degradation_count,
                        "parallel_failure_rollback_count": parallel_failure_rollback_count,
                        "review_policy": review_policy.clone(),
                        "reviews": review_decisions.clone(),
                        "gates_ok": gates_ok,
                        "work_grade": work_grade_decision.decided.as_str(),
                        "execution_decision_artifact_path": execution_decision_artifact_path.display().to_string(),
                        "primary_failover_artifact_path": primary_failover_artifact_path.display().to_string(),
                        "primary_failover_report": {
                            "failover_policy": primary_secondary_policy.failover_policy.clone(),
                            "total_subtasks": primary_failover_reports.len(),
                            "failover_count": primary_failover_count,
                        },
                        "pipeline_metrics_artifact_path": pipeline_metrics_artifact_path.display().to_string(),
                        "learning_artifact_path": learning_artifact_path.display().to_string(),
                        "consultation_summary": consultation_summary.clone(),
                        "consultation_artifact_path": consultation_artifact_path
                            .as_ref()
                            .map(|path| path.display().to_string()),
                        "executor": executor_label,
                    }),
                    None,
                    (exec_stop_ts.saturating_sub(exec_started_ts)) as u64 * 1000,
                );

                self.send_result(
                    request_id,
                    json!({
                        "ok": failed == 0 && (!auto_gates || gates_ok),
                        "summary": summary,
                        "execution_metrics": {
                            "subtask_parallelism": phase_parallelism,
                            "subtask_parallelism_base": phase_parallelism_base,
                            "adaptive_parallelism": adaptive_parallelism,
                            "parallelism_tuned": parallelism_tuned,
                            "adaptive_routing": adaptive_routing,
                            "predicted_success_rate_tuned": predicted_success_rate_tuned,
                            "adaptive_agent_order": adaptive_agent_order,
                            "agent_order_tuned": agent_order_tuned,
                            "capability_governance": capability_governance.clone(),
                            "optimization_policy": optimization_outcome.report,
                            "auto_research": auto_research,
                            "research_summary": research_summary.clone(),
                            "consultation_summary": consultation_summary.clone(),
                            "role_aware_assignment": role_aware_assignment,
                            "adaptive_failure_strategy": adaptive_failure_strategy,
                            "failure_strategy_tuned": failure_strategy_tuned,
                            "failure_strategy": failure_strategy,
                            "clarification_rounds": learning_clarification.rounds,
                            "clarification_quality_score": learning_clarification.quality_score,
                            "requirement_change_count": learning_clarification.requirement_change_count,
                            "review_reject_root_cause": review_reject_root_cause,
                            "phases_executed": phases_executed,
                            "halted_early": halted_early,
                            "parallel_utilization": parallel_utilization,
                            "serial_degradation_count": serial_degradation_count,
                            "parallel_failure_rollback_count": parallel_failure_rollback_count,
                            "serial_work_ms": serial_work_ms,
                            "critical_path_ms": critical_path_ms,
                            "parallel_efficiency": parallel_efficiency,
                            "parallel_speedup": parallel_speedup,
                        },
                        "auto_gates": auto_gates,
                        "review_policy": review_policy,
                        "reviews": review_decisions,
                        "adaptive_work_grade": adaptive_work_grade,
                        "gates_ok": gates_ok,
                        "capability_governance": capability_governance,
                        "blue5": {
                            "doc": blue5_doc,
                            "auto": blue5_auto,
                            "primary_secondary_policy": primary_secondary_policy,
                        },
                        "gate_reports": gate_reports,
                        "final_conclusion": final_conclusion,
                        "plan_artifact_path": plan_artifact_path.display().to_string(),
                        "workflow_meta": workflow_meta,
                        "workflow_artifact_path": workflow_artifact_path
                            .as_ref()
                            .map(|path| path.display().to_string()),
                        "work_grade": {
                            "requested": work_grade_decision.requested.as_str(),
                            "decided": work_grade_decision.decided.as_str(),
                            "decision_action": work_grade_decision.decision_action.clone(),
                            "risk_score": work_grade_decision.risk_score,
                            "reasons": work_grade_decision.reasons.clone(),
                        },
                        "artifact_path": artifact_path.display().to_string(),
                        "execution_decision_artifact_path": execution_decision_artifact_path.display().to_string(),
                        "primary_failover_artifact_path": primary_failover_artifact_path.display().to_string(),
                        "primary_failover_report": {
                            "failover_policy": primary_secondary_policy.failover_policy.clone(),
                            "total_subtasks": primary_failover_reports.len(),
                            "failover_count": primary_failover_count,
                            "reports": primary_failover_reports,
                        },
                        "learning_artifact_path": learning_artifact_path.display().to_string(),
                        "work_grade_artifact_path": work_grade_artifact_path.display().to_string(),
                        "optimization_artifact_path": optimization_artifact_path.display().to_string(),
                        "pipeline_metrics_artifact_path": pipeline_metrics_artifact_path.display().to_string(),
                        "research_artifact_path": research_artifact_path
                            .as_ref()
                            .map(|path| path.display().to_string()),
                        "consultation_artifact_path": consultation_artifact_path
                            .as_ref()
                            .map(|path| path.display().to_string()),
                    }),
                )
                .await
            }
            "learning.summary" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let limit = params
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .map(|v| v.clamp(1, 500) as usize)
                    .unwrap_or(50);

                let ledger = self.artifact_ledger();
                let latest_path = ledger.latest_path("spec", "latest-learning.json");

                let bus = fs::read_to_string(&latest_path)
                    .ok()
                    .and_then(|raw| serde_json::from_str::<WorkflowLearningBusArtifact>(&raw).ok())
                    .unwrap_or(WorkflowLearningBusArtifact {
                        generated_at: now_ts(),
                        total_events: 0,
                        events: Vec::new(),
                    });

                let sampled = bus
                    .events
                    .iter()
                    .rev()
                    .take(limit)
                    .cloned()
                    .collect::<Vec<_>>();
                let sampled_count = sampled.len();

                let mut total_clarification_rounds: u64 = 0;
                let mut total_clarification_quality: f64 = 0.0;
                let mut total_requirement_change_count: u64 = 0;
                let mut total_risk_score: f64 = 0.0;
                let mut total_predicted_success_rate: f64 = 0.0;
                let mut total_parallel_efficiency: f64 = 0.0;
                let mut total_parallel_speedup: f64 = 0.0;
                let mut gate_pass_count: usize = 0;
                let mut runtime_healthy_count: usize = 0;
                let mut review_reject_root_causes: HashMap<String, usize> = HashMap::new();
                let mut total_primary_stability_sum: f64 = 0.0;
                let mut total_secondary_utilization_sum: f64 = 0.0;
                let mut total_failover_count_sum: u64 = 0;

                for event in &sampled {
                    total_clarification_rounds =
                        total_clarification_rounds.saturating_add(event.clarification_rounds as u64);
                    total_clarification_quality += event.clarification_quality_score;
                    total_requirement_change_count = total_requirement_change_count
                        .saturating_add(event.requirement_change_count as u64);
                    total_risk_score += event.risk_score;
                    total_predicted_success_rate += event.predicted_success_rate as f64;
                    total_parallel_efficiency += event.parallel_efficiency;
                    total_parallel_speedup += event.parallel_speedup;
                    if event.gates_ok {
                        gate_pass_count = gate_pass_count.saturating_add(1);
                    }
                    if event.runtime_healthy {
                        runtime_healthy_count = runtime_healthy_count.saturating_add(1);
                    }
                    total_primary_stability_sum += event.primary_stability_score;
                    total_secondary_utilization_sum += event.secondary_utilization_rate;
                    total_failover_count_sum =
                        total_failover_count_sum.saturating_add(event.failover_count as u64);

                    let cause = event.review_reject_root_cause.trim();
                    if !cause.is_empty() {
                        *review_reject_root_causes
                            .entry(cause.to_string())
                            .or_insert(0) += 1;
                    }
                }

                let denominator = sampled_count.max(1) as f64;
                self.send_result(
                    request_id,
                    json!({
                        "ok": true,
                        "summary": {
                            "artifact_path": latest_path.display().to_string(),
                            "total_events": bus.total_events,
                            "sampled_events": sampled_count,
                            "sample_limit": limit,
                            "averages": {
                                "clarification_rounds": total_clarification_rounds as f64 / denominator,
                                "clarification_quality_score": total_clarification_quality / denominator,
                                "risk_score": total_risk_score / denominator,
                                "predicted_success_rate": total_predicted_success_rate / denominator,
                                "parallel_efficiency": total_parallel_efficiency / denominator,
                                "parallel_speedup": total_parallel_speedup / denominator,
                                "primary_stability_score": total_primary_stability_sum / denominator,
                                "secondary_utilization_rate": total_secondary_utilization_sum / denominator,
                            },
                            "totals": {
                                "requirement_change_count": total_requirement_change_count,
                                "failover_count": total_failover_count_sum,
                            },
                            "rates": {
                                "gates_pass_rate": gate_pass_count as f64 / denominator,
                                "runtime_healthy_rate": runtime_healthy_count as f64 / denominator,
                            },
                            "review_reject_root_causes": review_reject_root_causes,
                        }
                    }),
                )
                .await
            }
            "primary_secondary.summary" => {
                // M10: aggregate primary-secondary governance metrics
                let params = request.params.unwrap_or_else(|| json!({}));
                let limit = params
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .map(|v| v.clamp(1, 500) as usize)
                    .unwrap_or(50);

                let ledger = self.artifact_ledger();
                let learning_path = ledger.latest_path("spec", "latest-learning.json");
                let policy_path =
                    ledger.latest_path("spec", "latest-primary-secondary-policy.json");

                let bus = fs::read_to_string(&learning_path)
                    .ok()
                    .and_then(|raw| serde_json::from_str::<WorkflowLearningBusArtifact>(&raw).ok())
                    .unwrap_or(WorkflowLearningBusArtifact {
                        generated_at: now_ts(),
                        total_events: 0,
                        events: Vec::new(),
                    });

                let sampled = bus
                    .events
                    .iter()
                    .rev()
                    .take(limit)
                    .cloned()
                    .collect::<Vec<_>>();
                let sampled_count = sampled.len();

                let mut ps_failover_count: u64 = 0;
                let mut ps_primary_stability: f64 = 0.0;
                let mut ps_secondary_utilization: f64 = 0.0;
                let mut failover_root_causes: HashMap<String, usize> = HashMap::new();

                for event in &sampled {
                    ps_failover_count =
                        ps_failover_count.saturating_add(event.failover_count as u64);
                    ps_primary_stability += event.primary_stability_score;
                    ps_secondary_utilization += event.secondary_utilization_rate;
                    let cause = event.failover_root_cause.trim();
                    if !cause.is_empty() {
                        *failover_root_causes.entry(cause.to_string()).or_insert(0) += 1;
                    }
                }

                let denominator = sampled_count.max(1) as f64;
                let latest_policy = fs::read_to_string(&policy_path)
                    .ok()
                    .and_then(|raw| serde_json::from_str::<Value>(&raw).ok());

                self.send_result(
                    request_id,
                    json!({
                        "ok": true,
                        "summary": {
                            "learning_artifact_path": learning_path.display().to_string(),
                            "policy_artifact_path": policy_path.display().to_string(),
                            "total_events": bus.total_events,
                            "sampled_events": sampled_count,
                            "sample_limit": limit,
                            "averages": {
                                "primary_stability_score": ps_primary_stability / denominator,
                                "secondary_utilization_rate": ps_secondary_utilization / denominator,
                            },
                            "totals": {
                                "failover_count": ps_failover_count,
                            },
                            "failover_root_causes": failover_root_causes,
                            "latest_policy": latest_policy,
                        }
                    }),
                )
                .await
            }
            "runtime.health" => {
                let report = self.runtime_healthcheck_report()?;
                let artifact_path = persist_runtime_healthcheck(&self.artifact_ledger(), &report)?;
                let runtime_details = report
                    .components
                    .iter()
                    .find(|component| component.name == "runtime")
                    .map(|component| component.details.clone())
                    .unwrap_or_else(|| json!({}));
                let sqlite_cache_entries = report
                    .components
                    .iter()
                    .find(|component| component.name == "cache")
                    .and_then(|component| component.details.get("entries"))
                    .cloned()
                    .unwrap_or(Value::Null);
                let vector = report
                    .components
                    .iter()
                    .find(|component| component.name == "vector")
                    .map(|component| component.details.clone())
                    .unwrap_or(Value::Null);
                self.send_result(
                    request_id,
                    json!({
                        "ok": report.overall_status != CheckStatus::Error,
                        "report": report,
                        "artifact_path": artifact_path.display().to_string(),
                        "memory_cache_entries": self.memory_cache.active_entries(),
                        "sqlite_cache_entries": sqlite_cache_entries,
                        "circuit_breaker": runtime_details.get("circuit_breaker").cloned().unwrap_or(Value::Null),
                        "rate_limiter": runtime_details.get("rate_limiter").cloned().unwrap_or(Value::Null),
                        "inflight": runtime_details.get("inflight").cloned().unwrap_or(Value::Null),
                        "vector": vector,
                        "lifecycle": runtime_details.get("lifecycle").cloned().unwrap_or(Value::Null),
                        "maintenance": runtime_details.get("maintenance").cloned().unwrap_or(Value::Null),
                        "review_gate": runtime_details.get("review_gate").cloned().unwrap_or(Value::Null),
                        "telemetry": runtime_details.get("telemetry").cloned().unwrap_or(Value::Null),
                    }),
                )
                .await
            }
            "action.check" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let kind = params
                    .get("kind")
                    .and_then(|v| v.as_str())
                    .and_then(ActionCheckKind::parse)
                    .unwrap_or(ActionCheckKind::All);
                let report = run_action_check(&self.artifact_ledger(), kind)?;
                self.send_result(
                    request_id,
                    json!({
                        "ok": report.ok,
                        "report": report,
                    }),
                )
                .await
            }
            "phase.status" => {
                let limiter = self
                    .phase_rate_limiter
                    .snapshot()
                    .into_iter()
                    .map(|(phase, (tokens, capacity))| {
                        (
                            phase,
                            json!({
                                "tokens": tokens,
                                "capacity": capacity,
                            }),
                        )
                    })
                    .collect::<serde_json::Map<String, Value>>();
                let inflight = self.inflight_limiter.snapshot().1;
                self.send_result(
                    request_id,
                    json!({
                        "rate_limiter": limiter,
                        "inflight": inflight,
                    }),
                )
                .await
            }
            "breaker.status" => {
                let now = now_ts();
                let status = self
                    .circuit_breakers
                    .snapshot()
                    .into_iter()
                    .map(|(agent, snapshot)| {
                        (
                            agent,
                            json!({
                                "consecutive_failures": snapshot.consecutive_failures,
                                "state": snapshot.state,
                                "open_until": snapshot.open_until,
                                "probe_in_flight": snapshot.probe_in_flight,
                                "open": snapshot.open_until.map(|ts| ts > now).unwrap_or(false),
                            }),
                        )
                    })
                    .collect::<serde_json::Map<String, Value>>();
                self.send_result(request_id, Value::Object(status)).await
            }
            "breaker.reset" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let target = params.get("agent").and_then(|v| v.as_str());
                let removed = if let Some(agent_name) = target {
                    self.circuit_breakers
                        .inner
                        .lock()
                        .ok()
                        .and_then(|mut guard| guard.remove(agent_name).map(|_| 1_usize))
                        .unwrap_or(0)
                } else {
                    self.circuit_breakers
                        .inner
                        .lock()
                        .map(|mut guard| {
                            let count = guard.len();
                            guard.clear();
                            count
                        })
                        .unwrap_or(0)
                };
                self.send_result(request_id, json!({"ok": true, "removed": removed}))
                    .await
            }
            "config.reload" => {
                let reloaded = self.reload_runtime_config().await?;
                self.send_result(request_id, reloaded).await
            }
            "cache.clear" => {
                let memory_removed = self.memory_cache.clear_all();
                let sqlite_removed = if let Some(cache) = self.cache_handle() {
                    self.cache_clear(cache.clone()).await.unwrap_or(0)
                } else {
                    0
                };

                let result = json!({
                    "ok": true,
                    "memory_removed": memory_removed,
                    "sqlite_removed": sqlite_removed,
                });
                self.send_result(request_id, result).await
            }
            "vector.clear" => {
                let (memory_removed, summary_removed) =
                    if let Some(store) = self.vector_store_handle() {
                        self.vector_clear(store.clone()).await?
                    } else {
                        (0, 0)
                    };

                let result = json!({
                    "ok": true,
                    "vector_removed": memory_removed,
                    "summary_removed": summary_removed,
                });
                self.send_result(request_id, result).await
            }
            "maintenance.gc" => {
                let cycle = self.run_maintenance_cycle("rpc").await;
                let result = json!({
                    "ok": true,
                    "memory_expired_removed": cycle.memory_expired_removed,
                    "sqlite_expired_removed": cycle.sqlite_expired_removed,
                    "cache_vacuumed": cycle.cache_vacuumed,
                    "vector_vacuumed": cycle.vector_vacuumed,
                    "maintenance": self.maintenance.snapshot(),
                });
                self.send_result(request_id, result).await
            }
            "autotune.get" => {
                if let Some(autotune) = self.autotune_handle() {
                    let state = autotune.lock().await;
                    let result = state.snapshot();
                    self.send_result(request_id, result).await
                } else {
                    self.send_error(
                        request_id,
                        -32603,
                        "autotune is not enabled".to_string(),
                        None,
                    )
                    .await
                }
            }
            "autotune.reset" => {
                if let Some(autotune) = self.autotune_handle() {
                    if let Some(config) = self.autotune_config_snapshot() {
                        let new_state = {
                            let mut state = autotune.lock().await;
                            *state = AutoTuneState::new(&config);
                            state.clone()
                        };
                        if let Some(path) = self.autotune_state_path_snapshot() {
                            let path_ref = path.as_str();
                            if let Err(e) = new_state.save(path_ref) {
                                warn!("{}", crate::i18n::tf("warning.failed_save_autotune", &[("error", &format!("{}", e))]));
                            }
                        } else {
                            warn!("autotune reset skipped persistence because no resolved state path is available");
                        }
                        self.send_result(request_id, json!({"ok": true})).await
                    } else {
                        self.send_error(
                            request_id,
                            -32603,
                            "autotune config not available".to_string(),
                            None,
                        )
                        .await
                    }
                } else {
                    self.send_error(
                        request_id,
                        -32603,
                        "autotune is not enabled".to_string(),
                        None,
                    )
                    .await
                }
            }
            "conversation.checkpoint.create" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let conversation_id_raw = params
                    .get("conversation_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("default");
                let conversation_id = match validate_storage_key(
                    conversation_id_raw,
                    "conversation_id",
                    MAX_CONVERSATION_ID_LEN,
                ) {
                    Ok(value) => value,
                    Err(message) => {
                        return self.send_error(request_id, -32602, message, None).await;
                    }
                };
                let branch_id_raw = params
                    .get("branch_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("main");
                let branch_id =
                    match validate_storage_key(branch_id_raw, "branch_id", MAX_BRANCH_ID_LEN) {
                        Ok(value) => value,
                        Err(message) => {
                            return self.send_error(request_id, -32602, message, None).await;
                        }
                    };
                let note = params
                    .get("note")
                    .and_then(|v| v.as_str())
                    .map(|v| v.to_string());
                let messages_value = match params.get("messages") {
                    Some(value) => value.clone(),
                    None => {
                        return self
                            .send_error(
                                request_id,
                                -32602,
                                "messages is required for conversation.checkpoint.create"
                                    .to_string(),
                                None,
                            )
                            .await;
                    }
                };
                let messages: Vec<Message> = match serde_json::from_value(messages_value) {
                    Ok(value) => value,
                    Err(err) => {
                        return self
                            .send_error(
                                request_id,
                                -32602,
                                crate::i18n::tf("error.invalid_messages_payload", &[("error", &format!("{err}"))]),
                                None,
                            )
                            .await;
                    }
                };

                match self.create_conversation_checkpoint(&conversation_id, &branch_id, messages, note)
                {
                    Ok(checkpoint) => {
                        self.send_result(
                            request_id,
                            json!({
                                "ok": true,
                                "checkpoint": checkpoint,
                            }),
                        )
                        .await
                    }
                    Err(message) => self.send_error(request_id, -32603, message, None).await,
                }
            }
            "conversation.checkpoint.list" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let conversation_id_raw = params
                    .get("conversation_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("default");
                let conversation_id = match validate_storage_key(
                    conversation_id_raw,
                    "conversation_id",
                    MAX_CONVERSATION_ID_LEN,
                ) {
                    Ok(value) => value,
                    Err(message) => {
                        return self.send_error(request_id, -32602, message, None).await;
                    }
                };
                let branch_id = match params.get("branch_id").and_then(|v| v.as_str()) {
                    Some(value) => {
                        match validate_storage_key(value, "branch_id", MAX_BRANCH_ID_LEN) {
                            Ok(valid) => Some(valid),
                            Err(message) => {
                                return self.send_error(request_id, -32602, message, None).await;
                            }
                        }
                    }
                    None => None,
                };
                let limit = params
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(50)
                    .min(500) as usize;

                match self.list_conversation_checkpoints(&conversation_id, branch_id.as_deref(), limit)
                {
                    Ok(checkpoints) => {
                        self.send_result(
                            request_id,
                            json!({
                                "ok": true,
                                "count": checkpoints.len(),
                                "checkpoints": checkpoints,
                            }),
                        )
                        .await
                    }
                    Err(message) => self.send_error(request_id, -32603, message, None).await,
                }
            }
            "conversation.rollback" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let conversation_id_raw = params
                    .get("conversation_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("default");
                let conversation_id = match validate_storage_key(
                    conversation_id_raw,
                    "conversation_id",
                    MAX_CONVERSATION_ID_LEN,
                ) {
                    Ok(value) => value,
                    Err(message) => {
                        return self.send_error(request_id, -32602, message, None).await;
                    }
                };
                let checkpoint_id = match params.get("checkpoint_id").and_then(|v| v.as_str()) {
                    Some(value) => {
                        match validate_storage_key(value, "checkpoint_id", MAX_CHECKPOINT_ID_LEN) {
                            Ok(valid) => valid,
                            Err(message) => {
                                return self.send_error(request_id, -32602, message, None).await;
                            }
                        }
                    }
                    None => {
                        return self
                            .send_error(
                                request_id,
                                -32602,
                                "checkpoint_id is required for conversation.rollback"
                                    .to_string(),
                                None,
                            )
                            .await;
                    }
                };
                let target_branch = match params.get("branch_id").and_then(|v| v.as_str()) {
                    Some(value) => {
                        match validate_storage_key(value, "branch_id", MAX_BRANCH_ID_LEN) {
                            Ok(valid) => Some(valid),
                            Err(message) => {
                                return self.send_error(request_id, -32602, message, None).await;
                            }
                        }
                    }
                    None => None,
                };

                if let Some(checkpoint) = self.rollback_conversation_checkpoint(
                    &conversation_id,
                    &checkpoint_id,
                    target_branch.as_deref(),
                ) {
                    self.send_result(
                        request_id,
                        json!({
                            "ok": true,
                            "conversation_id": conversation_id.clone(),
                            "branch_id": checkpoint.branch_id,
                            "checkpoint": checkpoint,
                            "messages": checkpoint.messages,
                        }),
                    )
                    .await
                } else {
                    self.send_error(
                        request_id,
                        -32602,
                        format!(
                            "checkpoint '{}' not found in conversation '{}'",
                            checkpoint_id, conversation_id
                        ),
                        None,
                    )
                    .await
                }
            }
            "conversation.checkpoint.prune" => {
                let params = request.params.unwrap_or_else(|| json!({}));
                let conversation_id_raw = params
                    .get("conversation_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("default");
                let conversation_id = match validate_storage_key(
                    conversation_id_raw,
                    "conversation_id",
                    MAX_CONVERSATION_ID_LEN,
                ) {
                    Ok(value) => value,
                    Err(message) => {
                        return self.send_error(request_id, -32602, message, None).await;
                    }
                };
                let branch_id = match params.get("branch_id").and_then(|v| v.as_str()) {
                    Some(value) => {
                        match validate_storage_key(value, "branch_id", MAX_BRANCH_ID_LEN) {
                            Ok(valid) => Some(valid),
                            Err(message) => {
                                return self.send_error(request_id, -32602, message, None).await;
                            }
                        }
                    }
                    None => None,
                };
                let keep = match params.get("keep") {
                    Some(value) => match value.as_u64() {
                        Some(0) => {
                            return self
                                .send_error(
                                    request_id,
                                    -32602,
                                    "keep must be >= 1 for conversation.checkpoint.prune"
                                        .to_string(),
                                    None,
                                )
                                .await;
                        }
                        Some(valid) => valid.min(500) as usize,
                        None => {
                            return self
                                .send_error(
                                    request_id,
                                    -32602,
                                    "keep must be an integer >= 1 for conversation.checkpoint.prune"
                                        .to_string(),
                                    None,
                                )
                                .await;
                        }
                    },
                    None => 20,
                };

                let prune =
                    self.prune_conversation_checkpoints(&conversation_id, branch_id.as_deref(), keep);
                self.send_result(
                    request_id,
                    json!({
                        "ok": true,
                        "conversation_id": conversation_id,
                        "removed": prune.removed,
                        "repaired_heads": prune.repaired_heads,
                        "dropped_heads": prune.dropped_heads,
                    }),
                )
                .await
            }
            "shutdown" => {
                self.begin_shutdown("rpc shutdown");
                self.send_result(
                    request_id,
                    json!({
                        "ok": true,
                        "lifecycle": self.lifecycle.snapshot(),
                    }),
                )
                .await
            }
            other => {
                self.send_error(
                    request_id,
                    -32601,
                    ProxyError::UnknownMethod(other.to_string()).to_string(),
                    None,
                )
                .await
            }
            }
        }
        .await;

        let duration_ms = started.elapsed().as_millis() as u64;
        match &result {
            Ok(_) => self.record_trace_event(
                &trace,
                "request.end",
                "ok",
                "rpc",
                json!({
                    "method": method,
                    "request_id": trace.request_id,
                }),
                None,
                duration_ms,
            ),
            Err(err) => {
                self.record_trace_event(
                    &trace,
                    "request.end",
                    "error",
                    "rpc",
                    json!({
                        "method": method,
                        "request_id": trace.request_id,
                    }),
                    Some(err.to_string()),
                    duration_ms,
                );
                // Enhanced telemetry logging for errors
                telemetry_enhanced::log::error_with_context(
                    err,
                    "request_processing",
                    Some(&trace.request_id),
                );
            }
        }

        if let Some(span) = request_span {
            self.telemetry.end_span(
                span,
                vec![
                    KeyValue::new("request.duration_ms", duration_ms as i64),
                    KeyValue::new(
                        "request.status",
                        if result.is_ok() { "ok" } else { "error" },
                    ),
                ],
            );
        }

        // Enhanced telemetry logging for request completion
        let status_code = if result.is_ok() { 200 } else { 500 };
        telemetry_enhanced::log::request_complete(
            "rpc",
            &trace.method,
            &trace.request_id,
            status_code,
            duration_ms as f64,
        );

        result
    }

    fn new_request_trace(&self, request: &JsonRpcRequest) -> RequestTraceContext {
        let counter = TRACE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = format!(
            "{}:{}:{}:{}",
            request.method,
            request
                .id
                .as_ref()
                .map(value_to_id)
                .unwrap_or_else(|| "none".to_string()),
            now_ms(),
            counter
        );
        RequestTraceContext {
            trace_id: hash_hex(&base, 32),
            span_id: hash_hex(&format!("{}:span", base), 16),
            method: request.method.clone(),
            request_id: request
                .id
                .as_ref()
                .map(value_to_id)
                .unwrap_or_else(|| "none".to_string()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn record_trace_event(
        &self,
        trace: &RequestTraceContext,
        event_type: &str,
        status: &str,
        phase: &str,
        inputs: Value,
        error: Option<String>,
        duration_ms: u64,
    ) {
        let pua_stage = infer_pua_stage(event_type, phase);
        let attributes = normalize_trace_attributes(event_type, phase, status, inputs);
        let event = TraceEvent {
            timestamp: now_ms().to_string(),
            event_type: event_type.to_string(),
            task_id: trace.request_id.clone(),
            phase: phase.to_string(),
            agent: None,
            tool: None,
            status: status.to_string(),
            inputs: json!({
                "trace_id": trace.trace_id,
                "span_id": trace.span_id,
                "method": trace.method,
                "attributes": attributes,
            }),
            outputs: None,
            duration_ms,
            error,
            pua_stage,
        };

        if let Ok(mut guard) = self.trace_events.lock() {
            guard.push(event);
            if guard.len() > TRACE_BUFFER_MAX {
                let extra = guard.len() - TRACE_BUFFER_MAX;
                guard.drain(0..extra);
            }
        } else {
            warn!("failed to record trace event: trace_events lock poisoned");
        }
    }

    fn trace_snapshot(&self, limit: usize) -> Vec<TraceEvent> {
        self.trace_events
            .lock()
            .map(|guard| {
                guard
                    .iter()
                    .rev()
                    .take(limit.max(1))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    }

    fn trace_metrics_snapshot(&self) -> Value {
        let slow_top_n = self.runtime_config_snapshot().trace_slow_top_n.max(1);
        let events = self
            .trace_events
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default();

        let mut requests = events
            .iter()
            .filter(|e| e.event_type == "request.end")
            .map(|e| {
                let method = e
                    .inputs
                    .get("attributes")
                    .and_then(|v| v.get("method"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                json!({
                    "request_id": e.task_id,
                    "method": method,
                    "duration_ms": e.duration_ms,
                    "status": e.status,
                    "timestamp": e.timestamp,
                })
            })
            .collect::<Vec<_>>();

        requests.sort_by(|a, b| {
            b.get("duration_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(0)
                .cmp(&a.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(0))
        });
        requests.truncate(slow_top_n);

        let mut phase_buckets: HashMap<String, Vec<u64>> = HashMap::new();
        for event in &events {
            if event.duration_ms == 0 {
                continue;
            }
            if event.event_type.starts_with("phase.") || event.event_type == "request.end" {
                phase_buckets
                    .entry(event.phase.clone())
                    .or_default()
                    .push(event.duration_ms);
            }
        }

        let mut by_phase = serde_json::Map::new();
        for (phase, mut samples) in phase_buckets {
            samples.sort_unstable();
            let p95 = percentile(&samples, 95.0);
            let p99 = percentile(&samples, 99.0);
            by_phase.insert(
                phase,
                json!({
                    "count": samples.len(),
                    "p95_ms": p95,
                    "p99_ms": p99,
                }),
            );
        }

        let mut by_pua_stage: HashMap<String, u64> = HashMap::new();
        for event in &events {
            if let Some(stage) = event.pua_stage.as_ref() {
                *by_pua_stage.entry(stage.clone()).or_insert(0) += 1;
            }
        }

        json!({
            "sampling_rate": self.telemetry.sampling_rate(),
            "buffered_events": events.len(),
            "slow_requests_top_n": requests,
            "phase_latency": by_phase,
            "pua_stage_counts": by_pua_stage,
        })
    }

}
