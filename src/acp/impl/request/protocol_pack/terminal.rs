use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

// ── Terminal handlers ───────────────────────────────────────────────────

/// Reader thread for one of the terminal's pipes.
///
/// Blocking pipe reads run on this dedicated thread — never under the global
/// terminal-state lock — so a quiet process (e.g. `sleep 30` with no output)
/// cannot stall `terminal/output`, `terminal/kill` or `terminal/release`.
/// Previously `terminal/output` held the global lock across a blocking pipe
/// read, so such a process wedged the whole terminal subsystem (the API could
/// not kill it) and occupied a blocking-pool thread indefinitely.
fn spawn_pipe_reader<R: std::io::Read + Send + 'static>(
    mut reader: R,
    output_buffer: Arc<StdMutex<Vec<u8>>>,
    truncated: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break, // EOF: pipe closed (process exited or killed)
                Ok(n) => {
                    let mut guard = output_buffer.lock().unwrap_or_else(|p| p.into_inner());
                    guard.extend_from_slice(&buf[..n]);
                    if guard.len() > super::MAX_TERMINAL_OUTPUT_BYTES {
                        let drop = guard.len() - super::MAX_TERMINAL_OUTPUT_BYTES;
                        guard.drain(..drop);
                        truncated.store(true, Ordering::Relaxed);
                    }
                }
                Err(_) => break,
            }
        }
    })
}

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

    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow::anyhow!("failed to spawn terminal process '{}': {}", command, e))?;

    // Move the pipes into dedicated reader threads BEFORE inserting into the
    // map: the threads own the blocking reads, the handlers only ever touch
    // the shared buffer under a brief lock.
    let (output_buffer, truncated, readers) = {
        let output_buffer: Arc<StdMutex<Vec<u8>>> = Arc::new(StdMutex::new(Vec::new()));
        let truncated = Arc::new(AtomicBool::new(false));
        let mut readers = Vec::new();
        if let Some(stdout) = child.stdout.take() {
            readers.push(spawn_pipe_reader(
                stdout,
                output_buffer.clone(),
                truncated.clone(),
            ));
        }
        if let Some(stderr) = child.stderr.take() {
            readers.push(spawn_pipe_reader(
                stderr,
                output_buffer.clone(),
                truncated.clone(),
            ));
        }
        (output_buffer, truncated, readers)
    };

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
                output_buffer,
                truncated,
                read_offset: 0,
                exited: false,
                exit_code: None,
                readers,
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
///
/// Returns only the output accumulated since the previous `terminal/output`
/// call (incremental semantics): the `read_offset` is advanced past the bytes
/// returned, so a long-lived process never re-serializes its full history.
///
/// Fully non-blocking: the reader threads own the pipes, so this only copies
/// the buffered delta under a brief lock and polls `try_wait` for the exit
/// status. A quiet live process cannot stall this handler or block
/// `terminal/kill`.
pub async fn terminal_output_payload(_server: &AcpServer, params: Value) -> Result<Value> {
    let terminal_id = params
        .get("terminalId")
        .and_then(Value::as_str)
        .unwrap_or_default();

    let terminal_id_owned = terminal_id.to_string();
    let (output, truncated, exit_status) = {
        let mut state = super::acp_terminal_state()
            .lock()
            .unwrap_or_else(|poisoned| {
                warn!("ACP terminal state lock poisoned in handle_terminal_output, recovering");
                poisoned.into_inner()
            });
        if let Some(proc) = state.get_mut(&terminal_id_owned) {
            let exit_code = proc.child.try_wait().ok().flatten().map(|status| {
                proc.exited = true;
                status.code()
            });
            if let Some(code) = exit_code {
                proc.exit_code = code;
            }

            // Incremental read: only bytes past the previous read offset.
            let delta: Vec<u8> = {
                let guard = proc.output_buffer.lock().unwrap_or_else(|p| p.into_inner());
                let new_bytes = guard[proc.read_offset..].to_vec();
                proc.read_offset = guard.len();
                new_bytes
            };
            let is_truncated = proc.truncated.load(Ordering::Relaxed);
            let exit = proc
                .exit_code
                .map(|code| crate::schema::TerminalExitStatus {
                    exit_code: Some(code as u32),
                    signal: None,
                    meta: None,
                });

            (
                String::from_utf8_lossy(&delta).to_string(),
                is_truncated,
                exit,
            )
        } else {
            (String::new(), false, None)
        }
    };

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
                // DETACH (don't join) the reader threads: a grandchild that
                // inherited the pipe write ends would keep a reader blocked
                // forever — joining here would hang `terminal/release` and
                // leak a blocking-pool thread. Detached readers exit on their
                // own once the pipes close (the killed process's write ends).
                drop(p.readers);
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
///
/// Polls `try_wait` on the async runtime: no blocking `wait()` and no lock
/// held across the poll, so `terminal/kill` / `terminal/release` stay
/// responsive while we wait. Capped at [`exec_common::MAX_TIMEOUT_SECS`] so a
/// never-exiting process cannot hold this request (and a blocking-pool thread)
/// forever — the client re-issues the wait after killing the process.
pub async fn terminal_wait_for_exit_payload(_server: &AcpServer, params: Value) -> Result<Value> {
    let terminal_id = params
        .get("terminalId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let exit_code = if !terminal_id.is_empty() {
        let timeout = std::time::Duration::from_secs(
            crate::orchestration::tool::exec_common::MAX_TIMEOUT_SECS,
        );
        let started = std::time::Instant::now();
        let mut code = None;
        loop {
            let (exited, cur) = {
                let mut state = super::acp_terminal_state()
                    .lock()
                    .unwrap_or_else(|poisoned| {
                        warn!(
                            "ACP terminal state lock poisoned in handle_terminal_wait_for_exit, recovering"
                        );
                        poisoned.into_inner()
                    });
                match state.get_mut(&terminal_id) {
                    Some(proc) => {
                        if proc.exited {
                            (true, proc.exit_code)
                        } else {
                            match proc.child.try_wait().ok().flatten() {
                                Some(status) => {
                                    proc.exited = true;
                                    let c = status.code();
                                    proc.exit_code = c;
                                    (true, c)
                                }
                                None => (false, None),
                            }
                        }
                    }
                    // Terminal already released — nothing to wait for.
                    None => (true, None),
                }
            };
            if exited {
                code = cur;
                break;
            }
            if started.elapsed() >= timeout {
                warn!(
                    "terminal/wait_for_exit timed out after {}s — re-issue after terminal/kill",
                    crate::orchestration::tool::exec_common::MAX_TIMEOUT_SECS
                );
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        code
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
