impl AcpServer {
    pub fn new(
        flow: Arc<FlowManager>,
        registry: Arc<AgentRegistry>,
        cache: Option<Arc<ResponseCache>>,
        vector_store: Option<Arc<VectorStore>>,
        vector_config: Option<VectorConfig>,
        autotune: Option<Arc<Mutex<AutoTuneState>>>,
        autotune_config: Option<AutoTuneConfig>,
        autotune_state_path: Option<String>,
        runtime_config: RuntimeConfig,
        config_path: Option<PathBuf>,
        forced_phase: Option<String>,
        http_client: Option<reqwest::Client>,
        verbose: bool,
    ) -> Self {
        let telemetry = Arc::new(TelemetryRuntime::new(&runtime_config));
        Self {
            flow: Arc::new(StdMutex::new(flow)),
            registry: Arc::new(StdMutex::new(registry)),
            cache: Arc::new(StdMutex::new(cache)),
            vector_store: Arc::new(StdMutex::new(vector_store)),
            vector_config: Arc::new(StdMutex::new(vector_config)),
            autotune: Arc::new(StdMutex::new(autotune)),
            autotune_config: Arc::new(StdMutex::new(autotune_config)),
            autotune_state_path: Arc::new(StdMutex::new(autotune_state_path)),
            runtime_config: Arc::new(StdMutex::new(runtime_config)),
            metrics: Arc::new(RuntimeMetrics::default()),
            online_controller: Arc::new(StdMutex::new(OnlineControllerState::default())),
            telemetry,
            trace_events: Arc::new(StdMutex::new(Vec::new())),
            memory_cache: Arc::new(MemoryResponseCache::default()),
            conversation_store: Arc::new(StdMutex::new(HashMap::new())),
            conversation_touch_order: Arc::new(StdMutex::new(Vec::new())),
            maintenance: Arc::new(MaintenanceTracker::default()),
            lifecycle: Arc::new(LifecycleState::default()),
            circuit_breakers: Arc::new(CircuitBreakerRegistry::default()),
            adaptive_model_selector: Arc::new(StdMutex::new(AdaptiveModelSelector::new())),
            phase_rate_limiter: Arc::new(PhaseRateLimiter::default()),
            inflight_limiter: Arc::new(InflightLimiter::default()),
            config_path,
            forced_phase,
            http_client,
            verbose,
            output: Arc::new(Mutex::new(tokio::io::stdout())),
            shutdown_notify: Arc::new(Notify::new()),
        }
    }

    /// Run the ACP server
    ///
    /// This method starts the server, handles incoming requests from stdin,
    /// and manages the server lifecycle.
    ///
    /// # Returns
    /// * `Result<()>` - Returns Ok(()) on successful shutdown, or an error if something goes wrong
    pub async fn run(&mut self) -> Result<()> {
        // Spawn background maintenance loop
        let background_task = self.spawn_background_maintenance_loop();
        let stdin = tokio::io::stdin();
        let mut reader = BufReader::new(stdin).lines();

        // Process incoming requests from stdin
        while let Some(line) = reader.next_line().await? {
            if self.lifecycle.is_shutting_down() {
                break;
            }

            if line.trim().is_empty() {
                continue;
            }

            // Parse JSON-RPC request
            let request: JsonRpcRequest = match serde_json::from_str(&line) {
                Ok(req) => req,
                Err(err) => {
                    self.send_error(
                        None,
                        -32700,
                        crate::i18n::tf("error.parse_error", &[("error", &format!("{err}"))]),
                        None,
                    )
                    .await?;
                    continue;
                }
            };

            // Validate JSON-RPC version
            if request.jsonrpc != "2.0" {
                self.send_error(
                    request.id,
                    -32600,
                    ProxyError::InvalidRequest("jsonrpc must be 2.0".to_string()).to_string(),
                    None,
                )
                .await?;
                continue;
            }

            let method = request.method.clone();
            if self.verbose {
                debug!("incoming method: {method}");
            }

            // Handle request in a separate task to avoid blocking the main loop
            let id_for_response = request.id.clone();
            let handle = tokio::spawn(async move { request });
            let request = match handle.await {
                Ok(req) => req,
                Err(join_err) => {
                    self.send_error(
                        id_for_response,
                        -32603,
                        crate::i18n::tf(
                            "error.request_handling_panic",
                            &[("error", &format!("{join_err}"))],
                        ),
                        None,
                    )
                    .await?;
                    continue;
                }
            };

            // Process the request
            let response = self.handle_request(request).await;
            if let Err(err) = response {
                error!(method = %method, "request failed: {err:#}");
            }

            // Check if shutdown is requested
            if method == "shutdown" || self.lifecycle.is_shutting_down() {
                info!("{}", crate::i18n::t("info.shutdown_requested"));
                break;
            }
        }

        // Shutdown sequence
        self.begin_shutdown(&crate::i18n::t("info.shutdown_sequence"));
        self.wait_for_inflight_drain().await;
        self.shutdown_notify.notify_waiters();

        // Wait for background task to complete
        if let Err(err) = background_task.await {
            warn!("background maintenance task exited unexpectedly: {}", err);
        }

        Ok(())
    }

    fn routing_handles(&self) -> Result<(Arc<FlowManager>, Arc<AgentRegistry>)> {
        let flow_guard = self.flow.lock().map_err(|_| {
            anyhow::anyhow!(
                "{}",
                crate::i18n::tf("error.mutex_poisoned", &[("name", "flow")])
            )
        })?;
        let registry_guard = self.registry.lock().map_err(|_| {
            anyhow::anyhow!(
                "{}",
                crate::i18n::tf("error.mutex_poisoned", &[("name", "registry")])
            )
        })?;
        Ok((flow_guard.clone(), registry_guard.clone()))
    }

    fn cache_handle(&self) -> Option<Arc<ResponseCache>> {
        self.cache.lock().ok().and_then(|guard| guard.clone())
    }

    fn artifact_ledger(&self) -> ArtifactLedger {
        ArtifactLedger::new(self.config_path.as_deref())
    }

    fn vector_store_handle(&self) -> Option<Arc<VectorStore>> {
        self.vector_store
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
    }

    fn vector_config_snapshot(&self) -> Option<VectorConfig> {
        self.vector_config
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
    }

    fn autotune_handle(&self) -> Option<Arc<Mutex<AutoTuneState>>> {
        self.autotune.lock().ok().and_then(|guard| guard.clone())
    }

    fn autotune_config_snapshot(&self) -> Option<AutoTuneConfig> {
        self.autotune_config
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
    }

    fn autotune_state_path_snapshot(&self) -> Option<String> {
        self.autotune_state_path
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
    }

    fn runtime_config_snapshot(&self) -> RuntimeConfig {
        self.runtime_config
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    fn runtime_healthcheck_report(&self) -> Result<crate::reinforcement::RuntimeHealthcheckReport> {
        let cache = self.cache_handle();
        let vector_store = self.vector_store_handle();
        let mut report = build_runtime_healthcheck_report(
            self.config_path.as_deref(),
            cache.as_deref(),
            vector_store.as_deref(),
        )?;

        let (global_inflight, phase_inflight) = self.inflight_limiter.snapshot();
        let runtime_status =
            if self.lifecycle.is_shutting_down() || self.circuit_breakers.open_count() > 0 {
                CheckStatus::Warn
            } else {
                CheckStatus::Healthy
            };

        report.components.push(ComponentReport {
            name: "runtime".to_string(),
            status: runtime_status,
            message: crate::i18n::t("info.runtime_controller_snapshot"),
            details: json!({
                "memory_cache_entries": self.memory_cache.active_entries(),
                "lazy_load_cache": lazy_load_cache_snapshot(),
                "circuit_breaker": {
                    "open_agents": self.circuit_breakers.open_count(),
                    "half_open_agents": self.circuit_breakers.half_open_count(),
                    "tracked_agents": self.circuit_breakers.tracked_agents(),
                    "agents": self.circuit_breakers.snapshot(),
                },
                "rate_limiter": {
                    "tracked_phases": self.phase_rate_limiter.tracked_phases(),
                },
                "inflight": {
                    "global": global_inflight,
                    "per_phase": phase_inflight,
                },
                "lifecycle": self.lifecycle.snapshot(),
                "maintenance": self.maintenance.snapshot(),
                "review_gate": {
                    "total": self.metrics.snapshot().review_gate_total,
                    "approved": self.metrics.snapshot().review_gate_approved_total,
                    "rejected": self.metrics.snapshot().review_gate_rejected_total,
                    "timeout": self.metrics.snapshot().review_gate_timeout_total,
                    "degraded": self.metrics.snapshot().review_gate_degraded_total,
                    "invalid_response": self.metrics.snapshot().review_gate_invalid_response_total,
                },
                "telemetry": {
                    "enabled": self.telemetry.is_enabled(),
                    "sampling_rate": self.telemetry.sampling_rate(),
                },
            }),
        });

        report.overall_status =
            aggregate_status(report.components.iter().map(|component| component.status));
        Ok(report)
    }

    fn persist_checkpoint_summary(&self, checkpoint: &ConversationCheckpoint) {
        let summary = CheckpointSummaryArtifact {
            checkpoint_id: checkpoint.checkpoint_id.clone(),
            conversation_id: checkpoint.conversation_id.clone(),
            branch_id: checkpoint.branch_id.clone(),
            parent_checkpoint_id: checkpoint.parent_checkpoint_id.clone(),
            created_at: checkpoint.created_at,
            note: checkpoint.note.clone(),
            message_count: checkpoint.messages.len(),
            message_chars: total_message_chars(&checkpoint.messages),
            assistant_excerpt: assistant_excerpt(&checkpoint.messages),
        };

        if let Err(err) = self
            .artifact_ledger()
            .write_json("checkpoints", "latest.json", &summary)
        {
            warn!(
                "{}",
                crate::i18n::tf(
                    "warning.failed_persist_checkpoint",
                    &[("error", &format!("{}", err))]
                )
            );
        }
    }

    fn begin_shutdown(&self, reason: &str) {
        if self.lifecycle.start_shutdown(reason) {
            self.shutdown_notify.notify_waiters();
        }
    }

    async fn wait_for_inflight_drain(&self) {
        let timeout_seconds = self.runtime_config_snapshot().shutdown_drain_seconds.max(1);
        let deadline = Instant::now() + Duration::from_secs(timeout_seconds);

        loop {
            let (global_inflight, _) = self.inflight_limiter.snapshot();
            if global_inflight == 0 {
                return;
            }

            if Instant::now() >= deadline {
                warn!(
                    "shutdown drain timeout reached with {} in-flight request(s) still tracked",
                    global_inflight
                );
                return;
            }

            sleep(Duration::from_millis(100)).await;
        }
    }

    async fn run_maintenance_cycle(&self, source: &str) -> MaintenanceCycleResult {
        match perform_maintenance_cycle(
            Arc::clone(&self.memory_cache),
            Arc::clone(&self.cache),
            Arc::clone(&self.vector_store),
            Arc::clone(&self.runtime_config),
            Arc::clone(&self.maintenance),
            source,
        )
        .await
        {
            Ok(result) => result,
            Err(err) => {
                warn!("maintenance cycle '{}' failed: {}", source, err);
                MaintenanceCycleResult::default()
            }
        }
    }

    fn spawn_background_maintenance_loop(&self) -> JoinHandle<()> {
        let runtime_config = Arc::clone(&self.runtime_config);
        let memory_cache = Arc::clone(&self.memory_cache);
        let cache = Arc::clone(&self.cache);
        let vector_store = Arc::clone(&self.vector_store);
        let maintenance = Arc::clone(&self.maintenance);
        let lifecycle = Arc::clone(&self.lifecycle);
        let circuit_breakers = Arc::clone(&self.circuit_breakers);
        let phase_rate_limiter = Arc::clone(&self.phase_rate_limiter);
        let inflight_limiter = Arc::clone(&self.inflight_limiter);
        let shutdown_notify = Arc::clone(&self.shutdown_notify);

        tokio::spawn(async move {
            run_background_maintenance_loop(
                runtime_config,
                memory_cache,
                cache,
                vector_store,
                maintenance,
                lifecycle,
                circuit_breakers,
                phase_rate_limiter,
                inflight_limiter,
                shutdown_notify,
            )
            .await;
        })
    }

}
