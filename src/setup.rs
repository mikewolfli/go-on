//! setup.rs
//! Auto-generated English doc: module overview.
//!
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use crate::i18n::{t, tf};
use anyhow::{Context, Result};

// 简化模式和复杂模式配置文件模板名称
const SIMPLE_TEMPLATE: &str = "config.toml.autopilot-simple";
const COMPLEX_TEMPLATE: &str = "config.toml.autopilot-complex";

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

/// Setup profile mode: simple autopilot or complex autopilot.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SetupProfile {
    Simple,
    Complex,
}

/// Secret storage mode for setup: environment variables or system keyring.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SecretMode {
    Env,
    Keyring,
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

    let profile = match options.profile {
        Some(value) => value,
        None => match prompt_choice(&t("setup.prompt_profile"), &["1", "2"], "1")?.as_str() {
            "2" => SetupProfile::Complex,
            _ => SetupProfile::Simple,
        },
    };
    let template_name = match profile {
        SetupProfile::Simple => SIMPLE_TEMPLATE,
        SetupProfile::Complex => COMPLEX_TEMPLATE,
    };

    let template_path = find_template(template_name)
        .ok_or_else(|| anyhow::anyhow!("template file '{}' not found", template_name))?;
    let mut content = fs::read_to_string(&template_path)
        .with_context(|| format!("failed to read setup template: {}", template_path.display()))?;

    let secret_mode = match options.secret_mode {
        Some(value) => value,
        None => match prompt_choice(&t("setup.prompt_secret_mode"), &["1", "2"], "2")?.as_str() {
            "1" => SecretMode::Env,
            _ => SecretMode::Keyring,
        },
    };

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

    let should_store_secrets = secret_mode == SecretMode::Keyring
        && if options.prompt_for_secrets {
            prompt_yes_no(&t("setup.prompt_store_secrets"), true)?
        } else {
            false
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
/// Accepts case-insensitive "simple" or "complex".
pub fn parse_setup_profile(value: &str) -> Result<SetupProfile> {
    if value.eq_ignore_ascii_case("simple") {
        return Ok(SetupProfile::Simple);
    }
    if value.eq_ignore_ascii_case("complex") {
        return Ok(SetupProfile::Complex);
    }
    anyhow::bail!(
        "{}",
        crate::i18n::tf("error.invalid_setup_profile", &[("value", value)])
    )
}

/// Parse secret mode string to SecretMode enum.
///
/// Accepts case-insensitive "env" or "keyring".
pub fn parse_secret_mode(value: &str) -> Result<SecretMode> {
    if value.eq_ignore_ascii_case("env") {
        return Ok(SecretMode::Env);
    }
    if value.eq_ignore_ascii_case("keyring") {
        return Ok(SecretMode::Keyring);
    }
    anyhow::bail!(
        "{}",
        crate::i18n::tf("error.invalid_secret_mode", &[("value", value)])
    )
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
    anyhow::bail!(
        "{}",
        crate::i18n::tf("error.invalid_secret_action", &[("value", value)])
    )
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
            let value = value.ok_or_else(|| {
                anyhow::anyhow!("{}", crate::i18n::t("error.secret_value_required"))
            })?;
            let entry = keyring::Entry::new(service, account).map_err(|err| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf("error.keyring_open", &[("error", &format!("{}", err))])
                )
            })?;
            entry.set_password(value).map_err(|err| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf("error.keyring_write", &[("error", &format!("{}", err))])
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
                    crate::i18n::tf("error.keyring_open", &[("error", &format!("{}", err))])
                )
            })?;
            let secret = entry.get_password().map_err(|err| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf("error.keyring_read", &[("error", &format!("{}", err))])
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
                    crate::i18n::tf("error.keyring_open", &[("error", &format!("{}", err))])
                )
            })?;
            entry.delete_credential().map_err(|err| {
                anyhow::anyhow!(
                    "{}",
                    crate::i18n::tf("error.keyring_delete", &[("error", &format!("{}", err))])
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
        .ok_or_else(|| {
            anyhow::anyhow!(
                "{}",
                crate::i18n::tf("error.unknown_secret_name", &[("name", name)])
            )
        })
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
            crate::i18n::tf("warning.invalid_value", &[("allowed", &allowed.join(", "))])
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
            parse_setup_profile("simple").unwrap(),
            SetupProfile::Simple
        ));
        assert!(matches!(
            parse_setup_profile("complex").unwrap(),
            SetupProfile::Complex
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
