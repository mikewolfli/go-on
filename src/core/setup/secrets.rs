//! Secret management sub-module (GAP-B53-23).
//!
//! Contains all secret-related logic: keyring access, env-var resolution,
//! secret pool management, and the CLI `go-on setup secret` subcommands.
//!
//! Functions that cross-reference items in the parent `setup` module use
//! `super::` to refer to them.

use std::collections::BTreeSet;

use crate::i18n::runtime::{t, tf};
use anyhow::Result;

// ---------------------------------------------------------------------------
// Re-exports / types
// ---------------------------------------------------------------------------

/// Secret storage mode for setup: environment variables or system keyring.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SecretMode {
    Env,
    Keyring,
    AutoDetect,
}

/// Action to perform on a secret: set, get, delete, or list.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SecretAction {
    Set,
    Get,
    Delete,
    List,
}

// ---------------------------------------------------------------------------
// Public API — CLI-facing functions
// ---------------------------------------------------------------------------

/// Parse secret mode string to `SecretMode` enum.
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

/// Parse secret action string to `SecretAction` enum.
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

/// Run a secret command (set/get/delete/list) against the OS keyring.
///
/// # Sync boundary
///
/// This function performs synchronous keyring I/O via the `keyring` crate.
/// When called from an async context (e.g. a tokio task), the caller MUST
/// wrap this call in `tokio::task::spawn_blocking` to avoid blocking the
/// async runtime.
pub fn run_secret_command(
    action: SecretAction,
    name: Option<&str>,
    value: Option<&str>,
) -> Result<()> {
    match action {
        SecretAction::List => {
            for (name, service, account) in secret_targets() {
                let entry = keyring::Entry::new(&service, &account)
                    .map_err(|err| anyhow::anyhow!("failed to open keyring entry: {}", err))?;
                match entry.get_password() {
                    Ok(secret) => {
                        let count = parse_secret_pool_entries(&secret).len();
                        println!("{}: present ({} key(s))", name, count);
                    }
                    Err(_) => println!("{}: missing", name),
                }
            }
            Ok(())
        }
        SecretAction::Set => {
            let (service, account) = resolve_secret_target(name)?;
            let value =
                value.ok_or_else(|| anyhow::anyhow!("{}", t("error.secret_value_required")))?;
            let entry = keyring::Entry::new(&service, &account).map_err(|err| {
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
            let entry = keyring::Entry::new(&service, &account).map_err(|err| {
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
            println!("{}", mask_secret_pool_entry(&secret));
            Ok(())
        }
        SecretAction::Delete => {
            let (service, account) = resolve_secret_target(name)?;
            let entry = keyring::Entry::new(&service, &account).map_err(|err| {
                anyhow::anyhow!(
                    "{}",
                    tf("error.keyring_open", &[("error", &format!("{}", err))])
                )
            })?;
            let current = entry.get_password().map_err(|err| {
                anyhow::anyhow!(
                    "{}",
                    tf("error.keyring_read", &[("error", &format!("{}", err))])
                )
            })?;
            let mut values = parse_secret_pool_entries(&current);
            if values.is_empty() {
                anyhow::bail!("secret pool is empty");
            }

            if let Some(selector) = value {
                if let Ok(index) = selector.parse::<usize>() {
                    if index == 0 || index > values.len() {
                        anyhow::bail!("invalid secret pool index {}", index);
                    }
                    values.remove(index - 1);
                } else {
                    let position = values
                        .iter()
                        .position(|item| item == selector)
                        .ok_or_else(|| anyhow::anyhow!("secret pool item not found"))?;
                    values.remove(position);
                }
            } else {
                let secret_name = name.unwrap_or_default();
                if values.len() == 1 {
                    if !super::prompt_yes_no(
                        &format!(
                            "Delete the only key for {} ({})?",
                            secret_name,
                            mask_secret_pool_entry(&values[0])
                        ),
                        false,
                    )? {
                        println!("Canceled.");
                        return Ok(());
                    }
                    values.clear();
                } else {
                    let Some(index) =
                        super::prompt_secret_pool_deletion_selection(secret_name, &values)?
                    else {
                        println!("Canceled.");
                        return Ok(());
                    };
                    values.remove(index);
                }
            }

            if values.is_empty() {
                entry.delete_credential().map_err(|err| {
                    anyhow::anyhow!(
                        "{}",
                        tf("error.keyring_delete", &[("error", &format!("{}", err))])
                    )
                })?;
            } else {
                entry
                    .set_password(&join_secret_pool_entries(&values))
                    .map_err(|err| {
                        anyhow::anyhow!(
                            "{}",
                            tf("error.keyring_write", &[("error", &format!("{}", err))])
                        )
                    })?;
            }
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

// ---------------------------------------------------------------------------
// Provider detection helpers
// ---------------------------------------------------------------------------

/// Detect which providers have secrets available for the given mode.
pub(super) fn detect_available_providers(secret_mode: &SecretMode) -> Vec<String> {
    let env_providers = detect_available_providers_from_env();
    let keyring_providers = detect_available_providers_from_keyring();

    let mut providers = Vec::new();
    for spec in super::provider_specs() {
        let provider = spec.name.as_str();

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

    providers
}

/// Return the required env-var names for a given provider.
fn required_envs_for_provider(provider: &str) -> Vec<String> {
    let Some(spec) = super::provider_spec_by_name(provider) else {
        return Vec::new();
    };
    let mut envs = Vec::new();
    if let Some(api) = spec.api_key_env.as_ref() {
        envs.push(api.clone());
    }
    if let Some(secret) = spec.secret_key_env.as_ref() {
        envs.push(secret.clone());
    }
    envs
}

/// Detect providers whose env vars are all set.
pub(super) fn detect_available_providers_from_env() -> Vec<String> {
    super::provider_specs()
        .iter()
        .filter(|spec| {
            let required_envs = required_envs_for_provider(spec.name.as_str());
            !required_envs.is_empty()
                && required_envs.iter().all(|name| std::env::var(name).is_ok())
        })
        .map(|spec| spec.name.clone())
        .collect()
}

/// Detect providers whose keyring entries all exist.
fn detect_available_providers_from_keyring() -> Vec<String> {
    super::provider_specs()
        .iter()
        .filter(|spec| {
            let required_envs = required_envs_for_provider(spec.name.as_str());
            !required_envs.is_empty()
                && required_envs
                    .iter()
                    .all(|env_name| keyring_secret_available(env_name))
        })
        .map(|spec| spec.name.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// Keyring helpers
// ---------------------------------------------------------------------------

/// Check whether a secret for the given env-var name exists in the keyring.
fn keyring_secret_available(env_name: &str) -> bool {
    let Some((service, account)) = keyring_target_for_env(env_name) else {
        return false;
    };

    keyring::Entry::new(&service, &account)
        .and_then(|entry| entry.get_password())
        .is_ok()
}

/// Map an env-var name to a keyring account name.
fn keyring_account_for_env(env_name: &str) -> String {
    match env_name {
        "GITHUB_COPILOT_TOKEN" => "github_copilot_token".to_string(),
        _ => env_name.to_ascii_lowercase(),
    }
}

/// Collect all secret env-var names across all provider specs.
fn provider_secret_env_names() -> Vec<String> {
    let mut env_names = BTreeSet::new();
    for spec in super::provider_specs() {
        if let Some(env_name) = spec.api_key_env.as_ref() {
            env_names.insert(env_name.clone());
        }
        if let Some(env_name) = spec.secret_key_env.as_ref() {
            env_names.insert(env_name.clone());
        }
    }
    env_names.into_iter().collect()
}

/// Return all known secret targets as `(name, service, account)` tuples.
fn secret_targets() -> Vec<(String, String, String)> {
    let mut targets = BTreeSet::new();
    for env_name in provider_secret_env_names() {
        let account = keyring_account_for_env(&env_name);
        targets.insert((account.clone(), super::KEYRING_SERVICE.to_string(), account));
    }
    targets.into_iter().collect()
}

/// Map an env-var name to a `(service, account)` keyring target.
fn keyring_target_for_env(env_name: &str) -> Option<(String, String)> {
    Some((
        super::KEYRING_SERVICE.to_string(),
        keyring_account_for_env(env_name),
    ))
}

/// Build a config-string reference for a secret based on the current mode.
///
/// In `Env` mode the raw env-var name is used; in `Keyring` mode a
/// `keyring://` URI is used; in `AutoDetect` mode the existing
/// env-var value is preferred, falling back to the keyring URI.
pub(super) fn secret_reference(env_name: &str, secret_mode: &SecretMode) -> String {
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

/// Build a `keyring://service/account` URI for the given env-var name.
fn keyring_reference(env_name: &str) -> Option<String> {
    keyring_target_for_env(env_name)
        .map(|(service, account)| format!("keyring://{}/{}", service, account))
}

// ---------------------------------------------------------------------------
// Env-to-keyring conversion
// ---------------------------------------------------------------------------

/// Convert known env variable placeholder names in template content into
/// keyring references.
///
/// This is used when setup is requested with keyring secret mode.
pub fn convert_env_placeholders_to_keyring(content: &str) -> String {
    let mut out = content.to_string();

    // Replace keyring-URL env entries (provider specs now use keyring:// refs directly).
    for env_name in provider_secret_env_names() {
        if let Some(reference) = keyring_reference(&env_name) {
            out = out.replace(&env_name, &reference);
        }
    }

    // Also handle raw env var names (legacy template format).
    let raw_env_names: &[(&str, &str)] = &[
        ("OPENAI_API_KEY", "keyring://go-on/openai_api_key"),
        ("ANTHROPIC_API_KEY", "keyring://go-on/anthropic_api_key"),
        ("DEEPSEEK_API_KEY", "keyring://go-on/deepseek_api_key"),
        ("DOUBAO_API_KEY", "keyring://go-on/doubao_api_key"),
        ("WENXIN_API_KEY", "keyring://go-on/wenxin_api_key"),
        ("WENXIN_SECRET_KEY", "keyring://go-on/wenxin_secret_key"),
        ("COPILOT_API_KEY", "keyring://go-on/copilot_api_key"),
        (
            "GITHUB_COPILOT_TOKEN",
            "keyring://go-on/github_copilot_token",
        ),
        (
            "OTHER_PROVIDER_API_KEY",
            "keyring://go-on/openai_compatible_api_key",
        ),
    ];
    for (raw_name, reference) in raw_env_names {
        out = out.replace(raw_name, reference);
    }

    out
}

// ---------------------------------------------------------------------------
// Interactive keyring storage
// ---------------------------------------------------------------------------

/// Interactive flow to store all configured secrets into system keyring.
///
/// Prompts user for each secret key and stores non-empty values.
pub(super) fn store_keyring_secrets_interactive(
    selected_providers: &[String],
    custom_agents: &[super::CustomAgentSpec],
) -> Result<()> {
    println!("{}", t("setup.enter_secrets"));
    let mut required_envs = Vec::new();
    for provider in selected_providers {
        for env_name in required_envs_for_provider(provider) {
            if !required_envs
                .iter()
                .any(|existing: &String| existing == &env_name)
            {
                required_envs.push(env_name);
            }
        }
    }
    for agent in custom_agents {
        for env_name in [agent.api_key_env.as_ref(), agent.secret_key_env.as_ref()]
            .into_iter()
            .flatten()
        {
            if !required_envs
                .iter()
                .any(|existing: &String| existing == env_name)
            {
                required_envs.push(env_name.clone());
            }
        }
    }

    let mut handled_envs = BTreeSet::new();

    for (name, service, account) in secret_targets() {
        if let Some(env_name) = secret_name_to_env(&name) {
            handled_envs.insert(env_name.to_string());
            if !required_envs.iter().any(|existing| existing == &env_name) {
                continue;
            }
        }

        let values = super::prompt_secret_pool_values(&name)?;
        if values.is_empty() {
            continue;
        }

        let entry = keyring::Entry::new(&service, &account)
            .map_err(|err| anyhow::anyhow!("failed to open keyring entry: {}", err))?;
        entry
            .set_password(&join_secret_pool_entries(&values))
            .map_err(|err| anyhow::anyhow!("failed to write keyring entry: {}", err))?;
    }

    for env_name in required_envs {
        if handled_envs.contains(&env_name) {
            continue;
        }
        let values = super::prompt_secret_pool_values(&env_name)?;
        if values.is_empty() {
            continue;
        }
        let Some((service, account)) = keyring_target_for_env(&env_name) else {
            continue;
        };
        let entry = keyring::Entry::new(&service, &account)
            .map_err(|err| anyhow::anyhow!("failed to open keyring entry: {}", err))?;
        entry
            .set_password(&join_secret_pool_entries(&values))
            .map_err(|err| anyhow::anyhow!("failed to write keyring entry: {}", err))?;
    }

    Ok(())
}

/// Look up an env-var name by its keyring account name.
fn secret_name_to_env(secret_name: &str) -> Option<String> {
    provider_secret_env_names()
        .into_iter()
        .find(|env_name| keyring_account_for_env(env_name) == secret_name)
}

// ---------------------------------------------------------------------------
// Secret target resolution
// ---------------------------------------------------------------------------

/// Resolve secret command name to keyring service/account.
///
/// Used by `run_secret_command` handlers to map human-readable secret names.
fn resolve_secret_target(name: Option<&str>) -> Result<(String, String)> {
    let name = name.ok_or_else(|| anyhow::anyhow!("--secret-name is required"))?;
    if let Some((_, service, account)) = secret_targets()
        .iter()
        .find(|(known_name, _, _)| *known_name == name)
    {
        return Ok((service.clone(), account.clone()));
    }

    if let Some(locator) = name.strip_prefix(crate::shared::keyring_ref::KEYRING_PREFIX) {
        let (service, account) = locator.split_once('/').ok_or_else(|| {
            anyhow::anyhow!(
                "invalid keyring secret reference '{}': expected keyring://<service>/<account>",
                name
            )
        })?;
        return Ok((service.to_string(), account.to_string()));
    }

    keyring_target_for_env(name)
        .ok_or_else(|| anyhow::anyhow!("{}", tf("error.unknown_secret_name", &[("name", name)])))
}

// ---------------------------------------------------------------------------
// Secret pool helpers
// ---------------------------------------------------------------------------

/// Parse a raw secret string into individual pool entries.
///
/// Entries can be separated by newlines or commas (when more than one).
fn parse_secret_pool_entries(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    let multiline: Vec<String> = trimmed
        .lines()
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect();
    if multiline.len() > 1 {
        return multiline;
    }

    if trimmed.contains(',') {
        let comma_split: Vec<String> = trimmed
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(str::to_string)
            .collect();
        if comma_split.len() > 1 {
            return comma_split;
        }
    }

    vec![trimmed.to_string()]
}

/// Join a slice of secret pool entries into a single string (newline-separated).
fn join_secret_pool_entries(values: &[String]) -> String {
    values.join("\n")
}

/// Mask a secret for safe display, showing first/last 4 chars.
pub(super) fn mask_secret_pool_entry(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    let len = chars.len();
    if len <= 8 {
        return format!("{} (len={})", "*".repeat(len.min(4)), len);
    }

    let prefix: String = chars.iter().take(4).collect();
    let suffix: String = chars.iter().skip(len.saturating_sub(4)).collect();
    format!("{}...{}", prefix, suffix)
}
