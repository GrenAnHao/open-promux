use super::*;

pub(super) fn spawn_health_checks(
    upstreams: Vec<Arc<UpstreamState>>,
    interval_millis: u64,
    unhealthy_after_failures: u64,
) {
    tokio::spawn(async move {
        let interval = Duration::from_millis(interval_millis.max(1));
        loop {
            futures::future::join_all(upstreams.iter().map(|upstream| async move {
                let upstream_url = upstream.config.url.trim_end_matches('/');
                let target = format!("{upstream_url}/models");
                let (healthy, detail) =
                    match apply_model_list_headers(upstream.client.get(&target), &upstream.config)
                        .send()
                        .await
                    {
                        Ok(resp) => {
                            let status = resp.status();
                            (status.is_success(), format!("status={status}"))
                        }
                        Err(err) => (false, format!("error={err}")),
                    };
                let update = upstream
                    .set_health_check_result(healthy, unhealthy_after_failures)
                    .await;
                log_health_check_result(upstream, &target, update, &detail);
            }))
            .await;
            tokio::time::sleep(interval).await;
        }
    });
}

fn log_health_check_result(
    upstream: &UpstreamState,
    target: &str,
    update: UpstreamHealthUpdate,
    detail: &str,
) {
    let upstream_name = upstream_log_name(&upstream.config);
    if update.current_healthy {
        if update.first_check || !update.previous_healthy {
            tracing::info!(
                "[health] upstream {upstream_name} healthy target={target} failures={} {detail}",
                update.failures
            );
        }
        return;
    }

    if update.previous_healthy {
        tracing::warn!(
            "[health] upstream {upstream_name} marked unhealthy target={target} failures={} {detail}",
            update.failures
        );
    } else {
        tracing::warn!(
            "[health] upstream {upstream_name} still unhealthy target={target} failures={} {detail}",
            update.failures
        );
    }
}

pub(super) fn spawn_model_cache_warmup(upstreams: Vec<Arc<UpstreamState>>) {
    tokio::spawn(async move {
        futures::future::join_all(upstreams.into_iter().map(|upstream| async move {
            if let Err(status) = fetch_model_items_cached(&upstream, "[startup]").await {
                tracing::warn!(
                    "[startup] failed to prefetch upstream {} models: {status}",
                    upstream.config.url
                );
            }
        }))
        .await;
    });
}

pub(super) fn upstream_client(upstream: &UpstreamConfig) -> Client {
    let mut builder = Client::builder().no_proxy();

    if let Some(proxy_config) = upstream.proxy.as_ref() {
        let proxy_url = format!(
            "{}://{}:{}",
            upstream.proxy_type.scheme(),
            proxy_config.host,
            proxy_config.port
        );
        let proxy = Proxy::all(&proxy_url)
            .unwrap_or_else(|e| panic!("failed to configure upstream proxy {proxy_url}: {e}"));
        let proxy = match (
            proxy_config.username.as_deref(),
            proxy_config.password.as_deref(),
        ) {
            (Some(username), Some(password)) => proxy.basic_auth(username, password),
            _ => proxy,
        };
        builder = builder.proxy(proxy);
    }

    builder
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(32)
        .tcp_keepalive(Duration::from_secs(60))
        .tcp_nodelay(true)
        .build()
        .expect("failed to build upstream HTTP client")
}

pub(super) fn apply_upstream_auth(
    builder: RequestBuilder,
    upstream: &UpstreamConfig,
) -> RequestBuilder {
    if upstream.api_key.is_empty() {
        builder
    } else if upstream.auth_header.eq_ignore_ascii_case("authorization")
        && !upstream.api_key.to_ascii_lowercase().starts_with("bearer ")
    {
        builder.header(
            &upstream.auth_header,
            format!("Bearer {}", upstream.api_key),
        )
    } else {
        builder.header(&upstream.auth_header, &upstream.api_key)
    }
}

pub(super) fn apply_anthropic_headers(builder: RequestBuilder) -> RequestBuilder {
    builder.header("anthropic-version", "2023-06-01")
}

pub(super) fn apply_model_list_headers(
    builder: RequestBuilder,
    config: &UpstreamConfig,
) -> RequestBuilder {
    let builder = apply_upstream_auth(builder, config);
    if matches!(config.api_format, UpstreamApiFormat::AnthropicMessages) {
        apply_anthropic_headers(builder)
    } else {
        builder
    }
}

pub(super) fn is_proxy_authorized(config: &Config, headers: &HeaderMap) -> bool {
    let Some(auth_key) = config.auth_key.as_deref().filter(|key| !key.is_empty()) else {
        return true;
    };
    let expected = format!("Bearer {auth_key}");
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == expected)
}

pub(super) fn unauthorized_response() -> Response {
    (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
}

pub(super) fn estimate_tokens_from_bytes(bytes: &[u8]) -> u64 {
    (bytes.len() as u64).div_ceil(4).max(1)
}

/// Best-effort name for traffic-stats bucketing. Falls back to the upstream
/// URL when no `name = "..."` was set in config.
pub(super) fn stats_upstream_name(config: &UpstreamConfig) -> String {
    config.name.clone().unwrap_or_else(|| config.url.clone())
}

/// Bucket the request into [`TrafficStats`] using the per-upstream name and
/// the requested model (falling back to `<unknown>` when the caller's body
/// has no `model` field). Cheap: only the first call per new bucket takes
/// the write lock; subsequent calls follow the atomic-add fast path.
pub(super) async fn record_request_metric(
    state: &Arc<AppState>,
    upstream_config: &UpstreamConfig,
    model: Option<&str>,
    ok: bool,
    bytes_in: u64,
    bytes_out: u64,
    latency_ms: u64,
) {
    state
        .traffic_stats()
        .record(
            &stats_upstream_name(upstream_config),
            model.unwrap_or("<unknown>"),
            ok,
            bytes_in,
            bytes_out,
            latency_ms,
        )
        .await;
}

pub(super) fn dump_upstream_error_debug(
    label: &str,
    status: StatusCode,
    upstream: &UpstreamConfig,
    target: &str,
    original_request: &serde_json::Value,
    upstream_request: &[u8],
    upstream_error: &[u8],
) {
    let debug_dir = std::env::var_os("OPEN_PROMUX_DEBUG_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("debug"));
    if let Err(e) = std::fs::create_dir_all(&debug_dir) {
        tracing::warn!("{label} failed to create debug directory: {e}");
        return;
    }

    let upstream_request = serde_json::from_slice(upstream_request).unwrap_or_else(|_| {
        serde_json::Value::String(String::from_utf8_lossy(upstream_request).into_owned())
    });
    let upstream_error = serde_json::from_slice(upstream_error).unwrap_or_else(|_| {
        serde_json::Value::String(String::from_utf8_lossy(upstream_error).into_owned())
    });
    let dump = serde_json::json!({
        "label": label,
        "status": status.as_u16(),
        "upstream": {
            "name": upstream.name,
            "url": upstream.url,
            "api_format": format!("{:?}", upstream.api_format),
            "target": target,
        },
        "original_request": original_request,
        "upstream_request": upstream_request,
        "upstream_error": upstream_error,
    });
    let file_name = format!(
        "upstream-error-{}-{}.json",
        label
            .trim_matches(|ch| ch == '[' || ch == ']')
            .replace(['/', ' '], "_"),
        uuid::Uuid::new_v4()
    );
    let path = debug_dir.join(file_name);
    match serde_json::to_vec_pretty(&dump)
        .map_err(std::io::Error::other)
        .and_then(|bytes| std::fs::write(&path, bytes))
    {
        Ok(()) => tracing::warn!(
            "{label} wrote upstream error debug dump: {}",
            path.display()
        ),
        Err(e) => tracing::warn!("{label} failed to write upstream error debug dump: {e}"),
    }
}

/// Persist a single client-side request body to `./debug/` (or the path
/// overridden by `OPEN_PROMUX_DEBUG_DIR`) when the user has opted into
/// conversation logging from the desktop **Debug** panel.
///
/// Only called from the `/v1/chat/completions`, `/v1/responses`, and
/// `/v1/messages` handlers, and only when `config.debug.enabled &&
/// config.debug.log_conversations` is true — the rest of the gateway is
/// untouched. Upstream responses are already captured by
/// [`dump_upstream_error_debug`] whenever an upstream returns a non-2xx.
pub(super) fn dump_conversation_debug(label: &str, request_body: &[u8]) {
    let debug_dir = std::env::var_os("OPEN_PROMUX_DEBUG_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("debug"));
    if let Err(e) = std::fs::create_dir_all(&debug_dir) {
        tracing::warn!("{label} conversation debug dump: failed to create dir: {e}");
        return;
    }

    let body_value =
        serde_json::from_slice::<serde_json::Value>(request_body).unwrap_or_else(|_| {
            serde_json::Value::String(String::from_utf8_lossy(request_body).into_owned())
        });
    let dump = serde_json::json!({
        "label": label,
        "timestamp_ms": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or_default(),
        "request": body_value,
    });
    let sanitized_label = label
        .trim_matches(|ch| ch == '[' || ch == ']')
        .replace(['/', ' '], "_");
    let file_name = format!(
        "conversation-{}-{}.json",
        sanitized_label,
        uuid::Uuid::new_v4(),
    );
    let path = debug_dir.join(file_name);
    match serde_json::to_vec_pretty(&dump)
        .map_err(std::io::Error::other)
        .and_then(|bytes| std::fs::write(&path, bytes))
    {
        Ok(()) => tracing::debug!(
            "{label} wrote conversation debug dump: {}",
            path.display()
        ),
        Err(e) => tracing::warn!("{label} failed to write conversation debug dump: {e}"),
    }
}

pub(super) fn should_retry_status(status: StatusCode) -> bool {
    status == StatusCode::FORBIDDEN
        || status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}

async fn retry_delay(attempt: usize) {
    tokio::time::sleep(Duration::from_millis((attempt as u64) * 250)).await;
}

fn upstream_log_name(upstream: &UpstreamConfig) -> &str {
    upstream.name.as_deref().unwrap_or("<unnamed>")
}

pub(super) fn log_upstream_target(label: &str, upstream: &UpstreamConfig, target: &str) {
    if let Some(name) = upstream.name.as_deref() {
        tracing::info!("{label} -> upstream {name}: {target}");
    } else {
        tracing::info!("{label} -> upstream: {target}");
    }
}

pub(super) async fn send_with_retries<F>(
    label: &str,
    upstream: &UpstreamConfig,
    mut build: F,
) -> Result<reqwest::Response, reqwest::Error>
where
    F: FnMut() -> RequestBuilder,
{
    for failed_attempts in 0..=MAX_RETRIES {
        let upstream_name = upstream_log_name(upstream);
        if failed_attempts == 0 {
            tracing::info!("{label} upstream {upstream_name} sending request");
        } else {
            tracing::info!(
                "{label} upstream {upstream_name} retry attempt {failed_attempts}/{MAX_RETRIES}"
            );
        }

        match build().send().await {
            Ok(resp) => {
                let status =
                    StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);

                if should_retry_status(status) && failed_attempts < MAX_RETRIES {
                    if failed_attempts == 0 {
                        tracing::warn!(
                            "{label} upstream {upstream_name} request returned retryable status {status}; retrying"
                        );
                    } else {
                        tracing::warn!(
                            "{label} upstream {upstream_name} retry attempt {failed_attempts}/{MAX_RETRIES} returned retryable status {status}; retrying"
                        );
                    }
                    retry_delay(failed_attempts + 1).await;
                    continue;
                }

                return Ok(resp);
            }
            Err(e) if failed_attempts < MAX_RETRIES => {
                if failed_attempts == 0 {
                    tracing::warn!(
                        "{label} upstream {upstream_name} request failed: {e}; retrying"
                    );
                } else {
                    tracing::warn!(
                        "{label} upstream {upstream_name} retry attempt {failed_attempts}/{MAX_RETRIES} failed: {e}; retrying"
                    );
                }
                retry_delay(failed_attempts + 1).await;
            }
            Err(e) => return Err(e),
        }
    }

    unreachable!()
}

pub(super) struct UpstreamSelection<'a> {
    pub(super) upstream: &'a UpstreamState,
    pub(super) upstream_model: Option<String>,
    pub(super) requested_model: Option<String>,
    pub(super) route_model: Option<String>,
    pub(super) used_fallback: bool,
}

fn resolve_model_alias<'a>(config: &'a Config, model: &'a str) -> &'a str {
    config
        .routing
        .model_aliases
        .get(model)
        .map(String::as_str)
        .unwrap_or(model)
}

pub(super) fn prefix_model_item_id(item: &mut serde_json::Value, name: &str) {
    let Some(obj) = item.as_object_mut() else {
        return;
    };
    let Some(id) = obj.get("id").and_then(|id| id.as_str()).map(str::to_string) else {
        return;
    };

    obj.insert(
        "id".into(),
        serde_json::Value::String(format!("{name}:{id}")),
    );
}

pub(super) fn append_model_alias_items(
    body: &mut serde_json::Value,
    aliases: &std::collections::BTreeMap<String, String>,
) {
    let Some(items) = body.get_mut("data").and_then(|data| data.as_array_mut()) else {
        return;
    };

    for alias in aliases.keys() {
        if items
            .iter()
            .any(|item| item.get("id").and_then(|id| id.as_str()) == Some(alias.as_str()))
        {
            continue;
        }

        items.push(serde_json::json!({
            "id": alias,
            "object": "model"
        }));
    }
}

pub(super) fn log_model_route(label: &str, selection: &UpstreamSelection<'_>) {
    let in_model = selection.requested_model.as_deref().unwrap_or("<missing>");
    let route_model = selection.route_model.as_deref().unwrap_or("<none>");
    let out_model = selection.upstream_model.as_deref().unwrap_or("<unchanged>");
    let upstream_name = upstream_log_name(&selection.upstream.config);
    let fallback = if selection.used_fallback {
        " fallback=true"
    } else {
        ""
    };

    tracing::info!(
        "{label} ROUTE IN(model={in_model}) => ROUTE(model={route_model}) => OUT(upstream={upstream_name}, model={out_model}){fallback}"
    );
}

pub(super) fn log_upstream_status(
    label: &str,
    selection: &UpstreamSelection<'_>,
    status: StatusCode,
) {
    let out_model = selection.upstream_model.as_deref().unwrap_or("<unchanged>");
    let upstream_name = upstream_log_name(&selection.upstream.config);

    tracing::info!("{label} OUT status={status} upstream={upstream_name} model={out_model}");
}

pub(super) async fn fetch_model_items(
    upstream: &UpstreamState,
    label: &str,
) -> Result<ModelItemsSnapshot, StatusCode> {
    let config = &upstream.config;
    let upstream_url = config.url.trim_end_matches('/');
    let target = format!("{upstream_url}/models");
    let _upstream_permit = upstream.acquire_permit().await;
    let upstream_resp = send_with_retries(label, config, || {
        apply_model_list_headers(upstream.client.get(&target), config)
    })
    .await
    .map_err(|e| {
        tracing::error!("{label} upstream request failed: {e}");
        StatusCode::BAD_GATEWAY
    })?;
    let status =
        StatusCode::from_u16(upstream_resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    tracing::debug!("{label} upstream protocol: {:?}", upstream_resp.version());
    let bytes = upstream_resp.bytes().await.map_err(|e| {
        tracing::error!("{label} failed to read upstream response: {e}");
        StatusCode::BAD_GATEWAY
    })?;

    if status.is_client_error() || status.is_server_error() {
        tracing::warn!(
            "{label} upstream error: {}",
            String::from_utf8_lossy(&bytes)
        );
        return Err(status);
    }

    let body: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
        tracing::error!("{label} failed to parse upstream models response: {e}");
        StatusCode::BAD_GATEWAY
    })?;

    let items = body
        .get("data")
        .and_then(|data| data.as_array())
        .cloned()
        .unwrap_or_default();
    tracing::debug!(
        "{label} refreshed model cache for upstream {} with {} models",
        upstream.config.url,
        items.len()
    );
    Ok(upstream.store_model_items(items).await)
}

pub(super) async fn fetch_model_items_cached(
    upstream: &UpstreamState,
    label: &str,
) -> Result<ModelItemsSnapshot, StatusCode> {
    if let Some(snapshot) = upstream.fresh_model_items().await {
        tracing::debug!(
            "{label} model cache hit for upstream {} with {} models",
            upstream.config.url,
            snapshot.len()
        );
        return Ok(snapshot);
    }

    tracing::debug!(
        "{label} model cache miss for upstream {}",
        upstream.config.url
    );
    let _refresh = upstream.model_cache_refresh.lock().await;
    if let Some(snapshot) = upstream.fresh_model_items().await {
        tracing::debug!(
            "{label} model cache hit after waiting for upstream {} with {} models",
            upstream.config.url,
            snapshot.len()
        );
        return Ok(snapshot);
    }
    match fetch_model_items(upstream, label).await {
        Ok(snapshot) => Ok(snapshot),
        Err(status) => {
            if let Some(snapshot) = upstream.stale_model_items().await {
                tracing::warn!(
                    "{label} using stale model cache for upstream {} after refresh failed: {status}",
                    upstream.config.url
                );
                Ok(snapshot)
            } else {
                Err(status)
            }
        }
    }
}

fn order_upstream_candidates<'a>(
    state: &'a AppState,
    mut candidates: Vec<&'a UpstreamState>,
) -> Vec<&'a UpstreamState> {
    if candidates.len() > 1 && state.config.routing.load_balance == LoadBalanceStrategy::RoundRobin
    {
        let start = state.select_index(candidates.len());
        candidates.rotate_left(start);
    }
    candidates
}

pub(super) async fn select_upstreams_for_model<'a>(
    state: &'a AppState,
    model: Option<&str>,
) -> Vec<UpstreamSelection<'a>> {
    let requested_model = model;
    let route_model = model.map(|model| resolve_model_alias(&state.config, model));
    let require_model_match = state.config.routing.fallback_model.is_some();
    let selections =
        select_upstreams_for_resolved_model(state, route_model, require_model_match).await;

    if !selections.is_empty() {
        return attach_route_metadata(selections, requested_model, route_model, false);
    }

    if let Some(requested_model) = requested_model {
        let route_model_display = route_model.unwrap_or(requested_model);
        if let Some(fallback_model) = state.config.routing.fallback_model.as_deref() {
            tracing::warn!(
                "{ANSI_RED_BOLD}[router] ⚠ 路由模型找不到: IN={requested_model} ROUTE={route_model_display}; fallback_model={fallback_model}{ANSI_RESET}"
            );

            let fallback_selections =
                select_upstreams_for_resolved_model(state, Some(fallback_model), true).await;
            if !fallback_selections.is_empty() {
                return attach_route_metadata(
                    fallback_selections,
                    Some(requested_model),
                    Some(fallback_model),
                    true,
                );
            }

            tracing::warn!(
                "{ANSI_RED_BOLD}[router] ⚠ 兜底模型也找不到: fallback_model={fallback_model}{ANSI_RESET}"
            );
        } else {
            tracing::warn!(
                "{ANSI_RED_BOLD}[router] ⚠ 路由模型找不到: IN={requested_model} ROUTE={route_model_display}; 未配置 fallback_model{ANSI_RESET}"
            );
        }
    }

    attach_route_metadata(selections, requested_model, route_model, false)
}

fn attach_route_metadata<'a>(
    selections: Vec<UpstreamSelection<'a>>,
    requested_model: Option<&str>,
    route_model: Option<&str>,
    used_fallback: bool,
) -> Vec<UpstreamSelection<'a>> {
    selections
        .into_iter()
        .map(|mut selection| {
            selection.requested_model = requested_model.map(ToString::to_string);
            selection.route_model = route_model.map(ToString::to_string);
            selection.used_fallback = used_fallback;
            selection
        })
        .collect()
}

async fn select_upstreams_for_resolved_model<'a>(
    state: &'a AppState,
    route_model: Option<&str>,
    require_model_match: bool,
) -> Vec<UpstreamSelection<'a>> {
    let upstreams = &state.upstreams;
    if route_model.is_none() || (upstreams.len() <= 1 && !require_model_match) {
        let mut selected = None;
        for upstream in upstreams {
            if upstream.is_healthy().await {
                selected = Some(upstream.as_ref());
                break;
            }
        }
        let upstream = match selected.or_else(|| upstreams.first().map(Arc::as_ref)) {
            Some(upstream) => upstream,
            None => return Vec::new(),
        };
        return Some(upstream)
            .map(|upstream| {
                let upstream_model = route_model.map(|model| {
                    upstream
                        .config
                        .name
                        .as_ref()
                        .and_then(|name| model.strip_prefix(&format!("{name}:")))
                        .unwrap_or(model)
                        .to_string()
                });
                UpstreamSelection {
                    upstream,
                    upstream_model,
                    requested_model: None,
                    route_model: None,
                    used_fallback: false,
                }
            })
            .into_iter()
            .collect();
    }

    let model = route_model.unwrap();
    if let Some((upstream_name, upstream_model)) = model.split_once(':') {
        for upstream in upstreams {
            if upstream.config.name.as_deref() != Some(upstream_name) {
                continue;
            }
            if !upstream.is_healthy().await {
                continue;
            }

            match fetch_model_items_cached(upstream, "[router]").await {
                Ok(snapshot) if snapshot.contains_model(upstream_model) => {
                    return vec![UpstreamSelection {
                        upstream: upstream.as_ref(),
                        upstream_model: Some(upstream_model.to_string()),
                        requested_model: None,
                        route_model: None,
                        used_fallback: false,
                    }];
                }
                Ok(_) => {}
                Err(status) => {
                    tracing::warn!(
                        "[router] failed to inspect upstream {} models: {status}",
                        upstream.config.url
                    );
                }
            }
        }

        return Vec::new();
    }

    let mut candidates = Vec::new();
    for upstream in upstreams {
        if !upstream.is_healthy().await {
            continue;
        }
        match fetch_model_items_cached(upstream, "[router]").await {
            Ok(snapshot) if snapshot.contains_model(model) => {
                candidates.push(upstream.as_ref());
            }
            Ok(_) => {}
            Err(status) => {
                tracing::warn!(
                    "[router] failed to inspect upstream {} models: {status}",
                    upstream.config.url
                );
            }
        }
    }

    if candidates.is_empty() {
        Vec::new()
    } else {
        order_upstream_candidates(state, candidates)
            .into_iter()
            .map(|upstream| UpstreamSelection {
                upstream,
                upstream_model: Some(model.to_string()),
                requested_model: None,
                route_model: None,
                used_fallback: false,
            })
            .collect()
    }
}
