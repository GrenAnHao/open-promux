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
    let level_hint = resolve_config_log_level().unwrap_or_else(|| "info".to_string());
    install_tracing(&log_bus, &level_hint);

    // First-run bootstrap: drop a friendly default config.toml on disk so
    // a fresh install opens the desktop UI with sensible values prefilled
    // (host = 127.0.0.1, port = 8080, no upstreams) instead of an empty
    // form. Existing configs are never overwritten.
    let initial_config_path = state::default_config_path();
    match state::ensure_default_config(&initial_config_path) {
        Ok(true) => tracing::info!("wrote default config to {}", initial_config_path.display()),
        Ok(false) => {}
        Err(err) => tracing::warn!(
            "could not write default config to {}: {err}",
            initial_config_path.display()
        ),
    }

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
            commands::get_upstream_health,
            commands::get_logs_snapshot,
            commands::clear_logs,
            commands::open_config_dir,
            commands::open_config_file,
            commands::open_debug_dir,
            commands::get_autostart_enabled,
            commands::set_autostart_enabled,
            commands::get_preferences,
            commands::save_preferences,
            commands::get_traffic_stats,
            commands::clear_traffic_stats,
            commands::fetch_upstream_models,
            commands::chat_probe_upstream,
        ])
        .setup(move |app| {
            log_bridge::spawn(app.handle().clone(), log_bus.clone());
            if let Err(err) = tray::install(app.handle()) {
                tracing::warn!("tray install failed: {err}");
            }
            // DevTools stay available via the `devtools` feature flag in
            // Cargo.toml (right-click → Inspect, or Ctrl+Shift+I) but we no
            // longer pop them open automatically; an always-on inspector
            // window is distracting during normal use.
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
///
/// `level_hint` is the fallback filter directive used when `RUST_LOG` is
/// not set; the desktop UI feeds it from `config.debug.log_level` so the
/// user's chosen verbosity sticks across restarts.
fn install_tracing(bus: &LogBus, level_hint: &str) {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level_hint));

    let registry = tracing_subscriber::registry()
        .with(env_filter)
        .with(tracing_subscriber::fmt::layer().with_target(true))
        .with(bus.layer());

    let _ = registry.try_init();
}

/// Peek the on-disk config (if any) to decide the starting tracing
/// filter. Runs before [`install_tracing`], so a broken `config.toml`
/// must not abort process boot — every failure path quietly degrades to
/// `info`. Only honoured when the user has actually flipped the Debug
/// panel's master switch on.
fn resolve_config_log_level() -> Option<String> {
    let path = state::default_config_path();
    if !path.is_file() {
        return None;
    }
    let config = open_promux::Config::load_path(&path).ok()?;
    if !config.debug.enabled {
        return None;
    }
    Some(config.debug.log_level.as_str().to_string())
}
