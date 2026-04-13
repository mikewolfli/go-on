use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;
use tauri::State;

use crate::state::AppState;

#[tauri::command]
pub fn invoke_runtime_rpc(
    state: State<'_, AppState>,
    method: String,
    params_json: Option<String>,
) -> Result<String, String> {
    let inner = state
        .0
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;
    let executable = inner.config.executable_path.clone();
    let working_dir = inner.config.working_dir.clone();
    let mut env_overrides = inner.config.extra_env.clone();
    drop(inner);

    if method.trim().is_empty() {
        return Err("method cannot be empty".to_string());
    }

    let config_path = std::path::Path::new(&working_dir).join("config.toml");
    let executable_path = {
        let path = std::path::PathBuf::from(&executable);
        if path.is_absolute() {
            path
        } else {
            std::path::PathBuf::from(&working_dir).join(path)
        }
    };

    let mut cmd = Command::new(&executable_path);
    cmd.current_dir(&working_dir)
        .arg("--config")
        .arg(config_path)
        .arg("--verbose")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    for (k, v) in env_overrides.drain() {
        cmd.env(k, v);
    }

    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "failed to open child stdin".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "failed to open child stdout".to_string())?;

    let (tx, rx) = mpsc::channel::<Result<Value, String>>();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(content) => {
                    if content.trim().is_empty() {
                        continue;
                    }
                    if let Ok(v) = serde_json::from_str::<Value>(&content) {
                        let _ = tx.send(Ok(v));
                        return;
                    }
                }
                Err(err) => {
                    let _ = tx.send(Err(err.to_string()));
                    return;
                }
            }
        }
        let _ = tx.send(Err("no JSON-RPC response received".to_string()));
    });

    let params_value = if let Some(raw) = params_json {
        if raw.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str::<Value>(&raw).unwrap_or_else(|_| json!({}))
        }
    } else {
        json!({})
    };

    let req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params_value
    });

    let line = format!("{}\n", req);
    stdin
        .write_all(line.as_bytes())
        .map_err(|e| e.to_string())?;
    stdin.flush().map_err(|e| e.to_string())?;

    let result = rx
        .recv_timeout(Duration::from_secs(12))
        .map_err(|_| "rpc timeout".to_string())?;

    let _ = child.kill();
    let _ = child.wait();

    match result {
        Ok(v) => {
            if let Some(err) = v.get("error") {
                let code = err.get("code").and_then(|x| x.as_i64()).unwrap_or(-1);
                let message = err
                    .get("message")
                    .and_then(|x| x.as_str())
                    .unwrap_or("unknown rpc error");
                return Err(format!("rpc_error:{code}:{message}"));
            }

            let payload = v.get("result").cloned().unwrap_or(v);
            Ok(serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string()))
        }
        Err(err) => Err(err),
    }
}
