use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use futures::StreamExt;
use reqwest::{Client, Proxy, RequestBuilder};
use serde::Serialize;
use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use crate::config::{Config, LoadBalanceStrategy, UpstreamApiFormat, UpstreamConfig};
use crate::convert;
use crate::stats::TrafficStats;
use crate::types::*;

const MAX_RETRIES: usize = 3;
const MODEL_CACHE_TTL: Duration = Duration::from_secs(300);
const ANSI_RED_BOLD: &str = "\x1b[1;31m";
const ANSI_RESET: &str = "\x1b[0m";

pub struct AppState {
    config: Config,
    upstreams: Vec<Arc<UpstreamState>>,
    next_upstream: AtomicUsize,
    global_request_limiter: Option<FixedWindowRateLimiter>,
    global_token_limiter: Option<FixedWindowRateLimiter>,
    traffic_stats: Arc<TrafficStats>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpstreamHealthSnapshot {
    pub index: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub url: String,
    pub api_format: UpstreamApiFormat,
    pub checked: bool,
    pub healthy: bool,
    pub failures: u64,
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
    snapshot: ModelItemsSnapshot,
    expires_at: Instant,
}

#[derive(Clone)]
struct ModelItemsSnapshot {
    items: Arc<[serde_json::Value]>,
    ids: Arc<HashSet<String>>,
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
    checked: bool,
    healthy: bool,
    failures: u64,
}

#[derive(Clone, Copy)]
struct UpstreamHealthUpdate {
    first_check: bool,
    previous_healthy: bool,
    current_healthy: bool,
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
            traffic_stats: Arc::new(TrafficStats::new()),
        }
    }

    /// Cloneable handle to the in-process traffic statistics. Lives next
    /// to the running server and is wiped only when the server restarts
    /// or [`TrafficStats::clear`] is called explicitly.
    pub fn traffic_stats(&self) -> Arc<TrafficStats> {
        self.traffic_stats.clone()
    }

    pub async fn upstream_health_snapshot(&self) -> Vec<UpstreamHealthSnapshot> {
        let mut snapshot = Vec::with_capacity(self.upstreams.len());
        for (index, upstream) in self.upstreams.iter().enumerate() {
            let health = upstream.health.read().await;
            snapshot.push(UpstreamHealthSnapshot {
                index,
                name: upstream.config.name.clone(),
                url: upstream.config.url.clone(),
                api_format: upstream.config.api_format,
                checked: health.checked,
                healthy: health.healthy,
                failures: health.failures,
            });
        }
        snapshot
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
        if let Some(limiter) = self.global_request_limiter.as_ref()
            && !limiter.try_acquire(1).await
        {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        if let Some(limiter) = self.global_token_limiter.as_ref()
            && !limiter.try_acquire(tokens).await
        {
            return Err(StatusCode::TOO_MANY_REQUESTS);
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
                checked: false,
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

    async fn fresh_model_items(&self) -> Option<ModelItemsSnapshot> {
        let cache = self.model_cache.read().await;
        let cached = cache.as_ref()?;
        if Instant::now() < cached.expires_at {
            Some(cached.snapshot.clone())
        } else {
            None
        }
    }

    async fn check_rate_limits(&self, tokens: u64) -> Result<(), StatusCode> {
        if let Some(limiter) = self.request_limiter.as_ref()
            && !limiter.try_acquire(1).await
        {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        if let Some(limiter) = self.token_limiter.as_ref()
            && !limiter.try_acquire(tokens).await
        {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        Ok(())
    }

    async fn is_healthy(&self) -> bool {
        self.health.read().await.healthy
    }

    async fn set_health_check_result(
        &self,
        healthy: bool,
        unhealthy_after_failures: u64,
    ) -> UpstreamHealthUpdate {
        let mut health = self.health.write().await;
        let first_check = !health.checked;
        let previous_healthy = health.healthy;
        health.checked = true;
        if healthy {
            health.healthy = true;
            health.failures = 0;
        } else {
            health.failures = health.failures.saturating_add(1);
            if health.failures >= unhealthy_after_failures {
                health.healthy = false;
            }
        }
        UpstreamHealthUpdate {
            first_check,
            previous_healthy,
            current_healthy: health.healthy,
            failures: health.failures,
        }
    }

    async fn stale_model_items(&self) -> Option<ModelItemsSnapshot> {
        self.model_cache
            .read()
            .await
            .as_ref()
            .map(|cached| cached.snapshot.clone())
    }

    async fn store_model_items(&self, items: Vec<serde_json::Value>) -> ModelItemsSnapshot {
        let snapshot = ModelItemsSnapshot::new(items);
        *self.model_cache.write().await = Some(CachedModels {
            snapshot: snapshot.clone(),
            expires_at: Instant::now() + MODEL_CACHE_TTL,
        });
        snapshot
    }
}

impl ModelItemsSnapshot {
    fn new(items: Vec<serde_json::Value>) -> Self {
        let ids = items
            .iter()
            .filter_map(|item| {
                item.get("id")
                    .and_then(|id| id.as_str())
                    .map(ToString::to_string)
            })
            .collect();
        Self {
            items: Arc::from(items),
            ids: Arc::new(ids),
        }
    }

    fn len(&self) -> usize {
        self.items.len()
    }

    fn contains_model(&self, model: &str) -> bool {
        self.ids.contains(model)
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

mod chat;
mod messages;
mod models;
mod rectifier;
mod responses;
mod stream_bridge;
mod support;

pub use chat::chat_completions;
pub use messages::messages;
pub use models::models;
use rectifier::*;
pub use responses::responses;
use stream_bridge::*;
use support::*;

#[cfg(test)]
mod tests;
