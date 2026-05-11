//! Tauri 2 desktop wrapper around the embedded `open-promux` library.

mod autostart;
mod commands;
mod log_bridge;
mod preferences;
mod state;
mod tray;

use open_promux::LogBus;
use tauri::{Manager, RunEvent, WindowEvent};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

pub fn run() {
    let log_bus = LogBus::new();
    install_tracing(&log_bus);

    let mut builder = tauri::Builder::default();

    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }));
    }

    builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .manage(state::DesktopState::new(
            log_bus.clone(),
            state::default_config_path(),
        ))
        .invoke_handler(tauri::generate_handler![
            commands::get_runtime_info,
            commands::set_config_path,
            commands::load_config,
            commands::load_config_text,
            commands::save_config,
            commands::save_config_text,
            commands::start_server,
            commands::stop_server,
            commands::get_status,
            commands::get_logs_snapshot,
            commands::clear_logs,
            commands::open_config_dir,
            commands::open_config_file,
            commands::open_debug_dir,
            commands::probe_upstream,
            commands::get_autostart_enabled,
            commands::set_autostart_enabled,
            commands::get_preferences,
            commands::save_preferences,
        ])
        .setup(move |app| {
            log_bridge::spawn(app.handle().clone(), log_bus.clone());
            if let Err(err) = tray::install(app.handle()) {
                tracing::warn!("tray install failed: {err}");
            }
            #[cfg(debug_assertions)]
            if let Some(window) = app.get_webview_window("main") {
                window.open_devtools();
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| match event {
            // Hide on close instead of quitting; users quit via tray menu.
            RunEvent::WindowEvent {
                label,
                event: WindowEvent::CloseRequested { api, .. },
                ..
            } if label == "main" => {
                api.prevent_close();
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.hide();
                }
            }
            RunEvent::ExitRequested { .. } => {
                tracing::info!("open-promux desktop exit requested");
            }
            _ => {}
        });
}

/// Wire up a global tracing subscriber that fans events into both stdout
/// (helpful when launched from a terminal) and the in-process [`LogBus`]
/// consumed by the desktop UI.
fn install_tracing(bus: &LogBus) {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let registry = tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer().with_target(true))
        .with(bus.layer());

    let _ = registry.try_init();
}
