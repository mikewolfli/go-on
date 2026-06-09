impl GoOnApp {
    /// Save the given tab's transient UI state into `self.ui_state`.
    fn save_tab_ui_state(&mut self, tab: &str) {
        match tab {
            "chat" => {
                self.views.chat_view.save_ui_state(&mut self.ui_state);
            }
            "monitor" => {
                self.ui_state.monitor_metrics_window =
                    self.views.monitor_view.metrics_window.clone();
                self.ui_state.monitor_auto_refresh_interval =
                    self.views.monitor_view.auto_refresh_interval;
                self.ui_state.monitor_provider_filter =
                    self.views.monitor_view.provider_filter.clone();
            }
            "providers" => {
                self.ui_state.providers_selected_provider =
                    self.views.providers_view.selected_provider.clone();
                self.ui_state.providers_new_model = self.views.providers_view.new_model.clone();
                self.ui_state.providers_new_label = self.views.providers_view.new_label.clone();
            }
            "skills" => {
                self.ui_state.skills_show_create = self.views.skills_view.show_create;
                self.ui_state.skills_show_import = self.views.skills_view.show_import;
                self.ui_state.skills_selected_skill_name =
                    self.views.skills_view.selected_skill_name.clone();
                self.ui_state.skills_edit_desc = self.views.skills_view.edit_desc.clone();
                self.ui_state.skills_edit_prompt = self.views.skills_view.edit_prompt.clone();
                self.ui_state.skills_edit_schema = self.views.skills_view.edit_schema.clone();
                self.ui_state.skills_test_input = self.views.skills_view.test_input.clone();
                self.ui_state.skills_rollback_version =
                    self.views.skills_view.rollback_version.clone();
                self.ui_state.skills_create_name = self.views.skills_view.create_name.clone();
                self.ui_state.skills_create_desc = self.views.skills_view.create_desc.clone();
                self.ui_state.skills_create_prompt = self.views.skills_view.create_prompt.clone();
                self.ui_state.skills_create_schema =
                    self.views.skills_view.create_input_schema.clone();
                self.ui_state.skills_import_url = self.views.skills_view.import_url.clone();
            }
            "workflow" => {
                self.ui_state.workflow_run_status_filter =
                    self.views.workflow_view.run_status_filter.clone();
                self.ui_state.workflow_selected_run_id =
                    self.views.workflow_view.selected_run_id.clone();
                self.ui_state.workflow_new_name = self.views.workflow_view.new_name.clone();
                self.ui_state.workflow_new_command = self.views.workflow_view.new_command.clone();
            }
            "config" => {
                self.ui_state.config_editor_draft = self.views.config_editor_view.draft.clone();
                self.ui_state.config_editor_search =
                    self.views.config_editor_view.search_query.clone();
                self.ui_state.config_editor_snapshots =
                    self.views.config_editor_view.snapshots.clone();
            }
            _ => {}
        }
    }

    /// Restore the given tab's transient UI state from `self.ui_state`.
    fn restore_tab_ui_state(&mut self, tab_name: &str) {
        match tab_name {
            "chat" => {
                // Only restore mode if saved value is valid — otherwise keep existing default
                let valid_modes = ["ask", "plan", "edit", "safeguard", "full_auto"];
                if !self.ui_state.selected_mode.is_empty()
                    && valid_modes.contains(&self.ui_state.selected_mode.as_str())
                {
                    self.views.chat_view.selected_mode = self.ui_state.selected_mode.clone();
                }
                self.views.chat_view.show_token_details = self.ui_state.show_token_details;
                self.views.chat_view.enable_markdown = self.ui_state.enable_markdown;
                self.views.chat_view.show_model_picker = self.ui_state.show_model_picker;
                self.views.chat_view.show_prompts = self.ui_state.show_prompts;
                if let Some(json) = &self.ui_state.model_stats_json {
                    if let Ok(stats) = serde_json::from_str(json) {
                        self.views.chat_view.model_stats = stats;
                    }
                }
                if self.ui_state.active_session < self.views.chat_view.sessions.len() {
                    self.views.chat_view.active_session = self.ui_state.active_session;
                }
                self.views.chat_view.input = self.ui_state.input_draft.clone();
                self.views.chat_view.session_search_query =
                    self.ui_state.session_search_query.clone();
                self.views.chat_view.template_search_query =
                    self.ui_state.template_search_query.clone();
            }
            "monitor" => {
                self.views.monitor_view.metrics_window =
                    self.ui_state.monitor_metrics_window.clone();
                if self.ui_state.monitor_auto_refresh_interval > 0 {
                    self.views.monitor_view.auto_refresh_interval =
                        self.ui_state.monitor_auto_refresh_interval;
                }
                self.views.monitor_view.provider_filter =
                    self.ui_state.monitor_provider_filter.clone();
            }
            "providers" => {
                self.views.providers_view.selected_provider =
                    self.ui_state.providers_selected_provider.clone();
                if !self.ui_state.providers_new_model.is_empty() {
                    self.views.providers_view.new_model = self.ui_state.providers_new_model.clone();
                }
                self.views.providers_view.new_label = self.ui_state.providers_new_label.clone();
            }
            "skills" => {
                self.views.skills_view.show_create = self.ui_state.skills_show_create;
                self.views.skills_view.show_import = self.ui_state.skills_show_import;
                if !self.ui_state.skills_selected_skill_name.is_empty() {
                    self.views
                        .skills_view
                        .load_skill_editor_by_name(&self.ui_state.skills_selected_skill_name);
                }
                self.views.skills_view.edit_desc = self.ui_state.skills_edit_desc.clone();
                self.views.skills_view.edit_prompt = self.ui_state.skills_edit_prompt.clone();
                self.views.skills_view.edit_schema = self.ui_state.skills_edit_schema.clone();
                self.views.skills_view.test_input = self.ui_state.skills_test_input.clone();
                self.views.skills_view.rollback_version =
                    self.ui_state.skills_rollback_version.clone();
                self.views.skills_view.create_name = self.ui_state.skills_create_name.clone();
                self.views.skills_view.create_desc = self.ui_state.skills_create_desc.clone();
                self.views.skills_view.create_prompt = self.ui_state.skills_create_prompt.clone();
                self.views.skills_view.create_input_schema =
                    self.ui_state.skills_create_schema.clone();
                self.views.skills_view.import_url = self.ui_state.skills_import_url.clone();
            }
            "workflow" => {
                self.views.workflow_view.run_status_filter =
                    self.ui_state.workflow_run_status_filter.clone();
                self.views.workflow_view.selected_run_id =
                    self.ui_state.workflow_selected_run_id.clone();
                self.views.workflow_view.new_name = self.ui_state.workflow_new_name.clone();
                self.views.workflow_view.new_command = self.ui_state.workflow_new_command.clone();
            }
            "config" => {
                self.views.config_editor_view.draft = self.ui_state.config_editor_draft.clone();
                self.views.config_editor_view.search_query =
                    self.ui_state.config_editor_search.clone();
                self.views.config_editor_view.snapshots =
                    self.ui_state.config_editor_snapshots.clone();
            }
            _ => {}
        }
    }
}

impl GoOnApp {
    fn active_tabs_precomputed(&self) -> Vec<String> {
        let features = &self.config_store.shared().features;
        let mut tabs = Vec::new();
        if features.monitor {
            tabs.push("monitor".into());
        }
        if features.chat {
            tabs.push("chat".into());
        }
        if features.skills {
            tabs.push("skills".into());
        }
        if features.workflow {
            tabs.push("workflow".into());
        }
        if features.autotune {
            tabs.push("autotune".into());
        }
        if features.show_prompts_tab {
            tabs.push("prompts".into());
        }
        if features.chat && features.show_risk_decision_tab {
            tabs.push("risk_decision".into());
        }
        if features.security {
            tabs.push("security".into());
        }
        if features.config {
            tabs.push("config".into());
        }
        if features.providers {
            tabs.push("providers".into());
        }
        tabs.push("about".into());
        tabs.push("settings".into());
        tabs
    }

    fn tab_label(&self, tab: &str) -> String {
        match tab {
            "monitor" => self.i18n.t("tab.monitor"),
            "chat" => self.i18n.t("tab.chat"),
            "skills" => self.i18n.t("tab.skills"),
            "workflow" => self.i18n.t("tab.workflow"),
            "autotune" => self.i18n.t("tab.autotune"),
            "prompts" => self.i18n.t("tab.prompts"),
            "risk_decision" => self.i18n.t("tab.riskDecision"),
            "security" => self.i18n.t("tab.security"),
            "config" => self.i18n.t("tab.config"),
            "providers" => self.i18n.t("tab.providers"),
            "about" => self.i18n.t("tab.about"),
            "settings" => self.i18n.t("tab.settings"),
            _ => std::borrow::Cow::Borrowed(tab),
        }
        .to_string()
    }

    /// Compute a hash of all state that affects the UI rendering.
    /// Used by the double-buffering render gate to skip frames when
    /// nothing visible has changed.
    fn compute_render_hash(&self) -> u64 {
        use std::hash::Hash;
        let mut hasher = DefaultHasher::new();
        // Config changes (theme, language, features, etc.)
        self.config_store
            .config_shared_fingerprint
            .hash(&mut hasher);
        // Tab selection
        self.active_tab.hash(&mut hasher);
        // Setup screen visibility
        self.show_setup.hash(&mut hasher);
        // Backend connection state
        let is_connected = self
            .views
            .monitor_view
            .health
            .as_ref()
            .is_some_and(|h| h.connected);
        is_connected.hash(&mut hasher);
        // Provider availability
        self.has_providers.hash(&mut hasher);
        // Backend loading spinner
        self.connection.pending_refresh.hash(&mut hasher);
        // Crash badge visibility
        self.crash.backend_crash_count.hash(&mut hasher);
        // Toast visibility
        let toast_visible = self
            .blocked_tab_toast_shown
            .is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(5));
        toast_visible.hash(&mut hasher);
        // Stale models warning
        self.connection.backend.stale_models().hash(&mut hasher);
        // Last applied theme (skips full re-layout when unchanged)
        self.last_applied_theme.hash(&mut hasher);
        hasher.finish()
    }
}
