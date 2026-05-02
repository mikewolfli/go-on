use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::State;
use toml_edit::{value, Array, DocumentMut, Item, Table};

use crate::state::AppState;

fn normalize_protocol_mode(value: &str) -> Option<&'static str> {
    match value.trim().trim_matches('"').to_ascii_lowercase().as_str() {
        "adaptive" | "auto" => Some("adaptive"),
        "acp_stdio" | "acp+stdio" | "acp-stdio" | "acp" => Some("acp_stdio"),
        "acp_http" | "acp+http" | "acp-http" => Some("acp_http"),
        "mcp_stdio" | "mcp+stdio" | "mcp-stdio" | "mcp" => Some("mcp_stdio"),
        "mcp_http" | "mcp+http" | "mcp-http" => Some("mcp_http"),
        "from_config" | "" => None,
        _ => None,
    }
}

#[derive(Debug, serde::Deserialize, Clone)]
struct ProviderCatalogFile {
    providers: Vec<ProviderCatalogSpec>,
}

#[derive(Debug, serde::Deserialize, Clone)]
struct ProviderCatalogSpec {
    name: String,
    #[serde(rename = "type")]
    agent_type: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    chat_path: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    api_key_env: Option<String>,
    #[serde(default)]
    secret_key_env: Option<String>,
    #[serde(default)]
    anthropic_version: Option<String>,
    #[serde(default)]
    max_tokens: Option<u32>,
    #[serde(default)]
    supports_system: Option<bool>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCatalogEntry {
    pub name: String,
    pub agent_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret_key_env: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_system: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub configured_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub configured_env_var: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSelectionSaveResult {
    pub provider: String,
    pub model: String,
    pub config_path: String,
    pub note: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CopilotTokenResult {
    pub found: bool,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_masked: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_plain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification_uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_in_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub poll_interval_seconds: Option<u64>,
    pub note: String,
}

#[derive(Debug, serde::Deserialize)]
struct GithubDeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[allow(dead_code)] // F-GAP-13 — reserved for future security governor wiring
    verification_uri_complete: Option<String>,
    expires_in: u64,
    interval: Option<u64>,
}

#[derive(Debug, serde::Deserialize)]
struct GithubAccessTokenResponse {
    access_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Debug, Clone)]
struct DeviceFlowSession {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_at: u64,
    interval_seconds: u64,
}

const GITHUB_DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const GITHUB_DEVICE_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
// GitHub CLI OAuth App client id (public).
const GITHUB_OAUTH_CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";

static DEVICE_FLOW_SESSION: OnceLock<Mutex<Option<DeviceFlowSession>>> = OnceLock::new();

fn device_flow_session() -> &'static Mutex<Option<DeviceFlowSession>> {
    DEVICE_FLOW_SESSION.get_or_init(|| Mutex::new(None))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn token_result_found(source: String, token: String, note: String) -> CopilotTokenResult {
    CopilotTokenResult {
        found: true,
        source,
        token_masked: Some(mask_token(&token)),
        token_plain: Some(token),
        verification_uri: None,
        user_code: None,
        expires_in_seconds: None,
        poll_interval_seconds: None,
        note,
    }
}

fn token_result_pending(
    source: String,
    session: &DeviceFlowSession,
    note: String,
) -> CopilotTokenResult {
    let now = now_secs();
    CopilotTokenResult {
        found: false,
        source,
        token_masked: None,
        token_plain: None,
        verification_uri: Some(session.verification_uri.clone()),
        user_code: Some(session.user_code.clone()),
        expires_in_seconds: Some(session.expires_at.saturating_sub(now)),
        poll_interval_seconds: Some(session.interval_seconds),
        note,
    }
}

fn start_device_flow() -> Result<DeviceFlowSession, String> {
    let client = reqwest::blocking::Client::new();
    let response = client
        .post(GITHUB_DEVICE_CODE_URL)
        .header("Accept", "application/json")
        .header("User-Agent", "go-on-gui/0.6.1")
        .form(&[
            ("client_id", GITHUB_OAUTH_CLIENT_ID),
            ("scope", "read:user"),
        ])
        .send()
        .map_err(|e| format!("failed to request GitHub device code: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        return Err(format!("device code request failed ({status}): {body}"));
    }

    let payload: GithubDeviceCodeResponse = response
        .json()
        .map_err(|e| format!("invalid device code response: {e}"))?;

    Ok(DeviceFlowSession {
        device_code: payload.device_code,
        user_code: payload.user_code,
        verification_uri: payload.verification_uri,
        expires_at: now_secs() + payload.expires_in,
        interval_seconds: payload.interval.unwrap_or(5).max(1),
    })
}

fn poll_device_flow_access_token(
    session: &DeviceFlowSession,
) -> Result<GithubAccessTokenResponse, String> {
    let client = reqwest::blocking::Client::new();
    let response = client
        .post(GITHUB_DEVICE_TOKEN_URL)
        .header("Accept", "application/json")
        .header("User-Agent", "go-on-gui/0.6.1")
        .form(&[
            ("client_id", GITHUB_OAUTH_CLIENT_ID),
            ("device_code", session.device_code.as_str()),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .send()
        .map_err(|e| format!("failed to poll GitHub access token: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        return Err(format!("access token polling failed ({status}): {body}"));
    }

    response
        .json::<GithubAccessTokenResponse>()
        .map_err(|e| format!("invalid access token response: {e}"))
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AutoConfigureResult {
    pub linked: bool,
    pub executable_path: Option<String>,
    pub reason: String,
}

fn env_file_path(working_dir: &str) -> PathBuf {
    PathBuf::from(working_dir).join(".env.goon")
}

fn config_file_path(working_dir: &str) -> PathBuf {
    PathBuf::from(working_dir).join("config.toml")
}

fn config_template_path(working_dir: &str) -> PathBuf {
    PathBuf::from(working_dir).join("config.toml.autopilot-adaptive")
}

fn provider_catalog_path(working_dir: &str) -> Option<PathBuf> {
    let candidates = [
        PathBuf::from(working_dir).join("providers.toml"),
        std::env::current_dir().ok()?.join("providers.toml"),
    ];

    candidates.into_iter().find(|path| path.exists())
}

fn resolve_executable_path(executable_path: &str, working_dir: &str) -> PathBuf {
    let path = PathBuf::from(executable_path);
    if path.is_absolute() {
        path
    } else {
        PathBuf::from(working_dir).join(path)
    }
}

fn is_expected_backend_filename(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
        return false;
    };
    let lower = name.to_lowercase();
    lower == "go-on" || lower == "go-on.exe"
}

fn validate_executable_identity(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err("file not found".to_string());
    }
    if !path.is_file() {
        return Err("path is not a file".to_string());
    }
    if !is_expected_backend_filename(path) {
        return Err("filename is not go-on/go-on.exe".to_string());
    }

    let probes = ["--version", "-V", "--help"];
    for arg in probes {
        if let Ok(output) = Command::new(path).arg(arg).output() {
            let mut merged = String::new();
            merged.push_str(&String::from_utf8_lossy(&output.stdout));
            merged.push('\n');
            merged.push_str(&String::from_utf8_lossy(&output.stderr));
            let lower = merged.to_lowercase();
            if lower.contains("go-on") {
                return Ok(());
            }
        }
    }

    Err("identity probe failed".to_string())
}

fn discover_backend_executable_in_directory(base_dir: &Path, default_exe: &str) -> Option<PathBuf> {
    let candidate_dirs = [
        base_dir.to_path_buf(),
        base_dir.join("bin"),
        base_dir.join("exec"),
        base_dir.join("backend"),
    ];

    for dir in candidate_dirs {
        let candidate = dir.join(default_exe);
        if candidate.exists()
            && candidate.is_file()
            && is_expected_backend_filename(&candidate)
            && validate_executable_identity(&candidate).is_ok()
        {
            return Some(candidate);
        }
    }

    None
}

fn discover_backend_executable(default_exe: &str) -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();

    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(parent) = current_exe.parent() {
            roots.push(parent.to_path_buf());
            if let Some(contents_dir) = parent.parent() {
                roots.push(contents_dir.to_path_buf());
                roots.push(contents_dir.join("Resources"));
                roots.push(contents_dir.join("Resources").join("backend"));
            }
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        roots.push(cwd);
    }

    roots.sort();
    roots.dedup();

    for root in roots {
        if let Some(found) = discover_backend_executable_in_directory(&root, default_exe) {
            return Some(found);
        }
    }

    None
}

fn load_env_file(path: &PathBuf) -> HashMap<String, String> {
    let mut result = HashMap::new();
    let Ok(content) = fs::read_to_string(path) else {
        return result;
    };

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = trimmed.split_once('=') {
            result.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    result
}

fn save_env_file(path: &PathBuf, entries: &HashMap<String, String>) -> Result<(), String> {
    let mut keys = entries.keys().cloned().collect::<Vec<_>>();
    keys.sort();
    let mut out = String::new();
    for k in keys {
        if let Some(v) = entries.get(&k) {
            out.push_str(&format!("{k}={v}\n"));
        }
    }
    fs::write(path, out).map_err(|e| e.to_string())
}

fn mask_token(token: &str) -> String {
    if token.len() <= 8 {
        return "********".to_string();
    }
    format!("{}****{}", &token[..4], &token[token.len() - 4..])
}

fn infer_env_var(provider: &str) -> String {
    let normalized = provider
        .trim()
        .to_uppercase()
        .replace('-', "_")
        .replace(' ', "_");
    format!("{normalized}_API_KEY")
}

fn fallback_provider_catalog() -> Vec<ProviderCatalogSpec> {
    vec![
        ProviderCatalogSpec {
            name: "anthropic".to_string(),
            agent_type: "claude".to_string(),
            url: Some("https://api.anthropic.com".to_string()),
            chat_path: None,
            model: Some("claude-3-7-sonnet-latest".to_string()),
            api_key_env: Some("ANTHROPIC_API_KEY".to_string()),
            secret_key_env: None,
            anthropic_version: Some("2023-06-01".to_string()),
            max_tokens: Some(4096),
            supports_system: Some(true),
        },
        ProviderCatalogSpec {
            name: "copilot".to_string(),
            agent_type: "copilot".to_string(),
            url: Some("http://127.0.0.1:8080".to_string()),
            chat_path: None,
            model: Some("copilot".to_string()),
            api_key_env: Some("GITHUB_COPILOT_TOKEN".to_string()),
            secret_key_env: None,
            anthropic_version: None,
            max_tokens: None,
            supports_system: Some(true),
        },
        ProviderCatalogSpec {
            name: "gemini".to_string(),
            agent_type: "gemini".to_string(),
            url: Some("https://generativelanguage.googleapis.com/v1beta/openai".to_string()),
            chat_path: Some("/chat/completions".to_string()),
            model: Some("gemini-2.0-flash".to_string()),
            api_key_env: Some("GEMINI_API_KEY".to_string()),
            secret_key_env: None,
            anthropic_version: None,
            max_tokens: None,
            supports_system: Some(true),
        },
        ProviderCatalogSpec {
            name: "openai".to_string(),
            agent_type: "openai".to_string(),
            url: Some("https://api.openai.com/v1".to_string()),
            chat_path: None,
            model: Some("gpt-4o-mini".to_string()),
            api_key_env: Some("OPENAI_API_KEY".to_string()),
            secret_key_env: None,
            anthropic_version: None,
            max_tokens: None,
            supports_system: Some(true),
        },
    ]
}

fn load_provider_catalog(working_dir: &str) -> Vec<ProviderCatalogSpec> {
    let Some(path) = provider_catalog_path(working_dir) else {
        return fallback_provider_catalog();
    };

    let Ok(content) = fs::read_to_string(path) else {
        return fallback_provider_catalog();
    };

    toml::from_str::<ProviderCatalogFile>(&content)
        .map(|catalog| catalog.providers)
        .unwrap_or_else(|_| fallback_provider_catalog())
}

fn load_toml_document(path: &Path) -> Result<DocumentMut, String> {
    if !path.exists() {
        return Ok(DocumentMut::new());
    }

    let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
    if content.trim().is_empty() {
        return Ok(DocumentMut::new());
    }

    content
        .parse::<DocumentMut>()
        .map_err(|e| format!("failed to parse TOML {}: {e}", path.to_string_lossy()))
}

fn read_configured_agent_settings(
    config_path: &Path,
) -> HashMap<String, (Option<String>, Option<String>)> {
    let Ok(doc) = load_toml_document(config_path) else {
        return HashMap::new();
    };

    let mut result = HashMap::new();
    let Some(agents) = doc["agents"].as_table_like() else {
        return result;
    };

    for (name, item) in agents.iter() {
        let Some(table) = item.as_table_like() else {
            continue;
        };
        let model = table
            .get("model")
            .and_then(|item| item.as_str())
            .map(|value| value.to_string());
        let env_var = table
            .get("api_key_env")
            .and_then(|item| item.as_str())
            .or_else(|| table.get("secret_key_env").and_then(|item| item.as_str()))
            .map(|value| value.to_string());
        result.insert(name.to_string(), (model, env_var));
    }

    result
}

fn ensure_table(item: &mut Item) -> &mut Table {
    if !item.is_table() {
        *item = Item::Table(Table::new());
    }
    item.as_table_mut().expect("table just created")
}

fn set_string(table: &mut Table, key: &str, value_opt: Option<&str>) {
    if let Some(value_text) = value_opt {
        table[key] = value(value_text);
    } else {
        table.remove(key);
    }
}

fn set_bool(table: &mut Table, key: &str, value_opt: Option<bool>) {
    if let Some(value_bool) = value_opt {
        table[key] = value(value_bool);
    } else {
        table.remove(key);
    }
}

fn set_integer(table: &mut Table, key: &str, value_opt: Option<u32>) {
    if let Some(value_int) = value_opt {
        table[key] = value(i64::from(value_int));
    } else {
        table.remove(key);
    }
}

#[tauri::command]
pub fn list_provider_catalog(
    state: State<'_, AppState>,
) -> Result<Vec<ProviderCatalogEntry>, String> {
    let inner = state
        .0
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;
    let working_dir = inner.config.working_dir.clone();
    drop(inner);

    let providers = load_provider_catalog(&working_dir);
    let configured = read_configured_agent_settings(&config_file_path(&working_dir));

    let mut entries = providers
        .into_iter()
        .map(|spec| {
            let configured_settings = configured.get(&spec.name).cloned().unwrap_or((None, None));
            ProviderCatalogEntry {
                name: spec.name,
                agent_type: spec.agent_type,
                default_model: spec.model,
                api_key_env: spec.api_key_env,
                secret_key_env: spec.secret_key_env,
                url: spec.url,
                chat_path: spec.chat_path,
                supports_system: spec.supports_system,
                configured_model: configured_settings.0,
                configured_env_var: configured_settings.1,
            }
        })
        .collect::<Vec<_>>();

    entries.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(entries)
}

#[tauri::command]
pub fn save_provider_selection(
    state: State<'_, AppState>,
    provider: String,
    model: String,
    env_var: Option<String>,
) -> Result<ProviderSelectionSaveResult, String> {
    let provider_name = provider.trim().to_string();
    if provider_name.is_empty() {
        return Err("provider is empty".to_string());
    }

    let model_name = if model.trim().is_empty() {
        "auto".to_string()
    } else {
        model.trim().to_string()
    };

    let inner = state
        .0
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;
    let working_dir = inner.config.working_dir.clone();
    drop(inner);

    let providers = load_provider_catalog(&working_dir);
    let spec = providers
        .into_iter()
        .find(|entry| entry.name == provider_name)
        .ok_or_else(|| format!("unknown provider: {provider_name}"))?;

    let config_path = config_file_path(&working_dir);
    let mut doc = load_toml_document(&config_path)?;
    let agents = ensure_table(&mut doc["agents"]);
    let agent = ensure_table(&mut agents[&provider_name]);

    set_string(agent, "type", Some(spec.agent_type.as_str()));
    set_string(agent, "url", spec.url.as_deref());
    set_string(agent, "chat_path", spec.chat_path.as_deref());
    set_string(
        agent,
        "api_key_env",
        env_var
            .as_deref()
            .filter(|value_text| !value_text.trim().is_empty())
            .or(spec.api_key_env.as_deref()),
    );
    set_string(agent, "secret_key_env", spec.secret_key_env.as_deref());
    set_string(
        agent,
        "anthropic_version",
        spec.anthropic_version.as_deref(),
    );
    set_string(agent, "model", Some(model_name.as_str()));
    set_integer(agent, "max_tokens", spec.max_tokens);
    set_bool(agent, "supports_system", spec.supports_system);

    if let Some(default_phase) = doc["default_phase"]
        .as_str()
        .map(|value_text| value_text.to_string())
    {
        if let Some(phases) = doc["phases"].as_table_mut() {
            if let Some(phase_item) = phases.get_mut(&default_phase) {
                if phase_item.is_table() {
                    let phase_table = phase_item.as_table_mut().expect("checked table");
                    let mut agents = Array::default();
                    agents.push(provider_name.as_str());
                    phase_table["agents"] = value(agents);
                }
            }
        }
    }

    fs::write(&config_path, doc.to_string()).map_err(|e| e.to_string())?;

    Ok(ProviderSelectionSaveResult {
        provider: provider_name.clone(),
        model: model_name.clone(),
        config_path: config_path.to_string_lossy().to_string(),
        note: format!("saved provider {provider_name} with model {model_name} and updated default phase routing"),
    })
}

#[tauri::command]
pub fn configure_service(
    state: State<'_, AppState>,
    executable_path: String,
    working_dir: String,
    protocol_mode: Option<String>,
) -> Result<(), String> {
    let mut inner = state
        .0
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;

    inner.config.executable_path = executable_path;
    inner.config.working_dir = working_dir.clone();
    inner.config.log_path = PathBuf::from(&working_dir)
        .join("go-on.log")
        .to_string_lossy()
        .to_string();
    inner.config.protocol_mode = protocol_mode
        .as_deref()
        .and_then(normalize_protocol_mode)
        .map(|s| s.to_string());

    let env_path = env_file_path(&working_dir);
    inner.config.extra_env = load_env_file(&env_path);
    Ok(())
}

#[tauri::command]
pub fn configure_service_by_executable(
    state: State<'_, AppState>,
    executable_path: String,
) -> Result<(), String> {
    let trimmed = executable_path.trim();
    if trimmed.is_empty() {
        return Err("executable path is empty".to_string());
    }

    let exe = PathBuf::from(trimmed);
    let working_dir = exe
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_string_lossy()
        .to_string();

    let mut inner = state
        .0
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;

    inner.config.executable_path = exe.to_string_lossy().to_string();
    inner.config.working_dir = working_dir.clone();
    inner.config.log_path = PathBuf::from(&working_dir)
        .join("go-on.log")
        .to_string_lossy()
        .to_string();

    let env_path = env_file_path(&working_dir);
    inner.config.extra_env = load_env_file(&env_path);
    Ok(())
}

#[tauri::command]
pub fn configure_service_by_directory(
    state: State<'_, AppState>,
    directory_path: String,
) -> Result<(), String> {
    let trimmed = directory_path.trim();
    if trimmed.is_empty() {
        return Err("directory path is empty".to_string());
    }

    let dir = PathBuf::from(trimmed);
    if !dir.exists() {
        return Err(format!("directory not found: {}", dir.to_string_lossy()));
    }
    if !dir.is_dir() {
        return Err(format!("not a directory: {}", dir.to_string_lossy()));
    }

    let default_exe = if cfg!(target_os = "windows") {
        "go-on.exe"
    } else {
        "go-on"
    };

    let Some(found_exe) = discover_backend_executable_in_directory(&dir, default_exe) else {
        return Err(
            "go-on executable not found under selected directory (checked root/bin/exec/backend)"
                .to_string(),
        );
    };

    let working_dir = found_exe
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_string_lossy()
        .to_string();

    let mut inner = state
        .0
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;

    inner.config.executable_path = found_exe.to_string_lossy().to_string();
    inner.config.working_dir = working_dir.clone();
    inner.config.log_path = PathBuf::from(&working_dir)
        .join("go-on.log")
        .to_string_lossy()
        .to_string();

    let env_path = env_file_path(&working_dir);
    inner.config.extra_env = load_env_file(&env_path);
    Ok(())
}

#[tauri::command]
pub fn backend_executable_exists(state: State<'_, AppState>) -> Result<bool, String> {
    let inner = state
        .0
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;

    let configured = inner.config.executable_path.trim();
    if configured.is_empty() {
        return Ok(false);
    }

    let resolved =
        resolve_executable_path(&inner.config.executable_path, &inner.config.working_dir);
    if !resolved.exists() || !resolved.is_file() {
        return Ok(false);
    }

    if !is_expected_backend_filename(&resolved) {
        return Ok(false);
    }

    Ok(validate_executable_identity(&resolved).is_ok())
}

#[tauri::command]
pub fn auto_configure_backend_path(
    state: State<'_, AppState>,
) -> Result<AutoConfigureResult, String> {
    let default_exe = if cfg!(target_os = "windows") {
        "go-on.exe"
    } else {
        "go-on"
    };

    let Some(found_exe) = discover_backend_executable(default_exe) else {
        return Ok(AutoConfigureResult {
            linked: false,
            executable_path: None,
            reason: "not_found_or_unverified".to_string(),
        });
    };

    if let Err(reason) = validate_executable_identity(&found_exe) {
        return Ok(AutoConfigureResult {
            linked: false,
            executable_path: Some(found_exe.to_string_lossy().to_string()),
            reason,
        });
    }

    let working_dir = found_exe
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_string_lossy()
        .to_string();

    let mut inner = state
        .0
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;

    inner.config.executable_path = found_exe.to_string_lossy().to_string();
    inner.config.working_dir = working_dir.clone();
    inner.config.log_path = PathBuf::from(&working_dir)
        .join("go-on.log")
        .to_string_lossy()
        .to_string();
    inner.config.extra_env = load_env_file(&env_file_path(&working_dir));

    Ok(AutoConfigureResult {
        linked: true,
        executable_path: Some(found_exe.to_string_lossy().to_string()),
        reason: "linked".to_string(),
    })
}

#[tauri::command]
pub fn exit_app(app: tauri::AppHandle) {
    app.exit(0);
}

#[tauri::command]
pub fn reset_default_settings(state: State<'_, AppState>) -> Result<String, String> {
    let inner = state
        .0
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;
    let working_dir = inner.config.working_dir.clone();
    drop(inner);

    let template = config_template_path(&working_dir);
    let target = config_file_path(&working_dir);
    if !template.exists() {
        return Err(format!(
            "template not found: {}",
            template.to_string_lossy()
        ));
    }
    fs::copy(&template, &target).map_err(|e| e.to_string())?;
    Ok(format!(
        "restored default config: {}",
        target.to_string_lossy()
    ))
}

#[tauri::command]
pub fn set_provider_api_key(
    state: State<'_, AppState>,
    provider: String,
    env_var: Option<String>,
    api_key: String,
) -> Result<String, String> {
    if api_key.trim().is_empty() {
        return Err("api key is empty".to_string());
    }

    let mut inner = state
        .0
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;

    let key = env_var
        .unwrap_or_else(|| infer_env_var(&provider))
        .trim()
        .to_string();
    inner.config.extra_env.insert(key.clone(), api_key.clone());

    let env_path = env_file_path(&inner.config.working_dir);
    let mut file_map = load_env_file(&env_path);
    file_map.insert(key.clone(), api_key);
    save_env_file(&env_path, &file_map)?;

    Ok(format!("saved {key} to {}", env_path.to_string_lossy()))
}

#[tauri::command]
pub fn clear_provider_api_key(
    state: State<'_, AppState>,
    provider: String,
    env_var: Option<String>,
) -> Result<String, String> {
    let mut inner = state
        .0
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;

    let key = env_var
        .unwrap_or_else(|| infer_env_var(&provider))
        .trim()
        .to_string();
    inner.config.extra_env.remove(&key);

    let env_path = env_file_path(&inner.config.working_dir);
    let mut file_map = load_env_file(&env_path);
    file_map.remove(&key);
    save_env_file(&env_path, &file_map)?;

    Ok(format!("cleared {key} from {}", env_path.to_string_lossy()))
}

#[tauri::command]
pub fn fetch_github_copilot_token() -> Result<CopilotTokenResult, String> {
    let candidates = ["GITHUB_COPILOT_TOKEN", "GITHUB_TOKEN", "GH_TOKEN"];
    for key in candidates {
        if let Ok(value) = std::env::var(key) {
            if !value.trim().is_empty() {
                return Ok(token_result_found(
                    format!("env:{key}"),
                    value,
                    "token found in environment".to_string(),
                ));
            }
        }
    }

    let output = Command::new("gh").args(["auth", "token"]).output();
    if let Ok(out) = output {
        if out.status.success() {
            let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !token.is_empty() {
                return Ok(token_result_found(
                    "gh auth token".to_string(),
                    token,
                    "token retrieved via GitHub CLI".to_string(),
                ));
            }
        }
    }

    // Device Flow fallback:
    // 1) First click returns user_code + verification_uri.
    // 2) User verifies on GitHub.
    // 3) Next click polls and returns the access token.
    let lock = device_flow_session();
    let mut guard = lock
        .lock()
        .map_err(|_| "device flow session lock poisoned".to_string())?;

    if let Some(existing) = guard.as_mut() {
        if now_secs() >= existing.expires_at {
            *guard = None;
        } else {
            let poll = poll_device_flow_access_token(existing)?;
            if let Some(token) = poll.access_token {
                *guard = None;
                return Ok(token_result_found(
                    "github_device_flow".to_string(),
                    token,
                    "token retrieved via GitHub device verification".to_string(),
                ));
            }

            let err = poll
                .error
                .unwrap_or_else(|| "authorization_pending".to_string());
            let desc = poll.error_description.unwrap_or_default();

            if err == "slow_down" {
                existing.interval_seconds = existing.interval_seconds.saturating_add(5);
            }
            if err == "expired_token" || err == "access_denied" {
                *guard = None;
                return Ok(CopilotTokenResult {
                    found: false,
                    source: "github_device_flow".to_string(),
                    token_masked: None,
                    token_plain: None,
                    verification_uri: None,
                    user_code: None,
                    expires_in_seconds: None,
                    poll_interval_seconds: None,
                    note: format!(
                        "device flow {err}; please click import again to generate a new verification code. {desc}"
                    )
                    .trim()
                    .to_string(),
                });
            }

            return Ok(token_result_pending(
                "github_device_flow".to_string(),
                existing,
                format!(
                    "verification pending: open {} and enter code {}. Then click import again. {}",
                    existing.verification_uri, existing.user_code, desc
                )
                .trim()
                .to_string(),
            ));
        }
    }

    let session = start_device_flow()?;
    let result = token_result_pending(
        "github_device_flow".to_string(),
        &session,
        format!(
            "open {} and enter code {} to authorize. Then click import again.",
            session.verification_uri, session.user_code
        ),
    );
    *guard = Some(session);
    Ok(result)
}
