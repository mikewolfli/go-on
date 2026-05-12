use crate::backend::{BackendClient, ProviderCapabilityModel};
use crate::config::{save_app_config, AppConfig, ProviderConfig};
use crate::i18n::I18n;
use crate::views::security_prefs;
use serde_json;
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub struct ProvidersView {
    /// Selected provider name from a predefined list (for Add)
    selected_provider: String,
    new_key: String,
    new_model: String,
    /// Index of provider being updated in existing list (-1 = none)
    update_target: isize,
    status: String,
    sending: bool,
    pending_delete_confirmation: Option<usize>,
    pending_rx: mpsc::Receiver<String>,
    pending_tx: mpsc::Sender<String>,
    /// Models fetched from backend: provider → [model_id, ...]
    remote_models: std::collections::HashMap<String, Vec<String>>,
    /// Whether we've tried to fetch models
    models_loaded: bool,
    provider_ops_status: std::collections::HashMap<String, String>,
    provider_capabilities: std::collections::HashMap<String, Vec<ProviderCapabilityModel>>,
    /// Cached security prefs — reloaded at most once per 10s to avoid per-frame disk reads.
    cached_security: security_prefs::SecurityPrefs,
    security_last_load: Instant,
}

/// Provider names for the dropdown (34 total, matching providers.toml)
/// This is the CANONICAL source of provider names used throughout the codebase.
/// `config.rs` and `app.rs` import this to avoid hardcoded lists.
pub const PROVIDER_NAMES: &[&str] = &[
    // OpenAI Family (4)
    "openai",
    "openai_compatible",
    "anthropic",
    "cohere",
    // Chinese Vendors (15)
    "deepseek",
    "wenxin",
    "qianfan",
    "qwen",
    "glm",
    "yi",
    "hunyuan",
    "doubao",
    "facewall",
    "langboat",
    "skywork",
    "stepfun",
    "xihu",
    "moonshot",
    "minimax",
    // Other Vendors (15)
    "ai21",
    "aleph",
    "copilot",
    "deepquest",
    "fireworks",
    "gemini",
    "groq",
    "llama",
    "loopai",
    "mistral",
    "nim",
    "perplexity",
    "replicate",
    "titan",
    "together",
];

fn provider_label(i18n: &I18n, provider: &str) -> String {
    let key = format!("provider.{}", provider.to_lowercase());
    let label = i18n.t(&key);
    if label.as_ref() == key {
        provider.to_string()
    } else {
        label.into_owned()
    }
}

fn models_for_provider(provider: &str) -> &'static [&'static str] {
    match provider.to_lowercase().as_str() {
        "deepseek" => &["auto", "deepseek-v4-flash", "deepseek-v4-pro"],
        "openai" => &[
            "auto",
            "gpt-4o",
            "gpt-4o-mini",
            "gpt-4-turbo",
            "gpt-3.5-turbo",
        ],
        "openai_compatible" => &["auto"],
        "anthropic" => &[
            "auto",
            "claude-sonnet-4-20250514",
            "claude-3-5-sonnet-20241022",
            "claude-3-opus-20240229",
            "claude-3-haiku-20240307",
        ],
        "cohere" => &["auto", "command-r-plus-08-2024", "command-r"],
        "wenxin" => &["auto", "ERNIE-4.5-8K", "ERNIE-4.0", "ERNIE-3.5"],
        "qianfan" => &["auto", "ERNIE-Bot", "ERNIE-Bot-turbo"],
        "qwen" => &["auto", "qwen-max-2025-01-25", "qwen-plus", "qwen-turbo"],
        "glm" => &["auto", "glm-4-flash", "glm-4-plus"],
        "yi" => &["auto", "yi-lightning", "yi-large"],
        "hunyuan" => &["auto", "hunyuan-turbo-latest"],
        "doubao" => &["auto", "doubao-1.5-pro-32k-250115"],
        "gemini" => &[
            "auto",
            "gemini-2.5-flash-preview-04-17",
            "gemini-2.0-flash",
            "gemini-1.5-pro",
        ],
        "groq" => &[
            "auto",
            "llama-3.3-70b-versatile",
            "llama-3.1-8b-instant",
            "mixtral-8x7b-32768",
        ],
        "mistral" => &[
            "auto",
            "mistral-small-latest",
            "mistral-medium-latest",
            "mistral-large-latest",
        ],
        "copilot" => &["auto", "github-copilot"],
        "facewall" | "langboat" | "skywork" | "xihu" | "deepquest" | "fireworks" | "loopai"
        | "titan" => &["auto"],
        "stepfun" => &["auto", "step-2-16k-2505"],
        "moonshot" => &["auto", "moonshot-v1-8k"],
        "minimax" => &["auto", "MiniMax-Text-01"],
        "ai21" => &["auto", "jamba-1.5-mini"],
        "aleph" => &["auto", "luminous-base-control"],
        "llama" => &["auto", "llama3.2", "llama3.1"],
        "nim" => &["auto", "meta/llama-3.1-70b-instruct"],
        "perplexity" => &["auto", "sonar-pro", "sonar"],
        "replicate" => &["auto", "meta/meta-llama-3-70b-instruct"],
        "together" => &["auto", "meta-llama/Meta-Llama-3.1-70B-Instruct-Turbo"],
        _ => &["auto"],
    }
}

impl ProvidersView {
    pub fn new() -> Self {
        let (pending_tx, pending_rx) = mpsc::channel();
        Self {
            selected_provider: PROVIDER_NAMES[0].to_string(),
            new_key: String::new(),
            new_model: "auto".to_string(),
            update_target: -1,
            status: String::new(),
            sending: false,
            pending_delete_confirmation: None,
            pending_rx,
            pending_tx,
            remote_models: std::collections::HashMap::new(),
            models_loaded: false,
            provider_ops_status: std::collections::HashMap::new(),
            provider_capabilities: std::collections::HashMap::new(),
            cached_security: security_prefs::load(),
            security_last_load: Instant::now(),
        }
    }

    pub fn reset_loaded_state(&mut self) {
        self.models_loaded = false;
        self.remote_models.clear();
        self.provider_capabilities.clear();
        self.provider_ops_status.clear();
    }

    fn process_pending(&mut self) {
        // Limit event processing per frame to prevent UI freeze
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
            } else if let Some(rest) = msg.strip_prefix("__ops__:") {
                if let Some((provider, status)) = rest.split_once(':') {
                    self.provider_ops_status
                        .insert(provider.to_string(), status.to_string());
                }
            } else if let Some(rest) = msg.strip_prefix("__caps__:") {
                if let Some((provider, payload)) = rest.split_once(':') {
                    if let Ok(models) =
                        serde_json::from_str::<Vec<ProviderCapabilityModel>>(payload)
                    {
                        self.provider_capabilities
                            .insert(provider.to_string(), models);
                    }
                }
            } else {
                self.sending = false;
                self.status = msg;
            }
        }
    }

    /// Fetch models from backend on first load. Spawns a background task.
    fn ensure_models_loaded(&mut self, backend: &BackendClient, ctx: &egui::Context) {
        if !self.models_loaded {
            self.models_loaded = true;
            let backend_clone = backend.clone();
            let tx = self.pending_tx.clone();
            let ctx_clone = ctx.clone();
            tokio::spawn(async move {
                let models = match tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    backend_clone.fetch_models(),
                )
                .await
                {
                    Ok(m) => m,
                    Err(_) => {
                        #[cfg(debug_assertions)]
                        eprintln!("Warning: Failed to fetch models from backend (timeout)");
                        std::collections::HashMap::new()
                    }
                };
                let msg = format!(
                    "__models__:{}",
                    serde_json::to_string(&models).unwrap_or_default()
                );
                let _ = tx.send(msg);
                ctx_clone.request_repaint_after(Duration::from_millis(16));
            });
        }
    }

    /// Reload security prefs at most once per 10 seconds.
    fn refresh_security_cache(&mut self) {
        if self.security_last_load.elapsed() >= std::time::Duration::from_secs(10) {
            self.cached_security = security_prefs::load();
            self.security_last_load = Instant::now();
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        i18n: &I18n,
        config: &mut AppConfig,
        backend: &BackendClient,
        ctx: &egui::Context,
        ops_enabled: bool,
    ) -> bool {
        let mut changed = false;
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                self.process_pending();
                self.ensure_models_loaded(backend, ctx);
                self.refresh_security_cache();

                // Copy needed bools to avoid holding a reference to self over closures.
                let redact_keys = self.cached_security.redact_api_keys_in_ui;
                let confirm_dangerous = self.cached_security.confirm_dangerous_actions;

                ui.heading(i18n.t("providers.title"));
                ui.separator();
                ui.add_space(8.0);

                // ── Add new provider section ──────────────────────────────────
                // Basic key CRUD is always available (not gated by ops_enabled).
                // ops_enabled only controls advanced operations like test/capabilities.
                {
                    ui.label(i18n.t("providers.add_new"));
                    ui.horizontal(|ui| {
                        ui.label(i18n.t("providers.provider"));
                        egui::ComboBox::from_id_salt("add_provider_sel")
                            .selected_text(provider_label(i18n, &self.selected_provider))
                            .show_ui(ui, |ui| {
                                for p in PROVIDER_NAMES {
                                    if ui
                                        .selectable_value(
                                            &mut self.selected_provider,
                                            p.to_string(),
                                            provider_label(i18n, p),
                                        )
                                        .clicked()
                                    {
                                        self.new_model = "auto".to_string();
                                    }
                                }
                            });
                        ui.label(i18n.t("providers.api_key"));
                        ui.add(
                            egui::TextEdit::singleline(&mut self.new_key)
                                .password(true)
                                .hint_text(i18n.t("common.apiKeyPlaceholder"))
                                .desired_width(260.0),
                        );
                        // Show auto-push hint when updating an existing provider
                        if self.update_target >= 0 {
                            ui.colored_label(
                                egui::Color32::from_rgb(100, 180, 100),
                                i18n.t("providers.auto_push_hint")
                            );
                        }
                        ui.label(i18n.t("providers.model"));
                        egui::ComboBox::from_id_salt("add_model_sel")
                            .selected_text({
                                if self.new_model == "auto" {
                                    i18n.t("providers.auto").to_string()
                                } else {
                                    format!(
                                        "{}: {}",
                                        provider_label(i18n, &self.selected_provider),
                                        self.new_model
                                    )
                                }
                            })
                            .show_ui(ui, |ui| {
                                // Show hint for copilot
                                if self.selected_provider.to_lowercase() == "copilot" {
                                    ui.label(i18n.t("providers.copilot_hint"));
                                }
                                let models = models_for_provider(&self.selected_provider);
                                for m in models {
                                    let display_name = if m == &"auto" {
                                        i18n.t("providers.auto").to_string()
                                    } else {
                                        format!(
                                            "{}: {}",
                                            provider_label(i18n, &self.selected_provider),
                                            m
                                        )
                                    };
                                    ui.selectable_value(
                                        &mut self.new_model,
                                        m.to_string(),
                                        display_name,
                                    );
                                }
                            });
                        if ui
                            .add_enabled(
                                !self.new_key.is_empty()
                                    || self.selected_provider.to_lowercase() == "copilot",
                                egui::Button::new(i18n.t("providers.add")),
                            )
                            .clicked()
                        {
                            let name = self.selected_provider.clone();
                            let key: String = self.new_key
                                .chars()
                                .filter(|c| !c.is_control() || *c == '\t')
                                .collect::<String>()
                                .trim()
                                .to_string();
                            let model = self.new_model.trim().to_string();
                            let provider_lower = name.to_lowercase();

                            // Try keyring, but don't block the save if it fails
                            if let Err(e) =
                                crate::keyring_util::store_api_key(&provider_lower, &key)
                            {
                                eprintln!(
                                    "Warning: failed to store API key in system keyring: {}",
                                    e
                                );
                            }

                            let provider_exists = config.providers.iter().any(|p| p.name == name);
                            if provider_exists {
                                // Provider already exists — update key and model if a new key was provided
                                if !key.is_empty() {
                                    if let Some(existing) = config.providers.iter_mut().find(|p| p.name == name) {
                                        existing.api_key = key.clone();
                                        existing.validated = true;
                                        if !model.is_empty() && model != "auto" {
                                            existing.model = model.clone();
                                        }
                                    }
                                    save_app_config(config);
                                    self.status = format!(
                                        "{} '{}' {}.",
                                        i18n.t("providers.api_key"),
                                        provider_label(i18n, &name),
                                        i18n.t("providers.updated")
                                    );
                                    // Auto-push updated key to backend
                                    if !self.sending {
                                        self.sending = true;
                                        let tx = self.pending_tx.clone();
                                        let backend_clone = backend.clone();
                                        let push_name = name.clone();
                                        let push_key = key.clone();
                                        let push_model = model.clone();
                                        if push_model.is_empty() || push_model == "auto" {
                                            // Get the model from config if exists
                                            let _ = config.providers.iter().find(|p| p.name == name).map(|p| p.model.clone());
                                        }
                                        let ctx_clone = ctx.clone();
                                        let ok_fmt = format!(
                                            "{} '{}' {}",
                                            i18n.t("providers.api_key"),
                                            provider_label(i18n, &name),
                                            i18n.t("providers.push_success")
                                        );
                                        let err_fmt = format!(
                                            "{} '{}': %s",
                                            i18n.t("providers.push_failed"),
                                            provider_label(i18n, &name)
                                        );
                                        tokio::spawn(async move {
                                            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                                            let result = tokio::time::timeout(
                                                std::time::Duration::from_secs(10),
                                                backend_clone.configure_provider(&push_name, &push_key, &push_model),
                                            ).await;
                                            let msg = match result {
                                                Ok(Ok(_)) => ok_fmt,
                                                Ok(Err(e)) => err_fmt.replace("%s", &e),
                                                Err(_) => err_fmt.replace("%s", "timeout"),
                                            };
                                            let _ = tx.send(msg);
                                            ctx_clone.request_repaint_after(Duration::from_millis(16));
                                        });
                                    }
                                } else {
                                    self.status = format!(
                                        "{} '{}' {}.",
                                        i18n.t("providers.provider"),
                                        provider_label(i18n, &name),
                                        i18n.t("providers.already_exists")
                                    );
                                }
                            } else {
                                config.providers.push(ProviderConfig {
                                    name: name.clone(),
                                    api_key: key.clone(),
                                    model: model.clone(),
                                    validated: true,
                                });
                                save_app_config(config);
                                self.status = format!(
                                    "{} '{}' {}.",
                                    i18n.t("providers.provider"),
                                    provider_label(i18n, &name),
                                    i18n.t("providers.added")
                                );
                                // Auto-push new provider key to backend so it takes effect immediately.
                                if !self.sending {
                                    self.sending = true;
                                    let tx = self.pending_tx.clone();
                                    let backend_clone = backend.clone();
                                    let push_name = name.clone();
                                    let push_key = key.clone();
                                    let push_model = model.clone();
                                    let ctx_clone = ctx.clone();
                                    let ok_fmt = format!(
                                        "{} '{}' {}",
                                        i18n.t("providers.api_key"),
                                        provider_label(i18n, &name),
                                        i18n.t("providers.push_success")
                                    );
                                    let err_fmt = format!(
                                        "{} '{}': %s",
                                        i18n.t("providers.push_failed"),
                                        provider_label(i18n, &name)
                                    );
                                    tokio::spawn(async move {
                                        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                                        let result = tokio::time::timeout(
                                            std::time::Duration::from_secs(10),
                                            backend_clone.configure_provider(&push_name, &push_key, &push_model),
                                        ).await;
                                        let msg = match result {
                                            Ok(Ok(_)) => ok_fmt,
                                            Ok(Err(e)) => err_fmt.replace("%s", &e),
                                            Err(_) => err_fmt.replace("%s", "timeout"),
                                        };
                                        let _ = tx.send(msg);
                                        ctx_clone.request_repaint_after(Duration::from_millis(16));
                                    });
                                }
                            }
                            self.new_key.clear();
                            changed = true;
                        }
                    });
                }

                ui.add_space(16.0);
                ui.separator();
                ui.add_space(8.0);

                // ── Existing providers list ────────────────────────────────────
                ui.label(i18n.t("providers.saved"));
                let mut remove_idx = None;
                for (idx, provider) in config.providers.iter_mut().enumerate() {
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(provider_label(i18n, &provider.name));
                            let cached_key = crate::keyring_util::get_api_key_with_fallback(
                                &provider.name.to_lowercase(),
                                Some(&provider.api_key),
                            )
                            .unwrap_or_default();
                            let key_preview_str = if redact_keys {
                                "********".to_string()
                            } else if cached_key.len() > 8 {
                                cached_key[..8].to_string()
                            } else if !cached_key.is_empty() {
                                cached_key.clone()
                            } else {
                                i18n.t("providers.noKey").to_string()
                            };
                            ui.label(format!(
                                "{} {}",
                                i18n.t("providers.key_preview"),
                                key_preview_str
                            ));
                            ui.label(i18n.t("providers.model"));
                            // Model dropdown for saved providers
                            let models = models_for_provider(&provider.name);
                            egui::ComboBox::from_id_salt(format!("model_{}", idx))
                                .selected_text({
                                    if provider.model == "auto" || provider.model.is_empty() {
                                        i18n.t("providers.auto").to_string()
                                    } else {
                                        format!(
                                            "{}: {}",
                                            provider_label(i18n, &provider.name),
                                            provider.model
                                        )
                                    }
                                })
                                .show_ui(ui, |ui| {
                                    for m in models {
                                        let display_name = if m == &"auto" {
                                            i18n.t("providers.auto").to_string()
                                        } else {
                                            format!(
                                                "{}: {}",
                                                provider_label(i18n, &provider.name),
                                                m
                                            )
                                        };
                                        if ui
                                            .selectable_value(
                                                &mut provider.model,
                                                m.to_string(),
                                                display_name,
                                            )
                                            .clicked()
                                        {
                                            changed = true;
                                            // Auto-push updated model to backend
                                            if !self.sending {
                                                self.sending = true;
                                                let tx_push = self.pending_tx.clone();
                                                let backend_push = backend.clone();
                                                let name_push = provider.name.clone();
                                                let model_push = m.to_string();
                                                let ctx_push = ctx.clone();
                                                let key_push = crate::keyring_util::get_api_key_with_fallback(
                                                    &name_push.to_lowercase(),
                                                    Some(&provider.api_key),
                                                ).unwrap_or_default();
                                                tokio::spawn(async move {
                                                    tokio::time::sleep(
                                                        std::time::Duration::from_millis(100),
                                                    )
                                                    .await;
                                                    let _ = backend_push
                                                        .configure_provider(
                                                            &name_push,
                                                            &key_push,
                                                            &model_push,
                                                        )
                                                        .await;
                                                    // Clear sending flag after push
                                                    let _ = tx_push.send(String::new());
                                                    ctx_push.request_repaint_after(Duration::from_millis(16));
                                                });
                                            }
                                        }
                                    }
                                });
                            if ui
                                .checkbox(&mut provider.validated, i18n.t("providers.validated"))
                                .changed()
                            {
                                changed = true;
                            }

                            // Update button - opens inline edit for this provider
                            // Key update is a basic operation (not gated by ops_enabled).
                            if self.update_target == idx as isize {
                                if ui.button(i18n.t("providers.save_key")).clicked() {
                                    let new_key: String = self.new_key
                                        .chars()
                                        .filter(|c| !c.is_control() || *c == '\t')
                                        .collect::<String>()
                                        .trim()
                                        .to_string();
                                    if !new_key.is_empty() {
                                        let provider_lower = provider.name.to_lowercase();
                                        let provider_name = provider.name.clone();
                                        // Store to system keyring
                                        match crate::keyring_util::store_api_key(
                                            &provider_lower,
                                            &new_key,
                                        ) {
                                            Ok(_) => {
                                                eprintln!(
                                                    "keyring: stored key for '{}'",
                                                    provider_lower
                                                );
                                            }
                                            Err(e) => {
                                                eprintln!(
                                                    "keyring: failed to store key for '{}': {}",
                                                    provider_lower, e
                                                );
                                            }
                                        }
                                        // Always save to config as well (dual storage)
                                        provider.api_key = new_key.clone();
                                        provider.validated = true;
                                        self.status = format!(
                                            "{} '{}' {}.",
                                            i18n.t("providers.api_key"),
                                            provider_label(i18n, &provider_name),
                                            i18n.t("providers.updated")
                                        );
                                        self.new_key.clear();
                                        self.update_target = -1;
                                        changed = true;

                                        // Auto-push to backend after saving
                                        if !self.sending {
                                            self.sending = true;
                                            let tx = self.pending_tx.clone();
                                            let backend_clone = backend.clone();
                                            let name = provider_name.clone();
                                            let key = new_key;
                                            let model = provider.model.clone();
                                            let ctx_clone = ctx.clone();
                                            let ok_fmt = format!(
                                                "{} '{}' {}",
                                                i18n.t("providers.api_key"),
                                                provider_label(i18n, &name),
                                                i18n.t("providers.push_success")
                                            );
                                            let err_fmt = format!(
                                                "{} '{}': %s",
                                                i18n.t("providers.push_failed"),
                                                provider_label(i18n, &name)
                                            );
                                            tokio::spawn(async move {
                                                // Small delay to ensure keyring write completes
                                                tokio::time::sleep(std::time::Duration::from_millis(100)).await;

                                                let result = match tokio::time::timeout(
                                                    std::time::Duration::from_secs(10),
                                                    backend_clone.configure_provider(&name, &key, &model),
                                                )
                                                .await
                                                {
                                                    Ok(r) => r,
                                                    Err(_) => {
                                                        #[cfg(debug_assertions)]
                                                        eprintln!("Warning: auto-push configure_provider timed out");
                                                        Err("timeout".to_string())
                                                    }
                                                };
                                                let msg = match result {
                                                    Ok(_) => ok_fmt,
                                                    Err(e) => err_fmt.replace("%s", &e.to_string()),
                                                };
                                                let _ = tx.send(msg);
                                                ctx_clone.request_repaint_after(Duration::from_millis(16));
                                            });
                                        }
                                    }
                                }
                                if ui.button(i18n.t("providers.cancel")).clicked() {
                                    self.update_target = -1;
                                    self.new_key.clear();
                                }
                            } else if ui.button(i18n.t("providers.update_key")).clicked()
                            {
                                self.update_target = idx as isize;
                                self.new_key.clear();
                                self.status = format!(
                                    "{} '{}' {}.",
                                    i18n.t("providers.enter_new_key"),
                                    provider_label(i18n, &provider.name),
                                    i18n.t("providers.save_key")
                                );
                            }

                            if ops_enabled && ui.button(i18n.t("providers.push")).clicked()
                                && !self.sending
                            {
                                self.sending = true;
                                self.status.clear();
                                let tx = self.pending_tx.clone();
                                let backend_clone = backend.clone();
                                let name = provider.name.clone();
                                // Read API key from keyring (fallback to config)
                                let key = crate::keyring_util::get_api_key_with_fallback(
                                    &name.to_lowercase(),
                                    Some(&provider.api_key),
                                )
                                .unwrap_or_default();
                                let model = provider.model.clone();
                                let ctx_clone = ctx.clone();
                                let ok_fmt = format!(
                                    "{} %s {}.",
                                    i18n.t("providers.provider"),
                                    i18n.t("providers.configured")
                                );
                                let err_fmt = format!("{} %s", i18n.t("providers.push_failed"));
                                tokio::spawn(async move {
                                    // Add timeout to prevent hanging
                                    let result = match tokio::time::timeout(
                                        std::time::Duration::from_secs(10),
                                        backend_clone.configure_provider(&name, &key, &model),
                                    )
                                    .await
                                    {
                                        Ok(r) => r,
                                        Err(_) => {
                                            #[cfg(debug_assertions)]
                                            eprintln!("Warning: configure_provider timed out");
                                            Err("timeout".to_string())
                                        }
                                    };
                                    let msg = match result {
                                        Ok(_) => ok_fmt.replace("%s", &name),
                                        Err(e) => err_fmt.replace("%s", &e.to_string()),
                                    };
                                    let _ = tx.send(msg);
                                    ctx_clone.request_repaint_after(Duration::from_millis(16));
                                });
                            }
                            if ops_enabled
                                && ui.button(i18n.t("providers.ops.testConn")).clicked()
                            {
                                let name = provider.name.clone();
                                let tx = self.pending_tx.clone();
                                let backend_clone = backend.clone();
                                let ctx_clone = ctx.clone();
                                let ok_tpl = i18n.t("providers.ops.connStatus").to_string();
                                let err_tpl = i18n.t("providers.ops.connStatusFailed").to_string();
                                tokio::spawn(async move {
                                    // Add timeout to prevent hanging
                                    let result = match tokio::time::timeout(
                                        std::time::Duration::from_secs(10),
                                        backend_clone.provider_test_connection(&name),
                                    )
                                    .await
                                    {
                                        Ok(r) => r,
                                        Err(_) => {
                                            #[cfg(debug_assertions)]
                                            eprintln!(
                                                "Warning: provider_test_connection timed out"
                                            );
                                            Err("timeout".to_string())
                                        }
                                    };
                                    let msg = match result {
                                        Ok(v) => {
                                            let ok = v
                                                .get("ok")
                                                .and_then(serde_json::Value::as_bool)
                                                .unwrap_or(false);
                                            let latency = v
                                                .get("latency_ms")
                                                .and_then(serde_json::Value::as_u64)
                                                .unwrap_or(0);
                                            format!(
                                                "__ops__:{}:{}",
                                                name,
                                                ok_tpl
                                                    .replace("{ok}", &ok.to_string())
                                                    .replace("{latency}", &latency.to_string())
                                            )
                                        }
                                        Err(e) => format!(
                                            "__ops__:{}:{}",
                                            name,
                                            err_tpl.replace("{error}", &e.to_string())
                                        ),
                                    };
                                    let _ = tx.send(msg);
                                    ctx_clone.request_repaint_after(Duration::from_millis(16));
                                });
                            }
                            if ops_enabled
                                && ui.button(i18n.t("providers.ops.testCompletion")).clicked()
                            {
                                let name = provider.name.clone();
                                let model = if provider.model.is_empty() {
                                    None
                                } else {
                                    Some(provider.model.clone())
                                };
                                let tx = self.pending_tx.clone();
                                let backend_clone = backend.clone();
                                let ctx_clone = ctx.clone();
                                let ok_tpl = i18n.t("providers.ops.completionStatus").to_string();
                                let err_tpl =
                                    i18n.t("providers.ops.completionStatusFailed").to_string();
                                tokio::spawn(async move {
                                    // Add timeout to prevent hanging
                                    let result = match tokio::time::timeout(
                                        std::time::Duration::from_secs(10),
                                        backend_clone
                                            .provider_test_completion(&name, model.as_deref()),
                                    )
                                    .await
                                    {
                                        Ok(r) => r,
                                        Err(_) => {
                                            #[cfg(debug_assertions)]
                                            eprintln!(
                                                "Warning: provider_test_completion timed out"
                                            );
                                            Err("timeout".to_string())
                                        }
                                    };
                                    let msg = match result {
                                        Ok(v) => {
                                            let ok = v
                                                .get("ok")
                                                .and_then(serde_json::Value::as_bool)
                                                .unwrap_or(false);
                                            let model = v
                                                .get("model")
                                                .and_then(serde_json::Value::as_str)
                                                .unwrap_or("-");
                                            format!(
                                                "__ops__:{}:{}",
                                                name,
                                                ok_tpl
                                                    .replace("{ok}", &ok.to_string())
                                                    .replace("{model}", model)
                                            )
                                        }
                                        Err(e) => format!(
                                            "__ops__:{}:{}",
                                            name,
                                            err_tpl.replace("{error}", &e.to_string())
                                        ),
                                    };
                                    let _ = tx.send(msg);
                                    ctx_clone.request_repaint_after(Duration::from_millis(16));
                                });
                            }
                            if ops_enabled
                                && ui.button(i18n.t("providers.ops.capabilities")).clicked()
                            {
                                let name = provider.name.clone();
                                let tx = self.pending_tx.clone();
                                let backend_clone = backend.clone();
                                let ctx_clone = ctx.clone();
                                let count_tpl =
                                    i18n.t("providers.ops.capabilitiesCount").to_string();
                                let failed_tpl =
                                    i18n.t("providers.ops.capabilitiesFailed").to_string();
                                let encode_failed_tpl =
                                    i18n.t("providers.ops.capabilitiesEncodeFailed").to_string();
                                tokio::spawn(async move {
                                    // Add timeout to prevent hanging
                                    let result = match tokio::time::timeout(
                                        std::time::Duration::from_secs(10),
                                        backend_clone.provider_capabilities(&name),
                                    )
                                    .await
                                    {
                                        Ok(r) => r,
                                        Err(_) => {
                                            #[cfg(debug_assertions)]
                                            eprintln!("Warning: provider_capabilities timed out");
                                            Err("timeout".to_string())
                                        }
                                    };
                                    let msg = match result {
                                        Ok(models) => match serde_json::to_string(&models) {
                                            Ok(payload) => {
                                                let _ = tx.send(format!(
                                                    "__ops__:{}:{}",
                                                    name,
                                                    count_tpl.replace("{count}", &models.len().to_string())
                                                ));
                                                format!("__caps__:{}:{}", name, payload)
                                            }
                                            Err(e) => format!(
                                                "__ops__:{}:{}",
                                                name,
                                                encode_failed_tpl.replace("{error}", &e.to_string())
                                            ),
                                        },
                                        Err(e) => format!(
                                            "__ops__:{}:{}",
                                            name,
                                            failed_tpl.replace("{error}", &e.to_string())
                                        ),
                                    };
                                    let _ = tx.send(msg);
                                    ctx_clone.request_repaint_after(Duration::from_millis(16));
                                });
                            }
                            let delete_label = if self.pending_delete_confirmation == Some(idx) {
                                i18n.t("providers.confirm_delete")
                            } else {
                                i18n.t("providers.delete")
                            };
                            // Delete is a basic operation (not gated by ops_enabled).
                            if ui.button(delete_label).clicked() {
                                if confirm_dangerous
                                    && self.pending_delete_confirmation != Some(idx)
                                {
                                    self.pending_delete_confirmation = Some(idx);
                                    self.status = format!(
                                        "{} {}.",
                                        i18n.t("providers.click_delete_again"),
                                        provider_label(i18n, &provider.name)
                                    );
                                } else {
                                    remove_idx = Some(idx);
                                    self.pending_delete_confirmation = None;
                                }
                            }
                        });
                        if let Some(ops_status) = self.provider_ops_status.get(&provider.name) {
                            ui.label(ops_status);
                        }
                        if let Some(models) = self.provider_capabilities.get(&provider.name) {
                            for model in models.iter().take(3) {
                                let model_name = model
                                    .name
                                    .as_deref()
                                    .unwrap_or(model.id.as_str())
                                    .to_string();
                                let context = model
                                    .context_window
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| "-".to_string());
                                let tool = model.tool_calling.unwrap_or(false);
                                let vision = model.vision.unwrap_or(false);
                                let cost = model
                                    .cost_tier
                                    .clone()
                                    .unwrap_or_else(|| "-".to_string());
                                ui.label(format!(
                                    "{} | {}={} | {}={} | {}={} | {}={}",
                                    model_name,
                                    i18n.t("providers.cap.context"),
                                    context,
                                    i18n.t("providers.cap.tool"),
                                    tool,
                                    i18n.t("providers.cap.vision"),
                                    vision,
                                    i18n.t("providers.cap.cost"),
                                    cost
                                ));
                            }
                            if models.len() > 3 {
                                ui.label(
                                    i18n
                                        .t("providers.cap.moreModels")
                                        .replace("{count}", &(models.len() - 3).to_string()),
                                );
                            }
                        }
                    });
                    ui.add_space(4.0);
                }

                if let Some(idx) = remove_idx {
                    // Clean up keyring entry before removing from config
                    if let Some(removed) = config.providers.get(idx) {
                        let _ = crate::keyring_util::delete_api_key(&removed.name.to_lowercase());
                    }
                    config.providers.remove(idx);
                    changed = true;
                    self.status = i18n.t("providers.removed").to_string();
                } else if self.pending_delete_confirmation.is_some()
                    && !config.providers.is_empty()
                    && self.pending_delete_confirmation.unwrap_or(0) >= config.providers.len()
                {
                    self.pending_delete_confirmation = None;
                }

                if !self.status.is_empty() {
                    ui.label(&self.status);
                }
            });
        changed
    }
}
