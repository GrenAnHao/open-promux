use axum::{
    Router,
    routing::{get, post},
};
use std::{net::SocketAddr, path::PathBuf, sync::Arc};
use tracing_subscriber::EnvFilter;

mod config;
mod convert;
mod proxy;
mod types;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let config_path: PathBuf = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "config.toml".into())
        .into();

    tracing::info!("loading config from {}", config_path.display());
    let config = config::Config::load(&config_path);
    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    let state = Arc::new(proxy::AppState::new(config));

    let app = Router::new()
        .route("/v1/models", get(proxy::models))
        .route("/v1/chat/completions", post(proxy::chat_completions))
        .route("/v1/responses", post(proxy::responses))
        .with_state(state);

    tracing::info!("openproxy listening on {addr}");

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}
