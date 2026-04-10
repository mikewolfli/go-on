//! setup.rs
//! Auto-generated English doc: module overview.
//!
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::config::AdaptiveConfig;
use crate::i18n::runtime::{t, tf};
use anyhow::{Context, Result};

// 自适应配置模板名称
const ADAPTIVE_TEMPLATE: &str = "config.toml.autopilot-adaptive";

// AI提供商列表和对应的环境变量
const AI_PROVIDERS: &[(&str, &[&str])] = &[
    ("openai", &["OPENAI_API_KEY"]),
    ("anthropic", &["ANTHROPIC_API_KEY"]),
    ("deepseek", &["DEEPSEEK_API_KEY"]),
    ("wenxin", &["WENXIN_API_KEY", "WENXIN_SECRET_KEY"]),
    ("doubao", &["DOUBAO_API_KEY"]),
    ("copilot", &[]), // 本地运行，不需要API密钥
];

// Secret key targets used for keyring operations.
// Each tuple is (command name, keyring service, keyring account).
const SECRET_TARGETS: &[(&str, &str, &str)] = &[
    ("deepseek_api_key", "go-on", "deepseek_api_key"),
    ("wenxin_api_key", "go-on", "wenxin_api_key"),
    ("wenxin_secret_key", "go-on", "wenxin_secret_key"),
    ("anthropic_api_key", "go-on", "anthropic_api_key"),
    ("doubao_api_key", "go-on", "doubao_api_key"),
    (
        "openai_compatible_api_key",
        "go-on",
        "openai_compatible_api_key",
    ),
];

/// Setup profile mode: adaptive autopilot with AI-driven configuration.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SetupProfile {
    Adaptive,
}

/// Secret storage mode for setup: environment variables or system keyring.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SecretMode {
    Env,
    Keyring,
    AutoDetect,
}

/// Options controlling go-on setup behavior.
///
/// - `profile`: chosen setup profile; if None, user is prompted.
/// - `secret_mode`: how to store secrets; if None, user is prompted.
/// - `force`: overwrite existing config without prompting when true.
/// - `prompt_for_secrets`: whether to ask to set keyring secrets immediately.
pub struct SetupOptions {
    pub profile: Option<SetupProfile>,
    pub secret_mode: Option<SecretMode>,
    pub force: bool,
    pub prompt_for_secrets: bool,
}

impl Default for SetupOptions {
    fn default() -> Self {
        Self {
            profile: None,
            secret_mode: None,
            force: false,
            prompt_for_secrets: true,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SecretAction {
    Set,
    Get,
    Delete,
    List,
}

#[allow(dead_code)]
/// Run setup with default options.
///
/// This helper is a thin wrapper around `run_setup_with_options`.
pub fn run_setup(config_path: &Path) -> Result<()> {
    run_setup_with_options(config_path, SetupOptions::default())
}

/// Entry point for setup logic.
///
/// Handles profile selection, secret mode, writing config template, writing RULES files,
/// and optionally storing secrets into keyring.
pub fn run_setup_with_options(config_path: &Path, options: SetupOptions) -> Result<()> {
    println!("{}", t("setup.title"));
    println!(
        "{}",
        tf(
            "setup.target_config",
            &[("path", &config_path.display().to_string())]
        )
    );

    if config_path.exists()
        && !options.force
        && !prompt_yes_no(&t("setup.prompt_overwrite"), false)?
    {
        println!("{}", t("setup.canceled"));
        return Ok(());
    }

    let _profile = options.profile.unwrap_or(SetupProfile::Adaptive);
    let template_name = ADAPTIVE_TEMPLATE;

    let _template_path = find_template(template_name)
        .ok_or_else(|| anyhow::anyhow!("template file '{}' not found", template_name))?;

    let secret_mode = match options.secret_mode {
        Some(value) => value,
        None => {
            // 自动检测：如果已有环境变量，使用Env模式，否则询问
            let has_env_vars = !detect_available_providers_from_env().is_empty();
            if has_env_vars {
                println!("{}", t("setup.auto_detected_env_vars"));
                SecretMode::Env
            } else {
                match prompt_choice(&t("setup.prompt_secret_mode"), &["1", "2", "3"], "3")?.as_str()
                {
                    "1" => SecretMode::Env,
                    "2" => SecretMode::Keyring,
                    _ => SecretMode::AutoDetect,
                }
            }
        }
    };

    // 检测可用的AI提供商
    let available_providers = detect_available_providers(&secret_mode);

    let mut adaptive_config = AdaptiveConfig::auto_detect();
    if available_providers.is_empty() {
        println!("{}", t("setup.no_providers_detected"));
        println!("{}", t("setup.setup_copilot_only"));
        adaptive_config.minimal_config.available_providers = vec!["copilot".to_string()];
    } else {
        println!("{}", t("setup.detected_providers"));
        for provider in &available_providers {
            println!("  - {}", provider);
        }
        adaptive_config.minimal_config.available_providers = available_providers;
    }

    let mut content = generate_adaptive_config_toml(&adaptive_config, &secret_mode);

    // 如果使用keyring模式，转换环境变量占位符
    if secret_mode == SecretMode::Keyring {
        content = convert_env_placeholders_to_keyring(&content);
    }

    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory: {}", parent.display()))?;
    }
    fs::write(config_path, content)
        .with_context(|| format!("failed to write config file: {}", config_path.display()))?;

    write_default_rules(config_path.parent().unwrap_or_else(|| Path::new(".")))?;

    let should_store_secrets = match secret_mode {
        SecretMode::Keyring => {
            if options.prompt_for_secrets {
                prompt_yes_no(&t("setup.prompt_store_secrets"), true)?
            } else {
                false
            }
        }

        SecretMode::AutoDetect => {
            // 自动检测模式下，询问是否要设置API密钥
            prompt_yes_no(&t("setup.prompt_setup_api_keys"), true)?
        }
        _ => false,
    };

    if should_store_secrets {
        store_keyring_secrets_interactive()?;
    }

    println!("{}", t("setup.complete"));
    println!(
        "{}",
        tf(
            "setup.next_step",
            &[("config", &config_path.to_string_lossy())]
        )
    );
    Ok(())
}

/// Parse setup profile string to SetupProfile enum.
///
/// Accepts case-insensitive "adaptive".
pub fn parse_setup_profile(value: &str) -> Result<SetupProfile> {
    if value.eq_ignore_ascii_case("adaptive") {
        return Ok(SetupProfile::Adaptive);
    }
    anyhow::bail!("{}", tf("error.invalid_setup_profile", &[("value", value)]))
}

/// Parse secret mode string to SecretMode enum.
///
/// Accepts case-insensitive "env", "keyring", or "auto".
pub fn parse_secret_mode(value: &str) -> Result<SecretMode> {
    if value.eq_ignore_ascii_case("env") {
        return Ok(SecretMode::Env);
    }
    if value.eq_ignore_ascii_case("keyring") {
        return Ok(SecretMode::Keyring);
    }
    if value.eq_ignore_ascii_case("auto") || value.eq_ignore_ascii_case("autodetect") {
        return Ok(SecretMode::AutoDetect);
    }
    anyhow::bail!("{}", tf("error.invalid_secret_mode", &[("value", value)]))
}

/// Parse secret action string to SecretAction enum.
///
/// Accepts case-insensitive set|get|delete|list.
pub fn parse_secret_action(value: &str) -> Result<SecretAction> {
    if value.eq_ignore_ascii_case("set") {
        return Ok(SecretAction::Set);
    }
    if value.eq_ignore_ascii_case("get") {
        return Ok(SecretAction::Get);
    }
    if value.eq_ignore_ascii_case("delete") {
        return Ok(SecretAction::Delete);
    }
    if value.eq_ignore_ascii_case("list") {
        return Ok(SecretAction::List);
    }
    anyhow::bail!("{}", tf("error.invalid_secret_action", &[("value", value)]))
}

pub fn run_secret_command(
    action: SecretAction,
    name: Option<&str>,
    value: Option<&str>,
) -> Result<()> {
    match action {
        SecretAction::List => {
            for (name, service, account) in SECRET_TARGETS {
                let entry = keyring::Entry::new(service, account)
                    .map_err(|err| anyhow::anyhow!("failed to open keyring entry: {}", err))?;
                let status = if entry.get_password().is_ok() {
                    "present"
                } else {
                    "missing"
                };
                println!(
                    "{}",
                    tf("setup.secret_status", &[("name", name), ("status", status)])
                );
            }
            Ok(())
        }
        SecretAction::Set => {
            let (service, account) = resolve_secret_target(name)?;
            let value =
                value.ok_or_else(|| anyhow::anyhow!("{}", t("error.secret_value_required")))?;
            let entry = keyring::Entry::new(service, account).map_err(|err| {
                anyhow::anyhow!(
                    "{}",
                    tf("error.keyring_open", &[("error", &format!("{}", err))])
                )
            })?;
            entry.set_password(value).map_err(|err| {
                anyhow::anyhow!(
                    "{}",
                    tf("error.keyring_write", &[("error", &format!("{}", err))])
                )
            })?;
            println!(
                "{}",
                tf("setup.secret_stored", &[("name", name.unwrap_or_default())])
            );
            Ok(())
        }
        SecretAction::Get => {
            let (service, account) = resolve_secret_target(name)?;
            let entry = keyring::Entry::new(service, account).map_err(|err| {
                anyhow::anyhow!(
                    "{}",
                    tf("error.keyring_open", &[("error", &format!("{}", err))])
                )
            })?;
            let secret = entry.get_password().map_err(|err| {
                anyhow::anyhow!(
                    "{}",
                    tf("error.keyring_read", &[("error", &format!("{}", err))])
                )
            })?;
            println!("{}", secret);
            Ok(())
        }
        SecretAction::Delete => {
            let (service, account) = resolve_secret_target(name)?;
            let entry = keyring::Entry::new(service, account).map_err(|err| {
                anyhow::anyhow!(
                    "{}",
                    tf("error.keyring_open", &[("error", &format!("{}", err))])
                )
            })?;
            entry.delete_credential().map_err(|err| {
                anyhow::anyhow!(
                    "{}",
                    tf("error.keyring_delete", &[("error", &format!("{}", err))])
                )
            })?;
            println!(
                "{}",
                tf(
                    "setup.secret_deleted",
                    &[("name", name.unwrap_or_default())]
                )
            );
            Ok(())
        }
    }
}

fn detect_available_providers(secret_mode: &SecretMode) -> Vec<String> {
    let env_providers = detect_available_providers_from_env();
    let keyring_providers = detect_available_providers_from_keyring();

    let mut providers = Vec::new();
    for (provider, _) in AI_PROVIDERS {
        if *provider == "copilot" {
            continue;
        }

        let include = match secret_mode {
            SecretMode::Env => env_providers.iter().any(|item| item == provider),
            SecretMode::Keyring => keyring_providers.iter().any(|item| item == provider),
            SecretMode::AutoDetect => {
                env_providers.iter().any(|item| item == provider)
                    || keyring_providers.iter().any(|item| item == provider)
            }
        };

        if include {
            providers.push((*provider).to_string());
        }
    }

    if providers.is_empty() {
        providers.push("copilot".to_string());
    }

    providers
}

fn detect_available_providers_from_env() -> Vec<String> {
    AI_PROVIDERS
        .iter()
        .filter(|(provider, required_envs)| {
            *provider != "copilot"
                && !required_envs.is_empty()
                && required_envs.iter().all(|name| std::env::var(name).is_ok())
        })
        .map(|(provider, _)| (*provider).to_string())
        .collect()
}

fn detect_available_providers_from_keyring() -> Vec<String> {
    AI_PROVIDERS
        .iter()
        .filter(|(provider, required_envs)| {
            *provider != "copilot"
                && !required_envs.is_empty()
                && required_envs
                    .iter()
                    .all(|env_name| keyring_secret_available(env_name))
        })
        .map(|(provider, _)| (*provider).to_string())
        .collect()
}

fn keyring_secret_available(env_name: &str) -> bool {
    let Some((service, account)) = keyring_target_for_env(env_name) else {
        return false;
    };

    keyring::Entry::new(service, account)
        .and_then(|entry| entry.get_password())
        .is_ok()
}

fn keyring_target_for_env(env_name: &str) -> Option<(&'static str, &'static str)> {
    match env_name {
        "DEEPSEEK_API_KEY" => Some(("go-on", "deepseek_api_key")),
        "WENXIN_API_KEY" => Some(("go-on", "wenxin_api_key")),
        "WENXIN_SECRET_KEY" => Some(("go-on", "wenxin_secret_key")),
        "ANTHROPIC_API_KEY" => Some(("go-on", "anthropic_api_key")),
        "DOUBAO_API_KEY" => Some(("go-on", "doubao_api_key")),
        "OPENAI_API_KEY" => Some(("go-on", "openai_compatible_api_key")),
        _ => None,
    }
}

fn secret_reference(env_name: &str, secret_mode: &SecretMode) -> String {
    match secret_mode {
        SecretMode::Env => env_name.to_string(),
        SecretMode::Keyring => keyring_reference(env_name).unwrap_or_else(|| env_name.to_string()),
        SecretMode::AutoDetect => {
            if std::env::var(env_name).is_ok() {
                env_name.to_string()
            } else {
                keyring_reference(env_name).unwrap_or_else(|| env_name.to_string())
            }
        }
    }
}

fn keyring_reference(env_name: &str) -> Option<String> {
    keyring_target_for_env(env_name)
        .map(|(service, account)| format!("keyring://{}/{}", service, account))
}

fn generate_adaptive_config_toml(
    adaptive_config: &AdaptiveConfig,
    secret_mode: &SecretMode,
) -> String {
    let providers = if adaptive_config
        .minimal_config
        .available_providers
        .is_empty()
    {
        vec!["copilot".to_string()]
    } else {
        adaptive_config.minimal_config.available_providers.clone()
    };
    let review_agents = non_copilot_agents(&providers);
    let delivery_agents = if providers.iter().any(|item| item == "copilot") {
        vec!["copilot".to_string()]
    } else {
        vec![providers[0].clone()]
    };

    let mut content = String::new();
    content.push_str(&format!(
        "default_phase = \"{}\"\nmodel_selection_mode = \"adaptive\"\n\n",
        adaptive_config.minimal_config.default_phase
    ));

    if adaptive_config.minimal_config.enable_cache {
        content.push_str(
            "[cache]\nenabled = true\npath = \"acp_cache.sqlite3\"\ndefault_ttl_seconds = 3600\nmax_entries = 5000\n\n",
        );
    }

    if adaptive_config.minimal_config.enable_vector_memory {
        content.push_str(
            "[vector]\nenabled = true\nauto_mode = true\npath = \"acp_vector.sqlite3\"\ndimensions = 192\nmin_query_chars = 80\ntop_k = 2\nmin_similarity = 0.82\nmax_snippet_chars = 800\nmax_entries = 10000\nsummary_enabled = true\nsummary_trigger_messages = 8\nsummary_max_chars = 1200\n\n",
        );
    }

    content.push_str(
        "[runtime]\nmaintenance_interval_seconds = 60\nhealth_interval_seconds = 120\nshutdown_drain_seconds = 30\nsqlite_vacuum_interval_cycles = 60\n\n",
    );

    for provider in &providers {
        append_agent_block(&mut content, provider, secret_mode);
    }

    content.push_str("[flow]\nname = \"Autopilot Adaptive\"\nphases = [\"planning\", \"coding\", \"review\", \"delivery\"]\n\n");
    content.push_str(&format!(
        "[phases.planning]\ndescription = \"Adaptive planning phase\"\nagents = {}\nfallback = true\nprinciples = [\"Choose the smallest correct plan\", \"Use the available agents adaptively\"]\n\n",
        toml_array(&providers)
    ));
    content.push_str(&format!(
        "[phases.coding]\ndescription = \"Adaptive coding phase\"\nagents = {}\nfallback = true\nprinciples = [\"Make the smallest correct change\", \"Do not claim done without verification\"]\n\n",
        toml_array(&providers)
    ));
    content.push_str(&format!(
        "[phases.coding.options]\nautopilot_complexity = \"auto\"\nrequest_timeout_seconds = 150\nreview_timeout_seconds = 60\ncache_enabled = {}\nvector_enabled = {}\nsummary_enabled = {}\nfull_auto_review_agents = {}\nphase_max_inflight = 24\nglobal_max_inflight = 128\n\n",
        adaptive_config.minimal_config.enable_cache,
        adaptive_config.minimal_config.enable_vector_memory,
        adaptive_config.minimal_config.enable_vector_memory,
        toml_array(&review_agents)
    ));
    content.push_str(&format!(
        "[phases.review]\ndescription = \"Adaptive review phase\"\nagents = {}\nfallback = true\n\n",
        toml_array(&review_agents)
    ));
    content.push_str(
        "[phases.review.options]\nrequest_timeout_seconds = 60\nreview_timeout_policy = \"reject\"\nreview_min_response_chars = 12\n\n",
    );
    content.push_str(&format!(
        "[phases.delivery]\ndescription = \"Adaptive delivery phase\"\nagents = {}\nfallback = false\n",
        toml_array(&delivery_agents)
    ));

    content
}

fn append_agent_block(content: &mut String, provider: &str, secret_mode: &SecretMode) {
    match provider {
        "copilot" => {
            content.push_str(
                "[agents.copilot]\ntype = \"copilot\"\nurl = \"http://127.0.0.1:8080\"\n\n",
            );
        }
        "deepseek" => {
            content.push_str(&format!(
                "[agents.deepseek]\ntype = \"deepseek\"\napi_key_env = \"{}\"\nmodel = \"deepseek-chat\"\n\n",
                secret_reference("DEEPSEEK_API_KEY", secret_mode)
            ));
        }
        "wenxin" => {
            content.push_str(&format!(
                "[agents.wenxin]\ntype = \"wenxin\"\napi_key_env = \"{}\"\nsecret_key_env = \"{}\"\nmodel = \"ERNIE-Bot\"\n\n",
                secret_reference("WENXIN_API_KEY", secret_mode),
                secret_reference("WENXIN_SECRET_KEY", secret_mode)
            ));
        }
        "anthropic" => {
            content.push_str(&format!(
                "[agents.anthropic]\ntype = \"claude\"\nurl = \"https://api.anthropic.com\"\napi_key_env = \"{}\"\nmodel = \"claude-3-7-sonnet-latest\"\nanthropic_version = \"2023-06-01\"\nmax_tokens = 4096\n\n",
                secret_reference("ANTHROPIC_API_KEY", secret_mode)
            ));
        }
        "openai" => {
            content.push_str(&format!(
                "[agents.openai]\ntype = \"openai\"\nurl = \"https://api.openai.com/v1\"\napi_key_env = \"{}\"\nmodel = \"gpt-4o-mini\"\nsupports_system = true\n\n",
                secret_reference("OPENAI_API_KEY", secret_mode)
            ));
        }
        "doubao" => {
            content.push_str(&format!(
                "[agents.doubao]\ntype = \"doubao\"\nurl = \"https://ark.cn-beijing.volces.com/api/v3\"\nchat_path = \"/chat/completions\"\napi_key_env = \"{}\"\nmodel = \"doubao-1-5-pro-32k-250115\"\nsupports_system = true\n\n",
                secret_reference("DOUBAO_API_KEY", secret_mode)
            ));
        }
        _ => {}
    }
}

fn non_copilot_agents(providers: &[String]) -> Vec<String> {
    let filtered: Vec<String> = providers
        .iter()
        .filter(|item| item.as_str() != "copilot")
        .cloned()
        .collect();

    if filtered.is_empty() {
        providers.to_vec()
    } else {
        filtered
    }
}

fn toml_array(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|item| format!("\"{}\"", item)).collect();
    format!("[{}]", quoted.join(", "))
}

/// Locate setup template file for the provided template name.
///
/// Search order:
/// 1. directory containing current executable
/// 2. current working directory
///
/// Returns first existing match.
fn find_template(name: &str) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(name));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join(name));
    }

    candidates.into_iter().find(|path| path.exists())
}

/// Convert known env variable placeholder names in template content into keyring references.
///
/// This is used when setup is requested with keyring secret mode.
fn convert_env_placeholders_to_keyring(content: &str) -> String {
    let mappings = [
        ("DEEPSEEK_API_KEY", "keyring://go-on/deepseek_api_key"),
        ("WENXIN_API_KEY", "keyring://go-on/wenxin_api_key"),
        ("WENXIN_SECRET_KEY", "keyring://go-on/wenxin_secret_key"),
        ("ANTHROPIC_API_KEY", "keyring://go-on/anthropic_api_key"),
        ("DOUBAO_API_KEY", "keyring://go-on/doubao_api_key"),
        (
            "OTHER_PROVIDER_API_KEY",
            "keyring://go-on/openai_compatible_api_key",
        ),
    ];

    let mut out = content.to_string();
    for (from, to) in mappings {
        out = out.replace(from, to);
    }
    out
}

/// Create default RULES files in the provided config directory.
///
/// This ensures baseline rule overlay files exist for policy and review behavior.
fn write_default_rules(config_dir: &Path) -> Result<()> {
    let rules_dir = config_dir.join("RULES");
    fs::create_dir_all(&rules_dir)
        .with_context(|| format!("failed to create RULES directory: {}", rules_dir.display()))?;

    write_if_missing(
        &config_dir.join("RULES.md"),
        "# Project Rule Overlay\n\n- Keep ACP protocol compatibility stable.\n- Favor safe and test-backed changes.\n",
    )?;
    write_if_missing(
        &rules_dir.join("global.md"),
        "# Global Rules\n\n- Preserve runtime safety and observability.\n- Do not leak secrets in logs or responses.\n",
    )?;
    write_if_missing(
        &rules_dir.join("local.md"),
        "# Local Overlay\n\n- Add machine or developer local overrides here.\n",
    )?;
    write_if_missing(
        &rules_dir.join("coding.md"),
        "# Coding Rules\n\n- Keep changes minimal and reviewable.\n- Add tests for non-trivial logic updates.\n",
    )?;
    write_if_missing(
        &rules_dir.join("review.md"),
        "# Review Rules\n\n- Enforce strict completeness: no placeholders, no TODO-only branches, and no unhandled errors.\n- Require evidence-backed review outcomes for non-trivial changes.\n",
    )?;

    Ok(())
}

/// Write content to file only if the file does not already exist.
fn write_if_missing(path: &Path, content: &str) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    fs::write(path, content)
        .with_context(|| format!("failed to write file: {}", path.display()))?;
    Ok(())
}

/// Interactive flow to store all configured secrets into system keyring.
///
/// Prompts user for each secret key and stores non-empty values.
fn store_keyring_secrets_interactive() -> Result<()> {
    println!("{}", t("setup.enter_secrets"));
    for (name, service, account) in SECRET_TARGETS {
        let value = prompt_value(name)?;
        if value.trim().is_empty() {
            continue;
        }

        let entry = keyring::Entry::new(service, account)
            .map_err(|err| anyhow::anyhow!("failed to open keyring entry: {}", err))?;
        entry
            .set_password(value.trim())
            .map_err(|err| anyhow::anyhow!("failed to write keyring entry: {}", err))?;
    }

    Ok(())
}

/// Resolve secret command name to keyring service/account.
/// Used by run_secret_command handlers to map human-readable secret names.
fn resolve_secret_target(name: Option<&str>) -> Result<(&'static str, &'static str)> {
    let name = name.ok_or_else(|| anyhow::anyhow!("--secret-name is required"))?;
    SECRET_TARGETS
        .iter()
        .find(|(known_name, _, _)| *known_name == name)
        .map(|(_, service, account)| (*service, *account))
        .ok_or_else(|| anyhow::anyhow!("{}", tf("error.unknown_secret_name", &[("name", name)])))
}

fn prompt_choice(prompt: &str, allowed: &[&str], default: &str) -> Result<String> {
    loop {
        print!("{} [{}]: ", prompt, default);
        io::stdout().flush().context("failed to flush stdout")?;

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .context("failed to read input")?;
        let value = {
            let trimmed = input.trim();
            if trimmed.is_empty() {
                default.to_string()
            } else {
                trimmed.to_string()
            }
        };

        if allowed.iter().any(|item| *item == value) {
            return Ok(value);
        }

        println!(
            "{}",
            tf("warning.invalid_value", &[("allowed", &allowed.join(", "))])
        );
    }
}

fn prompt_yes_no(prompt: &str, default_yes: bool) -> Result<bool> {
    let default = if default_yes { "Y/n" } else { "y/N" };
    loop {
        print!("{} [{}]: ", prompt, default);
        io::stdout().flush().context("failed to flush stdout")?;

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .context("failed to read input")?;
        let trimmed = input.trim();

        if trimmed.is_empty() {
            return Ok(default_yes);
        }
        if trimmed.eq_ignore_ascii_case("y") || trimmed.eq_ignore_ascii_case("yes") {
            return Ok(true);
        }
        if trimmed.eq_ignore_ascii_case("n") || trimmed.eq_ignore_ascii_case("no") {
            return Ok(false);
        }
    }
}

fn prompt_value(prompt: &str) -> Result<String> {
    print!("{}: ", prompt);
    io::stdout().flush().context("failed to flush stdout")?;
    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .context("failed to read input")?;
    Ok(input.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        convert_env_placeholders_to_keyring, parse_secret_action, parse_secret_mode,
        parse_setup_profile, SecretAction, SecretMode, SetupProfile,
    };

    #[test]
    fn converts_known_env_vars_to_keyring_refs() {
        let input = "api_key_env = \"DEEPSEEK_API_KEY\"\nsecret_key_env = \"WENXIN_SECRET_KEY\"\n";
        let out = convert_env_placeholders_to_keyring(input);
        assert!(out.contains("keyring://go-on/deepseek_api_key"));
        assert!(out.contains("keyring://go-on/wenxin_secret_key"));
    }

    #[test]
    fn parses_setup_profile_secret_mode_and_action() {
        assert!(matches!(
            parse_setup_profile("adaptive").unwrap(),
            SetupProfile::Adaptive
        ));
        assert!(matches!(
            parse_secret_mode("auto").unwrap(),
            SecretMode::AutoDetect
        ));
        assert!(matches!(parse_secret_mode("env").unwrap(), SecretMode::Env));
        assert!(matches!(
            parse_secret_mode("keyring").unwrap(),
            SecretMode::Keyring
        ));
        assert!(matches!(
            parse_secret_action("list").unwrap(),
            SecretAction::List
        ));
    }
}
