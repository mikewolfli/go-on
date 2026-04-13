use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use tauri::State;

use crate::state::AppState;

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CopilotTokenResult {
    pub found: bool,
    pub source: String,
    pub token_masked: Option<String>,
    pub token_plain: Option<String>,
    pub note: String,
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

fn discover_backend_executable(default_exe: &str) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(parent) = current_exe.parent() {
            candidates.push(parent.join(default_exe));
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join(default_exe));
    }

    if cfg!(target_os = "windows") {
        if let Ok(output) = Command::new("where").arg(default_exe).output() {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                if let Some(first) = text.lines().find(|line| !line.trim().is_empty()) {
                    candidates.push(PathBuf::from(first.trim()));
                }
            }
        }
    } else if let Ok(output) = Command::new("which").arg(default_exe).output() {
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout);
            if let Some(first) = text.lines().find(|line| !line.trim().is_empty()) {
                candidates.push(PathBuf::from(first.trim()));
            }
        }
    }

    candidates.into_iter().find(|p| {
        p.exists() && p.is_file() && is_expected_backend_filename(p) && validate_executable_identity(p).is_ok()
    })
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

#[tauri::command]
pub fn configure_service(
    state: State<'_, AppState>,
    executable_path: String,
    working_dir: String,
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
pub fn backend_executable_exists(state: State<'_, AppState>) -> Result<bool, String> {
    let inner = state
        .0
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;
    let resolved = resolve_executable_path(&inner.config.executable_path, &inner.config.working_dir);
    Ok(resolved.exists() && resolved.is_file())
}

#[tauri::command]
pub fn auto_configure_backend_path(state: State<'_, AppState>) -> Result<AutoConfigureResult, String> {
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
                return Ok(CopilotTokenResult {
                    found: true,
                    source: format!("env:{key}"),
                    token_masked: Some(mask_token(&value)),
                    token_plain: Some(value),
                    note: "token found in environment".to_string(),
                });
            }
        }
    }

    let output = Command::new("gh").args(["auth", "token"]).output();
    if let Ok(out) = output {
        if out.status.success() {
            let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !token.is_empty() {
                return Ok(CopilotTokenResult {
                    found: true,
                    source: "gh auth token".to_string(),
                    token_masked: Some(mask_token(&token)),
                    token_plain: Some(token),
                    note: "token retrieved via GitHub CLI".to_string(),
                });
            }
        }
    }

    Ok(CopilotTokenResult {
        found: false,
        source: "none".to_string(),
        token_masked: None,
        token_plain: None,
        note: "not found in env or gh cli".to_string(),
    })
}
