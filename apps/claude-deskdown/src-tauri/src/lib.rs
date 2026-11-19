//! ClaudeDeskDown: use original Claude Desktop in full-feature mode, directly connecting to your configured upstream (Base URL + Key).

mod commands;
mod host;
mod log;
mod upstreams;

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, WindowEvent};

const MAIN_WINDOW: &str = "main";
const TRAY_SHOW: &str = "tray_show";
const TRAY_QUIT: &str = "tray_quit";
/// Single-instance lock port (staggered from Codex++'s 57319 / 57320). Set `CLAUDE_DESKDOWN_GUARD_PORT` to override.
const GUARD_PORT: u16 = 57519;

static EXITING: AtomicBool = AtomicBool::new(false);

pub fn run() {
    host::DeskDownHost::install();
    log::append("app.start", serde_json::json!({ "version": env!("CARGO_PKG_VERSION") }));
    let guard_port = std::env::var("CLAUDE_DESKDOWN_GUARD_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(GUARD_PORT);
    // Another instance already running: let system bring it to front (on macOS, open same app), then exit.
    let Ok(_guard) = std::net::TcpListener::bind(("127.0.0.1", guard_port)) else {
        log::append("app.already_running", serde_json::json!({ "port": guard_port }));
        return;
    };
    // After app restart, Claude might still be on old proxy port: resume local service on original port so its retries connect.
    std::thread::spawn(|| match claude_local::resume_attached_proxy() {
        Ok(Some(port)) => log::append("claude_local.resumed", serde_json::json!({ "port": port })),
        Ok(None) => {}
        Err(error) => log::append("claude_local.resume_failed", serde_json::json!({ "message": format!("{error:#}") })),
    });

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            install_tray(app)?;
            if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
                let hide_target = window.clone();
                window.on_window_event(move |event| {
                    // Close window = minimize only: local service must stay running for Claude to connect. Real quit via tray menu.
                    if let WindowEvent::CloseRequested { api, .. } = event
                        && !EXITING.load(Ordering::SeqCst)
                    {
                        api.prevent_close();
                        let _ = hide_target.hide();
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::claude_local_status,
            commands::claude_local_configure,
            commands::claude_local_action,
            commands::deskdown_load,
            commands::deskdown_save_profile,
            commands::deskdown_delete_profile,
            commands::deskdown_set_active,
            commands::deskdown_import,
            commands::deskdown_fetch_models,
            commands::deskdown_set_voice,
            commands::dictation_local_model_status,
            commands::download_dictation_local_model,
        ])
        .build(tauri::generate_context!());
    let app = match app {
        Ok(app) => app,
        Err(error) => {
            log::append("app.build_failed", serde_json::json!({ "message": error.to_string() }));
            return;
        }
    };
    app.run(|app_handle, event| match event {
        tauri::RunEvent::ExitRequested { api, code, .. } => {
            // Tauri also requests exit when all windows hidden: don't exit unless user clicked "Quit".
            if code.is_none() && !EXITING.load(Ordering::SeqCst) {
                api.prevent_exit();
            }
        }
        tauri::RunEvent::Exit => {
            claude_local::shutdown_on_exit();
            claude_local::dictation::server::remove_receipt();
            log::append("app.exit", serde_json::json!({}));
        }
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Reopen { .. } => show_main_window(app_handle),
        _ => {
            let _ = app_handle;
        }
    });
}

fn show_main_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn install_tray<R: tauri::Runtime>(app: &tauri::App<R>) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, TRAY_SHOW, "Show ClaudeDeskDown", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, TRAY_QUIT, "Quit (also stops local service)", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;
    let mut builder = TrayIconBuilder::with_id("claude_deskdown_tray")
        .menu(&menu)
        .tooltip("ClaudeDeskDown")
        .show_menu_on_left_click(!cfg!(windows))
        .on_menu_event(|app, event| match event.id.as_ref() {
            TRAY_SHOW => show_main_window(app),
            TRAY_QUIT => {
                EXITING.store(true, Ordering::SeqCst);
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event
                && cfg!(windows)
            {
                show_main_window(tray.app_handle());
            }
        });
    // macOS menu bar uses monochrome template icon (system colors by theme); other platforms use color app icon.
    #[cfg(target_os = "macos")]
    {
        builder = builder.icon(tauri::include_image!("icons/tray@2x.png")).icon_as_template(true);
    }
    #[cfg(not(target_os = "macos"))]
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }
    builder.build(app)?;
    Ok(())
}