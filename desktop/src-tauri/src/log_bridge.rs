//! Streams [`open_promux::LogBus`] broadcast messages into the Tauri event bus.
//!
//! The renderer subscribes to `log://line` to receive new lines and
//! `log://reset` when the log buffer is cleared. New subscribers should
//! also fetch `get_logs_snapshot` once on mount to backfill the history.

use open_promux::LogBus;
use tauri::{AppHandle, Emitter, async_runtime};
use tokio::sync::broadcast::error::RecvError;

/// Tauri event name for incremental log lines.
pub const EVENT_LOG_LINE: &str = "log://line";

/// Tauri event name fired when [`LogBus::clear`] is invoked.
#[allow(dead_code)] // reserved for future "log clear" emit usage
pub const EVENT_LOG_RESET: &str = "log://reset";

/// Spawn a background task that forwards every broadcast message to the
/// frontend. The task exits cleanly when the app handle is dropped.
pub fn spawn(app: AppHandle, log_bus: LogBus) {
    let mut rx = log_bus.subscribe();
    // `tauri::async_runtime::spawn` works from inside `setup` hooks where
    // `tokio::spawn` would panic with "no reactor running".
    async_runtime::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(line) => {
                    if let Err(err) = app.emit(EVENT_LOG_LINE, line) {
                        tracing::debug!("log bridge emit failed: {err}");
                    }
                }
                Err(RecvError::Lagged(skipped)) => {
                    tracing::warn!("log bridge dropped {skipped} log lines (renderer too slow)");
                }
                Err(RecvError::Closed) => break,
            }
        }
    });
}
