//! open-promux core library
//!
//! Exposes the proxy router builder, application state, configuration types,
//! and a controllable [`ServerHandle`] for embedding the proxy inside other
//! processes (e.g. the Tauri desktop UI).
//!
//! The CLI binary (`src/main.rs`) is a thin wrapper around [`run_cli`].

mod config;
mod convert;
pub mod logs;
mod proxy;
mod runtime;
pub mod stats;
mod types;

pub use config::{
    Config, HealthConfig, LoadBalanceStrategy, PerformanceConfig, RectifierConfig, RoutingConfig,
    UpstreamApiFormat, UpstreamConfig, UpstreamProxyConfig, UpstreamProxyType,
};
pub use logs::{LogBus, LogBusLayer, LogLine};
pub use proxy::AppState;
pub use runtime::{ServerHandle, ServerStartError, ServerStartInfo, build_router, run_cli, serve};
pub use stats::{CounterSnapshot, ModelCounters, TrafficSnapshot, TrafficStats, UpstreamCounters};
