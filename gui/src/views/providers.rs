use crate::backend::{BackendClient, ProviderCapabilityModel};
use crate::config::{save_app_config, AppConfig, ProviderConfig};
use crate::i18n::I18n;
use crate::views::security_prefs;
use keyring;
use serde_json::Value;
use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

pub struct ProvidersView {
    /// Selected provider name from a predefined list (for Add)
    pub selected_provider: String,
    new_key: String,
    /// Additional secret key for providers like wenxin that need dual-auth
    new_secret_key: String,
    pub new_model: String,
    /// Label distinguishing multiple instances of the same provider
    pub new_label: String,
    /// Index of provider being updated in existing list (-1 = none)
    update_target: isize,
    status: String,
    sending: bool,
    pending_delete_confirmation: Option<usize>,
    pending_rx: mpsc::Receiver<String>,
    pending_tx: mpsc::SyncSender<String>,
    /// Models fetched from backend: provider → [model_id, ...]
    remote_models: std::collections::HashMap<String, Vec<String>>,
    /// Whether we've tried to fetch models
    models_loaded: bool,
    provider_ops_status: std::collections::HashMap<String, String>,
    provider_capabilities: std::collections::HashMap<String, Vec<ProviderCapabilityModel>>,
    /// Cached security prefs — reloaded at most once per 10s to avoid per-frame disk reads.
    cached_security: security_prefs::SecurityPrefs,
    security_last_load: Instant,

    // ── GitHub Copilot OAuth Device Code state ──
    /// Current step: None = idle, "requesting", "polling", "done", "error"
    copilot_device_state: Option<String>,
    /// The device_code returned by GitHub (for polling)
    copilot_device_code: String,
    /// The user_code displayed to the user
    copilot_user_code: String,
    /// The verification URI (e.g. https://github.com/login/device)
    copilot_verification_uri: String,
    /// Polling interval in seconds (from server)
    copilot_poll_interval: u64,
    /// Timestamp of last poll attempt
    copilot_last_poll: Instant,
    /// Number of poll attempts for current device flow
    copilot_poll_attempts: u64,
    /// Number of times GitHub requested slower polling
    copilot_slow_down_count: u64,
    /// Last poll result summary for debugging
    copilot_last_poll_result: String,
    /// Access token obtained after authorization
    copilot_access_token: String,
    /// Status message for the copilot auth section
    copilot_status: String,
    /// Flag to trigger config reload after copilot auth completes
    copilot_needs_reload: bool,
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
    // Chinese Vendors (16)
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
    "siliconflow",
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
            "gpt-4.1",
            "gpt-4.1-mini",
            "gpt-4.1-nano",
            "o1",
            "o3-mini",
            "gpt-4o",
            "gpt-4o-mini",
            "gpt-4-turbo",
            "gpt-3.5-turbo",
        ],
        "openai_compatible" => &["auto"],
        "anthropic" => &[
            "auto",
            "claude-sonnet-4-20250514",
            "claude-3-7-sonnet-latest",
            "claude-3-5-haiku-latest",
            "claude-3-5-sonnet-20241022",
            "claude-3-opus-20240229",
            "claude-3-haiku-20240307",
        ],
        "cohere" => &["auto", "command-r-plus", "command-r", "command-r7b"],
        "wenxin" => &["auto", "ERNIE-4.5-8K", "ERNIE-4.0", "ERNIE-3.5"],
        "qianfan" => &[
            "auto",
            "ERNIE-4.5-8K",
            "ERNIE-4.0-8K",
            "ERNIE-3.5-8K",
            "ERNIE-Speed-8K",
        ],
        "qwen" => &[
            "auto",
            "qwen-max",
            "qwen-plus",
            "qwen-turbo",
            "qwen2.5-72b-instruct",
        ],
        "glm" => &["auto", "glm-4-flash", "glm-4-plus"],
        "yi" => &["auto", "yi-lightning", "yi-large"],
        "hunyuan" => &["auto", "hunyuan-turbo-latest"],
        "doubao" => &["auto", "doubao-1.5-pro-32k-250115"],
        "gemini" => &[
            "auto",
            "gemini-2.5-flash",
            "gemini-2.5-pro",
            "gemini-2.0-flash",
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
        "copilot" => &["auto", "gpt-4o", "gpt-4o-mini"],
        "facewall" | "langboat" | "skywork" | "xihu" | "deepquest" | "fireworks" | "loopai"
        | "titan" => &["auto"],
        "stepfun" => &["auto", "step-2-16k", "step-1-32k"],
        "moonshot" => &["auto", "moonshot-v1-8k", "moonshot-v1-32k"],
        "minimax" => &["auto", "MiniMax-Text-01"],
        "ai21" => &["auto", "jamba-1.5-mini"],
        "aleph" => &["auto", "luminous-base"],
        "llama" => &["auto", "llama3.2", "llama3.1"],
        "nim" => &["auto", "meta/llama-3.1-70b-instruct"],
        "perplexity" => &["auto", "sonar-pro", "sonar"],
        "replicate" => &["auto", "meta/meta-llama-3-70b-instruct"],
        "together" => &["auto", "meta-llama/Meta-Llama-3.1-70B-Instruct-Turbo"],
        _ => &["auto"],
    }
}

fn provider_requires_secret(provider: &str) -> bool {
    matches!(provider.to_lowercase().as_str(), "wenxin" | "qianfan")
}

impl ProvidersView {
    pub fn new() -> Self {
        let (pending_tx, pending_rx) = mpsc::sync_channel(256);
        Self {
            selected_provider: PROVIDER_NAMES[0].to_string(),
            new_key: String::new(),
            new_secret_key: String::new(),
            new_model: "auto".to_string(),
            new_label: String::new(),
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
            copilot_status: String::new(),
            copilot_needs_reload: false,
        }
    }

    pub fn reset_loaded_state(&mut self) {
        self.models_loaded = false;
        self.remote_models.clear();
        self.provider_capabilities.clear();
        self.provider_ops_status.clear();
    }

    fn process_pending(&mut self, i18n: &I18n) {
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
                            self.new_key = self.copilot_access_token.clone();
                            // Request config reload to refresh monitoring status
                            self.copilot_needs_reload = true;
                            // Immediately persist to keyring so the token survives app restart
                            // even if the user doesn't click Save.
                            if let Err(e) = crate::keyring_util::store_api_key("copilot", token) {
                                eprintln!("Warning: failed to store Copilot token in keyring (copilot_api_key): {e}");
                            }
                            // Also write to github_copilot_token for backend compatibility
                            if let Err(e) = keyring::Entry::new("go-on", "github_copilot_token")
                                .and_then(|entry| entry.set_password(token))
                            {
                                eprintln!("Warning: failed to store Copilot token in keyring (github_copilot_token): {e}");
                            }
                            // Also set env vars so CopilotAgent can read them immediately
                            // if the backend process reads from inherited env.
                            std::env::set_var("GITHUB_TOKEN", token);
                            std::env::set_var("GITHUB_COPILOT_TOKEN", token);
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
                                // RFC 8628: increase polling interval by at least 5s on slow_down.
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
                let _ = tx.try_send(msg);
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
                self.process_pending(i18n);
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
                        // ── Secret key field (dual-auth providers) ──
                        if provider_requires_secret(&self.selected_provider) {
                            ui.label(i18n.t("providers.secret_key"));
                            ui.add(
                                egui::TextEdit::singleline(&mut self.new_secret_key)
                                    .password(true)
                                    .hint_text(i18n.t("providers.secret_key_placeholder"))
                                    .desired_width(260.0),
                            );
                        }
                        // Show auto-push hint when updating an existing provider
                        if self.update_target >= 0 {
                            ui.colored_label(
                                egui::Color32::from_rgb(100, 180, 100),
                                i18n.t("providers.auto_push_hint")
                            );
                        }
                        // Label field: required when adding a second instance of the same provider
                        ui.label(i18n.t("providers.label"));
                        let existing_same = config.providers.iter().filter(|p| p.name == self.selected_provider).count();
                        if existing_same > 0 {
                            ui.colored_label(
                                egui::Color32::from_rgb(220, 160, 50),
                                i18n.t("providers.labelRequiredHint"),
                            );
                        }
                        ui.add(
                            egui::TextEdit::singleline(&mut self.new_label)
                                .hint_text(if existing_same > 0 { i18n.t("providers.labelPlaceholderRequired") } else { i18n.t("providers.labelPlaceholder") })
                                .desired_width(120.0),
                        );
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
                        // ── Copilot Device Code authorization ──
                        if self.selected_provider.to_lowercase() == "copilot" {
                            ui.add_space(4.0);
                            ui.separator();
                            ui.add_space(2.0);

                            if self.copilot_device_state.is_none()
                                && ui.button(i18n.t("providers.copilot_authorize")).clicked()
                            {
                                self.copilot_device_state = Some("requesting".to_string());
                                self.copilot_status = i18n.t("providers.copilot_requesting").to_string();
                                let tx = self.pending_tx.clone();
                                let ctx_clone = ctx.clone();
                                tokio::spawn(async move {
                                    // Try common proxy ports, no env var dependency
                                    let proxy_urls = [
                                        "http://127.0.0.1:15732",
                                        "http://127.0.0.1:7890",
                                        "http://127.0.0.1:10809",
                                        "http://127.0.0.1:10808",
                                        "http://127.0.0.1:1080",
                                        "http://127.0.0.1:33210",
                                    ];
                                    let env_url = std::env::var("HTTPS_PROXY")
                                        .or_else(|_| std::env::var("https_proxy"))
                                        .ok();
                                    let mut client = reqwest::Client::new();
                                    for url in env_url
                                        .as_deref()
                                        .into_iter()
                                        .chain(proxy_urls.iter().copied())
                                    {
                                        if let Ok(p) = reqwest::Proxy::https(url) {
                                            if let Ok(c) = reqwest::Client::builder()
                                                .proxy(p)
                                                .build()
                                            {
                                                client = c;
                                                break;
                                            }
                                        }
                                    }
                                    let params = [
                                        ("client_id", "01ab8ac9400c4e429b23"),
                                        ("scope", "read:user,copilot"),
                                    ];
                                    match tokio::time::timeout(
                                        std::time::Duration::from_secs(15),
                                        client
                                            .post("https://github.com/login/device/code")
                                            .header("Accept", "application/json")
                                            .form(&params)
                                            .send(),
                                    )
                                    .await
                                    {
                                        Ok(Ok(resp)) if resp.status().is_success() => {
                                            match resp.json::<serde_json::Value>().await {
                                                Ok(body) => {
                                                    let msg = format!("__copilot_device__:{}", serde_json::to_string(&body).unwrap_or_default());
                                                    let _ = tx.try_send(msg);
                                                }
                                                Err(e) => {
                                                    let msg = format!("__copilot_device_err__:Parse error: {}", e);
                                                    let _ = tx.try_send(msg);
                                                }
                                            }
                                        }
                                        Ok(Ok(resp)) => {
                                            let status = resp.status();
                                            let text = resp.text().await.unwrap_or_default();
                                            let msg = format!("__copilot_device_err__:GitHub {status}: {text}");
                                            let _ = tx.try_send(msg);
                                        }
                                        Ok(Err(e)) => {
                                            let msg = format!("__copilot_device_err__:{}", e);
                                            let _ = tx.try_send(msg);
                                        }
                                        Err(_) => {
                                            let msg = "__copilot_device_err__:Request timed out.".to_string();
                                            let _ = tx.try_send(msg);
                                        }
                                    }
                                    ctx_clone.request_repaint_after(Duration::from_millis(16));
                                });
                            }

                            if let Some(state) = self.copilot_device_state.clone() {
                                let mut open = true;
                                egui::Window::new(i18n.t("providers.copilot_authorize"))
                                    .id(egui::Id::new("copilot_device_auth"))
                                    .open(&mut open)
                                    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                                    .resizable(false)
                                    .collapsible(false)
                                    .show(ui.ctx(), |ui| {
                                        match state.as_str() {
                                            "requesting" => {
                                                ui.horizontal(|ui| {
                                                    ui.spinner();
                                                    ui.label(i18n.t("providers.copilot_requesting"));
                                                });
                                            }
                                            "polling" => {
                                                ui.vertical(|ui| {
                                                    ui.heading(i18n.t("providers.copilot_authorize"));
                                                    ui.add_space(8.0);
                                                    ui.label(i18n.t("providers.copilot_open_url"));
                                                    if ui.link(&self.copilot_verification_uri).clicked() {
                                                        let _ = webbrowser::open(&self.copilot_verification_uri);
                                                    }
                                                    ui.add_space(4.0);
                                                    ui.horizontal(|ui| {
                                                        ui.label(i18n.t("providers.copilot_enter_code"));
                                                        ui.add(
                                                            egui::Label::new(
                                                                egui::RichText::new(&self.copilot_user_code)
                                                                    .size(28.0)
                                                                    .color(egui::Color32::from_rgb(60, 180, 100))
                                                                    .monospace()
                                                            )
                                                        );
                                                    });
                                                    ui.add_space(8.0);
                                                    ui.horizontal(|ui| {
                                                        ui.spinner();
                                                        ui.label(&self.copilot_status);
                                                    });
                                                    ui.add_space(6.0);
                                                    let last_poll_age = self.copilot_last_poll.elapsed().as_secs();
                                                    ui.small(format!(
                                                        "Debug: polls={}, interval={}s, slow_downs={}, last_poll={}s ago",
                                                        self.copilot_poll_attempts,
                                                        self.copilot_poll_interval,
                                                        self.copilot_slow_down_count,
                                                        last_poll_age
                                                    ));
                                                    if !self.copilot_last_poll_result.is_empty() {
                                                        ui.small(format!(
                                                            "Debug: last_result={}"
                                                            , self.copilot_last_poll_result
                                                        ));
                                                    }
                                                });
                                            }
                                            "done" => {
                                                ui.vertical(|ui| {
                                                    ui.colored_label(
                                                        egui::Color32::from_rgb(60, 180, 100),
                                                        i18n.t("providers.copilot_authorized"),
                                                    );
                                                    ui.add_space(4.0);
                                                    if !self.copilot_access_token.is_empty() {
                                                        let preview = if self.copilot_access_token.len() > 8 {
                                                            format!("{}...{}", &self.copilot_access_token[..4], &self.copilot_access_token[self.copilot_access_token.len()-4..])
                                                        } else {
                                                            "********".to_string()
                                                        };
                                                        ui.label(format!("{}: {}", i18n.t("providers.tokenPreview"), preview));
                                                    }
                                                    ui.add_space(8.0);
                                                    if ui.button(i18n.t("common.close")).clicked() {
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
                                                    if ui.button(i18n.t("providers.copilot_retry")).clicked() {
                                                        self.copilot_device_state = None;
                                                        self.copilot_status.clear();
                                                    }
                                                    if ui.button(i18n.t("common.close")).clicked() {
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

                            ui.add_space(4.0);
                            ui.separator();
                            ui.add_space(2.0);
                        }

                        let selected_is_copilot = self.selected_provider.to_lowercase() == "copilot";
                        let selected_requires_secret = provider_requires_secret(&self.selected_provider);
                        let can_add = if selected_is_copilot {
                            true
                        } else if selected_requires_secret {
                            !self.new_key.trim().is_empty() && !self.new_secret_key.trim().is_empty()
                        } else {
                            !self.new_key.trim().is_empty()
                        };

                        if ui
                            .add_enabled(can_add, egui::Button::new(i18n.t("providers.add")))
                            .clicked()
                        {
                            let name = self.selected_provider.clone();
                            let key: String = self.new_key
                                .chars()
                                .filter(|c| !c.is_control() || *c == '\t')
                                .collect::<String>()
                                .trim()
                                .to_string();
                            let secret_key: String = self.new_secret_key
                                .chars()
                                .filter(|c| !c.is_control() || *c == '\t')
                                .collect::<String>()
                                .trim()
                                .to_string();
                            let model = self.new_model.trim().to_string();
                            let provider_lower = name.to_lowercase();

                            if provider_requires_secret(&provider_lower) && secret_key.is_empty() {
                                self.status = i18n.t("providers.secret_key_placeholder").to_string();
                                return;
                            }

                            // Try keyring, but don't block the save if it fails
                            if let Err(e) =
                                crate::keyring_util::store_api_key(&provider_lower, &key)
                            {
                                eprintln!(
                                    "Warning: failed to store API key in system keyring: {}",
                                    e
                                );
                            }

                            if !secret_key.is_empty() {
                                if let Err(e) =
                                    crate::keyring_util::store_secret_key(&provider_lower, &secret_key)
                                {
                                    eprintln!(
                                        "Warning: failed to store secret key in system keyring: {}",
                                        e
                                    );
                                }
                            }

                            // Check if adding a duplicate provider name.
                            // If the user provides a label, it becomes a separate agent entry.
                            // If no label, update the existing matching provider (or the first unlabeled one).
                            let existing_count = config.providers.iter().filter(|p| p.name == name).count();
                            let label = self.new_label.trim().to_string();

                            if existing_count > 0 && label.is_empty() {
                                // No label provided, but multiple instances exist or would exist —
                                // update the first matching entry instead.
                                if !key.is_empty() {
                                    if let Some(existing) = config.providers.iter_mut().find(|p| p.name == name && p.label.is_empty()) {
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
                                        let push_secret_key = secret_key.clone();
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
                                        let has_secret = !push_secret_key.is_empty();
                                        tokio::spawn(async move {
                                            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                                            let result = if has_secret {
                                                tokio::time::timeout(
                                                    std::time::Duration::from_secs(10),
                                                    backend_clone.configure_provider_with_secret(&push_name, &push_key, &push_secret_key, &push_model),
                                                ).await
                                            } else {
                                                tokio::time::timeout(
                                                    std::time::Duration::from_secs(10),
                                                    backend_clone.configure_provider(&push_name, &push_key, &push_model),
                                                ).await
                                            };
                                            let msg = match result {
                                                Ok(Ok(_)) => ok_fmt,
                                                Ok(Err(e)) => err_fmt.replace("%s", &e),
                                                Err(_) => err_fmt.replace("%s", "timeout"),
                                            };
                                            let _ = tx.try_send(msg);
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
                                // New provider entry (possibly a labeled duplicate for multi-model)
                                let label_clean = label.replace(' ', "_");
                                config.providers.push(ProviderConfig {
                                    name: name.clone(),
                                    api_key: key.clone(),
                                    model: model.clone(),
                                    validated: true,
                                    label: label_clean.clone(),
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
                                    let push_secret_key = secret_key.clone();
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
                                    let has_secret = !push_secret_key.is_empty();
                                    tokio::spawn(async move {
                                        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                                        let result = if has_secret {
                                            tokio::time::timeout(
                                                std::time::Duration::from_secs(10),
                                                backend_clone.configure_provider_with_secret(&push_name, &push_key, &push_secret_key, &push_model),
                                            ).await
                                        } else {
                                            tokio::time::timeout(
                                                std::time::Duration::from_secs(10),
                                                backend_clone.configure_provider(&push_name, &push_key, &push_model),
                                            ).await
                                        };
                                        let msg = match result {
                                            Ok(Ok(_)) => ok_fmt,
                                            Ok(Err(e)) => err_fmt.replace("%s", &e),
                                            Err(_) => err_fmt.replace("%s", "timeout"),
                                        };
                                        let _ = tx.try_send(msg);
                                        ctx_clone.request_repaint_after(Duration::from_millis(16));
                                    });
                                }
                            }
                            self.new_key.clear();
                            self.new_secret_key.clear();
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
                            let display_name = if provider.label.is_empty() {
                                provider_label(i18n, &provider.name).to_string()
                            } else {
                                format!("{} ({})", provider_label(i18n, &provider.name), provider.label)
                            };
                            ui.label(&display_name);
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
                                                    let _ = tx_push.try_send(String::new());
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
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.new_key)
                                        .password(true)
                                        .hint_text(i18n.t("common.apiKeyPlaceholder"))
                                        .desired_width(260.0),
                                );
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

                                                let secret = crate::keyring_util::get_secret_key(&name.to_lowercase())
                                                    .unwrap_or_default();
                                                let has_secret = !secret.is_empty();
                                                let result = if has_secret {
                                                    match tokio::time::timeout(
                                                        std::time::Duration::from_secs(10),
                                                        backend_clone.configure_provider_with_secret(&name, &key, &secret, &model),
                                                    )
                                                    .await
                                                    {
                                                        Ok(r) => r,
                                                        Err(_) => {
                                                            #[cfg(debug_assertions)]
                                                            eprintln!("Warning: auto-push configure_provider timed out");
                                                            Err("timeout".to_string())
                                                        }
                                                    }
                                                } else {
                                                    match tokio::time::timeout(
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
                                                    }
                                                };
                                                let msg = match result {
                                                    Ok(_) => ok_fmt,
                                                    Err(e) => err_fmt.replace("%s", &e.to_string()),
                                                };
                                                let _ = tx.try_send(msg);
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
                                let secret = crate::keyring_util::get_secret_key(&name.to_lowercase())
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
                                    let has_secret = !secret.is_empty();
                                    let result = if has_secret {
                                        match tokio::time::timeout(
                                            std::time::Duration::from_secs(10),
                                            backend_clone.configure_provider_with_secret(&name, &key, &secret, &model),
                                        )
                                        .await
                                        {
                                            Ok(r) => r,
                                            Err(_) => {
                                                #[cfg(debug_assertions)]
                                                eprintln!("Warning: configure_provider timed out");
                                                Err("timeout".to_string())
                                            }
                                        }
                                    } else {
                                        match tokio::time::timeout(
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
                                        }
                                    };
                                    let msg = match result {
                                        Ok(_) => ok_fmt.replace("%s", &name),
                                        Err(e) => err_fmt.replace("%s", &e.to_string()),
                                    };
                                    let _ = tx.try_send(msg);
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
                                    let _ = tx.try_send(msg);
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
                                    let _ = tx.try_send(msg);
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
                                                let _ = tx.try_send(format!(
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
                                    let _ = tx.try_send(msg);
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
                        // Only delete keyring entry if this is the LAST provider with this name,
                        // otherwise other labeled instances would lose their shared key.
                        let remaining = config.providers.iter().filter(|p| p.name == removed.name).count();
                        if remaining <= 1 {
                            let _ = crate::keyring_util::delete_api_key(&removed.name.to_lowercase());
                        }
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

                // ── Trigger config reload after copilot auth ──
                if self.copilot_needs_reload {
                    self.copilot_needs_reload = false;
                    let tx = self.pending_tx.clone();
                    let backend_clone = backend.clone();
                    let ctx_clone = ctx.clone();
                    tokio::spawn(async move {
                        let _ = backend_clone.reload_config().await;
                        let _ = tx.try_send("Config reloaded for copilot.".to_string());
                        ctx_clone.request_repaint_after(Duration::from_millis(16));
                    });
                }

                // ── Copilot Device Code auto-poll (uses system proxy) ──
                if self.copilot_device_state.as_deref() == Some("polling") {
                    // Keep UI frames ticking so polling continues even when the user is idle.
                    ctx.request_repaint_after(Duration::from_millis(250));
                    let elapsed = self.copilot_last_poll.elapsed();
                    if elapsed >= Duration::from_secs(self.copilot_poll_interval) {
                        self.copilot_last_poll = Instant::now();
                        self.copilot_poll_attempts = self.copilot_poll_attempts.saturating_add(1);
                        let tx = self.pending_tx.clone();
                        let device_code = self.copilot_device_code.clone();
                        let ctx_clone = ctx.clone();
                        #[cfg(debug_assertions)]
                        {
                            let proxy_url = std::env::var("HTTPS_PROXY").unwrap_or_default();
                            eprintln!("[poll] device_code={}, HTTPS_PROXY={}", &device_code[..8.min(device_code.len())], proxy_url);
                        }
                        tokio::spawn(async move {
                            fn build_proxy_client() -> reqwest::Client {
                                let proxy_urls = [
                                    "http://127.0.0.1:15732",
                                    "http://127.0.0.1:7890",
                                    "socks5://127.0.0.1:7890",
                                    "http://127.0.0.1:10809",
                                    "http://127.0.0.1:10808",
                                    "http://127.0.0.1:1080",
                                    "http://127.0.0.1:33210",
                                ];
                                let env_url = std::env::var("HTTPS_PROXY")
                                    .or_else(|_| std::env::var("https_proxy"))
                                    .ok();
                                for url in env_url
                                    .as_deref()
                                    .into_iter()
                                    .chain(proxy_urls.iter().copied())
                                {
                                    if let Ok(p) = reqwest::Proxy::https(url) {
                                        if let Ok(client) = reqwest::Client::builder()
                                            .proxy(p)
                                            .build()
                                        {
                                            return client;
                                        }
                                    }
                                }
                                // Fallback: try with SSL verification disabled
                                // (some environments have corporate cert issues)
                                if let Ok(client) = reqwest::Client::builder()
                                    .danger_accept_invalid_certs(true)
                                    .build()
                                {
                                    eprintln!(
                                        "WARNING: copilot poll falling back to dangerous SSL (no certificate verification)"
                                    );
                                    return client;
                                }
                                reqwest::Client::new()
                            }
                            static COPILOT_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
                            let poll_client = COPILOT_CLIENT.get_or_init(build_proxy_client);
                            let poll_params = [
                                ("client_id", "01ab8ac9400c4e429b23"),
                                ("device_code", &device_code),
                                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                            ];
                            match tokio::time::timeout(
                                std::time::Duration::from_secs(15),
                                        poll_client
                                            .post("https://github.com/login/oauth/access_token")
                                            .header("Accept", "application/json")
                                            .form(&poll_params)
                                            .send(),
                                    )
                                    .await
                                    {
                                Ok(Ok(resp)) => {
                                    match resp.json::<serde_json::Value>().await {
                                        Ok(body) => {
                                            let msg = format!("__copilot_poll__:{}", serde_json::to_string(&body).unwrap_or_default());
                                            let _ = tx.try_send(msg);
                                        }
                                        Err(e) => {
                                            let msg = format!("__copilot_poll_err__:Parse error: {}", e);
                                            let _ = tx.try_send(msg);
                                        }
                                    }
                                }
                                Ok(Err(e)) => {
                                    let msg = format!("__copilot_poll_err__:{}", e);
                                    let _ = tx.try_send(msg);
                                }
                                Err(_) => {
                                    let msg = "__copilot_poll_err__:Poll timed out.".to_string();
                                    let _ = tx.try_send(msg);
                                }
                            }
                            ctx_clone.request_repaint_after(Duration::from_millis(16));
                        });
                    }
                }
            });

        changed
    }
}
