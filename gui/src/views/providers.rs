use crate::backend::BackendClient;
use crate::config::{save_app_config, AppConfig, ProviderConfig};
use crate::i18n::I18n;
use crate::views::security_prefs;
use serde_json;
use std::sync::mpsc;

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
}

/// Provider names for the dropdown (34 total, matching providers.toml)
const PROVIDER_NAMES: &[&str] = &[
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
        }
    }

    fn process_pending(&mut self) {
        while let Ok(msg) = self.pending_rx.try_recv() {
            if let Some(models_json) = msg.strip_prefix("__models__:") {
                if let Ok(models) = serde_json::from_str::<std::collections::HashMap<String, Vec<String>>>(models_json) {
                    self.remote_models = models;
                }
            } else {
                self.sending = false;
                self.status = msg;
            }
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        i18n: &I18n,
        config: &mut AppConfig,
        backend: &BackendClient,
        ctx: &egui::Context,
    ) {
        self.process_pending();

        // Fetch models from backend if not yet loaded
        if !self.models_loaded {
            self.models_loaded = true;
            let backend_clone = backend.clone();
            let tx = self.pending_tx.clone();
            let ctx_clone = ctx.clone();
            tokio::spawn(async move {
                let models = backend_clone.fetch_models().await;
                // Send models back via pending channel
                let msg = format!("__models__:{}", serde_json::to_string(&models).unwrap_or_default());
                let _ = tx.send(msg);
                ctx_clone.request_repaint();
            });
        }

        let mut changed = false;
        let security = security_prefs::load();

        ui.heading(i18n.t("providers.title"));
        ui.separator();
        ui.add_space(8.0);

        // ── Add new provider section ──────────────────────────────────
        ui.label(i18n.t("providers.add_new"));
        ui.horizontal(|ui| {
            ui.label(i18n.t("providers.provider"));
            egui::ComboBox::from_id_salt("add_provider_sel")
                .selected_text(&self.selected_provider)
                .show_ui(ui, |ui| {
                    for p in PROVIDER_NAMES {
                        if ui
                            .selectable_value(&mut self.selected_provider, p.to_string(), *p)
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
                    .hint_text("sk-...")
                    .desired_width(260.0),
            );
            ui.label(i18n.t("providers.model"));
            egui::ComboBox::from_id_salt("add_model_sel")
                .selected_text({
                    if self.new_model == "auto" {
                        i18n.t("providers.auto").to_string()
                    } else {
                        format!("{}: {}", self.selected_provider, self.new_model)
                    }
                })
                .show_ui(ui, |ui| {
                    // Show hint for copilot
                    if self.selected_provider.to_lowercase() == "copilot" {
                        ui.label(i18n.t("providers.copilot_hint"));
                    }
                    let models: &[&str] = match self.selected_provider.to_lowercase().as_str() {
                        "deepseek" => &[
                            "auto",
                            "deepseek-v4-flash",
                            "deepseek-v4-pro",
                        ],
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
                        "facewall" => &["auto"],
                        "langboat" => &["auto"],
                        "skywork" => &["auto"],
                        "stepfun" => &["auto", "step-2-16k-2505"],
                        "xihu" => &["auto"],
                        "moonshot" => &["auto", "moonshot-v1-8k"],
                        "minimax" => &["auto", "MiniMax-Text-01"],
                        "ai21" => &["auto", "jamba-1.5-mini"],
                        "aleph" => &["auto", "luminous-base-control"],
                        "copilot" => &["auto", "github-copilot"],
                        "deepquest" => &["auto"],
                        "fireworks" => &["auto"],
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
                        "llama" => &["auto", "llama3.2", "llama3.1"],
                        "loopai" => &["auto"],
                        "mistral" => &[
                            "auto",
                            "mistral-small-latest",
                            "mistral-medium-latest",
                            "mistral-large-latest",
                        ],
                        "nim" => &["auto", "meta/llama-3.1-70b-instruct"],
                        "perplexity" => &["auto", "sonar-pro", "sonar"],
                        "replicate" => &["auto", "meta/meta-llama-3-70b-instruct"],
                        "titan" => &["auto"],
                        "together" => &["auto", "meta-llama/Meta-Llama-3.1-70B-Instruct-Turbo"],
                        _ => &["auto"],
                    };
                    for m in models {
                        let display_name = if m == &"auto" {
                            i18n.t("providers.auto").to_string()
                        } else {
                            format!("{}: {}", self.selected_provider, m)
                        };
                        ui.selectable_value(&mut self.new_model, m.to_string(), display_name);
                    }
                });
            if ui
                .add_enabled(
                    !self.new_key.is_empty() || self.selected_provider.to_lowercase() == "copilot",
                    egui::Button::new(i18n.t("providers.add")),
                )
                .clicked()
            {
                let name = self.selected_provider.clone();
                let key = self.new_key.trim().to_string();
                let model = self.new_model.trim().to_string();
                let provider_lower = name.to_lowercase();

                // Try keyring, but don't block the save if it fails
                if let Err(e) = crate::keyring_util::store_api_key(&provider_lower, &key) {
                    eprintln!("Warning: failed to store API key in system keyring: {}", e);
                }

                if !config.providers.iter().any(|p| p.name == name) {
                    config.providers.push(ProviderConfig {
                        name: name.clone(),
                        api_key: key,
                        model,
                        validated: true,
                    });
                    save_app_config(config);
                    self.status = format!(
                        "{} '{}' {}.",
                        i18n.t("providers.provider"),
                        name,
                        i18n.t("providers.added")
                    );
                } else {
                    self.status = format!(
                        "{} '{}' {}.",
                        i18n.t("providers.provider"),
                        name,
                        i18n.t("providers.already_exists")
                    );
                }
                self.new_key.clear();
                changed = true;
            }
        });

        ui.add_space(16.0);
        ui.separator();
        ui.add_space(8.0);

        // ── Existing providers list ────────────────────────────────────
        ui.label(i18n.t("providers.saved"));
        let mut remove_idx = None;
        for (idx, provider) in config.providers.iter_mut().enumerate() {
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(&provider.name);
                    let key_preview = if security.redact_api_keys_in_ui {
                        "********"
                    } else if provider.api_key.len() > 8 {
                        &provider.api_key[..8]
                    } else {
                        &provider.api_key
                    };
                    ui.label(format!(
                        "{} {}",
                        i18n.t("providers.key_preview"),
                        key_preview
                    ));
                    ui.label(i18n.t("providers.model"));
                    // Model dropdown for saved providers
                    let models: &[&str] = match provider.name.to_lowercase().as_str() {
                        "deepseek" => &[
                            "auto",
                            "deepseek-v4-flash",
                            "deepseek-v4-pro",
                        ],
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
                        _ => &["auto"],
                    };
                    egui::ComboBox::from_id_salt(format!("model_{}", idx))
                        .selected_text({
                            if provider.model == "auto" || provider.model.is_empty() {
                                i18n.t("providers.auto").to_string()
                            } else {
                                format!("{}: {}", provider.name, provider.model)
                            }
                        })
                        .show_ui(ui, |ui| {
                            for m in models {
                                let display_name = if m == &"auto" {
                                    i18n.t("providers.auto").to_string()
                                } else {
                                    format!("{}: {}", provider.name, m)
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
                    if self.update_target == idx as isize {
                        if ui.button(i18n.t("providers.save_key")).clicked() {
                            let new_key = self.new_key.trim().to_string();
                            if !new_key.is_empty() {
                                let provider_lower = provider.name.to_lowercase();
                                let provider_name = provider.name.clone();
                                // Try keyring, don't block save if it fails
                                if let Err(e) = crate::keyring_util::store_api_key(&provider_lower, &new_key) {
                                    eprintln!("Warning: failed to store API key in system keyring: {}", e);
                                }
                                provider.api_key = new_key;
                                provider.validated = true;
                                self.status = format!(
                                    "{} '{}' {}.",
                                    i18n.t("providers.api_key"),
                                    provider_name,
                                    i18n.t("providers.updated")
                                );
                                self.new_key.clear();
                                self.update_target = -1;
                                changed = true;
                            }
                        }
                        if ui.button(i18n.t("providers.cancel")).clicked() {
                            self.update_target = -1;
                            self.new_key.clear();
                        }
                    } else if ui.button(i18n.t("providers.update_key")).clicked() {
                        self.update_target = idx as isize;
                        self.new_key.clear();
                        self.status = format!(
                            "{} '{}' {}.",
                            i18n.t("providers.enter_new_key"),
                            provider.name,
                            i18n.t("providers.save_key")
                        );
                    }

                    if ui.button(i18n.t("providers.push")).clicked() && !self.sending {
                        self.sending = true;
                        self.status.clear();
                        let tx = self.pending_tx.clone();
                        let backend_clone = backend.clone();
                        let name = provider.name.clone();
                        let key = provider.api_key.clone();
                        let model = provider.model.clone();
                        let ctx_clone = ctx.clone();
                        let ok_fmt = format!(
                            "{} %s {}.",
                            i18n.t("providers.provider"),
                            i18n.t("providers.configured")
                        );
                        let err_fmt = format!("{} %s", i18n.t("providers.push_failed"));
                        tokio::spawn(async move {
                            let msg =
                                match backend_clone.configure_provider(&name, &key, &model).await {
                                    Ok(_) => ok_fmt.replace("%s", &name),
                                    Err(e) => err_fmt.replace("%s", &e.to_string()),
                                };
                            let _ = tx.send(msg);
                            ctx_clone.request_repaint();
                        });
                    }
                    let delete_label = if self.pending_delete_confirmation == Some(idx) {
                        i18n.t("providers.confirm_delete")
                    } else {
                        i18n.t("providers.delete")
                    };
                    if ui.button(delete_label).clicked() {
                        if security.confirm_dangerous_actions
                            && self.pending_delete_confirmation != Some(idx)
                        {
                            self.pending_delete_confirmation = Some(idx);
                            self.status = format!(
                                "{} {}.",
                                i18n.t("providers.click_delete_again"),
                                provider.name
                            );
                        } else {
                            remove_idx = Some(idx);
                            self.pending_delete_confirmation = None;
                        }
                    }
                });
            });
            ui.add_space(4.0);
        }

        if let Some(idx) = remove_idx {
            config.providers.remove(idx);
            changed = true;
            self.status = i18n.t("providers.removed").to_string();
        } else if self.pending_delete_confirmation.is_some()
            && !config.providers.is_empty()
            && self.pending_delete_confirmation.unwrap_or(0) >= config.providers.len()
        {
            self.pending_delete_confirmation = None;
        }

        if changed {
            save_app_config(config);
        }

        if !self.status.is_empty() {
            ui.label(&self.status);
        }
    }
}
