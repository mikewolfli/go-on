#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod state;

use std::thread;
use std::time::Duration;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "tray_show", "Show Main", true, None::<&str>)?;
    let mini = MenuItem::with_id(app, "tray_mini", "Mini Console", true, None::<&str>)?;
    let start = MenuItem::with_id(app, "tray_start", "Start Service", true, None::<&str>)?;
    let stop = MenuItem::with_id(app, "tray_stop", "Stop Service", true, None::<&str>)?;
    let restart = MenuItem::with_id(app, "tray_restart", "Restart Service", true, None::<&str>)?;
    let recover = MenuItem::with_id(app, "tray_recover", "Recover Service", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "tray_quit", "Quit", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[&show, &mini, &start, &stop, &restart, &recover, &quit],
    )?;

    TrayIconBuilder::with_id("main-tray")
        .tooltip("go-on GUI")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "tray_show" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            "tray_mini" => {
                if let Some(window) = app.get_webview_window("mini") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            "tray_start" => {
                let _ = commands::process::tray_start(app);
            }
            "tray_stop" => {
                let _ = commands::process::tray_stop(app);
            }
            "tray_restart" => {
                let _ = commands::process::tray_restart(app);
            }
            "tray_recover" => {
                let _ = commands::process::tray_recover(app);
            }
            "tray_quit" => {
                app.exit(0);
            }
            _ => {}
        })
        .build(app)?;

    Ok(())
}

fn main() {
    tauri::Builder::default()
        .manage(state::AppState::default())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .setup(|app| {
            build_tray(&app.handle())?;

            let app_handle = app.handle().clone();
            thread::spawn(move || loop {
                let _ = commands::process::watchdog_tick(&app_handle);
                thread::sleep(Duration::from_secs(1));
            });

            if let Some(main) = app.get_webview_window("main") {
                let app_handle = app.handle().clone();
                main.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        if let Some(window) = app_handle.get_webview_window("main") {
                            let _ = window.hide();
                        }
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::config::configure_service,
            commands::config::reset_default_settings,
            commands::config::set_provider_api_key,
            commands::config::clear_provider_api_key,
            commands::config::fetch_github_copilot_token,
            commands::process::start_service,
            commands::process::stop_service,
            commands::process::restart_service,
            commands::process::service_status,
            commands::process::run_cli_command,
            commands::process::show_mini_console,
            commands::process::hide_mini_console,
            commands::health::check_health,
            commands::integrations::get_editor_integration_status,
            commands::runtime_ops::invoke_runtime_rpc,
            commands::metrics::get_ai_usage_snapshot,
            commands::metrics::get_usage_heatmap,
            commands::metrics::get_endpoint_health_stats,
            commands::logs::read_recent_logs,
        ])
        .run(tauri::generate_context!())
        .expect("error while running go-on GUI");
}
