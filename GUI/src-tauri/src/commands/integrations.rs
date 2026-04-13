use serde::Serialize;
use std::path::Path;
use std::process::Command;
use std::{fs, path::PathBuf};

struct ProbeResult {
    ok: bool,
    code: Option<u16>,
    detail: String,
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

fn detect_protocol_mode() -> String {
    let candidates = [
        PathBuf::from("config.toml"),
        PathBuf::from("../config.toml"),
        PathBuf::from("../../config.toml"),
    ];

    for path in candidates {
        if let Ok(text) = fs::read_to_string(&path) {
            let lower = text.to_lowercase();
            if let Some(idx) = lower.find("mode") {
                let tail = &lower[idx..];
                if tail.contains("\"acp\"") {
                    return "acp".to_string();
                }
                if tail.contains("\"mcp\"") {
                    return "mcp".to_string();
                }
                if tail.contains("\"auto\"") {
                    return "auto-adaptive".to_string();
                }
            }
        }
    }

    "unknown".to_string()
}

#[tauri::command]
pub fn get_editor_integration_status() -> Result<Vec<EditorIntegrationStatus>, String> {
    let zed_processes = count_processes(&["zed.exe", "zed"]);
    let vscode_processes = count_processes(&["code.exe", "code", "code - insiders", "codium"]);
    let protocol_mode = detect_protocol_mode();

    let zed_models_probe = probe_http("http://127.0.0.1:8090/v1/models");
    let zed_health_probe = probe_http("http://127.0.0.1:8090/health");
    let vscode_probe = probe_http("http://127.0.0.1:8090/health");

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
            endpoint: Some("http://127.0.0.1:8090/health".to_string()),
            endpoint_ok: zed_health_probe.ok,
            endpoint_code: zed_health_probe.code,
            addon_present: false,
            note: if zed_health_probe.ok {
                if protocol_mode == "mcp" {
                    "Health reachable, but protocol mode is mcp; ACP/A2A may be rejected"
                        .to_string()
                } else if protocol_mode == "auto-adaptive" {
                    "Auto-adaptive mode enabled; ACP/A2A and MCP are negotiated automatically"
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
            endpoint: Some("http://127.0.0.1:8090/v1/models".to_string()),
            endpoint_ok: zed_models_probe.ok,
            endpoint_code: zed_models_probe.code,
            addon_present: false,
            note: if zed_models_probe.ok {
                if protocol_mode == "acp" {
                    "Models endpoint reachable, but protocol mode is acp; MCP provider may be limited".to_string()
                } else if protocol_mode == "auto-adaptive" {
                    "Auto-adaptive mode enabled; MCP provider and ACP/A2A are both supported"
                        .to_string()
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
            endpoint: Some("http://127.0.0.1:8090/health".to_string()),
            endpoint_ok: vscode_probe.ok,
            endpoint_code: vscode_probe.code,
            addon_present: vscode_addon_present,
            note: if vscode_addon_present && vscode_probe.ok {
                "Extension workspace detected; runtime.health semantic probe passed".to_string()
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
