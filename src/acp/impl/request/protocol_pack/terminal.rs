use super::*;

// ── Terminal handlers ───────────────────────────────────────────────────

pub async fn terminal_create_payload(_server: &AcpServer, params: Value) -> Result<Value> {
    let command = params
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let args: Vec<String> = params
        .get("args")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let cwd = params
        .get("cwd")
        .and_then(Value::as_str)
        .map(ToString::to_string);

    let terminal_id = super::generate_terminal_id();

    let mut cmd = std::process::Command::new(&command);
    cmd.args(&args);
    if let Some(ref dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let child = cmd
        .spawn()
        .map_err(|e| anyhow::anyhow!("failed to spawn terminal process '{}': {}", command, e))?;

    {
        let mut state = super::acp_terminal_state()
            .lock()
            .unwrap_or_else(|poisoned| {
                warn!("ACP terminal state lock poisoned in handle_terminal_create, recovering");
                poisoned.into_inner()
            });
        state.insert(
            terminal_id.clone(),
            super::TerminalProcess {
                child,
                output_buffer: Vec::new(),
                exited: false,
                exit_code: None,
            },
        );
    }

    Ok(serde_json::to_value(
        &crate::schema::CreateTerminalResponse {
            terminal_id: crate::schema::TerminalId::new(&terminal_id),
            meta: None,
        },
    )?)
}

/// Handle `terminal/output` — reads buffered terminal output.
pub async fn terminal_output_payload(_server: &AcpServer, params: Value) -> Result<Value> {
    let terminal_id = params
        .get("terminalId")
        .and_then(Value::as_str)
        .unwrap_or_default();

    let terminal_id_owned = terminal_id.to_string();
    let (output, truncated, exit_status) = tokio::task::spawn_blocking(move || {
        let mut state = super::acp_terminal_state()
            .lock()
            .unwrap_or_else(|poisoned| {
                warn!("ACP terminal state lock poisoned in handle_terminal_output, recovering");
                poisoned.into_inner()
            });
        if let Some(proc) = state.get_mut(&terminal_id_owned) {
            if let Some(ref mut stdout) = proc.child.stdout {
                use std::io::Read;
                let mut buf = [0u8; 4096];
                loop {
                    match stdout.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => proc.output_buffer.extend_from_slice(&buf[..n]),
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(_) => break,
                    }
                }
            }
            if let Some(ref mut stderr) = proc.child.stderr {
                use std::io::Read;
                let mut buf = [0u8; 4096];
                loop {
                    match stderr.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => proc.output_buffer.extend_from_slice(&buf[..n]),
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(_) => break,
                    }
                }
            }

            let exit_code = proc.child.try_wait().ok().flatten().map(|status| {
                proc.exited = true;
                status.code()
            });
            if let Some(code) = exit_code {
                proc.exit_code = code;
            }

            let output_str = String::from_utf8_lossy(&proc.output_buffer).to_string();
            let is_truncated = proc.output_buffer.len() > 65536;
            let exit = proc
                .exit_code
                .map(|code| crate::schema::TerminalExitStatus {
                    exit_code: Some(code as u32),
                    signal: None,
                    meta: None,
                });

            (output_str, is_truncated, exit)
        } else {
            (String::new(), false, None)
        }
    })
    .await
    .unwrap_or((String::new(), false, None));

    Ok(serde_json::to_value(
        &crate::schema::TerminalOutputResponse {
            output,
            truncated,
            exit_status,
            meta: None,
        },
    )?)
}

/// Handle `terminal/release` — releases terminal resources.
pub async fn handle_terminal_release(
    _server: &AcpServer,
    params: Value,
) -> Result<DispatchOutput> {
    let terminal_id = params
        .get("terminalId")
        .and_then(Value::as_str)
        .unwrap_or_default();

    if !terminal_id.is_empty() {
        let proc =
            {
                let mut state = super::acp_terminal_state().lock().unwrap_or_else(|poisoned| {
                warn!("ACP terminal state lock poisoned in handle_terminal_release, recovering");
                poisoned.into_inner()
            });
                state.remove(terminal_id)
            };
        if let Some(mut p) = proc {
            let _ = p.child.kill();
            tokio::task::spawn_blocking(move || {
                let _ = p.child.wait();
            })
            .await
            .ok();
        }
    }

    Ok(DispatchOutput::empty())
}

/// Handle `terminal/kill` — kills a terminal process.
pub async fn handle_terminal_kill(
    _server: &AcpServer,
    params: Value,
) -> Result<DispatchOutput> {
    let terminal_id = params
        .get("terminalId")
        .and_then(Value::as_str)
        .unwrap_or_default();

    if !terminal_id.is_empty() {
        let mut state = super::acp_terminal_state()
            .lock()
            .unwrap_or_else(|poisoned| {
                warn!("ACP terminal state lock poisoned in handle_terminal_kill, recovering");
                poisoned.into_inner()
            });
        if let Some(proc) = state.get_mut(terminal_id) {
            let _ = proc.child.kill();
        }
    }

    Ok(DispatchOutput::empty())
}

/// Handle `terminal/wait_for_exit` — waits for a terminal process to exit.
pub async fn terminal_wait_for_exit_payload(
    _server: &AcpServer,
    params: Value,
) -> Result<Value> {
    let terminal_id = params
        .get("terminalId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let exit_code = if !terminal_id.is_empty() {
        let tid = terminal_id.clone();
        tokio::task::spawn_blocking(move || -> Option<i32> {
            let mut state = super::acp_terminal_state()
                .lock()
                .unwrap_or_else(|poisoned| {
                    warn!(
                    "ACP terminal state lock poisoned in handle_terminal_wait_for_exit, recovering"
                );
                    poisoned.into_inner()
                });
            if let Some(proc) = state.get_mut(&tid) {
                let status = proc.child.wait().ok()?;
                proc.exited = true;
                let code = status.code();
                proc.exit_code = code;
                return code;
            }
            None
        })
        .await
        .unwrap_or(None)
    } else {
        None
    };

    Ok(serde_json::to_value(
        &crate::schema::WaitForTerminalExitResponse {
            exit_status: crate::schema::TerminalExitStatus {
                exit_code: exit_code.map(|c| c as u32),
                signal: None,
                meta: None,
            },
            meta: None,
        },
    )?)
}
