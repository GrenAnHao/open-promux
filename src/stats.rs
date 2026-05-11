//! Traffic statistics: per-upstream and per-(upstream, model) counters
//! collected at request boundaries.
//!
//! Counters are atomic so the hot path (incrementing on each request) is
//! lock-free; reading a snapshot only blocks while we copy the BTreeMap of
//! buckets. Designed to be read by the desktop UI's Stats page on a
//! 1–2 second poll.

use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// Top-level statistics container. Lives inside `AppState`.
pub struct TrafficStats {
    started_at: Instant,
    global: AtomicCounters,
    upstreams: RwLock<BTreeMap<String, AtomicCounters>>,
    models: RwLock<BTreeMap<ModelKey, AtomicCounters>>,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct ModelKey {
    upstream: String,
    model: String,
}

#[derive(Default)]
struct AtomicCounters {
    requests_total: AtomicU64,
    requests_success: AtomicU64,
    requests_error: AtomicU64,
    bytes_in: AtomicU64,
    bytes_out: AtomicU64,
    latency_ms_sum: AtomicU64,
    latency_ms_count: AtomicU64,
    latency_ms_max: AtomicU64,
}

impl AtomicCounters {
    fn record(&self, ok: bool, bytes_in: u64, bytes_out: u64, latency_ms: u64) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
        if ok {
            self.requests_success.fetch_add(1, Ordering::Relaxed);
        } else {
            self.requests_error.fetch_add(1, Ordering::Relaxed);
        }
        self.bytes_in.fetch_add(bytes_in, Ordering::Relaxed);
        self.bytes_out.fetch_add(bytes_out, Ordering::Relaxed);
        self.latency_ms_sum.fetch_add(latency_ms, Ordering::Relaxed);
        self.latency_ms_count.fetch_add(1, Ordering::Relaxed);
        // Atomic max: spin-CAS on the latency_ms_max field.
        let mut current = self.latency_ms_max.load(Ordering::Relaxed);
        while latency_ms > current {
            match self.latency_ms_max.compare_exchange_weak(
                current,
                latency_ms,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
    }

    fn snapshot(&self) -> CounterSnapshot {
        let total = self.requests_total.load(Ordering::Relaxed);
        let success = self.requests_success.load(Ordering::Relaxed);
        let error = self.requests_error.load(Ordering::Relaxed);
        let bytes_in = self.bytes_in.load(Ordering::Relaxed);
        let bytes_out = self.bytes_out.load(Ordering::Relaxed);
        let latency_sum = self.latency_ms_sum.load(Ordering::Relaxed);
        let latency_count = self.latency_ms_count.load(Ordering::Relaxed);
        let latency_max = self.latency_ms_max.load(Ordering::Relaxed);
        let latency_avg = if latency_count > 0 {
            latency_sum / latency_count
        } else {
            0
        };
        CounterSnapshot {
            requests_total: total,
            requests_success: success,
            requests_error: error,
            bytes_in,
            bytes_out,
            latency_ms_avg: latency_avg,
            latency_ms_max: latency_max,
        }
    }

    fn reset(&self) {
        self.requests_total.store(0, Ordering::Relaxed);
        self.requests_success.store(0, Ordering::Relaxed);
        self.requests_error.store(0, Ordering::Relaxed);
        self.bytes_in.store(0, Ordering::Relaxed);
        self.bytes_out.store(0, Ordering::Relaxed);
        self.latency_ms_sum.store(0, Ordering::Relaxed);
        self.latency_ms_count.store(0, Ordering::Relaxed);
        self.latency_ms_max.store(0, Ordering::Relaxed);
    }
}

impl Default for TrafficStats {
    fn default() -> Self {
        Self::new()
    }
}

impl TrafficStats {
    pub fn new() -> Self {
        Self {
            started_at: Instant::now(),
            global: AtomicCounters::default(),
            upstreams: RwLock::new(BTreeMap::new()),
            models: RwLock::new(BTreeMap::new()),
        }
    }

    /// Record a single request. Cheap: only takes the upstream / model
    /// `RwLock`s in write mode the very first time a new key is observed;
    /// subsequent calls follow a fast read-lock + atomic-add path.
    pub async fn record(
        &self,
        upstream_name: &str,
        model: &str,
        ok: bool,
        bytes_in: u64,
        bytes_out: u64,
        latency_ms: u64,
    ) {
        self.global.record(ok, bytes_in, bytes_out, latency_ms);

        // Per-upstream bucket
        {
            let upstreams = self.upstreams.read().await;
            if let Some(bucket) = upstreams.get(upstream_name) {
                bucket.record(ok, bytes_in, bytes_out, latency_ms);
            } else {
                drop(upstreams);
                let mut upstreams = self.upstreams.write().await;
                let bucket = upstreams
                    .entry(upstream_name.to_string())
                    .or_insert_with(AtomicCounters::default);
                bucket.record(ok, bytes_in, bytes_out, latency_ms);
            }
        }

        // Per-(upstream, model) bucket
        let key = ModelKey {
            upstream: upstream_name.to_string(),
            model: model.to_string(),
        };
        {
            let models = self.models.read().await;
            if let Some(bucket) = models.get(&key) {
                bucket.record(ok, bytes_in, bytes_out, latency_ms);
            } else {
                drop(models);
                let mut models = self.models.write().await;
                let bucket = models.entry(key).or_insert_with(AtomicCounters::default);
                bucket.record(ok, bytes_in, bytes_out, latency_ms);
            }
        }
    }

    /// Take a JSON-serialisable snapshot. Reads the upstream / model maps
    /// under their RwLocks but does not block the hot path (writers only
    /// take the write lock for first-time bucket creation).
    pub async fn snapshot(&self) -> TrafficSnapshot {
        let upstreams_guard = self.upstreams.read().await;
        let upstreams = upstreams_guard
            .iter()
            .map(|(name, bucket)| UpstreamCounters {
                upstream: name.clone(),
                counters: bucket.snapshot(),
            })
            .collect();
        drop(upstreams_guard);

        let models_guard = self.models.read().await;
        let models = models_guard
            .iter()
            .map(|(key, bucket)| ModelCounters {
                upstream: key.upstream.clone(),
                model: key.model.clone(),
                counters: bucket.snapshot(),
            })
            .collect();
        drop(models_guard);

        TrafficSnapshot {
            uptime_seconds: self.started_at.elapsed().as_secs(),
            global: self.global.snapshot(),
            upstreams,
            models,
        }
    }

    /// Wipe all counters. Per-upstream / per-model bucket map structure is
    /// kept (empty buckets), so the desktop UI doesn't lose its column
    /// headers immediately after a clear.
    pub async fn clear(&self) {
        self.global.reset();
        for bucket in self.upstreams.read().await.values() {
            bucket.reset();
        }
        for bucket in self.models.read().await.values() {
            bucket.reset();
        }
    }
}

/// Convenience for proxy endpoints: record a request only if a stats
/// instance is reachable.
pub async fn record_to(
    stats: &Arc<TrafficStats>,
    upstream_name: &str,
    model: &str,
    ok: bool,
    bytes_in: u64,
    bytes_out: u64,
    latency_ms: u64,
) {
    stats
        .record(upstream_name, model, ok, bytes_in, bytes_out, latency_ms)
        .await;
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CounterSnapshot {
    pub requests_total: u64,
    pub requests_success: u64,
    pub requests_error: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub latency_ms_avg: u64,
    pub latency_ms_max: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamCounters {
    pub upstream: String,
    pub counters: CounterSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCounters {
    pub upstream: String,
    pub model: String,
    pub counters: CounterSnapshot,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrafficSnapshot {
    /// Uptime since the stats container was created (= server start).
    pub uptime_seconds: u64,
    pub global: CounterSnapshot,
    pub upstreams: Vec<UpstreamCounters>,
    pub models: Vec<ModelCounters>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn records_and_aggregates_per_upstream_and_per_model() {
        let stats = TrafficStats::new();
        stats.record("a", "m1", true, 100, 200, 30).await;
        stats.record("a", "m1", true, 50, 80, 10).await;
        stats.record("a", "m2", false, 5, 0, 50).await;
        stats.record("b", "m1", true, 10, 20, 5).await;

        let snap = stats.snapshot().await;
        assert_eq!(snap.global.requests_total, 4);
        assert_eq!(snap.global.requests_success, 3);
        assert_eq!(snap.global.requests_error, 1);
        assert_eq!(snap.global.bytes_in, 165);
        assert_eq!(snap.global.bytes_out, 300);
        assert_eq!(snap.global.latency_ms_max, 50);

        assert_eq!(snap.upstreams.len(), 2);
        let a = &snap
            .upstreams
            .iter()
            .find(|u| u.upstream == "a")
            .unwrap()
            .counters;
        assert_eq!(a.requests_total, 3);
        assert_eq!(a.requests_error, 1);

        assert_eq!(snap.models.len(), 3);
        let am1 = &snap
            .models
            .iter()
            .find(|m| m.upstream == "a" && m.model == "m1")
            .unwrap()
            .counters;
        assert_eq!(am1.requests_total, 2);
        assert_eq!(am1.bytes_in, 150);
    }

    #[tokio::test]
    async fn clear_resets_counters_but_keeps_buckets() {
        let stats = TrafficStats::new();
        stats.record("a", "m1", true, 1, 1, 1).await;
        stats.clear().await;
        let snap = stats.snapshot().await;
        assert_eq!(snap.global.requests_total, 0);
        // Buckets stay so the UI keeps its rows.
        assert_eq!(snap.upstreams.len(), 1);
        assert_eq!(snap.upstreams[0].counters.requests_total, 0);
    }
}
