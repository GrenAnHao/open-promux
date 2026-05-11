//! Runtime helpers for embedding the proxy server.
//!
//! Provides:
//! - [`build_router`]: constructs the axum router for an [`AppState`].
//! - [`serve`]: convenience for running the proxy on a `SocketAddr`.
//! - [`ServerHandle`]: a controllable handle that exposes the bound port,
//!   the start time, and a graceful-shutdown trigger. The desktop UI uses
//!   this to start/stop the embedded proxy without restarting the process.
//! - [`run_cli`]: CLI entrypoint reused by `src/main.rs`.

use std::{
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::Arc,
    time::Instant,
};

use axum::{
    Router,
    routing::{get, post},
};
use tokio::{net::TcpListener, sync::oneshot, task::JoinHandle};
use tracing_subscriber::EnvFilter;

use crate::{
    config::Config,
    proxy::{self, AppState},
};

/// Errors returned when starting the embedded proxy server.
#[derive(Debug)]
pub enum ServerStartError {
    /// The configuration could not be turned into a usable runtime state.
    InvalidConfig(String),
    /// Binding the TCP listener failed.
    Bind {
        addr: SocketAddr,
        source: std::io::Error,
    },
}

impl std::fmt::Display for ServerStartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidConfig(msg) => write!(f, "invalid runtime configuration: {msg}"),
            Self::Bind { addr, source } => write!(f, "failed to bind {addr}: {source}"),
        }
    }
}

impl std::error::Error for ServerStartError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidConfig(_) => None,
            Self::Bind { source, .. } => Some(source),
        }
    }
}

/// Information returned after a successful server start.
#[derive(Debug, Clone)]
pub struct ServerStartInfo {
    /// Local socket address actually bound (port 0 resolves to an OS-chosen port).
    pub local_addr: SocketAddr,
    /// Wall-clock instant the server began accepting connections.
    pub started_at: Instant,
}

/// A controllable, in-process proxy server.
///
/// Drop the handle to abort the server task without graceful shutdown, or
/// call [`ServerHandle::shutdown`] for a graceful stop that waits for the
/// accept loop to exit.
pub struct ServerHandle {
    info: ServerStartInfo,
    state: Arc<AppState>,
    shutdown_tx: Option<oneshot::Sender<()>>,
    join: Option<JoinHandle<()>>,
}

impl ServerHandle {
    /// Returns metadata captured at start time.
    pub fn info(&self) -> &ServerStartInfo {
        &self.info
    }

    /// Returns the bound local address.
    pub fn local_addr(&self) -> SocketAddr {
        self.info.local_addr
    }

    /// Returns the start instant.
    pub fn started_at(&self) -> Instant {
        self.info.started_at
    }

    /// Cloneable handle to the running server's [`AppState`].
    ///
    /// Used by the desktop wrapper to read live traffic statistics without
    /// going through the HTTP surface.
    pub fn state(&self) -> &Arc<AppState> {
        &self.state
    }

    /// Sends the shutdown signal and waits for the server task to exit.
    ///
    /// Calling this more than once is a no-op after the first invocation.
    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(join) = self.join.take() {
            let _ = join.await;
        }
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(join) = self.join.take() {
            join.abort();
        }
    }
}

/// Build the axum router for a shared [`AppState`].
///
/// Exposed primarily for tests; production code typically calls [`serve`].
pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/models", get(proxy::models))
        .route("/v1/chat/completions", post(proxy::chat_completions))
        .route("/v1/responses", post(proxy::responses))
        .route("/v1/messages", post(proxy::messages))
        .with_state(state)
}

/// Start the embedded proxy server bound to `addr`.
///
/// The returned [`ServerHandle`] keeps the server task alive; use
/// [`ServerHandle::shutdown`] to stop gracefully.
pub async fn serve(addr: SocketAddr, config: Config) -> Result<ServerHandle, ServerStartError> {
    let state = Arc::new(AppState::new(config));
    let app = build_router(state.clone());

    let listener = TcpListener::bind(addr)
        .await
        .map_err(|source| ServerStartError::Bind { addr, source })?;
    let local_addr = listener
        .local_addr()
        .map_err(|source| ServerStartError::Bind { addr, source })?;

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let started_at = Instant::now();

    let join = tokio::spawn(async move {
        let server = axum::serve(listener, app).with_graceful_shutdown(async move {
            let _ = shutdown_rx.await;
        });
        if let Err(err) = server.await {
            tracing::error!("open-promux server stopped with error: {err}");
        }
    });

    Ok(ServerHandle {
        info: ServerStartInfo {
            local_addr,
            started_at,
        },
        state,
        shutdown_tx: Some(shutdown_tx),
        join: Some(join),
    })
}

/// CLI entrypoint reused by `src/main.rs`.
///
/// Initialises the default tracing subscriber, loads the config from the
/// first positional argument (or `config.toml` by default), and runs the
/// server until the process is terminated.
pub async fn run_cli() {
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
    let config = Config::load(&config_path);
    let port = config.port;

    let addr = SocketAddr::new(IpAddr::from([0, 0, 0, 0]), port);
    tracing::info!("open-promux listening on {addr}");

    let handle = match serve(addr, config).await {
        Ok(handle) => handle,
        Err(err) => {
            tracing::error!("failed to start open-promux: {err}");
            std::process::exit(1);
        }
    };

    if let Some(join) = handle_into_join(handle) {
        let _ = join.await;
    }
}

/// Consume the handle without sending the shutdown signal so the CLI keeps
/// running until the process is killed.
fn handle_into_join(mut handle: ServerHandle) -> Option<JoinHandle<()>> {
    handle.shutdown_tx = None;
    handle.join.take()
}
