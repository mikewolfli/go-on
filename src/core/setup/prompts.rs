//! Interactive setup prompt sub-module (GAP-B53-23).
//!
//! Contains all interactive prompt functions for setup: provider selection,
//! setup level selection, custom agent configuration, yes/no prompts, etc.

use std::collections::HashSet;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

use crate::config::AdaptiveConfig;
use crate::i18n::runtime::{t, tf};
use anyhow::{Context, Result};

use super::config_gen::{CustomAgentSpec, LocalModelOptions};
use super::secrets::SecretMode;
use super::{SetupLevel, SetupOptions, SetupProfile};

// ═══════════════════════════════════════════════════════════════════════════
// Public entry points
// ═══════════════════════════════════════════════════════════════════════════

/// Entry point for setup logic.
///
/// Handles profile selection, secret mode, writing config template, writing RULES files,
/// and optionally storing secrets into keyring.
#[must_use]
#[allow(clippy::double_must_use)]
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
    let setup_level = match options.level {
        Some(level) => level,
        None => prompt_setup_level()?,
    };
    // The adaptive config is generated in code by
    // `config_gen::generate_adaptive_config_toml` below — there is no
    // on-disk template file to look up, so no existence check is needed.

    let secret_mode = match options.secret_mode {
        Some(value) => value,
        None => {
            // Auto-detect: use Env mode if env vars are already set, otherwise prompt.
            let has_env_vars = !super::secrets::detect_available_providers_from_env().is_empty();
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

    // Detect available AI providers.
    let detected_providers = super::secrets::detect_available_providers(&secret_mode);
    let available_providers = prompt_provider_selection(&detected_providers, setup_level)?;
    // Quick mode: skip extra-agent prompt to keep the flow minimal.
    let custom_agents = if setup_level == SetupLevel::Quick {
        Vec::new()
    } else {
        prompt_additional_agents()?
    };

    let mut adaptive_config = AdaptiveConfig::auto_detect();
    apply_setup_level_to_config(&mut adaptive_config, setup_level)?;
    if available_providers.is_empty() {
        anyhow::bail!("{}", t("setup.provider_selection_required"));
    }
    println!("{}", t("setup.detected_providers"));
    for provider in &available_providers {
        println!("  - {}", provider);
    }
    adaptive_config.minimal_config.available_providers = available_providers;

    let mut content = super::config_gen::generate_adaptive_config_toml(
        &adaptive_config,
        &secret_mode,
        &custom_agents,
    );

    // If using keyring mode, convert env-var placeholders to keyring references.
    if secret_mode == SecretMode::Keyring {
        content = super::secrets::convert_env_placeholders_to_keyring(&content);
    }

    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory: {}", parent.display()))?;
    }
    fs::write(config_path, &content)
        .with_context(|| format!("failed to write config file: {}", config_path.display()))?;

    super::config_gen::write_default_rules(config_path.parent().unwrap_or_else(|| Path::new(".")))?;

    let should_store_secrets = match secret_mode {
        SecretMode::Keyring if options.prompt_for_secrets => {
            prompt_yes_no(&t("setup.prompt_store_secrets"), true)?
        }
        SecretMode::Keyring => false,
        SecretMode::AutoDetect => {
            // Auto-detect mode: ask whether to set up API keys now.
            prompt_yes_no(&t("setup.prompt_setup_api_keys"), true)?
        }
        _ => false,
    };

    if should_store_secrets {
        super::secrets::store_keyring_secrets_interactive(
            &adaptive_config.minimal_config.available_providers,
            &custom_agents,
        )?;
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

/// Parse setup level string to SetupLevel enum.
/// Accepts case-insensitive quick|standard|custom.
pub fn parse_setup_level(value: &str) -> Result<SetupLevel> {
    if value.eq_ignore_ascii_case("quick") {
        return Ok(SetupLevel::Quick);
    }
    if value.eq_ignore_ascii_case("standard") {
        return Ok(SetupLevel::Standard);
    }
    if value.eq_ignore_ascii_case("custom") {
        return Ok(SetupLevel::Custom);
    }
    anyhow::bail!("{}", tf("error.invalid_setup_level", &[("value", value)]))
}

// ═══════════════════════════════════════════════════════════════════════════
// Custom agent prompts
// ═══════════════════════════════════════════════════════════════════════════

/// Interactively ask the user whether they want to add any custom agents beyond the
/// catalog providers.  Returns a (possibly empty) list of `CustomAgentSpec` values.
fn prompt_additional_agents() -> Result<Vec<CustomAgentSpec>> {
    const KNOWN_TYPES: &[&str] = &[
        "openai",
        "anthropic",
        "gemini",
        "deepseek",
        "groq",
        "glm",
        "doubao",
        "wenxin",
        "hunyuan",
        "kimi",
        "qwen",
        "moonshot",
        "mistral",
        "llama",
        "copilot",
        "openai_compatible",
        "siliconflow",
    ];

    if !prompt_yes_no(
        "Add extra agents beyond the catalog above? (e.g. self-hosted / local models) [n]",
        false,
    )? {
        return Ok(Vec::new());
    }

    let mut agents: Vec<CustomAgentSpec> = Vec::new();

    loop {
        println!(
            "\n{}",
            tf(
                "cli.custom_agent_title",
                &[("name", &(agents.len() + 1).to_string())]
            )
        );

        // Name
        let name = loop {
            let raw = prompt_value(&format!("  {}", t("cli.agent_name_prompt")))?;
            let trimmed = raw.trim().to_string();
            if trimmed.is_empty() {
                println!("  {}", t("cli.agent_name_required"));
                continue;
            }
            if trimmed.contains(|c: char| c.is_whitespace()) {
                println!("  {}", t("cli.agent_name_no_spaces"));
                continue;
            }
            break trimmed;
        };

        // Type
        println!(
            "  {}",
            tf(
                "cli.agent_type_available",
                &[("types", &KNOWN_TYPES.join(", "))]
            )
        );
        let agent_type = loop {
            let raw = prompt_value(&format!("  {}", t("cli.agent_type_prompt")))?;
            let trimmed = raw.trim().to_string();
            if trimmed.is_empty() {
                break "openai_compatible".to_string();
            }
            if KNOWN_TYPES.contains(&trimmed.as_str()) {
                break trimmed;
            }
            println!(
                "  {}",
                tf(
                    "cli.agent_type_unknown",
                    &[("types", &KNOWN_TYPES.join(", "))]
                )
            );
        };

        // URL (required for non-managed types)
        let url = {
            let raw = prompt_value(&format!("  {}", t("cli.base_url_prompt")))?;
            let trimmed = raw.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        };

        // API key env var
        let api_key_env = {
            let raw = prompt_value(&format!("  {}", t("cli.api_key_env_prompt")))?;
            let trimmed = raw.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        };

        // Secret key env var (e.g. for providers that need two keys)
        let secret_key_env = {
            let raw = prompt_value(&format!("  {}", t("cli.secret_key_env_prompt")))?;
            let trimmed = raw.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        };

        // Model
        let model = {
            let raw = prompt_value(&format!("  {}", t("cli.model_name_prompt")))?;
            let trimmed = raw.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        };

        agents.push(CustomAgentSpec {
            name,
            agent_type,
            url,
            api_key_env,
            secret_key_env,
            model,
        });

        if !prompt_yes_no("Add another custom agent?", false)? {
            break;
        }
    }

    Ok(agents)
}

// ═══════════════════════════════════════════════════════════════════════════
// Provider selection prompts
// ═══════════════════════════════════════════════════════════════════════════

/// Prompt the user to select one or more providers.
/// - Quick level: flat numbered list, no region step.
/// - Standard / Custom level: two-step region → provider flow.
fn prompt_provider_selection(
    detected_providers: &[String],
    setup_level: SetupLevel,
) -> Result<Vec<String>> {
    if matches!(setup_level, SetupLevel::Quick) {
        return prompt_provider_selection_quick(detected_providers);
    }
    prompt_provider_selection_full(detected_providers)
}

/// Flat provider picker for Quick setup — no region step, single numbered list.
fn prompt_provider_selection_quick(detected_providers: &[String]) -> Result<Vec<String>> {
    let specs = super::provider_specs();
    loop {
        println!("\n{}", t("cli.select_provider"));
        println!();
        for (i, spec) in specs.iter().enumerate() {
            let mark = if detected_providers.contains(&spec.name) {
                t("cli.detected_marker")
            } else {
                "".to_string()
            };
            let region = spec.region.as_deref().unwrap_or("Other");
            println!("  {:>2}. {}{}  [{}]", i + 1, spec.name, mark, region);
        }
        if !detected_providers.is_empty() {
            let default_nums: Vec<String> = detected_providers
                .iter()
                .filter_map(|p| {
                    specs
                        .iter()
                        .position(|s| &s.name == p)
                        .map(|i| (i + 1).to_string())
                })
                .collect();
            println!("\n  {} {}", t("cli.detected_note"), default_nums.join(","));
            print!(
                "\n{} [{}]: ",
                t("cli.enter_numbers"),
                default_nums.join(",")
            );
        } else {
            print!("\n{}: ", t("cli.enter_numbers"));
        }
        io::stdout().flush().context("failed to flush stdout")?;
        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .context("failed to read input")?;
        let value = input.trim();
        if value.is_empty() {
            if detected_providers.is_empty() {
                println!("  {}", t("cli.provider_required"));
                continue;
            }
            return Ok(detected_providers.to_vec());
        }
        if value.eq_ignore_ascii_case("all") {
            return Ok(specs.iter().map(|s| s.name.clone()).collect());
        }
        let mut selected: Vec<String> = Vec::new();
        let mut invalid = None;
        for token in value.split(',') {
            let raw = token.trim();
            if raw.is_empty() {
                continue;
            }
            if let Ok(idx) = raw.parse::<usize>() {
                if idx >= 1 && idx <= specs.len() {
                    let name = specs[idx - 1].name.clone();
                    if !selected.contains(&name) {
                        selected.push(name);
                    }
                    continue;
                }
            }
            if let Some(spec) = specs.iter().find(|s| s.name.eq_ignore_ascii_case(raw)) {
                if !selected.contains(&spec.name) {
                    selected.push(spec.name.clone());
                }
            } else {
                invalid = Some(raw.to_string());
                break;
            }
        }
        if let Some(bad) = invalid {
            println!("  {}", tf("cli.invalid_selection", &[("value", &bad)]));
            continue;
        }
        if selected.is_empty() {
            println!("  {}", t("cli.provider_required"));
            continue;
        }
        return Ok(selected);
    }
}

fn prompt_provider_selection_full(detected_providers: &[String]) -> Result<Vec<String>> {
    const REGION_ORDER: &[&str] = &["Global", "China", "Europe", "Local", "Other"];

    let specs = super::provider_specs();

    // Build canonical ordered region list
    let mut seen: HashSet<String> = HashSet::new();
    let mut ordered_regions: Vec<String> = Vec::new();
    for &r in REGION_ORDER {
        if specs
            .iter()
            .any(|s| s.region.as_deref().unwrap_or("Other") == r)
            && seen.insert(r.to_string())
        {
            ordered_regions.push(r.to_string());
        }
    }
    for spec in specs.iter() {
        let r = spec.region.as_deref().unwrap_or("Other").to_string();
        if seen.insert(r.clone()) {
            ordered_regions.push(r);
        }
    }

    // ── Step 1: select region(s) ──────────────────────────────────────────
    let selected_regions: Vec<String> = loop {
        println!("\nStep 1/2 — Select region(s)");
        println!();
        for (i, region) in ordered_regions.iter().enumerate() {
            let region_specs: Vec<_> = specs
                .iter()
                .filter(|s| s.region.as_deref().unwrap_or("Other") == region.as_str())
                .collect();
            let det_count = region_specs
                .iter()
                .filter(|s| detected_providers.contains(&s.name))
                .count();
            let preview: Vec<&str> = region_specs
                .iter()
                .take(4)
                .map(|s| s.name.as_str())
                .collect();
            let mut preview_str = preview.join(", ");
            if region_specs.len() > 4 {
                preview_str.push_str(&format!(", ... ({} total)", region_specs.len()));
            }
            let det_mark = if det_count > 0 {
                format!(" [{} detected *]", det_count)
            } else {
                String::new()
            };
            println!("  {:>2}. {}{}  — {}", i + 1, region, det_mark, preview_str);
        }
        if !detected_providers.is_empty() {
            println!("\n  (* = detected from environment / keyring)");
        }

        // Build default hint from detected providers' regions
        let auto_regions: Vec<String> = {
            let mut v: Vec<String> = Vec::new();
            for p in detected_providers {
                if let Some(spec) = specs.iter().find(|s| &s.name == p) {
                    let r = spec.region.as_deref().unwrap_or("Other").to_string();
                    if !v.contains(&r) {
                        v.push(r);
                    }
                }
            }
            v
        };
        let default_hint = if auto_regions.is_empty() {
            "all".to_string()
        } else {
            auto_regions
                .iter()
                .filter_map(|r| {
                    ordered_regions
                        .iter()
                        .position(|x| x == r)
                        .map(|i| (i + 1).to_string())
                })
                .collect::<Vec<_>>()
                .join(",")
        };

        print!(
            "\nEnter region numbers (e.g. 1,3) or \"all\" [{}]: ",
            default_hint
        );
        io::stdout().flush().context("failed to flush stdout")?;

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .context("failed to read input")?;
        let value = input.trim();

        if value.is_empty() {
            if auto_regions.is_empty() {
                break ordered_regions.clone();
            } else {
                break auto_regions;
            }
        }
        if value.eq_ignore_ascii_case("all") {
            break ordered_regions.clone();
        }

        let mut chosen: Vec<String> = Vec::new();
        let mut invalid = None;
        for token in value.split(',') {
            let raw = token.trim();
            if raw.is_empty() {
                continue;
            }
            if let Ok(idx) = raw.parse::<usize>() {
                if idx >= 1 && idx <= ordered_regions.len() {
                    let r = ordered_regions[idx - 1].clone();
                    if !chosen.contains(&r) {
                        chosen.push(r);
                    }
                    continue;
                }
            }
            if let Some(r) = ordered_regions.iter().find(|r| r.eq_ignore_ascii_case(raw)) {
                if !chosen.contains(r) {
                    chosen.push(r.clone());
                }
            } else {
                invalid = Some(raw.to_string());
                break;
            }
        }
        if let Some(bad) = invalid {
            println!("  Invalid region: '{}'. Try again.", bad);
            continue;
        }
        if chosen.is_empty() {
            println!("  At least one region is required.");
            continue;
        }
        break chosen;
    };

    // ── Step 2: select provider(s) within chosen region(s) ───────────────
    loop {
        println!(
            "\nStep 2/2 — Select providers from: {}",
            selected_regions.join(", ")
        );
        println!();

        let mut index_map: Vec<String> = Vec::new();
        for region in &selected_regions {
            let mut first_in_region = true;
            for spec in specs
                .iter()
                .filter(|s| s.region.as_deref().unwrap_or("Other") == region.as_str())
            {
                if first_in_region {
                    println!("  [{}]", region);
                    first_in_region = false;
                }
                index_map.push(spec.name.clone());
                let mark = if detected_providers.contains(&spec.name) {
                    " *"
                } else {
                    ""
                };
                println!("    {:>2}. {}{}", index_map.len(), spec.name, mark);
            }
        }

        let scoped_detected: Vec<String> = detected_providers
            .iter()
            .filter(|p| index_map.contains(p))
            .cloned()
            .collect();

        if !scoped_detected.is_empty() {
            println!(
                "\n  (* = detected. Default: {})",
                scoped_detected.join(", ")
            );
        }

        let default_hint = if scoped_detected.is_empty() {
            "enter numbers or \"all\"".to_string()
        } else {
            scoped_detected
                .iter()
                .filter_map(|p| {
                    index_map
                        .iter()
                        .position(|x| x == p)
                        .map(|i| (i + 1).to_string())
                })
                .collect::<Vec<_>>()
                .join(",")
        };

        print!("\nSelect providers [{}]: ", default_hint);
        io::stdout().flush().context("failed to flush stdout")?;

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .context("failed to read input")?;
        let value = input.trim();

        if value.is_empty() {
            if scoped_detected.is_empty() {
                println!("  At least one provider is required.");
                continue;
            }
            return Ok(scoped_detected);
        }
        if value.eq_ignore_ascii_case("all") {
            return Ok(index_map);
        }

        let mut selected: Vec<String> = Vec::new();
        let mut invalid = None;
        for token in value.split(',') {
            let raw = token.trim();
            if raw.is_empty() {
                continue;
            }
            if let Ok(idx) = raw.parse::<usize>() {
                if idx >= 1 && idx <= index_map.len() {
                    selected.push(index_map[idx - 1].clone());
                    continue;
                }
                invalid = Some(raw.to_string());
                break;
            }
            if index_map.contains(&raw.to_string()) {
                selected.push(raw.to_string());
            } else {
                invalid = Some(raw.to_string());
                break;
            }
        }
        if let Some(bad) = invalid {
            println!(
                "{}",
                tf(
                    "error.invalid_provider_selection",
                    &[("value", bad.as_str())]
                )
            );
            continue;
        }

        selected.sort();
        selected.dedup();

        if selected.is_empty() {
            println!("{}", t("setup.provider_selection_required"));
            continue;
        }

        return Ok(selected);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Setup level prompts
// ═══════════════════════════════════════════════════════════════════════════

fn prompt_setup_level() -> Result<SetupLevel> {
    let selected = prompt_choice(&t("setup.prompt_level"), &["1", "2", "3"], "2")?;
    match selected.as_str() {
        "1" => Ok(SetupLevel::Quick),
        "2" => Ok(SetupLevel::Standard),
        _ => Ok(SetupLevel::Custom),
    }
}

fn apply_setup_level_to_config(config: &mut AdaptiveConfig, level: SetupLevel) -> Result<()> {
    match level {
        SetupLevel::Quick => {
            config.minimal_config.default_phase = "coding".to_string();
            config.minimal_config.enable_cache = true;
            config.minimal_config.enable_vector_memory = false;
            println!("{}", t("setup.level_quick_applied"));
        }
        SetupLevel::Standard => {
            config.minimal_config.default_phase = "coding".to_string();
            config.minimal_config.enable_cache = true;
            config.minimal_config.enable_vector_memory = true;
            println!("{}", t("setup.level_standard_applied"));
        }
        SetupLevel::Custom => {
            config.minimal_config.default_phase = prompt_choice(
                &t("setup.prompt_default_phase"),
                &["planning", "coding", "review", "delivery"],
                "coding",
            )?;
            config.minimal_config.enable_cache =
                prompt_yes_no(&t("setup.prompt_enable_cache"), true)?;
            config.minimal_config.enable_vector_memory =
                prompt_yes_no(&t("setup.prompt_enable_vector"), true)?;
            println!("{}", t("setup.level_custom_applied"));
        }
    }
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// Secret pool prompts
// ═══════════════════════════════════════════════════════════════════════════

pub(crate) fn prompt_secret_pool_deletion_selection(
    secret_name: &str,
    values: &[String],
) -> Result<Option<usize>> {
    if values.is_empty() {
        return Ok(None);
    }

    println!("Select a key to delete from {}:", secret_name);
    for (index, value) in values.iter().enumerate() {
        println!(
            "  {}. {}",
            index + 1,
            super::secrets::mask_secret_pool_entry(value)
        );
    }
    println!("  0. Cancel");

    loop {
        let choice = prompt_value("Delete which key")?;
        let trimmed = choice.trim();
        if trimmed.is_empty() || trimmed == "0" {
            return Ok(None);
        }
        if let Ok(index) = trimmed.parse::<usize>() {
            if (1..=values.len()).contains(&index) {
                return Ok(Some(index - 1));
            }
        }

        println!("Invalid selection. Choose 0-{}.", values.len());
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Generic prompt helpers
// ═══════════════════════════════════════════════════════════════════════════

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

pub(crate) fn prompt_yes_no(prompt: &str, default_yes: bool) -> Result<bool> {
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

pub(crate) fn prompt_secret_pool_values(prompt: &str) -> Result<Vec<String>> {
    println!("{} (enter one key per line, leave blank to finish)", prompt);
    let mut values = Vec::new();
    loop {
        let value = prompt_value(&format!("{} #{}", prompt, values.len() + 1))?;
        if value.trim().is_empty() {
            break;
        }
        values.push(value.trim().to_string());
    }
    Ok(values)
}

// ═══════════════════════════════════════════════════════════════════════════
// add_local_model
// ═══════════════════════════════════════════════════════════════════════════

/// Add or update a local model agent entry in the config file.
///
/// # Why this bypasses `AppConfig::load` (sync-boundary note)
///
/// Like [`apply_recommended_to_config`](crate::setup::config_gen::apply_recommended_to_config),
/// this mutates the raw `toml::Value` tree (read → insert agent/phases entries
/// → re-serialize) rather than a typed `AppConfig` round-trip, because:
///
/// 1. `AppConfig` has no `Serialize` impl, so a typed load→modify→save cycle
///    is not possible.
/// 2. Raw-TOML mutation preserves all unrelated keys (unknown keys, comments
///    are lost either way, but unknown keys survive), while a typed round-trip
///    would drop anything the structs do not model.
/// 3. `--add-model` may target a config that parses as TOML but fails typed
///    validation; the surgical edit should still succeed.
///
/// Migration / auto-rules / legacy-key sync are re-applied on the next real
/// `AppConfig::load` — this command does not materialize them into the file.
pub fn add_local_model(config_path: &Path, mut options: LocalModelOptions) -> Result<()> {
    if !config_path.exists() {
        anyhow::bail!("config file does not exist: {}", config_path.display());
    }

    let mut name = options
        .name
        .take()
        .unwrap_or_else(|| "local_model".to_string())
        .trim()
        .to_string();
    if name.is_empty() {
        name = "local_model".to_string();
    }

    let mut url = options.url.take().unwrap_or_default();
    if url.trim().is_empty() {
        url = prompt_value("Local model URL (for example http://127.0.0.1:11434/v1)")?;
    }
    if url.trim().is_empty() {
        anyhow::bail!("local model url is required");
    }

    let mut agent_type = options
        .agent_type
        .take()
        .unwrap_or_else(|| "openai".to_string());
    if agent_type.trim().is_empty() {
        agent_type = "openai".to_string();
    }

    let mut model = options
        .model
        .take()
        .unwrap_or_else(|| "local-model".to_string());
    if model.trim().is_empty() {
        model = "local-model".to_string();
    }

    let content = fs::read_to_string(config_path)
        .with_context(|| format!("failed to read config file: {}", config_path.display()))?;
    let mut root: toml::Value = toml::from_str(&content)
        .with_context(|| format!("failed to parse toml: {}", config_path.display()))?;

    let table = root
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("root toml is not table"))?;

    let agents = ensure_table(table, "agents")?;
    let mut agent_table = toml::map::Map::new();
    agent_table.insert(
        "type".to_string(),
        toml::Value::String(agent_type.trim().to_string()),
    );
    agent_table.insert(
        "url".to_string(),
        toml::Value::String(url.trim().to_string()),
    );
    agent_table.insert(
        "model".to_string(),
        toml::Value::String(model.trim().to_string()),
    );
    agent_table.insert("supports_system".to_string(), toml::Value::Boolean(true));

    if let Some(api_key_env) = options.api_key_env.take() {
        if !api_key_env.trim().is_empty() {
            agent_table.insert(
                "api_key_env".to_string(),
                toml::Value::String(api_key_env.trim().to_string()),
            );
        }
    }
    if let Some(secret_key_env) = options.secret_key_env.take() {
        if !secret_key_env.trim().is_empty() {
            agent_table.insert(
                "secret_key_env".to_string(),
                toml::Value::String(secret_key_env.trim().to_string()),
            );
        }
    }

    agents.insert(name.clone(), toml::Value::Table(agent_table));

    if options.apply_to_phases {
        let phases = ensure_table(table, "phases")?;
        for phase_name in ["planning", "coding", "review", "delivery"] {
            let Some(phase) = phases
                .get_mut(phase_name)
                .and_then(|value| value.as_table_mut())
            else {
                continue;
            };
            ensure_string_array_contains(phase, "agents", &name)?;

            if phase_name == "coding" {
                let options = ensure_table(phase, "options")?;
                ensure_string_array_contains(options, "full_auto_review_agents", &name)?;
            }
        }
    }

    let output = toml::to_string_pretty(&root).context("failed to serialize updated config")?;
    fs::write(config_path, output)
        .with_context(|| format!("failed to write config file: {}", config_path.display()))?;
    println!(
        "added local model '{}' to {}",
        name,
        config_path.to_string_lossy()
    );
    Ok(())
}

pub(crate) fn ensure_table<'a>(
    parent: &'a mut toml::map::Map<String, toml::Value>,
    key: &str,
) -> Result<&'a mut toml::map::Map<String, toml::Value>> {
    let value = parent
        .entry(key.to_string())
        .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
    if !value.is_table() {
        *value = toml::Value::Table(toml::map::Map::new());
    }
    value
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("expected table after normalization"))
}

pub(crate) fn ensure_string_array_contains(
    parent: &mut toml::map::Map<String, toml::Value>,
    key: &str,
    item: &str,
) -> Result<()> {
    let value = parent
        .entry(key.to_string())
        .or_insert_with(|| toml::Value::Array(Vec::new()));
    if !value.is_array() {
        *value = toml::Value::Array(Vec::new());
    }
    let array = value
        .as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("expected array after normalization"))?;
    let exists = array
        .iter()
        .any(|entry| entry.as_str().map(|v| v == item).unwrap_or(false));
    if !exists {
        array.push(toml::Value::String(item.to_string()));
    }
    Ok(())
}
