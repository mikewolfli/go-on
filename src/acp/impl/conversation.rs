impl AcpServer {
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

}
