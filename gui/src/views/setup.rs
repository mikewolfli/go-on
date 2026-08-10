use crate::backend::BackendClient;
use crate::config::{save_app_config, AppConfig, ProviderConfig};
use crate::i18n::I18n;
use crate::views::providers::{models_for_provider, provider_requires_secret};
use serde_json::Value;
use std::sync::mpsc;
use std::time::Instant;

fn provider_label(i18n: &I18n, provider: &str) -> String {
    let key = format!("provider.{}", provider.to_lowercase());
    let label = i18n.t(&key);
    if label.as_ref() == key {
        provider.to_string()
    } else {
        label.into_owned()
    }
}

pub struct SetupView {
    selected_provider: String,
    api_key: String,
    new_secret_key: String,
    new_label: String,
    selected_model: String,
    error_msg: String,
    success_msg: String,
    remote_models: std::collections::HashMap<String, Vec<String>>,
    models_loaded: bool,
    provider_names: Vec<String>,
    catalog_loaded: bool,
    pending_rx: mpsc::Receiver<String>,
    pending_tx: mpsc::SyncSender<String>,

    // ── GitHub Copilot OAuth Device Code state ──
    copilot_device_state: Option<String>,
    copilot_device_code: String,
    copilot_user_code: String,
    copilot_verification_uri: String,
    copilot_poll_interval: u64,
    copilot_last_poll: Instant,
    copilot_poll_attempts: u64,
    copilot_slow_down_count: u64,
    copilot_last_poll_result: String,
    copilot_access_token: String,
    copilot_token_stored: bool,
    copilot_status: String,
    copilot_poll_repaint_requested: bool,
    sending: bool,
}

impl SetupView {
    pub fn new() -> Self {
        let (pending_tx, pending_rx) = mpsc::sync_channel(256);
        Self {
            selected_provider: "openai".to_string(),
            api_key: String::new(),
            new_secret_key: String::new(),
            new_label: String::new(),
            selected_model: "auto".to_string(),
            error_msg: String::new(),
            success_msg: String::new(),
            remote_models: std::collections::HashMap::new(),
            models_loaded: false,
            provider_names: crate::views::providers::PROVIDER_NAMES
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
            catalog_loaded: false,
            pending_rx,
            pending_tx,

            // Copilot Device Code state
            copilot_device_state: None,
            copilot_device_code: String::new(),
            copilot_user_code: String::new(),
            copilot_verification_uri: String::new(),
            copilot_poll_interval: 5,
            copilot_last_poll: Instant::now(),
            copilot_poll_attempts: 0,
            copilot_slow_down_count: 0,
            copilot_last_poll_result: String::new(),
            copilot_access_token: String::new(),
            copilot_token_stored: false,
            copilot_status: String::new(),
            copilot_poll_repaint_requested: false,
            sending: false,
        }
    }

    fn process_pending(&mut self, i18n: &I18n, config: &mut AppConfig) {
        const MAX_EVENTS_PER_FRAME: usize = 12;
        for _ in 0..MAX_EVENTS_PER_FRAME {
            let Ok(msg) = self.pending_rx.try_recv() else {
                break;
            };
            if let Some(models_json) = msg.strip_prefix("__models__:") {
                if let Ok(models) = serde_json::from_str::<
                    std::collections::HashMap<String, Vec<String>>,
                >(models_json)
                {
                    self.remote_models = models;
                }
            } else if let Some(catalog_json) = msg.strip_prefix("__catalog__:") {
                if let Ok(value) = serde_json::from_str::<Value>(catalog_json) {
                    if let Some(items) = value.get("catalog").and_then(Value::as_array) {
                        let mut names = items
                            .iter()
                            .filter_map(|item| item.get("name").and_then(Value::as_str))
                            .map(ToString::to_string)
                            .collect::<Vec<_>>();
                        names.sort();
                        names.dedup();
                        if !names.is_empty() {
                            self.provider_names = names;
                            if !self
                                .provider_names
                                .iter()
                                .any(|name| name.eq_ignore_ascii_case(&self.selected_provider))
                            {
                                self.selected_provider = self.provider_names[0].clone();
                            }
                        }
                    }
                }
            } else if let Some(rest) = msg.strip_prefix("__copilot_device__:") {
                // Initial device code response
                if let Ok(resp) = serde_json::from_str::<serde_json::Value>(rest) {
                    self.copilot_device_code =
                        resp["device_code"].as_str().unwrap_or("").to_string();
                    self.copilot_user_code = resp["user_code"].as_str().unwrap_or("").to_string();
                    self.copilot_verification_uri = resp["verification_uri"]
                        .as_str()
                        .unwrap_or("https://github.com/login/device")
                        .to_string();
                    self.copilot_poll_interval = resp["interval"].as_u64().unwrap_or(5).max(5);
                    self.copilot_status = String::new();
                    #[cfg(debug_assertions)]
                    eprintln!(
                        "[device] user_code={}, uri={}, interval={}",
                        self.copilot_user_code,
                        self.copilot_verification_uri,
                        self.copilot_poll_interval
                    );
                    self.copilot_device_state = Some("polling".to_string());
                    self.copilot_last_poll = Instant::now();
                    self.copilot_poll_repaint_requested = false;
                    self.copilot_poll_attempts = 0;
                    self.copilot_slow_down_count = 0;
                    self.copilot_last_poll_result.clear();
                    self.copilot_status = String::new();
                } else {
                    self.copilot_device_state = Some("error".to_string());
                    self.copilot_status = "Failed to parse device code response".to_string();
                }
            } else if let Some(err_msg) = msg.strip_prefix("__copilot_device_err__:") {
                self.copilot_device_state = Some("error".to_string());
                self.copilot_status = err_msg.to_string();
            } else if let Some(rest) = msg.strip_prefix("__copilot_poll__:") {
                // Poll response — GitHub returns access_token on success, or error field on failure.
                if let Ok(resp) = serde_json::from_str::<serde_json::Value>(rest) {
                    // Check for access_token first (success case)
                    if let Some(token) = resp.get("access_token").and_then(Value::as_str) {
                        if !token.is_empty() {
                            self.copilot_last_poll_result =
                                "authorized(access_token received)".to_string();
                            self.copilot_access_token = token.to_string();
                            self.copilot_device_state = Some("done".to_string());
                            self.copilot_status =
                                i18n.t("providers.copilot_authorized").to_string();
                            self.api_key = self.copilot_access_token.clone();
                            self.copilot_token_stored = true;
                            // Immediately persist to keyring so the token survives app restart
                            if let Err(e) = crate::keyring_util::store_api_key("copilot", token) {
                                eprintln!("Warning: failed to store Copilot token in keyring (copilot_api_key): {e}");
                            }
                            if let Err(e) = crate::keyring_util::store_copilot_token(token) {
                                eprintln!("Warning: failed to store Copilot token in keyring (github_copilot_token): {e}");
                            }
                            // Auto-create a Copilot provider entry in config so the user
                            // doesn't need to manually click Save after OAuth completes.
                            if !config
                                .providers
                                .iter()
                                .any(|p| p.name.eq_ignore_ascii_case("copilot"))
                            {
                                config.providers.push(ProviderConfig {
                                    name: "copilot".to_string(),
                                    api_key: token.to_string(),
                                    secret_key: String::new(),
                                    model: "auto".to_string(),
                                    validated: true,
                                    label: String::new(),
                                });
                            }
                        }
                    } else if let Some(error) = resp.get("error").and_then(Value::as_str) {
                        self.copilot_last_poll_result = format!("oauth_error={}", error);
                        match error {
                            "authorization_pending" => {
                                self.copilot_status =
                                    i18n.t("providers.copilot_waiting").to_string();
                            }
                            "slow_down" => {
                                self.copilot_slow_down_count =
                                    self.copilot_slow_down_count.saturating_add(1);
                                self.copilot_poll_interval =
                                    self.copilot_poll_interval.saturating_add(5).min(60);
                                self.copilot_status = format!(
                                    "{} (backoff to {} {})",
                                    i18n.t("providers.copilot_waiting"),
                                    self.copilot_poll_interval,
                                    i18n.t("common.seconds")
                                );
                            }
                            "expired_token" => {
                                self.copilot_device_state = Some("error".to_string());
                                self.copilot_status =
                                    i18n.t("providers.copilot_expired").to_string();
                            }
                            "access_denied" => {
                                self.copilot_device_state = Some("error".to_string());
                                self.copilot_status =
                                    i18n.t("providers.copilot_denied").to_string();
                            }
                            _ => {
                                self.copilot_device_state = Some("error".to_string());
                                self.copilot_status =
                                    format!("{} {}", i18n.t("common.error"), error);
                            }
                        }
                    }
                } else {
                    self.copilot_last_poll_result = "parse_error(response json)".to_string();
                    self.copilot_device_state = Some("error".to_string());
                    self.copilot_status = "Failed to parse poll response".to_string();
                }
            } else if let Some(err_msg) = msg.strip_prefix("__copilot_poll_err__:") {
                self.copilot_last_poll_result = format!("request_error={}", err_msg);
                self.copilot_device_state = Some("error".to_string());
                self.copilot_status = err_msg.to_string();
            } else {
                self.sending = false;
                if !msg.is_empty() {
                    self.error_msg = msg;
                }
            }
        }
    }

    fn ensure_models_loaded(&mut self, backend: &BackendClient, ctx: &egui::Context) {
        if !self.models_loaded {
            self.models_loaded = true;
            let backend_clone = backend.clone();
            let tx = self.pending_tx.clone();
            let ctx_clone = ctx.clone();
            tokio::spawn(async move {
                let models: std::collections::HashMap<String, Vec<String>> = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    backend_clone.fetch_models(),
                )
                .await
                .unwrap_or_else(|_| {
                    #[cfg(debug_assertions)]
                    eprintln!("Warning: Failed to fetch models from backend (timeout)");
                    std::collections::HashMap::new()
                });
                let msg = format!(
                    "__models__:{}",
                    serde_json::to_string(&models).unwrap_or_default()
                );
                let _ = tx.try_send(msg);
                ctx_clone.request_repaint();
            });
        }

        if !self.catalog_loaded {
            self.catalog_loaded = true;
            let backend_clone = backend.clone();
            let tx = self.pending_tx.clone();
            let ctx_clone = ctx.clone();
            tokio::spawn(async move {
                let catalog = match tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    backend_clone.provider_catalog(),
                )
                .await
                {
                    Ok(Ok(value)) => value,
                    _ => Value::Null,
                };
                let msg = format!(
                    "__catalog__:{}",
                    serde_json::to_string(&catalog).unwrap_or_default()
                );
                let _ = tx.try_send(msg);
                ctx_clone.request_repaint();
            });
        }
    }

    fn backend_models_for_provider(&self, provider: &str) -> Option<Vec<String>> {
        let key = provider.to_lowercase();
        self.remote_models.iter().find_map(|(name, models)| {
            if name.eq_ignore_ascii_case(&key) || name.eq_ignore_ascii_case(provider) {
                Some(models.clone())
            } else {
                None
            }
        })
    }

    fn available_models_for_selected_provider(&self) -> Vec<String> {
        let mut models = Vec::<String>::new();
        models.push("auto".to_string());

        if let Some(remote) = self.backend_models_for_provider(&self.selected_provider) {
            for model in remote {
                if !model.trim().is_empty() && model != "auto" {
                    models.push(model);
                }
            }
        }

        for fallback in models_for_provider(&self.selected_provider) {
            if *fallback != "auto" {
                models.push((*fallback).to_string());
            }
        }

        let mut deduped = Vec::<String>::new();
        let mut seen = std::collections::HashSet::<String>::new();
        for model in models {
            if seen.insert(model.clone()) {
                deduped.push(model);
            }
        }
        deduped
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        i18n: &I18n,
        config: &mut AppConfig,
        backend: &BackendClient,
    ) -> bool {
        let mut done = false;
        let ctx = ui.ctx().clone();

        self.ensure_models_loaded(backend, &ctx);
        self.process_pending(i18n, config);

        egui::CentralPanel::default().show(ui, |ui: &mut egui::Ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(60.0);
                ui.heading(i18n.t("setup.title"));
                ui.add_space(10.0);
                ui.label(i18n.t("setup.hint"));
                ui.add_space(20.0);

                if !self.success_msg.is_empty() {
                    ui.colored_label(egui::Color32::from_rgb(20, 120, 70), &self.success_msg);
                    ui.add_space(10.0);
                    if ui.button(i18n.t("app.start")).clicked() {
                        done = true;
                    }
                    return;
                }

                // ── Provider selection ──
                ui.horizontal(|ui| {
                    ui.label(i18n.t("setup.provider"));
                    let provider_options = self.provider_names.clone();
                    egui::ComboBox::from_id_salt("provider_sel")
                        .selected_text(provider_label(i18n, &self.selected_provider))
                        .show_ui(ui, |ui| {
                            for p in &provider_options {
                                if ui
                                    .selectable_value(
                                        &mut self.selected_provider,
                                        p.to_string(),
                                        provider_label(i18n, p),
                                    )
                                    .clicked()
                                {
                                    self.api_key.clear();
                                    self.new_secret_key.clear();
                                    self.selected_model = "auto".to_string();
                                    self.copilot_token_stored = false;
                                    self.copilot_device_state = None;
                                    self.copilot_status.clear();
                                }
                            }
                        });
                });
                ui.add_space(8.0);

                // ── API Key ──
                ui.horizontal(|ui| {
                    ui.label(i18n.t("setup.apiKey"));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.api_key)
                            .password(true)
                            .hint_text(i18n.t("common.apiKeyPlaceholder"))
                            .desired_width(300.0),
                    );
                });
                ui.add_space(8.0);

                // ── Secret Key field (dual-auth providers: wenxin, qianfan) ──
                if provider_requires_secret(&self.selected_provider.to_lowercase()) {
                    ui.horizontal(|ui| {
                        ui.label(i18n.t("providers.secret_key"));
                        ui.add(
                            egui::TextEdit::singleline(&mut self.new_secret_key)
                                .password(true)
                                .hint_text(i18n.t("providers.secret_key_placeholder"))
                                .desired_width(300.0),
                        );
                    });
                    ui.add_space(8.0);
                }

                // ── Label field ──
                ui.horizontal(|ui| {
                    ui.label(i18n.t("providers.label"));
                    let existing_same = config
                        .providers
                        .iter()
                        .filter(|p| p.name == self.selected_provider)
                        .count();
                    if existing_same > 0 {
                        ui.colored_label(
                            egui::Color32::from_rgb(220, 160, 50),
                            i18n.t("providers.labelRequiredHint"),
                        );
                    }
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_label)
                            .hint_text(if existing_same > 0 {
                                i18n.t("providers.labelPlaceholderRequired")
                            } else {
                                i18n.t("providers.labelPlaceholder")
                            })
                            .desired_width(120.0),
                    );
                });
                ui.add_space(8.0);

                // ── Model selection ──
                ui.horizontal(|ui| {
                    ui.label(i18n.t("setup.model"));
                    egui::ComboBox::from_id_salt("model_sel")
                        .selected_text({
                            if self.selected_model == "auto" {
                                i18n.t("providers.auto").to_string()
                            } else {
                                format!(
                                    "{}: {}",
                                    provider_label(i18n, &self.selected_provider),
                                    self.selected_model
                                )
                            }
                        })
                        .show_ui(ui, |ui| {
                            // Show hint for copilot
                            if self.selected_provider.to_lowercase() == "copilot" {
                                ui.label(i18n.t("providers.copilot_hint"));
                            }
                            let models = self.available_models_for_selected_provider();
                            for m in models {
                                let display_name = if m == "auto" {
                                    i18n.t("providers.auto").to_string()
                                } else {
                                    format!(
                                        "{}: {}",
                                        provider_label(i18n, &self.selected_provider),
                                        m
                                    )
                                };
                                ui.selectable_value(
                                    &mut self.selected_model,
                                    m,
                                    display_name,
                                );
                            }
                        });
                });

                // ── Copilot Device Code authorization ──
                if self.selected_provider.to_lowercase() == "copilot" {
                    ui.add_space(4.0);
                    ui.separator();
                    ui.add_space(2.0);

                    if self.copilot_device_state.is_none()
                        && ui.button(i18n.t("providers.copilot_authorize")).clicked()
                    {
                        self.copilot_device_state = Some("requesting".to_string());
                        self.copilot_status =
                            i18n.t("providers.copilot_requesting").to_string();
                        let backend_clone = backend.clone();
                        let tx = self.pending_tx.clone();
                        let ctx_clone = ctx.clone();
                        tokio::spawn(async move {
                            match tokio::time::timeout(
                                std::time::Duration::from_secs(15),
                                backend_clone.copilot_device_code(),
                            )
                            .await
                            {
                                Ok(Ok(body)) => {
                                    let msg = format!(
                                        "__copilot_device__:{}",
                                        serde_json::to_string(&body)
                                            .unwrap_or_default()
                                    );
                                    if let Err(e) = tx.try_send(msg) {
                                        eprintln!(
                                            "WARN: setup try_send failed: {:?}",
                                            e
                                        );
                                    }
                                }
                                Ok(Err(e)) => {
                                    let msg =
                                        format!("__copilot_device_err__:{}", e);
                                    if let Err(e) = tx.try_send(msg) {
                                        eprintln!(
                                            "WARN: setup try_send failed: {:?}",
                                            e
                                        );
                                    }
                                }
                                Err(_) => {
                                    let msg = "__copilot_device_err__:Request timed out."
                                        .to_string();
                                    if let Err(e) = tx.try_send(msg) {
                                        eprintln!(
                                            "WARN: setup try_send failed: {:?}",
                                            e
                                        );
                                    }
                                }
                            }
                            ctx_clone.request_repaint();
                        });
                    }

                    // ── Copilot device auth modal ──
                    if let Some(state) = self.copilot_device_state.clone() {
                        let mut open = true;
                        egui::Window::new(i18n.t("providers.copilot_authorize"))
                            .id(egui::Id::new("setup_copilot_device_auth"))
                            .open(&mut open)
                            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                            .resizable(false)
                            .collapsible(false)
                            .show(ui.ctx(), |ui| {
                                match state.as_str() {
                                    "requesting" => {
                                        ui.horizontal(|ui| {
                                            ui.spinner();
                                            ui.label(
                                                i18n.t("providers.copilot_requesting"),
                                            );
                                        });
                                    }
                                    "polling" => {
                                        ui.vertical(|ui| {
                                            ui.heading(
                                                i18n.t("providers.copilot_authorize"),
                                            );
                                            ui.add_space(8.0);
                                            ui.label(
                                                i18n.t("providers.copilot_open_url"),
                                            );
                                            if ui
                                                .link(&self.copilot_verification_uri)
                                                .clicked()
                                            {
                                                let _ = webbrowser::open(
                                                    &self.copilot_verification_uri,
                                                );
                                            }
                                            ui.add_space(4.0);
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    i18n.t(
                                                        "providers.copilot_enter_code",
                                                    ),
                                                );
                                                ui.add(
                                                    egui::Label::new(
                                                        egui::RichText::new(
                                                            &self.copilot_user_code,
                                                        )
                                                        .size(28.0)
                                                        .color(egui::Color32::from_rgb(
                                                            60, 180, 100,
                                                        ))
                                                        .monospace(),
                                                    ),
                                                );
                                            });
                                            ui.add_space(8.0);
                                            ui.horizontal(|ui| {
                                                ui.spinner();
                                                ui.label(&self.copilot_status);
                                            });
                                            ui.add_space(6.0);
                                            let last_poll_age = self
                                                .copilot_last_poll
                                                .elapsed()
                                                .as_secs();
                                            ui.small(format!(
                                                "Debug: polls={}, interval={}s, slow_downs={}, last_poll={}s ago",
                                                self.copilot_poll_attempts,
                                                self.copilot_poll_interval,
                                                self.copilot_slow_down_count,
                                                last_poll_age
                                            ));
                                            if !self.copilot_last_poll_result.is_empty() {
                                                ui.small(format!(
                                                    "Debug: last_result={}",
                                                    self.copilot_last_poll_result
                                                ));
                                            }
                                        });
                                    }
                                    "done" => {
                                        ui.vertical(|ui| {
                                            ui.colored_label(
                                                egui::Color32::from_rgb(60, 180, 100),
                                                i18n.t(
                                                    "providers.copilot_authorized",
                                                ),
                                            );
                                            ui.add_space(4.0);
                                            if !self.copilot_access_token.is_empty() {
                                                let preview = if self
                                                    .copilot_access_token
                                                    .len()
                                                    > 8
                                                {
                                                    format!(
                                                        "{}...{}",
                                                        &self.copilot_access_token[..4],
                                                        &self.copilot_access_token[self
                                                            .copilot_access_token
                                                            .len()
                                                            - 4..]
                                                    )
                                                } else {
                                                    "********".to_string()
                                                };
                                                ui.label(format!(
                                                    "{}: {}",
                                                    i18n.t("providers.tokenPreview"),
                                                    preview
                                                ));
                                            }
                                            ui.add_space(8.0);
                                            if ui
                                                .button(i18n.t("common.close"))
                                                .clicked()
                                            {
                                                self.copilot_device_state = None;
                                            }
                                        });
                                    }
                                    "error" => {
                                        ui.vertical(|ui| {
                                            ui.colored_label(
                                                egui::Color32::from_rgb(220, 80, 80),
                                                &self.copilot_status,
                                            );
                                            ui.add_space(8.0);
                                            if ui
                                                .button(
                                                    i18n.t("providers.copilot_retry"),
                                                )
                                                .clicked()
                                            {
                                                self.copilot_device_state = None;
                                                self.copilot_status.clear();
                                            }
                                            if ui
                                                .button(i18n.t("common.close"))
                                                .clicked()
                                            {
                                                self.copilot_device_state = None;
                                            }
                                        });
                                    }
                                    _ => {}
                                }
                            });
                        if !open {
                            self.copilot_device_state = None;
                        }
                    }
                }

                // ── Enterprise environment selector ──
                if config.features.setup_enterprise {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.label(i18n.t("setup.environment"));
                        egui::ComboBox::from_id_salt("setup_environment")
                            .selected_text(config.enterprise.active_environment.clone())
                            .show_ui(ui, |ui| {
                                for env in &config.enterprise.environments {
                                    if ui
                                        .selectable_label(
                                            config.enterprise.active_environment == env.name,
                                            &env.name,
                                        )
                                        .clicked()
                                    {
                                        config.enterprise.active_environment = env.name.clone();
                                        config.backend_url = env.backend_url.clone();
                                    }
                                }
                            });
                    });

                    ui.horizontal(|ui| {
                        ui.label(i18n.t("setup.secretSource"));
                        egui::ComboBox::from_id_salt("setup_secret_source")
                            .selected_text(config.enterprise.secret_source.clone())
                            .show_ui(ui, |ui| {
                                for source in ["keyring", "env", "file", "auto"] {
                                    if ui
                                        .selectable_label(
                                            config.enterprise.secret_source == source,
                                            source,
                                        )
                                        .clicked()
                                    {
                                        config.enterprise.secret_source = source.to_string();
                                    }
                                }
                            });
                    });
                }

                ui.add_space(10.0);

                // ── Error message ──
                if !self.error_msg.is_empty() {
                    let text = self.error_msg.clone();
                    let resp = ui.colored_label(egui::Color32::RED, &text);
                    resp.context_menu(|ui| {
                        if ui.button(i18n.t("common.copyButton")).clicked() {
                            ui.ctx().copy_text(text.clone());
                            ui.close();
                        }
                    });
                }

                // ── Save / Skip buttons ──
                ui.horizontal(|ui| {
                    let selected_requires_secret =
                        provider_requires_secret(&self.selected_provider.to_lowercase());
                    let can_save = if self.selected_provider.to_lowercase() == "copilot" {
                        // Copilot can save even without a key if OAuth was completed
                        !self.api_key.is_empty() || self.copilot_token_stored
                    } else if selected_requires_secret {
                        !self.api_key.is_empty() && !self.new_secret_key.is_empty()
                    } else {
                        !self.api_key.is_empty()
                    };

                    if ui
                        .add_enabled(
                            can_save && !self.sending,
                            egui::Button::new(i18n.t("setup.save")),
                        )
                        .clicked()
                    {
                        let api_key: String = self
                            .api_key
                            .chars()
                            .filter(|c| !c.is_control() || *c == '\t')
                            .collect::<String>()
                            .trim()
                            .to_string();
                        let secret_key: String = self
                            .new_secret_key
                            .chars()
                            .filter(|c| !c.is_control() || *c == '\t')
                            .collect::<String>()
                            .trim()
                            .to_string();
                        let model = self.selected_model.trim().to_string();
                        let provider_lower = self.selected_provider.to_lowercase();

                        if provider_requires_secret(&provider_lower) && secret_key.is_empty() {
                            self.error_msg =
                                i18n.t("providers.secret_key_placeholder").to_string();
                            ctx.request_repaint();
                            return;
                        }

                        // Store to system keyring (best-effort)
                        if let Err(e) =
                            crate::keyring_util::store_api_key(&provider_lower, &api_key)
                        {
                            eprintln!(
                                "keyring: failed to store key for '{}': {}",
                                provider_lower, e
                            );
                        }

                        if !secret_key.is_empty() {
                            if let Err(e) = crate::keyring_util::store_secret_key(
                                &provider_lower,
                                &secret_key,
                            ) {
                                eprintln!(
                                    "keyring: failed to store secret key for '{}': {}",
                                    provider_lower, e
                                );
                            }
                        }

                        // Handle label / duplicate detection
                        let existing_count = config
                            .providers
                            .iter()
                            .filter(|p| p.name == self.selected_provider)
                            .count();
                        let label = self.new_label.trim().to_string();

                        if existing_count > 0 && label.is_empty() {
                            // No label provided, update the first matching entry
                            if !api_key.is_empty() {
                                if let Some(existing) = config
                                    .providers
                                    .iter_mut()
                                    .find(|p| p.name == self.selected_provider && p.label.is_empty())
                                {
                                    existing.validated = true;
                                    if !model.is_empty() && model != "auto" {
                                        existing.model = model.clone();
                                    }
                                }
                                save_app_config(config);
                                // Auto-push to backend
                                if !self.sending {
                                    self.sending = true;
                                    let tx = self.pending_tx.clone();
                                    let backend_clone = backend.clone();
                                    let push_name = self.selected_provider.clone();
                                    let push_key = api_key.clone();
                                    let push_secret_key = secret_key.clone();
                                    let push_model = model.clone();
                                    let ctx_clone = ctx.clone();
                                    let ok_fmt = format!(
                                        "{} '{}' {}",
                                        i18n.t("providers.api_key"),
                                        provider_label(i18n, &push_name),
                                        i18n.t("providers.push_success")
                                    );
                                    let err_fmt = format!(
                                        "{} '{}': %s",
                                        i18n.t("providers.push_failed"),
                                        provider_label(i18n, &push_name)
                                    );
                                    tokio::spawn(async move {
                                        tokio::time::sleep(
                                            std::time::Duration::from_millis(300),
                                        )
                                        .await;
                                        let has_secret = !push_secret_key.is_empty();
                                        let result = if has_secret {
                                            tokio::time::timeout(
                                                std::time::Duration::from_secs(10),
                                                backend_clone
                                                    .configure_provider_with_secret(
                                                        &push_name,
                                                        &push_key,
                                                        &push_secret_key,
                                                        &push_model,
                                                    ),
                                            )
                                            .await
                                        } else {
                                            tokio::time::timeout(
                                                std::time::Duration::from_secs(10),
                                                backend_clone.configure_provider(
                                                    &push_name,
                                                    &push_key,
                                                    &push_model,
                                                ),
                                            )
                                            .await
                                        };
                                        let msg = match result {
                                            Ok(Ok(_)) => ok_fmt,
                                            Ok(Err(e)) => err_fmt.replace("%s", &e),
                                            Err(_) => {
                                                err_fmt.replace("%s", "timeout")
                                            }
                                        };
                                        if let Err(e) = tx.try_send(msg) {
                                            eprintln!(
                                                "WARN: setup try_send failed: {:?}",
                                                e
                                            );
                                        }
                                        ctx_clone.request_repaint();
                                    });
                                }
                            }
                        } else {
                            // New provider entry (possibly labeled duplicate)
                            let label_clean = label.replace(' ', "_");
                            config.providers.push(ProviderConfig {
                                name: self.selected_provider.clone(),
                                api_key: api_key.clone(),
                                secret_key: String::new(),
                                model: model.clone(),
                                validated: true,
                                label: label_clean,
                            });
                            save_app_config(config);
                            // Auto-push to backend for new entry
                            if !self.sending {
                                self.sending = true;
                                let tx = self.pending_tx.clone();
                                let backend_clone = backend.clone();
                                let push_name = self.selected_provider.clone();
                                let push_key = api_key.clone();
                                let push_secret_key = secret_key.clone();
                                let push_model = model.clone();
                                let ctx_clone = ctx.clone();
                                let ok_fmt = format!(
                                    "{} '{}' {}",
                                    i18n.t("providers.api_key"),
                                    provider_label(i18n, &push_name),
                                    i18n.t("providers.push_success")
                                );
                                let err_fmt = format!(
                                    "{} '{}': %s",
                                    i18n.t("providers.push_failed"),
                                    provider_label(i18n, &push_name)
                                );
                                tokio::spawn(async move {
                                    tokio::time::sleep(
                                        std::time::Duration::from_millis(300),
                                    )
                                    .await;
                                    let has_secret = !push_secret_key.is_empty();
                                    let result = if has_secret {
                                        tokio::time::timeout(
                                            std::time::Duration::from_secs(10),
                                            backend_clone
                                                .configure_provider_with_secret(
                                                    &push_name,
                                                    &push_key,
                                                    &push_secret_key,
                                                    &push_model,
                                                ),
                                        )
                                        .await
                                    } else {
                                        tokio::time::timeout(
                                            std::time::Duration::from_secs(10),
                                            backend_clone.configure_provider(
                                                &push_name,
                                                &push_key,
                                                &push_model,
                                            ),
                                        )
                                        .await
                                    };
                                    let msg = match result {
                                        Ok(Ok(_)) => ok_fmt,
                                        Ok(Err(e)) => err_fmt.replace("%s", &e),
                                        Err(_) => {
                                            err_fmt.replace("%s", "timeout")
                                        }
                                    };
                                    if let Err(e) = tx.try_send(msg) {
                                        eprintln!(
                                            "WARN: setup try_send failed: {:?}",
                                            e
                                        );
                                    }
                                    ctx_clone.request_repaint();
                                });
                            }
                        }

                        self.api_key = api_key;
                        self.error_msg.clear();
                        self.success_msg = i18n.t("setup.success").to_string();
                        ctx.request_repaint();
                    }

                    if ui.button(i18n.t("setup.skip")).clicked() {
                        done = true;
                    }
                });
            });
        });

        // ── Copilot Device Code auto-poll (uses system proxy) ──
        if self.copilot_device_state.as_deref() == Some("polling") {
            let poll_interval = std::time::Duration::from_secs(self.copilot_poll_interval);
            let elapsed = self.copilot_last_poll.elapsed();
            if elapsed >= poll_interval {
                self.copilot_last_poll = Instant::now();
                self.copilot_poll_repaint_requested = false;
                self.copilot_poll_attempts = self.copilot_poll_attempts.saturating_add(1);
                let backend_clone = backend.clone();
                let tx = self.pending_tx.clone();
                let device_code = self.copilot_device_code.clone();
                let ctx_clone = ctx.clone();
                #[cfg(debug_assertions)]
                {
                    let proxy_url = std::env::var("HTTPS_PROXY").unwrap_or_default();
                    eprintln!(
                        "[poll] device_code={}, HTTPS_PROXY={}",
                        &device_code[..8.min(device_code.len())],
                        proxy_url
                    );
                }
                tokio::spawn(async move {
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(15),
                        backend_clone.copilot_device_code_poll(&device_code),
                    )
                    .await
                    {
                        Ok(Ok(body)) => {
                            let msg = format!(
                                "__copilot_poll__:{}",
                                serde_json::to_string(&body).unwrap_or_default()
                            );
                            if let Err(e) = tx.try_send(msg) {
                                eprintln!("WARN: setup try_send failed: {:?}", e);
                            }
                        }
                        Ok(Err(e)) => {
                            let msg = format!("__copilot_poll_err__:{}", e);
                            if let Err(e) = tx.try_send(msg) {
                                eprintln!("WARN: setup try_send failed: {:?}", e);
                            }
                        }
                        Err(_) => {
                            let msg = "__copilot_poll_err__:Poll timed out.".to_string();
                            if let Err(e) = tx.try_send(msg) {
                                eprintln!("WARN: setup try_send failed: {:?}", e);
                            }
                        }
                    }
                    ctx_clone.request_repaint();
                });
            } else if !self.copilot_poll_repaint_requested {
                let remaining = poll_interval.saturating_sub(elapsed);
                ctx.request_repaint_after(remaining.max(std::time::Duration::from_millis(100)));
                self.copilot_poll_repaint_requested = true;
            }
        }

        done
    }
}
