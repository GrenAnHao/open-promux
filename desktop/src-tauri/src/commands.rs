//! Tauri commands exposed to the desktop UI.
//!
//! Naming convention: every command is a thin wrapper that turns the
//! `open_promux` library API or filesystem/registry side-effect into a
//! JSON-friendly result and a `Result<_, String>` shape (Tauri serializes
//! `String` errors directly).

use std::{net::SocketAddr, path::PathBuf, time::Duration};

use open_promux::{
    Config, LogLine, ServerStartError, TrafficSnapshot, UpstreamApiFormat, UpstreamConfig,
    UpstreamHealthSnapshot, serve,
};
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
    write_config_text(&path, &config.to_toml_string().map_err(|e| e.to_string())?)?;
    reload_running_server(&state).await;
    Ok(())
}

#[tauri::command]
pub async fn save_config_text(
    state: State<'_, DesktopState>,
    content: String,
) -> Result<(), String> {
    // Validate that the raw text still parses before persisting it.
    Config::from_toml_str(&content).map_err(|e| format!("invalid TOML: {e}"))?;
    let path = state.config_path.lock().await.clone();
    write_config_text(&path, &content)?;
    reload_running_server(&state).await;
    Ok(())
}

#[tauri::command]
pub async fn start_server(state: State<'_, DesktopState>) -> Result<ServerStatus, String> {
    do_start_server(&state).await
}

/// Shared body of [`start_server`] and [`reload_running_server`]: locks the
/// server slot, reads the on-disk config, and spawns a fresh embedded
/// server. Returns an error if a server is already running, or if any of
/// the config / bind / serve steps fail.
async fn do_start_server(state: &DesktopState) -> Result<ServerStatus, String> {
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
    let host: std::net::IpAddr = config.host.parse().unwrap_or_else(|_| {
        tracing::warn!(
            "invalid host `{}` in config; falling back to 0.0.0.0",
            config.host
        );
        std::net::IpAddr::from([0, 0, 0, 0])
    });
    let addr = SocketAddr::new(host, port);

    let handle = match serve(addr, config).await {
        Ok(handle) => handle,
        Err(ServerStartError::Bind { addr, source }) => {
            return Err(format!("bind {addr} failed: {source}"));
        }
        Err(other) => return Err(other.to_string()),
    };

    let info = handle.info().clone();
    *server_slot = Some(handle);
    tracing::info!("server started on {}", info.local_addr);

    Ok(ServerStatus {
        running: true,
        address: Some(info.local_addr.ip().to_string()),
        port: Some(info.local_addr.port()),
        uptime_seconds: 0,
    })
}

/// Auto-reload helper used after every config save.
///
/// When the embedded server is running, gracefully shut it down and start a
/// fresh one from the just-written config so changes to `upstreams`,
/// `routing`, rate limits, etc. take effect immediately. When no server is
/// running this is a no-op.
///
/// Failures during restart are logged but not propagated: the save itself
/// already succeeded, and the UI's status polling will surface the stopped
/// state to the user.
async fn reload_running_server(state: &DesktopState) {
    // Take the handle out so `do_start_server` does not see a "already
    // running" slot when we re-enter it below.
    let handle = {
        let mut slot = state.server.lock().await;
        slot.take()
    };
    let Some(handle) = handle else { return };
    handle.shutdown().await;

    match do_start_server(state).await {
        Ok(_) => tracing::info!("server reloaded after config save"),
        Err(err) => tracing::warn!("server reload failed after config save: {err}"),
    }
}

#[tauri::command]
pub async fn stop_server(state: State<'_, DesktopState>) -> Result<(), String> {
    let handle = state.server.lock().await.take();
    if let Some(handle) = handle {
        handle.shutdown().await;
        tracing::info!("server stopped");
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
pub async fn get_upstream_health(
    state: State<'_, DesktopState>,
) -> Result<Vec<UpstreamHealthSnapshot>, String> {
    let server = state.server.lock().await;
    let Some(handle) = server.as_ref() else {
        return Ok(Vec::new());
    };
    Ok(handle.state().upstream_health_snapshot().await)
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

/// Result of [`fetch_upstream_models`]: a sorted, de-duplicated list of
/// model ids exposed by the upstream's `/models` endpoint, plus the
/// round-trip latency so the UI can surface "slow" upstreams without an
/// extra probe.
#[derive(Debug, Clone, Serialize)]
pub struct FetchedModels {
    pub models: Vec<String>,
    pub latency_ms: u64,
}

#[tauri::command]
pub async fn fetch_upstream_models(upstream: UpstreamConfig) -> Result<FetchedModels, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("build client failed: {e}"))?;

    let endpoint = format!("{}/models", upstream.url.trim_end_matches('/'));
    let mut request = client.get(&endpoint);
    request = apply_upstream_auth(request, &upstream);
    if matches!(upstream.api_format, UpstreamApiFormat::AnthropicMessages) {
        // Anthropic's /v1/models requires an api-version header. Sending
        // it to OpenAI-style endpoints is harmless (unknown header → ignored).
        request = request.header("anthropic-version", "2023-06-01");
    }

    let started = std::time::Instant::now();
    let resp = request
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    let latency_ms = started.elapsed().as_millis() as u64;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| format!("read body failed: {e}"))?;

    if !status.is_success() {
        tracing::warn!("model fetch failed for {}: {status}", upstream.url);
        return Err(format!(
            "upstream {status}: {}",
            body.chars().take(300).collect::<String>()
        ));
    }

    let parsed: ModelsResponse = serde_json::from_str(&body).map_err(|e| {
        format!(
            "parse /models response failed: {e}; body starts with: {}",
            body.chars().take(120).collect::<String>()
        )
    })?;

    let mut models: Vec<String> = parsed.data.into_iter().map(|m| m.id).collect();
    models.sort();
    models.dedup();
    tracing::info!(
        "fetched {} models from {} in {}ms",
        models.len(),
        upstream.url,
        latency_ms
    );
    Ok(FetchedModels { models, latency_ms })
}

#[derive(serde::Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<ModelItem>,
}

#[derive(serde::Deserialize)]
struct ModelItem {
    id: String,
}

/// Result of [`chat_probe_upstream`]: a real (tiny) chat round-trip
/// against the upstream using the user-selected model. Returns the
/// HTTP status, latency, a short textual preview of the assistant's
/// reply (when parseable), and an error message on failure.
#[derive(Debug, Clone, Serialize)]
pub struct ChatProbeResult {
    pub ok: bool,
    pub status: u16,
    pub latency_ms: u64,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[tauri::command]
pub async fn chat_probe_upstream(
    upstream: UpstreamConfig,
    model: String,
    prompt: Option<String>,
) -> Result<ChatProbeResult, String> {
    if model.trim().is_empty() {
        return Err("model is required for chat probe".into());
    }
    let prompt = prompt
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| "Reply with the single word: pong".to_string());

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| format!("build client failed: {e}"))?;

    let base = upstream.url.trim_end_matches('/');
    let (endpoint, body) = match upstream.api_format {
        UpstreamApiFormat::ChatCompletions => (
            format!("{base}/chat/completions"),
            serde_json::json!({
                "model": &model,
                "messages": [{"role": "user", "content": prompt}],
                "max_tokens": 32,
                "stream": false,
            }),
        ),
        UpstreamApiFormat::AnthropicMessages => (
            format!("{base}/messages"),
            serde_json::json!({
                "model": &model,
                "messages": [{"role": "user", "content": prompt}],
                "max_tokens": 32,
                "stream": false,
            }),
        ),
        UpstreamApiFormat::Responses => (
            format!("{base}/responses"),
            serde_json::json!({
                "model": &model,
                "input": prompt,
                "max_output_tokens": 32,
                "stream": false,
            }),
        ),
    };

    let mut request = client.post(&endpoint).json(&body);
    request = apply_upstream_auth(request, &upstream);
    if matches!(upstream.api_format, UpstreamApiFormat::AnthropicMessages) {
        request = request.header("anthropic-version", "2023-06-01");
    }

    let started = std::time::Instant::now();
    let resp = match request.send().await {
        Ok(resp) => resp,
        Err(err) => {
            return Ok(ChatProbeResult {
                ok: false,
                status: 0,
                latency_ms: started.elapsed().as_millis() as u64,
                model,
                preview: None,
                message: Some(err.to_string()),
            });
        }
    };
    let latency_ms = started.elapsed().as_millis() as u64;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    let preview = extract_chat_preview(&text, upstream.api_format);
    tracing::info!(
        "chat probe completed for {} model={} status={} latency={}ms",
        upstream.url,
        model,
        status,
        latency_ms
    );

    Ok(ChatProbeResult {
        ok: status.is_success(),
        status: status.as_u16(),
        latency_ms,
        model,
        preview,
        message: if status.is_success() {
            None
        } else {
            Some(text.chars().take(400).collect())
        },
    })
}

/// Best-effort extraction of the assistant's reply from each supported
/// upstream format. Returns `None` when the body is not parseable JSON
/// or when no obvious "text" field exists; callers fall back to the
/// raw body excerpt in that case.
fn extract_chat_preview(body: &str, format: UpstreamApiFormat) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let text: String = match format {
        UpstreamApiFormat::ChatCompletions => value
            .get("choices")?
            .as_array()?
            .first()?
            .get("message")?
            .get("content")?
            .as_str()?
            .to_string(),
        UpstreamApiFormat::AnthropicMessages => value
            .get("content")?
            .as_array()?
            .iter()
            .filter_map(|block| block.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join(""),
        UpstreamApiFormat::Responses => {
            if let Some(direct) = value.get("output_text").and_then(|v| v.as_str()) {
                direct.to_string()
            } else {
                value
                    .get("output")?
                    .as_array()?
                    .iter()
                    .filter_map(|item| {
                        item.get("content")?
                            .as_array()?
                            .iter()
                            .filter_map(|c| c.get("text").and_then(|t| t.as_str()))
                            .next()
                            .map(String::from)
                    })
                    .collect::<Vec<_>>()
                    .join("")
            }
        }
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.chars().take(240).collect())
    }
}

/// Attach the upstream's auth header to a `reqwest` builder, mirroring
/// the logic the running proxy uses so probes and real traffic both hit
/// the upstream with byte-identical credentials.
///
/// - Empty `api_key` → no header is added (anonymous endpoints, mocks).
/// - Empty / missing `auth_header` → falls back to `Authorization`.
/// - `Authorization` header without a `Bearer ` prefix → prepends one.
fn apply_upstream_auth(
    request: reqwest::RequestBuilder,
    upstream: &UpstreamConfig,
) -> reqwest::RequestBuilder {
    if upstream.api_key.is_empty() {
        return request;
    }
    let header_name = if upstream.auth_header.is_empty() {
        "Authorization"
    } else {
        upstream.auth_header.as_str()
    };
    let value = if header_name.eq_ignore_ascii_case("Authorization")
        && !upstream.api_key.starts_with("Bearer ")
    {
        format!("Bearer {}", upstream.api_key)
    } else {
        upstream.api_key.clone()
    };
    request.header(header_name, value)
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

#[tauri::command]
pub async fn get_traffic_stats(state: State<'_, DesktopState>) -> Result<TrafficSnapshot, String> {
    let server = state.server.lock().await;
    let Some(handle) = server.as_ref() else {
        // Server not running yet → return an empty snapshot so the UI can
        // still render its table headers without an error toast.
        return Ok(TrafficSnapshot::default());
    };
    Ok(handle.state().traffic_stats().snapshot().await)
}

#[tauri::command]
pub async fn clear_traffic_stats(state: State<'_, DesktopState>) -> Result<(), String> {
    let server = state.server.lock().await;
    let Some(handle) = server.as_ref() else {
        return Ok(());
    };
    handle.state().traffic_stats().clear().await;
    Ok(())
}

fn write_config_text(path: &std::path::Path, content: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create {} failed: {e}", parent.display()))?;
    }
    std::fs::write(path, content).map_err(|e| format!("write {} failed: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::Client;

    fn upstream(api_key: &str, auth_header: &str) -> UpstreamConfig {
        UpstreamConfig {
            name: None,
            url: "http://example.test/v1".to_string(),
            api_key: api_key.to_string(),
            auth_header: auth_header.to_string(),
            proxy: None,
            proxy_type: Default::default(),
            api_format: UpstreamApiFormat::ChatCompletions,
            max_concurrent_requests: None,
            rpm: None,
            tpm: None,
        }
    }

    fn header_value(req: &reqwest::Request, name: &str) -> Option<String> {
        req.headers()
            .get(name)
            .map(|v| v.to_str().unwrap().to_string())
    }

    #[test]
    fn apply_upstream_auth_omits_header_when_api_key_is_empty() {
        let client = Client::new();
        let req = apply_upstream_auth(client.get("http://example.test"), &upstream("", ""))
            .build()
            .unwrap();
        assert!(header_value(&req, "authorization").is_none());
    }

    #[test]
    fn apply_upstream_auth_uses_authorization_with_bearer_prefix() {
        let client = Client::new();
        let req = apply_upstream_auth(client.get("http://example.test"), &upstream("sk-1", ""))
            .build()
            .unwrap();
        assert_eq!(
            header_value(&req, "authorization").as_deref(),
            Some("Bearer sk-1")
        );
    }

    #[test]
    fn apply_upstream_auth_preserves_existing_bearer_prefix() {
        let client = Client::new();
        let req = apply_upstream_auth(
            client.get("http://example.test"),
            &upstream("Bearer sk-1", ""),
        )
        .build()
        .unwrap();
        assert_eq!(
            header_value(&req, "authorization").as_deref(),
            Some("Bearer sk-1")
        );
    }

    #[test]
    fn apply_upstream_auth_uses_custom_header_without_bearer_prefix() {
        let client = Client::new();
        let req = apply_upstream_auth(
            client.get("http://example.test"),
            &upstream("rawkey", "x-api-key"),
        )
        .build()
        .unwrap();
        assert_eq!(header_value(&req, "x-api-key").as_deref(), Some("rawkey"));
        // Authorization must not be set in this branch.
        assert!(header_value(&req, "authorization").is_none());
    }

    #[test]
    fn extract_chat_preview_reads_chat_completions_message() {
        let body = serde_json::json!({
            "choices": [{ "message": { "content": "pong" } }]
        })
        .to_string();
        assert_eq!(
            extract_chat_preview(&body, UpstreamApiFormat::ChatCompletions),
            Some("pong".to_string())
        );
    }

    #[test]
    fn extract_chat_preview_concatenates_anthropic_text_blocks() {
        let body = serde_json::json!({
            "content": [
                { "type": "text", "text": "po" },
                { "type": "text", "text": "ng" }
            ]
        })
        .to_string();
        assert_eq!(
            extract_chat_preview(&body, UpstreamApiFormat::AnthropicMessages),
            Some("pong".to_string())
        );
    }

    #[test]
    fn extract_chat_preview_prefers_responses_output_text() {
        let body = serde_json::json!({ "output_text": "pong" }).to_string();
        assert_eq!(
            extract_chat_preview(&body, UpstreamApiFormat::Responses),
            Some("pong".to_string())
        );
    }

    #[test]
    fn extract_chat_preview_falls_back_to_responses_output_content() {
        let body = serde_json::json!({
            "output": [{ "content": [{ "type": "output_text", "text": "pong" }] }]
        })
        .to_string();
        assert_eq!(
            extract_chat_preview(&body, UpstreamApiFormat::Responses),
            Some("pong".to_string())
        );
    }

    #[test]
    fn extract_chat_preview_returns_none_for_unparseable_body() {
        assert_eq!(
            extract_chat_preview("not-json", UpstreamApiFormat::ChatCompletions),
            None
        );
    }

    #[test]
    fn extract_chat_preview_returns_none_when_text_is_whitespace_only() {
        let body = serde_json::json!({
            "choices": [{ "message": { "content": "   \n  " } }]
        })
        .to_string();
        assert_eq!(
            extract_chat_preview(&body, UpstreamApiFormat::ChatCompletions),
            None
        );
    }
}
