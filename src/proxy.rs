use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use futures::StreamExt;
use reqwest::{Client, Proxy, RequestBuilder};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use crate::config::{Config, LoadBalanceStrategy, UpstreamConfig};
use crate::convert;
use crate::types::*;

const MAX_RETRIES: usize = 3;
const MODEL_CACHE_TTL: Duration = Duration::from_secs(300);

pub struct AppState {
    config: Config,
    upstreams: Vec<Arc<UpstreamState>>,
    next_upstream: AtomicUsize,
    global_request_limiter: Option<FixedWindowRateLimiter>,
    global_token_limiter: Option<FixedWindowRateLimiter>,
}

struct UpstreamState {
    config: UpstreamConfig,
    client: Client,
    model_cache: tokio::sync::RwLock<Option<CachedModels>>,
    model_cache_refresh: tokio::sync::Mutex<()>,
    concurrency_limit: Option<Arc<tokio::sync::Semaphore>>,
    request_limiter: Option<FixedWindowRateLimiter>,
    token_limiter: Option<FixedWindowRateLimiter>,
    health: tokio::sync::RwLock<UpstreamHealth>,
}

struct CachedModels {
    items: Vec<serde_json::Value>,
    expires_at: Instant,
}

struct FixedWindowRateLimiter {
    limit: u64,
    window: tokio::sync::Mutex<FixedWindowRateWindow>,
}

struct FixedWindowRateWindow {
    started_at: Instant,
    used: u64,
}

#[derive(Clone, Copy)]
struct UpstreamHealth {
    healthy: bool,
    failures: u64,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        let upstream_max_concurrent_requests = config.performance.upstream_max_concurrent_requests;
        let global_request_limiter = FixedWindowRateLimiter::new(config.performance.global_rpm);
        let global_token_limiter = FixedWindowRateLimiter::new(config.performance.global_tpm);
        let upstreams = config
            .configured_upstreams()
            .into_iter()
            .cloned()
            .map(|upstream| {
                Arc::new(UpstreamState::new(
                    upstream,
                    upstream_max_concurrent_requests,
                ))
            })
            .collect::<Vec<_>>();
        if upstreams.len() > 1 {
            spawn_model_cache_warmup(upstreams.clone());
        }
        if config.health.enabled {
            spawn_health_checks(
                upstreams.clone(),
                config.health.interval_millis,
                config.health.unhealthy_after_failures,
            );
        }

        Self {
            config,
            upstreams,
            next_upstream: AtomicUsize::new(0),
            global_request_limiter,
            global_token_limiter,
        }
    }

    fn select_index(&self, len: usize) -> usize {
        match self.config.routing.load_balance {
            LoadBalanceStrategy::First => 0,
            LoadBalanceStrategy::RoundRobin => {
                self.next_upstream.fetch_add(1, Ordering::Relaxed) % len
            }
        }
    }

    async fn check_global_rate_limits(&self, tokens: u64) -> Result<(), StatusCode> {
        if let Some(limiter) = self.global_request_limiter.as_ref() {
            if !limiter.try_acquire(1).await {
                return Err(StatusCode::TOO_MANY_REQUESTS);
            }
        }
        if let Some(limiter) = self.global_token_limiter.as_ref() {
            if !limiter.try_acquire(tokens).await {
                return Err(StatusCode::TOO_MANY_REQUESTS);
            }
        }
        Ok(())
    }
}

impl UpstreamState {
    fn new(config: UpstreamConfig, max_concurrent_requests: Option<usize>) -> Self {
        let client = upstream_client(&config);
        let concurrency_limit = config
            .max_concurrent_requests
            .or(max_concurrent_requests)
            .filter(|limit| *limit > 0)
            .map(|limit| Arc::new(tokio::sync::Semaphore::new(limit)));
        let request_limiter = FixedWindowRateLimiter::new(config.rpm);
        let token_limiter = FixedWindowRateLimiter::new(config.tpm);

        Self {
            config,
            client,
            model_cache: tokio::sync::RwLock::new(None),
            model_cache_refresh: tokio::sync::Mutex::new(()),
            concurrency_limit,
            request_limiter,
            token_limiter,
            health: tokio::sync::RwLock::new(UpstreamHealth {
                healthy: true,
                failures: 0,
            }),
        }
    }

    async fn acquire_permit(&self) -> Option<tokio::sync::OwnedSemaphorePermit> {
        let limit = self.concurrency_limit.as_ref()?;
        Some(
            limit
                .clone()
                .acquire_owned()
                .await
                .expect("upstream concurrency semaphore closed"),
        )
    }

    async fn fresh_model_items(&self) -> Option<Vec<serde_json::Value>> {
        let cache = self.model_cache.read().await;
        let cached = cache.as_ref()?;
        if Instant::now() < cached.expires_at {
            Some(cached.items.clone())
        } else {
            None
        }
    }

    async fn check_rate_limits(&self, tokens: u64) -> Result<(), StatusCode> {
        if let Some(limiter) = self.request_limiter.as_ref() {
            if !limiter.try_acquire(1).await {
                return Err(StatusCode::TOO_MANY_REQUESTS);
            }
        }
        if let Some(limiter) = self.token_limiter.as_ref() {
            if !limiter.try_acquire(tokens).await {
                return Err(StatusCode::TOO_MANY_REQUESTS);
            }
        }
        Ok(())
    }

    async fn is_healthy(&self) -> bool {
        self.health.read().await.healthy
    }

    async fn set_health_check_result(&self, healthy: bool, unhealthy_after_failures: u64) {
        let mut health = self.health.write().await;
        if healthy {
            health.healthy = true;
            health.failures = 0;
        } else {
            health.failures = health.failures.saturating_add(1);
            if health.failures >= unhealthy_after_failures {
                health.healthy = false;
            }
        }
    }

    async fn stale_model_items(&self) -> Option<Vec<serde_json::Value>> {
        self.model_cache
            .read()
            .await
            .as_ref()
            .map(|cached| cached.items.clone())
    }

    async fn store_model_items(&self, items: Vec<serde_json::Value>) {
        *self.model_cache.write().await = Some(CachedModels {
            items,
            expires_at: Instant::now() + MODEL_CACHE_TTL,
        });
    }
}

impl FixedWindowRateLimiter {
    fn new(limit: Option<u64>) -> Option<Self> {
        let limit = limit.filter(|limit| *limit > 0)?;
        Some(Self {
            limit,
            window: tokio::sync::Mutex::new(FixedWindowRateWindow {
                started_at: Instant::now(),
                used: 0,
            }),
        })
    }

    async fn try_acquire(&self, amount: u64) -> bool {
        if amount > self.limit {
            return false;
        }

        let mut window = self.window.lock().await;
        if window.started_at.elapsed() >= Duration::from_secs(60) {
            window.started_at = Instant::now();
            window.used = 0;
        }
        if window.used.saturating_add(amount) > self.limit {
            return false;
        }
        window.used += amount;
        true
    }
}

fn spawn_health_checks(
    upstreams: Vec<Arc<UpstreamState>>,
    interval_millis: u64,
    unhealthy_after_failures: u64,
) {
    tokio::spawn(async move {
        let interval = Duration::from_millis(interval_millis.max(1));
        loop {
            for upstream in &upstreams {
                let upstream_url = upstream.config.url.trim_end_matches('/');
                let target = format!("{upstream_url}/models");
                let healthy =
                    match apply_upstream_auth(upstream.client.get(&target), &upstream.config)
                        .send()
                        .await
                    {
                        Ok(resp) => resp.status().is_success(),
                        Err(_) => false,
                    };
                upstream
                    .set_health_check_result(healthy, unhealthy_after_failures)
                    .await;
            }
            tokio::time::sleep(interval).await;
        }
    });
}

fn spawn_model_cache_warmup(upstreams: Vec<Arc<UpstreamState>>) {
    tokio::spawn(async move {
        for upstream in upstreams {
            if let Err(status) = fetch_model_items_cached(&upstream, "[startup]").await {
                tracing::warn!(
                    "[startup] failed to prefetch upstream {} models: {status}",
                    upstream.config.url
                );
            }
        }
    });
}

fn upstream_client(upstream: &UpstreamConfig) -> Client {
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

fn apply_upstream_auth(builder: RequestBuilder, upstream: &UpstreamConfig) -> RequestBuilder {
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

fn is_proxy_authorized(config: &Config, headers: &HeaderMap) -> bool {
    let Some(auth_key) = config.auth_key.as_deref().filter(|key| !key.is_empty()) else {
        return true;
    };
    let expected = format!("Bearer {auth_key}");
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == expected)
}

fn unauthorized_response() -> Response {
    (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
}

fn estimate_tokens(value: &serde_json::Value) -> u64 {
    let chars = value.to_string().chars().count() as u64;
    chars.div_ceil(4).max(1)
}

fn should_retry_status(status: StatusCode) -> bool {
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

fn log_upstream_target(label: &str, upstream: &UpstreamConfig, target: &str) {
    if let Some(name) = upstream.name.as_deref() {
        tracing::info!("{label} -> upstream {name}: {target}");
    } else {
        tracing::info!("{label} -> upstream: {target}");
    }
}

async fn send_with_retries<F>(
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

struct UpstreamSelection<'a> {
    upstream: &'a UpstreamState,
    upstream_model: Option<String>,
}

fn prefix_model_item_id(item: &mut serde_json::Value, name: &str) {
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

async fn fetch_model_items(
    upstream: &UpstreamState,
    label: &str,
) -> Result<Vec<serde_json::Value>, StatusCode> {
    let config = &upstream.config;
    let upstream_url = config.url.trim_end_matches('/');
    let target = format!("{upstream_url}/models");
    let _upstream_permit = upstream.acquire_permit().await;
    let upstream_resp = send_with_retries(label, config, || {
        apply_upstream_auth(upstream.client.get(&target), config)
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
    upstream.store_model_items(items.clone()).await;
    Ok(items)
}

async fn fetch_model_items_cached(
    upstream: &UpstreamState,
    label: &str,
) -> Result<Vec<serde_json::Value>, StatusCode> {
    if let Some(items) = upstream.fresh_model_items().await {
        tracing::debug!(
            "{label} model cache hit for upstream {} with {} models",
            upstream.config.url,
            items.len()
        );
        return Ok(items);
    }

    tracing::debug!(
        "{label} model cache miss for upstream {}",
        upstream.config.url
    );
    let _refresh = upstream.model_cache_refresh.lock().await;
    if let Some(items) = upstream.fresh_model_items().await {
        tracing::debug!(
            "{label} model cache hit after waiting for upstream {} with {} models",
            upstream.config.url,
            items.len()
        );
        return Ok(items);
    }
    match fetch_model_items(upstream, label).await {
        Ok(items) => Ok(items),
        Err(status) => {
            if let Some(items) = upstream.stale_model_items().await {
                tracing::warn!(
                    "{label} using stale model cache for upstream {} after refresh failed: {status}",
                    upstream.config.url
                );
                Ok(items)
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

async fn select_upstreams_for_model<'a>(
    state: &'a AppState,
    model: Option<&str>,
) -> Vec<UpstreamSelection<'a>> {
    let upstreams = &state.upstreams;
    if upstreams.len() <= 1 || model.is_none() {
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
                let upstream_model = model.map(|model| {
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
                }
            })
            .into_iter()
            .collect();
    }

    let model = model.unwrap();
    if let Some((upstream_name, upstream_model)) = model.split_once(':') {
        for upstream in upstreams {
            if upstream.config.name.as_deref() != Some(upstream_name) {
                continue;
            }
            if !upstream.is_healthy().await {
                continue;
            }

            match fetch_model_items_cached(upstream, "[router]").await {
                Ok(items)
                    if items.iter().any(|item| {
                        item.get("id").and_then(|id| id.as_str()) == Some(upstream_model)
                    }) =>
                {
                    return vec![UpstreamSelection {
                        upstream: upstream.as_ref(),
                        upstream_model: Some(upstream_model.to_string()),
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
            Ok(items)
                if items
                    .iter()
                    .any(|item| item.get("id").and_then(|id| id.as_str()) == Some(model)) =>
            {
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
            })
            .collect()
    }
}

pub async fn chat_completions(State(state): State<Arc<AppState>>, req: Request<Body>) -> Response {
    let start = Instant::now();
    let (parts, body) = req.into_parts();

    if !is_proxy_authorized(&state.config, &parts.headers) {
        return unauthorized_response();
    }

    let body_bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("failed to read request body: {e}");
            return (StatusCode::BAD_REQUEST, "failed to read body").into_response();
        }
    };

    let request_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap_or_default();
    let request_tokens = estimate_tokens(&request_json);
    if state
        .check_global_rate_limits(request_tokens)
        .await
        .is_err()
    {
        return (StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded").into_response();
    }
    let is_stream = request_json
        .get("stream")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);
    let model = request_json.get("model").and_then(|m| m.as_str());

    tracing::info!("[passthrough] POST /v1/chat/completions stream={is_stream}");

    let selections = select_upstreams_for_model(&state, model).await;
    if selections.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            "model not found in configured upstreams",
        )
            .into_response();
    }
    let last_selection_index = selections.len() - 1;

    for (selection_index, selection) in selections.into_iter().enumerate() {
        let can_failover =
            state.config.routing.automatic_failover && selection_index < last_selection_index;
        let upstream = selection.upstream;
        let upstream_config = &upstream.config;
        if upstream.check_rate_limits(request_tokens).await.is_err() {
            if can_failover {
                tracing::warn!("[passthrough] upstream rate limit exceeded; failing over");
                continue;
            }
            return (StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded").into_response();
        }

        let upstream_url = upstream_config.url.trim_end_matches('/');
        let target = format!("{upstream_url}/chat/completions");
        log_upstream_target("[passthrough]", upstream_config, &target);
        let upstream_body = if let Some(upstream_model) = selection.upstream_model.as_ref() {
            let mut upstream_json = request_json.clone();
            if let Some(obj) = upstream_json.as_object_mut() {
                obj.insert(
                    "model".into(),
                    serde_json::Value::String(upstream_model.clone()),
                );
            }
            serde_json::to_vec(&upstream_json).unwrap_or_else(|_| body_bytes.to_vec())
        } else {
            body_bytes.to_vec()
        };

        let upstream_permit = upstream.acquire_permit().await;
        let upstream_resp = match send_with_retries("[passthrough]", upstream_config, || {
            let mut builder = apply_upstream_auth(
                upstream
                    .client
                    .post(&target)
                    .header("content-type", "application/json"),
                upstream_config,
            );

            for (key, value) in parts.headers.iter() {
                if key == "host" || key == "authorization" {
                    continue;
                }
                if let Ok(v) = value.to_str() {
                    builder = builder.header(key.as_str(), v);
                }
            }

            builder.body(upstream_body.clone())
        })
        .await
        {
            Ok(r) => r,
            Err(e) => {
                if can_failover {
                    tracing::warn!("[passthrough] upstream request failed: {e}; failing over");
                    continue;
                }
                tracing::error!("[passthrough] upstream request failed: {e}");
                return (StatusCode::BAD_GATEWAY, "upstream request failed").into_response();
            }
        };

        let status = StatusCode::from_u16(upstream_resp.status().as_u16())
            .unwrap_or(StatusCode::BAD_GATEWAY);
        tracing::debug!(
            "[passthrough] upstream protocol: {:?}",
            upstream_resp.version()
        );

        tracing::info!("[passthrough] upstream responded: {status}");

        if can_failover && should_retry_status(status) {
            tracing::warn!("[passthrough] upstream returned {status}; failing over");
            continue;
        }

        if is_stream && status.is_success() {
            tracing::info!(
                "[passthrough] streaming response, elapsed={}ms",
                start.elapsed().as_millis()
            );
            let stream = upstream_resp.bytes_stream();
            let body = Body::from_stream(stream.map(move |r| {
                let _upstream_permit = &upstream_permit;
                r.map_err(std::io::Error::other)
            }));

            return Response::builder()
                .status(status)
                .header("content-type", "text/event-stream")
                .header("cache-control", "no-cache")
                .body(body)
                .unwrap();
        }

        return match upstream_resp.bytes().await {
            Ok(bytes) => {
                if status.is_client_error() || status.is_server_error() {
                    tracing::warn!(
                        "[passthrough] upstream error: {}",
                        String::from_utf8_lossy(&bytes)
                    );
                }
                tracing::info!(
                    "[passthrough] done, {}B, elapsed={}ms",
                    bytes.len(),
                    start.elapsed().as_millis()
                );
                let mut resp = Response::new(Body::from(bytes));
                *resp.status_mut() = status;
                resp.headers_mut()
                    .insert("content-type", "application/json".parse().unwrap());
                resp
            }
            Err(e) => {
                tracing::error!("[passthrough] failed to read upstream response: {e}");
                (StatusCode::BAD_GATEWAY, "failed to read upstream response").into_response()
            }
        };
    }

    (StatusCode::BAD_GATEWAY, "upstream request failed").into_response()
}

pub async fn responses(State(state): State<Arc<AppState>>, req: Request<Body>) -> Response {
    let start = Instant::now();
    let (parts, body) = req.into_parts();

    if !is_proxy_authorized(&state.config, &parts.headers) {
        return unauthorized_response();
    }

    let body_bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("failed to read request body: {e}");
            return (StatusCode::BAD_REQUEST, "failed to read body").into_response();
        }
    };

    let responses_req: ResponsesRequest = match serde_json::from_slice(&body_bytes) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("[responses] failed to parse request: {e}");
            return (StatusCode::BAD_REQUEST, format!("invalid request: {e}")).into_response();
        }
    };
    let request_json: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap_or_default();
    let request_tokens = estimate_tokens(&request_json);
    if state
        .check_global_rate_limits(request_tokens)
        .await
        .is_err()
    {
        return (StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded").into_response();
    }

    let is_stream = responses_req.stream.unwrap_or(false);
    tracing::info!(
        "[responses] POST /v1/responses model={} stream={is_stream}",
        responses_req.model
    );

    let chat_req = convert::responses_to_chat(&responses_req);

    let chat_body = match serde_json::to_vec(&chat_req) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!("[responses] failed to serialize chat request: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "conversion error").into_response();
        }
    };

    let selections = select_upstreams_for_model(&state, Some(&responses_req.model)).await;
    if selections.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            "model not found in configured upstreams",
        )
            .into_response();
    }
    let last_selection_index = selections.len() - 1;
    let mut selected_response = None;

    for (selection_index, selection) in selections.into_iter().enumerate() {
        let can_failover =
            state.config.routing.automatic_failover && selection_index < last_selection_index;
        let upstream = selection.upstream;
        let upstream_config = &upstream.config;

        if upstream.check_rate_limits(request_tokens).await.is_err() {
            if can_failover {
                tracing::warn!("[responses] upstream rate limit exceeded; failing over");
                continue;
            }
            return (StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded").into_response();
        }

        let upstream_model = selection
            .upstream_model
            .clone()
            .unwrap_or_else(|| responses_req.model.clone());
        let upstream_chat_body = if upstream_model != chat_req.model {
            let mut chat_json = serde_json::to_value(&chat_req).unwrap_or_default();
            if let Some(obj) = chat_json.as_object_mut() {
                obj.insert("model".into(), serde_json::Value::String(upstream_model));
            }
            serde_json::to_vec(&chat_json).unwrap_or_else(|_| chat_body.clone())
        } else {
            chat_body.clone()
        };

        let upstream_url = upstream_config.url.trim_end_matches('/');
        let target = format!("{upstream_url}/chat/completions");
        log_upstream_target("[responses]", upstream_config, &target);

        let upstream_permit = upstream.acquire_permit().await;
        let upstream_resp = match send_with_retries("[responses]", upstream_config, || {
            apply_upstream_auth(
                upstream
                    .client
                    .post(&target)
                    .header("content-type", "application/json"),
                upstream_config,
            )
            .body(upstream_chat_body.clone())
        })
        .await
        {
            Ok(r) => r,
            Err(e) => {
                if can_failover {
                    tracing::warn!("[responses] upstream request failed: {e}; failing over");
                    continue;
                }
                tracing::error!("[responses] upstream request failed: {e}");
                return (StatusCode::BAD_GATEWAY, "upstream request failed").into_response();
            }
        };

        let status = StatusCode::from_u16(upstream_resp.status().as_u16())
            .unwrap_or(StatusCode::BAD_GATEWAY);
        tracing::debug!(
            "[responses] upstream protocol: {:?}",
            upstream_resp.version()
        );

        tracing::info!("[responses] upstream responded: {status}");

        if can_failover && should_retry_status(status) {
            tracing::warn!("[responses] upstream returned {status}; failing over");
            continue;
        }

        selected_response = Some((upstream_resp, status, upstream_permit));
        break;
    }

    let Some((upstream_resp, status, upstream_permit)) = selected_response else {
        return (StatusCode::BAD_GATEWAY, "upstream request failed").into_response();
    };

    if status.is_client_error() || status.is_server_error() {
        let err_bytes = upstream_resp.bytes().await.unwrap_or_default();
        tracing::warn!(
            "[responses] upstream error: {}",
            String::from_utf8_lossy(&err_bytes)
        );
        let mut resp = Response::new(Body::from(err_bytes));
        *resp.status_mut() = status;
        resp.headers_mut()
            .insert("content-type", "application/json".parse().unwrap());
        return resp;
    }

    // ── Non-streaming ──
    if !is_stream {
        return match upstream_resp.bytes().await {
            Ok(bytes) => {
                let chat_resp: ChatResponse = match serde_json::from_slice(&bytes) {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::error!("[responses] failed to parse upstream response: {e}");
                        return (
                            StatusCode::BAD_GATEWAY,
                            format!("invalid upstream response: {e}"),
                        )
                            .into_response();
                    }
                };

                let responses_resp = convert::chat_to_responses(&chat_resp);
                let out = serde_json::to_vec(&responses_resp).unwrap();
                tracing::info!(
                    "[responses] done, output items={}, {}B, elapsed={}ms",
                    responses_resp.output.len(),
                    out.len(),
                    start.elapsed().as_millis()
                );
                let mut resp = Response::new(Body::from(out));
                *resp.status_mut() = StatusCode::OK;
                resp.headers_mut()
                    .insert("content-type", "application/json".parse().unwrap());
                resp
            }
            Err(e) => {
                tracing::error!("[responses] failed to read upstream response: {e}");
                (StatusCode::BAD_GATEWAY, "failed to read upstream response").into_response()
            }
        };
    }

    // ── Streaming ──
    tracing::info!("[responses] starting stream conversion");
    let stream = upstream_resp.bytes_stream();
    let model = responses_req.model.clone();

    let transformed = futures::stream::unfold(
        (
            stream,
            convert::StreamState::new(),
            convert::SseDecoder::new(),
            model,
            false,
            Vec::new(),
            None::<ChatUsage>,
            false,
        ),
        |(
            mut stream,
            mut state,
            mut decoder,
            model,
            mut started,
            mut pending,
            mut last_usage,
            mut completed,
        )| async move {
            loop {
                // drain pending
                if !pending.is_empty() {
                    let event = pending.remove(0);
                    return Some((
                        Ok::<_, std::io::Error>(event),
                        (
                            stream, state, decoder, model, started, pending, last_usage, completed,
                        ),
                    ));
                }

                // read next chunk from upstream
                match stream.next().await {
                    Some(Ok(chunk_bytes)) => {
                        for data in decoder.push(&chunk_bytes) {
                            if data.trim() == "[DONE]" {
                                tracing::info!("[responses] stream: upstream sent [DONE]");
                                if !completed {
                                    let finish_events = convert::convert_stream_finish(&mut state);
                                    pending.extend(finish_events);
                                    let end_event =
                                        convert::convert_stream_end(&state, last_usage.as_ref());
                                    pending.push(end_event);
                                    completed = true;
                                }
                                continue;
                            }

                            let chunk: ChatChunk = match serde_json::from_str(&data) {
                                Ok(c) => c,
                                Err(e) => {
                                    tracing::warn!(
                                        "[responses] stream: failed to parse chunk: {e}"
                                    );
                                    continue;
                                }
                            };

                            // save usage if present
                            if chunk.usage.is_some() {
                                last_usage = chunk.usage.clone();
                            }

                            // emit stream start on first chunk
                            if !started {
                                tracing::info!(
                                    "[responses] stream: first chunk received, emitting start events"
                                );
                                let start_events =
                                    convert::convert_stream_start(&mut state, &model);
                                pending.extend(start_events);
                                started = true;
                            }

                            let chunk_events = convert::convert_stream_chunk(&mut state, &chunk);
                            pending.extend(chunk_events);
                        }
                    }
                    Some(Err(e)) => {
                        tracing::error!("[responses] upstream stream error: {e}");
                        return None;
                    }
                    None => {
                        tracing::info!("[responses] stream: upstream connection closed");
                        if started && !completed {
                            let finish_events = convert::convert_stream_finish(&mut state);
                            pending.extend(finish_events);
                            let end_event =
                                convert::convert_stream_end(&state, last_usage.as_ref());
                            pending.push(end_event);
                            completed = true;
                            continue;
                        }
                        return None;
                    }
                }
            }
        },
    );

    let body = Body::from_stream(transformed.map(move |r| {
        let _upstream_permit = &upstream_permit;
        r.map_err(std::io::Error::other)
    }));

    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .body(body)
        .unwrap()
}

pub async fn models(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if !is_proxy_authorized(&state.config, &headers) {
        return unauthorized_response();
    }

    let upstreams = &state.upstreams;

    if upstreams.is_empty() {
        return (StatusCode::BAD_GATEWAY, "no upstreams configured").into_response();
    }

    if upstreams.len() == 1 {
        let upstream = &upstreams[0];
        let upstream_config = &upstream.config;
        let upstream_url = upstream_config.url.trim_end_matches('/');
        let target = format!("{upstream_url}/models");
        log_upstream_target("[models] GET /v1/models", upstream_config, &target);
        let _upstream_permit = upstream.acquire_permit().await;

        let upstream_resp = match send_with_retries("[models]", upstream_config, || {
            apply_upstream_auth(upstream.client.get(&target), upstream_config)
        })
        .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("[models] upstream request failed: {e}");
                return (StatusCode::BAD_GATEWAY, "upstream request failed").into_response();
            }
        };

        let status = StatusCode::from_u16(upstream_resp.status().as_u16())
            .unwrap_or(StatusCode::BAD_GATEWAY);
        tracing::debug!("[models] upstream protocol: {:?}", upstream_resp.version());

        return match upstream_resp.bytes().await {
            Ok(bytes) => {
                if status.is_client_error() || status.is_server_error() {
                    tracing::warn!(
                        "[models] upstream error: {}",
                        String::from_utf8_lossy(&bytes)
                    );
                }
                let bytes = if let Some(name) = upstream_config.name.as_ref() {
                    match serde_json::from_slice::<serde_json::Value>(&bytes) {
                        Ok(mut body) => {
                            if let Some(items) =
                                body.get_mut("data").and_then(|data| data.as_array_mut())
                            {
                                for item in items {
                                    prefix_model_item_id(item, name);
                                }
                            }
                            serde_json::to_vec(&body).unwrap_or_else(|_| bytes.to_vec())
                        }
                        Err(_) => bytes.to_vec(),
                    }
                } else {
                    bytes.to_vec()
                };
                let mut resp = Response::new(Body::from(bytes));
                *resp.status_mut() = status;
                resp.headers_mut()
                    .insert("content-type", "application/json".parse().unwrap());
                resp
            }
            Err(e) => {
                tracing::error!("[models] failed to read upstream response: {e}");
                (StatusCode::BAD_GATEWAY, "failed to read upstream response").into_response()
            }
        };
    }

    let mut merged = Vec::new();
    for upstream in upstreams {
        match fetch_model_items(upstream, "[models]").await {
            Ok(items) => {
                for mut item in items {
                    if let Some(name) = upstream.config.name.as_ref() {
                        prefix_model_item_id(&mut item, name);
                    }
                    merged.push(item);
                }
            }
            Err(status) => {
                return (status, "failed to fetch upstream models").into_response();
            }
        }
    }

    let body = serde_json::json!({
        "object": "list",
        "data": merged
    });

    let mut resp = Response::new(Body::from(body.to_string()));
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut()
        .insert("content-type", "application/json".parse().unwrap());
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Json, Router,
        body::Bytes,
        routing::{get, post},
    };
    use serde_json::json;
    use std::{
        collections::HashSet,
        convert::Infallible,
        net::SocketAddr,
        sync::atomic::{AtomicUsize, Ordering},
    };
    use tokio::sync::Mutex;

    async fn spawn_upstream(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    async fn spawn_upstream_with_connect_info(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        format!("http://{addr}")
    }

    fn test_app_state(config: Config) -> Arc<AppState> {
        Arc::new(AppState::new(config))
    }

    fn test_config(upstream_url: String) -> Arc<AppState> {
        test_app_state(Config {
            port: 0,
            auth_key: None,
            performance: crate::config::PerformanceConfig::default(),
            routing: crate::config::RoutingConfig::default(),
            health: crate::config::HealthConfig::default(),
            upstream: Some(crate::config::UpstreamConfig {
                name: None,
                url: upstream_url,
                api_key: String::new(),
                auth_header: "Authorization".into(),
                proxy: None,
                proxy_type: crate::config::UpstreamProxyType::Http,
                max_concurrent_requests: None,
                rpm: None,
                tpm: None,
            }),
            upstreams: Vec::new(),
        })
    }

    fn test_multi_config(upstream_urls: Vec<String>) -> Arc<AppState> {
        test_app_state(Config {
            port: 0,
            auth_key: None,
            performance: crate::config::PerformanceConfig::default(),
            routing: crate::config::RoutingConfig::default(),
            health: crate::config::HealthConfig::default(),
            upstream: None,
            upstreams: upstream_urls
                .into_iter()
                .map(|url| crate::config::UpstreamConfig {
                    name: None,
                    url,
                    api_key: String::new(),
                    auth_header: "Authorization".into(),
                    proxy: None,
                    proxy_type: crate::config::UpstreamProxyType::Http,
                    max_concurrent_requests: None,
                    rpm: None,
                    tpm: None,
                })
                .collect(),
        })
    }

    fn test_auth_config(upstream_url: String) -> Arc<AppState> {
        test_app_state(Config {
            port: 0,
            auth_key: Some("proxy-secret".into()),
            performance: crate::config::PerformanceConfig::default(),
            routing: crate::config::RoutingConfig::default(),
            health: crate::config::HealthConfig::default(),
            upstream: Some(crate::config::UpstreamConfig {
                name: None,
                url: upstream_url,
                api_key: String::new(),
                auth_header: "Authorization".into(),
                proxy: None,
                proxy_type: crate::config::UpstreamProxyType::Http,
                max_concurrent_requests: None,
                rpm: None,
                tpm: None,
            }),
            upstreams: Vec::new(),
        })
    }

    fn responses_request(stream: bool) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "model": "test-model",
                    "stream": stream,
                    "input": "hello"
                })
                .to_string(),
            ))
            .unwrap()
    }

    fn responses_model_request(model: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "model": model,
                    "input": "hello"
                })
                .to_string(),
            ))
            .unwrap()
    }

    fn chat_model_request(model: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "model": model,
                    "messages": [{"role": "user", "content": "hello"}]
                })
                .to_string(),
            ))
            .unwrap()
    }

    fn chat_request_without_model() -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "messages": [{"role": "user", "content": "hello"}]
                })
                .to_string(),
            ))
            .unwrap()
    }

    fn chat_request() -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "model": "test-model",
                    "messages": [{"role": "user", "content": "hello"}]
                })
                .to_string(),
            ))
            .unwrap()
    }

    #[tokio::test]
    async fn chat_completions_should_reuse_upstream_connection_across_requests() {
        let remote_ports = Arc::new(Mutex::new(HashSet::new()));
        let app = Router::new().route(
            "/chat/completions",
            post({
                let remote_ports = remote_ports.clone();
                move |axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<SocketAddr>| {
                    let remote_ports = remote_ports.clone();
                    async move {
                        remote_ports.lock().await.insert(addr.port());
                        Json(json!({
                            "id": "chatcmpl_1",
                            "model": "test-model",
                            "choices": [{
                                "index": 0,
                                "message": {"role": "assistant", "content": "ok"},
                                "finish_reason": "stop"
                            }]
                        }))
                    }
                }
            }),
        );
        let config = test_config(spawn_upstream_with_connect_info(app).await);

        let first = chat_completions(State(config.clone()), chat_request()).await;
        let second = chat_completions(State(config), chat_request()).await;

        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(second.status(), StatusCode::OK);
        assert_eq!(remote_ports.lock().await.len(), 1);
    }

    #[tokio::test]
    async fn chat_completions_should_limit_concurrent_requests_per_upstream_when_configured() {
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let app = Router::new().route(
            "/chat/completions",
            post({
                let active = active.clone();
                let max_active = max_active.clone();
                move || {
                    let active = active.clone();
                    let max_active = max_active.clone();
                    async move {
                        let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                        max_active.fetch_max(current, Ordering::SeqCst);
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                        active.fetch_sub(1, Ordering::SeqCst);
                        Json(json!({
                            "id": "chatcmpl_1",
                            "model": "test-model",
                            "choices": [{
                                "index": 0,
                                "message": {"role": "assistant", "content": "ok"},
                                "finish_reason": "stop"
                            }]
                        }))
                    }
                }
            }),
        );
        let upstream_url = spawn_upstream(app).await;
        let config: Config = toml::from_str(&format!(
            r#"
[performance]
upstream_max_concurrent_requests = 1

[upstream]
url = "{upstream_url}"
"#
        ))
        .unwrap();
        let state = test_app_state(config);

        let (first, second) = tokio::join!(
            chat_completions(State(state.clone()), chat_request()),
            chat_completions(State(state), chat_request())
        );

        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(second.status(), StatusCode::OK);
        assert_eq!(max_active.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn chat_completions_should_reject_request_when_global_rpm_limit_is_exhausted() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let app = Router::new().route(
            "/chat/completions",
            post({
                let attempts = attempts.clone();
                move || {
                    let attempts = attempts.clone();
                    async move {
                        attempts.fetch_add(1, Ordering::SeqCst);
                        Json(json!({
                            "id": "chatcmpl_1",
                            "model": "test-model",
                            "choices": [{
                                "index": 0,
                                "message": {"role": "assistant", "content": "ok"},
                                "finish_reason": "stop"
                            }]
                        }))
                    }
                }
            }),
        );
        let upstream_url = spawn_upstream(app).await;
        let config: Config = toml::from_str(&format!(
            r#"
[performance]
global_rpm = 1

[upstream]
url = "{upstream_url}"
"#
        ))
        .unwrap();
        let state = test_app_state(config);

        let first = chat_completions(State(state.clone()), chat_request()).await;
        let second = chat_completions(State(state), chat_request()).await;

        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn chat_completions_should_reject_request_when_upstream_tpm_limit_would_be_exceeded() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let app = Router::new().route(
            "/chat/completions",
            post({
                let attempts = attempts.clone();
                move || {
                    let attempts = attempts.clone();
                    async move {
                        attempts.fetch_add(1, Ordering::SeqCst);
                        Json(json!({}))
                    }
                }
            }),
        );
        let upstream_url = spawn_upstream(app).await;
        let config: Config = toml::from_str(&format!(
            r#"
[upstream]
url = "{upstream_url}"
tpm = 1
"#
        ))
        .unwrap();

        let resp = chat_completions(State(test_app_state(config)), chat_request()).await;

        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(attempts.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn chat_completions_should_send_upstream_request_through_configured_http_proxy() {
        let proxy_attempts = Arc::new(AtomicUsize::new(0));
        let proxy_app = Router::new().route(
            "/v1/chat/completions",
            post({
                let proxy_attempts = proxy_attempts.clone();
                move || {
                    let proxy_attempts = proxy_attempts.clone();
                    async move {
                        proxy_attempts.fetch_add(1, Ordering::SeqCst);
                        Json(json!({
                            "id": "chatcmpl_1",
                            "model": "test-model",
                            "choices": [{
                                "index": 0,
                                "message": {"role": "assistant", "content": "ok"},
                                "finish_reason": "stop"
                            }]
                        }))
                    }
                }
            }),
        );
        let proxy_url = spawn_upstream(proxy_app).await;
        let proxy_addr = proxy_url.trim_start_matches("http://");
        let config: Config = toml::from_str(&format!(
            r#"
[[upstreams]]
url = "http://127.0.0.1:9/v1"
proxy = "{proxy_addr}"
proxy_type = "http"
"#
        ))
        .unwrap();
        let req = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "model": "test-model",
                    "messages": [{"role": "user", "content": "hello"}]
                })
                .to_string(),
            ))
            .unwrap();

        let resp = chat_completions(State(test_app_state(config)), req).await;

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(proxy_attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn responses_should_reject_request_when_proxy_auth_key_is_missing() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let app = Router::new().route(
            "/chat/completions",
            post({
                let attempts = attempts.clone();
                move || {
                    let attempts = attempts.clone();
                    async move {
                        attempts.fetch_add(1, Ordering::SeqCst);
                        Json(json!({})).into_response()
                    }
                }
            }),
        );
        let config = test_auth_config(spawn_upstream(app).await);

        let resp = responses(State(config), responses_request(false)).await;

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(attempts.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn responses_should_accept_request_when_proxy_auth_key_matches() {
        let app = Router::new().route(
            "/chat/completions",
            post(|| async {
                Json(json!({
                    "id": "chatcmpl_1",
                    "model": "test-model",
                    "choices": [{
                        "index": 0,
                        "message": {"role": "assistant", "content": "ok"},
                        "finish_reason": "stop"
                    }],
                    "usage": {
                        "prompt_tokens": 1,
                        "completion_tokens": 1,
                        "total_tokens": 2
                    }
                }))
            }),
        );
        let config = test_auth_config(spawn_upstream(app).await);
        let req = Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header("content-type", "application/json")
            .header("authorization", "Bearer proxy-secret")
            .body(Body::from(
                json!({
                    "model": "test-model",
                    "stream": false,
                    "input": "hello"
                })
                .to_string(),
            ))
            .unwrap();

        let resp = responses(State(config), req).await;

        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn chat_completions_should_reject_request_when_proxy_auth_key_is_missing() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let app = Router::new().route(
            "/chat/completions",
            post({
                let attempts = attempts.clone();
                move || {
                    let attempts = attempts.clone();
                    async move {
                        attempts.fetch_add(1, Ordering::SeqCst);
                        Json(json!({})).into_response()
                    }
                }
            }),
        );
        let config = test_auth_config(spawn_upstream(app).await);
        let req = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "model": "test-model",
                    "messages": [{"role": "user", "content": "hello"}]
                })
                .to_string(),
            ))
            .unwrap();

        let resp = chat_completions(State(config), req).await;

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(attempts.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn models_should_reject_request_when_proxy_auth_key_is_missing() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let app = Router::new().route(
            "/models",
            get({
                let attempts = attempts.clone();
                move || {
                    let attempts = attempts.clone();
                    async move {
                        attempts.fetch_add(1, Ordering::SeqCst);
                        Json(json!({})).into_response()
                    }
                }
            }),
        );
        let config = test_auth_config(spawn_upstream(app).await);

        let resp = models(State(config), HeaderMap::new()).await;

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(attempts.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn responses_should_retry_retryable_upstream_status_before_success() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let app = Router::new().route(
            "/chat/completions",
            post({
                let attempts = attempts.clone();
                move || {
                    let attempts = attempts.clone();
                    async move {
                        if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                            return (
                                StatusCode::TOO_MANY_REQUESTS,
                                Json(json!({"error": "rate limited"})),
                            )
                                .into_response();
                        }

                        Json(json!({
                            "id": "chatcmpl_1",
                            "model": "test-model",
                            "choices": [{
                                "index": 0,
                                "message": {"role": "assistant", "content": "ok"},
                                "finish_reason": "stop"
                            }],
                            "usage": {
                                "prompt_tokens": 1,
                                "completion_tokens": 1,
                                "total_tokens": 2
                            }
                        }))
                        .into_response()
                    }
                }
            }),
        );
        let config = test_config(spawn_upstream(app).await);

        let resp = responses(State(config), responses_request(false)).await;

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn responses_should_make_initial_request_then_retry_three_times() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let app = Router::new().route(
            "/chat/completions",
            post({
                let attempts = attempts.clone();
                move || {
                    let attempts = attempts.clone();
                    async move {
                        if attempts.fetch_add(1, Ordering::SeqCst) < 3 {
                            return (
                                StatusCode::BAD_GATEWAY,
                                Json(json!({"error": "temporary failure"})),
                            )
                                .into_response();
                        }

                        Json(json!({
                            "id": "chatcmpl_1",
                            "model": "test-model",
                            "choices": [{
                                "index": 0,
                                "message": {"role": "assistant", "content": "ok"},
                                "finish_reason": "stop"
                            }],
                            "usage": {
                                "prompt_tokens": 1,
                                "completion_tokens": 1,
                                "total_tokens": 2
                            }
                        }))
                        .into_response()
                    }
                }
            }),
        );
        let config = test_config(spawn_upstream(app).await);

        let resp = responses(State(config), responses_request(false)).await;

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(attempts.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn responses_stream_should_convert_split_sse_and_complete_output() {
        let chunks = vec![
            r#"data: {"id":"chunk_1","model":"test-model","choices":[{"index":0,"delta":{"content":"hel"#,
            r#"lo"},"finish_reason":null}]}

data: {"id":"chunk_2","model":"test-model","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}

data: [DONE]

"#,
        ];
        let app = Router::new().route(
            "/chat/completions",
            post(move || {
                let chunks = chunks.clone();
                async move {
                    let stream = futures::stream::iter(
                        chunks
                            .into_iter()
                            .map(|chunk| Ok::<_, Infallible>(Bytes::from(chunk))),
                    );

                    Response::builder()
                        .status(StatusCode::OK)
                        .header("content-type", "text/event-stream")
                        .body(Body::from_stream(stream))
                        .unwrap()
                }
            }),
        );
        let config = test_config(spawn_upstream(app).await);

        let resp = responses(State(config), responses_request(true)).await;
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();

        assert!(text.contains("response.output_text.delta"));
        assert!(text.contains("\"delta\":\"hello\""));
        assert!(text.contains("response.completed"));
        assert!(text.contains("\"output\":[{\"content\""));
        assert!(text.contains("\"text\":\"hello\""));
    }

    #[tokio::test]
    async fn responses_stream_should_complete_output_when_done_arrives_without_finish_reason() {
        let chunks = vec![
            r#"data: {"id":"chunk_1","model":"test-model","choices":[{"index":0,"delta":{"content":"hello"},"finish_reason":null}]}

data: [DONE]

"#,
        ];
        let app = Router::new().route(
            "/chat/completions",
            post(move || {
                let chunks = chunks.clone();
                async move {
                    let stream = futures::stream::iter(
                        chunks
                            .into_iter()
                            .map(|chunk| Ok::<_, Infallible>(Bytes::from(chunk))),
                    );

                    Response::builder()
                        .status(StatusCode::OK)
                        .header("content-type", "text/event-stream")
                        .body(Body::from_stream(stream))
                        .unwrap()
                }
            }),
        );
        let config = test_config(spawn_upstream(app).await);

        let resp = responses(State(config), responses_request(true)).await;
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(bytes.to_vec()).unwrap();

        assert!(text.contains("response.output_item.done"));
        assert!(text.contains("\"output\":[{\"content\""));
        assert!(text.contains("\"text\":\"hello\""));
    }

    #[tokio::test]
    async fn models_should_proxy_upstream_model_list() {
        let app = Router::new().route(
            "/models",
            get(|| async {
                Json(json!({
                    "object": "list",
                    "data": [{
                        "id": "test-model",
                        "object": "model"
                    }]
                }))
            }),
        );
        let config = test_config(spawn_upstream(app).await);

        let resp = models(State(config), HeaderMap::new()).await;
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"][0]["id"], "test-model");
    }

    #[tokio::test]
    async fn models_should_merge_model_lists_from_multiple_upstreams() {
        let upstream_a = Router::new().route(
            "/models",
            get(|| async {
                Json(json!({
                    "object": "list",
                    "data": [{"id": "model-a", "object": "model"}]
                }))
            }),
        );
        let upstream_b = Router::new().route(
            "/models",
            get(|| async {
                Json(json!({
                    "object": "list",
                    "data": [{"id": "model-b", "object": "model"}]
                }))
            }),
        );
        let config = test_multi_config(vec![
            spawn_upstream(upstream_a).await,
            spawn_upstream(upstream_b).await,
        ]);

        let resp = models(State(config), HeaderMap::new()).await;
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let ids: Vec<_> = body["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model["id"].as_str().unwrap())
            .collect();

        assert_eq!(status, StatusCode::OK);
        assert_eq!(ids, vec!["model-a", "model-b"]);
    }

    #[tokio::test]
    async fn models_should_prefix_model_ids_with_upstream_names_when_configured() {
        let upstream_a = Router::new().route(
            "/models",
            get(|| async {
                Json(json!({
                    "object": "list",
                    "data": [{"id": "shared-model", "object": "model"}]
                }))
            }),
        );
        let upstream_b = Router::new().route(
            "/models",
            get(|| async {
                Json(json!({
                    "object": "list",
                    "data": [{"id": "shared-model", "object": "model"}]
                }))
            }),
        );
        let config: Config = toml::from_str(&format!(
            r#"
[[upstreams]]
name = "openai"
url = "{}"

[[upstreams]]
name = "local"
url = "{}"
"#,
            spawn_upstream(upstream_a).await,
            spawn_upstream(upstream_b).await
        ))
        .unwrap();

        let resp = models(State(test_app_state(config)), HeaderMap::new()).await;
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let ids: Vec<_> = body["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model["id"].as_str().unwrap())
            .collect();

        assert_eq!(status, StatusCode::OK);
        assert_eq!(ids, vec!["openai:shared-model", "local:shared-model"]);
    }

    #[tokio::test]
    async fn responses_should_route_prefixed_model_and_strip_prefix_before_upstream() {
        let upstream_a_called = Arc::new(AtomicUsize::new(0));
        let upstream_b_called = Arc::new(AtomicUsize::new(0));
        let upstream_a = Router::new()
            .route(
                "/models",
                get(|| async {
                    Json(json!({
                        "object": "list",
                        "data": [{"id": "shared-model", "object": "model"}]
                    }))
                }),
            )
            .route(
                "/chat/completions",
                post({
                    let upstream_a_called = upstream_a_called.clone();
                    move || {
                        let upstream_a_called = upstream_a_called.clone();
                        async move {
                            upstream_a_called.fetch_add(1, Ordering::SeqCst);
                            Json(json!({"error": "wrong upstream"})).into_response()
                        }
                    }
                }),
            );
        let upstream_b = Router::new()
            .route(
                "/models",
                get(|| async {
                    Json(json!({
                        "object": "list",
                        "data": [{"id": "shared-model", "object": "model"}]
                    }))
                }),
            )
            .route(
                "/chat/completions",
                post({
                    let upstream_b_called = upstream_b_called.clone();
                    move |Json(body): Json<serde_json::Value>| {
                        let upstream_b_called = upstream_b_called.clone();
                        async move {
                            upstream_b_called.fetch_add(1, Ordering::SeqCst);
                            Json(json!({
                                "id": "chatcmpl_b",
                                "model": body["model"],
                                "choices": [{
                                    "index": 0,
                                    "message": {"role": "assistant", "content": "from local"},
                                    "finish_reason": "stop"
                                }]
                            }))
                            .into_response()
                        }
                    }
                }),
            );
        let config: Config = toml::from_str(&format!(
            r#"
[[upstreams]]
name = "openai"
url = "{}"

[[upstreams]]
name = "local"
url = "{}"
"#,
            spawn_upstream(upstream_a).await,
            spawn_upstream(upstream_b).await
        ))
        .unwrap();
        let req = Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "model": "local:shared-model",
                    "input": "hello"
                })
                .to_string(),
            ))
            .unwrap();

        let resp = responses(State(test_app_state(config)), req).await;
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(status, StatusCode::OK);
        assert_eq!(upstream_a_called.load(Ordering::SeqCst), 0);
        assert_eq!(upstream_b_called.load(Ordering::SeqCst), 1);
        assert_eq!(body["model"], "shared-model");
        assert_eq!(body["output"][0]["content"][0]["text"], "from local");
    }

    #[tokio::test]
    async fn responses_should_route_to_upstream_that_lists_requested_model() {
        let upstream_a_called = Arc::new(AtomicUsize::new(0));
        let upstream_b_called = Arc::new(AtomicUsize::new(0));
        let upstream_a = Router::new()
            .route(
                "/models",
                get(|| async {
                    Json(json!({
                        "object": "list",
                        "data": [{"id": "model-a", "object": "model"}]
                    }))
                }),
            )
            .route(
                "/chat/completions",
                post({
                    let upstream_a_called = upstream_a_called.clone();
                    move || {
                        let upstream_a_called = upstream_a_called.clone();
                        async move {
                            upstream_a_called.fetch_add(1, Ordering::SeqCst);
                            Json(json!({"error": "wrong upstream"})).into_response()
                        }
                    }
                }),
            );
        let upstream_b = Router::new()
            .route(
                "/models",
                get(|| async {
                    Json(json!({
                        "object": "list",
                        "data": [{"id": "model-b", "object": "model"}]
                    }))
                }),
            )
            .route(
                "/chat/completions",
                post({
                    let upstream_b_called = upstream_b_called.clone();
                    move || {
                        let upstream_b_called = upstream_b_called.clone();
                        async move {
                            upstream_b_called.fetch_add(1, Ordering::SeqCst);
                            Json(json!({
                                "id": "chatcmpl_b",
                                "model": "model-b",
                                "choices": [{
                                    "index": 0,
                                    "message": {"role": "assistant", "content": "from b"},
                                    "finish_reason": "stop"
                                }]
                            }))
                            .into_response()
                        }
                    }
                }),
            );
        let config = test_multi_config(vec![
            spawn_upstream(upstream_a).await,
            spawn_upstream(upstream_b).await,
        ]);
        let req = Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "model": "model-b",
                    "input": "hello"
                })
                .to_string(),
            ))
            .unwrap();

        let resp = responses(State(config), req).await;
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(status, StatusCode::OK);
        assert_eq!(upstream_a_called.load(Ordering::SeqCst), 0);
        assert_eq!(upstream_b_called.load(Ordering::SeqCst), 1);
        assert_eq!(body["output"][0]["content"][0]["text"], "from b");
    }

    #[tokio::test]
    async fn responses_should_reuse_cached_model_lists_for_repeated_plain_model_routing() {
        let upstream_a_model_calls = Arc::new(AtomicUsize::new(0));
        let upstream_b_model_calls = Arc::new(AtomicUsize::new(0));
        let upstream_b_called = Arc::new(AtomicUsize::new(0));
        let upstream_a = Router::new().route(
            "/models",
            get({
                let upstream_a_model_calls = upstream_a_model_calls.clone();
                move || {
                    let upstream_a_model_calls = upstream_a_model_calls.clone();
                    async move {
                        upstream_a_model_calls.fetch_add(1, Ordering::SeqCst);
                        Json(json!({
                            "object": "list",
                            "data": [{"id": "model-a", "object": "model"}]
                        }))
                    }
                }
            }),
        );
        let upstream_b = Router::new()
            .route(
                "/models",
                get({
                    let upstream_b_model_calls = upstream_b_model_calls.clone();
                    move || {
                        let upstream_b_model_calls = upstream_b_model_calls.clone();
                        async move {
                            upstream_b_model_calls.fetch_add(1, Ordering::SeqCst);
                            Json(json!({
                                "object": "list",
                                "data": [{"id": "model-b", "object": "model"}]
                            }))
                        }
                    }
                }),
            )
            .route(
                "/chat/completions",
                post({
                    let upstream_b_called = upstream_b_called.clone();
                    move || {
                        let upstream_b_called = upstream_b_called.clone();
                        async move {
                            upstream_b_called.fetch_add(1, Ordering::SeqCst);
                            Json(json!({
                                "id": "chatcmpl_b",
                                "model": "model-b",
                                "choices": [{
                                    "index": 0,
                                    "message": {"role": "assistant", "content": "from b"},
                                    "finish_reason": "stop"
                                }]
                            }))
                        }
                    }
                }),
            );
        let config = test_multi_config(vec![
            spawn_upstream(upstream_a).await,
            spawn_upstream(upstream_b).await,
        ]);

        let first = responses(State(config.clone()), responses_model_request("model-b")).await;
        let second = responses(State(config), responses_model_request("model-b")).await;

        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(second.status(), StatusCode::OK);
        assert_eq!(upstream_a_model_calls.load(Ordering::SeqCst), 1);
        assert_eq!(upstream_b_model_calls.load(Ordering::SeqCst), 1);
        assert_eq!(upstream_b_called.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn startup_should_prefetch_model_lists_before_first_routed_request() {
        let upstream_a_model_calls = Arc::new(AtomicUsize::new(0));
        let upstream_b_model_calls = Arc::new(AtomicUsize::new(0));
        let upstream_b_seen_model = Arc::new(Mutex::new(None));
        let upstream_a = Router::new().route(
            "/models",
            get({
                let upstream_a_model_calls = upstream_a_model_calls.clone();
                move || {
                    let upstream_a_model_calls = upstream_a_model_calls.clone();
                    async move {
                        upstream_a_model_calls.fetch_add(1, Ordering::SeqCst);
                        Json(json!({
                            "object": "list",
                            "data": [{"id": "model-a", "object": "model"}]
                        }))
                    }
                }
            }),
        );
        let upstream_b = Router::new()
            .route(
                "/models",
                get({
                    let upstream_b_model_calls = upstream_b_model_calls.clone();
                    move || {
                        let upstream_b_model_calls = upstream_b_model_calls.clone();
                        async move {
                            upstream_b_model_calls.fetch_add(1, Ordering::SeqCst);
                            Json(json!({
                                "object": "list",
                                "data": [{"id": "model-b", "object": "model"}]
                            }))
                        }
                    }
                }),
            )
            .route(
                "/chat/completions",
                post({
                    let upstream_b_seen_model = upstream_b_seen_model.clone();
                    move |Json(body): Json<serde_json::Value>| {
                        let upstream_b_seen_model = upstream_b_seen_model.clone();
                        async move {
                            *upstream_b_seen_model.lock().await = body
                                .get("model")
                                .and_then(|model| model.as_str())
                                .map(str::to_string);
                            Json(json!({
                                "id": "chatcmpl_b",
                                "model": "model-b",
                                "choices": [{
                                    "index": 0,
                                    "message": {"role": "assistant", "content": "from b"},
                                    "finish_reason": "stop"
                                }]
                            }))
                        }
                    }
                }),
            );
        let state = test_multi_config(vec![
            spawn_upstream(upstream_a).await,
            spawn_upstream(upstream_b).await,
        ]);

        for _ in 0..20 {
            if upstream_a_model_calls.load(Ordering::SeqCst) == 1
                && upstream_b_model_calls.load(Ordering::SeqCst) == 1
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        assert_eq!(upstream_a_model_calls.load(Ordering::SeqCst), 1);
        assert_eq!(upstream_b_model_calls.load(Ordering::SeqCst), 1);

        let resp = chat_completions(State(state), chat_model_request("model-b")).await;

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(upstream_a_model_calls.load(Ordering::SeqCst), 1);
        assert_eq!(upstream_b_model_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            upstream_b_seen_model.lock().await.as_deref(),
            Some("model-b")
        );
    }

    #[tokio::test]
    async fn chat_completions_should_round_robin_between_upstreams_that_expose_the_same_model() {
        let upstream_a_called = Arc::new(AtomicUsize::new(0));
        let upstream_b_called = Arc::new(AtomicUsize::new(0));
        let upstream_a = Router::new()
            .route(
                "/models",
                get(|| async {
                    Json(json!({
                        "object": "list",
                        "data": [{"id": "shared-model", "object": "model"}]
                    }))
                }),
            )
            .route(
                "/chat/completions",
                post({
                    let upstream_a_called = upstream_a_called.clone();
                    move || {
                        let upstream_a_called = upstream_a_called.clone();
                        async move {
                            upstream_a_called.fetch_add(1, Ordering::SeqCst);
                            Json(json!({
                                "id": "chatcmpl_a",
                                "model": "shared-model",
                                "choices": [{
                                    "index": 0,
                                    "message": {"role": "assistant", "content": "from a"},
                                    "finish_reason": "stop"
                                }]
                            }))
                        }
                    }
                }),
            );
        let upstream_b = Router::new()
            .route(
                "/models",
                get(|| async {
                    Json(json!({
                        "object": "list",
                        "data": [{"id": "shared-model", "object": "model"}]
                    }))
                }),
            )
            .route(
                "/chat/completions",
                post({
                    let upstream_b_called = upstream_b_called.clone();
                    move || {
                        let upstream_b_called = upstream_b_called.clone();
                        async move {
                            upstream_b_called.fetch_add(1, Ordering::SeqCst);
                            Json(json!({
                                "id": "chatcmpl_b",
                                "model": "shared-model",
                                "choices": [{
                                    "index": 0,
                                    "message": {"role": "assistant", "content": "from b"},
                                    "finish_reason": "stop"
                                }]
                            }))
                        }
                    }
                }),
            );
        let upstream_a_url = spawn_upstream(upstream_a).await;
        let upstream_b_url = spawn_upstream(upstream_b).await;
        let config: Config = toml::from_str(&format!(
            r#"
[routing]
load_balance = "round_robin"

[[upstreams]]
url = "{upstream_a_url}"

[[upstreams]]
url = "{upstream_b_url}"
"#
        ))
        .unwrap();
        let state = test_app_state(config);

        let first =
            chat_completions(State(state.clone()), chat_model_request("shared-model")).await;
        let second = chat_completions(State(state), chat_model_request("shared-model")).await;

        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(second.status(), StatusCode::OK);
        assert_eq!(upstream_a_called.load(Ordering::SeqCst), 1);
        assert_eq!(upstream_b_called.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn chat_completions_should_fail_over_to_next_matching_upstream_when_enabled() {
        let upstream_a_called = Arc::new(AtomicUsize::new(0));
        let upstream_b_called = Arc::new(AtomicUsize::new(0));
        let upstream_a = Router::new()
            .route(
                "/models",
                get(|| async {
                    Json(json!({
                        "object": "list",
                        "data": [{"id": "shared-model", "object": "model"}]
                    }))
                }),
            )
            .route(
                "/chat/completions",
                post({
                    let upstream_a_called = upstream_a_called.clone();
                    move || {
                        let upstream_a_called = upstream_a_called.clone();
                        async move {
                            upstream_a_called.fetch_add(1, Ordering::SeqCst);
                            (
                                StatusCode::BAD_GATEWAY,
                                Json(json!({"error": "temporary failure"})),
                            )
                        }
                    }
                }),
            );
        let upstream_b = Router::new()
            .route(
                "/models",
                get(|| async {
                    Json(json!({
                        "object": "list",
                        "data": [{"id": "shared-model", "object": "model"}]
                    }))
                }),
            )
            .route(
                "/chat/completions",
                post({
                    let upstream_b_called = upstream_b_called.clone();
                    move || {
                        let upstream_b_called = upstream_b_called.clone();
                        async move {
                            upstream_b_called.fetch_add(1, Ordering::SeqCst);
                            Json(json!({
                                "id": "chatcmpl_b",
                                "model": "shared-model",
                                "choices": [{
                                    "index": 0,
                                    "message": {"role": "assistant", "content": "from b"},
                                    "finish_reason": "stop"
                                }]
                            }))
                            .into_response()
                        }
                    }
                }),
            );
        let upstream_a_url = spawn_upstream(upstream_a).await;
        let upstream_b_url = spawn_upstream(upstream_b).await;
        let config: Config = toml::from_str(&format!(
            r#"
[routing]
automatic_failover = true

[[upstreams]]
url = "{upstream_a_url}"

[[upstreams]]
url = "{upstream_b_url}"
"#
        ))
        .unwrap();

        let resp = chat_completions(
            State(test_app_state(config)),
            chat_model_request("shared-model"),
        )
        .await;

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(upstream_a_called.load(Ordering::SeqCst), 4);
        assert_eq!(upstream_b_called.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn responses_should_fail_over_to_next_matching_upstream_when_enabled() {
        let upstream_a_called = Arc::new(AtomicUsize::new(0));
        let upstream_b_called = Arc::new(AtomicUsize::new(0));
        let upstream_a = Router::new()
            .route(
                "/models",
                get(|| async {
                    Json(json!({
                        "object": "list",
                        "data": [{"id": "test-model", "object": "model"}]
                    }))
                }),
            )
            .route(
                "/chat/completions",
                post({
                    let upstream_a_called = upstream_a_called.clone();
                    move || {
                        let upstream_a_called = upstream_a_called.clone();
                        async move {
                            upstream_a_called.fetch_add(1, Ordering::SeqCst);
                            (
                                StatusCode::BAD_GATEWAY,
                                Json(json!({"error": "temporary failure"})),
                            )
                        }
                    }
                }),
            );
        let upstream_b = Router::new()
            .route(
                "/models",
                get(|| async {
                    Json(json!({
                        "object": "list",
                        "data": [{"id": "test-model", "object": "model"}]
                    }))
                }),
            )
            .route(
                "/chat/completions",
                post({
                    let upstream_b_called = upstream_b_called.clone();
                    move || {
                        let upstream_b_called = upstream_b_called.clone();
                        async move {
                            upstream_b_called.fetch_add(1, Ordering::SeqCst);
                            Json(json!({
                                "id": "chatcmpl_b",
                                "model": "test-model",
                                "choices": [{
                                    "index": 0,
                                    "message": {"role": "assistant", "content": "from b"},
                                    "finish_reason": "stop"
                                }]
                            }))
                            .into_response()
                        }
                    }
                }),
            );
        let upstream_a_url = spawn_upstream(upstream_a).await;
        let upstream_b_url = spawn_upstream(upstream_b).await;
        let config: Config = toml::from_str(&format!(
            r#"
[routing]
automatic_failover = true

[[upstreams]]
url = "{upstream_a_url}"

[[upstreams]]
url = "{upstream_b_url}"
"#
        ))
        .unwrap();

        let resp = responses(State(test_app_state(config)), responses_request(false)).await;
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(status, StatusCode::OK);
        assert_eq!(upstream_a_called.load(Ordering::SeqCst), 4);
        assert_eq!(upstream_b_called.load(Ordering::SeqCst), 1);
        assert_eq!(body["output"][0]["content"][0]["text"], "from b");
    }

    #[tokio::test]
    async fn chat_completions_should_skip_unhealthy_upstreams_when_health_check_is_enabled() {
        let upstream_a_called = Arc::new(AtomicUsize::new(0));
        let upstream_b_called = Arc::new(AtomicUsize::new(0));
        let upstream_a = Router::new()
            .route(
                "/models",
                get(|| async { (StatusCode::BAD_GATEWAY, Json(json!({"error": "unhealthy"}))) }),
            )
            .route(
                "/chat/completions",
                post({
                    let upstream_a_called = upstream_a_called.clone();
                    move || {
                        let upstream_a_called = upstream_a_called.clone();
                        async move {
                            upstream_a_called.fetch_add(1, Ordering::SeqCst);
                            Json(json!({"error": "wrong upstream"})).into_response()
                        }
                    }
                }),
            );
        let upstream_b = Router::new()
            .route(
                "/models",
                get(|| async {
                    Json(json!({
                        "object": "list",
                        "data": [{"id": "test-model", "object": "model"}]
                    }))
                }),
            )
            .route(
                "/chat/completions",
                post({
                    let upstream_b_called = upstream_b_called.clone();
                    move || {
                        let upstream_b_called = upstream_b_called.clone();
                        async move {
                            upstream_b_called.fetch_add(1, Ordering::SeqCst);
                            Json(json!({
                                "id": "chatcmpl_b",
                                "model": "test-model",
                                "choices": [{
                                    "index": 0,
                                    "message": {"role": "assistant", "content": "from b"},
                                    "finish_reason": "stop"
                                }]
                            }))
                            .into_response()
                        }
                    }
                }),
            );
        let upstream_a_url = spawn_upstream(upstream_a).await;
        let upstream_b_url = spawn_upstream(upstream_b).await;
        let config: Config = toml::from_str(&format!(
            r#"
[health]
enabled = true
interval_millis = 25
unhealthy_after_failures = 1

[[upstreams]]
url = "{upstream_a_url}"

[[upstreams]]
url = "{upstream_b_url}"
"#
        ))
        .unwrap();
        let state = test_app_state(config);
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;

        let resp = chat_completions(State(state), chat_request_without_model()).await;

        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(upstream_a_called.load(Ordering::SeqCst), 0);
        assert_eq!(upstream_b_called.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn models_should_send_bearer_authorization_when_using_openai_auth_header() {
        let app = Router::new().route(
            "/models",
            get(|headers: axum::http::HeaderMap| async move {
                Json(json!({
                    "authorization": headers
                        .get("authorization")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("")
                }))
            }),
        );
        let config = test_app_state(Config {
            port: 0,
            auth_key: None,
            performance: crate::config::PerformanceConfig::default(),
            routing: crate::config::RoutingConfig::default(),
            health: crate::config::HealthConfig::default(),
            upstream: Some(crate::config::UpstreamConfig {
                name: None,
                url: spawn_upstream(app).await,
                api_key: "test-key".into(),
                auth_header: "Authorization".into(),
                proxy: None,
                proxy_type: crate::config::UpstreamProxyType::Http,
                max_concurrent_requests: None,
                rpm: None,
                tpm: None,
            }),
            upstreams: Vec::new(),
        });

        let resp = models(State(config), HeaderMap::new()).await;
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(body["authorization"], "Bearer test-key");
    }

    #[tokio::test]
    async fn models_should_not_duplicate_existing_bearer_authorization_prefix() {
        let app = Router::new().route(
            "/models",
            get(|headers: axum::http::HeaderMap| async move {
                Json(json!({
                    "authorization": headers
                        .get("authorization")
                        .and_then(|value| value.to_str().ok())
                        .unwrap_or("")
                }))
            }),
        );
        let config = test_app_state(Config {
            port: 0,
            auth_key: None,
            performance: crate::config::PerformanceConfig::default(),
            routing: crate::config::RoutingConfig::default(),
            health: crate::config::HealthConfig::default(),
            upstream: Some(crate::config::UpstreamConfig {
                name: None,
                url: spawn_upstream(app).await,
                api_key: "Bearer test-key".into(),
                auth_header: "Authorization".into(),
                proxy: None,
                proxy_type: crate::config::UpstreamProxyType::Http,
                max_concurrent_requests: None,
                rpm: None,
                tpm: None,
            }),
            upstreams: Vec::new(),
        });

        let resp = models(State(config), HeaderMap::new()).await;
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(body["authorization"], "Bearer test-key");
    }

    #[tokio::test]
    async fn models_should_make_initial_request_then_retry_three_times_and_return_final_upstream_error_body()
     {
        let attempts = Arc::new(AtomicUsize::new(0));
        let app = Router::new().route(
            "/models",
            get({
                let attempts = attempts.clone();
                move || {
                    let attempts = attempts.clone();
                    async move {
                        attempts.fetch_add(1, Ordering::SeqCst);
                        (
                            StatusCode::BAD_GATEWAY,
                            Json(json!({"error": "upstream failed"})),
                        )
                            .into_response()
                    }
                }
            }),
        );
        let config = test_config(spawn_upstream(app).await);

        let resp = models(State(config), HeaderMap::new()).await;
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();

        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(attempts.load(Ordering::SeqCst), 4);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
            json!({"error": "upstream failed"})
        );
    }
}
