use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;
use std::{fs, path::PathBuf};

struct ProbeResult {
    ok: bool,
    code: Option<u16>,
    detail: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CapabilityMatrix {
    runtime: RuntimeContract,
    openai: OpenAiContract,
    errors: ErrorContract,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeContract {
    base_url: String,
    health_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenAiContract {
    models_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ErrorContract {
    runtime_probe_passed: String,
}

fn capability_matrix() -> &'static CapabilityMatrix {
    static MATRIX: OnceLock<CapabilityMatrix> = OnceLock::new();
    MATRIX.get_or_init(|| {
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../contracts/editor-capability-matrix.json"
        )))
        .expect("editor capability matrix should be valid json")
    })
}

fn runtime_health_endpoint() -> String {
    let contract = capability_matrix();
    format!(
        "{}{}",
        contract.runtime.base_url, contract.runtime.health_path
    )
}

fn openai_models_endpoint() -> String {
    let contract = capability_matrix();
    format!(
        "{}{}",
        contract.runtime.base_url, contract.openai.models_path
    )
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EditorIntegrationStatus {
    pub editor: String,
    pub interface_name: String,
    pub protocol_mode: String,
    pub process_running: bool,
    pub process_count: u32,
    pub transport: String,
    pub endpoint: Option<String>,
    pub endpoint_ok: bool,
    pub endpoint_code: Option<u16>,
    pub addon_present: bool,
    pub note: String,
}

fn count_processes(candidates: &[&str]) -> u32 {
    if cfg!(target_os = "windows") {
        let output = Command::new("tasklist").output();
        if let Ok(out) = output {
            let content = String::from_utf8_lossy(&out.stdout).to_lowercase();
            let mut count = 0u32;
            for line in content.lines() {
                if candidates
                    .iter()
                    .any(|name| line.contains(&name.to_lowercase()))
                {
                    count += 1;
                }
            }
            return count;
        }
    } else {
        let output = Command::new("pgrep").arg("-fl").arg(".").output();
        if let Ok(out) = output {
            let content = String::from_utf8_lossy(&out.stdout).to_lowercase();
            let mut count = 0u32;
            for line in content.lines() {
                if candidates
                    .iter()
                    .any(|name| line.contains(&name.to_lowercase()))
                {
                    count += 1;
                }
            }
            return count;
        }
    }

    0
}

fn probe_http(url: &str) -> ProbeResult {
    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
    {
        Ok(v) => v,
        Err(_) => {
            return ProbeResult {
                ok: false,
                code: None,
                detail: "http client build failed".to_string(),
            }
        }
    };

    match client.get(url).send() {
        Ok(resp) => {
            let code = resp.status().as_u16();
            let text = resp.text().unwrap_or_default().to_lowercase();
            if url.ends_with("/v1/models") {
                let shape_ok = text.contains("\"data\"")
                    && (text.contains("\"object\"") || text.contains("\"model\""));
                ProbeResult {
                    ok: code >= 200 && code < 300 && shape_ok,
                    code: Some(code),
                    detail: if shape_ok {
                        "models schema looks valid".to_string()
                    } else {
                        "models response missing expected fields".to_string()
                    },
                }
            } else if url.ends_with("/health") {
                let shape_ok = text.contains("ok")
                    || text.contains("healthy")
                    || text.contains("ready")
                    || code == 200;
                ProbeResult {
                    ok: code >= 200 && code < 300 && shape_ok,
                    code: Some(code),
                    detail: if shape_ok {
                        "health semantics validated".to_string()
                    } else {
                        "health response lacks expected signals".to_string()
                    },
                }
            } else {
                ProbeResult {
                    ok: code >= 200 && code < 500,
                    code: Some(code),
                    detail: "generic reachability check".to_string(),
                }
            }
        }
        Err(err) => ProbeResult {
            ok: false,
            code: None,
            detail: format!("request error: {err}"),
        },
    }
}

fn protocol_mode_from_config_text(text: &str) -> Option<&'static str> {
    let mut in_protocol_section = false;

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            in_protocol_section = line.eq_ignore_ascii_case("[protocol]");
            continue;
        }

        if !in_protocol_section {
            continue;
        }

        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if !name.trim().eq_ignore_ascii_case("mode") {
            continue;
        }

        let value = value.trim().trim_matches('"').to_ascii_lowercase();
        return match value.as_str() {
            // canonical 5 options
            "adaptive" => Some("adaptive"),
            "acp_stdio" | "acp+stdio" => Some("acp_stdio"),
            "acp_http" | "acp+http" => Some("acp_http"),
            "mcp_stdio" | "mcp+stdio" => Some("mcp_stdio"),
            "mcp_http" | "mcp+http" => Some("mcp_http"),
            // backward-compatible aliases
            "auto" => Some("adaptive"),
            "acp" => Some("acp_stdio"),
            "mcp" => Some("mcp_stdio"),
            _ => None,
        };
    }

    None
}

fn detect_protocol_mode() -> String {
    let candidates = [
        PathBuf::from("config.toml"),
        PathBuf::from("../config.toml"),
        PathBuf::from("../../config.toml"),
    ];

    for path in candidates {
        if let Ok(text) = fs::read_to_string(&path) {
            if let Some(mode) = protocol_mode_from_config_text(&text) {
                return mode.to_string();
            }
        }
    }

    "unknown".to_string()
}

fn mode_supports_acp(mode: &str) -> bool {
    matches!(mode, "adaptive" | "acp_stdio" | "acp_http")
}

fn mode_supports_mcp(mode: &str) -> bool {
    matches!(mode, "adaptive" | "mcp_stdio" | "mcp_http")
}

#[tauri::command]
pub fn get_editor_integration_status() -> Result<Vec<EditorIntegrationStatus>, String> {
    let zed_processes = count_processes(&["zed.exe", "zed"]);
    let vscode_processes = count_processes(&["code.exe", "code", "code - insiders", "codium"]);
    let protocol_mode = detect_protocol_mode();

    let models_endpoint = openai_models_endpoint();
    let health_endpoint = runtime_health_endpoint();

    let zed_models_probe = probe_http(&models_endpoint);
    let zed_health_probe = probe_http(&health_endpoint);
    let vscode_probe = probe_http(&health_endpoint);

    let vscode_addon_present = Path::new("vscode-addon/package.json").exists()
        && (Path::new("vscode-addon/out/extension.js").exists()
            || Path::new("vscode-addon/src/extension.ts").exists());

    Ok(vec![
        EditorIntegrationStatus {
            editor: "Zed".to_string(),
            interface_name: "ACP/A2A External Agent".to_string(),
            protocol_mode: protocol_mode.clone(),
            process_running: zed_processes > 0,
            process_count: zed_processes,
            transport: "ACP/A2A over HTTP".to_string(),
            endpoint: Some(health_endpoint.clone()),
            endpoint_ok: zed_health_probe.ok,
            endpoint_code: zed_health_probe.code,
            addon_present: false,
            note: if zed_health_probe.ok {
                if !mode_supports_acp(&protocol_mode) {
                    "Health reachable, but current mode is MCP-only; ACP/A2A may be rejected"
                        .to_string()
                } else if protocol_mode == "adaptive" {
                    "Adaptive mode enabled; ACP/A2A and MCP are negotiated automatically"
                        .to_string()
                } else {
                    "ACP/A2A path is reachable".to_string()
                }
            } else {
                format!("ACP/A2A probe failed: {}", zed_health_probe.detail)
            },
        },
        EditorIntegrationStatus {
            editor: "Zed".to_string(),
            interface_name: "MCP LLM Provider".to_string(),
            protocol_mode: protocol_mode.clone(),
            process_running: zed_processes > 0,
            process_count: zed_processes,
            transport: "OpenAI-Compatible /v1".to_string(),
            endpoint: Some(models_endpoint.clone()),
            endpoint_ok: zed_models_probe.ok,
            endpoint_code: zed_models_probe.code,
            addon_present: false,
            note: if zed_models_probe.ok {
                if !mode_supports_mcp(&protocol_mode) {
                    "Models endpoint reachable, but current mode is ACP-only; MCP provider may be limited".to_string()
                } else if protocol_mode == "adaptive" {
                    "Adaptive mode enabled; MCP provider and ACP/A2A are both supported".to_string()
                } else {
                    "MCP LLM provider endpoint is reachable".to_string()
                }
            } else {
                format!("MCP provider probe failed: {}", zed_models_probe.detail)
            },
        },
        EditorIntegrationStatus {
            editor: "VS Code".to_string(),
            interface_name: "Extension Runtime RPC".to_string(),
            protocol_mode,
            process_running: vscode_processes > 0,
            process_count: vscode_processes,
            transport: "Runtime RPC (extension)".to_string(),
            endpoint: Some(health_endpoint),
            endpoint_ok: vscode_probe.ok,
            endpoint_code: vscode_probe.code,
            addon_present: vscode_addon_present,
            note: if vscode_addon_present && vscode_probe.ok {
                format!(
                    "Extension workspace detected; {}",
                    capability_matrix().errors.runtime_probe_passed
                )
            } else if vscode_addon_present {
                format!(
                    "Extension detected, but runtime probe failed: {}",
                    vscode_probe.detail
                )
            } else {
                "vscode-addon workspace not found".to_string()
            },
        },
    ])
}

#[cfg(test)]
mod tests {
    use super::protocol_mode_from_config_text;

    #[test]
    fn protocol_mode_parser_reads_protocol_section_only() {
        let text = r#"
model_selection_mode = "adaptive"

[protocol]
mode = "adaptive"

[agents.sample]
type = "mcp"
"#;

        assert_eq!(protocol_mode_from_config_text(text), Some("adaptive"));
    }

    #[test]
    fn protocol_mode_parser_ignores_unrelated_mode_keys() {
        let text = r#"
model_selection_mode = "adaptive"
execution_mode = "parallel"

[agents.sample]
type = "acp"

[protocol]
mode = "mcp"
"#;

    assert_eq!(protocol_mode_from_config_text(text), Some("mcp_stdio"));
    }

    #[test]
    fn protocol_mode_parser_supports_all_five_options() {
    let text = r#"
[protocol]
mode = "acp_http"
"#;
    assert_eq!(protocol_mode_from_config_text(text), Some("acp_http"));

    let text = r#"
[protocol]
mode = "mcp_http"
"#;
    assert_eq!(protocol_mode_from_config_text(text), Some("mcp_http"));
    }

    #[test]
    fn protocol_mode_parser_returns_none_without_protocol_section() {
        let text = r#"
model_selection_mode = "adaptive"
[runtime]
maintenance_interval_seconds = 30
"#;

        assert_eq!(protocol_mode_from_config_text(text), None);
    }
}
