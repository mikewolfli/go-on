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
        super::make_room_for_terminal();
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
                read_offset: 0,
                truncated: false,
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

/// Drain available output from a blocking pipe into the process output buffer.
///
/// Reads until EOF, an error, or a `WouldBlock`. The caller runs on the
/// blocking pool (`spawn_blocking`), so no async worker is starved. The buffer
/// is capped at [`super::MAX_TERMINAL_OUTPUT_BYTES`]: beyond it the oldest
/// bytes are dropped and the `truncated` flag is set, so an unread
/// firehose cannot grow the buffer without bound.
fn drain(reader: &mut impl std::io::Read, output_buffer: &mut Vec<u8>, truncated: &mut bool) {
    let mut buf = [0u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                output_buffer.extend_from_slice(&buf[..n]);
                if output_buffer.len() > super::MAX_TERMINAL_OUTPUT_BYTES {
                    let drop = output_buffer.len() - super::MAX_TERMINAL_OUTPUT_BYTES;
                    output_buffer.drain(..drop);
                    *truncated = true;
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(_) => break,
        }
    }
}

/// Handle `terminal/output` — reads buffered terminal output.
///
/// Returns only the output accumulated since the previous `terminal/output`
/// call (incremental semantics): the `read_offset` is advanced past the bytes
/// returned, so a long-lived process never re-serializes its full history.
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
                drain(stdout, &mut proc.output_buffer, &mut proc.truncated);
            }
            if let Some(ref mut stderr) = proc.child.stderr {
                drain(stderr, &mut proc.output_buffer, &mut proc.truncated);
            }

            let exit_code = proc.child.try_wait().ok().flatten().map(|status| {
                proc.exited = true;
                status.code()
            });
            if let Some(code) = exit_code {
                proc.exit_code = code;
            }

            // Incremental read: only bytes past the previous read offset.
            let new_bytes: Vec<u8> = proc.output_buffer[proc.read_offset..].to_vec();
            proc.read_offset = proc.output_buffer.len();
            let output_str = String::from_utf8_lossy(&new_bytes).to_string();
            let is_truncated = proc.truncated;
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
pub async fn handle_terminal_release(_server: &AcpServer, params: Value) -> Result<DispatchOutput> {
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
pub async fn handle_terminal_kill(_server: &AcpServer, params: Value) -> Result<DispatchOutput> {
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
pub async fn terminal_wait_for_exit_payload(_server: &AcpServer, params: Value) -> Result<Value> {
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
