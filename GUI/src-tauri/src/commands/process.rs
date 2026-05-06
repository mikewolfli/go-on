use std::fs::OpenOptions;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::SystemTime;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

use anyhow::{anyhow, Result};
use chrono::{DateTime, Local};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::state::{AppState, ManagedProcess, ServiceStatus};

fn format_time(t: SystemTime) -> String {
    let datetime: DateTime<Local> = t.into();
    datetime.to_rfc3339()
}

fn current_status(inner: &mut crate::state::InnerState) -> ServiceStatus {
    if let Some(proc) = inner.process.as_mut() {
        match proc.child.try_wait() {
            Ok(Some(exit_status)) => {
                if inner.stop_requested {
                    inner.crash_message = None;
                } else {
                    let code = exit_status
                        .code()
                        .map(|x| x.to_string())
                        .unwrap_or_else(|| "signal".to_string());
                    inner.crash_message =
                        Some(format!("service exited unexpectedly (code={code})"));
                    inner.crash_notified = false;
                }
                inner.stop_requested = false;
                inner.process = None;
            }
            Ok(None) => {}
            Err(_) => {
                if !inner.stop_requested {
                    inner.crash_message =
                        Some("service state check failed unexpectedly".to_string());
                    inner.crash_notified = false;
                }
                inner.stop_requested = false;
                inner.process = None;
            }
        }
    }

    if let Some(proc) = inner.process.as_ref() {
        let uptime = SystemTime::now()
            .duration_since(proc.started_at)
            .map(|d| d.as_secs())
            .ok();
        return ServiceStatus {
            running: true,
            pid: Some(proc.pid),
            executable_path: Some(inner.config.executable_path.clone()),
            working_dir: Some(inner.config.working_dir.clone()),
            uptime_seconds: uptime,
            started_at: Some(format_time(proc.started_at)),
            last_error: None,
        };
    }

    ServiceStatus {
        running: false,
        pid: None,
        executable_path: Some(inner.config.executable_path.clone()),
        working_dir: Some(inner.config.working_dir.clone()),
        uptime_seconds: None,
        started_at: None,
        last_error: inner.crash_message.clone(),
    }
}

fn start_service_impl(state: &AppState) -> Result<ServiceStatus> {
    let mut inner = state.0.lock().map_err(|_| anyhow!("state lock poisoned"))?;

    if current_status(&mut inner).running {
        return Ok(current_status(&mut inner));
    }

    let exe_path = {
        let path = PathBuf::from(&inner.config.executable_path);
        if path.is_absolute() {
            path
        } else {
            PathBuf::from(&inner.config.working_dir).join(path)
        }
    };

    if !exe_path.exists() {
        return Err(anyhow!(
            "startup_error:file_missing:{}",
            exe_path.to_string_lossy()
        ));
    }
    if !exe_path.is_file() {
        return Err(anyhow!(
            "startup_error:not_a_file:{}",
            exe_path.to_string_lossy()
        ));
    }

    let stdout_log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&inner.config.log_path)?;
    let stderr_log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&inner.config.log_path)?;

    let mut cmd = Command::new(&exe_path);
    #[cfg(target_os = "windows")]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd.current_dir(&inner.config.working_dir)
        .stdout(Stdio::from(stdout_log))
        .stderr(Stdio::from(stderr_log));

    // Default to acp_http so the backend exposes a health endpoint on 127.0.0.1:8090
    // that the GUI can connect to. If the config specifies a protocol mode, use that instead.
    let protocol_mode = inner
        .config
        .protocol_mode
        .as_deref()
        .filter(|m| !m.is_empty())
        .unwrap_or("acp_http");
    cmd.arg("--protocol-mode").arg(protocol_mode);

    for (k, v) in &inner.config.extra_env {
        cmd.env(k, v);
    }

    let mut child = cmd.spawn().map_err(|e| {
        let reason = match e.kind() {
            std::io::ErrorKind::PermissionDenied => "permission_denied",
            std::io::ErrorKind::NotFound => "file_missing",
            _ => "spawn_failed",
        };
        anyhow!("startup_error:{reason}:{}", e)
    })?;

    for _ in 0..4 {
        thread::sleep(std::time::Duration::from_millis(300));
        if let Ok(Some(status)) = child.try_wait() {
            let code = status
                .code()
                .map(|x| x.to_string())
                .unwrap_or_else(|| "signal".to_string());
            return Err(anyhow!("startup_error:exited_early:{code}"));
        }
    }

    // Final check: try_wait once more to catch exit between loop and process store
    if let Ok(Some(status)) = child.try_wait() {
        let code = status
            .code()
            .map(|x| x.to_string())
            .unwrap_or_else(|| "signal".to_string());
        return Err(anyhow!("startup_error:exited_early:{code}"));
    }

    let pid = child.id();

    inner.process = Some(ManagedProcess {
        child,
        pid,
        started_at: SystemTime::now(),
    });
    inner.stop_requested = false;
    inner.crash_message = None;
    inner.crash_notified = false;

    Ok(current_status(&mut inner))
}

fn stop_service_impl(state: &AppState) -> Result<ServiceStatus> {
    let mut inner = state.0.lock().map_err(|_| anyhow!("state lock poisoned"))?;
    inner.stop_requested = true;

    // Take the process out so child.wait() can run in another thread with timeout,
    // preventing the Mutex from being blocked indefinitely.
    if let Some(mut proc) = inner.process.take() {
        let _ = proc.child.kill();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = proc.child.wait();
            let _ = tx.send(());
        });
        let _ = rx.recv_timeout(std::time::Duration::from_secs(3));
    }
    inner.stop_requested = false;

    Ok(current_status(&mut inner))
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct CrashEventPayload {
    message: String,
    timestamp: String,
}

fn set_tray_hint(app: &AppHandle, hint: Option<&str>) {
    if let Some(tray) = app.tray_by_id("main-tray") {
        let _ = tray.set_tooltip(hint);
    }
}

pub fn watchdog_tick(app: &AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    let crash_message = {
        let mut inner = state.0.lock().map_err(|_| anyhow!("state lock poisoned"))?;
        let status = current_status(&mut inner);
        if !status.running {
            if let Some(message) = inner.crash_message.clone() {
                if !inner.crash_notified {
                    inner.crash_notified = true;
                    Some(CrashEventPayload {
                        message,
                        timestamp: Local::now().to_rfc3339(),
                    })
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            set_tray_hint(app, Some("go-on running"));
            None
        }
    };
    // emit 时已释放锁
    if let Some(payload) = crash_message {
        let _ = app.emit("service-crash", payload);
        set_tray_hint(app, Some("go-on crashed: use Recover Service"));
    }
    Ok(())
}

fn restart_service_impl(state: &AppState) -> Result<ServiceStatus> {
    {
        let mut inner = state.0.lock().map_err(|_| anyhow!("state lock poisoned"))?;
        if let Some(last) = inner.last_restart_at {
            if last.elapsed().as_secs_f64() < 2.0 {
                return Err(anyhow!("restart debounce active"));
            }
        }
        inner.last_restart_at = Some(std::time::Instant::now());
    }

    let _ = stop_service_impl(state)?;
    start_service_impl(state)
}

#[tauri::command]
pub fn start_service(state: State<'_, AppState>) -> Result<ServiceStatus, String> {
    start_service_impl(&state).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn stop_service(state: State<'_, AppState>) -> Result<ServiceStatus, String> {
    stop_service_impl(&state).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn restart_service(state: State<'_, AppState>) -> Result<ServiceStatus, String> {
    restart_service_impl(&state).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn service_status(state: State<'_, AppState>) -> Result<ServiceStatus, String> {
    let mut inner = state
        .0
        .lock()
        .map_err(|_| "state lock poisoned".to_string())?;
    Ok(current_status(&mut inner))
}

#[tauri::command]
pub fn run_cli_command(state: State<'_, AppState>, command: String) -> Result<String, String> {
    let (executable_path, working_dir) = {
        let inner = state
            .0
            .lock()
            .map_err(|_| "state lock poisoned".to_string())?;
        (
            inner.config.executable_path.clone(),
            inner.config.working_dir.clone(),
        )
    }; // 锁在此释放

    let args: Vec<&str> = command.split_whitespace().collect();
    let output = Command::new(&executable_path)
        .current_dir(&working_dir)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;

    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    if !output.stderr.is_empty() {
        text.push_str("\n[stderr]\n");
        text.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    Ok(text)
}

#[tauri::command]
pub fn show_mini_console(app: AppHandle) -> Result<(), String> {
    let mini = app
        .get_webview_window("mini")
        .ok_or_else(|| "mini window not found".to_string())?;
    mini.show().map_err(|e| e.to_string())?;
    mini.set_focus().map_err(|e| e.to_string())?;
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.hide();
    }
    Ok(())
}

#[tauri::command]
pub fn hide_mini_console(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("mini")
        .ok_or_else(|| "mini window not found".to_string())?;
    window.hide().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn switch_to_main_window(app: AppHandle) -> Result<(), String> {
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| "main window not found".to_string())?;
    main.show().map_err(|e| e.to_string())?;
    main.set_focus().map_err(|e| e.to_string())?;
    if let Some(mini) = app.get_webview_window("mini") {
        let _ = mini.hide();
    }
    Ok(())
}

#[tauri::command]
pub fn switch_to_mini_window(app: AppHandle) -> Result<(), String> {
    let mini = app
        .get_webview_window("mini")
        .ok_or_else(|| "mini window not found".to_string())?;
    mini.show().map_err(|e| e.to_string())?;
    mini.set_focus().map_err(|e| e.to_string())?;
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.hide();
    }
    Ok(())
}

pub fn tray_start(app: &AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    let _ = start_service_impl(&state)?;
    set_tray_hint(app, Some("go-on running"));
    Ok(())
}

pub fn tray_stop(app: &AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    let _ = stop_service_impl(&state)?;
    set_tray_hint(app, Some("go-on stopped"));
    Ok(())
}

pub fn tray_restart(app: &AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    let _ = restart_service_impl(&state)?;
    set_tray_hint(app, Some("go-on running"));
    Ok(())
}

pub fn tray_recover(app: &AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    let _ = restart_service_impl(&state)?;
    set_tray_hint(app, Some("go-on running"));
    Ok(())
}
