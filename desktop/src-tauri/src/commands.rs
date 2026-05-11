//! Tauri commands exposed to the desktop UI.
//!
//! Naming convention: every command is a thin wrapper that turns the
//! `open_promux` library API or filesystem/registry side-effect into a
//! JSON-friendly result and a `Result<_, String>` shape (Tauri serializes
//! `String` errors directly).

use std::{net::SocketAddr, path::PathBuf, time::Duration};

use open_promux::{Config, LogLine, ServerStartError, serve};
use serde::Serialize;
use tauri::{AppHandle, State};

use crate::{autostart, preferences, state::DesktopState};

/// Static metadata about the desktop runtime, surfaced to the UI on boot.
#[derive(Debug, Clone, Serialize)]
pub struct RuntimeInfo {
    pub version: &'static str,
    pub config_path: String,
    pub config_exists: bool,
    pub platform: &'static str,
}

/// Snapshot of the embedded server status.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ServerStatus {
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Seconds since the server started; `0` when not running.
    pub uptime_seconds: u64,
}

/// Result of pinging an upstream `/models` endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct UpstreamProbeResult {
    pub ok: bool,
    pub status: u16,
    pub latency_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[tauri::command]
pub async fn get_runtime_info(state: State<'_, DesktopState>) -> Result<RuntimeInfo, String> {
    let config_path = state.config_path.lock().await.clone();
    Ok(RuntimeInfo {
        version: env!("CARGO_PKG_VERSION"),
        config_exists: config_path.is_file(),
        config_path: config_path.display().to_string(),
        platform: std::env::consts::OS,
    })
}

#[tauri::command]
pub async fn set_config_path(
    state: State<'_, DesktopState>,
    path: String,
) -> Result<RuntimeInfo, String> {
    let new_path = PathBuf::from(path);
    {
        let mut current = state.config_path.lock().await;
        *current = new_path;
    }
    get_runtime_info(state).await
}

#[tauri::command]
pub async fn load_config(state: State<'_, DesktopState>) -> Result<Config, String> {
    let path = state.config_path.lock().await.clone();
    if !path.is_file() {
        // Return an empty config so the UI can render the form for the
        // first-time setup without crashing.
        return Config::from_toml_str("").map_err(|e| e.to_string());
    }
    Config::load_path(&path)
}

#[tauri::command]
pub async fn load_config_text(state: State<'_, DesktopState>) -> Result<String, String> {
    let path = state.config_path.lock().await.clone();
    if !path.is_file() {
        return Ok(String::new());
    }
    std::fs::read_to_string(&path).map_err(|e| format!("read {} failed: {e}", path.display()))
}

#[tauri::command]
pub async fn save_config(state: State<'_, DesktopState>, config: Config) -> Result<(), String> {
    let path = state.config_path.lock().await.clone();
    write_config_text(&path, &config.to_toml_string().map_err(|e| e.to_string())?)
}

#[tauri::command]
pub async fn save_config_text(
    state: State<'_, DesktopState>,
    content: String,
) -> Result<(), String> {
    // Validate that the raw text still parses before persisting it.
    Config::from_toml_str(&content).map_err(|e| format!("invalid TOML: {e}"))?;
    let path = state.config_path.lock().await.clone();
    write_config_text(&path, &content)
}

#[tauri::command]
pub async fn start_server(state: State<'_, DesktopState>) -> Result<ServerStatus, String> {
    let mut server_slot = state.server.lock().await;
    if server_slot.is_some() {
        return Err("server already running".into());
    }

    let path = state.config_path.lock().await.clone();
    let config = if path.is_file() {
        Config::load_path(&path)?
    } else {
        return Err(format!(
            "config file does not exist: {}. save it first before starting.",
            path.display()
        ));
    };
    let port = config.port;
    let addr = SocketAddr::from(([0, 0, 0, 0], port));

    let handle = match serve(addr, config).await {
        Ok(handle) => handle,
        Err(ServerStartError::Bind { addr, source }) => {
            return Err(format!("bind {addr} failed: {source}"));
        }
        Err(other) => return Err(other.to_string()),
    };

    let info = handle.info().clone();
    *server_slot = Some(handle);

    Ok(ServerStatus {
        running: true,
        address: Some(info.local_addr.ip().to_string()),
        port: Some(info.local_addr.port()),
        uptime_seconds: 0,
    })
}

#[tauri::command]
pub async fn stop_server(state: State<'_, DesktopState>) -> Result<(), String> {
    let handle = state.server.lock().await.take();
    if let Some(handle) = handle {
        handle.shutdown().await;
    }
    Ok(())
}

#[tauri::command]
pub async fn get_status(state: State<'_, DesktopState>) -> Result<ServerStatus, String> {
    let server = state.server.lock().await;
    let Some(handle) = server.as_ref() else {
        return Ok(ServerStatus::default());
    };
    let info = handle.info();
    let uptime_seconds = info.started_at.elapsed().as_secs();
    Ok(ServerStatus {
        running: true,
        address: Some(info.local_addr.ip().to_string()),
        port: Some(info.local_addr.port()),
        uptime_seconds,
    })
}

#[tauri::command]
pub async fn get_logs_snapshot(state: State<'_, DesktopState>) -> Result<Vec<LogLine>, String> {
    Ok(state.log_bus.snapshot())
}

#[tauri::command]
pub async fn clear_logs(state: State<'_, DesktopState>) -> Result<(), String> {
    state.log_bus.clear();
    Ok(())
}

#[tauri::command]
pub async fn open_config_dir(app: AppHandle, state: State<'_, DesktopState>) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;

    let path = state.config_path.lock().await.clone();
    let target = path.parent().map(PathBuf::from).unwrap_or(path);
    if !target.exists() {
        std::fs::create_dir_all(&target)
            .map_err(|e| format!("create {} failed: {e}", target.display()))?;
    }
    app.opener()
        .open_path(target.to_string_lossy(), None::<&str>)
        .map_err(|e| format!("open path failed: {e}"))
}

#[tauri::command]
pub async fn open_config_file(
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;

    let path = state.config_path.lock().await.clone();
    if !path.is_file() {
        return Err(format!("config file does not exist: {}", path.display()));
    }
    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| format!("open file failed: {e}"))
}

#[tauri::command]
pub async fn open_debug_dir(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;

    let dir = std::env::current_dir()
        .map_err(|e| format!("current_dir failed: {e}"))?
        .join("debug");
    if !dir.exists() {
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("create {} failed: {e}", dir.display()))?;
    }
    app.opener()
        .open_path(dir.to_string_lossy(), None::<&str>)
        .map_err(|e| format!("open debug dir failed: {e}"))
}

#[tauri::command]
pub async fn probe_upstream(
    url: String,
    api_key: Option<String>,
    auth_header: Option<String>,
) -> Result<UpstreamProbeResult, String> {
    let endpoint = format!("{}/models", url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("build client failed: {e}"))?;

    let mut request = client.get(&endpoint);
    if let Some(key) = api_key.as_deref().filter(|k| !k.is_empty()) {
        let header_name = auth_header
            .as_deref()
            .filter(|h| !h.is_empty())
            .unwrap_or("Authorization");
        let header_value =
            if header_name.eq_ignore_ascii_case("Authorization") && !key.starts_with("Bearer ") {
                format!("Bearer {key}")
            } else {
                key.to_string()
            };
        request = request.header(header_name, header_value);
    }

    let started = std::time::Instant::now();
    match request.send().await {
        Ok(resp) => {
            let status = resp.status();
            let latency_ms = started.elapsed().as_millis() as u64;
            Ok(UpstreamProbeResult {
                ok: status.is_success(),
                status: status.as_u16(),
                latency_ms,
                message: if status.is_success() {
                    None
                } else {
                    Some(status.canonical_reason().unwrap_or("unknown").into())
                },
            })
        }
        Err(err) => {
            let latency_ms = started.elapsed().as_millis() as u64;
            Ok(UpstreamProbeResult {
                ok: false,
                status: 0,
                latency_ms,
                message: Some(err.to_string()),
            })
        }
    }
}

#[tauri::command]
pub async fn get_autostart_enabled() -> Result<bool, String> {
    autostart::is_enabled()
}

#[tauri::command]
pub async fn set_autostart_enabled(enabled: bool) -> Result<(), String> {
    autostart::set_enabled(enabled)
}

#[tauri::command]
pub async fn get_preferences(
    state: State<'_, DesktopState>,
) -> Result<preferences::DesktopPreferences, String> {
    let config_path = state.config_path.lock().await.clone();
    let path = preferences::preferences_path(&config_path);
    Ok(preferences::load(&path))
}

#[tauri::command]
pub async fn save_preferences(
    state: State<'_, DesktopState>,
    preferences: preferences::DesktopPreferences,
) -> Result<(), String> {
    let config_path = state.config_path.lock().await.clone();
    let path = preferences::preferences_path(&config_path);
    preferences::save(&path, &preferences)
}

fn write_config_text(path: &std::path::Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create {} failed: {e}", parent.display()))?;
    }
    std::fs::write(path, content).map_err(|e| format!("write {} failed: {e}", path.display()))
}
